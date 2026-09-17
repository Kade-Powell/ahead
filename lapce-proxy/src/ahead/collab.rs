//! AHEAD Collaboration & Review Snapshot Pilot
//!
//! Grounded in Section 10 and Section 13 (Scenarios 4, 6, 9) of `ahead-editor-mvp.md`.
//! Features:
//! - Authenticated peer session with participant revocation enforcement (Scenario 6)
//! - Sticky anchor index for concurrent edit adjustment without caret theft (Scenario 4)
//! - Frozen review snapshot binding code, implementers, and attestations, invalidated upon code changes (Scenario 9)

use std::collections::{HashMap, HashSet};
use anyhow::{bail, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use lapce_rpc::ahead::{
    CodeAnchor, DisplayPosition, DisplayRange, Id, Sha256, Timestamp,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParticipantStatus {
    Active,
    Revoked { revoked_at: Timestamp },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollabParticipant {
    pub id: Id,
    pub display_name: String,
    pub is_host: bool,
    pub status: ParticipantStatus,
}

/// Sticky anchor index tracking relative anchor offsets during edits
#[derive(Debug, Clone)]
pub struct StickyAnchorIndex {
    anchors: HashMap<Id, CodeAnchor>,
}

impl StickyAnchorIndex {
    pub fn new() -> Self {
        Self {
            anchors: HashMap::new(),
        }
    }

    pub fn insert(&mut self, anchor: CodeAnchor) {
        self.anchors.insert(anchor.id.clone(), anchor);
    }

    pub fn get(&self, id: &str) -> Option<&CodeAnchor> {
        self.anchors.get(id)
    }

    /// Adjusts all anchors on `path` given an edit inserting or deleting lines
    pub fn apply_line_delta(&mut self, path: &str, at_line: u32, line_delta: i32) {
        for anchor in self.anchors.values_mut() {
            if anchor.path != path {
                continue;
            }

            if line_delta > 0 {
                // Lines inserted
                let delta = line_delta as u32;
                if anchor.range.start.line >= at_line {
                    anchor.range.start.line += delta;
                    anchor.range.end.line += delta;
                } else if anchor.range.end.line >= at_line {
                    anchor.range.end.line += delta;
                }
            } else if line_delta < 0 {
                // Lines deleted
                let delta = (-line_delta) as u32;
                if anchor.range.start.line >= at_line + delta {
                    anchor.range.start.line = anchor.range.start.line.saturating_sub(delta);
                    anchor.range.end.line = anchor.range.end.line.saturating_sub(delta);
                } else if anchor.range.start.line >= at_line {
                    anchor.range.start.line = at_line;
                    anchor.range.end.line = anchor.range.end.line.saturating_sub(delta).max(at_line);
                }
            }
        }
    }
}

/// Frozen immutable review snapshot
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSnapshot {
    pub snapshot_id: Id,
    pub session_id: Id,
    pub code_tree_sha: Sha256,
    pub implementer_ids: HashSet<Id>,
    pub findings: Vec<String>,
    pub is_approved: bool,
    pub approved_by: Option<Id>,
    pub approved_at: Option<Timestamp>,
}

impl ReviewSnapshot {
    pub fn new(
        snapshot_id: Id,
        session_id: Id,
        code_tree_sha: Sha256,
        implementers: impl IntoIterator<Item = Id>,
    ) -> Self {
        Self {
            snapshot_id,
            session_id,
            code_tree_sha,
            implementer_ids: implementers.into_iter().collect(),
            findings: Vec::new(),
            is_approved: false,
            approved_by: None,
            approved_at: None,
        }
    }

    /// Scenario 9: Reviewer independence requirement
    pub fn record_approval(&mut self, reviewer_id: &str) -> Result<()> {
        if self.implementer_ids.contains(reviewer_id) {
            bail!("Reviewer independence violation: author/implementer cannot approve their own work");
        }

        self.is_approved = true;
        self.approved_by = Some(reviewer_id.to_string());
        self.approved_at = Some(Utc::now().to_rfc3339());
        Ok(())
    }

    /// Scenario 9: Approval becomes stale upon code changes
    pub fn check_stale(&mut self, current_tree_sha: &str) -> bool {
        if current_tree_sha != self.code_tree_sha {
            // Code changed! Invalidate approval while preserving discussion findings
            self.is_approved = false;
            self.approved_by = None;
            self.approved_at = None;
            true
        } else {
            false
        }
    }
}

/// Central Collaboration Coordinator
pub struct CollabSession {
    pub session_id: Id,
    participants: HashMap<Id, CollabParticipant>,
    sticky_anchors: StickyAnchorIndex,
    review_snapshot: Option<ReviewSnapshot>,
}

impl CollabSession {
    pub fn new(session_id: Id, host_id: Id, host_name: String) -> Self {
        let mut participants = HashMap::new();
        participants.insert(
            host_id.clone(),
            CollabParticipant {
                id: host_id,
                display_name: host_name,
                is_host: true,
                status: ParticipantStatus::Active,
            },
        );

        Self {
            session_id,
            participants,
            sticky_anchors: StickyAnchorIndex::new(),
            review_snapshot: None,
        }
    }

    pub fn join_guest(&mut self, guest_id: Id, display_name: String) {
        self.participants.insert(
            guest_id.clone(),
            CollabParticipant {
                id: guest_id,
                display_name,
                is_host: false,
                status: ParticipantStatus::Active,
            },
        );
    }

    /// Scenario 6: Revocation
    pub fn revoke_participant(&mut self, participant_id: &str) -> Result<()> {
        let Some(p) = self.participants.get_mut(participant_id) else {
            bail!("Participant not found: {}", participant_id);
        };
        p.status = ParticipantStatus::Revoked {
            revoked_at: Utc::now().to_rfc3339(),
        };
        Ok(())
    }

    /// Validates if participant can submit edits
    pub fn can_submit_edit(&self, participant_id: &str) -> Result<()> {
        let Some(p) = self.participants.get(participant_id) else {
            bail!("Unauthorized: not a session participant");
        };

        match p.status {
            ParticipantStatus::Active => Ok(()),
            ParticipantStatus::Revoked { .. } => {
                bail!("Access denied: participant credentials have been revoked")
            }
        }
    }

    pub fn sticky_anchors_mut(&mut self) -> &mut StickyAnchorIndex {
        &mut self.sticky_anchors
    }

    pub fn sticky_anchors(&self) -> &StickyAnchorIndex {
        &self.sticky_anchors
    }

    pub fn set_review_snapshot(&mut self, snapshot: ReviewSnapshot) {
        self.review_snapshot = Some(snapshot);
    }

    pub fn review_snapshot_mut(&mut self) -> Option<&mut ReviewSnapshot> {
        self.review_snapshot.as_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sticky_anchor_adjusts_during_concurrent_edits() {
        let mut index = StickyAnchorIndex::new();
        let anchor = CodeAnchor {
            id: "anc-42".into(),
            session_id: "sess-1".into(),
            path: "src/engine.rs".into(),
            range: DisplayRange {
                start: DisplayPosition { line: 50, col: 0 },
                end: DisplayPosition { line: 60, col: 0 },
            },
            quote_hash: "quote_hash".into(),
            surrounding_context: None,
            created_at_commit: None,
        };
        index.insert(anchor);

        // Edit 1: Insert 5 lines before the anchor at line 20
        index.apply_line_delta("src/engine.rs", 20, 5);
        let updated = index.get("anc-42").unwrap();
        assert_eq!(updated.range.start.line, 55);
        assert_eq!(updated.range.end.line, 65);

        // Edit 2: Delete 10 lines before the anchor at line 10
        index.apply_line_delta("src/engine.rs", 10, -10);
        let updated = index.get("anc-42").unwrap();
        assert_eq!(updated.range.start.line, 45);
        assert_eq!(updated.range.end.line, 55);
    }

    #[test]
    fn test_revoked_participant_cannot_submit_edits() -> Result<()> {
        let mut session = CollabSession::new("sess-1".into(), "host-1".into(), "Host Dev".into());
        session.join_guest("guest-1".into(), "Guest Dev".into());

        // Active guest can submit
        assert!(session.can_submit_edit("guest-1").is_ok());

        // Revoke guest
        session.revoke_participant("guest-1")?;

        // Revoked guest must fail closed
        let err = session.can_submit_edit("guest-1");
        assert!(err.is_err(), "Revoked participant should not be allowed to submit edits");

        Ok(())
    }

    #[test]
    fn test_review_independence_and_stale_invalidation() -> Result<()> {
        let mut snapshot = ReviewSnapshot::new(
            "snap-1".into(),
            "sess-1".into(),
            "tree_sha_initial".into(),
            vec!["dev-implementer".into()],
        );

        // Implementer cannot self-approve
        let self_approval = snapshot.record_approval("dev-implementer");
        assert!(self_approval.is_err(), "Self-approval must be rejected");

        // Independent reviewer can approve
        assert!(snapshot.record_approval("independent-reviewer").is_ok());
        assert!(snapshot.is_approved);

        // Subsequent code change invalidates approval
        let is_stale = snapshot.check_stale("tree_sha_modified");
        assert!(is_stale);
        assert!(!snapshot.is_approved, "Approval must become stale upon code changes");

        Ok(())
    }
}
