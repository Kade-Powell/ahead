//! AHEAD Session Host
//!
//! Grounded in Section 6 and Section 11 of `ahead-editor-mvp.md`.
//! Serves as the central mediation and effect boundary in `ahead-proxy`.
//! Manages active sessions, enforces phase transitions and Learn/Assist boundaries,
//! routes voice frames and prediction requests, and persists durable events.

use anyhow::{Context, Result, bail};
use parking_lot::{Mutex, RwLock};
use sha2::Digest;
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

use ahead_rpc::ahead::{
    AheadRequest, AssistanceMode, CodeAnchor, CodeComment, DisplayRange,
    GithubIssueRef, HarnessKind, Id, LearningArc, LearningRecord, MemoryDocument,
    MemoryExcerpt, MemoryScope, MemoryWriteResult, Participant, PredictionRequest,
    PredictionResult, RepoPath, Revision, SessionExportBundle, SessionLifecycle,
    SessionListItem, SessionParticipantRecord, SessionPolicySnapshot, SessionRole,
    SessionTask, SessionView, TaskIntent, VoiceControl, WorkKind, WorkSession,
    WorkflowPhase, WorkflowState,
};

use super::{
    auth::GitHubAuthManager,
    prediction::{
        OpenBufferContext, PredictionEngine, PredictionProviderConfig,
        PredictionWorkContext,
    },
    store::SessionStore,
    voice::VoiceSession,
};
use ahead_agent::{HarnessController, path_is_allowed};
use ahead_core::search::WorkspaceFileIndex;

