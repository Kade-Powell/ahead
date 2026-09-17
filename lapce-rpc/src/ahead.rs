//! AHEAD Native Editor Protocol & DTOs
//!
//! Grounded in `docs/development/ahead-editor-contracts.ts` and `docs/development/ahead-editor-mvp.md`.
//! Serves as the typed RPC and storage bridge between the Floem UI, Proxy Session Host,
//! and governing agent runtimes.

use serde::{Deserialize, Serialize};

pub type Id = String;
pub type Sha256 = String;
pub type GitOid = String;
pub type Timestamp = String;
pub type Revision = u64;
pub type RepoPath = String;

/// The six core work categories
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkKind {
    ProductChange,
    CorrectiveDebugging,
    InternalImprovement,
    Investigation,
    Decision,
    OperationalStabilization,
}

impl WorkKind {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::ProductChange => "Product Change",
            Self::CorrectiveDebugging => "Corrective Debugging",
            Self::InternalImprovement => "Internal Improvement",
            Self::Investigation => "Investigation",
            Self::Decision => "Decision",
            Self::OperationalStabilization => "Operational Stabilization",
        }
    }
}

/// Assistance modes defining the capability ceiling
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistanceMode {
    /// Read-only tools, no command execution, no automated edit generation or predictions.
    Learn,
    /// Bounded mechanical work, tests for existing behavior, docs, boilerplate scaffolding.
    Assist,
}

