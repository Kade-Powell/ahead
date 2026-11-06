//! AHEAD Session Host
//!
//! Grounded in Section 6 and Section 11 of `ahead-editor-mvp.md`.
//! Serves as the central mediation and effect boundary in `ahead-proxy`.
//! Manages active sessions, enforces phase transitions and Learn/Assist boundaries,
//! routes voice frames and prediction requests, and persists durable events.

use anyhow::{Context, Result, bail};
use parking_lot::{Mutex, RwLock};
use sha2::Digest;
#[cfg(not(any(unix, windows)))]
use std::fs::OpenOptions;
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

use ahead_rpc::ahead::{
    AHEAD_ACTOR_ID, AheadRequest, AssistanceMode, CodeAnchor, CodeComment,
    ConversationMessage, DisplayRange, GithubIssueRef, HarnessKind, Id, LearningArc,
    LearningRecord, MemoryDocument, MemoryExcerpt, MemoryScope, MemoryWriteResult,
    Participant, PredictionAcceptance, PredictionBinding, PredictionModelContext,
    PredictionRequest, PredictionResult, RepoPath, Revision, SessionExportBundle,
    SessionLifecycle, SessionListItem, SessionParticipantRecord,
    SessionPolicySnapshot, SessionRole, SessionTask, SessionView, TaskIntent,
    VoiceCallLease, VoiceControl, VoiceTranscriptSpeaker, VoiceTranscriptTiming,
    WorkKind, WorkSession, WorkflowPhase, WorkflowState, mentioned_participants,
};

use super::{
    auth::GitHubAuthManager,
    prediction::{
        MAX_PREDICTION_CONTEXT_BYTES, OpenBufferContext, PredictionContextSnapshot,
        PredictionEngine, PredictionProviderConfig, PredictionSessionVersion,
        PredictionWorkContext,
    },
    store::SessionStore,
    voice::VoiceSession,
};
use ahead_agent::{HarnessController, path_is_allowed};
use ahead_core::search::WorkspaceFileIndex;

