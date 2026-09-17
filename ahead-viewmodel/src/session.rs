//! Session/chat/proposal/tracker pure transitions.
//!
//! Ported 1:1 from the logic inside `lapce-app/src/ahead/state.rs`
//! (`AheadState` methods), minus the Floem signals. The Floem shell keeps
//! working by delegating to these functions; the GPUI shell will call the
//! same functions from entities.

use lapce_rpc::ahead::{ChangeProposal, SessionView, VoiceTranscriptUpdate};

/// Minimal chat message (mirrors `AgentChatMessage` without timestamps-as-logic).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub id: String,
    pub sender: String,
    pub text: String,
    pub is_challenge: bool,
}

/// Agent/system message appended to the pairing feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMessage {
    pub sender: String,
    pub text: String,
    pub is_challenge: bool,
}

/// Tracker outbox entry (mirrors `TrackerOutboxEntry`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackerEntry {
    pub id: String,
    pub target: String,
    pub description: String,
    pub status: String,
}

/// Proposal list entry summarized for view decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalSummary {
    pub id: String,
}

/// Full per-session snapshot a shell holds in its own reactive state.
#[derive(Debug, Clone, Default)]
pub struct SessionSnapshot {
    pub active: Option<SessionView>,
    pub saved: Vec<SessionView>,
    pub chat: Vec<ChatMessage>,
    pub pending_proposals: Vec<ChangeProposal>,
    pub tracker: Vec<TrackerEntry>,
    pub transcripts: Vec<VoiceTranscriptUpdate>,
}

fn next_id(prefix: &str, len: usize) -> String {
    format!("{prefix}-{}", len + 1)
}

/// Validates wizard input before Begin. Returns the error text the UI shows.
pub fn begin_validation_error(title: &str, resume_id: Option<&str>) -> Option<String> {
    if resume_id.is_some() {
        return None;
    }
    if title.trim().is_empty() {
        return Some("Give the work a title before beginning.".to_string());
    }
    None
}

/// Adopts a durable session: sets active, clears per-session scaffolds so a
/// reopened session never shows another session's chat/proposals/transcripts.
pub fn adopt_session(snapshot: &mut SessionSnapshot, view: SessionView) {
    snapshot.active = Some(view);
    snapshot.chat.clear();
    snapshot.pending_proposals.clear();
    snapshot.tracker.clear();
    snapshot.transcripts.clear();
}

/// Applies a host-acknowledged mode change.
pub fn apply_mode(snapshot: &mut SessionSnapshot, view: SessionView) {
    snapshot.active = Some(view);
}

/// Applies a host-acknowledged workflow state.
pub fn apply_workflow(snapshot: &mut SessionSnapshot, session_id: &str, view: SessionView) {
    debug_assert_eq!(view.session.id, session_id);
    snapshot.active = Some(view);
}

/// Records a human-authored chat message. Empty/blank text is dropped.
/// Returns true when a message was recorded.
pub fn push_human_message(snapshot: &mut SessionSnapshot, text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    let id = next_id("msg", snapshot.chat.len());
    snapshot.chat.push(ChatMessage {
        id,
        sender: "Human".to_string(),
        text: text.to_string(),
        is_challenge: false,
    });
    true
}

/// Appends an agent/system message to the pairing feed.
pub fn push_agent_message(snapshot: &mut SessionSnapshot, msg: AgentMessage) {
    let id = next_id("msg", snapshot.chat.len());
    snapshot.chat.push(ChatMessage {
        id,
        sender: msg.sender,
        text: msg.text,
        is_challenge: msg.is_challenge,
    });
}

/// Drops a proposal from the pending list. Returns true when one was removed.
pub fn drop_proposal(snapshot: &mut SessionSnapshot, id: &str) -> bool {
    let before = snapshot.pending_proposals.len();
    snapshot.pending_proposals.retain(|p| p.id != id);
    snapshot.pending_proposals.len() != before
}

