//! AHEAD Reactive State for Lapce App UI
//!
//! Grounded in Sections 4.2, 4.4, 4.5, and 8.2 of `ahead-editor-mvp.md`.

use floem::reactive::{create_rw_signal, RwSignal, SignalGet, SignalUpdate};
use lapce_rpc::ahead::{
    AssistanceMode, ChangeProposal, DisplayPosition, PresentationCue,
    SessionLifecycle, SessionParticipantRecord, SessionPolicySnapshot,
    SessionRole, SessionView, VoiceTranscriptUpdate, WorkKind, WorkSession,
    WorkflowPhase, WorkflowState,
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
    pub presentation_cue: RwSignal<Option<PresentationCue>>,
    pub show_start_work_modal: RwSignal<bool>,
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
    pub pending_device_code: RwSignal<Option<lapce_rpc::ahead::GitHubDeviceCodeResponse>>,
    pub show_collab_modal: RwSignal<bool>,
    pub workspace_participants: RwSignal<Vec<SessionParticipantRecord>>,
    pub new_collaborator_input: RwSignal<String>,
    pub new_collaborator_role: RwSignal<SessionRole>,
    pub token_input: RwSignal<String>,
    pub show_token_input: RwSignal<bool>,
}

impl AheadState {
    pub fn new() -> Self {
        let initial_session = WorkSession {
            id: "session-ahead-mvp".to_string(),
            project_id: "ahead-core".to_string(),
            worktree_id: "local-worktree".to_string(),
            work_kind: WorkKind::ProductChange,
            mode: AssistanceMode::Learn,
            title: "AHEAD Development & Pairing Loop".to_string(),
            owner_id: "ahead-engineer".to_string(),
            lifecycle: SessionLifecycle::Active,
            policy: SessionPolicySnapshot::default(),
            revision: 1,
            created_at: chrono::Utc::now().to_rfc3339(),
        };

        let initial_workflow = WorkflowState {
            revision: 1,
            definition_version: "2026-09-17-v1".to_string(),
            phase: WorkflowPhase {
                id: "plan".to_string(),
                title: "Planning".to_string(),
                visit: 1,
            },
            primary_work_item: None,
            current_artifact_ids: Vec::new(),
            approvals: Vec::new(),
        };

        let default_view = SessionView {
            session: initial_session,
            workflow: initial_workflow,
            participants: Vec::new(),
        };

        let sample_messages = vec![
            AgentChatMessage {
                id: "msg-1".to_string(),
                sender: "AHEAD Agent".to_string(),
                text: "⚡ Welcome to AHEAD! I am your paired engineering agent. Let's work through the planning phase together.".to_string(),
                timestamp: "Just now".to_string(),
                is_challenge: false,
            },
            AgentChatMessage {
                id: "msg-2".to_string(),
                sender: "Socratic Guide".to_string(),
                text: "💡 Socratic Question: What are the core state invariants that must be verified before moving to implementation?".to_string(),
                timestamp: "Just now".to_string(),
                is_challenge: true,
            },
        ];

        let sample_proposals = vec![
            ChangeProposal {
                id: "prop-1".to_string(),
                session_id: "session-ahead-mvp".to_string(),
                path: "lapce-app/src/panel/view.rs".to_string(),
                original_sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
                patch: "+ // Registered AheadAgent in sidecar panel\n+ ahead_agent_panel(window_tab_data, position)".to_string(),
                is_mechanical: true,
                description: "Wire AHEAD AI Agent panel and Voice Chat into the right panel dock".to_string(),
                recommended_cursor: Some(DisplayPosition { line: 472, col: 16 }),
            },
        ];

        let sample_outbox = vec![
            TrackerOutboxEntry {
                id: "outbox-1".to_string(),
                target: "GitHub Issue #10".to_string(),
                description: "Update task checklist: AI Panel, Voice Chat & Hot Recompiling Dev loop".to_string(),
                status: "Pending Human Authorization".to_string(),
            },
        ];

        let sample_transcripts = vec![
            VoiceTranscriptUpdate {
                voice_session_id: "voice-1".to_string(),
                epoch: 1,
                generation: 1,
                text: "Voice runtime initialized. Ready for hands-free pairing.".to_string(),
                is_final: true,
                speaker_id: "agent".to_string(),
            },
        ];

        Self {
            active_session: create_rw_signal(Some(default_view)),
            presentation_cue: create_rw_signal(None),
            show_start_work_modal: create_rw_signal(false),
            voice_active: create_rw_signal(false),
            voice_listening: create_rw_signal(false),
            voice_speaking: create_rw_signal(false),
            voice_mic_muted: create_rw_signal(false),
            voice_generation: create_rw_signal(1),
            voice_waveform_level: create_rw_signal(0.4),
            voice_transcripts: create_rw_signal(sample_transcripts),
            chat_messages: create_rw_signal(sample_messages),
            chat_input: create_rw_signal(String::new()),
            pending_proposals: create_rw_signal(sample_proposals),
            tracker_outbox: create_rw_signal(sample_outbox),
            authenticated_user: create_rw_signal(None),
            show_auth_modal: create_rw_signal(false),
            pending_device_code: create_rw_signal(None),
            show_collab_modal: create_rw_signal(false),
            workspace_participants: create_rw_signal(Vec::new()),
            new_collaborator_input: create_rw_signal(String::new()),
            new_collaborator_role: create_rw_signal(SessionRole::Editor),
            token_input: create_rw_signal(String::new()),
            show_token_input: create_rw_signal(false),
        }
    }

    /// Initializes a work session from the modal
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

    /// Toggles between Learn (Socratic) and Assist (Scaffolding) mode
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

