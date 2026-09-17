//! AHEAD Reactive State for Lapce App UI
//!
//! Grounded in Sections 4.2, 4.4, and 4.5 of `ahead-editor-mvp.md`.

use floem::reactive::{create_rw_signal, RwSignal, SignalUpdate};
use lapce_rpc::ahead::{
    AssistanceMode, PresentationCue, SessionView, WorkKind, WorkSession,
    WorkflowPhase, WorkflowState, SessionLifecycle, SessionPolicySnapshot,
};

#[derive(Clone, Copy, Debug)]
pub struct AheadState {
    pub active_session: RwSignal<Option<SessionView>>,
    pub presentation_cue: RwSignal<Option<PresentationCue>>,
    pub show_start_work_modal: RwSignal<bool>,
    pub voice_active: RwSignal<bool>,
}

impl AheadState {
    pub fn new() -> Self {
        Self {
            active_session: create_rw_signal(None),
            presentation_cue: create_rw_signal(None),
            show_start_work_modal: create_rw_signal(false),
            voice_active: create_rw_signal(false),
        }
    }

    /// Initializes a sample initial work session
    pub fn start_local_session(
        &self,
        work_kind: WorkKind,
        mode: AssistanceMode,
        title: String,
    ) {
        let session = WorkSession {
            id: "session-local".to_string(),
            project_id: "project-local".to_string(),
            worktree_id: "worktree-local".to_string(),
            work_kind,
            mode,
            title,
            owner_id: "user-local".to_string(),
            lifecycle: SessionLifecycle::Active,
            policy: SessionPolicySnapshot::default(),
            revision: 1,
            created_at: chrono::Utc::now().to_rfc3339(),
        };

        let initial_phase = match work_kind {
            WorkKind::ProductChange => WorkflowPhase {
                id: "plan".to_string(),
                title: "Planning".to_string(),
                visit: 1,
            },
            WorkKind::CorrectiveDebugging => WorkflowPhase {
                id: "hypothesize".to_string(),
                title: "Hypothesis".to_string(),
                visit: 1,
            },
            WorkKind::InternalImprovement => WorkflowPhase {
                id: "invariants".to_string(),
                title: "Invariants".to_string(),
                visit: 1,
            },
            WorkKind::Investigation => WorkflowPhase {
                id: "scrutinize".to_string(),
                title: "Investigation".to_string(),
                visit: 1,
            },
            WorkKind::Decision => WorkflowPhase {
                id: "framing".to_string(),
                title: "Framing".to_string(),
                visit: 1,
            },
            WorkKind::OperationalStabilization => WorkflowPhase {
                id: "stabilize".to_string(),
                title: "Stabilization".to_string(),
                visit: 1,
            },
        };

        let workflow = WorkflowState {
            revision: 1,
            definition_version: "2026-09-17-v1".to_string(),
            phase: initial_phase,
            primary_work_item: None,
            current_artifact_ids: Vec::new(),
            approvals: Vec::new(),
        };

        self.active_session.set(Some(SessionView {
            session,
            workflow,
            participants: Vec::new(),
        }));
        self.show_start_work_modal.set(false);
    }

    pub fn toggle_mode(&self) {
        self.active_session.update(|opt| {
            if let Some(view) = opt {
                view.session.mode = match view.session.mode {
                    AssistanceMode::Learn => AssistanceMode::Assist,
                    AssistanceMode::Assist => AssistanceMode::Learn,
                };
                view.session.revision += 1;
            }
        });
    }
}

impl Default for AheadState {
    fn default() -> Self {
        Self::new()
    }
}