/// Marks a tracker entry dispatched. Local-only until GitHub publish lands.
pub fn queue_tracker_dispatch(snapshot: &mut SessionSnapshot, id: &str) -> bool {
    for item in snapshot.tracker.iter_mut() {
        if item.id == id {
            item.status = "Queued locally (GitHub publish not wired yet)".to_string();
            return true;
        }
    }
    false
}

/// Next workflow phase for the given current phase id.
pub fn next_phase(current: &str) -> (&'static str, &'static str) {
    match current {
        "plan" | "hypothesize" => ("invariants", "Invariants"),
        "invariants" => ("implement", "Implementation"),
        "implement" => ("verify", "Verification"),
        "verify" => ("review", "Review & Signoff"),
        "review" => ("complete", "Completed"),
        _ => ("plan", "Planning"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_chat_is_dropped() {
        let mut s = SessionSnapshot::default();
        assert!(!push_human_message(&mut s, "   "));
        assert!(s.chat.is_empty());
    }

    #[test]
    fn human_then_agent_ids_sequence() {
        let mut s = SessionSnapshot::default();
        assert!(push_human_message(&mut s, "hello"));
        push_agent_message(
            &mut s,
            AgentMessage { sender: "AHEAD Agent".into(), text: "hi".into(), is_challenge: false },
        );
        assert_eq!(s.chat[0].id, "msg-1");
        assert_eq!(s.chat[1].id, "msg-2");
    }

    #[test]
    fn adopt_clears_scaffolds() {
        let mut s = SessionSnapshot::default();
        push_human_message(&mut s, "x");
        s.pending_proposals.push(ChangeProposal {
            id: "p".into(),
            session_id: "s".into(),
            path: "a".into(),
            original_sha256: "h".into(),
            patch: "p".into(),
            is_mechanical: true,
            description: "d".into(),
            recommended_cursor: None,
        });
        let view = SessionView {
            session: lapce_rpc::ahead::WorkSession {
                id: "sess".into(),
                project_id: "p".into(),
                worktree_id: "w".into(),
                work_kind: lapce_rpc::ahead::WorkKind::ProductChange,
                mode: lapce_rpc::ahead::AssistanceMode::Assist,
                title: "t".into(),
                owner_id: "o".into(),
                lifecycle: lapce_rpc::ahead::SessionLifecycle::Active,
                policy: lapce_rpc::ahead::SessionPolicySnapshot::default(),
                revision: 1,
                created_at: "now".into(),
            },
            workflow: lapce_rpc::ahead::WorkflowState {
                revision: 1,
                definition_version: "v".into(),
                phase: lapce_rpc::ahead::WorkflowPhase { id: "plan".into(), title: "Planning".into(), visit: 1 },
                primary_work_item: None,
                current_artifact_ids: Vec::new(),
                approvals: Vec::new(),
            },
            participants: Vec::new(),
        };
        adopt_session(&mut s, view);
        assert!(s.chat.is_empty());
        assert!(s.pending_proposals.is_empty());
        assert!(s.active.is_some());
    }

    #[test]
    fn wizard_validation() {
        assert!(begin_validation_error("", None).is_some());
        assert!(begin_validation_error("  ", None).is_some());
        assert!(begin_validation_error("", Some("resume-id")).is_none());
        assert!(begin_validation_error("Title", None).is_none());
    }

    #[test]
    fn phase_map() {
        assert_eq!(next_phase("plan"), ("invariants", "Invariants"));
        assert_eq!(next_phase("implement"), ("verify", "Verification"));
        assert_eq!(next_phase("review"), ("complete", "Completed"));
        assert_eq!(next_phase("bogus"), ("plan", "Planning"));
    }

    #[test]
    fn drop_and_dispatch() {
        let mut s = SessionSnapshot::default();
        assert!(!drop_proposal(&mut s, "nope"));
        s.tracker.push(TrackerEntry { id: "t".into(), target: "g".into(), description: "d".into(), status: "pending".into() });
        assert!(queue_tracker_dispatch(&mut s, "t"));
        assert!(s.tracker[0].status.contains("Queued locally"));
        assert!(!queue_tracker_dispatch(&mut s, "missing"));
    }
}