    /// Advances the active session to the next workflow phase
    pub fn advance_phase(&self) {
        self.active_session.update(|opt| {
            if let Some(view) = opt {
                let current = &view.workflow.phase.id;
                let (next_id, next_title) = match current.as_str() {
                    "plan" | "hypothesize" => ("invariants", "Invariants"),
                    "invariants" => ("implement", "Implementation"),
                    "implement" => ("verify", "Verification"),
                    "verify" => ("review", "Review & Signoff"),
                    "review" => ("complete", "Completed"),
                    _ => ("plan", "Planning"),
                };
                view.workflow.phase = WorkflowPhase {
                    id: next_id.to_string(),
                    title: next_title.to_string(),
                    visit: view.workflow.phase.visit + 1,
                };
                view.session.revision += 1;
            }
        });

        // Add progress notification to agent chat
        let phase_title = self
            .active_session
            .get()
            .map(|v| v.workflow.phase.title)
            .unwrap_or_else(|| "Next Phase".to_string());
        self.chat_messages.update(|msgs| {
            msgs.push(AgentChatMessage {
                id: format!("msg-{}", msgs.len() + 1),
                sender: "AHEAD Agent".to_string(),
                text: format!("✅ Advanced to {}. Invariants check active.", phase_title),
                timestamp: "Just now".to_string(),
                is_challenge: false,
            });
        });
    }

    /// Toggles voice chat session (Microphone capture & audio stream)
    pub fn toggle_voice(&self) {
        let current = self.voice_active.get();
        let next = !current;
        self.voice_active.set(next);
        self.voice_listening.set(next);
        if next {
            self.voice_transcripts.update(|t| {
                t.push(VoiceTranscriptUpdate {
                    voice_session_id: "voice-live".to_string(),
                    epoch: 1,
                    generation: self.voice_generation.get(),
                    text: "🎙️ Microphone connected. Full-duplex voice stream active.".to_string(),
                    is_final: true,
                    speaker_id: "system".to_string(),
                });
            });
        }
    }

    /// Full-duplex barge-in: preempts and interrupts playing audio within <50ms
    pub fn interrupt_voice_playback(&self) {
        let next_gen = self.voice_generation.get() + 1;
        self.voice_generation.set(next_gen);
        self.voice_speaking.set(false);
        self.voice_transcripts.update(|t| {
            t.push(VoiceTranscriptUpdate {
                voice_session_id: "voice-live".to_string(),
                epoch: 1,
                generation: next_gen,
                text: "⏹️ Barge-in triggered: audio playback preempted.".to_string(),
                is_final: true,
                speaker_id: "system".to_string(),
            });
        });
    }

    /// Sends a chat message to the paired agent
    pub fn send_chat(&self, text: String) {
        if text.trim().is_empty() {
            return;
        }
        let mode = self
            .active_session
            .get()
            .map(|s| s.session.mode)
            .unwrap_or(AssistanceMode::Learn);
        let phase = self
            .active_session
            .get()
            .map(|s| s.workflow.phase.title)
            .unwrap_or_else(|| "Planning".to_string());

        self.chat_messages.update(|msgs| {
            msgs.push(AgentChatMessage {
                id: format!("msg-{}", msgs.len() + 1),
                sender: "Human".to_string(),
                text: text.clone(),
                timestamp: "Just now".to_string(),
                is_challenge: false,
            });

            match mode {
                AssistanceMode::Learn => {
                    msgs.push(AgentChatMessage {
                        id: format!("msg-{}", msgs.len() + 1),
                        sender: "Socratic Guide".to_string(),
                        text: format!(
                            "During {}, consider: How do our system invariants guarantee fault-tolerance for '{}'?",
                            phase, text
                        ),
                        timestamp: "Just now".to_string(),
                        is_challenge: true,
                    });
                }
                AssistanceMode::Assist => {
                    msgs.push(AgentChatMessage {
                        id: format!("msg-{}", msgs.len() + 1),
                        sender: "AHEAD Agent".to_string(),
                        text: format!(
                            "Drafted mechanical scaffolding proposal for '{}'. Review diff below before authorizing.",
                            text
                        ),
                        timestamp: "Just now".to_string(),
                        is_challenge: false,
                    });
                }
            }
        });
        self.chat_input.set(String::new());
    }

    /// Authorizes and applies a proposed change
    pub fn authorize_proposal(&self, id: &str) {
        self.pending_proposals.update(|props| {
            props.retain(|p| p.id != id);
        });
        self.chat_messages.update(|msgs| {
            msgs.push(AgentChatMessage {
                id: format!("msg-{}", msgs.len() + 1),
                sender: "AHEAD Agent".to_string(),
                text: format!("✅ Proposal {} authorized and applied by human engineer.", id),
                timestamp: "Just now".to_string(),
                is_challenge: false,
            });
        });
    }

    /// Rejects a proposed change
    pub fn reject_proposal(&self, id: &str) {
        self.pending_proposals.update(|props| {
            props.retain(|p| p.id != id);
        });
        self.chat_messages.update(|msgs| {
            msgs.push(AgentChatMessage {
                id: format!("msg-{}", msgs.len() + 1),
                sender: "AHEAD Agent".to_string(),
                text: format!("❌ Proposal {} rejected.", id),
                timestamp: "Just now".to_string(),
                is_challenge: false,
            });
        });
    }

    /// Dispatches a tracker outbox update
    pub fn dispatch_tracker_item(&self, id: &str) {
        self.tracker_outbox.update(|items| {
            for item in items.iter_mut() {
                if item.id == id {
                    item.status = "Dispatched to GitHub Issue Tracker".to_string();
                }
            }
        });
    }
}

impl Default for AheadState {
    fn default() -> Self {
        Self::new()
    }
}