const MAX_PREDICTION_DISCUSSION_MESSAGES: usize = 6;
const MAX_PREDICTION_DISCUSSION_MESSAGE_BYTES: usize = 2 * 1024;
const MAX_PREDICTION_DISCUSSION_BYTES: usize = 8 * 1024;
const PREDICTION_DISCUSSION_TRUNCATION_MARKER: &str = " … [truncated]";

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamManifest {
    #[serde(default)]
    members: Vec<TeamMember>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamMember {
    github: String,
    display_name: String,
    role: SessionRole,
}

pub struct AheadSessionHost {
    store: Arc<RwLock<SessionStore>>,
    auth: Arc<RwLock<GitHubAuthManager>>,
    active_sessions: Arc<RwLock<HashMap<Id, SessionView>>>,
    active_voice_sessions: Arc<RwLock<HashMap<Id, Arc<VoiceSession>>>>,
    workspace_participants: Arc<RwLock<Vec<SessionParticipantRecord>>>,
    shared_terminal: Arc<RwLock<HashMap<Id, String>>>,
    tracker: Arc<RwLock<super::tracker::TrackerAdapter>>,
    review_snapshots: Arc<RwLock<HashMap<Id, super::collab::ReviewSnapshot>>>,
    workspace: Arc<RwLock<Option<std::path::PathBuf>>>,
    memory_write_lock: Mutex<()>,
    recovery_owner: Mutex<Option<super::recovery::RecoveryOwner>>,
    notification_sink: RwLock<Option<ahead_agent::HarnessNotificationSink>>,
    prediction_contexts:
        Mutex<VecDeque<(Id, PredictionSessionVersion, bool, String)>>,
    /// Streams conversations through the built-in runtime or an external ACP agent.
    harness: HarnessController,
}

fn recent_prediction_discussion(
    store: &SessionStore,
    session_id: &str,
    max_bytes: usize,
) -> Result<String> {
    const HEADER: &str = "Recent session discussion:\n";
    if max_bytes <= HEADER.len() {
        return Ok(String::new());
    }

    let messages = store
        .list_messages_page(session_id, None, 24)?
        .messages
        .into_iter()
        .filter(|message| {
            message.status == "complete"
                && matches!(message.role.as_str(), "human" | "agent")
                && !message.content.trim().is_empty()
        })
        .rev()
        .take(MAX_PREDICTION_DISCUSSION_MESSAGES)
        .collect::<Vec<_>>();
    if messages.is_empty() {
        return Ok(String::new());
    }

    let budget = max_bytes.min(MAX_PREDICTION_DISCUSSION_BYTES);
    let mut remaining = budget.saturating_sub(HEADER.len());
    let mut lines = Vec::new();
    for message in messages {
        let label = if message.role == "human" {
            "Human"
        } else {
            "Agent"
        };
        let prefix = format!("- {label}: ");
        let body_budget = remaining.saturating_sub(prefix.len() + 1);
        if body_budget == 0 {
            break;
        }

        let body = message
            .content
            .trim()
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\n', "\n  ");
        let body_limit = body_budget.min(MAX_PREDICTION_DISCUSSION_MESSAGE_BYTES);
        let truncated = body.len() > body_limit;
        let marker = if truncated
            && body_limit > PREDICTION_DISCUSSION_TRUNCATION_MARKER.len()
        {
            PREDICTION_DISCUSSION_TRUNCATION_MARKER
        } else {
            ""
        };
        let excerpt = utf8_prefix(&body, body_limit.saturating_sub(marker.len()));
        if excerpt.is_empty() {
            continue;
        }
        let line = format!("{prefix}{excerpt}{marker}\n");
        remaining = remaining.saturating_sub(line.len());
        lines.push(line);
        if remaining == 0 {
            break;
        }
    }

    if lines.is_empty() {
        return Ok(String::new());
    }
    lines.reverse();
    let mut discussion = String::with_capacity(budget.saturating_sub(remaining));
    discussion.push_str(HEADER);
    for line in lines {
        discussion.push_str(&line);
    }
    Ok(discussion)
}

fn utf8_prefix(value: &str, max_bytes: usize) -> &str {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

impl AheadSessionHost {
    pub fn new(store: SessionStore) -> Self {
        let auth_mgr = GitHubAuthManager::new();
        let user = auth_mgr.get_active_user(None);
        let host_record = SessionParticipantRecord {
            participant: Participant::Human {
                id: user.login.clone(),
                subject: user.email.clone().unwrap_or_default(),
                display_name: user
                    .name
                    .clone()
                    .unwrap_or_else(|| user.login.clone()),
            },
            role: SessionRole::Owner,
        };
        let store = Arc::new(RwLock::new(store));
        let harness = HarnessController::new(Arc::new(
            crate::ahead::store::SharedSessionStore(store.clone()),
        ));
        Self {
            store,
            auth: Arc::new(RwLock::new(auth_mgr)),
            active_sessions: Arc::new(RwLock::new(HashMap::new())),
            active_voice_sessions: Arc::new(RwLock::new(HashMap::new())),
            workspace_participants: Arc::new(RwLock::new(vec![host_record])),
            shared_terminal: Arc::new(RwLock::new(HashMap::new())),
            tracker: Arc::new(RwLock::new(super::tracker::TrackerAdapter::new())),
            review_snapshots: Arc::new(RwLock::new(HashMap::new())),
            workspace: Arc::new(RwLock::new(None)),
            memory_write_lock: Mutex::new(()),
            recovery_owner: Mutex::new(None),
            notification_sink: RwLock::new(None),
            prediction_contexts: Mutex::new(VecDeque::new()),
            harness,
        }
    }

    pub fn in_memory() -> Result<Self> {
        let store = SessionStore::in_memory()?;
        Ok(Self::new(store))
    }

    /// Binds the workspace used by the selected harness runtime.
    pub fn set_workspace(&self, workspace: std::path::PathBuf) {
        let mut owner = self.recovery_owner.lock();
        *owner = None;
        *self.workspace.write() = Some(workspace.clone());
        self.prediction_contexts.lock().clear();
        drop(owner);
        self.harness.set_workspace(workspace);
    }

    fn checked_mcp_workspace(&self, requested: &Path) -> Result<PathBuf> {
        let active = self
            .workspace
            .read()
            .clone()
            .context("Workspace must be initialized before managing MCP servers")?
            .canonicalize()
            .context("AHEAD workspace is unavailable")?;
        let requested = requested
            .canonicalize()
            .context("Requested MCP workspace is unavailable")?;
        anyhow::ensure!(
            active == requested,
            "MCP approval belongs to a different active workspace"
        );
        Ok(active)
    }

    fn with_recovery_owner<T>(
        &self,
        operation: impl FnOnce(
            &super::recovery::RecoveryOwner,
            &SessionStore,
        ) -> Result<T>,
    ) -> Result<T> {
        let mut owner = self.recovery_owner.lock();
        if owner.is_none() {
            let workspace = self
                .workspace
                .read()
                .clone()
                .context("editor recovery requires an active workspace")?;
            *owner = Some(super::recovery::RecoveryOwner::new(&workspace)?);
        }
        operation(
            owner
                .as_ref()
                .context("editor recovery owner unavailable")?,
            &self.store.read(),
        )
    }

    pub fn set_file_index(&self, index: Arc<WorkspaceFileIndex>) {
        self.harness.set_file_index(index);
    }

    /// Installs the UI notification sink used for streamed agent output.
    pub fn set_notification_sink(&self, sink: ahead_agent::HarnessNotificationSink) {
        *self.notification_sink.write() = Some(sink.clone());
        self.harness.set_notification_sink(sink);
    }

    fn notify_prediction_context_changed(&self, session_id: &str) {
        if let Some(sink) = self.notification_sink.read().clone() {
            sink(
                ahead_rpc::ahead::AheadNotification::PredictionContextChanged {
                    session_id: Some(session_id.to_string()),
                },
            );
        }
    }

    /// Shared streamed-harness controller (tests and status reporting).
    pub fn harness(&self) -> &HarnessController {
        &self.harness
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_auth_for_test(&self, auth: GitHubAuthManager) {
        *self.auth.write() = auth;
    }

    /// Handles an incoming AheadRequest
    pub fn handle_request(
        &self,
        request: AheadRequest,
    ) -> Result<serde_json::Value> {
        match request {
            AheadRequest::StartWork {
                work_kind,
                title,
                starting_point,
                work_item,
                harness,
                external_agent_id,
                parent_session_id,
            } => {
                let parent = if let Some(parent_session_id) =
                    parent_session_id.as_deref()
                {
                    anyhow::ensure!(
                        harness == Some(HarnessKind::ExternalAcp),
                        "Implementation handoff requires an external agent"
                    );
                    anyhow::ensure!(
                        external_agent_id.is_some(),
                        "Choose an installed external agent for the handoff"
                    );
                    let parent = self
                        .get_session(parent_session_id)?
                        .context("Parent AHEAD session not found")?;
                    let parent_backend =
                        self.store.read().get_harness_binding(parent_session_id)?;
                    anyhow::ensure!(
                        !parent_backend.as_ref().is_some_and(
                            |(_, backend)| backend.starts_with("external-agent")
                        ),
                        "Implementation handoff requires an AHEAD parent session"
                    );
                    anyhow::ensure!(
                        matches!(parent.session.lifecycle, SessionLifecycle::Active),
                        "Parent AHEAD session must be active"
                    );
                    anyhow::ensure!(
                        parent.task.intent == TaskIntent::Assistance,
                        "Teaching tasks cannot hand off implementation"
                    );
                    anyhow::ensure!(
                        parent.workflow.phase.id == "implement",
                        "Implementation handoff is available in the implementation phase"
                    );
                    Some(parent)
                } else {
                    None
                };
                let backend = harness
                    .map(|harness| {
                        Self::selected_harness_backend(
                            harness,
                            external_agent_id.as_deref(),
                        )
                    })
                    .transpose()?;
                let starting_point = if let Some(parent) = &parent {
                    self.implementation_handoff_context(parent, &starting_point)?
                } else {
                    starting_point
                };
                let view = self.start_work_with_binding(
                    work_kind.or_else(|| {
                        parent.as_ref().map(|view| view.session.work_kind)
                    }),
                    title,
                    starting_point,
                    work_item,
                    backend.as_deref(),
                    parent.as_ref().map(|view| view.task.id.as_str()),
                )?;
                Ok(serde_json::to_value(view)?)
            }
            AheadRequest::ListExternalAcpAdapters => {
                Ok(serde_json::to_value(ahead_agent::external_acp_adapters()?)?)
            }
            AheadRequest::SetExternalAcpAdapterInstalled {
                adapter_id,
                installed,
            } => {
                ahead_agent::set_external_acp_adapter_installed(
                    &adapter_id,
                    installed,
                )?;
                Ok(serde_json::Value::Null)
            }
            AheadRequest::ListMcpServerDeclarations { workspace } => {
                let workspace = self.checked_mcp_workspace(&workspace)?;
                Ok(serde_json::to_value(ahead_agent::mcp_server_declarations(
                    &workspace,
                )?)?)
            }
            AheadRequest::SetMcpServerApproval {
                workspace,
                server_id,
                expected_fingerprint,
                action,
            } => {
                let workspace = self.checked_mcp_workspace(&workspace)?;
                ahead_agent::set_mcp_server_approval(
                    &workspace,
                    &server_id,
                    &expected_fingerprint,
                    action,
                )?;
                Ok(serde_json::Value::Null)
            }
            AheadRequest::GetSession { session_id } => {
                let session = self.get_session(&session_id)?;
                Ok(serde_json::to_value(session)?)
            }
            AheadRequest::ListSessions => {
                Ok(serde_json::to_value(self.list_sessions()?)?)
            }
            AheadRequest::ListRecentWorkspaceFiles => {
                Ok(serde_json::to_value(self.list_recent_workspace_files()?)?)
            }
            AheadRequest::RecordRecentWorkspaceFile { path, opened_at } => {
                let path = self.canonical_workspace_file_path(&path)?;
                self.store
                    .write()
                    .record_recent_workspace_file(&path, opened_at)?;
                Ok(serde_json::Value::Null)
            }
            AheadRequest::ListEditorRecoveries => {
                self.with_recovery_owner(|owner, store| {
                    owner.reclaim_abandoned(store)?;
                    Ok(serde_json::to_value(
                        store.editor_recovery_summaries(&owner.id)?,
                    )?)
                })
            }
            AheadRequest::ReadEditorRecovery { buffer_id } => self
                .with_recovery_owner(|owner, store| {
                    Ok(serde_json::to_value(
                        store.editor_recovery(&owner.id, &buffer_id)?,
                    )?)
                }),
            AheadRequest::WriteEditorRecovery { snapshot } => self
                .with_recovery_owner(|owner, store| {
                    Ok(serde_json::to_value(
                        store.write_editor_recovery(&owner.id, &snapshot)?,
                    )?)
                }),
            AheadRequest::SearchMemory { query, limit } => {
                Ok(serde_json::to_value(self.search_memory(&query, limit)?)?)
            }
            AheadRequest::ReadMemory { scope } => {
                Ok(serde_json::to_value(self.read_memory(scope)?)?)
            }
            AheadRequest::WriteMemory {
                scope,
                message_id,
                content,
            } => Ok(serde_json::to_value(self.write_memory(
                scope,
                &message_id,
                &content,
            )?)?),
            AheadRequest::ReplaceMemory {
                scope,
                expected_sha256,
                content,
            } => Ok(serde_json::to_value(self.replace_memory(
                scope,
                &expected_sha256,
                &content,
            )?)?),
            AheadRequest::ArchiveSession { session_id } => {
                self.archive_session(&session_id)?;
                Ok(serde_json::json!({ "archived": true }))
            }
            AheadRequest::AdvancePhase {
                session_id,
                expected_revision,
                target_phase_id,
            } => Ok(serde_json::to_value(self.advance_phase(
                &session_id,
                expected_revision,
                target_phase_id,
            )?)?),
            AheadRequest::CreateAnchor {
                session_id,
                path,
                range,
                quote,
            } => Ok(serde_json::to_value(self.create_anchor(
                &session_id,
                path,
                range,
                quote,
                "human",
            )?)?),
            AheadRequest::RecordPredictionAcceptance {
                session_id,
                path,
                range,
                binding,
                model_context,
                accepted_text_sha256,
            } => Ok(serde_json::to_value(self.record_prediction_acceptance(
                &session_id,
                path,
                range,
                binding,
                model_context,
                accepted_text_sha256,
            )?)?),
            AheadRequest::ListPredictionAcceptances { session_id } => {
                self.get_session(&session_id)?
                    .context("Session not found")?;
                Ok(serde_json::to_value(
                    self.store.read().list_prediction_acceptances(&session_id)?,
                )?)
            }
            AheadRequest::CreateCodeComment {
                session_id,
                path,
                range,
                quote,
                source_sha256,
                body,
            } => Ok(serde_json::to_value(self.create_code_comment(
                &session_id,
                path,
                range,
                quote,
                source_sha256,
                body,
            )?)?),
            AheadRequest::ListCodeComments { session_id } => {
                self.get_session(&session_id)?
                    .context("Session not found")?;
                self.ensure_code_comments_supported(&session_id)?;
                Ok(serde_json::to_value(
                    self.store.read().list_code_comments(&session_id)?,
                )?)
            }
            AheadRequest::ResolveCodeComment {
                session_id,
                comment_id,
            } => {
                let actor_id = self.comment_actor(&session_id)?;
                Ok(serde_json::to_value(
                    self.store.read().resolve_code_comment(
                        &session_id,
                        &comment_id,
                        &actor_id,
                    )?,
                )?)
            }
            AheadRequest::ListAnchorsForPaths { paths } => {
                Ok(serde_json::to_value(self.anchors_for_paths(&paths)?)?)
            }
            AheadRequest::AgentTurnStart { request } => {
                let mut request = request;
                request.session_context.clear();
                let turn_id = self.start_streamed_agent_turn(request)?;
                Ok(serde_json::json!({ "turn_id": turn_id }))
            }
            AheadRequest::PostHumanMessage {
                session_id,
                content,
            } => Ok(serde_json::to_value(self.post_human_message_as(
                &session_id,
                &content,
                &self.auth.read().get_active_user(None).login,
                None,
            )?)?),
            AheadRequest::RecordVoiceTranscript {
                session_id,
                call_id,
                segment_id,
                speaker,
                start_ms,
                end_ms,
                content,
            } => Ok(serde_json::to_value(self.record_voice_transcript(
                &session_id,
                &call_id,
                &segment_id,
                speaker,
                start_ms,
                end_ms,
                &content,
            )?)?),
            AheadRequest::StartVoiceCall { session_id } => {
                Ok(serde_json::to_value(self.start_voice_call(&session_id)?)?)
            }
            AheadRequest::EndVoiceCall { lease } => {
                let ended = self.end_voice_call(&lease)?;
                Ok(serde_json::json!({ "ended": ended }))
            }
            AheadRequest::PublishSharedTerminal {
                session_id,
                content,
            } => {
                self.publish_shared_terminal(&session_id, content)?;
                Ok(serde_json::json!({ "published": true }))
            }
            AheadRequest::ShareSession { .. }
            | AheadRequest::StopSharingSession { .. }
            | AheadRequest::PublishSharedPresence { .. }
            | AheadRequest::GetSharedPresence { .. }
            | AheadRequest::ReadSharedBuffer { .. }
            | AheadRequest::ReplaceSharedBuffer { .. }
            | AheadRequest::JoinSharedSession { .. }
            | AheadRequest::PollSharedSession { .. }
            | AheadRequest::PostSharedHumanMessage { .. }
            | AheadRequest::StartSharedAgentTurn { .. }
            | AheadRequest::AddSessionParticipant { .. }
            | AheadRequest::RevokeSessionParticipant { .. }
            | AheadRequest::LeaveSharedSession { .. } => {
                bail!("Sharing is handled by the proxy connection")
            }
            AheadRequest::DiscardSharedRemoteRecovery { .. } => {
                bail!("Shared edit recovery is handled by the proxy connection")
            }
            AheadRequest::AgentTurnCancel { session_id } => {
                let cancelled = self.harness.cancel_turn(&session_id)?;
                Ok(serde_json::json!({ "cancelled": cancelled }))
            }
            AheadRequest::AgentSessionPrepare { session_id } => {
                self.harness.prepare_external_session(&session_id)?;
                Ok(serde_json::json!({ "ready": true }))
            }
            AheadRequest::AgentAuthenticate {
                session_id,
                method_id,
            } => {
                self.harness
                    .authenticate_external_agent(&session_id, &method_id)?;
                Ok(serde_json::json!({ "authenticated": true }))
            }
            AheadRequest::AgentTerminalAuthLaunch {
                session_id,
                method_id,
            } => Ok(serde_json::to_value(
                self.harness
                    .external_agent_terminal_auth_launch(&session_id, &method_id)?,
            )?),
            AheadRequest::AgentTerminalAuthComplete {
                session_id,
                succeeded,
            } => {
                self.harness
                    .complete_external_agent_terminal_auth(&session_id, succeeded)?;
                Ok(serde_json::json!({ "reconnected": succeeded }))
            }
            AheadRequest::AgentConfigOptionSet {
                session_id,
                config_id,
                value,
            } => {
                self.harness
                    .set_config_option(&session_id, &config_id, &value)?;
                Ok(serde_json::json!({ "updated": true }))
            }
            AheadRequest::AgentTurnRetry {
                session_id,
                turn_id,
            } => {
                let request = self.harness.retry_request(&session_id, &turn_id)?;
                let new_turn_id = self.start_streamed_agent_turn(request)?;
                self.harness.forget_turn_request(&turn_id)?;
                Ok(serde_json::json!({ "turn_id": new_turn_id }))
            }
            AheadRequest::AgentUserInputAnswer {
                session_id,
                request_id,
                answers,
            } => {
                self.harness
                    .answer_user_input(&session_id, &request_id, answers)?;
                Ok(serde_json::json!({ "answered": true }))
            }
            AheadRequest::AgentBufferSnapshotsResponse {
                session_id,
                turn_id,
                request_id,
                buffers,
            } => {
                self.harness.answer_buffer_snapshots(
                    &session_id,
                    &turn_id,
                    &request_id,
                    buffers,
                    None,
                )?;
                Ok(serde_json::json!({ "received": true }))
            }
            AheadRequest::AgentBufferSnapshotsFailed {
                session_id,
                turn_id,
                request_id,
                message,
            } => {
                self.harness.answer_buffer_snapshots(
                    &session_id,
                    &turn_id,
                    &request_id,
                    Vec::new(),
                    Some(message),
                )?;
                Ok(serde_json::json!({ "received": true }))
            }
            AheadRequest::AgentPresentationResponse {
                session_id,
                turn_id,
                request_id,
                applied,
                message,
            } => {
                self.harness.answer_editor_presentation(
                    &session_id,
                    &turn_id,
                    &request_id,
                    applied,
                    message,
                )?;
                Ok(serde_json::json!({ "received": true }))
            }
            AheadRequest::HarnessStatus { session_id } => {
                Ok(self.harness.harness_status(&session_id)?)
            }
            AheadRequest::ConversationMessagesPage {
                session_id,
                before,
                limit,
            } => Ok(serde_json::to_value(self.harness.messages_page(
                &session_id,
                before,
                limit,
            )?)?),
            AheadRequest::AgentRuntimeState { session_id } => {
                Ok(serde_json::to_value(
                    self.store.read().get_agent_runtime_state(&session_id)?,
                )?)
            }
            AheadRequest::AgentSkills {
                session_id,
                model,
                model_provider,
            } => Ok(serde_json::to_value(self.harness.available_skills(
                &session_id,
                model.as_deref(),
                model_provider.as_deref(),
            )?)?),
            AheadRequest::AgentReasoningOptions {
                session_id,
                model,
                model_provider,
            } => Ok(serde_json::to_value(self.harness.reasoning_options(
                &session_id,
                &model,
                model_provider.as_deref(),
            )?)?),
            AheadRequest::WorkItemCreate { session_id, title } => {
                let owner = self.auth.read().get_active_user(None).login;
                let item = self.store.read().create_work_item(
                    &session_id,
                    &title,
                    &owner,
                )?;
                self.notify_prediction_context_changed(&session_id);
                Ok(serde_json::to_value(item)?)
            }
            AheadRequest::WorkItemList { session_id } => {
                let items = self.store.read().list_work_items(&session_id)?;
                Ok(serde_json::to_value(items)?)
            }
            AheadRequest::WorkItemSetStatus { item_id, status } => {
                let item =
                    self.store.read().set_work_item_status(&item_id, status)?;
                self.notify_prediction_context_changed(&item.session_id);
                Ok(serde_json::to_value(item)?)
            }
            AheadRequest::WorkItemNote {
                item_id,
                kind,
                body_markdown,
            } => {
                let owner = self.auth.read().get_active_user(None).login;
                let session_id =
                    self.work_item_session(&item_id)?.unwrap_or_default();
                let event = self.store.read().add_work_item_event(
                    &item_id,
                    &session_id,
                    &kind,
                    &body_markdown,
                    &owner,
                )?;
                self.notify_prediction_context_changed(&session_id);
                Ok(serde_json::to_value(event)?)
            }
            AheadRequest::WorkItemClose {
                item_id,
                summary_markdown,
                issue_ref,
                follow_ups_json,
                skip_reason,
            } => {
                let session_id = self.work_item_session(&item_id)?;
                let closeout = self.store.read().close_work_item(
                    &item_id,
                    &summary_markdown,
                    issue_ref,
                    follow_ups_json,
                    skip_reason,
                )?;
                if let Some(session_id) = session_id {
                    self.notify_prediction_context_changed(&session_id);
                }
                Ok(serde_json::to_value(closeout)?)
            }
            AheadRequest::ConversationSummarize {
                session_id,
                phase,
                summary_markdown,
                message_id_range,
            } => {
                let summary = self.store.read().save_conversation_summary(
                    &session_id,
                    &phase,
                    &summary_markdown,
                    &message_id_range,
                )?;
                self.notify_prediction_context_changed(&session_id);
                Ok(serde_json::to_value(summary)?)
            }
            AheadRequest::ConversationList { session_id } => {
                let summaries =
                    self.store.read().list_conversation_summaries(&session_id)?;
                Ok(serde_json::to_value(summaries)?)
            }
            AheadRequest::SessionExport { session_id } => {
                let bundle = self.session_export(&session_id)?;
                Ok(serde_json::to_value(bundle)?)
            }
            AheadRequest::SessionRestore { bundle } => {
                let view = self.session_restore(bundle)?;
                Ok(serde_json::to_value(view)?)
            }
            AheadRequest::ReviewCapture {
                session_id,
                code_tree_sha,
                implementer_ids,
                findings,
            } => {
                let snapshot = self.review_capture(
                    &session_id,
                    code_tree_sha,
                    implementer_ids,
                    findings,
                )?;
                Ok(serde_json::to_value(snapshot)?)
            }
            AheadRequest::ReviewAttest {
                snapshot_id,
                reviewer_id,
            } => {
                let snapshot = self.review_attest(&snapshot_id, &reviewer_id)?;
                Ok(serde_json::to_value(snapshot)?)
            }
            AheadRequest::ReviewGet { session_id } => {
                let snapshot = self.review_get(&session_id)?;
                Ok(serde_json::to_value(snapshot)?)
            }
            AheadRequest::TrackerStage {
                session_id,
                issue,
                payload,
                observed_body_sha,
            } => {
                let outbox_id = self.tracker.write().stage_update(
                    session_id,
                    issue,
                    super::tracker::TrackerUpdatePayload {
                        title: payload.title,
                        body_markdown: payload.body_markdown,
                        labels: None,
                        state: payload.state,
                    },
                    observed_body_sha,
                )?;
                Ok(serde_json::to_value(
                    serde_json::json!({ "outbox_id": outbox_id }),
                )?)
            }
            AheadRequest::TrackerAuthorize { outbox_id } => {
                let authorizer = self.auth.read().get_active_user(None).login;
                self.tracker
                    .write()
                    .authorize_update(&outbox_id, &authorizer)?;
                Ok(serde_json::json!({ "status": "authorized" }))
            }
            AheadRequest::TrackerPublish { outbox_id } => {
                let token = self.auth.read().get_access_token().map(str::to_string);
                let Some(token) = token else {
                    anyhow::bail!(
                        "Tracker publish needs a signed-in GitHub identity (token unavailable)"
                    );
                };
                let status = self.tracker.write().publish_to_github(
                    &outbox_id,
                    &token,
                    "https://api.github.com",
                )?;
                Ok(serde_json::to_value(status)?)
            }
            AheadRequest::TrackerGet { outbox_id } => {
                let item = self
                    .tracker
                    .read()
                    .get_item(&outbox_id)
                    .cloned()
                    .context("Outbox item not found")?;
                Ok(serde_json::to_value(item)?)
            }
            AheadRequest::RequestPrediction { request } => {
                let res = self.request_prediction(request, &[])?;
                Ok(serde_json::to_value(res)?)
            }
            AheadRequest::VoiceControl {
                session_id,
                call_id,
                connection_epoch,
                control,
            } => {
                self.handle_voice_control(
                    &session_id,
                    &call_id,
                    connection_epoch,
                    control,
                )?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            AheadRequest::GitHubAuthStart => {
                let device_code =
                    format!("{:x}", sha2::Sha256::digest(Uuid::new_v4().as_bytes()));
                let user_code = format!(
                    "{}-{}",
                    device_code[0..4].to_uppercase(),
                    device_code[4..8].to_uppercase()
                );
                let resp = ahead_rpc::ahead::GitHubDeviceCodeResponse {
                    device_code,
                    user_code,
                    verification_uri: "https://github.com/login/device".into(),
                    expires_in: 900,
                    interval: 5,
                };
                Ok(serde_json::to_value(resp)?)
            }
            AheadRequest::GitHubAuthPoll { device_code: _ } => {
                let user = self.auth.read().get_active_user(None);
                Ok(serde_json::to_value(user)?)
            }
            AheadRequest::GitHubAuthSignInWithToken { token } => {
                let user = self.auth.write().sign_in_with_token(token)?;
                Ok(serde_json::to_value(user)?)
            }
            AheadRequest::GitHubAuthDetectCli => {
                let user = self.auth.write().detect_github_cli()?;
                Ok(serde_json::to_value(user)?)
            }
            AheadRequest::GitHubAuthSignOut => {
                self.auth.write().sign_out()?;
                let user = self.auth.read().get_active_user(None);
                Ok(serde_json::to_value(user)?)
            }
            AheadRequest::GetAuthenticatedUser => {
                let user = self.auth.read().get_active_user(None);
                Ok(serde_json::to_value(user)?)
            }
            AheadRequest::GetWorkspaceParticipants => {
                let parts = self.get_workspace_participants()?;
                Ok(serde_json::to_value(parts)?)
            }
            AheadRequest::AddWorkspaceParticipant { user_handle, role } => {
                let parts = self.add_workspace_participant(user_handle, role)?;
                Ok(serde_json::to_value(parts)?)
            }
            AheadRequest::RevokeWorkspaceParticipant { user_handle } => {
                let parts = self.revoke_workspace_participant(&user_handle)?;
                Ok(serde_json::to_value(parts)?)
            }
        }
    }

    fn team_members(&self) -> Result<Option<Vec<TeamMember>>> {
        let Some(workspace) = self.workspace.read().clone() else {
            return Ok(None);
        };
        let Some(content) =
            ahead_core::config::read_ahead_config(&workspace, "team.toml")?
        else {
            return Ok(None);
        };
        let manifest: TeamManifest =
            toml::from_str(&content).context("Invalid .ahead/team.toml")?;
        anyhow::ensure!(manifest.members.len() <= 128, "Too many team members");
        let mut handles = std::collections::HashSet::new();
        for member in &manifest.members {
            let handle = member.github.as_str();
            anyhow::ensure!(
                !handle.is_empty()
                    && handle.len() <= 64
                    && handle
                        .as_bytes()
                        .first()
                        .is_some_and(u8::is_ascii_alphanumeric)
                    && handle
                        .as_bytes()
                        .last()
                        .is_some_and(u8::is_ascii_alphanumeric)
                    && handle
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                    && handles.insert(handle.to_ascii_lowercase()),
                "Invalid or duplicate GitHub username in .ahead/team.toml"
            );
            anyhow::ensure!(
                !member.display_name.trim().is_empty()
                    && member.display_name.len() <= 128
                    && member.role != SessionRole::Owner,
                "Invalid team member name or role in .ahead/team.toml"
            );
        }
        Ok(Some(manifest.members))
    }

    pub(crate) fn ensure_team_member(&self, login: &str) -> Result<()> {
        if self.workspace.read().is_none() {
            return Ok(());
        }
        let members = self
            .team_members()?
            .context("Add .ahead/team.toml before sharing")?;
        anyhow::ensure!(
            members
                .iter()
                .any(|member| member.github.eq_ignore_ascii_case(login)),
            "This GitHub user is not in .ahead/team.toml"
        );
        Ok(())
    }

    pub fn get_workspace_participants(
        &self,
    ) -> Result<Vec<SessionParticipantRecord>> {
        let mut parts = self.workspace_participants.read().clone();
        let user = self.auth.read().get_active_user(None);
        if let Some(owner) = parts.iter_mut().find(|p| p.role == SessionRole::Owner)
        {
            owner.participant = Participant::Human {
                id: user.login.clone(),
                subject: if user.id != 0 {
                    format!("github:{}", user.id)
                } else {
                    user.email.clone().unwrap_or_default()
                },
                display_name: user
                    .name
                    .clone()
                    .unwrap_or_else(|| user.login.clone()),
            };
        }
        if let Some(members) = self.team_members()? {
            parts.retain(|record| record.role == SessionRole::Owner);
            parts.extend(members.into_iter().map(|member| {
                SessionParticipantRecord {
                    participant: Participant::Human {
                        id: member.github.clone(),
                        subject: member.github,
                        display_name: member.display_name,
                    },
                    role: member.role,
                }
            }));
        }
        Ok(parts)
    }

    pub fn add_workspace_participant(
        &self,
        user_handle: String,
        role: SessionRole,
    ) -> Result<Vec<SessionParticipantRecord>> {
        let handle = user_handle
            .trim()
            .strip_prefix('@')
            .unwrap_or(user_handle.trim())
            .to_string();
        if handle.is_empty()
            || handle.len() > 64
            || !handle
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !handle
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !handle
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            anyhow::bail!(
                "Enter a GitHub username (e.g. octocat), not an email address"
            );
        }
        let mut parts = self.workspace_participants.write();
        if let Some(existing) = parts
            .iter_mut()
            .find(|p| p.participant.id().eq_ignore_ascii_case(&handle))
        {
            existing.role = role;
        } else {
            parts.push(SessionParticipantRecord {
                participant: Participant::Human {
                    id: handle.clone(),
                    subject: handle.clone(),
                    display_name: format!("@{handle}"),
                },
                role,
            });
        }
        Ok(parts.clone())
    }

    pub fn revoke_workspace_participant(
        &self,
        user_handle: &str,
    ) -> Result<Vec<SessionParticipantRecord>> {
        let mut parts = self.workspace_participants.write();
        let handle = user_handle.trim().trim_start_matches('@');
        parts.retain(|p| {
            !p.participant.id().eq_ignore_ascii_case(handle)
                || p.role == SessionRole::Owner
        });
        Ok(parts.clone())
    }

    pub fn start_work(
        &self,
        requested_work_kind: Option<WorkKind>,
        title: String,
        starting_point: String,
        work_item: Option<GithubIssueRef>,
    ) -> Result<SessionView> {
        self.start_work_with_binding(
            requested_work_kind,
            title,
            starting_point,
            work_item,
            None,
            None,
        )
    }

    fn start_work_with_binding(
        &self,
        requested_work_kind: Option<WorkKind>,
        title: String,
        starting_point: String,
        work_item: Option<GithubIssueRef>,
        initial_backend: Option<&str>,
        parent_task_id: Option<&str>,
    ) -> Result<SessionView> {
        let work_kind = requested_work_kind
            .unwrap_or_else(|| WorkKind::infer_from_request(&starting_point));
        let intent = if parent_task_id.is_some() {
            TaskIntent::Assistance
        } else {
            TaskIntent::infer_from_request(&starting_point)
        };
        let initial_phase = Self::initial_phase(work_kind, intent);
        let session_id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();

        let user = self.auth.read().get_active_user(None);
        let owner_id = user.login;
        let session = WorkSession {
            id: session_id.clone(),
            project_id: "project-local".to_string(),
            worktree_id: "worktree-local".to_string(),
            work_kind,
            title: title.clone(),
            owner_id: owner_id.clone(),
            lifecycle: SessionLifecycle::Active,
            policy: SessionPolicySnapshot::default(),
            revision: 1,
            created_at: now,
        };

        let task_id = Uuid::new_v4().to_string();
        let learning_arc = if intent == TaskIntent::Teaching {
            let arc_id = Uuid::new_v4().to_string();
            Some(LearningArc {
                id: arc_id.clone(),
                task_id: task_id.clone(),
                mission: starting_point.clone(),
                current_concept_id: None,
                state: "introduced".to_string(),
                records: vec![LearningRecord {
                    id: Uuid::new_v4().to_string(),
                    arc_id,
                    kind: "mission".to_string(),
                    content: starting_point.clone(),
                    source_refs: Vec::new(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                }],
                created_at: chrono::Utc::now().to_rfc3339(),
                updated_at: chrono::Utc::now().to_rfc3339(),
            })
        } else {
            None
        };
        let task = SessionTask {
            id: task_id,
            session_id: session_id.clone(),
            intent,
            work_kind,
            title: title.clone(),
            objective: starting_point.clone(),
            parent_task_id: parent_task_id.map(str::to_string),
            learning_arc_id: learning_arc.as_ref().map(|arc| arc.id.clone()),
            created_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
        };

        let workflow = WorkflowState {
            revision: 1,
            definition_version: "2026-09-17-v1".to_string(),
            phase: initial_phase,
            primary_work_item: work_item,
            current_artifact_ids: Vec::new(),
            approvals: Vec::new(),
        };

        let mut participants = self.get_workspace_participants()?;
        if self.team_members()?.is_some() {
            participants.retain(|record| record.role == SessionRole::Owner);
        }
        participants.push(SessionParticipantRecord {
            participant: Participant::Ai {
                id: "ai-assistant".to_string(),
                backend_id: "ahead-loop".to_string(),
                on_behalf_of: owner_id,
                display_name: "AHEAD Pair".to_string(),
            },
            role: SessionRole::Editor,
        });

        let view = SessionView {
            session,
            task,
            learning_arc,
            workflow,
            participants,
        };

        {
            let mut store = self.store.write();
            if let Some(backend) = initial_backend {
                store.insert_session_with_harness_binding(&view, backend)?;
            } else {
                store.insert_session(&view)?;
            }
        }

        {
            let mut active = self.active_sessions.write();
            active.insert(session_id.clone(), view.clone());
        }

        // Initialize voice session
        {
            let voice_id = format!("voice-{session_id}");
            let voice = Arc::new(VoiceSession::new(session_id.clone(), voice_id));
            let mut voice_map = self.active_voice_sessions.write();
            voice_map.insert(session_id, voice);
        }

        Ok(view)
    }

    fn selected_harness_backend(
        harness: HarnessKind,
        external_agent_id: Option<&str>,
    ) -> Result<String> {
        match harness {
            HarnessKind::Ahead => Ok("ahead-pending".to_string()),
            HarnessKind::ExternalAcp => {
                let Some(adapter_id) = external_agent_id else {
                    return Ok("external-agent-pending".to_string());
                };
                anyhow::ensure!(
                    !adapter_id.trim().is_empty() && adapter_id == adapter_id.trim(),
                    "External ACP adapter ID must not be empty or padded"
                );
                anyhow::ensure!(
                    ahead_agent::external_acp_adapter_is_installed(adapter_id)?,
                    "ACP agent `{adapter_id}` is not installed in AHEAD"
                );
                Ok(format!("external-agent:{adapter_id}"))
            }
        }
    }

    fn initial_phase(work_kind: WorkKind, intent: TaskIntent) -> WorkflowPhase {
        if intent == TaskIntent::Teaching {
            return WorkflowPhase {
                id: "investigation-scrutinize".to_string(),
                title: "Learn & Gather Evidence".to_string(),
                visit: 1,
            };
        }

        match work_kind {
            WorkKind::InternalImprovement => WorkflowPhase {
                id: "implement".to_string(),
                title: "Write & Check".to_string(),
                visit: 1,
            },
            WorkKind::ProductChange => WorkflowPhase {
                id: "plan".to_string(),
                title: "Questions & Outline".to_string(),
                visit: 1,
            },
            WorkKind::CorrectiveDebugging | WorkKind::Investigation => {
                WorkflowPhase {
                    id: "investigation-scrutinize".to_string(),
                    title: "Explore & Gather Evidence".to_string(),
                    visit: 1,
                }
            }
            WorkKind::Decision => WorkflowPhase {
                id: "decision-framing".to_string(),
                title: "Frame the Decision".to_string(),
                visit: 1,
            },
            WorkKind::OperationalStabilization => WorkflowPhase {
                id: "implement".to_string(),
                title: "Stabilize & Verify".to_string(),
                visit: 1,
            },
        }
    }

    pub fn get_session(&self, session_id: &str) -> Result<Option<SessionView>> {
        {
            let active = self.active_sessions.read();
            if let Some(v) = active.get(session_id) {
                return Ok(Some(v.clone()));
            }
        }
        let store = self.store.read();
        store.get_session(session_id)
    }
    pub fn list_sessions(&self) -> Result<Vec<SessionListItem>> {
        let store = self.store.read();
        let mut summaries = store.list_sessions()?;
        let active = self.active_sessions.read();
        for (id, view) in active.iter() {
            if let Some(existing) = summaries.iter_mut().find(|item| item.id == *id)
            {
                existing.title = view.session.title.clone();
                existing.lifecycle = view.session.lifecycle.clone();
                existing.created_at = view.session.created_at.clone();
            } else {
                summaries.push(SessionListItem {
                    id: id.clone(),
                    title: view.session.title.clone(),
                    lifecycle: view.session.lifecycle.clone(),
                    created_at: view.session.created_at.clone(),
                    updated_at: view.session.created_at.clone(),
                    backend: store
                        .get_harness_binding(id)?
                        .map(|(_, backend)| backend),
                    parent_session_id: None,
                });
            }
        }
        summaries.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(summaries)
    }

    fn search_memory(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryExcerpt>> {
        if self.workspace.read().is_none() {
            return Ok(Vec::new());
        }
        let _memory_guard = self.memory_write_lock.lock();
        for scope in [MemoryScope::Project, MemoryScope::User] {
            self.sync_memory_source(scope)?;
        }
        self.store
            .read()
            .search_memory(query, limit)?
            .into_iter()
            .map(|hit| {
                let (scope, source) = match hit.scope.as_str() {
                    "project" => (MemoryScope::Project, ".ahead/memories/MEMORY.md"),
                    "user" => (MemoryScope::User, "~/.ahead/memories/MEMORY.md"),
                    scope => bail!("unknown AHEAD memory scope `{scope}`"),
                };
                Ok(MemoryExcerpt {
                    scope,
                    source: source.to_string(),
                    line: hit.line,
                    excerpt: hit.excerpt,
                })
            })
            .collect()
    }

    fn sync_memory_source(&self, scope: MemoryScope) -> Result<Option<String>> {
        let path = self.memory_source_path(scope, false)?;
        let content = Self::read_memory_content(&path)?;
        self.store.read().sync_memory_content(
            scope.as_str(),
            &path,
            content.as_deref(),
        )
    }

    pub(crate) fn sync_memory_sources(&self) {
        let _memory_guard = self.memory_write_lock.lock();
        for scope in [MemoryScope::Project, MemoryScope::User] {
            if let Err(error) = self.sync_memory_source(scope) {
                tracing::warn!(
                    scope = scope.as_str(),
                    cause = %error.root_cause(),
                    "Could not index AHEAD memory"
                );
            }
        }
    }

    fn list_recent_workspace_files(&self) -> Result<Vec<RepoPath>> {
        let paths = self.store.read().recent_workspace_files()?;
        let mut recent = Vec::with_capacity(paths.len());
        for path in paths {
            match self.canonical_workspace_file_path(&path) {
                Ok(path) if !recent.contains(&path) => recent.push(path),
                Ok(_) => {}
                Err(error) => {
                    eprintln!(
                        "AHEAD skipped a stale recent workspace file: {error:#}"
                    );
                }
            }
        }
        Ok(recent)
    }

    fn canonical_workspace_file_path(&self, relative_path: &str) -> Result<String> {
        let relative = Path::new(relative_path);
        if relative_path.is_empty()
            || relative_path.contains('\\')
            || relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            bail!("recent workspace file path must be a normalized relative path");
        }

        let workspace =
            self.workspace.read().clone().context(
                "Workspace must be initialized before recent files are used",
            )?;
        let workspace = std::fs::canonicalize(workspace)
            .context("AHEAD workspace is unavailable")?;
        let path = std::fs::canonicalize(workspace.join(relative))
            .context("Recent workspace file is unavailable")?;
        if !path.starts_with(&workspace) || !path.is_file() {
            bail!("recent workspace file must be a file inside the workspace");
        }
        let relative = path
            .strip_prefix(&workspace)
            .context("Recent workspace file left its workspace")?;
        let relative = relative
            .to_str()
            .context("Recent workspace file path is not valid UTF-8")?;
        Ok(relative.replace(std::path::MAIN_SEPARATOR, "/"))
    }

    fn memory_source_path(
        &self,
        scope: MemoryScope,
        create_parent: bool,
    ) -> Result<PathBuf> {
        let Some(workspace) = self.workspace.read().clone() else {
            bail!("cannot access memory without an active workspace");
        };
        let source = ahead_agent::memory_sources(&workspace)
            .into_iter()
            .find(|source| source.scope == scope)
            .context("AHEAD memory source is unavailable")?;
        if !source.path.is_absolute() {
            bail!("AHEAD memory path must be absolute");
        }
        let parent = source
            .path
            .parent()
            .context("AHEAD memory file has no parent directory")?;
        let ahead_directory = parent
            .parent()
            .context("AHEAD memory directory has no AHEAD root")?;
        match std::fs::symlink_metadata(ahead_directory) {
            Ok(metadata) if Self::memory_metadata_is_reparse(&metadata) => {
                bail!("AHEAD memory directory must not resolve through a symlink");
            }
            Ok(metadata) if !metadata.is_dir() => {
                bail!("AHEAD memory root is not a directory");
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).context("Failed to inspect AHEAD memory root");
            }
        }
        match std::fs::symlink_metadata(parent) {
            Ok(metadata) if Self::memory_metadata_is_reparse(&metadata) => {
                bail!("AHEAD memory directory must not resolve through a symlink");
            }
            Ok(metadata) if !metadata.is_dir() => {
                bail!("AHEAD memory path parent is not a directory");
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .context("Failed to inspect AHEAD memory directory");
            }
        }
        if create_parent {
            #[cfg(unix)]
            {
                if scope == MemoryScope::User {
                    let home = ahead_directory
                        .parent()
                        .context("AHEAD user memory has no home directory")?;
                    std::fs::create_dir_all(home)
                        .context("Failed to create AHEAD user home")?;
                }
                Self::open_memory_directory(&source.path, true)?
                    .context("AHEAD memory directory was not created")?;
            }
            #[cfg(windows)]
            Self::open_memory_directory(&source.path, true)?
                .context("AHEAD memory directory was not created")?;
            #[cfg(not(any(unix, windows)))]
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create memory directory {}", parent.display())
            })?;
        } else if !parent.exists() {
            return Ok(source.path);
        }

        let expected_parent = match scope {
            MemoryScope::Project => std::fs::canonicalize(&workspace)
                .context("AHEAD workspace is unavailable")?
                .join(".ahead/memories"),
            MemoryScope::User => {
                let user_home = ahead_directory
                    .parent()
                    .context("AHEAD user memory has no home directory")?;
                std::fs::canonicalize(user_home)
                    .context("AHEAD user home is unavailable")?
                    .join(".ahead/memories")
            }
        };
        let canonical_parent = std::fs::canonicalize(parent)
            .context("AHEAD memory directory is unavailable")?;
        if canonical_parent != expected_parent {
            bail!("AHEAD memory directory must not resolve through a symlink");
        }
        match std::fs::symlink_metadata(&source.path) {
            Ok(metadata) if Self::memory_metadata_is_reparse(&metadata) => {
                bail!("AHEAD memory file must not be a symlink");
            }
            Ok(metadata) if !metadata.is_file() => {
                bail!("AHEAD memory path is not a regular file");
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Failed to inspect AHEAD memory file {}",
                        source.path.display()
                    )
                });
            }
        }
        Ok(source.path)
    }

    #[cfg(unix)]
    fn open_memory_directory(
        path: &Path,
        create: bool,
    ) -> Result<Option<std::fs::File>> {
        use rustix::fs::{Mode, OFlags, mkdirat, openat};

        let root = path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .context("AHEAD memory file has no workspace or user root")?;
        let root = match std::fs::canonicalize(root) {
            Ok(root) => root,
            Err(error)
                if !create && error.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(None);
            }
            Err(error) => {
                return Err(error).context("AHEAD memory root is unavailable");
            }
        };
        let mut directory = ahead_core::secure_fs::open_canonical_directory(&root)
            .context("Failed to open AHEAD memory root")?;
        for name in [".ahead", "memories"] {
            if create {
                match mkdirat(&directory, name, Mode::from_raw_mode(0o700)) {
                    Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                    Err(error) => {
                        return Err(error)
                            .context("Failed to create AHEAD memory directory");
                    }
                }
            }
            directory = match openat(
                &directory,
                name,
                OFlags::RDONLY
                    | OFlags::DIRECTORY
                    | OFlags::NOFOLLOW
                    | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(directory) => directory.into(),
                Err(rustix::io::Errno::NOENT) if !create => return Ok(None),
                Err(error) => {
                    return Err(error).context(
                        "AHEAD memory directory must not resolve through a symlink",
                    );
                }
            };
        }
        Ok(Some(directory))
    }

    #[cfg(windows)]
    fn open_memory_directory(
        path: &Path,
        create: bool,
    ) -> Result<Option<std::fs::File>> {
        super::memory_fs::open_memory_directory(path, create).context(
            "Failed to open AHEAD memory directory without following reparse points",
        )
    }

    fn read_memory_content(path: &std::path::Path) -> Result<Option<String>> {
        #[cfg(unix)]
        {
            let Some(directory) = Self::open_memory_directory(path, false)? else {
                return Ok(None);
            };
            Self::read_memory_content_at(&directory, path)
        }
        #[cfg(windows)]
        {
            let Some(directory) = Self::open_memory_directory(path, false)? else {
                return Ok(None);
            };
            Self::read_memory_content_at(&directory, path)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let mut options = OpenOptions::new();
            options.read(true);
            let mut file = match options.open(path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(None);
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "Failed to read AHEAD memory file {}",
                            path.display()
                        )
                    });
                }
            };
            Self::read_open_memory_content(&mut file, path).map(Some)
        }
    }

    #[cfg(unix)]
    fn read_memory_content_at(
        directory: &std::fs::File,
        path: &Path,
    ) -> Result<Option<String>> {
        use rustix::fs::{Mode, OFlags, openat};

        let mut file: std::fs::File = match openat(
            directory,
            "MEMORY.md",
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(file) => file.into(),
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(error) => {
                return Err(error).context("Failed to read AHEAD memory file");
            }
        };
        Self::read_open_memory_content(&mut file, path).map(Some)
    }

    #[cfg(windows)]
    fn read_memory_content_at(
        directory: &std::fs::File,
        path: &Path,
    ) -> Result<Option<String>> {
        let Some(mut file) = super::memory_fs::open_existing_memory_file(directory)?
        else {
            return Ok(None);
        };
        Self::read_open_memory_content(&mut file, path).map(Some)
    }

    fn read_open_memory_content(
        file: &mut std::fs::File,
        path: &std::path::Path,
    ) -> Result<String> {
        let metadata = file.metadata()?;
        if !metadata.is_file() || Self::memory_metadata_is_reparse(&metadata) {
            bail!("AHEAD memory source must be a regular file");
        }
        const MEMORY_DOCUMENT_LIMIT: u64 =
            ahead_rpc::ahead::MEMORY_DOCUMENT_MAX_BYTES as u64;
        if metadata.len() > MEMORY_DOCUMENT_LIMIT {
            bail!("AHEAD memory document exceeds the 32 KiB limit");
        }
        file.seek(SeekFrom::Start(0)).with_context(|| {
            format!("Failed to read AHEAD memory file {}", path.display())
        })?;
        let mut content = String::new();
        file.take(MEMORY_DOCUMENT_LIMIT + 1)
            .read_to_string(&mut content)
            .with_context(|| {
                format!("Failed to read AHEAD memory file {}", path.display())
            })?;
        if content.len() as u64 > MEMORY_DOCUMENT_LIMIT {
            bail!("AHEAD memory document exceeds the 32 KiB limit");
        }
        Ok(content)
    }

    fn memory_metadata_is_reparse(metadata: &std::fs::Metadata) -> bool {
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

            metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        }
        #[cfg(not(windows))]
        {
            metadata.file_type().is_symlink()
        }
    }

    fn read_memory(&self, scope: MemoryScope) -> Result<MemoryDocument> {
        let _read_guard = self.memory_write_lock.lock();
        let path = self.memory_source_path(scope, false)?;
        let content = Self::read_memory_content(&path)?.unwrap_or_default();
        let source = match scope {
            MemoryScope::Project => ".ahead/memories/MEMORY.md",
            MemoryScope::User => "~/.ahead/memories/MEMORY.md",
        };
        Ok(MemoryDocument {
            scope,
            source: source.to_string(),
            sha256: format!("{:x}", sha2::Sha256::digest(content.as_bytes())),
            content,
        })
    }

    fn replace_memory(
        &self,
        scope: MemoryScope,
        expected_sha256: &str,
        content: &str,
    ) -> Result<MemoryWriteResult> {
        if content.trim().is_empty() {
            bail!("memory replacement cannot be empty");
        }
        if content.len() > ahead_rpc::ahead::MEMORY_DOCUMENT_MAX_BYTES {
            bail!("memory replacement exceeds the 32 KiB limit");
        }
        if content.contains('\0') {
            bail!("memory replacement contains a NUL byte");
        }
        if expected_sha256.len() != 64
            || !expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            bail!("memory snapshot hash is invalid");
        }

        let _write_guard = self.memory_write_lock.lock();
        let path = self.memory_source_path(scope, true)?;
        #[cfg(unix)]
        let directory = Self::open_memory_directory(&path, true)?
            .context("AHEAD memory directory is unavailable")?;
        #[cfg(windows)]
        let directory = Self::open_memory_directory(&path, true)?
            .context("AHEAD memory directory is unavailable")?;
        #[cfg(unix)]
        let current =
            Self::read_memory_content_at(&directory, &path)?.unwrap_or_default();
        #[cfg(windows)]
        let current =
            Self::read_memory_content_at(&directory, &path)?.unwrap_or_default();
        #[cfg(not(any(unix, windows)))]
        let current = Self::read_memory_content(&path)?.unwrap_or_default();
        let current_sha256 =
            format!("{:x}", sha2::Sha256::digest(current.as_bytes()));
        if current_sha256 != expected_sha256 {
            bail!(
                "AHEAD memory changed since review; read it again before replacing"
            );
        }

        #[cfg(unix)]
        Self::replace_memory_content_at(&directory, content)?;
        #[cfg(windows)]
        Self::replace_memory_content_at(&directory, content)?;
        #[cfg(not(any(unix, windows)))]
        {
            let parent = path
                .parent()
                .context("AHEAD memory file has no parent directory")?;
            let temporary_path =
                parent.join(format!(".MEMORY.md.{}.tmp", Uuid::new_v4()));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            let mut file = options.open(&temporary_path).with_context(|| {
                format!(
                    "Failed to create replacement memory file {}",
                    temporary_path.display()
                )
            })?;
            let write_result = file
                .write_all(content.as_bytes())
                .and_then(|()| file.sync_all());
            drop(file);
            if let Err(error) = write_result {
                match std::fs::remove_file(&temporary_path) {
                    Ok(()) => {}
                    Err(cleanup_error)
                        if cleanup_error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(cleanup_error) => {
                        bail!(
                            "Failed to write memory replacement: {error}; failed to remove temporary file: {cleanup_error}"
                        );
                    }
                }
                return Err(error).context("Failed to write memory replacement");
            }
            if let Err(error) = std::fs::rename(&temporary_path, &path) {
                match std::fs::remove_file(&temporary_path) {
                    Ok(()) => {}
                    Err(cleanup_error)
                        if cleanup_error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(cleanup_error) => {
                        bail!(
                            "Failed to replace AHEAD memory file: {error}; failed to remove temporary file: {cleanup_error}"
                        );
                    }
                }
                return Err(error).context("Failed to replace AHEAD memory file");
            }
        }

        let index = self.store.read().sync_memory_content(
            scope.as_str(),
            &path,
            Some(content),
        );
        let (indexed, warning) = match index {
            Ok(_) => (true, None),
            Err(error) => (false, Some(error.to_string())),
        };
        Ok(MemoryWriteResult { indexed, warning })
    }

    #[cfg(unix)]
    fn replace_memory_content_at(
        directory: &std::fs::File,
        content: &str,
    ) -> Result<()> {
        use rustix::fs::{AtFlags, Mode, OFlags, openat, renameat, unlinkat};

        let temporary_name = format!(".MEMORY.md.{}.tmp", Uuid::new_v4());
        let result = (|| -> Result<()> {
            let mut temporary: std::fs::File = openat(
                directory,
                temporary_name.as_str(),
                OFlags::WRONLY
                    | OFlags::CREATE
                    | OFlags::EXCL
                    | OFlags::NOFOLLOW
                    | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            )?
            .into();
            temporary.write_all(content.as_bytes())?;
            temporary.sync_all()?;
            renameat(directory, temporary_name.as_str(), directory, "MEMORY.md")?;
            directory.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            match unlinkat(directory, temporary_name.as_str(), AtFlags::empty()) {
                Ok(()) | Err(rustix::io::Errno::NOENT) => {}
                Err(error) => {
                    tracing::warn!(%error, "Failed to remove temporary AHEAD memory file")
                }
            }
        }
        result.context("Failed to replace AHEAD memory file")
    }

    #[cfg(windows)]
    fn replace_memory_content_at(
        directory: &std::fs::File,
        content: &str,
    ) -> Result<()> {
        super::memory_fs::replace_memory_file(directory, content.as_bytes())
            .context("Failed to replace AHEAD memory file")
    }

    fn write_memory(
        &self,
        scope: MemoryScope,
        message_id: &str,
        content: &str,
    ) -> Result<MemoryWriteResult> {
        let content = content.trim();
        if content.is_empty() {
            bail!("memory note cannot be empty");
        }
        if content.len() > 8 * 1024 {
            bail!("memory note exceeds the 8 KiB limit");
        }
        if content.contains('\0') {
            bail!("memory note contains a NUL byte");
        }
        if message_id.is_empty()
            || !message_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            bail!("memory source message id is invalid");
        }
        let _write_guard = self.memory_write_lock.lock();
        let path = self.memory_source_path(scope, true)?;

        let marker = format!(
            "<!-- AHEAD memory source:{}:{} -->",
            scope.as_str(),
            message_id
        );
        #[cfg(unix)]
        let directory = Self::open_memory_directory(&path, true)?
            .context("AHEAD memory directory is unavailable")?;
        #[cfg(windows)]
        let directory = Self::open_memory_directory(&path, true)?
            .context("AHEAD memory directory is unavailable")?;
        #[cfg(unix)]
        let mut file = Self::open_memory_append_at(&directory)?;
        #[cfg(windows)]
        let mut file = Self::open_memory_append_at(&directory)?;
        #[cfg(not(any(unix, windows)))]
        let mut file = {
            let mut options = OpenOptions::new();
            options.read(true).create(true).append(true);
            options.open(&path).with_context(|| {
                format!("Failed to open AHEAD memory file {}", path.display())
            })?
        };
        let existing = Self::read_open_memory_content(&mut file, &path)?;
        if existing.lines().any(|line| line.trim() == marker) {
            let index = self.store.read().sync_memory_content(
                scope.as_str(),
                &path,
                Some(&existing),
            );
            let (indexed, warning) = match index {
                Ok(_) => (true, None),
                Err(error) => (false, Some(error.to_string())),
            };
            return Ok(MemoryWriteResult { indexed, warning });
        }

        let timestamp = chrono::Utc::now().to_rfc3339();
        let separator = if file.metadata()?.len() == 0 {
            ""
        } else {
            "\n\n"
        };
        let entry = format!(
            "{separator}{marker}\n## Saved from AHEAD · {timestamp}\n\n{content}\n"
        );
        if file.metadata()?.len().saturating_add(entry.len() as u64)
            > ahead_rpc::ahead::MEMORY_DOCUMENT_MAX_BYTES as u64
        {
            bail!("AHEAD memory document would exceed the 32 KiB limit");
        }
        file.write_all(entry.as_bytes()).with_context(|| {
            format!("Failed to append AHEAD memory file {}", path.display())
        })?;
        file.sync_data().with_context(|| {
            format!("Failed to sync AHEAD memory file {}", path.display())
        })?;

        let index = self.store.read().sync_memory_content(
            scope.as_str(),
            &path,
            Some(&format!("{existing}{entry}")),
        );
        let (indexed, warning) = match index {
            Ok(_) => (true, None),
            Err(error) => (false, Some(error.to_string())),
        };
        Ok(MemoryWriteResult { indexed, warning })
    }

    #[cfg(unix)]
    fn open_memory_append_at(directory: &std::fs::File) -> Result<std::fs::File> {
        use rustix::fs::{Mode, OFlags, openat};
        use std::os::unix::fs::MetadataExt;

        let file: std::fs::File = openat(
            directory,
            "MEMORY.md",
            OFlags::RDWR
                | OFlags::CREATE
                | OFlags::APPEND
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK
                | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .context("Failed to open AHEAD memory file")?
        .into();
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            bail!("AHEAD memory source must be a regular, singly-linked file");
        }
        Ok(file)
    }

    #[cfg(windows)]
    fn open_memory_append_at(directory: &std::fs::File) -> Result<std::fs::File> {
        super::memory_fs::open_memory_append(directory)
            .context("Failed to open AHEAD memory file")
    }

    pub fn archive_session(&self, session_id: &str) -> Result<()> {
        self.store.read().archive_session(session_id)?;
        self.active_sessions.write().remove(session_id);
        self.shared_terminal.write().remove(session_id);
        self.notify_prediction_context_changed(session_id);
        Ok(())
    }

    pub fn advance_phase(
        &self,
        session_id: &str,
        expected_revision: Revision,
        target_phase_id: String,
    ) -> Result<WorkflowState> {
        let title = format!("Phase {target_phase_id}");
        let new_phase = WorkflowPhase {
            id: target_phase_id,
            title,
            visit: 1,
        };

        let next_wf = {
            let mut store = self.store.write();
            store.advance_workflow_phase(session_id, expected_revision, new_phase)?
        };

        let mut active = self.active_sessions.write();
        if let Some(view) = active.get_mut(session_id) {
            view.workflow = next_wf.clone();
            view.session.revision += 1;
        }
        drop(active);
        self.notify_prediction_context_changed(session_id);
        Ok(next_wf)
    }

    /// Resolves a work item's owning session for event attribution.
    pub fn work_item_session(&self, item_id: &str) -> Result<Option<String>> {
        self.store.read().work_item_session(item_id)
    }

    pub fn create_anchor(
        &self,
        session_id: &str,
        path: RepoPath,
        range: DisplayRange,
        quote: String,
        actor_id: &str,
    ) -> Result<CodeAnchor> {
        let anchor_id = Uuid::new_v4().to_string();
        let quote_hash = format!(
            "{:x}",
            <sha2::Sha256 as sha2::Digest>::digest(quote.as_bytes())
        );

        let anchor = CodeAnchor {
            id: anchor_id,
            session_id: session_id.to_string(),
            actor_id: actor_id.to_string(),
            path,
            range,
            quote_hash,
            surrounding_context: Some(quote),
        };

        let mut store = self.store.write();
        store.record_edit_anchor(&anchor)?;

        Ok(anchor)
    }

    fn record_prediction_acceptance(
        &self,
        session_id: &str,
        path: RepoPath,
        range: DisplayRange,
        binding: PredictionBinding,
        model_context: Option<PredictionModelContext>,
        accepted_text_sha256: String,
    ) -> Result<PredictionAcceptance> {
        let actor_id = self.comment_actor(session_id)?;
        let path = self.canonical_workspace_file_path(&path)?;
        anyhow::ensure!(binding.version == 1, "Unsupported prediction binding");
        anyhow::ensure!(
            !binding.request_id.is_empty() && binding.request_id.len() <= 128,
            "Invalid prediction request ID"
        );
        anyhow::ensure!(
            binding.context_sha256.len() == 64
                && binding
                    .context_sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit()),
            "Invalid prediction context hash"
        );
        anyhow::ensure!(
            (range.start.line, range.start.col) < (range.end.line, range.end.col),
            "Prediction acceptance must cover inserted text"
        );
        anyhow::ensure!(
            accepted_text_sha256.len() == 64
                && accepted_text_sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit()),
            "Invalid accepted-text hash"
        );
        if let Some(model) = &model_context {
            anyhow::ensure!(
                !model.provider_id.trim().is_empty()
                    && model.provider_id.len() <= 256
                    && !model.model_id.trim().is_empty()
                    && model.model_id.len() <= 256,
                "Invalid prediction model identity"
            );
        }
        let acceptance = PredictionAcceptance {
            id: format!("prediction-acceptance:{}", binding.request_id),
            session_id: session_id.to_string(),
            actor_id,
            path,
            range,
            binding,
            model_context,
            accepted_text_sha256,
            accepted_at: chrono::Utc::now().to_rfc3339(),
        };
        self.store
            .read()
            .record_prediction_acceptance(&acceptance)?;
        Ok(acceptance)
    }

    pub fn create_code_comment(
        &self,
        session_id: &str,
        path: RepoPath,
        range: DisplayRange,
        quote: String,
        source_sha256: String,
        body: String,
    ) -> Result<CodeComment> {
        let actor_id = self.comment_actor(session_id)?;
        self.insert_code_comment(
            session_id,
            path,
            range,
            quote,
            source_sha256,
            body,
            actor_id,
        )
    }

    pub(crate) fn create_code_comment_as(
        &self,
        session_id: &str,
        path: RepoPath,
        range: DisplayRange,
        quote: String,
        source_sha256: String,
        body: String,
        actor_id: &str,
        github_id: u64,
    ) -> Result<CodeComment> {
        self.shared_comment_actor(session_id, actor_id, github_id)?;
        self.insert_code_comment(
            session_id,
            path,
            range,
            quote,
            source_sha256,
            body,
            actor_id.to_string(),
        )
    }

    fn insert_code_comment(
        &self,
        session_id: &str,
        path: RepoPath,
        range: DisplayRange,
        quote: String,
        source_sha256: String,
        body: String,
        actor_id: String,
    ) -> Result<CodeComment> {
        let path = self.canonical_workspace_file_path(&path)?;
        let body = body.trim();
        anyhow::ensure!(
            !body.is_empty() && body.len() <= 8_000,
            "Comment must be 1–8000 bytes"
        );
        anyhow::ensure!(
            !quote.is_empty() && quote.len() <= 4_000,
            "Select up to 4000 bytes of code"
        );
        anyhow::ensure!(
            (range.start.line, range.start.col) < (range.end.line, range.end.col),
            "Select a nonempty code range"
        );
        anyhow::ensure!(
            source_sha256.len() == 64
                && source_sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Invalid source revision"
        );
        let comment = CodeComment {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            actor_id,
            path,
            range,
            quote,
            source_sha256,
            body: body.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            resolved_at: None,
            resolved_by: None,
        };
        self.store.read().insert_code_comment(&comment)?;
        Ok(comment)
    }

    pub(crate) fn shared_code_comments(
        &self,
        session_id: &str,
    ) -> Result<Vec<CodeComment>> {
        self.shareable_session(session_id)?;
        self.store.read().list_code_comments(session_id)
    }

    pub(crate) fn publish_shared_terminal(
        &self,
        session_id: &str,
        content: String,
    ) -> Result<()> {
        self.can_host_share(session_id)?;
        anyhow::ensure!(
            content.len() <= 128 * 1024,
            "Terminal snapshot is too large"
        );
        self.shared_terminal
            .write()
            .insert(session_id.to_string(), content);
        Ok(())
    }

    pub(crate) fn shared_terminal_output(&self, session_id: &str) -> Result<String> {
        self.shareable_session(session_id)?;
        Ok(self
            .shared_terminal
            .read()
            .get(session_id)
            .cloned()
            .unwrap_or_default())
    }

    pub(crate) fn clear_shared_terminal(&self, session_id: &str) {
        self.shared_terminal.write().remove(session_id);
    }

    pub(crate) fn resolve_code_comment_as(
        &self,
        session_id: &str,
        comment_id: &str,
        actor_id: &str,
        github_id: u64,
    ) -> Result<CodeComment> {
        self.shared_comment_actor(session_id, actor_id, github_id)?;
        self.store
            .read()
            .resolve_code_comment(session_id, comment_id, actor_id)
    }

    fn shared_comment_actor(
        &self,
        session_id: &str,
        actor_id: &str,
        github_id: u64,
    ) -> Result<()> {
        let view = self.shareable_session(session_id)?;
        anyhow::ensure!(
            view.participants.iter().any(|record| {
                matches!(&record.participant, Participant::Human { id, subject, .. }
                if id.eq_ignore_ascii_case(actor_id)
                    && subject == &format!("github:{github_id}"))
                    && matches!(
                        record.role,
                        SessionRole::Owner
                            | SessionRole::Editor
                            | SessionRole::Reviewer
                    )
            }),
            "This participant cannot comment in the session"
        );
        Ok(())
    }

    fn comment_actor(&self, session_id: &str) -> Result<String> {
        let view = self.get_session(session_id)?.context("Session not found")?;
        anyhow::ensure!(
            self.list_sessions()?
                .iter()
                .any(|session| session.id == session_id),
            "Cannot comment on an archived session"
        );
        self.ensure_code_comments_supported(session_id)?;
        let actor_id = self.auth.read().get_active_user(None).login;
        anyhow::ensure!(
            view.participants.iter().any(|record| matches!(
                &record.participant, Participant::Human { id, .. } if id.eq_ignore_ascii_case(&actor_id)
            )),
            "Only a session participant can comment"
        );
        Ok(actor_id)
    }

    fn ensure_code_comments_supported(&self, session_id: &str) -> Result<()> {
        anyhow::ensure!(
            !self
                .store
                .read()
                .get_harness_binding(session_id)?
                .is_some_and(|(_, backend)| backend.starts_with("external-agent")),
            "Code comments are only available in managed AHEAD sessions"
        );
        Ok(())
    }

    pub(crate) fn shareable_session(&self, session_id: &str) -> Result<SessionView> {
        let mut view = self.get_session(session_id)?.context("Session not found")?;
        // The durable membership row is authoritative during invitation and revocation.
        view.participants = self
            .store
            .read()
            .get_session(session_id)?
            .context("Session not found")?
            .participants;
        anyhow::ensure!(
            matches!(view.session.lifecycle, SessionLifecycle::Active),
            "Only an active session can be shared"
        );
        anyhow::ensure!(
            !self
                .store
                .read()
                .get_harness_binding(session_id)?
                .is_some_and(|(_, backend)| backend.starts_with("external-agent")),
            "Only a managed AHEAD session can be shared"
        );
        Ok(view)
    }

    pub(crate) fn can_host_share(&self, session_id: &str) -> Result<SessionView> {
        let view = self.shareable_session(session_id)?;
        let auth = self.auth.read();
        anyhow::ensure!(
            auth.is_authenticated()
                && auth
                    .get_active_user(None)
                    .login
                    .eq_ignore_ascii_case(&view.session.owner_id),
            "Sign in as the session owner to share it"
        );
        if let Some(Participant::Human { subject, .. }) = view
            .participants
            .iter()
            .find(|record| record.role == SessionRole::Owner)
            .map(|record| &record.participant)
        {
            if subject.starts_with("github:") {
                anyhow::ensure!(
                    subject == &format!("github:{}", auth.get_active_user(None).id),
                    "This GitHub account is not the session owner"
                );
            }
        }
        Ok(view)
    }

    fn refresh_session_participants(&self, session_id: &str) -> Result<SessionView> {
        let updated = self
            .store
            .read()
            .get_session(session_id)?
            .context("Session not found")?;
        if let Some(active) = self.active_sessions.write().get_mut(session_id) {
            active.participants = updated.participants.clone();
            active.session.revision = updated.session.revision;
        }
        self.get_session(session_id)?.context("Session not found")
    }

    pub(crate) fn bind_owner_identity(&self, session_id: &str) -> Result<()> {
        let view = self.can_host_share(session_id)?;
        let user = self.auth.read().get_active_user(None);
        anyhow::ensure!(
            user.id != 0,
            "Session owner needs a verified GitHub identity"
        );
        let record = SessionParticipantRecord {
            participant: Participant::Human {
                id: view.session.owner_id,
                subject: format!("github:{}", user.id),
                display_name: user.name.unwrap_or(user.login),
            },
            role: SessionRole::Owner,
        };
        self.store
            .write()
            .upsert_session_participant(session_id, &record)?;
        self.refresh_session_participants(session_id)?;
        Ok(())
    }

    pub fn add_session_participant_verified(
        &self,
        session_id: &str,
        user: ahead_rpc::ahead::GitHubUser,
        role: SessionRole,
    ) -> Result<SessionView> {
        let view = self.can_host_share(session_id)?;
        anyhow::ensure!(user.id != 0, "GitHub identity is invalid");
        anyhow::ensure!(
            !user.login.eq_ignore_ascii_case(&view.session.owner_id),
            "Cannot change the session owner"
        );
        anyhow::ensure!(role != SessionRole::Owner, "Cannot invite another owner");
        let display_name = if self.workspace.read().is_some() {
            let members = self.team_members()?.context(
                "Add .ahead/team.toml before inviting session participants",
            )?;
            members
                .into_iter()
                .find(|member| member.github.eq_ignore_ascii_case(&user.login))
                .map(|member| member.display_name)
                .context("This GitHub user is not in .ahead/team.toml")?
        } else {
            user.name.unwrap_or_else(|| format!("@{}", user.login))
        };
        let record = SessionParticipantRecord {
            participant: Participant::Human {
                id: user.login.clone(),
                subject: format!("github:{}", user.id),
                display_name,
            },
            role,
        };
        self.store
            .write()
            .upsert_session_participant(session_id, &record)?;
        self.refresh_session_participants(session_id)
    }

    pub(crate) fn revoke_session_participant(
        &self,
        session_id: &str,
        user_handle: &str,
    ) -> Result<SessionView> {
        let view = self.can_host_share(session_id)?;
        let handle = user_handle.trim().trim_start_matches('@');
        anyhow::ensure!(
            !handle.eq_ignore_ascii_case(&view.session.owner_id),
            "Cannot remove the session owner"
        );
        anyhow::ensure!(view.participants.iter().any(|record| {
            matches!(&record.participant, Participant::Human { id, .. } if id.eq_ignore_ascii_case(handle))
        }), "This user is not a session participant");
        self.store
            .write()
            .delete_session_participant(session_id, handle)?;
        self.refresh_session_participants(session_id)
    }

    pub(crate) fn share_join_token(&self) -> Result<String> {
        let auth = self.auth.read();
        anyhow::ensure!(auth.is_authenticated(), "Sign in to GitHub before joining");
        auth.get_access_token()
            .map(str::to_string)
            .context("GitHub sign-in has no access token")
    }

    pub(crate) fn shared_messages_after(
        &self,
        session_id: &str,
        sequence: i64,
    ) -> Result<Vec<ConversationMessage>> {
        self.shareable_session(session_id)?;
        let store = self.store.read();
        let mut messages = store.list_messages_after(session_id, sequence, 10)?;
        if messages.is_empty() {
            if let Some(agent) = store.latest_agent_message(session_id)? {
                messages.push(agent);
            }
        }
        Ok(messages)
    }

    pub(crate) fn post_human_message_as(
        &self,
        session_id: &str,
        content: &str,
        actor_id: &str,
        github_id: Option<u64>,
    ) -> Result<ConversationMessage> {
        anyhow::ensure!(!content.trim().is_empty(), "Message is empty");
        anyhow::ensure!(content.len() <= 64 * 1024, "Message is too long");
        let view = self.shareable_session(session_id)?;
        let sender = view.participants.iter().find(|record| {
            matches!(&record.participant, Participant::Human { id, subject, .. }
                if id.eq_ignore_ascii_case(actor_id)
                    && github_id.is_none_or(|github_id| subject == &format!("github:{github_id}")))
        });
        anyhow::ensure!(
            sender.is_some_and(|record| matches!(
                record.role,
                SessionRole::Owner | SessionRole::Editor | SessionRole::Reviewer
            )),
            "This participant cannot send session messages"
        );
        let recipients =
            mentioned_participants(content, &view.participants, actor_id);
        anyhow::ensure!(
            !recipients.is_empty(),
            "Address another session participant with @ to send a human-only message"
        );
        let store = self.store.write();
        let message = ConversationMessage {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            turn_id: Uuid::new_v4().to_string(),
            sequence: store.next_message_sequence(session_id)?,
            role: format!("human_to:{}", recipients.join(",")),
            actor_id: actor_id.to_string(),
            content: content.to_string(),
            status: "complete".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            voice_timing: None,
        };
        store.upsert_message(&message)?;
        drop(store);
        if let Some(sink) = self.notification_sink.read().clone() {
            sink(
                ahead_rpc::ahead::AheadNotification::ConversationMessageAdded {
                    message: message.clone(),
                },
            );
        }
        Ok(message)
    }

    pub(crate) fn record_voice_transcript(
        &self,
        session_id: &str,
        call_id: &str,
        segment_id: &str,
        speaker: VoiceTranscriptSpeaker,
        start_ms: u64,
        end_ms: u64,
        content: &str,
    ) -> Result<ConversationMessage> {
        anyhow::ensure!(!content.trim().is_empty(), "Voice transcript is empty");
        anyhow::ensure!(content.len() <= 64 * 1024, "Voice transcript is too long");
        anyhow::ensure!(
            !call_id.trim().is_empty()
                && call_id.len() <= 256
                && call_id.chars().all(|character| !character.is_control()),
            "Voice call ID is invalid"
        );
        let segment_id = Uuid::parse_str(segment_id)
            .context("Voice transcript segment ID is invalid")?
            .to_string();
        anyhow::ensure!(
            start_ms <= end_ms
                && i64::try_from(start_ms).is_ok()
                && i64::try_from(end_ms).is_ok(),
            "Voice transcript timing is invalid"
        );

        let view = self.managed_voice_session(session_id)?;
        anyhow::ensure!(
            matches!(view.session.lifecycle, SessionLifecycle::Active),
            "Voice transcripts require an active session"
        );
        let actor_id = self.auth.read().get_active_user(None).login;

        let (role, message_actor) = match speaker {
            VoiceTranscriptSpeaker::Human => ("human", actor_id),
            VoiceTranscriptSpeaker::Agent => ("agent", AHEAD_ACTOR_ID.to_string()),
        };
        let message_id = format!("voice-{segment_id}");
        let store = self.store.write();
        if let Some(existing) = store.get_message(&message_id)? {
            anyhow::ensure!(
                existing.session_id == session_id
                    && existing.turn_id == format!("voice:{call_id}")
                    && existing.role == role
                    && existing.actor_id == message_actor
                    && existing.content == content,
                "Voice transcript segment ID was reused with different content"
            );
            let expected_timing = VoiceTranscriptTiming {
                call_id: call_id.to_string(),
                start_ms,
                end_ms,
            };
            if let Some(existing_timing) = existing.voice_timing.as_ref() {
                anyhow::ensure!(
                    existing_timing == &expected_timing,
                    "Voice transcript segment ID was reused with different timing"
                );
                return Ok(existing);
            }
            let mut existing = existing;
            existing.voice_timing = Some(expected_timing);
            store.upsert_voice_transcript(&existing)?;
            return Ok(existing);
        }
        let message = ConversationMessage {
            id: message_id,
            session_id: session_id.to_string(),
            turn_id: format!("voice:{call_id}"),
            sequence: store.next_message_sequence(session_id)?,
            role: role.to_string(),
            actor_id: message_actor,
            content: content.to_string(),
            status: "complete".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            voice_timing: Some(VoiceTranscriptTiming {
                call_id: call_id.to_string(),
                start_ms,
                end_ms,
            }),
        };
        store.upsert_voice_transcript(&message)?;
        drop(store);
        if let Some(sink) = self.notification_sink.read().clone() {
            sink(
                ahead_rpc::ahead::AheadNotification::ConversationMessageAdded {
                    message: message.clone(),
                },
            );
        }
        Ok(message)
    }

    pub(crate) fn start_shared_agent_turn(
        &self,
        session_id: &str,
        content: String,
        actor_id: &str,
        github_id: u64,
    ) -> Result<Id> {
        let view = self.shareable_session(session_id)?;
        anyhow::ensure!(!content.trim().is_empty(), "Message is empty");
        anyhow::ensure!(content.len() <= 64 * 1024, "Message is too long");
        self.start_streamed_agent_turn_as(
            ahead_rpc::ahead::AgentTurnRequestDto {
                session_id: session_id.to_string(),
                thread_id: format!("thread-{session_id}"),
                harness: HarnessKind::Ahead,
                external_agent_id: None,
                model: None,
                model_provider: None,
                model_context_window: None,
                reasoning_effort: None,
                user_message: content,
                session_context: String::new(),
                context: ahead_rpc::ahead::TurnEditorContext {
                    active_path: String::new(),
                    caret: ahead_rpc::ahead::DisplayPosition { line: 0, col: 0 },
                    selection: None,
                    file_content: String::new(),
                    selected_excerpt: None,
                    visible_end: None,
                    attached_anchor_ids: Vec::new(),
                    attached_files: Vec::new(),
                    attached_memories: Vec::new(),
                },
                invariants: Vec::new(),
                cwd: None,
                expected_policy_sha256: view.session.policy.sha256,
                read_only: false,
                scope: None,
            },
            actor_id,
            Some(github_id),
        )
    }

    /// Drops uncommitted attribution anchors on `paths` once they are committed.
    /// Git blame becomes authoritative from that point on.
    pub fn clear_committed_anchors(
        &self,
        paths: &[String],
        head_contents: &std::collections::HashMap<String, String>,
    ) -> Result<usize> {
        let mut store = self.store.write();
        store.clear_anchors_for_paths(paths, head_contents)
    }

    /// Uncommitted attribution anchors on the given paths, across sessions.
    pub fn anchors_for_paths(&self, paths: &[String]) -> Result<Vec<CodeAnchor>> {
        let store = self.store.read();
        store.list_anchors_for_paths(paths)
    }

    fn durable_prediction_context(&self, session_id: &str) -> Result<String> {
        let store = self.store.read();
        Self::build_durable_prediction_context(&store, session_id, usize::MAX)
    }

    fn cached_prediction_context(
        &self,
        session_id: &str,
    ) -> Result<(PredictionSessionVersion, String)> {
        self.cached_prediction_context_with_plan_compaction(session_id, false)
    }

    fn cached_compacted_prediction_context(
        &self,
        session_id: &str,
    ) -> Result<(PredictionSessionVersion, String)> {
        self.cached_prediction_context_with_plan_compaction(session_id, true)
    }

    fn can_compact_prediction_context(error: &anyhow::Error) -> bool {
        error.chain().any(|cause| {
            let message = cause.to_string();
            message.contains("Prediction durable work exceeds its context budget")
                || message.contains("Prediction summary exceeds its context budget")
        })
    }

    fn cached_prediction_context_with_plan_compaction(
        &self,
        session_id: &str,
        compact_plan: bool,
    ) -> Result<(PredictionSessionVersion, String)> {
        let store = self.store.read();
        let version = PredictionSessionVersion {
            format: 1,
            revision: store.prediction_context_revision(session_id)?,
        };
        let mut contexts = self.prediction_contexts.lock();
        if let Some(index) =
            contexts
                .iter()
                .position(|(id, cached_version, compact, _)| {
                    id == session_id
                        && cached_version == &version
                        && compact == &compact_plan
                })
        {
            let entry = contexts
                .remove(index)
                .context("prediction cache entry disappeared")?;
            let context = entry.3.clone();
            contexts.push_back(entry);
            return Ok((version, context));
        }
        let context = if compact_plan {
            Self::build_compacted_prediction_context(
                &store,
                session_id,
                MAX_PREDICTION_CONTEXT_BYTES,
            )?
        } else {
            match Self::build_durable_prediction_context(
                &store,
                session_id,
                MAX_PREDICTION_CONTEXT_BYTES,
            ) {
                Ok(context) => context,
                Err(error) if Self::can_compact_prediction_context(&error) => {
                    Self::build_compacted_prediction_context(
                        &store,
                        session_id,
                        MAX_PREDICTION_CONTEXT_BYTES,
                    )?
                }
                Err(error) => return Err(error),
            }
        };
        anyhow::ensure!(
            store.prediction_context_revision(session_id)? == version.revision,
            "prediction session context changed during assembly"
        );
        contexts.retain(|(id, cached_version, compact, _)| {
            id != session_id
                || (cached_version == &version && compact != &compact_plan)
        });
        if contexts.len() >= 8 {
            contexts.pop_front();
        }
        contexts.push_back((
            session_id.to_string(),
            version,
            compact_plan,
            context.clone(),
        ));
        Ok((version, context))
    }

    fn build_durable_prediction_context(
        store: &SessionStore,
        session_id: &str,
        max_bytes: usize,
    ) -> Result<String> {
        Self::build_prediction_context_with_plan_mode(
            store, session_id, max_bytes, false,
        )
    }

    fn build_compacted_prediction_context(
        store: &SessionStore,
        session_id: &str,
        max_bytes: usize,
    ) -> Result<String> {
        Self::build_prediction_context_with_plan_mode(
            store, session_id, max_bytes, true,
        )
    }

    fn build_prediction_context_with_plan_mode(
        store: &SessionStore,
        session_id: &str,
        max_bytes: usize,
        compact_plan: bool,
    ) -> Result<String> {
        let items = store.prediction_work_items(session_id, max_bytes)?;
        // Summaries have no audience metadata yet; after a human-only message,
        // do not trust them as model-facing context.
        let include_summaries = !store.has_human_only_messages(session_id)?;
        let mut context = String::new();
        if !items.is_empty() {
            context.push_str("Work items:\n");
            for item in &items {
                context.push_str(&format!("- [{:?}] {}\n", item.status, item.title));
                anyhow::ensure!(
                    context.len() <= max_bytes,
                    "Prediction durable work exceeds its context budget; no required context was dropped"
                );
                let remaining_bytes = max_bytes.saturating_sub(context.len());
                let events = if compact_plan {
                    store.compact_prediction_work_item_events(
                        &item.id,
                        remaining_bytes,
                    )?
                } else {
                    store.prediction_work_item_events(&item.id, remaining_bytes)?
                };
                for event in events {
                    context.push_str(&format!(
                        "  - {}: {}\n",
                        event.kind, event.body_markdown
                    ));
                    anyhow::ensure!(
                        context.len() <= max_bytes,
                        "Prediction durable work exceeds its context budget; no required context was dropped"
                    );
                    if let Some(artifact_id) = event.artifact_id {
                        let reference =
                            format!("    Related artifact ID: {artifact_id}\n");
                        if context.len().saturating_add(reference.len()) <= max_bytes
                        {
                            context.push_str(&reference);
                        }
                    }
                }
                anyhow::ensure!(
                    context.len() <= max_bytes,
                    "Prediction durable work exceeds its context budget; no required context was dropped"
                );
            }
        }
        let plan_budget = max_bytes.saturating_sub(context.len());
        let plan = if compact_plan {
            None
        } else {
            store.prediction_agent_plan(session_id, plan_budget)?
        };
        match plan {
            Some(plan) if !plan.is_empty() => {
                context.push_str("Current agent plan:\n");
                for step in plan {
                    context.push_str(&format!(
                        "- [{}] {}\n",
                        step.status, step.content
                    ));
                    anyhow::ensure!(
                        context.len() <= max_bytes,
                        "Prediction plan exceeds its context budget; no required context was dropped"
                    );
                }
            }
            Some(_) => {}
            None => {
                let summary =
                    store.prediction_agent_plan_summary(session_id, plan_budget)?;
                if !summary.status_counts.is_empty() {
                    context.push_str(
                        "Current agent plan (faithful compact summary):\n",
                    );
                    let has_active_steps = !summary.active_steps.is_empty();
                    let total = summary.status_counts.values().sum::<usize>();
                    let progress = summary
                        .status_counts
                        .iter()
                        .map(|(status, count)| format!("{status}: {count}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    context.push_str(&format!(
                        "Progress: {progress} ({total} steps).\n"
                    ));
                    if let Some(opening_step) = summary.opening_step {
                        let label = if !has_active_steps
                            && opening_step.status.eq_ignore_ascii_case("pending")
                        {
                            "Next plan step"
                        } else {
                            "Plan opening"
                        };
                        context.push_str(&format!(
                            "{label}: [{}] {}\n",
                            opening_step.status, opening_step.content
                        ));
                    }
                    for step in summary.active_steps {
                        context.push_str(&format!(
                            "Active plan step: [{}; priority {}] {}\n",
                            step.status, step.priority, step.content
                        ));
                    }
                    if let Some(step) = summary.next_step {
                        context.push_str(&format!(
                            "Next plan step: [{}; priority {}] {}\n",
                            step.status, step.priority, step.content
                        ));
                    }
                    anyhow::ensure!(
                        context.len() <= max_bytes,
                        "Prediction plan summary exceeds its context budget; no required context was dropped"
                    );
                }
            }
        }
        let discussion_budget = max_bytes
            .saturating_sub(context.len())
            .min(MAX_PREDICTION_DISCUSSION_BYTES);
        context.push_str(&recent_prediction_discussion(
            store,
            session_id,
            discussion_budget,
        )?);
        let summaries = if include_summaries {
            store.prediction_summaries(
                session_id,
                max_bytes.saturating_sub(context.len()),
            )?
        } else {
            Vec::new()
        };
        if !summaries.is_empty() {
            context.push_str("Conversation summaries:\n");
            for summary in summaries {
                context.push_str(&format!(
                    "- {}: {}\n",
                    summary.phase, summary.summary_markdown
                ));
                anyhow::ensure!(
                    context.len() <= max_bytes,
                    "Prediction summary exceeds its context budget; no required context was dropped"
                );
            }
        }
        Ok(context)
    }

    fn durable_session_context(&self, view: &SessionView) -> Result<String> {
        let durable = self.durable_prediction_context(&view.session.id)?;
        self.session_context_with_work(view, &durable)
    }

    fn session_context_with_work(
        &self,
        view: &SessionView,
        durable: &str,
    ) -> Result<String> {
        let mut context = format!(
            "Session: {}\nTask intent: {}\nWork kind: {}\nPhase: {} ({}, visit {})\n",
            view.session.title,
            view.task.intent.display_name(),
            view.session.work_kind.display_name(),
            view.workflow.phase.title,
            view.workflow.phase.id,
            view.workflow.phase.visit,
        );
        if !view.task.objective.trim().is_empty() {
            context.push_str(&format!("Objective: {}\n", view.task.objective));
        }
        if let Some(issue) = &view.workflow.primary_work_item {
            context.push_str(&format!(
                "Issue: {}/{}#{} — {}\n",
                issue.owner, issue.repo, issue.issue_number, issue.title
            ));
        }
        if !view.workflow.current_artifact_ids.is_empty() {
            context.push_str(&format!(
                "Current artifact IDs: {}\n",
                view.workflow.current_artifact_ids.join(", ")
            ));
        }
        context.push_str(&format!(
            "Durable documentation root: {}/. When creating lasting documents, use topic-named Markdown under research/, design/, plans/, verification/, or reviews/. Session history expires 30 days after archive.\n",
            self.documentation_root()?,
        ));
        if !durable.is_empty() {
            context.push_str("Durable work state:\n");
            context.push_str(durable);
        }
        Ok(context)
    }

    fn documentation_root(&self) -> Result<String> {
        let Some(workspace) = self.workspace.read().clone() else {
            return Ok("docs".to_string());
        };
        let config =
            ahead_core::config::read_ahead_config(&workspace, "config.toml")?;
        let root = if let Some(config) = config {
            let table: toml::Table =
                config.parse().context("Invalid .ahead/config.toml")?;
            match table.get("documentation") {
                Some(value) => {
                    let table = value
                        .as_table()
                        .context("[documentation] must be a table")?;
                    match table.get("root") {
                        Some(root) => root
                            .as_str()
                            .context("documentation.root must be a string")?
                            .to_string(),
                        None => "docs".to_string(),
                    }
                }
                None => "docs".to_string(),
            }
        } else {
            "docs".to_string()
        };
        anyhow::ensure!(
            !root.is_empty()
                && Path::new(&root)
                    .components()
                    .all(|component| matches!(component, Component::Normal(_))),
            "documentation.root must be a workspace-relative directory"
        );
        Ok(root)
    }

    fn implementation_handoff_context(
        &self,
        parent: &SessionView,
        delegated_scope: &str,
    ) -> Result<String> {
        let mut context = format!(
            "Implement the delegated work in this AHEAD workspace. The human owns design decisions and will return to the parent AHEAD session for verification and review.\nParent session: {}\nDelegated scope: {}\n\n",
            parent.session.id,
            if delegated_scope.trim().is_empty() {
                "Continue the agreed implementation"
            } else {
                delegated_scope.trim()
            },
        );
        context.push_str(&self.durable_session_context(parent)?);
        let store = self.store.read();
        if let Some(runtime) = store.get_agent_runtime_state(&parent.session.id)? {
            if !runtime.plan.is_empty() {
                context.push_str("\nCurrent plan:\n");
                for step in runtime.plan {
                    context.push_str(&format!(
                        "- [{}] {}\n",
                        step.status, step.content
                    ));
                }
            }
        }
        let page = store.list_messages_page(&parent.session.id, None, 20)?;
        if !page.messages.is_empty() {
            context.push_str("\nRecent parent conversation:\n");
            for message in page
                .messages
                .into_iter()
                .filter(|message| message.human_recipient_ids().is_none())
            {
                let excerpt: String = message.content.chars().take(1200).collect();
                context.push_str(&format!("{}: {}\n", message.role, excerpt));
            }
        }
        Ok(context)
    }

    pub fn request_prediction(
        &self,
        request: PredictionRequest,
        open_buffers: &[OpenBufferContext],
    ) -> Result<PredictionResult> {
        let snapshot = self.prepare_prediction(request.clone(), open_buffers)?;
        let result = PredictionEngine::predict(
            &snapshot.work,
            &snapshot.request,
            open_buffers,
            &snapshot.provider,
        )?;
        anyhow::ensure!(
            self.prepare_prediction(request, open_buffers)? == snapshot,
            "prediction context changed during inference"
        );
        Ok(result)
    }

    pub(crate) fn prepare_prediction(
        &self,
        request: PredictionRequest,
        open_buffers: &[OpenBufferContext],
    ) -> Result<PredictionContextSnapshot> {
        self.prepare_prediction_with_model_context(request, open_buffers, None)
    }

    pub(crate) fn prepare_prediction_with_model_context(
        &self,
        mut request: PredictionRequest,
        open_buffers: &[OpenBufferContext],
        model_context: Option<&PredictionModelContext>,
    ) -> Result<PredictionContextSnapshot> {
        if let Some(session_id) = request.session_id.as_deref() {
            anyhow::ensure!(
                !self.store.read().is_session_archived(session_id)?,
                "Archived sessions cannot provide prediction context"
            );
        }
        let view = request
            .session_id
            .as_deref()
            .map(|id| self.get_session(id))
            .transpose()?
            .flatten();
        if request.session_id.is_some() && view.is_none() {
            bail!("Session not found");
        }

        let mut session_context_version = None;
        let mut caller_context = String::new();
        let work = if let Some(view) = view.as_ref() {
            caller_context = std::mem::take(&mut request.work_context);
            let (version, durable) =
                match self.cached_prediction_context(&view.session.id) {
                    Ok(context) => context,
                    Err(error) if Self::can_compact_prediction_context(&error) => {
                        self.cached_compacted_prediction_context(&view.session.id)?
                    }
                    Err(error) => return Err(error),
                };
            session_context_version = Some(version);
            request.work_context = self.session_context_with_work(view, &durable)?;
            if !caller_context.trim().is_empty() {
                request.work_context.push('\n');
                request.work_context.push_str(&caller_context);
            }
            PredictionWorkContext {
                work_kind: view.session.work_kind,
                mode: view.task.intent.assistance_mode(),
                phase_title: view.workflow.phase.title.clone(),
                primary_issue: view
                    .workflow
                    .primary_work_item
                    .as_ref()
                    .map(|i| format!("#{} {}", i.issue_number, i.title)),
                active_invariants: Vec::new(),
            }
        } else {
            // Editor-only FIM remains available before a durable AHEAD session
            // exists. It still uses the explicit provider and editor context.
            PredictionWorkContext {
                work_kind: WorkKind::ProductChange,
                mode: AssistanceMode::Assist,
                phase_title: "Editor-only prediction".to_string(),
                primary_issue: None,
                active_invariants: Vec::new(),
            }
        };

        let workspace = self
            .workspace
            .read()
            .clone()
            .context("Prediction provider requires an initialized workspace")?;
        let mut target_paths = vec![request.path.as_str()];
        target_paths.extend(open_buffers.iter().map(|buffer| buffer.path.as_str()));
        let instruction_context =
            ahead_agent::project_instruction_prompt_for_targets(
                &workspace,
                &target_paths,
            )?;
        if !instruction_context.is_empty() {
            request.work_context =
                format!("{instruction_context}\n\n{}", request.work_context);
        }
        let provider = PredictionProviderConfig::from_workspace_for_model(
            Some(&workspace),
            model_context,
        )?;
        if let Some(view) = view.as_ref() {
            if !PredictionEngine::model_fim_context_fits(
                &work,
                &request,
                open_buffers,
                &provider,
                model_context,
            )? {
                let (_, durable) =
                    self.cached_compacted_prediction_context(&view.session.id)?;
                let mut compacted_work_context =
                    self.session_context_with_work(view, &durable)?;
                if !caller_context.trim().is_empty() {
                    compacted_work_context.push('\n');
                    compacted_work_context.push_str(&caller_context);
                }
                if !instruction_context.is_empty() {
                    compacted_work_context =
                        format!("{instruction_context}\n\n{compacted_work_context}");
                }
                let mut compacted_request = request.clone();
                compacted_request.work_context = compacted_work_context;
                anyhow::ensure!(
                    PredictionEngine::model_fim_context_fits(
                        &work,
                        &compacted_request,
                        open_buffers,
                        &provider,
                        model_context,
                    )?,
                    "Required FIM context exceeds the selected model's safe input budget after faithfully compacting the active plan; no session constraints or instructions were dropped"
                );
                request = compacted_request;
            }
        }
        PredictionEngine::validate_context_budget(&work, &request, open_buffers)?;
        Ok(PredictionContextSnapshot {
            work,
            request,
            provider,
            model_context: model_context.cloned(),
            session: view,
            session_context_version,
            workspace,
        })
    }

    fn start_streamed_agent_turn(
        &self,
        dto: ahead_rpc::ahead::AgentTurnRequestDto,
    ) -> Result<Id> {
        let actor_id = self.auth.read().get_active_user(None).login;
        self.start_streamed_agent_turn_as(dto, &actor_id, None)
    }

    fn start_streamed_agent_turn_as(
        &self,
        mut dto: ahead_rpc::ahead::AgentTurnRequestDto,
        actor_id: &str,
        github_id: Option<u64>,
    ) -> Result<Id> {
        dto.context.discard_stale_selection();
        let view = if github_id.is_some() {
            self.shareable_session(&dto.session_id)?
        } else {
            self.get_session(&dto.session_id)?
                .context("Session not found")?
        };
        anyhow::ensure!(
            matches!(view.session.lifecycle, SessionLifecycle::Active),
            "Only an active session can start an agent turn"
        );
        anyhow::ensure!(
            view.participants.iter().any(|record| {
                matches!(&record.participant, Participant::Human { id, subject, .. }
                    if id.eq_ignore_ascii_case(actor_id)
                        && github_id.is_none_or(|github_id| subject == &format!("github:{github_id}")))
                    && matches!(record.role, SessionRole::Owner | SessionRole::Editor)
            }),
            "This participant cannot start agent turns"
        );
        anyhow::ensure!(
            mentioned_participants(&dto.user_message, &view.participants, actor_id)
                .is_empty(),
            "Messages mentioning session participants are for people, not the agent"
        );
        dto.read_only |= dto.harness == HarnessKind::Ahead
            && view.task.intent == TaskIntent::Teaching;
        if dto.expected_policy_sha256.is_empty() {
            anyhow::bail!("Agent turn is missing its expected policy hash");
        }
        if dto.expected_policy_sha256 != view.session.policy.sha256 {
            anyhow::bail!(
                "Stale policy: expected {}, host holds {}",
                dto.expected_policy_sha256,
                view.session.policy.sha256
            );
        }
        if let Some(scope) = &dto.scope {
            if view.task.intent == TaskIntent::Assistance
                && !scope.allowed_paths.is_empty()
                && !dto.context.active_path.is_empty()
            {
                let workspace =
                    self.workspace.read().clone().context(
                        "Workspace must be initialized before a scoped turn",
                    )?;
                if !path_is_allowed(
                    &dto.context.active_path,
                    &scope.allowed_paths,
                    &workspace,
                ) {
                    anyhow::bail!(
                        "Active file {} is outside the approved mechanical scope",
                        dto.context.active_path
                    );
                }
            }
        }
        dto.session_context = compose_session_context(
            self.durable_session_context(&view)?,
            &dto.session_context,
        );
        set_current_message_author(&mut dto.session_context, actor_id);
        self.harness.start_turn(dto, actor_id)
    }

    /// Builds a full export bundle: session + workflow + participants +
    /// anchors + work items/events/close-outs +
    /// conversation summaries. Everything visible without the original DB.
    pub fn session_export(&self, session_id: &str) -> Result<SessionExportBundle> {
        let view = self.get_session(session_id)?.context("Session not found")?;
        let store = self.store.read();
        let anchors = store.list_anchors(session_id)?;
        let code_comments = store.list_code_comments(session_id)?;
        let work_items = store.list_work_items(session_id)?;
        let mut work_item_events = Vec::new();
        for item in &work_items {
            work_item_events.extend(store.list_work_item_events(&item.id, None)?);
        }
        let mut work_item_closeouts = Vec::new();
        for item in &work_items {
            if let Some(closeout) = store.get_work_item_closeout(&item.id)? {
                work_item_closeouts.push(closeout);
            }
        }
        let conversation_summaries =
            store.list_conversation_summaries(session_id)?;
        let conversation_messages = store.list_messages(session_id)?;
        let agent_runtime_state = store
            .get_agent_runtime_state(session_id)?
            .unwrap_or_default();
        Ok(SessionExportBundle {
            format_version: "ahead.editor/v0-draft".to_string(),
            exported_at: chrono::Utc::now().to_rfc3339(),
            session: view.session,
            task: view.task,
            learning_arc: view.learning_arc,
            workflow: view.workflow,
            participants: view.participants,
            conversation_messages,
            agent_runtime_state,
            anchors,
            code_comments,
            work_items,
            work_item_events,
            work_item_closeouts,
            conversation_summaries,
            anchors_note: "Anchors carry path + range + quote hash; resolve against current tree, never trust line numbers blindly.".to_string(),
        })
    }

    /// Restores a session from an export bundle into this store. Fails
    /// closed on id collision (no silent overwrite); the caller picks a
    /// fresh import (e.g. re-export with a new session id) instead.
    pub fn session_restore(
        &self,
        bundle: SessionExportBundle,
    ) -> Result<SessionView> {
        if bundle.format_version != "ahead.editor/v0-draft" {
            anyhow::bail!("Unsupported export format: {}", bundle.format_version);
        }
        if self.get_session(&bundle.session.id)?.is_some() {
            anyhow::bail!(
                "Session id already exists; import needs a fresh id, refusing overwrite"
            );
        }
        let view = SessionView {
            session: bundle.session.clone(),
            task: bundle.task.clone(),
            learning_arc: bundle.learning_arc.clone(),
            workflow: bundle.workflow.clone(),
            participants: bundle.participants.clone(),
        };
        {
            let mut store = self.store.write();
            store.insert_session(&view)?;
            for anchor in &bundle.anchors {
                store.insert_anchor(anchor)?;
            }
            for comment in &bundle.code_comments {
                store.insert_code_comment(comment)?;
            }
            for item in &bundle.work_items {
                store.insert_work_item_row(item)?;
            }
            for event in &bundle.work_item_events {
                store.insert_work_item_event_row(event)?;
            }
            for closeout in &bundle.work_item_closeouts {
                store.insert_work_item_closeout_row(closeout)?;
            }
            for summary in &bundle.conversation_summaries {
                store.insert_conversation_summary_row(summary)?;
            }
            for message in &bundle.conversation_messages {
                store.upsert_message(message)?;
            }
            store.set_agent_runtime_state(
                &bundle.session.id,
                &bundle.agent_runtime_state,
            )?;
        }
        {
            let mut active = self.active_sessions.write();
            active.insert(view.session.id.clone(), view.clone());
        }
        Ok(view)
    }

    /// Captures a frozen review snapshot: code tree hash, implementer set,
    /// and initial findings. Findings stay; attestation is a separate record
    /// below, with reviewer relationship preserved for policy evaluation.
    pub fn review_capture(
        &self,
        session_id: &str,
        code_tree_sha: String,
        implementer_ids: Vec<Id>,
        findings: Vec<String>,
    ) -> Result<ahead_rpc::ahead::ReviewSnapshotDto> {
        self.get_session(session_id)?.context("Session not found")?;
        let mut snapshot = super::collab::ReviewSnapshot::new(
            uuid::Uuid::new_v4().to_string(),
            session_id.to_string(),
            code_tree_sha,
            implementer_ids,
        );
        snapshot.findings = findings;
        let dto = snapshot.to_dto();
        self.review_snapshots
            .write()
            .insert(dto.snapshot_id.clone(), snapshot);
        Ok(dto)
    }

    /// Records an attestation on a snapshot. The reviewer relationship is
    /// preserved for the repository/team policy to interpret; AHEAD does not
    /// impose a universal independent-review gate. A code change (new capture
    /// with a different tree SHA) makes prior approval stale while preserving
    /// findings.
    pub fn review_attest(
        &self,
        snapshot_id: &str,
        reviewer_id: &str,
    ) -> Result<ahead_rpc::ahead::ReviewSnapshotDto> {
        let mut snapshots = self.review_snapshots.write();
        let Some(snapshot) = snapshots.get_mut(snapshot_id) else {
            anyhow::bail!("Review snapshot not found: {}", snapshot_id);
        };
        snapshot.record_approval(reviewer_id)?;
        Ok(snapshot.to_dto())
    }

    pub fn review_get(
        &self,
        session_id: &str,
    ) -> Result<Option<ahead_rpc::ahead::ReviewSnapshotDto>> {
        let snapshots = self.review_snapshots.read();
        let latest = snapshots
            .values()
            .filter(|s| s.session_id == session_id)
            .max_by_key(|s| s.snapshot_id.clone())
            .map(|s| s.to_dto());
        Ok(latest)
    }

    fn managed_voice_session(&self, session_id: &str) -> Result<SessionView> {
        let view = self.get_session(session_id)?.context("Session not found")?;
        let actor_id = self.auth.read().get_active_user(None).login;
        anyhow::ensure!(
            actor_id.eq_ignore_ascii_case(&view.session.owner_id)
                && view.participants.iter().any(|record| {
                    record.role == SessionRole::Owner
                        && matches!(
                            &record.participant,
                            Participant::Human { id, .. }
                                if id.eq_ignore_ascii_case(&actor_id)
                        )
                }),
            "Only the session owner can manage voice calls"
        );
        anyhow::ensure!(
            !self
                .store
                .read()
                .get_harness_binding(session_id)?
                .is_some_and(|(_, backend)| backend.starts_with("external-agent")),
            "Voice calls require a managed AHEAD session"
        );
        Ok(view)
    }

    pub(crate) fn start_voice_call(
        &self,
        session_id: &str,
    ) -> Result<VoiceCallLease> {
        let view = self.managed_voice_session(session_id)?;
        anyhow::ensure!(
            matches!(view.session.lifecycle, SessionLifecycle::Active),
            "Voice calls require an active session"
        );
        let voice = {
            let mut voice_map = self.active_voice_sessions.write();
            Arc::clone(voice_map.entry(session_id.to_string()).or_insert_with(
                || {
                    Arc::new(VoiceSession::new(
                        session_id.to_string(),
                        format!("voice-{session_id}"),
                    ))
                },
            ))
        };
        voice.start_call()
    }

    pub(crate) fn end_voice_call(&self, lease: &VoiceCallLease) -> Result<bool> {
        let _view = self.managed_voice_session(&lease.session_id)?;
        let voice = self
            .active_voice_sessions
            .read()
            .get(&lease.session_id)
            .cloned();
        Ok(voice.is_some_and(|voice| voice.end_call(lease)))
    }

    pub fn handle_voice_control(
        &self,
        session_id: &str,
        call_id: &str,
        connection_epoch: u64,
        control: VoiceControl,
    ) -> Result<()> {
        let _view = self.managed_voice_session(session_id)?;
        let voice = self
            .active_voice_sessions
            .read()
            .get(session_id)
            .cloned()
            .context("Voice call is not active")?;
        anyhow::ensure!(
            voice.matches_call(call_id, connection_epoch),
            "Voice control belongs to a stale connection"
        );
        voice.handle_control(control);
        Ok(())
    }
}

