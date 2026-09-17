//! AHEAD framework-agnostic session viewmodel.
//!
//! Pure logic shared by the Floem shell (today) and the future GPUI shell.
//! Zero UI-framework dependencies: no Floem, no GPUI, no Tauri. State is
//! plain Rust owned by the caller; each method is a total function over
//! its inputs. Reactive shells wrap these transitions in their own
//! signals/entities.
//!
//! Covered today: wizard validation + turn assembly, session adoption
//! hygiene, chat/proposal/tracker list transitions, voice intent +
//! generation, Codex turn-shape assembly, approval/sandbox mapping.
//! See `docs/development/decisions/0003-ui-port-max-reuse.md`.

pub mod editor_context;
pub mod session;
pub mod turn;
pub mod voice;

pub use editor_context::{caret_to_display, offset_to_display, relative_path, selection_range, EditorCursor};
pub use session::{
    adopt_session, apply_mode, apply_workflow, begin_validation_error,
    drop_proposal, next_phase, push_agent_message, push_human_message,
    queue_tracker_dispatch, AgentMessage, ChatMessage, ProposalSummary,
    SessionSnapshot, TrackerEntry,
};
pub use turn::{
    approval_policy_for_mode, codex_turn_params, decide_approval,
    sandbox_for_mode, ApprovalPolicy, Sandbox, TurnAssembly,
};
pub use voice::{barge_in, VoiceIntent};