/// Native capability flags enforced by the session host
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    ReadContext,
    PresentCode,
    RecordDraft,
    ProposeEdit,
    RunApprovedCheck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRole {
    Owner,
    Editor,
    Reviewer,
    Viewer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Participant {
    Human {
        id: Id,
        subject: String,
        display_name: String,
    },
    Ai {
        id: Id,
        backend_id: Id,
        on_behalf_of: Id,
        display_name: String,
    },
    System {
        id: Id,
        service: String,
        display_name: String,
    },
}

impl Participant {
    pub fn id(&self) -> &str {
        match self {
            Self::Human { id, .. } | Self::Ai { id, .. } | Self::System { id, .. } => id,
        }
    }

    pub fn display_name(&self) -> &str {
        match self {
            Self::Human { display_name, .. }
            | Self::Ai { display_name, .. }
            | Self::System { display_name, .. } => display_name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPolicySnapshot {
    pub id: Id,
    pub sha256: Sha256,
    pub assistance: String,
    pub work_item_required_before_phase: Option<String>,
    pub allowed_provider_ids: Vec<Id>,
    pub private_session: bool,
    pub external_writes: String,
    pub predictions: String,
}

impl Default for SessionPolicySnapshot {
    fn default() -> Self {
        Self {
            id: "policy-default".to_string(),
            sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            assistance: "maieutic-strict".to_string(),
            work_item_required_before_phase: Some("implement".to_string()),
            allowed_provider_ids: vec!["local-builtin".to_string()],
            private_session: true,
            external_writes: "human-only".to_string(),
            predictions: "explicit-mechanical-scope".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionLifecycle {
    Active,
    Paused { checkpoint_id: Id },
    Completed { checkpoint_id: Id },
    Archived { previous: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkSession {
    pub id: Id,
    pub project_id: Id,
    pub worktree_id: Id,
    pub work_kind: WorkKind,
    pub mode: AssistanceMode,
    pub title: String,
    pub owner_id: Id,
    pub lifecycle: SessionLifecycle,
    pub policy: SessionPolicySnapshot,
    pub revision: Revision,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowPhase {
    pub id: String,
    pub title: String,
    pub visit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubIssueRef {
    pub host: String,
    pub owner: String,
    pub repo: String,
    pub issue_number: u64,
    pub issue_node_id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRecord {
    pub artifact_revision_id: Id,
    pub content_sha256: Sha256,
    pub policy_sha256: Sha256,
    pub human_participant_id: Id,
    pub approved_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowState {
    pub revision: Revision,
    pub definition_version: String,
    pub phase: WorkflowPhase,
    pub primary_work_item: Option<GithubIssueRef>,
    pub current_artifact_ids: Vec<Id>,
    pub approvals: Vec<ApprovalRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionParticipantRecord {
    pub participant: Participant,
    pub role: SessionRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionView {
    pub session: WorkSession,
    pub workflow: WorkflowState,
    pub participants: Vec<SessionParticipantRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayPosition {
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayRange {
    pub start: DisplayPosition,
    pub end: DisplayPosition,
}

/// Durable code anchor for threads and presentations
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeAnchor {
    pub id: Id,
    pub session_id: Id,
    pub path: RepoPath,
    pub range: DisplayRange,
    pub quote_hash: Sha256,
    pub surrounding_context: Option<String>,
    pub created_at_commit: Option<GitOid>,
}

/// Presentation cue: highlights code without moving the human's caret
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentationCue {
    pub cue_id: Id,
    pub anchor: CodeAnchor,
    pub label: String,
    pub pointer_target: Option<DisplayPosition>,
    pub author_id: Id,
    pub display_duration_ms: Option<u64>,
}

/// Full-duplex voice frames and events
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceAudioChunk {
    pub voice_session_id: Id,
    pub epoch: u64,
    pub generation: u64,
    pub sequence: u64,
    pub audio_base64: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceTranscriptUpdate {
    pub voice_session_id: Id,
    pub epoch: u64,
    pub generation: u64,
    pub text: String,
    pub is_final: bool,
    pub speaker_id: Id,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoicePlaybackReceipt {
    pub voice_session_id: Id,
    pub generation: u64,
    pub played_up_to_offset_ms: u64,
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VoiceControl {
    InterruptPlayback { generation: u64 },
    SetMicMuted { muted: bool },
    SetSpeakerMuted { muted: bool },
    CancelCodingTask { task_id: Id },
}

/// Fast edit prediction payload
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PredictionRequest {
    pub request_id: Id,
    pub session_id: Id,
    pub path: RepoPath,
    pub cursor: DisplayPosition,
    pub prefix: String,
    pub suffix: String,
    pub work_context: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PredictionResult {
    pub request_id: Id,
    pub replacement: String,
    pub cursor_offset: usize,
}

/// Change proposal requiring explicit human approval
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeProposal {
    pub id: Id,
    pub session_id: Id,
    pub path: RepoPath,
    pub original_sha256: Sha256,
    pub patch: String,
    pub is_mechanical: bool,
    pub description: String,
    pub recommended_cursor: Option<DisplayPosition>,
}

/// RPC requests from UI (lapce-app) to Session Host (lapce-proxy)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum AheadRequest {
    StartWork {
        work_kind: WorkKind,
        mode: AssistanceMode,
        title: String,
        starting_point: String,
        work_item: Option<GithubIssueRef>,
    },
    GetSession {
        session_id: Id,
    },
    SetMode {
        session_id: Id,
        mode: AssistanceMode,
    },
    AdvancePhase {
        session_id: Id,
        expected_revision: Revision,
        target_phase_id: String,
    },
    CreateAnchor {
        session_id: Id,
        path: RepoPath,
        range: DisplayRange,
        quote: String,
    },
    ProposeEdit {
        session_id: Id,
        proposal: ChangeProposal,
    },
    AcceptProposal {
        session_id: Id,
        proposal_id: Id,
    },
    RequestPrediction {
        request: PredictionRequest,
    },
    VoiceControl {
        control: VoiceControl,
    },
    GitHubAuthStart,
    GitHubAuthPoll {
        device_code: String,
    },
    GitHubAuthSignInWithToken {
        token: String,
    },
    GitHubAuthDetectCli,
    GitHubAuthSignOut,
    GetAuthenticatedUser,
    GetWorkspaceParticipants,
    AddWorkspaceParticipant {
        user_handle: String,
        role: SessionRole,
    },
    RevokeWorkspaceParticipant {
        user_handle: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubUser {
    pub login: String,
    pub id: u64,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    pub email: Option<String>,
    pub is_authenticated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubDeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}


/// RPC notifications from Session Host to UI
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum AheadNotification {
    SessionUpdated {
        view: SessionView,
    },
    PresentationCueRevealed {
        cue: PresentationCue,
    },
    PresentationCueDismissed {
        cue_id: Id,
    },
    VoiceTranscript {
        update: VoiceTranscriptUpdate,
    },
    VoiceAudio {
        chunk: VoiceAudioChunk,
    },
    PredictionReady {
        result: PredictionResult,
    },
}
