//! AHEAD Session Host
//!
//! Grounded in Section 6 and Section 11 of `ahead-editor-mvp.md`.
//! Serves as the central mediation and effect boundary in `lapce-proxy`.
//! Manages active sessions, enforces phase transitions and Learn/Assist boundaries,
//! routes voice frames and prediction requests, and persists durable events.

use std::{collections::HashMap, sync::Arc};
use anyhow::{Context, Result};
use parking_lot::RwLock;
use sha2::Digest;
use uuid::Uuid;

use lapce_rpc::ahead::{
    AheadRequest, AssistanceMode, ChangeProposal, CodeAnchor,
    DisplayRange, GithubIssueRef, Id, Participant, PredictionRequest,
    PredictionResult, RepoPath, Revision, SessionLifecycle, SessionParticipantRecord,
    SessionPolicySnapshot, SessionRole, SessionView, VoiceControl, WorkKind,
    WorkSession, WorkflowPhase, WorkflowState,
};

use super::{
    auth::GitHubAuthManager,
    policy::PolicyEvaluator,
    prediction::{OpenBufferContext, PredictionEngine, PredictionWorkContext},
    store::SessionStore,
    voice::VoiceSession,
};

pub struct AheadSessionHost {
    store: Arc<RwLock<SessionStore>>,
    auth: Arc<RwLock<GitHubAuthManager>>,
    active_sessions: Arc<RwLock<HashMap<Id, SessionView>>>,
    active_voice_sessions: Arc<RwLock<HashMap<Id, Arc<VoiceSession>>>>,
    workspace_participants: Arc<RwLock<Vec<SessionParticipantRecord>>>,
}

impl AheadSessionHost {
    pub fn new(store: SessionStore) -> Self {
        let auth_mgr = GitHubAuthManager::new();
        let user = auth_mgr.get_active_user(None);
        let host_record = SessionParticipantRecord {
            participant: Participant::Human {
                id: user.login.clone(),
                subject: user.email.clone().unwrap_or_default(),
                display_name: user.name.clone().unwrap_or_else(|| user.login.clone()),
            },
            role: SessionRole::Owner,
        };
        Self {
            store: Arc::new(RwLock::new(store)),
            auth: Arc::new(RwLock::new(auth_mgr)),
            active_sessions: Arc::new(RwLock::new(HashMap::new())),
            active_voice_sessions: Arc::new(RwLock::new(HashMap::new())),
            workspace_participants: Arc::new(RwLock::new(vec![host_record])),
        }
    }

    pub fn in_memory() -> Result<Self> {
        let store = SessionStore::in_memory()?;
        Ok(Self::new(store))
    }