pub struct AheadSessionHost {
    store: Arc<RwLock<SessionStore>>,
    auth: Arc<RwLock<GitHubAuthManager>>,
    active_sessions: Arc<RwLock<HashMap<Id, SessionView>>>,
    active_session_id: Arc<RwLock<Option<Id>>>,
    active_voice_sessions: Arc<RwLock<HashMap<Id, Arc<VoiceSession>>>>,
    workspace_participants: Arc<RwLock<Vec<SessionParticipantRecord>>>,
    tracker: Arc<RwLock<super::tracker::TrackerAdapter>>,
    review_snapshots: Arc<RwLock<HashMap<Id, super::collab::ReviewSnapshot>>>,
    workspace: Arc<RwLock<Option<std::path::PathBuf>>>,
    memory_write_lock: Mutex<()>,
    recovery_owner: Mutex<Option<super::recovery::RecoveryOwner>>,
    /// Streams conversations through the built-in runtime or an external ACP agent.
    harness: HarnessController,
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
            active_session_id: Arc::new(RwLock::new(None)),
            active_voice_sessions: Arc::new(RwLock::new(HashMap::new())),
            workspace_participants: Arc::new(RwLock::new(vec![host_record])),
            tracker: Arc::new(RwLock::new(super::tracker::TrackerAdapter::new())),
            review_snapshots: Arc::new(RwLock::new(HashMap::new())),
            workspace: Arc::new(RwLock::new(None)),
            memory_write_lock: Mutex::new(()),
            recovery_owner: Mutex::new(None),
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
        self.harness.set_notification_sink(sink);
    }

    /// Shared streamed-harness controller (tests and status reporting).
    pub fn harness(&self) -> &HarnessController {
        &self.harness
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
                enabled,
            } => {
                let workspace = self.checked_mcp_workspace(&workspace)?;
                ahead_agent::set_mcp_server_approval(
                    &workspace,
                    &server_id,
                    &expected_fingerprint,
                    enabled,
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
            AheadRequest::AgentTurnCancel { session_id } => {
                let cancelled = self.harness.cancel_turn(&session_id)?;
                Ok(serde_json::json!({ "cancelled": cancelled }))
            }
            AheadRequest::AgentSessionPrepare { session_id } => {
                self.harness.prepare_external_session(&session_id)?;
                Ok(serde_json::json!({ "ready": true }))
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
            AheadRequest::WorkItemCreate { session_id, title } => {
                let owner = self.auth.read().get_active_user(None).login;
                let item = self.store.read().create_work_item(
                    &session_id,
                    &title,
                    &owner,
                )?;
                Ok(serde_json::to_value(item)?)
            }
            AheadRequest::WorkItemList { session_id } => {
                let items = self.store.read().list_work_items(&session_id)?;
                Ok(serde_json::to_value(items)?)
            }
            AheadRequest::WorkItemSetStatus { item_id, status } => {
                let item =
                    self.store.read().set_work_item_status(&item_id, status)?;
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
                Ok(serde_json::to_value(event)?)
            }
            AheadRequest::WorkItemClose {
                item_id,
                summary_markdown,
                issue_ref,
                follow_ups_json,
                skip_reason,
            } => {
                let closeout = self.store.read().close_work_item(
                    &item_id,
                    &summary_markdown,
                    issue_ref,
                    follow_ups_json,
                    skip_reason,
                )?;
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
            AheadRequest::VoiceControl { control } => {
                self.handle_voice_control(control)?;
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
                let parts = self.get_workspace_participants();
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

    pub fn get_workspace_participants(&self) -> Vec<SessionParticipantRecord> {
        let mut parts = self.workspace_participants.read().clone();
        let user = self.auth.read().get_active_user(None);
        if let Some(owner) = parts.iter_mut().find(|p| p.role == SessionRole::Owner)
        {
            owner.participant = Participant::Human {
                id: user.login.clone(),
                subject: user.email.clone().unwrap_or_default(),
                display_name: user
                    .name
                    .clone()
                    .unwrap_or_else(|| user.login.clone()),
            };
        }
        parts
    }

    pub fn add_workspace_participant(
        &self,
        user_handle: String,
        role: SessionRole,
    ) -> Result<Vec<SessionParticipantRecord>> {
        let handle = user_handle.trim().trim_start_matches('@').to_string();
        if handle.is_empty()
            || handle.contains('@')
            || handle.contains(' ')
            || handle.contains('/')
        {
            anyhow::bail!(
                "Enter a GitHub username (e.g. octocat), not an email address"
            );
        }
        let mut parts = self.workspace_participants.write();
        if let Some(existing) =
            parts.iter_mut().find(|p| p.participant.id() == handle)
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
            p.participant.id() != handle || p.role == SessionRole::Owner
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

        let mut participants = self.get_workspace_participants();
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
        *self.active_session_id.write() = Some(session_id.clone());

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
        self.store.read().sync_memory_file(scope.as_str(), &path)
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
            Ok(metadata) if metadata.file_type().is_symlink() => {
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
            Ok(metadata) if metadata.file_type().is_symlink() => {
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
            Ok(metadata) if metadata.file_type().is_symlink() => {
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

    fn read_memory_content(path: &std::path::Path) -> Result<String> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let mut file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(String::new());
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("Failed to read AHEAD memory file {}", path.display())
                });
            }
        };
        if !file.metadata()?.file_type().is_file() {
            bail!("AHEAD memory source must be a regular file");
        }
        const MEMORY_DOCUMENT_LIMIT: u64 =
            ahead_rpc::ahead::MEMORY_DOCUMENT_MAX_BYTES as u64;
        if file.metadata()?.len() > MEMORY_DOCUMENT_LIMIT {
            bail!("AHEAD memory document exceeds the 32 KiB limit");
        }
        let mut content = String::new();
        (&mut file)
            .take(MEMORY_DOCUMENT_LIMIT + 1)
            .read_to_string(&mut content)
            .with_context(|| {
                format!("Failed to read AHEAD memory file {}", path.display())
            })?;
        if content.len() as u64 > MEMORY_DOCUMENT_LIMIT {
            bail!("AHEAD memory document exceeds the 32 KiB limit");
        }
        Ok(content)
    }

    fn read_memory(&self, scope: MemoryScope) -> Result<MemoryDocument> {
        let _read_guard = self.memory_write_lock.lock();
        let path = self.memory_source_path(scope, false)?;
        let content = Self::read_memory_content(&path)?;
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
        let current = Self::read_memory_content(&path)?;
        let current_sha256 =
            format!("{:x}", sha2::Sha256::digest(current.as_bytes()));
        if current_sha256 != expected_sha256 {
            bail!(
                "AHEAD memory changed since review; read it again before replacing"
            );
        }

        let parent = path
            .parent()
            .context("AHEAD memory file has no parent directory")?;
        let temporary_path =
            parent.join(format!(".MEMORY.md.{}.tmp", Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
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

        let index = self.store.read().sync_memory_file(scope.as_str(), &path);
        let (indexed, warning) = match index {
            Ok(_) => (true, None),
            Err(error) => (false, Some(error.to_string())),
        };
        Ok(MemoryWriteResult { indexed, warning })
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
        let mut options = OpenOptions::new();
        options.read(true).create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let mut file = options.open(&path).with_context(|| {
            format!("Failed to open AHEAD memory file {}", path.display())
        })?;
        if !file.metadata()?.file_type().is_file() {
            bail!("AHEAD memory source must be a regular file");
        }
        file.seek(SeekFrom::Start(0)).with_context(|| {
            format!("Failed to read AHEAD memory file {}", path.display())
        })?;
        let mut existing = String::new();
        file.read_to_string(&mut existing).with_context(|| {
            format!("Failed to read AHEAD memory file {}", path.display())
        })?;
        if existing.lines().any(|line| line.trim() == marker) {
            let index = self.store.read().sync_memory_file(scope.as_str(), &path);
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

        let index = self.store.read().sync_memory_file(scope.as_str(), &path);
        let (indexed, warning) = match index {
            Ok(_) => (true, None),
            Err(error) => (false, Some(error.to_string())),
        };
        Ok(MemoryWriteResult { indexed, warning })
    }

    pub fn archive_session(&self, session_id: &str) -> Result<()> {
        self.store.read().archive_session(session_id)?;
        self.active_sessions.write().remove(session_id);
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

    fn comment_actor(&self, session_id: &str) -> Result<String> {
        let view = self.get_session(session_id)?.context("Session not found")?;
        anyhow::ensure!(
            self.list_sessions()?
                .iter()
                .any(|session| session.id == session_id),
            "Cannot comment on an archived session"
        );
        let actor_id = self.auth.read().get_active_user(None).login;
        anyhow::ensure!(
            view.participants.iter().any(|record| matches!(
                &record.participant, Participant::Human { id, .. } if id == &actor_id
            )),
            "Only a session participant can comment"
        );
        Ok(actor_id)
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
        let items = store.list_work_items(session_id)?;
        let summaries = store.list_conversation_summaries(session_id)?;
        let mut context = String::new();
        if !items.is_empty() {
            context.push_str("Work items:\n");
            for item in &items {
                context.push_str(&format!("- [{:?}] {}\n", item.status, item.title));
                for event in store.list_work_item_events(&item.id, Some(4))? {
                    context.push_str(&format!(
                        "  - {}: {}\n",
                        event.kind, event.body_markdown
                    ));
                }
            }
        }
        if !summaries.is_empty() {
            context.push_str("Conversation summaries:\n");
            for summary in summaries.iter().rev().take(4).rev() {
                context.push_str(&format!(
                    "- {}: {}\n",
                    summary.phase, summary.summary_markdown
                ));
            }
        }
        Ok(context)
    }

    fn durable_session_context(&self, view: &SessionView) -> Result<String> {
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
        context.push_str(&format!(
            "Durable documentation root: {}/. When creating lasting documents, use topic-named Markdown under research/, design/, plans/, verification/, or reviews/. Session history expires 30 days after archive.\n",
            self.documentation_root()?,
        ));
        let durable = self.durable_prediction_context(&view.session.id)?;
        if !durable.is_empty() {
            context.push_str("Durable work state:\n");
            context.push_str(&durable);
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
            for message in page.messages {
                let excerpt: String = message.content.chars().take(1200).collect();
                context.push_str(&format!("{}: {}\n", message.role, excerpt));
            }
        }
        Ok(context)
    }

    pub fn request_prediction(
        &self,
        mut request: PredictionRequest,
        open_buffers: &[OpenBufferContext],
    ) -> Result<PredictionResult> {
        let requested_session = request.session_id.clone();
        let session_id =
            if requested_session.is_empty() || requested_session == "active" {
                self.active_session_id.read().clone()
            } else {
                Some(requested_session.clone())
            };
        let view = session_id
            .as_deref()
            .map(|id| self.get_session(id))
            .transpose()?
            .flatten();
        if !requested_session.is_empty()
            && requested_session != "active"
            && view.is_none()
        {
            bail!("Session not found");
        }

        let work = if let Some(view) = view {
            let caller_context = std::mem::take(&mut request.work_context);
            request.work_context = self.durable_session_context(&view)?;
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
            request.work_context = if request.work_context.trim().is_empty() {
                instruction_context
            } else {
                format!("{instruction_context}\n\n{}", request.work_context)
            };
        }
        let provider = PredictionProviderConfig::from_workspace(Some(&workspace))?;
        PredictionEngine::predict(&work, &request, open_buffers, &provider)
    }

    fn start_streamed_agent_turn(
        &self,
        mut dto: ahead_rpc::ahead::AgentTurnRequestDto,
    ) -> Result<Id> {
        let view = self
            .get_session(&dto.session_id)?
            .context("Session not found")?;
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
        if dto.session_context.trim().is_empty() {
            dto.session_context = self.durable_session_context(&view)?;
        }
        self.harness.start_turn(dto)
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

    pub fn handle_voice_control(&self, control: VoiceControl) -> Result<()> {
        let voice_map = self.active_voice_sessions.read();
        for voice in voice_map.values() {
            voice.handle_control(control.clone());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[cfg(unix)]
    #[test]
    fn memory_read_refuses_a_symlinked_memory_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = temp.path().join("workspace");
        let memory_path = workspace.join(".ahead/memories/MEMORY.md");
        let external_path = temp.path().join("outside.md");
        std::fs::create_dir_all(memory_path.parent().context("memory parent")?)?;
        std::fs::write(&external_path, "leave this file alone")?;
        std::os::unix::fs::symlink(&external_path, &memory_path)?;
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
        let existing = "x".repeat(ahead_rpc::ahead::MEMORY_DOCUMENT_MAX_BYTES);
        std::fs::write(&memory_path, &existing)?;
        let host = AheadSessionHost::in_memory()?;
        host.set_workspace(workspace);

        let error = host
            .handle_request(AheadRequest::WriteMemory {
                scope: MemoryScope::Project,
                message_id: "document-limit-message".into(),
                content: "this append must not exceed the document limit".into(),
            })
            .expect_err("memory append must preserve the document limit");

        assert!(error.to_string().contains("32 KiB limit"));
        assert_eq!(std::fs::read_to_string(memory_path)?, existing);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn memory_write_refuses_a_symlinked_memory_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = temp.path().join("workspace");
        let memory_path = workspace.join(".ahead/memories/MEMORY.md");
        let external_path = temp.path().join("outside.md");
        std::fs::create_dir_all(memory_path.parent().context("memory parent")?)?;
        std::fs::write(&external_path, "leave this file alone")?;
        std::os::unix::fs::symlink(&external_path, &memory_path)?;
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

    #[cfg(unix)]
    #[test]
    fn memory_write_refuses_a_symlinked_memory_directory() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let workspace = temp.path().join("workspace");
        let ahead_directory = workspace.join(".ahead");
        let memory_directory = ahead_directory.join("memories");
        let external_directory = temp.path().join("outside-memories");
        std::fs::create_dir_all(&ahead_directory)?;
        std::os::unix::fs::symlink(&external_directory, &memory_directory)?;
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
        assert!(!external_directory.exists());
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
        assert!(host.active_session_id.read().is_none());
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
        assert_eq!(host.active_session_id.read().as_deref(), Some(session_id));
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
        assert!(host.active_session_id.read().is_none());
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
    fn prediction_resolves_the_active_durable_session() {
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
                    session_id: "active".to_string(),
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
        assert!(!context.contains("Do not retry a request"));
        assert!(context.contains("Earlier note 3"));
        assert!(context.contains("Earlier note 4"));
        assert!(
            context
                .contains("Latest decision: preserve the existing retry contract.")
        );
        assert!(context.contains("Use the existing idempotency key"));
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
            user_message: msg.to_string(),
            session_context: String::new(),
            context: ahead_rpc::ahead::TurnEditorContext {
                active_path: "src/retry.rs".to_string(),
                caret: ahead_rpc::ahead::DisplayPosition { line: 10, col: 0 },
                selection: None,
                file_content: "pub fn retry() {}".to_string(),
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
        let host = AheadSessionHost::in_memory().unwrap();
        host.set_workspace(PathBuf::from("/workspace"));
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
                total_tokens: 128,
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
