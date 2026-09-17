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
use lapce_rpc::ahead::{GithubIssueRef, Id, Timestamp};

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

pub struct TrackerAdapter {
    outbox: HashMap<Id, TrackerOutboxItem>,
    remote_mock_state: HashMap<String, (String, String)>, // key -> (body, sha)
}

impl TrackerAdapter {
    pub fn new() -> Self {
        Self {
            outbox: HashMap::new(),
            remote_mock_state: HashMap::new(),
        }
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
}

use sha2::Digest;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracker_conflict_detection_prevents_silent_overwrite() -> Result<()> {
        let mut tracker = TrackerAdapter::new();
        let issue = GithubIssueRef {
            host: "github.com".into(),
            owner: "ahead-editor".into(),
            repo: "ahead".into(),
            issue_number: 42,
            issue_node_id: "I_kwDOtest42".into(),
            title: "Support live voice".into(),
        };

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
}
