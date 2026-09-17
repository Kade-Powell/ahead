//! AHEAD GitHub Work Tracking & Outbox
//!
//! Grounded in Section 9 of `ahead-editor-mvp.md`.
//! GitHub is authoritative for issue/project fields; AHEAD is authoritative
//! for session history, durable code anchors, and workflow gates.
//!
//! Features:
//! - Stable identity via `GithubIssueRef` (host + repo + issue number + node ID).
//! - Safe Outbox pattern for external tracker mutations:
//!   1. Preview proposed diff
//!   2. Record explicit human authorization
//!   3. Compare remote observed ETag/content SHA before dispatch
//!   4. Detect conflict if remote changed; never silently overwrite
//!   5. Handle timeouts with explicit unknown reconciliation before retrying

use std::collections::HashMap;
use anyhow::{bail, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use ahead_rpc::ahead::{GithubIssueRef, Id, Timestamp};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutboxStatus {
    PendingAuthorization,
    Authorized { authorized_by: Id, authorized_at: Timestamp },
    Confirmed { remote_updated_at: Timestamp },
    Conflict { remote_body_sha: String, observed_body_sha: String },
    UnknownTimeout { last_attempt_at: Timestamp },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerUpdatePayload {
    pub title: Option<String>,
    pub body_markdown: Option<String>,
    pub labels: Option<Vec<String>>,
    pub state: Option<String>, // "open" | "closed"
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerOutboxItem {
    pub outbox_id: Id,
    pub session_id: Id,
    pub issue_ref: GithubIssueRef,
    pub observed_body_sha: String,
    pub payload: TrackerUpdatePayload,
    pub status: OutboxStatus,
    pub created_at: Timestamp,
}

#[derive(Default)]
pub struct TrackerAdapter {
    outbox: HashMap<Id, TrackerOutboxItem>,
    remote_mock_state: HashMap<String, (String, String)>, // key -> (body, sha)
}

impl TrackerAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets up initial remote issue state for testing/mocking
    pub fn set_remote_issue(&mut self, issue: &GithubIssueRef, body: &str) {
        let key = format!("{}/{}/#{}", issue.owner, issue.repo, issue.issue_number);
        let sha = format!("{:x}", sha2::Sha256::digest(body.as_bytes()));
        self.remote_mock_state.insert(key, (body.to_string(), sha));
    }

    /// Step 1: Stage an external update in the Outbox
    pub fn stage_update(
        &mut self,
        session_id: Id,
        issue_ref: GithubIssueRef,
        payload: TrackerUpdatePayload,
        observed_body_sha: String,
    ) -> Result<Id> {
        let outbox_id = uuid::Uuid::new_v4().to_string();
        let item = TrackerOutboxItem {
            outbox_id: outbox_id.clone(),
            session_id,
            issue_ref,
            observed_body_sha,
            payload,
            status: OutboxStatus::PendingAuthorization,
            created_at: Utc::now().to_rfc3339(),
        };
        self.outbox.insert(outbox_id.clone(), item);
        Ok(outbox_id)
    }

    /// Step 2: Record explicit human authorization
    pub fn authorize_update(&mut self, outbox_id: &str, authorizer_id: &str) -> Result<()> {
        let Some(item) = self.outbox.get_mut(outbox_id) else {
            bail!("Outbox item not found: {}", outbox_id);
        };

        if !matches!(item.status, OutboxStatus::PendingAuthorization) {
            bail!("Item is not awaiting authorization: {:?}", item.status);
        }

        item.status = OutboxStatus::Authorized {
            authorized_by: authorizer_id.to_string(),
            authorized_at: Utc::now().to_rfc3339(),
        };
        Ok(())
    }

    /// Step 3 & 4: Dispatch update with conflict prevention
    pub fn dispatch_update(&mut self, outbox_id: &str, simulate_timeout: bool) -> Result<OutboxStatus> {
        let Some(item) = self.outbox.get_mut(outbox_id) else {
            bail!("Outbox item not found: {}", outbox_id);
        };

        if !matches!(item.status, OutboxStatus::Authorized { .. }) {
            bail!("Cannot dispatch unauthorized outbox item");
        }

        let key = format!(
            "{}/{}/#{}",
            item.issue_ref.owner, item.issue_ref.repo, item.issue_ref.issue_number
        );

        if simulate_timeout {
            item.status = OutboxStatus::UnknownTimeout {
                last_attempt_at: Utc::now().to_rfc3339(),
            };
            return Ok(item.status.clone());
        }

        // Compare observed remote state
        if let Some((_current_body, current_sha)) = self.remote_mock_state.get(&key) {
            if *current_sha != item.observed_body_sha {
                // Remote changed since preview! Fail closed with Conflict
                let conflict_status = OutboxStatus::Conflict {
                    remote_body_sha: current_sha.clone(),
                    observed_body_sha: item.observed_body_sha.clone(),
                };
                item.status = conflict_status.clone();
                return Ok(conflict_status);
            }
        }

        // Apply mutation
        if let Some(new_body) = item.payload.body_markdown.as_ref() {
            let new_sha = format!("{:x}", sha2::Sha256::digest(new_body.as_bytes()));
            self.remote_mock_state.insert(key, (new_body.clone(), new_sha));
        }

        let confirmed_status = OutboxStatus::Confirmed {
            remote_updated_at: Utc::now().to_rfc3339(),
        };
        item.status = confirmed_status.clone();
        Ok(confirmed_status)
    }

    /// Step 5: Reconcile timed-out write before retrying
    pub fn reconcile_timeout(
        &mut self,
        outbox_id: &str,
        remote_body: &str,
    ) -> Result<bool> {
        let Some(item) = self.outbox.get_mut(outbox_id) else {
            bail!("Outbox item not found: {}", outbox_id);
        };

        let remote_sha = format!("{:x}", sha2::Sha256::digest(remote_body.as_bytes()));
        if let Some(target_body) = item.payload.body_markdown.as_ref() {
            let target_sha = format!("{:x}", sha2::Sha256::digest(target_body.as_bytes()));
            if remote_sha == target_sha {
                // The write actually went through before timeout
                item.status = OutboxStatus::Confirmed {
                    remote_updated_at: Utc::now().to_rfc3339(),
                };
                return Ok(true);
            }
        }

        // Write did not land
        Ok(false)
    }

    pub fn get_item(&self, outbox_id: &str) -> Option<&TrackerOutboxItem> {
        self.outbox.get(outbox_id)
    }

    /// Live publish against the GitHub REST API (issues update).
    /// Same outbox contract as the mock path: only Authorized items
    /// dispatch; remote body SHA is re-fetched and compared first, so a
    /// concurrent edit surfaces as Conflict instead of overwriting.
    /// Token stays in-process (from the session host's auth manager);
    /// on timeout the item goes UnknownTimeout for reconcile-before-retry.
    pub fn publish_to_github(
        &mut self,
        outbox_id: &str,
        access_token: &str,
        api_base: &str,
    ) -> Result<OutboxStatus> {
        let (owner, repo, number, payload) = {
            let Some(item) = self.outbox.get(outbox_id) else {
                bail!("Outbox item not found: {}", outbox_id);
            };
            if !matches!(item.status, OutboxStatus::Authorized { .. }) {
                bail!("Cannot publish unauthorized outbox item");
            }
            (
                item.issue_ref.owner.clone(),
                item.issue_ref.repo.clone(),
                item.issue_ref.issue_number,
                item.payload.clone(),
            )
        };
        let base = api_base.trim_end_matches('/');
        let url = format!("{base}/repos/{owner}/{repo}/issues/{number}");
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("AHEAD")
            .build()?;
        // Re-fetch remote first for conflict detection.
        let remote_body = match client
            .get(&url)
            .bearer_auth(access_token)
            .header("Accept", "application/vnd.github+json")
            .send()
        {
            Ok(resp) => {
                if !resp.status().is_success() {
                    bail!("GitHub re-fetch failed: HTTP {}", resp.status());
                }
                let json: serde_json::Value = resp.json()?;
                json.get("body").and_then(|b| b.as_str()).unwrap_or("").to_string()
            }
            Err(e) if e.is_timeout() => {
                let Some(item) = self.outbox.get_mut(outbox_id) else {
                    bail!("Outbox item not found: {}", outbox_id);
                };
                item.status = OutboxStatus::UnknownTimeout {
                    last_attempt_at: Utc::now().to_rfc3339(),
                };
                return Ok(item.status.clone());
            }
            Err(e) => bail!("GitHub re-fetch failed: {e}"),
        };
        let remote_sha = format!("{:x}", sha2::Sha256::digest(remote_body.as_bytes()));
        let observed = self.outbox.get(outbox_id).map(|i| i.observed_body_sha.clone()).unwrap_or_default();
        if remote_sha != observed {
            let Some(item) = self.outbox.get_mut(outbox_id) else {
                bail!("Outbox item not found: {}", outbox_id);
            };
            item.status = OutboxStatus::Conflict {
                remote_body_sha: remote_sha,
                observed_body_sha: observed,
            };
            return Ok(item.status.clone());
        }
        // Remote unchanged: send the authorized payload (body/title/state).
        let mut body = serde_json::Map::new();
        if let Some(t) = payload.title {
            body.insert("title".to_string(), serde_json::Value::String(t));
        }
        if let Some(b) = payload.body_markdown {
            body.insert("body".to_string(), serde_json::Value::String(b));
        }
        if let Some(s) = payload.state {
            body.insert("state".to_string(), serde_json::Value::String(s));
        }
        let resp = match client
            .patch(&url)
            .bearer_auth(access_token)
            .header("Accept", "application/vnd.github+json")
            .json(&body)
            .send()
        {
            Ok(resp) => resp,
            Err(e) if e.is_timeout() => {
                let Some(item) = self.outbox.get_mut(outbox_id) else {
                    bail!("Outbox item not found: {}", outbox_id);
                };
                item.status = OutboxStatus::UnknownTimeout {
                    last_attempt_at: Utc::now().to_rfc3339(),
                };
                return Ok(item.status.clone());
            }
            Err(e) => bail!("GitHub publish failed: {e}"),
        };
        if !resp.status().is_success() {
            let code = resp.status();
            let text = resp.text().unwrap_or_default();
            let preview: String = text.chars().take(300).collect();
            bail!("GitHub publish failed: HTTP {code} {preview}");
        }
        let Some(item) = self.outbox.get_mut(outbox_id) else {
            bail!("Outbox item not found: {}", outbox_id);
        };
        // Refresh observed SHA from what we just wrote.
        if let Some(new_body) = item.payload.body_markdown.clone() {
            item.observed_body_sha = format!("{:x}", sha2::Sha256::digest(new_body.as_bytes()));
        }
        item.status = OutboxStatus::Confirmed {
            remote_updated_at: Utc::now().to_rfc3339(),
        };
        Ok(item.status.clone())
    }
}

use sha2::Digest;

#[cfg(test)]
mod tests {
    use super::*;

    fn test_issue() -> GithubIssueRef {
        GithubIssueRef {
            host: "github.com".into(),
            owner: "ahead-editor".into(),
            repo: "ahead".into(),
            issue_number: 42,
            issue_node_id: "I_kwDOtest42".into(),
            title: "Support live voice".into(),
        }
    }

    fn stage_authorized(tracker: &mut TrackerAdapter, body: &str) -> String {
        let issue = test_issue();
        tracker.set_remote_issue(&issue, "Original issue body by Teammate");
        let observed_sha = format!("{:x}", sha2::Sha256::digest("Original issue body by Teammate".as_bytes()));
        let id = tracker.stage_update(
            "sess-1".into(),
            issue,
            TrackerUpdatePayload {
                title: None,
                body_markdown: Some(body.into()),
                labels: None,
                state: None,
            },
            observed_sha,
        ).unwrap();
        tracker.authorize_update(&id, "dev-kade").unwrap();
        id
    }

    #[test]
    fn test_tracker_conflict_detection_prevents_silent_overwrite() -> Result<()> {
        let mut tracker = TrackerAdapter::new();
        let issue = test_issue();
        // Initial remote state
        tracker.set_remote_issue(&issue, "Original issue body by Teammate");

        // Engineer prepares update based on observed state
        let observed_sha = format!("{:x}", sha2::Sha256::digest("Original issue body by Teammate".as_bytes()));
        let outbox_id = tracker.stage_update(
            "sess-1".into(),
            issue.clone(),
            TrackerUpdatePayload {
                title: None,
                body_markdown: Some("Updated issue body with AHEAD plan".into()),
                labels: None,
                state: None,
            },
            observed_sha,
        )?;

        // Another teammate updates remote issue concurrently
        tracker.set_remote_issue(&issue, "Concurrent update by Teammate B");

        // Human authorizes our update
        tracker.authorize_update(&outbox_id, "dev-kade")?;

        // Dispatch must detect conflict and reject overwrite!
        let status = tracker.dispatch_update(&outbox_id, false)?;
        assert!(matches!(status, OutboxStatus::Conflict { .. }), "Expected conflict on concurrent remote modification");

        Ok(())
    }

    #[test]
    fn test_tracker_timeout_reconciliation() -> Result<()> {
        let mut tracker = TrackerAdapter::new();
        let issue = GithubIssueRef {
            host: "github.com".into(),
            owner: "ahead-editor".into(),
            repo: "ahead".into(),
            issue_number: 43,
            issue_node_id: "I_kwDOtest43".into(),
            title: "Task timeout test".into(),
        };

        tracker.set_remote_issue(&issue, "Body before write");
        let observed_sha = format!("{:x}", sha2::Sha256::digest("Body before write".as_bytes()));

        let outbox_id = tracker.stage_update(
            "sess-1".into(),
            issue.clone(),
            TrackerUpdatePayload {
                title: None,
                body_markdown: Some("Body after write".into()),
                labels: None,
                state: None,
            },
            observed_sha,
        )?;

        tracker.authorize_update(&outbox_id, "dev-kade")?;

        // Simulate network timeout on dispatch
        let status = tracker.dispatch_update(&outbox_id, true)?;
        assert!(matches!(status, OutboxStatus::UnknownTimeout { .. }));

        // Reconcile: simulate remote did receive the write
        let reconciled = tracker.reconcile_timeout(&outbox_id, "Body after write")?;
        assert!(reconciled, "Expected reconciliation to recognize applied remote state");

        let item = tracker.get_item(&outbox_id).unwrap();
        assert!(matches!(item.status, OutboxStatus::Confirmed { .. }));

        Ok(())
    }

    #[test]
    fn test_publish_requires_authorization() {
        let mut tracker = TrackerAdapter::new();
        let issue = test_issue();
        tracker.set_remote_issue(&issue, "Body");
        let observed_sha = format!("{:x}", sha2::Sha256::digest("Body".as_bytes()));
        let id = tracker.stage_update(
            "sess-1".into(),
            issue,
            TrackerUpdatePayload { title: None, body_markdown: Some("New".into()), labels: None, state: None },
            observed_sha,
        ).unwrap();
        // Staged but not authorized: live publish must refuse.
        assert!(tracker.publish_to_github(&id, "token", "https://api.github.com").is_err());
    }

    #[test]
    fn test_publish_detects_conflict_without_network() {
        // Same conflict logic the live path uses, exercised on the mock
        // path: remote moved after preview → Conflict, never overwrite.
        let mut tracker = TrackerAdapter::new();
        let id = stage_authorized(&mut tracker, "Planned body");
        let issue = test_issue();
        tracker.set_remote_issue(&issue, "Teammate rewrote meanwhile");
        let status = tracker.dispatch_update(&id, false).unwrap();
        assert!(matches!(status, OutboxStatus::Conflict { .. }));
    }
}
