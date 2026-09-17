//! AHEAD Reactive State for Lapce App UI
//!
//! Grounded in Sections 4.2, 4.4, 4.5, and 8.2 of `ahead-editor-mvp.md`.

use floem::reactive::{create_rw_signal, RwSignal, SignalGet, SignalUpdate};
use lapce_rpc::ahead::{
    ChangeProposal, PresentationCue, SessionParticipantRecord, SessionRole,
    SessionView, VoiceTranscriptUpdate,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentChatMessage {
    pub id: String,
    pub sender: String, // "Human", "AHEAD Agent", "Socratic Guide"
    pub text: String,
    pub timestamp: String,
    pub is_challenge: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackerOutboxEntry {
    pub id: String,
    pub target: String,
    pub description: String,
    pub status: String,
}

#[derive(Clone, Copy, Debug)]
pub struct AheadState {
    pub active_session: RwSignal<Option<SessionView>>,
    pub saved_sessions: RwSignal<Vec<SessionView>>,
    pub session_error: RwSignal<Option<String>>,
    pub session_busy: RwSignal<bool>,
    pub presentation_cue: RwSignal<Option<PresentationCue>>,
    pub show_start_work_modal: RwSignal<bool>,
    pub wizard_step: RwSignal<u8>,
    pub wizard_title: RwSignal<String>,
    pub wizard_starting_point: RwSignal<String>,
    pub wizard_private: RwSignal<bool>,
    pub voice_active: RwSignal<bool>,
    pub voice_listening: RwSignal<bool>,
    pub voice_speaking: RwSignal<bool>,
    pub voice_mic_muted: RwSignal<bool>,
    pub voice_generation: RwSignal<u64>,
    pub voice_waveform_level: RwSignal<f32>,
    pub voice_transcripts: RwSignal<Vec<VoiceTranscriptUpdate>>,
    pub chat_messages: RwSignal<Vec<AgentChatMessage>>,
    pub chat_input: RwSignal<String>,
    pub pending_proposals: RwSignal<Vec<ChangeProposal>>,
    pub tracker_outbox: RwSignal<Vec<TrackerOutboxEntry>>,
    pub authenticated_user: RwSignal<Option<lapce_rpc::ahead::GitHubUser>>,
    pub show_auth_modal: RwSignal<bool>,
    pub auth_error: RwSignal<Option<String>>,
    pub auth_busy: RwSignal<bool>,
    pub pending_device_code: RwSignal<Option<lapce_rpc::ahead::GitHubDeviceCodeResponse>>,
    pub show_collab_modal: RwSignal<bool>,
    pub collab_error: RwSignal<Option<String>>,
    pub workspace_participants: RwSignal<Vec<SessionParticipantRecord>>,
    pub new_collaborator_input: RwSignal<String>,
    pub new_collaborator_role: RwSignal<SessionRole>,
    pub token_input: RwSignal<String>,
    pub show_token_input: RwSignal<bool>,
}

impl AheadState {
    pub fn new() -> Self {
        Self {
            active_session: create_rw_signal(None),
            saved_sessions: create_rw_signal(Vec::new()),
            session_error: create_rw_signal(None),
            session_busy: create_rw_signal(false),
            presentation_cue: create_rw_signal(None),
            show_start_work_modal: create_rw_signal(false),
            wizard_step: create_rw_signal(0),
            wizard_title: create_rw_signal(String::new()),
            wizard_starting_point: create_rw_signal(String::new()),
            wizard_private: create_rw_signal(true),
            voice_active: create_rw_signal(false),
            voice_listening: create_rw_signal(false),
            voice_speaking: create_rw_signal(false),
            voice_mic_muted: create_rw_signal(false),
            voice_generation: create_rw_signal(1),
            voice_waveform_level: create_rw_signal(0.0),
            voice_transcripts: create_rw_signal(Vec::new()),
            chat_messages: create_rw_signal(Vec::new()),
            chat_input: create_rw_signal(String::new()),
            pending_proposals: create_rw_signal(Vec::new()),
            tracker_outbox: create_rw_signal(Vec::new()),
            authenticated_user: create_rw_signal(None),
            show_auth_modal: create_rw_signal(false),
            auth_error: create_rw_signal(None),
            auth_busy: create_rw_signal(false),
            pending_device_code: create_rw_signal(None),
            show_collab_modal: create_rw_signal(false),
            collab_error: create_rw_signal(None),
            workspace_participants: create_rw_signal(Vec::new()),
            new_collaborator_input: create_rw_signal(String::new()),
            new_collaborator_role: create_rw_signal(SessionRole::Editor),
            token_input: create_rw_signal(String::new()),
            show_token_input: create_rw_signal(false),
        }
    }

    /// Adopts a durable session returned by the session host.
    /// Clears per-session conversation scaffolds so a reopened session
    /// never shows another session's chat, proposals, or transcripts.
    pub fn adopt_durable_session(&self, view: SessionView) {
        self.active_session.set(Some(view));
        self.chat_messages.set(Vec::new());
        self.pending_proposals.set(Vec::new());
        self.tracker_outbox.set(Vec::new());
        self.voice_transcripts.set(Vec::new());
        self.voice_active.set(false);
        self.voice_listening.set(false);
        self.voice_speaking.set(false);
        self.presentation_cue.set(None);
        self.session_error.set(None);
        self.session_busy.set(false);
        self.show_start_work_modal.set(false);
    }

    /// Requests a host-backed mode change. Local state only updates when the
    /// session host acknowledges, so the chip never diverges from durable state.
    pub fn apply_remote_mode(&self, view: SessionView) {
        self.active_session.set(Some(view));
        self.session_error.set(None);
        self.session_busy.set(false);
    }

    /// Applies a workflow state acknowledged by the host after AdvancePhase.
    pub fn apply_remote_workflow(&self, session_id: &str, workflow: SessionView) {
        debug_assert_eq!(workflow.session.id, session_id);
        self.active_session.set(Some(workflow));
        self.session_error.set(None);
        self.session_busy.set(false);
    }

    /// Local mic toggle. Tracks intent only; no audio capture exists yet, so
    /// this MUST NOT append fabricated transcripts.
    pub fn toggle_voice(&self) {
        let next = !self.voice_active.get();
        self.voice_active.set(next);
        self.voice_listening.set(next);
        if !next {
            self.voice_speaking.set(false);
        }
    }

    /// Full-duplex barge-in: preempts playing audio within <50ms by bumping
    /// the output generation. Input capture and coding tasks continue.
    pub fn interrupt_voice_playback(&self) {
        let next_gen = self.voice_generation.get() + 1;
        self.voice_generation.set(next_gen);
        self.voice_speaking.set(false);
    }

    /// Records a human-authored chat message locally. Agent replies arrive
    /// only via session-host notifications, never synthesized here.
    pub fn send_chat(&self, text: String) {
        if text.trim().is_empty() {
            return;
        }
        self.chat_messages.update(|msgs| {
            msgs.push(AgentChatMessage {
                id: format!("msg-{}", msgs.len() + 1),
                sender: "Human".to_string(),
                text: text.clone(),
                timestamp: "Just now".to_string(),
                is_challenge: false,
            });
        });
        self.chat_input.set(String::new());
    }

    /// Drops a proposal from the pending list (local filter; host
    /// acceptance flows through AcceptProposal RPC in the panel view).
    pub fn drop_proposal(&self, id: &str) {
        self.pending_proposals.update(|props| {
            props.retain(|p| p.id != id);
        });
    }

    /// Appends an agent/system message to the pairing feed.
    pub fn push_message(&self, sender: &str, text: String, is_challenge: bool) {
        self.chat_messages.update(|msgs| {
            msgs.push(AgentChatMessage {
                id: format!("msg-{}", msgs.len() + 1),
                sender: sender.to_string(),
                text,
                timestamp: "Just now".to_string(),
                is_challenge,
            });
        });
    }

    /// Marks a tracker outbox entry dispatched. Local-only until the
    /// GitHub publish flow lands; status text says so explicitly.
    pub fn dispatch_tracker_item(&self, id: &str) {
        self.tracker_outbox.update(|items| {
            for item in items.iter_mut() {
                if item.id == id {
                    item.status = "Queued locally (GitHub publish not wired yet)".to_string();
                }
            }
        });
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
}

impl Default for AheadState {
    fn default() -> Self {
        Self::new()
    }
}