fn compose_session_context(durable_context: String, turn_context: &str) -> String {
    if turn_context.trim().is_empty() {
        durable_context
    } else if durable_context.trim().is_empty() {
        turn_context.to_string()
    } else {
        format!("{durable_context}\n\n{turn_context}")
    }
}

fn set_current_message_author(context: &mut String, actor_id: &str) {
    if let Some((prefix, suffix)) = context.rsplit_once('\n')
        && suffix.starts_with("Current message author: @")
    {
        context.truncate(prefix.len());
    }
    context.push_str(&format!("\nCurrent message author: @{actor_id}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(unix, windows))]
    fn symlink_file(target: &Path, link: &Path) -> Result<()> {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link)?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(target, link)?;
        Ok(())
    }

    #[cfg(any(unix, windows))]
    fn symlink_dir(target: &Path, link: &Path) -> Result<()> {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link)?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(target, link)?;
        Ok(())
    }

    #[test]
    fn retry_replaces_the_author_without_duplicating_context() {
        let mut context = "Objective: keep the original snapshot".to_string();
        set_current_message_author(&mut context, "alice");
        set_current_message_author(&mut context, "bob");
        assert_eq!(
            context,
            "Objective: keep the original snapshot\nCurrent message author: @bob"
        );
    }

    #[test]
    fn supplemental_turn_context_keeps_durable_session_state() {
        assert_eq!(
            compose_session_context(
                "Session objective".into(),
                "Voice teaching instructions"
            ),
            "Session objective\n\nVoice teaching instructions"
        );
        assert_eq!(
            compose_session_context("Session objective".into(), "  "),
            "Session objective"
        );
        assert_eq!(
            compose_session_context(String::new(), "Voice teaching instructions"),
            "Voice teaching instructions"
        );
    }

    #[test]
    fn mcp_approval_requests_are_bound_to_the_active_workspace() -> Result<()> {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        let host = AheadSessionHost::in_memory()?;
        host.set_workspace(first.path().to_path_buf());
        assert_eq!(
            host.checked_mcp_workspace(first.path())?,
            first.path().canonicalize()?
        );
        assert!(host.checked_mcp_workspace(second.path()).is_err());
        Ok(())
    }

    #[test]
    fn session_list_projects_sidebar_metadata_and_keeps_full_detail_on_read() {
        let host = AheadSessionHost::in_memory().expect("open session host");
        let view = host
            .start_work(
                Some(WorkKind::ProductChange),
                "Session title".to_string(),
                "Build the feature".to_string(),
                None,
            )
            .expect("start work");
        let listed: Vec<SessionListItem> = serde_json::from_value(
            host.handle_request(AheadRequest::ListSessions)
                .expect("list sessions"),
        )
        .expect("decode sidebar metadata");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, view.session.id);
        assert_eq!(listed[0].title, view.session.title);
        assert_eq!(listed[0].lifecycle, SessionLifecycle::Active);
        assert_eq!(
            host.get_session(&listed[0].id)
                .expect("read session")
                .expect("session exists"),
            view
        );
    }

    #[test]
    fn addressed_message_is_durable_and_does_not_start_an_agent_turn() -> Result<()>
    {
        let host = AheadSessionHost::in_memory()?;
        assert!(
            host.add_workspace_participant("bob,alice".into(), SessionRole::Editor)
                .is_err()
        );
        host.add_workspace_participant("bob".into(), SessionRole::Editor)?;
        let view = host.start_work(
            Some(WorkKind::ProductChange),
            "Shared work".into(),
            "Discuss the change".into(),
            None,
        )?;
        let message: ConversationMessage = serde_json::from_value(
            host.handle_request(AheadRequest::PostHumanMessage {
                session_id: view.session.id.clone(),
                content: "@bob please review this".into(),
            })?,
        )?;
        assert_eq!(message.human_recipient_ids(), Some(vec!["bob"]));
        assert_eq!(
            host.store.read().list_messages(&view.session.id)?,
            vec![message]
        );
        assert!(
            host.store
                .read()
                .has_human_only_messages(&view.session.id)?
        );
        host.store.read().save_conversation_summary(
            &view.session.id,
            "plan",
            "private review detail",
            "message range",
        )?;
        assert!(
            !host
                .durable_session_context(&view)?
                .contains("private review detail")
        );
        assert!(!host.harness().has_active_turn(&view.session.id));
        assert!(
            host.handle_request(AheadRequest::PostHumanMessage {
                session_id: view.session.id.clone(),
                content: "@currentFile @unknown".into(),
            })
            .is_err()
        );
        host.active_sessions
            .write()
            .get_mut(&view.session.id)
            .expect("active session")
            .participants
            .iter_mut()
            .find(|record| record.participant.id() == view.session.owner_id)
            .expect("owner participant")
            .role = SessionRole::Reviewer;
        let error = host
            .start_streamed_agent_turn(turn_dto(
                &view.session.id,
                "run a turn",
                &view.session.policy.sha256,
            ))
            .expect_err("reviewer cannot use the host's agent account");
        assert!(error.to_string().contains("cannot start agent turns"));
        Ok(())
    }

    #[test]
    fn voice_transcripts_are_durable_session_messages() -> Result<()> {
        let host = AheadSessionHost::in_memory()?;
        let view = host.start_work(
            Some(WorkKind::ProductChange),
            "Voice work".into(),
            "Explain the code".into(),
            None,
        )?;
        let call_id = "voice-call-1";
        let human_segment_id = Uuid::new_v4().to_string();
        let agent_segment_id = Uuid::new_v4().to_string();

        let human: ConversationMessage = serde_json::from_value(
            host.handle_request(AheadRequest::RecordVoiceTranscript {
                session_id: view.session.id.clone(),
                call_id: call_id.into(),
                segment_id: human_segment_id.clone(),
                speaker: VoiceTranscriptSpeaker::Human,
                start_ms: 120,
                end_ms: 740,
                content: "Explain this function".into(),
            })?,
        )?;
        let human_retry: ConversationMessage = serde_json::from_value(
            host.handle_request(AheadRequest::RecordVoiceTranscript {
                session_id: view.session.id.clone(),
                call_id: call_id.into(),
                segment_id: human_segment_id.clone(),
                speaker: VoiceTranscriptSpeaker::Human,
                start_ms: 120,
                end_ms: 740,
                content: "Explain this function".into(),
            })?,
        )?;
        assert_eq!(human_retry, human);
        assert!(
            host.handle_request(AheadRequest::RecordVoiceTranscript {
                session_id: view.session.id.clone(),
                call_id: call_id.into(),
                segment_id: human_segment_id.clone(),
                speaker: VoiceTranscriptSpeaker::Human,
                start_ms: 120,
                end_ms: 740,
                content: "different content".into(),
            })
            .is_err()
        );
        assert!(
            host.handle_request(AheadRequest::RecordVoiceTranscript {
                session_id: view.session.id.clone(),
                call_id: call_id.into(),
                segment_id: human_segment_id.clone(),
                speaker: VoiceTranscriptSpeaker::Human,
                start_ms: 121,
                end_ms: 740,
                content: "Explain this function".into(),
            })
            .is_err()
        );
        let agent: ConversationMessage = serde_json::from_value(
            host.handle_request(AheadRequest::RecordVoiceTranscript {
                session_id: view.session.id.clone(),
                call_id: call_id.into(),
                segment_id: agent_segment_id.clone(),
                speaker: VoiceTranscriptSpeaker::Agent,
                start_ms: 900,
                end_ms: 1_300,
                content: "This function validates the input.".into(),
            })?,
        )?;

        assert_eq!(human.role, "human");
        assert_eq!(human.actor_id, view.session.owner_id);
        assert_eq!(
            human.voice_timing,
            Some(VoiceTranscriptTiming {
                call_id: call_id.into(),
                start_ms: 120,
                end_ms: 740,
            })
        );
        assert_eq!(agent.role, "agent");
        assert_eq!(agent.actor_id, AHEAD_ACTOR_ID);
        assert_eq!(
            agent.voice_timing,
            Some(VoiceTranscriptTiming {
                call_id: call_id.into(),
                start_ms: 900,
                end_ms: 1_300,
            })
        );
        assert_eq!(human.turn_id, agent.turn_id);
        assert_eq!(agent.sequence, human.sequence + 1);
        assert_eq!(
            host.store.read().list_messages(&view.session.id)?,
            vec![human.clone(), agent.clone()]
        );
        assert_eq!(
            host.store
                .read()
                .list_messages_page(&view.session.id, None, 2)?
                .messages,
            vec![human.clone(), agent.clone()]
        );
        assert!(!host.harness().has_active_turn(&view.session.id));
        host.active_sessions
            .write()
            .get_mut(&view.session.id)
            .expect("active session")
            .participants
            .iter_mut()
            .find(|record| record.participant.id() == view.session.owner_id)
            .expect("owner participant")
            .role = SessionRole::Reviewer;
        assert!(
            host.handle_request(AheadRequest::RecordVoiceTranscript {
                session_id: view.session.id,
                call_id: call_id.into(),
                segment_id: Uuid::new_v4().to_string(),
                speaker: VoiceTranscriptSpeaker::Human,
                start_ms: 1_400,
                end_ms: 1_800,
                content: "a reviewer cannot add voice history".into(),
            })
            .is_err()
        );

        Ok(())
    }

    #[test]
    fn voice_call_leases_are_session_owned_and_reject_stale_controls() -> Result<()>
    {
        let host = AheadSessionHost::in_memory()?;
        let view = host.start_work(
            Some(WorkKind::ProductChange),
            "Voice call".into(),
            "Explain the code".into(),
            None,
        )?;
        let start = || AheadRequest::StartVoiceCall {
            session_id: view.session.id.clone(),
        };
        let first: VoiceCallLease =
            serde_json::from_value(host.handle_request(start())?)?;
        assert_eq!(first.session_id, view.session.id);
        assert!(first.connection_epoch > 0);
        assert!(host.handle_request(start()).is_err());

        let ended = host.handle_request(AheadRequest::EndVoiceCall {
            lease: first.clone(),
        })?;
        assert_eq!(
            ended.get("ended").and_then(serde_json::Value::as_bool),
            Some(true)
        );

        let second: VoiceCallLease =
            serde_json::from_value(host.handle_request(start())?)?;
        assert!(second.connection_epoch > first.connection_epoch);
        assert!(
            host.handle_request(AheadRequest::VoiceControl {
                session_id: first.session_id.clone(),
                call_id: first.call_id.clone(),
                connection_epoch: first.connection_epoch,
                control: VoiceControl::SetMicMuted { muted: true },
            })
            .is_err()
        );
        assert!(
            host.handle_request(AheadRequest::VoiceControl {
                session_id: second.session_id.clone(),
                call_id: second.call_id.clone(),
                connection_epoch: second.connection_epoch,
                control: VoiceControl::SetMicMuted { muted: true },
            })
            .is_ok()
        );

        let stale_end =
            host.handle_request(AheadRequest::EndVoiceCall { lease: first })?;
        assert_eq!(
            stale_end.get("ended").and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert!(
            host.active_voice_sessions
                .read()
                .get(&view.session.id)
                .is_some_and(|voice| {
                    voice.matches_call(&second.call_id, second.connection_epoch)
                })
        );

        Ok(())
    }

    #[test]
    fn team_manifest_only_invites_selected_verified_members() -> Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::create_dir(temp.path().join(".ahead"))?;
        std::fs::write(
            temp.path().join(".ahead/team.toml"),
            "[[members]]\ngithub = 'bob'\ndisplay_name = 'Bob'\nrole = 'reviewer'\n",
        )?;
        let host = AheadSessionHost::in_memory()?;
        let mut auth = GitHubAuthManager::with_custom_dir(temp.path().join("auth"));
        auth.save_auth(
            "host-token".into(),
            ahead_rpc::ahead::GitHubUser {
                login: "host".into(),
                id: 1,
                name: None,
                avatar_url: None,
                email: None,
                is_authenticated: true,
            },
        )?;
        host.set_auth_for_test(auth);
        host.set_workspace(temp.path().to_path_buf());
        let team = host.get_workspace_participants()?;
        assert!(team.iter().any(|record| {
            record.participant.id() == "bob" && record.role == SessionRole::Reviewer
        }));
        let view = host.start_work(
            Some(WorkKind::ProductChange),
            "Shared work".into(),
            "Discuss".into(),
            None,
        )?;
        assert!(
            !view
                .participants
                .iter()
                .any(|record| record.participant.id() == "bob")
        );
        let member = |login: &str, id| ahead_rpc::ahead::GitHubUser {
            login: login.into(),
            id,
            name: None,
            avatar_url: None,
            email: None,
            is_authenticated: true,
        };
        assert!(
            host.add_session_participant_verified(
                &view.session.id,
                member("mallory", 99),
                SessionRole::Editor,
            )
            .is_err()
        );
        let joined = host.add_session_participant_verified(
            &view.session.id,
            member("bob", 42),
            SessionRole::Reviewer,
        )?;
        assert!(joined.participants.iter().any(|record| {
            record.participant.id() == "bob" && record.role == SessionRole::Reviewer
        }));
        host.add_session_participant_verified(
            &view.session.id,
            member("bob", 42),
            SessionRole::Viewer,
        )?;
        assert!(
            host.create_code_comment_as(
                &view.session.id,
                "src.rs".into(),
                DisplayRange {
                    start: ahead_rpc::ahead::DisplayPosition { line: 0, col: 0 },
                    end: ahead_rpc::ahead::DisplayPosition { line: 0, col: 2 },
                },
                "fn".into(),
                "0".repeat(64),
                "Review".into(),
                "bob",
                42,
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn editor_code_comments_are_rejected_for_external_agent_sessions() -> Result<()>
    {
        let temp = tempfile::tempdir()?;
        std::fs::write(temp.path().join("src.rs"), "fn main() {}\n")?;
        let host = AheadSessionHost::in_memory()?;
        host.set_workspace(temp.path().to_path_buf());

        let managed = host.start_work(
            Some(WorkKind::ProductChange),
            "Managed comments".into(),
            "Keep comments on the AHEAD session".into(),
            None,
        )?;
        let range = DisplayRange {
            start: ahead_rpc::ahead::DisplayPosition { line: 0, col: 0 },
            end: ahead_rpc::ahead::DisplayPosition { line: 0, col: 2 },
        };
        let managed_comment = host.create_code_comment(
            &managed.session.id,
            "src.rs".into(),
            range.clone(),
            "fn".into(),
            "0".repeat(64),
            "Managed comment".into(),
        )?;
        let listed: Vec<CodeComment> = serde_json::from_value(
            host.handle_request(AheadRequest::ListCodeComments {
                session_id: managed.session.id.clone(),
            })?,
        )?;
        assert_eq!(listed, vec![managed_comment.clone()]);

        let external = host.start_work(
            Some(WorkKind::ProductChange),
            "External comments".into(),
            "External ACP must not expose editor comments".into(),
            None,
        )?;
        host.store.read().set_harness_binding(
            &external.session.id,
            "acp-session",
            "external-agent:pi-acp",
        )?;
        assert!(
            host.create_code_comment(
                &external.session.id,
                "src.rs".into(),
                range,
                "fn".into(),
                "0".repeat(64),
                "Must be rejected".into(),
            )
            .is_err()
        );
        assert!(
            host.handle_request(AheadRequest::ListCodeComments {
                session_id: external.session.id.clone(),
            })
            .is_err()
        );

        let mut external_comment = managed_comment;
        external_comment.id = "external-comment".into();
        external_comment.session_id = external.session.id.clone();
        host.store.read().insert_code_comment(&external_comment)?;
        assert!(
            host.handle_request(AheadRequest::ResolveCodeComment {
                session_id: external.session.id,
                comment_id: external_comment.id,
            })
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn agent_skills_rpc_returns_project_source_without_a_host_path() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = temp.path().to_path_buf();
        let skill_directory = workspace.join(".agents/skills/rpc-source-check");
        std::fs::create_dir_all(&skill_directory)?;
        std::fs::write(
            skill_directory.join("SKILL.md"),
            "---\nname: rpc-source-check\ndescription: RPC source check\n---\nUse the project skill.\n",
        )?;
        let invalid_skill_directory =
            workspace.join(".agents/skills/invalid-project-skill");
        std::fs::create_dir_all(&invalid_skill_directory)?;
        std::fs::write(
            invalid_skill_directory.join("SKILL.md"),
            "---\nname: mismatched-name\ndescription: Invalid project skill\n---\nThis should be skipped.\n",
        )?;

        let host = AheadSessionHost::in_memory()?;
        host.set_workspace(workspace.clone());
        let session = host.start_work(
            Some(WorkKind::ProductChange),
            "Skill catalog route".into(),
            "Test project skill discovery".into(),
            None,
        )?;
        let session_id = session.session.id;
        let catalog: ahead_rpc::ahead::AgentSkillCatalog = serde_json::from_value(
            host.handle_request(AheadRequest::AgentSkills {
                session_id: session_id.clone(),
                model: None,
                model_provider: None,
            })?,
        )?;
        let skill = catalog
            .skills
            .iter()
            .find(|skill| skill.name == "rpc-source-check")
            .with_context(|| {
                format!(
                    "project skill is missing from the RPC catalog: {:?}",
                    catalog.skills
                )
            })?;

        assert_eq!(catalog.skipped_count, 1);
        assert_eq!(skill.source, ahead_rpc::ahead::AgentSkillSource::Project);
        assert_eq!(skill.description, "RPC source check");
        assert!(
            !serde_json::to_string(&catalog)?
                .contains(&workspace.display().to_string())
        );

        std::fs::write(
            skill_directory.join("SKILL.md"),
            "---\nname: rpc-source-check\ndescription: Updated RPC source check\n---\nUse the refreshed project skill.\n",
        )?;
        let refreshed_catalog: ahead_rpc::ahead::AgentSkillCatalog =
            serde_json::from_value(host.handle_request(
                AheadRequest::AgentSkills {
                    session_id,
                    model: None,
                    model_provider: None,
                },
            )?)?;
        let refreshed_skill = refreshed_catalog
            .skills
            .iter()
            .find(|skill| skill.name == "rpc-source-check")
            .context("updated project skill is missing from the RPC catalog")?;
        assert_eq!(refreshed_skill.description, "Updated RPC source check");
        Ok(())
    }

    #[test]
    fn recent_workspace_files_are_relative_and_reject_external_paths() -> Result<()>
    {
        let temporary = tempfile::tempdir()?;
        let workspace = temporary.path().join("workspace");
        let source = workspace.join("src/main.rs");
        std::fs::create_dir_all(source.parent().context("source directory")?)?;
        std::fs::write(&source, "fn main() {}")?;
        let host = AheadSessionHost::in_memory()?;
        host.set_workspace(workspace.clone());

        let response =
            host.handle_request(AheadRequest::RecordRecentWorkspaceFile {
                path: "src/main.rs".to_string(),
                opened_at: 1,
            })?;
        assert!(response.is_null());
        let recent: Vec<RepoPath> = serde_json::from_value(
            host.handle_request(AheadRequest::ListRecentWorkspaceFiles)?,
        )?;
        assert_eq!(recent, ["src/main.rs"]);

        for invalid_path in ["../outside.rs", r"src\main.rs", ""] {
            let error = host
                .handle_request(AheadRequest::RecordRecentWorkspaceFile {
                    path: invalid_path.to_string(),
                    opened_at: 2,
                })
                .expect_err("invalid relative path must be rejected");
            assert!(error.to_string().contains("normalized relative path"));
        }
        let error = host
            .handle_request(AheadRequest::RecordRecentWorkspaceFile {
                path: source.to_str().context("source path is UTF-8")?.to_string(),
                opened_at: 3,
            })
            .expect_err("absolute path must be rejected");
        assert!(error.to_string().contains("normalized relative path"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let external = temporary.path().join("external.rs");
            std::fs::write(&external, "outside")?;
            symlink(&external, workspace.join("src/external.rs"))?;
            let error = host
                .handle_request(AheadRequest::RecordRecentWorkspaceFile {
                    path: "src/external.rs".to_string(),
                    opened_at: 4,
                })
                .expect_err("symlink escaping the workspace must be rejected");
            assert!(error.to_string().contains("inside the workspace"));
        }

        std::fs::remove_file(source)?;
        let recent: Vec<RepoPath> = serde_json::from_value(
            host.handle_request(AheadRequest::ListRecentWorkspaceFiles)?,
        )?;
        assert!(recent.is_empty());
        Ok(())
    }

    #[test]
    fn memory_search_refreshes_workspace_memory_and_hides_host_paths() -> Result<()>
    {
        let temp = tempfile::tempdir()?;
        let workspace = temp.path().to_path_buf();
        let memory_path = workspace.join(".ahead/memories/MEMORY.md");
        std::fs::create_dir_all(memory_path.parent().context("memory parent")?)?;
        std::fs::write(
            &memory_path,
            "# Project memory\nAHEAD memory search unique needle\n",
        )?;
        let host = AheadSessionHost::in_memory()?;
        host.set_workspace(workspace.clone());

        let value = host.handle_request(AheadRequest::SearchMemory {
            query: "search unique needle".into(),
            limit: 8,
        })?;
        let hits: Vec<MemoryExcerpt> = serde_json::from_value(value)?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].scope, MemoryScope::Project);
        assert_eq!(hits[0].source, ".ahead/memories/MEMORY.md");
        assert_eq!(hits[0].line, 2);
        let host_path = workspace.to_string_lossy();
        assert!(!hits[0].source.contains(host_path.as_ref()));
        Ok(())
    }

    #[test]
    fn reviewed_memory_replacement_is_snapshot_checked_and_reindexed() -> Result<()>
    {
        let temp = tempfile::tempdir()?;
        let workspace = temp.path().to_path_buf();
        let path = workspace.join(".ahead/memories/MEMORY.md");
        std::fs::create_dir_all(path.parent().context("memory parent")?)?;
        std::fs::write(&path, "# Existing memory\nold unique phrase\n")?;
        let host = AheadSessionHost::in_memory()?;
        host.set_workspace(workspace);

        let snapshot: MemoryDocument = serde_json::from_value(
            host.handle_request(AheadRequest::ReadMemory {
                scope: MemoryScope::Project,
            })?,
        )?;
        assert_eq!(snapshot.source, ".ahead/memories/MEMORY.md");
        assert_eq!(snapshot.content, "# Existing memory\nold unique phrase\n");

        let replacement = "# Updated memory\nnew unique phrase\n";
        host.handle_request(AheadRequest::ReplaceMemory {
            scope: MemoryScope::Project,
            expected_sha256: snapshot.sha256.clone(),
            content: replacement.into(),
        })?;
        assert_eq!(std::fs::read_to_string(&path)?, replacement);
        let hits = host.search_memory("new unique phrase", 8)?;
        assert_eq!(
            hits.first().context("updated memory search hit")?.excerpt,
            "new unique phrase"
        );

        std::fs::write(&path, "# Newer external edit\nkeep this edit\n")?;
        let error = host
            .handle_request(AheadRequest::ReplaceMemory {
                scope: MemoryScope::Project,
                expected_sha256: snapshot.sha256,
                content: "# Stale replacement\n".into(),
            })
            .expect_err("stale snapshot must not replace a newer edit");
        assert!(error.to_string().contains("changed since review"));
        assert_eq!(
            std::fs::read_to_string(path)?,
            "# Newer external edit\nkeep this edit\n"
        );
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn memory_read_refuses_a_symlinked_memory_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = temp.path().join("workspace");
        let memory_path = workspace.join(".ahead/memories/MEMORY.md");
        let external_path = temp.path().join("outside.md");
        std::fs::create_dir_all(memory_path.parent().context("memory parent")?)?;
        std::fs::write(&external_path, "leave this file alone")?;
        symlink_file(&external_path, &memory_path)?;
        let host = AheadSessionHost::in_memory()?;
        host.set_workspace(workspace);

        let error = host
            .handle_request(AheadRequest::ReadMemory {
                scope: MemoryScope::Project,
            })
            .expect_err("memory reads must refuse a symlink");
        assert!(error.to_string().contains("must not be a symlink"));
        let error = host
            .search_memory("leave this file alone", 8)
            .expect_err("memory search must refuse a symlink");
        assert!(error.to_string().contains("must not be a symlink"));
        assert_eq!(
            std::fs::read_to_string(external_path)?,
            "leave this file alone"
        );
        Ok(())
    }

    #[test]
    fn memory_write_rejects_relative_source_before_creating_directories()
    -> Result<()> {
        let current_dir = std::env::current_dir()?;
        let temp = tempfile::tempdir_in(&current_dir)?;
        let workspace_path = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace_path)?;
        let workspace = workspace_path
            .strip_prefix(&current_dir)
            .context("temporary workspace is not relative to the test directory")?
            .to_path_buf();
        assert!(!workspace.is_absolute());
        let host = AheadSessionHost::in_memory()?;
        host.set_workspace(workspace);

        let result = host.handle_request(AheadRequest::WriteMemory {
            scope: MemoryScope::Project,
            message_id: "relative-workspace-test".into(),
            content: "must not create a relative memory path".into(),
        });
        let unexpected_memory_dir = workspace_path.join(".ahead");
        let directory_was_created = unexpected_memory_dir.exists();
        if directory_was_created {
            std::fs::remove_dir_all(&unexpected_memory_dir)?;
        }
        let error = result.expect_err("relative memory sources must be rejected");
        assert!(error.to_string().contains("memory path must be absolute"));
        assert!(!directory_was_created);
        Ok(())
    }

    #[test]
    fn memory_write_appends_only_to_selected_scope_and_refreshes_index() -> Result<()>
    {
        let temp = tempfile::tempdir()?;
        let workspace = temp.path().to_path_buf();
        let host = AheadSessionHost::in_memory()?;
        host.set_workspace(workspace.clone());

        let result: MemoryWriteResult = serde_json::from_value(
            host.handle_request(AheadRequest::WriteMemory {
                scope: MemoryScope::Project,
                message_id: "message-write-48271".into(),
                content: "AHEAD memory write retry token nonce 48271".into(),
            })?,
        )?;
        let duplicate: MemoryWriteResult = serde_json::from_value(
            host.handle_request(AheadRequest::WriteMemory {
                scope: MemoryScope::Project,
                message_id: "message-write-48271".into(),
                content: "duplicate content must not be appended".into(),
            })?,
        )?;
        let project_path = workspace.join(".ahead/memories/MEMORY.md");
        let project_content = std::fs::read_to_string(&project_path)?;
        assert!(result.indexed);
        assert_eq!(result.warning, None);
        assert!(duplicate.indexed);
        assert_eq!(
            project_content
                .matches("<!-- AHEAD memory source:project:message-write-48271 -->")
                .count(),
            1
        );
        assert!(!project_content.contains("duplicate content must not be appended"));
        assert!(
            project_content.contains("AHEAD memory write retry token nonce 48271")
        );
        let hits = host.search_memory("retry token nonce 48271", 8)?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].scope, MemoryScope::Project);
        Ok(())
    }

    #[test]
    fn memory_write_rejects_append_that_exceeds_document_limit() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = temp.path().to_path_buf();
        let memory_path = workspace.join(".ahead/memories/MEMORY.md");
        std::fs::create_dir_all(memory_path.parent().context("memory parent")?)?;
        let host = AheadSessionHost::in_memory()?;
        host.set_workspace(workspace);

        for document_len in [
            ahead_rpc::ahead::MEMORY_DOCUMENT_MAX_BYTES,
            ahead_rpc::ahead::MEMORY_DOCUMENT_MAX_BYTES * 16,
        ] {
            let existing = "x".repeat(document_len);
            std::fs::write(&memory_path, &existing)?;
            let error = host
                .handle_request(AheadRequest::WriteMemory {
                    scope: MemoryScope::Project,
                    message_id: "document-limit-message".into(),
                    content: "this append must not exceed the document limit".into(),
                })
                .expect_err("memory append must preserve the document limit");

            assert!(error.to_string().contains("32 KiB limit"));
            assert_eq!(std::fs::read_to_string(&memory_path)?, existing);
        }
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn memory_write_refuses_a_symlinked_memory_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = temp.path().join("workspace");
        let memory_path = workspace.join(".ahead/memories/MEMORY.md");
        let external_path = temp.path().join("outside.md");
        std::fs::create_dir_all(memory_path.parent().context("memory parent")?)?;
        std::fs::write(&external_path, "leave this file alone")?;
        symlink_file(&external_path, &memory_path)?;
        let host = AheadSessionHost::in_memory()?;
        host.set_workspace(workspace);

        let error = host
            .handle_request(AheadRequest::WriteMemory {
                scope: MemoryScope::Project,
                message_id: "symlink-test-message".into(),
                content: "must not be written".into(),
            })
            .expect_err("memory write must refuse a symlink");
        assert!(error.to_string().contains("must not be a symlink"));
        assert_eq!(
            std::fs::read_to_string(external_path)?,
            "leave this file alone"
        );
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn memory_write_refuses_a_symlinked_memory_directory() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = temp.path().join("workspace");
        let ahead_directory = workspace.join(".ahead");
        let memory_directory = ahead_directory.join("memories");
        let external_directory = temp.path().join("outside-memories");
        std::fs::create_dir_all(&ahead_directory)?;
        std::fs::create_dir_all(&external_directory)?;
        symlink_dir(&external_directory, &memory_directory)?;
        let host = AheadSessionHost::in_memory()?;
        host.set_workspace(workspace);

        let error = host
            .handle_request(AheadRequest::WriteMemory {
                scope: MemoryScope::Project,
                message_id: "symlink-directory-test".into(),
                content: "must not create the external memory directory".into(),
            })
            .expect_err("memory write must refuse a symlinked directory");
        assert!(
            error
                .to_string()
                .contains("must not resolve through a symlink")
        );
        assert!(!external_directory.join("MEMORY.md").exists());
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn memory_directory_handle_survives_parent_symlink_swap() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let workspace = temporary.path().join("workspace");
        let memory_directory = workspace.join(".ahead/memories");
        let original_directory = workspace.join(".ahead/original-memories");
        let outside_directory = temporary.path().join("outside");
        std::fs::create_dir_all(&memory_directory)?;
        std::fs::create_dir_all(&outside_directory)?;
        let path = memory_directory.join("MEMORY.md");
        let outside_path = outside_directory.join("MEMORY.md");
        std::fs::write(&path, "original memory")?;
        std::fs::write(&outside_path, "outside memory")?;

        let directory = AheadSessionHost::open_memory_directory(&path, false)?
            .context("memory directory is missing")?;
        std::fs::rename(&memory_directory, &original_directory)?;
        symlink_dir(&outside_directory, &memory_directory)?;

        assert_eq!(
            AheadSessionHost::read_memory_content_at(&directory, &path)?,
            Some("original memory".into())
        );
        AheadSessionHost::replace_memory_content_at(&directory, "replacement")?;
        let mut file = AheadSessionHost::open_memory_append_at(&directory)?;
        file.write_all(b" appended")?;
        assert_eq!(
            std::fs::read_to_string(original_directory.join("MEMORY.md"))?,
            "replacement appended"
        );
        assert_eq!(std::fs::read_to_string(outside_path)?, "outside memory");
        Ok(())
    }

    #[test]
    fn memory_write_rejects_empty_oversized_and_invalid_source_notes() -> Result<()>
    {
        let temp = tempfile::tempdir()?;
        let workspace = temp.path().to_path_buf();
        let host = AheadSessionHost::in_memory()?;
        host.set_workspace(workspace.clone());
        let oversized = "x".repeat(8 * 1024 + 1);
        for (message_id, content, expected) in [
            ("valid-message", "  ".to_string(), "cannot be empty"),
            ("valid-message", oversized, "exceeds the 8 KiB limit"),
            (
                "../invalid-message",
                "note content".to_string(),
                "source message id is invalid",
            ),
        ] {
            let error = host
                .handle_request(AheadRequest::WriteMemory {
                    scope: MemoryScope::Project,
                    message_id: message_id.into(),
                    content,
                })
                .expect_err("invalid memory note must be rejected");
            assert!(error.to_string().contains(expected));
        }
        assert!(!workspace.join(".ahead/memories/MEMORY.md").exists());
        Ok(())
    }

    #[test]
    fn unsolicited_agent_buffer_response_is_rejected() {
        let host = AheadSessionHost::in_memory().expect("open session host");
        let error = host
            .handle_request(AheadRequest::AgentBufferSnapshotsResponse {
                session_id: "missing".into(),
                turn_id: "stale".into(),
                request_id: "unknown".into(),
                buffers: Vec::new(),
            })
            .expect_err("unsolicited buffer response must be rejected");
        assert!(error.to_string().contains("No agent turn"));
        let error = host
            .handle_request(AheadRequest::AgentBufferSnapshotsFailed {
                session_id: "missing".into(),
                turn_id: "stale".into(),
                request_id: "unknown".into(),
                message: "snapshot limit exceeded".into(),
            })
            .expect_err("unsolicited buffer failure must be rejected");
        assert!(error.to_string().contains("No agent turn"));
    }

    #[test]
    fn requests_infer_work_kind_and_choose_their_starting_phase() {
        let host = AheadSessionHost::in_memory().unwrap();
        let cases = [
            (Some(WorkKind::InternalImprovement), "implement"),
            (Some(WorkKind::ProductChange), "plan"),
            (Some(WorkKind::Decision), "decision-framing"),
            (Some(WorkKind::Investigation), "investigation-scrutinize"),
        ];

        for (work_kind, phase_id) in cases {
            let view = host
                .start_work(
                    work_kind,
                    "Work request".to_string(),
                    "test outcome".to_string(),
                    None,
                )
                .unwrap();
            assert_eq!(view.session.work_kind, work_kind.unwrap());
            assert_eq!(view.workflow.phase.id, phase_id);
            assert_eq!(
                host.get_session(&view.session.id)
                    .unwrap()
                    .unwrap()
                    .session
                    .work_kind,
                work_kind.unwrap()
            );
        }
    }

    #[test]
    fn start_work_persists_requested_harness_tier_before_first_turn() {
        let host = AheadSessionHost::in_memory().unwrap();
        let value = host
            .handle_request(AheadRequest::StartWork {
                work_kind: Some(WorkKind::ProductChange),
                title: "Managed agent thread".to_string(),
                starting_point: "Verify the managed thread boundary".to_string(),
                work_item: None,
                harness: Some(HarnessKind::Ahead),
                external_agent_id: None,
                parent_session_id: None,
            })
            .unwrap();
        let session_id = value
            .get("session")
            .and_then(|session| session.get("id"))
            .and_then(serde_json::Value::as_str)
            .unwrap();
        let status = host
            .handle_request(AheadRequest::HarnessStatus {
                session_id: session_id.to_string(),
            })
            .unwrap();
        assert_eq!(
            status.get("backend").and_then(serde_json::Value::as_str),
            Some("ahead-pending")
        );
        assert_eq!(
            host.harness()
                .harness_status(session_id)
                .unwrap()
                .get("acp_session_id"),
            Some(&serde_json::Value::String(String::new()))
        );
    }

    #[test]
    fn implementation_handoff_keeps_parent_link_and_context() -> Result<()> {
        let host = AheadSessionHost::in_memory()?;
        let parent = host.start_work(
            Some(WorkKind::ProductChange),
            "Workspace search".into(),
            "Search open and saved files".into(),
            None,
        )?;
        let premature = host.handle_request(AheadRequest::StartWork {
            work_kind: None,
            title: "Too early".into(),
            starting_point: "Prototype the search results".into(),
            work_item: None,
            harness: Some(HarnessKind::ExternalAcp),
            external_agent_id: Some("not-curated".into()),
            parent_session_id: Some(parent.session.id.clone()),
        });
        assert!(
            premature
                .unwrap_err()
                .to_string()
                .contains("implementation phase")
        );
        host.store.read().upsert_message(
            &ahead_rpc::ahead::ConversationMessage {
                id: "handoff-message".into(),
                session_id: parent.session.id.clone(),
                turn_id: "handoff-turn".into(),
                sequence: 1,
                role: "human".into(),
                actor_id: "human".into(),
                content: "Keep the public API unchanged.".into(),
                status: "complete".into(),
                created_at: chrono::Utc::now().to_rfc3339(),
                voice_timing: None,
            },
        )?;
        let context = host.implementation_handoff_context(
            &parent,
            "Prototype the search results",
        )?;
        assert!(context.contains("Keep the public API unchanged."));
        let child = host.start_work_with_binding(
            Some(parent.session.work_kind),
            "Search prototype".into(),
            context,
            None,
            Some("external-agent-pending"),
            Some(&parent.task.id),
        )?;

        assert_eq!(
            child.task.parent_task_id.as_deref(),
            Some(parent.task.id.as_str())
        );
        assert!(
            child
                .task
                .objective
                .contains("Prototype the search results")
        );
        assert!(child.task.objective.contains("Search open and saved files"));
        assert_eq!(
            host.list_sessions()?
                .into_iter()
                .find(|item| item.id == child.session.id)
                .and_then(|item| item.parent_session_id),
            Some(parent.session.id),
        );
        Ok(())
    }

    #[test]
    fn failed_harness_binding_rolls_back_session_before_retry() -> Result<()> {
        let host = AheadSessionHost::in_memory()?;
        let request = || AheadRequest::StartWork {
            work_kind: Some(WorkKind::ProductChange),
            title: "Retryable session".to_string(),
            starting_point: "Create one correctly bound session".to_string(),
            work_item: None,
            harness: Some(HarnessKind::Ahead),
            external_agent_id: None,
            parent_session_id: None,
        };
        host.store
            .write()
            .set_harness_binding_failure_for_test(true)?;

        let error = host
            .handle_request(request())
            .expect_err("injected binding failure must abort creation");
        assert!(
            error
                .to_string()
                .contains("injected harness binding failure")
        );
        assert!(host.list_sessions()?.is_empty());
        assert!(host.active_sessions.read().is_empty());
        assert!(host.active_voice_sessions.read().is_empty());

        host.store
            .write()
            .set_harness_binding_failure_for_test(false)?;
        let value = host.handle_request(request())?;
        let session_id = value
            .get("session")
            .and_then(|session| session.get("id"))
            .and_then(serde_json::Value::as_str)
            .context("created session id is missing")?;
        assert_eq!(host.list_sessions()?.len(), 1);
        assert_eq!(
            host.store
                .read()
                .get_harness_binding(session_id)?
                .map(|(_, backend)| backend),
            Some("ahead-pending".to_string())
        );
        assert!(host.active_sessions.read().contains_key(session_id));
        Ok(())
    }

    #[test]
    fn unsupported_external_adapter_is_rejected_before_session_creation()
    -> Result<()> {
        let host = AheadSessionHost::in_memory()?;
        let error = host
            .handle_request(AheadRequest::StartWork {
                work_kind: Some(WorkKind::ProductChange),
                title: "Invalid adapter selection".to_string(),
                starting_point: "Do not persist this session".to_string(),
                work_item: None,
                harness: Some(HarnessKind::ExternalAcp),
                external_agent_id: Some("not-curated".to_string()),
                parent_session_id: None,
            })
            .expect_err("unsupported ACP adapters must be rejected");
        assert!(error.to_string().contains("not supported by AHEAD"));
        assert!(host.list_sessions()?.is_empty());
        assert!(host.active_sessions.read().is_empty());
        Ok(())
    }

    #[test]
    fn explicit_teaching_request_creates_durable_learning_task() {
        let host = AheadSessionHost::in_memory().unwrap();
        let view = host
            .start_work(
                None,
                "Retry path lesson".to_string(),
                "Teach me why this retry path behaves this way".to_string(),
                None,
            )
            .unwrap();

        assert_eq!(view.task.intent, TaskIntent::Teaching);
        assert_eq!(view.task.work_kind, WorkKind::Investigation);
        let arc = view.learning_arc.as_ref().expect("teaching arc");
        assert_eq!(arc.task_id, view.task.id);
        assert_eq!(arc.state, "introduced");
        assert_eq!(arc.records[0].kind, "mission");
        let stored = host
            .store
            .read()
            .get_session(&view.session.id)
            .unwrap()
            .unwrap();
        assert_eq!(stored.task.intent, TaskIntent::Teaching);
        assert_eq!(stored.learning_arc.unwrap().records.len(), 1);
    }

    #[test]
    fn test_session_host_end_to_end_flow() {
        let host = AheadSessionHost::in_memory().unwrap();

        // 1. Start Work
        let view = host
            .start_work(
                Some(WorkKind::ProductChange),
                "Implement Retries".to_string(),
                "Need exponential backoff for network calls".to_string(),
                Some(GithubIssueRef {
                    host: "github.com".to_string(),
                    owner: "owner".to_string(),
                    repo: "repo".to_string(),
                    issue_number: 142,
                    issue_node_id: "node142".to_string(),
                    title: "Improve retries".to_string(),
                }),
            )
            .unwrap();

        assert_eq!(view.session.work_kind, WorkKind::ProductChange);
        assert_eq!(view.workflow.phase.id, "plan");
        assert_eq!(view.workflow.revision, 1);

        // 2. Advance to implementation
        let next_wf = host
            .advance_phase(&view.session.id, 1, "implement".to_string())
            .unwrap();
        assert_eq!(next_wf.phase.id, "implement");
        assert_eq!(next_wf.revision, 2);

        // Advancing with stale revision must fail
        let stale_res =
            host.advance_phase(&view.session.id, 1, "verify".to_string());
        assert!(stale_res.is_err());

        // 3. Anchoring code
        let anchor = host
            .create_anchor(
                &view.session.id,
                "src/retry.rs".to_string(),
                DisplayRange {
                    start: ahead_rpc::ahead::DisplayPosition { line: 10, col: 0 },
                    end: ahead_rpc::ahead::DisplayPosition { line: 15, col: 20 },
                },
                "pub fn retry() {}".to_string(),
                "human",
            )
            .unwrap();
        assert_eq!(anchor.path, "src/retry.rs");
    }

    #[test]
    fn prediction_resolves_the_requested_durable_session() {
        let host = AheadSessionHost::in_memory().unwrap();
        let view = host
            .start_work(
                Some(WorkKind::ProductChange),
                "Prediction context".to_string(),
                "Starting".to_string(),
                None,
            )
            .unwrap();
        let error = host
            .request_prediction(
                PredictionRequest {
                    request_id: "prediction-1".to_string(),
                    session_id: Some(view.session.id.clone()),
                    path: "src/lib.rs".to_string(),
                    cursor: ahead_rpc::ahead::DisplayPosition { line: 0, col: 7 },
                    prefix: "pub fn".to_string(),
                    suffix: String::new(),
                    work_context: String::new(),
                },
                &[],
            )
            .unwrap_err();
        assert!(error.to_string().contains("initialized workspace"));
        assert_eq!(
            host.get_session(&view.session.id)
                .unwrap()
                .unwrap()
                .session
                .id,
            view.session.id
        );
    }

    #[test]
    fn prediction_context_includes_durable_work_state() {
        let host = AheadSessionHost::in_memory().unwrap();
        let view = host
            .start_work(
                Some(WorkKind::ProductChange),
                "Durable prediction context".to_string(),
                "Starting".to_string(),
                None,
            )
            .unwrap();
        let item = host
            .store
            .read()
            .create_work_item(
                &view.session.id,
                "Preserve retry idempotency",
                "human",
            )
            .unwrap();
        host.store
            .read()
            .add_work_item_event(
                &item.id,
                &view.session.id,
                "decision",
                "Do not retry a request after a committed response.",
                "human",
            )
            .unwrap();
        for index in 0..5 {
            host.store
                .read()
                .add_work_item_event(
                    &item.id,
                    &view.session.id,
                    "note",
                    &format!("Earlier note {index}"),
                    "human",
                )
                .unwrap();
        }
        host.store
            .read()
            .add_work_item_event(
                &item.id,
                &view.session.id,
                "decision",
                "Latest decision: preserve the existing retry contract.",
                "human",
            )
            .unwrap();
        host.store
            .read()
            .save_conversation_summary(
                &view.session.id,
                "plan",
                "Use the existing idempotency key.",
                "msg-1..msg-2",
            )
            .unwrap();

        let context = host.durable_prediction_context(&view.session.id).unwrap();
        assert!(context.contains("Preserve retry idempotency"));
        assert!(context.contains("Do not retry a request"));
        assert!(context.contains("Earlier note 3"));
        assert!(context.contains("Earlier note 4"));
        assert!(
            context
                .contains("Latest decision: preserve the existing retry contract.")
        );
        assert!(context.contains("Use the existing idempotency key"));
    }

    #[test]
    fn prepared_fim_context_includes_instructions_discussion_and_artifact_refs()
    -> Result<()> {
        let workspace = tempfile::tempdir()?;
        std::fs::create_dir_all(workspace.path().join(".ahead"))?;
        std::fs::create_dir_all(workspace.path().join("src"))?;
        std::fs::write(
            workspace.path().join("AGENTS.md"),
            "Root instruction: preserve retry semantics.\n",
        )?;
        std::fs::write(
            workspace.path().join("src/AGENTS.md"),
            "Source instruction: keep the public signature.\n",
        )?;
        std::fs::write(
            workspace.path().join(".ahead/settings.toml"),
            "[ai]\nactive_connection = 'FIM test'\n\n[[ai.connections]]\nname = 'FIM test'\nprovider_id = 'fim-test'\nbase_url = 'http://127.0.0.1:1234/v1'\nmodel = 'qwen2.5-coder'\n",
        )?;

        let host = AheadSessionHost::in_memory()?;
        host.set_workspace(workspace.path().to_path_buf());
        let view = host.start_work(
            Some(WorkKind::ProductChange),
            "Retry implementation".into(),
            "Keep the committed-response boundary unchanged".into(),
            None,
        )?;
        let item = host.store.read().create_work_item(
            &view.session.id,
            "Preserve retry contract",
            "human",
        )?;
        host.store.read().insert_work_item_event_row(
            &ahead_rpc::ahead::WorkItemEvent {
                id: Uuid::new_v4().to_string(),
                item_id: item.id,
                session_id: view.session.id.clone(),
                kind: "decision".into(),
                body_markdown: "Do not retry after a committed response.".into(),
                actor_id: "human".into(),
                anchor_id: None,
                artifact_id: Some("artifact-retry-contract".into()),
                created_at: chrono::Utc::now().to_rfc3339(),
            },
        )?;

        {
            let store = host.store.read();
            for (role, content) in [
                ("human", "Keep the idempotency key on retries."),
                ("human_to:reviewer", "PRIVATE_REVIEW_SENTINEL"),
                ("agent", "I will preserve the commit boundary."),
                ("agent", "Incomplete streaming text must stay out."),
            ] {
                let sequence = store.next_message_sequence(&view.session.id)?;
                store.upsert_message(&ConversationMessage {
                    id: Uuid::new_v4().to_string(),
                    session_id: view.session.id.clone(),
                    turn_id: Uuid::new_v4().to_string(),
                    sequence,
                    role: role.into(),
                    actor_id: "human".into(),
                    content: content.into(),
                    status: if content.starts_with("Incomplete") {
                        "streaming".into()
                    } else {
                        "complete".into()
                    },
                    created_at: chrono::Utc::now().to_rfc3339(),
                    voice_timing: None,
                })?;
            }
            store.save_conversation_summary(
                &view.session.id,
                "implementation",
                "PRIVATE_SUMMARY_SENTINEL",
                "recent messages",
            )?;
        }

        let snapshot = host.prepare_prediction(
            PredictionRequest {
                request_id: "fim-session-context".into(),
                session_id: Some(view.session.id.clone()),
                path: "src/main.rs".into(),
                cursor: ahead_rpc::ahead::DisplayPosition { line: 0, col: 0 },
                prefix: "pub fn retry".into(),
                suffix: " {\n}".into(),
                work_context: "Editor context: preserve the selected call site."
                    .into(),
            },
            &[],
        )?;
        let context = &snapshot.request.work_context;
        assert!(context.contains("Retry implementation"));
        assert!(context.contains("Keep the committed-response boundary unchanged"));
        assert!(context.contains("Preserve retry contract"));
        assert!(context.contains("Do not retry after a committed response."));
        assert!(context.contains("artifact-retry-contract"));
        assert!(context.contains("Keep the idempotency key on retries."));
        assert!(context.contains("I will preserve the commit boundary."));
        assert!(context.contains("Root instruction: preserve retry semantics."));
        assert!(context.contains("Source instruction: keep the public signature."));
        assert!(
            context.contains("Editor context: preserve the selected call site.")
        );
        assert!(!context.contains("PRIVATE_REVIEW_SENTINEL"));
        assert!(!context.contains("PRIVATE_SUMMARY_SENTINEL"));
        assert!(!context.contains("Incomplete streaming text"));
        assert!(snapshot.session_context_version.is_some());
        Ok(())
    }

    #[test]
    fn oversized_prediction_plan_keeps_goal_constraints_active_step_and_progress()
    -> Result<()> {
        let host = AheadSessionHost::in_memory()?;
        let view = host.start_work(
            Some(WorkKind::ProductChange),
            "Compact plan context".to_string(),
            "Preserve the retry contract".to_string(),
            None,
        )?;
        let item = host.store.read().create_work_item(
            &view.session.id,
            "Keep retry behavior stable",
            "human",
        )?;
        host.store.read().add_work_item_event(
            &item.id,
            &view.session.id,
            "invariant",
            "Never retry after a committed response.",
            "human",
        )?;
        for index in 0..6 {
            host.store.read().add_work_item_event(
                &item.id,
                &view.session.id,
                "note",
                &format!("Progress note {index}"),
                "human",
            )?;
        }

        let mut plan = vec![ahead_rpc::ahead::AgentPlanEntry {
            content: "Preserve the retry contract".to_string(),
            status: "completed".to_string(),
            priority: "high".to_string(),
        }];
        for index in 0..260 {
            plan.push(ahead_rpc::ahead::AgentPlanEntry {
                content: format!(
                    "Pending step {index}: {}",
                    "follow-up detail ".repeat(16)
                ),
                status: "pending".to_string(),
                priority: "medium".to_string(),
            });
        }
        plan.push(ahead_rpc::ahead::AgentPlanEntry {
            content:
                "Update the retry path without changing its idempotency guarantee"
                    .to_string(),
            status: "in_progress".to_string(),
            priority: "high".to_string(),
        });
        host.store.read().set_agent_runtime_state(
            &view.session.id,
            &ahead_rpc::ahead::AgentRuntimeState {
                plan,
                ..Default::default()
            },
        )?;

        let (_, context) = host.cached_prediction_context(&view.session.id)?;
        assert!(context.contains("faithful compact summary"));
        assert!(context.contains("completed: 1"));
        assert!(context.contains("pending: 260"));
        assert!(context.contains("in_progress: 1"));
        assert!(
            context
                .contains("Plan opening: [completed] Preserve the retry contract")
        );
        assert!(context.contains(
            "Active plan step: [in_progress; priority high] Update the retry path"
        ));
        assert!(context.contains("Never retry after a committed response."));
        assert!(!context.contains("Pending step 259"));

        let (compact_version, compact_context) =
            host.cached_compacted_prediction_context(&view.session.id)?;
        assert!(compact_context.contains("Never retry after a committed response."));
        assert_eq!(
            (0..6)
                .filter(|index| {
                    compact_context.contains(&format!("Progress note {index}"))
                })
                .count(),
            1
        );
        host.store.read().add_work_item_event(
            &item.id,
            &view.session.id,
            "decision",
            "Preserve the retry key across the compacted context.",
            "human",
        )?;
        let (updated_version, updated_context) =
            host.cached_compacted_prediction_context(&view.session.id)?;
        assert!(updated_version.revision > compact_version.revision);
        assert!(
            updated_context
                .contains("Preserve the retry key across the compacted context.")
        );
        Ok(())
    }

    #[test]
    fn oversized_recent_work_history_uses_compact_context_and_keeps_invariants()
    -> Result<()> {
        let host = AheadSessionHost::in_memory()?;
        let view = host.start_work(
            Some(WorkKind::ProductChange),
            "Compact work history".to_string(),
            "Keep the retry contract".to_string(),
            None,
        )?;
        let item = host.store.read().create_work_item(
            &view.session.id,
            "Preserve retry behavior",
            "human",
        )?;
        host.store.read().add_work_item_event(
            &item.id,
            &view.session.id,
            "invariant",
            "Never retry after a committed response.",
            "human",
        )?;
        for index in 0..4 {
            host.store.read().add_work_item_event(
                &item.id,
                &view.session.id,
                "note",
                &format!("large-note-{index}: {}", "x".repeat(20_000)),
                "human",
            )?;
        }

        let (_, context) = host.cached_prediction_context(&view.session.id)?;
        assert!(context.contains("Never retry after a committed response."));
        assert_eq!(context.matches("large-note-").count(), 1);
        assert!(context.len() < MAX_PREDICTION_CONTEXT_BYTES);
        assert_eq!(host.cached_prediction_context(&view.session.id)?.1, context);
        Ok(())
    }

    #[test]
    fn prediction_session_cache_refreshes_across_connections_and_visibility()
    -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let database_path = temporary.path().join("session.db");
        let host = AheadSessionHost::new(SessionStore::open(&database_path)?);
        host.set_workspace(temporary.path().to_path_buf());
        let session = host.start_work(
            Some(WorkKind::ProductChange),
            "Cache test".into(),
            "Keep constraints".into(),
            None,
        )?;
        let id = &session.session.id;
        let (initial_version, _) = host.cached_prediction_context(id)?;
        let writer = SessionStore::open(&database_path)?;
        let item = writer.create_work_item(id, "Preserve idempotency", "human")?;
        writer.add_work_item_event(
            &item.id,
            id,
            "invariant",
            "Never retry a committed request",
            "human",
        )?;
        for index in 0..8 {
            writer.add_work_item_event(
                &item.id,
                id,
                "note",
                &format!("Progress {index}"),
                "human",
            )?;
        }
        writer.save_conversation_summary(
            id,
            "plan",
            "Public plan summary",
            "1..2",
        )?;
        writer.set_agent_runtime_state(
            id,
            &ahead_rpc::ahead::AgentRuntimeState {
                plan: vec![ahead_rpc::ahead::AgentPlanEntry {
                    content: "Check the response before retrying".into(),
                    status: "in_progress".into(),
                    priority: "high".into(),
                }],
                ..Default::default()
            },
        )?;
        let (version, context) = host.cached_prediction_context(id)?;
        assert!(version.revision > initial_version.revision);
        assert!(context.contains("Never retry a committed request"));
        assert!(context.contains("Check the response before retrying"));
        assert!(context.contains("Public plan summary"));
        assert_eq!(host.cached_prediction_context(id)?.1, context);
        assert_eq!(host.prediction_contexts.lock().len(), 1);

        let other = host.start_work(
            Some(WorkKind::ProductChange),
            "Other".into(),
            "Unrelated".into(),
            None,
        )?;
        writer.create_work_item(&other.session.id, "Other work", "human")?;
        writer.record_recent_workspace_file("unrelated.rs", 1)?;
        assert!(host.cached_prediction_context(id)?.0 == version);

        writer.upsert_message(&ConversationMessage {
            id: "private-message".into(),
            session_id: id.clone(),
            turn_id: "turn".into(),
            sequence: 1,
            role: "human_to:teammate".into(),
            actor_id: "human".into(),
            content: "Private discussion must not reach FIM".into(),
            status: "complete".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            voice_timing: None,
        })?;
        let (private_version, private_context) =
            host.cached_prediction_context(id)?;
        assert!(private_version.revision > version.revision);
        assert!(!private_context.contains("Public plan summary"));
        assert!(!private_context.contains("Private discussion"));
        assert!(private_context.contains("Never retry a committed request"));

        writer.set_agent_runtime_state(id, &Default::default())?;
        assert!(
            !host
                .cached_prediction_context(id)?
                .1
                .contains("Check the response before retrying")
        );
        host.set_workspace(temporary.path().to_path_buf());
        assert!(host.prediction_contexts.lock().is_empty());
        Ok(())
    }

    #[test]
    fn prediction_session_cache_bounds_sessions_and_rejects_oversized_work()
    -> Result<()> {
        let host = AheadSessionHost::in_memory()?;
        for index in 0..9 {
            host.cached_prediction_context(&format!("session-{index}"))?;
        }
        let contexts = host.prediction_contexts.lock();
        assert_eq!(contexts.len(), 8);
        assert!(!contexts.iter().any(|(id, _, _, _)| id == "session-0"));
        drop(contexts);
        host.cached_prediction_context("session-1")?;
        host.cached_prediction_context("session-9")?;
        assert!(
            !host
                .prediction_contexts
                .lock()
                .iter()
                .any(|(id, _, _, _)| id == "session-2")
        );

        let session = host.start_work(
            Some(WorkKind::ProductChange),
            "Large plan".into(),
            "Preserve constraints".into(),
            None,
        )?;
        host.store.read().set_agent_runtime_state(
            &session.session.id,
            &ahead_rpc::ahead::AgentRuntimeState {
                plan: vec![ahead_rpc::ahead::AgentPlanEntry {
                    content: "日本語".repeat(MAX_PREDICTION_CONTEXT_BYTES / 8),
                    status: "in_progress".into(),
                    priority: "high".into(),
                }],
                ..Default::default()
            },
        )?;
        let error = host
            .cached_prediction_context(&session.session.id)
            .err()
            .context("oversized plan was cached")?;
        assert!(error.to_string().contains("context budget"));
        assert!(
            !host
                .prediction_contexts
                .lock()
                .iter()
                .any(|(id, _, _, _)| id == &session.session.id)
        );
        host.store.read().set_agent_runtime_state(
            &session.session.id,
            &ahead_rpc::ahead::AgentRuntimeState {
                plan: vec![ahead_rpc::ahead::AgentPlanEntry {
                    content: "Keep the active step".into(),
                    status: "in_progress".into(),
                    priority: "high".into(),
                }],
                warnings: vec!["unrelated runtime field".repeat(100_000)],
                ..Default::default()
            },
        )?;
        let cached = host.cached_prediction_context(&session.session.id)?.1;
        assert!(cached.contains("Keep the active step"));
        assert!(!cached.contains("unrelated runtime field"));
        Ok(())
    }

    #[test]
    fn prediction_context_rejects_oversized_turso_rows_before_accumulating_them()
    -> Result<()> {
        let host = AheadSessionHost::in_memory()?;
        let session = host.start_work(
            Some(WorkKind::ProductChange),
            "Large work record".into(),
            "Preserve all constraints".into(),
            None,
        )?;
        host.store.read().create_work_item(
            &session.session.id,
            &"日本語".repeat(MAX_PREDICTION_CONTEXT_BYTES / 8),
            "human",
        )?;
        let error = host
            .cached_prediction_context(&session.session.id)
            .unwrap_err();
        assert!(error.to_string().contains("context budget"));
        assert!(
            !host
                .prediction_contexts
                .lock()
                .iter()
                .any(|(id, _, _, _)| id == &session.session.id)
        );
        Ok(())
    }

    #[test]
    fn prediction_session_cache_reuses_large_durable_work() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let ahead_directory = temporary.path().join(".ahead");
        std::fs::create_dir_all(&ahead_directory)?;
        std::fs::create_dir_all(temporary.path().join("src"))?;
        let database_path = ahead_directory.join("session.db");
        std::fs::write(
            ahead_directory.join("settings.toml"),
            r#"
                [ai]
                active_connection = "FIM timing"

                [[ai.connections]]
                name = "FIM timing"
                provider_id = "fim-timing"
                provider = "openai-compatible"
                base_url = "http://127.0.0.1:9/v1"
                model = "fim-timing-model"
            "#,
        )?;
        std::fs::write(temporary.path().join("src/main.rs"), "fn main() {}\n")?;
        let host = AheadSessionHost::new(SessionStore::open(&database_path)?);
        host.set_workspace(temporary.path().to_path_buf());
        let session = host.start_work(
            Some(WorkKind::ProductChange),
            "Context timing".into(),
            "Keep the full plan".into(),
            None,
        )?;
        for index in 0..100 {
            let item = host.store.read().create_work_item(
                &session.session.id,
                &format!("Work item {index}"),
                "human",
            )?;
            host.store.read().add_work_item_event(
                &item.id,
                &session.session.id,
                "invariant",
                "Preserve the public retry contract",
                "human",
            )?;
        }
        let session_id = session.session.id;
        drop(host);
        let host = AheadSessionHost::new(SessionStore::open(&database_path)?);
        let started = std::time::Instant::now();
        let (version, expected) = host.cached_prediction_context(&session_id)?;
        let cold = started.elapsed();
        let mut warm = Vec::new();
        for _ in 0..100 {
            let started = std::time::Instant::now();
            let (current, context) = host.cached_prediction_context(&session_id)?;
            warm.push(started.elapsed());
            assert!(current == version);
            assert_eq!(context, expected);
        }
        warm.sort();
        eprintln!(
            "100-work-item file-backed Turso FIM context after host reopen ({} bytes): cold {cold:?}, warm median {:?}, p95 {:?}",
            expected.len(),
            warm[50],
            warm[94]
        );
        assert_eq!(host.prediction_contexts.lock().len(), 1);

        drop(host);
        let host = AheadSessionHost::new(SessionStore::open(&database_path)?);
        host.set_workspace(temporary.path().to_path_buf());
        let request = PredictionRequest {
            request_id: "fim-timing".into(),
            session_id: Some(session_id),
            path: "src/main.rs".into(),
            cursor: ahead_rpc::ahead::DisplayPosition { line: 0, col: 11 },
            prefix: "p".repeat(8 * 1024),
            suffix: "s".repeat(4 * 1024),
            work_context: String::new(),
        };
        let model_context = PredictionModelContext {
            provider_id: "fim-timing".into(),
            model_id: "fim-timing-model".into(),
            context_window: Some(65_536),
        };
        let open_buffers = (0..4)
            .map(|index| OpenBufferContext {
                path: format!("src/related-{index}.rs"),
                relevant_excerpt: "c".repeat(4 * 1024),
            })
            .collect::<Vec<_>>();
        let started = std::time::Instant::now();
        let cold_snapshot = host.prepare_prediction_with_model_context(
            request.clone(),
            &open_buffers,
            Some(&model_context),
        )?;
        let cold_prepare = started.elapsed();
        let mut warm_prepare = Vec::new();
        for _ in 0..100 {
            let started = std::time::Instant::now();
            let current = host.prepare_prediction_with_model_context(
                request.clone(),
                &open_buffers,
                Some(&model_context),
            )?;
            warm_prepare.push(started.elapsed());
            assert!(current == cold_snapshot);
        }
        warm_prepare.sort();
        eprintln!(
            "full FIM context prep (100 work items, 12 KiB active-file excerpt, four 4 KiB related excerpts): cold after host reopen {cold_prepare:?}, warm median {:?}, p95 {:?}",
            warm_prepare[50], warm_prepare[94]
        );
        Ok(())
    }

    fn turn_dto(
        session_id: &str,
        msg: &str,
        policy_sha: &str,
    ) -> ahead_rpc::ahead::AgentTurnRequestDto {
        ahead_rpc::ahead::AgentTurnRequestDto {
            session_id: session_id.to_string(),
            thread_id: "thread-test".to_string(),
            harness: ahead_rpc::ahead::HarnessKind::Ahead,
            external_agent_id: None,
            model: None,
            model_provider: None,
            model_context_window: None,
            reasoning_effort: None,
            user_message: msg.to_string(),
            session_context: String::new(),
            context: ahead_rpc::ahead::TurnEditorContext {
                active_path: "src/retry.rs".to_string(),
                caret: ahead_rpc::ahead::DisplayPosition { line: 10, col: 0 },
                selection: None,
                file_content: "pub fn retry() {}".to_string(),
                selected_excerpt: None,
                visible_end: None,
                attached_anchor_ids: Vec::new(),
                attached_files: Vec::new(),
                attached_memories: Vec::new(),
            },
            invariants: Vec::new(),
            cwd: None,
            expected_policy_sha256: policy_sha.to_string(),
            read_only: false,
            scope: None,
        }
    }

    #[test]
    fn test_streamed_agent_turn_stale_policy_and_scope_reject() {
        let workspace = tempfile::tempdir().unwrap();
        let host = AheadSessionHost::in_memory().unwrap();
        host.set_workspace(workspace.path().to_path_buf());
        let view = host
            .start_work(
                Some(WorkKind::ProductChange),
                "Retries".to_string(),
                "Starting".to_string(),
                None,
            )
            .unwrap();
        // Stale policy fails closed.
        assert!(
            host.start_streamed_agent_turn(turn_dto(
                &view.session.id,
                "scaffold",
                "stale",
            ))
            .is_err()
        );
        // Out-of-scope active file rejected when scope allowlists other paths.
        let policy_sha = view.session.policy.sha256.clone();
        let mut dto = turn_dto(&view.session.id, "scaffold", &policy_sha);
        dto.scope = Some(ahead_rpc::ahead::TurnMechanicalScope {
            scope_id: "scope-1".to_string(),
            instruction: "only lib".to_string(),
            human_contract_artifact_id: "art-1".to_string(),
            allowed_paths: vec!["src/lib/".to_string()],
            approved_by: "owner".to_string(),
        });
        assert!(host.start_streamed_agent_turn(dto).is_err());

        let mut sibling_path = turn_dto(&view.session.id, "scaffold", &policy_sha);
        sibling_path.context.active_path = "src-private/retry.rs".to_string();
        sibling_path.scope = Some(ahead_rpc::ahead::TurnMechanicalScope {
            scope_id: "scope-2".to_string(),
            instruction: "only src".to_string(),
            human_contract_artifact_id: "art-1".to_string(),
            allowed_paths: vec!["src".to_string()],
            approved_by: "owner".to_string(),
        });
        let error = host.start_streamed_agent_turn(sibling_path).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("outside the approved mechanical scope")
        );
    }

    #[test]
    fn streamed_agent_turn_requires_current_policy_before_runtime_start() {
        let host = AheadSessionHost::in_memory().unwrap();
        let view = host
            .start_work(
                Some(WorkKind::ProductChange),
                "Policy gate".to_string(),
                "Starting".to_string(),
                None,
            )
            .unwrap();

        let missing = turn_dto(&view.session.id, "start", "");
        assert!(
            host.start_streamed_agent_turn(missing)
                .unwrap_err()
                .to_string()
                .contains("missing its expected policy hash")
        );

        let stale = turn_dto(&view.session.id, "start", "stale");
        assert!(
            host.start_streamed_agent_turn(stale)
                .unwrap_err()
                .to_string()
                .contains("Stale policy")
        );
    }

    #[test]
    fn test_export_restore_roundtrip_into_fresh_store() {
        let host = AheadSessionHost::in_memory().unwrap();
        let view = host
            .start_work(
                Some(WorkKind::ProductChange),
                "Export me".to_string(),
                "Starting".to_string(),
                None,
            )
            .unwrap();
        host.create_anchor(
            &view.session.id,
            "src/a.rs".to_string(),
            ahead_rpc::ahead::DisplayRange {
                start: ahead_rpc::ahead::DisplayPosition { line: 1, col: 0 },
                end: ahead_rpc::ahead::DisplayPosition { line: 2, col: 0 },
            },
            "fn a() {}".to_string(),
            "human",
        )
        .unwrap();
        let item = host
            .store
            .read()
            .create_work_item(&view.session.id, "Ship it", "owner")
            .unwrap();
        host.store
            .read()
            .save_conversation_summary(
                &view.session.id,
                "plan",
                "Agreed",
                "msg-1..msg-2",
            )
            .unwrap();
        let message = ahead_rpc::ahead::ConversationMessage {
            id: "msg-export-1".to_string(),
            session_id: view.session.id.clone(),
            turn_id: "turn-export-1".to_string(),
            sequence: 1,
            role: "human".to_string(),
            actor_id: "human".to_string(),
            content: "Keep the retry invariant.".to_string(),
            status: "complete".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            voice_timing: None,
        };
        host.store.read().upsert_message(&message).unwrap();
        let comment = CodeComment {
            id: "comment-export-1".into(),
            session_id: view.session.id.clone(),
            actor_id: "human".into(),
            path: "src/a.rs".into(),
            range: DisplayRange {
                start: ahead_rpc::ahead::DisplayPosition { line: 1, col: 0 },
                end: ahead_rpc::ahead::DisplayPosition { line: 1, col: 3 },
            },
            quote: "fn a() {}".into(),
            source_sha256: "a".repeat(64),
            body: "Check this branch".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            resolved_at: None,
            resolved_by: None,
        };
        host.store.read().insert_code_comment(&comment).unwrap();

        let bundle = host.session_export(&view.session.id).unwrap();
        assert_eq!(bundle.session.id, view.session.id);
        assert_eq!(bundle.anchors.len(), 1);
        assert_eq!(bundle.work_items.len(), 1);
        assert_eq!(bundle.conversation_summaries.len(), 1);
        assert_eq!(bundle.conversation_messages.len(), 1);
        assert_eq!(bundle.conversation_messages[0].content, message.content);
        assert_eq!(bundle.code_comments, vec![comment.clone()]);

        // Serde round-trip: clean-machine reconstruction path.
        let json = serde_json::to_string(&bundle).unwrap();
        let bundle2: ahead_rpc::ahead::SessionExportBundle =
            serde_json::from_str(&json).unwrap();

        // Restore into a fresh host; id collision fails closed.
        let fresh = AheadSessionHost::in_memory().unwrap();
        let restored = fresh.session_restore(bundle2).unwrap();
        assert_eq!(restored.session.id, view.session.id);
        assert_eq!(restored.workflow.phase.id, view.workflow.phase.id);
        let items = fresh
            .store
            .read()
            .list_work_items(&view.session.id)
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, item.id);
        let summaries = fresh
            .store
            .read()
            .list_conversation_summaries(&view.session.id)
            .unwrap();
        assert_eq!(summaries.len(), 1);
        let messages = fresh.store.read().list_messages(&view.session.id).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "Keep the retry invariant.");
        assert_eq!(
            fresh
                .store
                .read()
                .list_code_comments(&view.session.id)
                .unwrap(),
            vec![comment]
        );

        // Second restore of the same bundle refuses overwrite.
        let bundle3 = host.session_export(&view.session.id).unwrap();
        assert!(fresh.session_restore(bundle3).is_err());
    }

    #[test]
    fn documentation_root_is_project_configured_and_workspace_relative() -> Result<()>
    {
        let workspace = tempfile::tempdir()?;
        std::fs::create_dir(workspace.path().join(".ahead"))?;
        let host = AheadSessionHost::in_memory()?;
        host.set_workspace(workspace.path().to_path_buf());
        assert_eq!(host.documentation_root()?, "docs");
        std::fs::write(
            workspace.path().join(".ahead/config.toml"),
            "[documentation]\nroot = 'engineering'\n",
        )?;
        assert_eq!(host.documentation_root()?, "engineering");
        std::fs::write(
            workspace.path().join(".ahead/config.toml"),
            "[documentation]\nroot = '../outside'\n",
        )?;
        assert!(host.documentation_root().is_err());
        Ok(())
    }

    #[test]
    fn export_restore_preserves_agent_presentation_state() -> Result<()> {
        use ahead_rpc::ahead::{
            AgentCommand, AgentPlanEntry, AgentRuntimeState, AgentToolCall,
            AgentUsage,
        };

        let host = AheadSessionHost::in_memory()?;
        let view = host.start_work(
            Some(WorkKind::ProductChange),
            "Export agent state".to_string(),
            "Keep the plan and tool cards".to_string(),
            None,
        )?;
        let expected = AgentRuntimeState {
            turn_id: Some("turn-export-state".to_string()),
            plan: vec![AgentPlanEntry {
                content: "Verify restored context".to_string(),
                status: "completed".to_string(),
                priority: "medium".to_string(),
            }],
            tool_calls: vec![AgentToolCall {
                id: "tool-export-state".to_string(),
                title: "Search workspace".to_string(),
                status: "completed".to_string(),
                kind: "search".to_string(),
            }],
            usage: Some(AgentUsage {
                used_tokens: 128,
                context_window: Some(4096),
            }),
            commands: vec![AgentCommand {
                name: "review".to_string(),
                description: "Review current changes".to_string(),
                input: Some(r#"{"hint":"path"}"#.to_string()),
            }],
            warnings: vec!["Code Mode host unavailable".to_string()],
        };
        host.store
            .read()
            .set_agent_runtime_state(&view.session.id, &expected)?;
        let json = serde_json::to_vec(&host.session_export(&view.session.id)?)?;
        let bundle = serde_json::from_slice(&json)?;
        let temporary = tempfile::tempdir()?;
        let database = temporary.path().join("restored-session.db");
        let restored = AheadSessionHost::new(SessionStore::open(&database)?);
        restored.session_restore(bundle)?;
        drop(restored);

        let reopened = SessionStore::open(&database)?;
        assert_eq!(
            reopened.get_agent_runtime_state(&view.session.id)?,
            Some(expected)
        );
        assert!(reopened.get_harness_binding(&view.session.id)?.is_none());
        Ok(())
    }

    #[test]
    fn test_review_capture_attest_independence_and_rpc() {
        let host = AheadSessionHost::in_memory().unwrap();
        let view = host
            .start_work(
                Some(WorkKind::ProductChange),
                "Review me".to_string(),
                "Starting".to_string(),
                None,
            )
            .unwrap();

        // Capture via typed method.
        let snap = host
            .review_capture(
                &view.session.id,
                "tree-sha-1".to_string(),
                vec!["dev-impl".to_string()],
                vec!["finding: check bounds".to_string()],
            )
            .unwrap();
        assert!(!snap.is_approved);
        assert_eq!(snap.findings.len(), 1);

        // AHEAD records self-review; repository/team policy decides whether
        // it satisfies the applicable PR requirement.
        let self_review = host.review_attest(&snap.snapshot_id, "dev-impl").unwrap();
        assert!(self_review.is_approved);
        assert_eq!(self_review.reviewer_is_implementer, Some(true));
        // Another reviewer can attest as well.
        let approved = host.review_attest(&snap.snapshot_id, "reviewer-2").unwrap();
        assert!(approved.is_approved);
        assert_eq!(approved.approved_by.as_deref(), Some("reviewer-2"));
        assert_eq!(approved.reviewer_is_implementer, Some(false));

        // Via RPC dispatch (serde round-trip).
        let val = host
            .handle_request(AheadRequest::ReviewGet {
                session_id: view.session.id,
            })
            .unwrap();
        let got: Option<ahead_rpc::ahead::ReviewSnapshotDto> =
            serde_json::from_value(val).unwrap();
        assert!(got.is_some());
        let val = host
            .handle_request(AheadRequest::ReviewAttest {
                snapshot_id: snap.snapshot_id,
                reviewer_id: "reviewer-3".to_string(),
            })
            .unwrap();
        let reattested: ahead_rpc::ahead::ReviewSnapshotDto =
            serde_json::from_value(val).unwrap();
        assert_eq!(reattested.approved_by.as_deref(), Some("reviewer-3"));
    }
}
