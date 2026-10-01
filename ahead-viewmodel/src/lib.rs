//! AHEAD framework-agnostic session viewmodel.
//!
//! Pure logic shared by the GPUI shell and host services. State is plain Rust
//! owned by the caller; each method is a total function over its inputs.
//!
//! Covered today: wizard validation, session adoption
//! hygiene, chat/proposal/tracker list transitions, voice intent +
//! generation.
//! See `docs/development/decisions/0003-ui-port-max-reuse.md`.

pub mod editor_context;
pub mod session;
pub mod voice;

pub use editor_context::{
    EditorCursor, caret_to_display, offset_to_display, relative_path,
    selection_range,
};
pub use session::{
    AgentMessage, ChatMessage, ProposalSummary, SessionSnapshot, TrackerEntry,
    adopt_session, apply_mode, apply_workflow, begin_validation_error,
    drop_proposal, next_phase, push_agent_message, push_human_message,
    queue_tracker_dispatch,
};
pub use voice::{VoiceIntent, barge_in};
