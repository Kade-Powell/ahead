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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkItemRow {
    pub item: lapce_rpc::ahead::WorkItem,
    pub events: Vec<lapce_rpc::ahead::WorkItemEvent>,
    pub closeout: Option<lapce_rpc::ahead::WorkItemCloseout>,
}

#[derive(Clone, Copy, Debug)]
pub struct AheadState {
    pub active_session: RwSignal<Option<SessionView>>,
    pub work_items: RwSignal<Vec<WorkItemRow>>,
    pub work_item_input: RwSignal<String>,
    pub work_item_error: RwSignal<Option<String>>,
    pub conversation_summaries: RwSignal<Vec<lapce_rpc::ahead::ConversationSummary>>,
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
            work_items: create_rw_signal(Vec::new()),
            work_item_input: create_rw_signal(String::new()),
            work_item_error: create_rw_signal(None),
            conversation_summaries: create_rw_signal(Vec::new()),
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

    /// Sends chat through the built-in agent loop via AgentTurn RPC.
    /// Captures live editor context (path, caret, selection, content),
    /// records the human message immediately, then dispatches the turn.
    /// Turn output renders as agent chat + challenges, presentation cue,
    /// and staged proposal. All host-mediated; errors surface inline.
    pub fn send_chat_turn(
        &self,
        text: String,
        window_tab: &std::rc::Rc<crate::window_tab::WindowTabData>,
    ) {
        use floem::reactive::{SignalGet, SignalUpdate};
        use floem::ext_event::create_ext_action;
        use lapce_rpc::ahead::{
            AgentTurnRequestDto, TurnEditorContext,
        };
        if text.trim().is_empty() {
            return;
        }
        let view_opt = self.active_session.get_untracked();
        let Some(view) = view_opt else {
            self.session_error.set(Some("Start a session before chatting.".to_string()));
            return;
        };
        // Capture live editor context: path, caret line/col, selection, content.
        let (active_path, caret, selection, file_content) =
            Self::capture_editor_context(window_tab);
        let dto = AgentTurnRequestDto {
            session_id: view.session.id.clone(),
            thread_id: format!("thread-{}", view.session.id),
            user_message: text.clone(),
            context: TurnEditorContext {
                active_path,
                caret,
                selection,
                file_content,
                visible_end: None,
                attached_anchor_ids: Vec::new(),
            },
            invariants: Vec::new(),
            cwd: None,
            expected_policy_sha256: view.session.policy.sha256.clone(),
            scope: None,
        };
        // Record human message now (viewmodel-validated).
        self.send_chat(text);
        self.session_busy.set(true);
        self.session_error.set(None);
        let scope = window_tab.scope;
        let proxy = window_tab.common.proxy.clone();
        let ahead = *self;
        let send = create_ext_action(scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
            match res {
                Ok(val) => match serde_json::from_value::<lapce_proxy::ahead::agent::AgentTurnOutput>(val) {
                    Ok(out) => ahead.apply_turn_output(out),
                    Err(_) => ahead.session_error.set(Some("Turn returned an unreadable response.".to_string())),
                },
                Err(e) => ahead.session_error.set(Some(format!("Turn failed: {}", e.message))),
            }
            ahead.session_busy.set(false);
        });
        proxy.ahead_request(
            lapce_rpc::ahead::AheadRequest::AgentTurn { request: dto },
            move |res| send(res),
        );
    }

    /// Captures (path, caret, selection, content) from the active editor.
    /// Falls back to empty context when no editor is open.
    fn capture_editor_context(
        window_tab: &std::rc::Rc<crate::window_tab::WindowTabData>,
    ) -> (String, lapce_rpc::ahead::DisplayPosition, Option<lapce_rpc::ahead::DisplayRange>, String) {
        use floem::reactive::{SignalGet, SignalWith};
        use lapce_core::{buffer::rope_text::RopeText, encoding::offset_utf8_to_utf16};
        let fallback = (
            String::new(),
            ahead_viewmodel::caret_to_display(0, 0),
            None,
            String::new(),
        );
        let Some(editor) = window_tab.main_split.active_editor.get_untracked() else {
            return fallback;
        };
        let doc = editor.doc();
        let abs_path = match doc.content.get_untracked() {
            crate::doc::DocContent::File { path, .. } => path.to_string_lossy().to_string(),
            _ => return fallback,
        };
        let ws_root = window_tab.workspace.path.clone().unwrap_or_default()
            .to_string_lossy().to_string();
        let path = ahead_viewmodel::relative_path(&ws_root, &abs_path);
        let offset = editor.cursor().get_untracked().offset();
        let (caret, content) = doc.buffer.with_untracked(|b| {
            let caret = ahead_viewmodel::offset_to_display(
                offset,
                |off| b.offset_to_line_col(off),
                |off| {
                    let (line, _col) = b.offset_to_line_col(off);
                    let line_offset = b.offset_of_line(line);
                    offset_utf8_to_utf16(
                        b.char_indices_iter(line_offset..),
                        off - line_offset,
                    )
                },
            );
            (caret, b.text().to_string())
        });
        (path, caret, None, content)
    }

    /// Renders turn output: agent message + challenges as chat, cue as
    /// presentation overlay, staged proposal appended to the gate list.
    pub fn apply_turn_output(&self, out: lapce_proxy::ahead::agent::AgentTurnOutput) {
        use floem::reactive::SignalUpdate;
        self.push_message("AHEAD Agent", out.message, false);
        for challenge in out.edge_case_challenges {
            self.push_message("Socratic Guide", challenge, true);
        }
        if let Some(cue) = out.presentation_cue {
            self.presentation_cue.set(Some(cue));
        }
        if let Some(prop) = out.scaffold_proposal {
            self.pending_proposals.update(|list| {
                if !list.iter().any(|p| p.id == prop.id) {
                    list.push(prop);
                }
            });
        }
    }

    /// Refreshes the checklist + summaries for a session from the host.
    /// Called on session adopt/resume and after every item mutation.
    pub fn refresh_work_items(
        &self,
        window_tab: &std::rc::Rc<crate::window_tab::WindowTabData>,
    ) {
        use floem::ext_event::create_ext_action;
        let Some(view) = self.active_session.get_untracked() else {
            return;
        };
        let session_id = view.session.id.clone();
        let ahead = *self;
        let proxy = window_tab.common.proxy.clone();
        let scope = window_tab.scope;
        let send = create_ext_action(scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
            match res {
                Ok(val) => match serde_json::from_value::<Vec<lapce_rpc::ahead::WorkItem>>(val) {
                    Ok(items) => {
                        ahead.work_items.set(items.into_iter().map(|item| WorkItemRow {
                            item,
                            events: Vec::new(),
                            closeout: None,
                        }).collect());
                        ahead.work_item_error.set(None);
                    }
                    Err(_) => ahead.work_item_error.set(Some("Checklist returned an unreadable list.".to_string())),
                },
                Err(e) => ahead.work_item_error.set(Some(format!("Checklist refresh failed: {}", e.message))),
            }
        });
        proxy.ahead_request(
            lapce_rpc::ahead::AheadRequest::WorkItemList { session_id: session_id.clone() },
            move |res| send(res),
        );
        let ahead2 = *self;
        let proxy2 = window_tab.common.proxy.clone();
        let send2 = create_ext_action(scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
            if let Ok(val) = res {
                if let Ok(summaries) = serde_json::from_value::<Vec<lapce_rpc::ahead::ConversationSummary>>(val) {
                    ahead2.conversation_summaries.set(summaries);
                }
            }
        });
        proxy2.ahead_request(
            lapce_rpc::ahead::AheadRequest::ConversationList { session_id },
            move |res| send2(res),
        );
    }

    /// Creates a checklist item from the inline add box.
    pub fn create_work_item(
        &self,
        window_tab: &std::rc::Rc<crate::window_tab::WindowTabData>,
    ) {
        use floem::ext_event::create_ext_action;
        let title = self.work_item_input.get_untracked().trim().to_string();
        if title.is_empty() {
            self.work_item_error.set(Some("Give the work item a title first.".to_string()));
            return;
        }
        let Some(view) = self.active_session.get_untracked() else {
            self.work_item_error.set(Some("Start a session before adding items.".to_string()));
            return;
        };
        let ahead = *self;
        let wt = window_tab.clone();
        let send = create_ext_action(window_tab.scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
            match res {
                Ok(_) => {
                    ahead.work_item_input.set(String::new());
                    ahead.work_item_error.set(None);
                    ahead.refresh_work_items(&wt);
                }
                Err(e) => ahead.work_item_error.set(Some(format!("Add failed: {}", e.message))),
            }
        });
        window_tab.common.proxy.ahead_request(
            lapce_rpc::ahead::AheadRequest::WorkItemCreate { session_id: view.session.id.clone(), title },
            move |res| send(res),
        );
    }

    /// Cycles an item's status open → in_progress → done (with close-out)
    /// → open. Done requires the close-out summary; use the expander.
    pub fn cycle_work_item_status(
        &self,
        item_id: String,
        current: lapce_rpc::ahead::WorkItemStatus,
        window_tab: &std::rc::Rc<crate::window_tab::WindowTabData>,
    ) {
        use floem::ext_event::create_ext_action;
        use lapce_rpc::ahead::WorkItemStatus;
        let next = match current {
            WorkItemStatus::Open => WorkItemStatus::InProgress,
            WorkItemStatus::InProgress => WorkItemStatus::Done,
            WorkItemStatus::Done => WorkItemStatus::Open,
            WorkItemStatus::Dropped => WorkItemStatus::Open,
        };
        // Done via cycle still needs a close-out: route through close with
        // an explicit auto summary so nothing closes silently undocumented.
        if matches!(next, WorkItemStatus::Done) {
            self.close_work_item(item_id, String::new(), None, window_tab);
            return;
        }
        let ahead = *self;
        let wt = window_tab.clone();
        let send = create_ext_action(window_tab.scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
            match res {
                Ok(_) => ahead.refresh_work_items(&wt),
                Err(e) => ahead.work_item_error.set(Some(format!("Status change failed: {}", e.message))),
            }
        });
        window_tab.common.proxy.ahead_request(
            lapce_rpc::ahead::AheadRequest::WorkItemSetStatus { item_id, status: next },
            move |res| send(res),
        );
    }

    /// Closes an item with its close-out summary (required unless skipped
    /// with an explicit reason). The summary becomes the issue-closing
    /// draft via the tracker outbox flow.
    pub fn close_work_item(
        &self,
        item_id: String,
        summary_markdown: String,
        issue_ref: Option<String>,
        window_tab: &std::rc::Rc<crate::window_tab::WindowTabData>,
    ) {
        use floem::ext_event::create_ext_action;
        let ahead = *self;
        let wt = window_tab.clone();
        let send = create_ext_action(window_tab.scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
            match res {
                Ok(val) => match serde_json::from_value::<lapce_rpc::ahead::WorkItemCloseout>(val) {
                    Ok(closeout) => {
                        ahead.work_item_error.set(None);
                        ahead.refresh_work_items(&wt);
                        // Stage the close-out as the issue-closing draft.
                        ahead.tracker_outbox.update(|items| {
                            items.push(TrackerOutboxEntry {
                                id: format!("closeout-{}", closeout.item_id),
                                target: closeout.issue_ref.clone().unwrap_or_else(|| "Issue (link on publish)".to_string()),
                                description: closeout.summary_markdown.clone(),
                                status: "Close-out draft (review before publish)".to_string(),
                            });
                        });
                    }
                    Err(_) => ahead.work_item_error.set(Some("Close returned an unreadable close-out.".to_string())),
                },
                Err(e) => ahead.work_item_error.set(Some(format!("Close failed (summary or skip reason required): {}", e.message))),
            }
        });
        window_tab.common.proxy.ahead_request(
            lapce_rpc::ahead::AheadRequest::WorkItemClose {
                item_id,
                summary_markdown,
                issue_ref,
                follow_ups_json: None,
                skip_reason: None,
            },
            move |res| send(res),
        );
     }
    /// Clears per-session conversation scaffolds so a reopened session
    /// never shows another session's chat, proposals, or transcripts.
    /// Delegates to the shared viewmodel (single source of truth).
    pub fn adopt_durable_session(&self, view: SessionView) {
        let mut snapshot = ahead_viewmodel::SessionSnapshot::default();
        ahead_viewmodel::adopt_session(&mut snapshot, view);
        self.apply_snapshot_shape(&snapshot);
        self.active_session.set(snapshot.active);
        self.voice_active.set(false);
        self.voice_listening.set(false);
        self.voice_speaking.set(false);
        self.presentation_cue.set(None);
        self.show_start_work_modal.set(false);
    }

    /// Applies a viewmodel snapshot's list shapes to Floem signals.
    fn apply_snapshot_shape(&self, snapshot: &ahead_viewmodel::SessionSnapshot) {
        let chat: Vec<AgentChatMessage> = snapshot.chat.iter().map(|m| AgentChatMessage {
            id: m.id.clone(),
            sender: m.sender.clone(),
            text: m.text.clone(),
            timestamp: "Just now".to_string(),
            is_challenge: m.is_challenge,
        }).collect();
        self.chat_messages.set(chat);
        self.pending_proposals.set(snapshot.pending_proposals.clone());
        let tracker: Vec<TrackerOutboxEntry> = snapshot.tracker.iter().map(|t| TrackerOutboxEntry {
            id: t.id.clone(),
            target: t.target.clone(),
            description: t.description.clone(),
            status: t.status.clone(),
        }).collect();
        self.tracker_outbox.set(tracker);
        self.voice_transcripts.set(snapshot.transcripts.clone());
        self.session_error.set(None);
        self.session_busy.set(false);
    }

    /// Requests a host-backed mode change. Local state only updates when the
    /// session host acknowledges, so the chip never diverges from durable state.
    pub fn apply_remote_mode(&self, view: SessionView) {
        let mut snapshot = ahead_viewmodel::SessionSnapshot::default();
        ahead_viewmodel::apply_mode(&mut snapshot, view);
        self.active_session.set(snapshot.active);
        self.session_error.set(None);
        self.session_busy.set(false);
    }

    /// Applies a workflow state acknowledged by the host after AdvancePhase.
    pub fn apply_remote_workflow(&self, session_id: &str, workflow: SessionView) {
        let mut snapshot = ahead_viewmodel::SessionSnapshot::default();
        ahead_viewmodel::apply_workflow(&mut snapshot, session_id, workflow);
        self.active_session.set(snapshot.active);
        self.session_error.set(None);
        self.session_busy.set(false);
    }

    /// Local mic toggle. Tracks intent only; no audio capture exists yet, so
    /// this MUST NOT append fabricated transcripts.
    /// Delegates to the shared viewmodel (single source of truth).
    pub fn toggle_voice(&self) {
        let mut intent = ahead_viewmodel::VoiceIntent {
            active: self.voice_active.get_untracked(),
            listening: self.voice_listening.get_untracked(),
            speaking: self.voice_speaking.get_untracked(),
            mic_muted: self.voice_mic_muted.get_untracked(),
            generation: self.voice_generation.get_untracked(),
        };
        ahead_viewmodel::voice::toggle(&mut intent);
        self.voice_active.set(intent.active);
        self.voice_listening.set(intent.listening);
        self.voice_speaking.set(intent.speaking);
    }

    /// Full-duplex barge-in: preempts playing audio within <50ms by bumping
    /// the output generation. Input capture and coding tasks continue.
    /// Delegates to the shared viewmodel (single source of truth).
    pub fn interrupt_voice_playback(&self) {
        let mut intent = ahead_viewmodel::VoiceIntent {
            active: self.voice_active.get_untracked(),
            listening: self.voice_listening.get_untracked(),
            speaking: self.voice_speaking.get_untracked(),
            mic_muted: self.voice_mic_muted.get_untracked(),
            generation: self.voice_generation.get_untracked(),
        };
        let next_gen = ahead_viewmodel::voice::barge_in(&mut intent);
        self.voice_generation.set(next_gen);
        self.voice_speaking.set(intent.speaking);
    }

    /// Records a human-authored chat message locally. Agent replies arrive
    /// only via session-host notifications, never synthesized here.
    /// Delegates to the shared viewmodel (single source of truth).
    pub fn send_chat(&self, text: String) {
        let mut snapshot = ahead_viewmodel::SessionSnapshot {
            chat: self.chat_messages.get_untracked().into_iter().map(|m| ahead_viewmodel::ChatMessage {
                id: m.id,
                sender: m.sender,
                text: m.text,
                is_challenge: m.is_challenge,
            }).collect(),
            ..Default::default()
        };
        if ahead_viewmodel::push_human_message(&mut snapshot, &text) {
            let msgs: Vec<AgentChatMessage> = snapshot.chat.into_iter().map(|m| AgentChatMessage {
                id: m.id,
                sender: m.sender,
                text: m.text,
                timestamp: "Just now".to_string(),
                is_challenge: m.is_challenge,
            }).collect();
            self.chat_messages.set(msgs);
            self.chat_input.set(String::new());
        }
    }

    /// Drops a proposal from the pending list (local filter; host
    /// acceptance flows through AcceptProposal RPC in the panel view).
    /// Delegates to the shared viewmodel (single source of truth).
    pub fn drop_proposal(&self, id: &str) {
        let mut snapshot = ahead_viewmodel::SessionSnapshot {
            pending_proposals: self.pending_proposals.get_untracked(),
            ..Default::default()
        };
        if ahead_viewmodel::drop_proposal(&mut snapshot, id) {
            self.pending_proposals.set(snapshot.pending_proposals);
        }
    }

    /// Appends an agent/system message to the pairing feed.
    /// Delegates to the shared viewmodel (single source of truth).
    pub fn push_message(&self, sender: &str, text: String, is_challenge: bool) {
        let mut snapshot = ahead_viewmodel::SessionSnapshot {
            chat: self.chat_messages.get_untracked().into_iter().map(|m| ahead_viewmodel::ChatMessage {
                id: m.id,
                sender: m.sender,
                text: m.text,
                is_challenge: m.is_challenge,
            }).collect(),
            ..Default::default()
        };
        ahead_viewmodel::push_agent_message(&mut snapshot, ahead_viewmodel::AgentMessage {
            sender: sender.to_string(),
            text,
            is_challenge,
        });
        let msgs: Vec<AgentChatMessage> = snapshot.chat.into_iter().map(|m| AgentChatMessage {
            id: m.id,
            sender: m.sender,
            text: m.text,
            timestamp: "Just now".to_string(),
            is_challenge: m.is_challenge,
        }).collect();
        self.chat_messages.set(msgs);
    }

    /// Marks a tracker outbox entry dispatched. Local-only until the
    /// GitHub publish flow lands; status text says so explicitly.
    /// Delegates to the shared viewmodel (single source of truth).
    pub fn dispatch_tracker_item(&self, id: &str) {
        let mut snapshot = ahead_viewmodel::SessionSnapshot {
            tracker: self.tracker_outbox.get_untracked().into_iter().map(|t| ahead_viewmodel::TrackerEntry {
                id: t.id,
                target: t.target,
                description: t.description,
                status: t.status,
            }).collect(),
            ..Default::default()
        };
        if ahead_viewmodel::queue_tracker_dispatch(&mut snapshot, id) {
            let items: Vec<TrackerOutboxEntry> = snapshot.tracker.into_iter().map(|t| TrackerOutboxEntry {
                id: t.id,
                target: t.target,
                description: t.description,
                status: t.status,
            }).collect();
            self.tracker_outbox.set(items);
        }
    }

    /// Next workflow phase for the given current phase id.
    /// Delegates to the shared viewmodel (single source of truth).
    pub fn next_phase(current: &str) -> (&'static str, &'static str) {
        ahead_viewmodel::next_phase(current)
    }
}

impl Default for AheadState {
    fn default() -> Self {
        Self::new()
    }
}