    /// Handles an incoming AheadRequest
    pub fn handle_request(&self, request: AheadRequest) -> Result<serde_json::Value> {
        match request {
            AheadRequest::StartWork {
                work_kind,
                mode,
                title,
                starting_point,
                work_item,
            } => {
                let view = self.start_work(
                    work_kind,
                    mode,
                    title,
                    starting_point,
                    work_item,
                )?;
                Ok(serde_json::to_value(view)?)
            }
            AheadRequest::GetSession { session_id } => {
                let session = self.get_session(&session_id)?;
                Ok(serde_json::to_value(session)?)
            }
            AheadRequest::ListSessions => {
                let sessions = self.list_sessions()?;
                Ok(serde_json::to_value(sessions)?)
            }
            AheadRequest::SetMode { session_id, mode } => {
                let session = self.set_mode(&session_id, mode)?;
                Ok(serde_json::to_value(session)?)
            }
            AheadRequest::AdvancePhase {
                session_id,
                expected_revision,
                target_phase_id,
            } => {
                let session = self.advance_phase(
                    &session_id,
                    expected_revision,
                    target_phase_id,
                )?;
                Ok(serde_json::to_value(session)?)
            }
            AheadRequest::CreateAnchor {
                session_id,
                path,
                range,
                quote,
            } => {
                let anchor = self.create_anchor(&session_id, path, range, quote)?;
                Ok(serde_json::to_value(anchor)?)
            }
            AheadRequest::ProposeEdit {
                session_id,
                proposal,
            } => {
                let res = self.propose_edit(&session_id, proposal)?;
                Ok(serde_json::to_value(res)?)
            }
            AheadRequest::AcceptProposal {
                session_id,
                proposal_id,
            } => {
                let res = self.accept_proposal(&session_id, &proposal_id)?;
                Ok(serde_json::to_value(res)?)
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
                let device_code = format!("{:x}", sha2::Sha256::digest(Uuid::new_v4().as_bytes()));
                let user_code = format!("{}-{}", &device_code[0..4].to_uppercase(), &device_code[4..8].to_uppercase());
                let resp = lapce_rpc::ahead::GitHubDeviceCodeResponse {
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
        if let Some(owner) = parts.iter_mut().find(|p| p.role == SessionRole::Owner) {
            owner.participant = Participant::Human {
                id: user.login.clone(),
                subject: user.email.clone().unwrap_or_default(),
                display_name: user.name.clone().unwrap_or_else(|| user.login.clone()),
            };
        }
        parts
    }

    pub fn add_workspace_participant(&self, user_handle: String, role: SessionRole) -> Result<Vec<SessionParticipantRecord>> {
        let handle = user_handle.trim().trim_start_matches('@').to_string();
        if handle.is_empty() || handle.contains('@') || handle.contains(' ') || handle.contains('/') {
            anyhow::bail!("Enter a GitHub username (e.g. octocat), not an email address");
        }
        let mut parts = self.workspace_participants.write();
        if let Some(existing) = parts.iter_mut().find(|p| p.participant.id() == handle) {
            existing.role = role;
        } else {
            parts.push(SessionParticipantRecord {
                participant: Participant::Human {
                    id: handle.clone(),
                    subject: handle.clone(),
                    display_name: format!("@{}", handle),
                },
                role,
            });
        }
        Ok(parts.clone())
    }

    pub fn revoke_workspace_participant(&self, user_handle: &str) -> Result<Vec<SessionParticipantRecord>> {
        let mut parts = self.workspace_participants.write();
        let handle = user_handle.trim().trim_start_matches('@');
        parts.retain(|p| p.participant.id() != handle || p.role == SessionRole::Owner);
        Ok(parts.clone())
    }

    pub fn start_work(
        &self,
        work_kind: WorkKind,
        mode: AssistanceMode,
        title: String,
        _starting_point: String,
        work_item: Option<GithubIssueRef>,
    ) -> Result<SessionView> {
        let session_id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();

        let initial_phase = match work_kind {
            WorkKind::ProductChange => WorkflowPhase {
                id: "plan".to_string(),
                title: "Planning".to_string(),
                visit: 1,
            },
            WorkKind::CorrectiveDebugging => WorkflowPhase {
                id: "hypothesize".to_string(),
                title: "Hypothesis & Reproduce".to_string(),
                visit: 1,
            },
            WorkKind::InternalImprovement => WorkflowPhase {
                id: "analyze-invariants".to_string(),
                title: "Analyze Invariants".to_string(),
                visit: 1,
            },
            WorkKind::Investigation => WorkflowPhase {
                id: "investigation-scrutinize".to_string(),
                title: "Scrutinize & Gather Evidence".to_string(),
                visit: 1,
            },
            WorkKind::Decision => WorkflowPhase {
                id: "decision-framing".to_string(),
                title: "Framing & Options".to_string(),
                visit: 1,
            },
            WorkKind::OperationalStabilization => WorkflowPhase {
                id: "stabilize".to_string(),
                title: "Triage & Stabilize".to_string(),
                visit: 1,
            },
        };

        let user = self.auth.read().get_active_user(None);
        let session = WorkSession {
            id: session_id.clone(),
            project_id: "project-local".to_string(),
            worktree_id: "worktree-local".to_string(),
            work_kind,
            mode,
            title,
            owner_id: user.login.clone(),
            lifecycle: SessionLifecycle::Active,
            policy: SessionPolicySnapshot::default(),
            revision: 1,
            created_at: now,
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
                backend_id: "codex-loop".to_string(),
                on_behalf_of: user.login.clone(),
                display_name: "AHEAD Pair".to_string(),
            },
            role: SessionRole::Editor,
        });

        let view = SessionView {
            session,
            workflow,
            participants,
        };

        {
            let mut store = self.store.write();
            store.insert_session(&view)?;
        }

        {
            let mut active = self.active_sessions.write();
            active.insert(session_id.clone(), view.clone());
        }

        // Initialize voice session
        {
            let voice_id = format!("voice-{}", session_id);
            let voice = Arc::new(VoiceSession::new(session_id.clone(), voice_id));
            let mut voice_map = self.active_voice_sessions.write();
            voice_map.insert(session_id, voice);
        }

        Ok(view)
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
    pub fn list_sessions(&self) -> Result<Vec<SessionView>> {
        let mut views = self.store.read().list_sessions()?;
        let active = self.active_sessions.read();
        for (id, view) in active.iter() {
            if let Some(existing) = views.iter_mut().find(|v| v.session.id == *id) {
                *existing = view.clone();
            } else {
                views.push(view.clone());
            }
        }
        Ok(views)
    }

    pub fn set_mode(&self, session_id: &str, mode: AssistanceMode) -> Result<()> {
        {
            let mut store = self.store.write();
            store.set_assistance_mode(session_id, mode)?;
        }
        let mut active = self.active_sessions.write();
        if let Some(view) = active.get_mut(session_id) {
            view.session.mode = mode;
            view.session.revision += 1;
        }
        Ok(())
    }

    pub fn advance_phase(
        &self,
        session_id: &str,
        expected_revision: Revision,
        target_phase_id: String,
    ) -> Result<WorkflowState> {
        let new_phase = WorkflowPhase {
            id: target_phase_id.clone(),
            title: format!("Phase {}", target_phase_id),
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

    pub fn create_anchor(
        &self,
        session_id: &str,
        path: RepoPath,
        range: DisplayRange,
        quote: String,
    ) -> Result<CodeAnchor> {
        let anchor_id = Uuid::new_v4().to_string();
        let quote_hash = format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(quote.as_bytes()));

        let anchor = CodeAnchor {
            id: anchor_id,
            session_id: session_id.to_string(),
            path,
            range,
            quote_hash,
            surrounding_context: Some(quote),
            created_at_commit: None,
        };

        let mut store = self.store.write();
        store.insert_anchor(&anchor)?;

        Ok(anchor)
    }

    pub fn propose_edit(&self, session_id: &str, proposal: ChangeProposal) -> Result<()> {
        let view = self.get_session(session_id)?
            .context("Session not found")?;

        PolicyEvaluator::authorize_proposal(
            &view.workflow.phase,
            view.session.mode,
            &view.session.policy,
            SessionRole::Editor,
            &proposal,
        )?;

        let mut store = self.store.write();
        store.insert_proposal(&proposal)?;
        Ok(())
    }

    pub fn accept_proposal(&self, _session_id: &str, proposal_id: &str) -> Result<()> {
        let mut store = self.store.write();
        store.accept_proposal(proposal_id)?;
        Ok(())
    }

    pub fn request_prediction(
        &self,
        request: PredictionRequest,
        open_buffers: &[OpenBufferContext],
    ) -> Result<PredictionResult> {
        let view = self.get_session(&request.session_id)?
            .context("Session not found")?;

        let work = PredictionWorkContext {
            work_kind: view.session.work_kind,
            mode: view.session.mode,
            phase_title: view.workflow.phase.title.clone(),
            primary_issue: view.workflow.primary_work_item.map(|i| format!("#{} {}", i.issue_number, i.title)),
            active_invariants: Vec::new(),
        };

        PredictionEngine::predict_mechanical(&work, &request, open_buffers)
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
    fn test_session_host_end_to_end_flow() {
        let host = AheadSessionHost::in_memory().unwrap();

        // 1. Start Work
        let view = host.start_work(
            WorkKind::ProductChange,
            AssistanceMode::Assist,
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
        ).unwrap();

        assert_eq!(view.session.work_kind, WorkKind::ProductChange);
        assert_eq!(view.workflow.phase.id, "plan");
        assert_eq!(view.workflow.revision, 1);

        // 2. Advance to implementation
        let next_wf = host.advance_phase(
            &view.session.id,
            1,
            "implement".to_string(),
        ).unwrap();
        assert_eq!(next_wf.phase.id, "implement");
        assert_eq!(next_wf.revision, 2);

        // Advancing with stale revision must fail
        let stale_res = host.advance_phase(&view.session.id, 1, "verify".to_string());
        assert!(stale_res.is_err());

        // 3. Anchoring code
        let anchor = host.create_anchor(
            &view.session.id,
            "src/retry.rs".to_string(),
            DisplayRange {
                start: lapce_rpc::ahead::DisplayPosition { line: 10, col: 0 },
                end: lapce_rpc::ahead::DisplayPosition { line: 15, col: 20 },
            },
            "pub fn retry() {}".to_string(),
        ).unwrap();
        assert_eq!(anchor.path, "src/retry.rs");

        // 4. Edit proposal in Assist mode
        let proposal = ChangeProposal {
            id: "prop-100".to_string(),
            session_id: view.session.id.clone(),
            path: "src/retry.rs".to_string(),
            original_sha256: "hash1".to_string(),
            patch: "+// retry boilerplate".to_string(),
            is_mechanical: true,
            description: "Add boilerplate".to_string(),
            recommended_cursor: None,
        };
        let prop_res = host.propose_edit(&view.session.id, proposal);
        assert!(prop_res.is_ok());

        // Accepting proposal
        let accept_res = host.accept_proposal(&view.session.id, "prop-100");
        assert!(accept_res.is_ok());

        // 5. Switching to Learn mode cancels ability to propose edits
        host.set_mode(&view.session.id, AssistanceMode::Learn).unwrap();
        let proposal_in_learn = ChangeProposal {
            id: "prop-101".to_string(),
            session_id: view.session.id.clone(),
            path: "src/retry.rs".to_string(),
            original_sha256: "hash2".to_string(),
            patch: "+// forbidden in learn".to_string(),
            is_mechanical: true,
            description: "Scaffold".to_string(),
            recommended_cursor: None,
        };
        let learn_prop_res = host.propose_edit(&view.session.id, proposal_in_learn);
        assert!(learn_prop_res.is_err());
    }
}
