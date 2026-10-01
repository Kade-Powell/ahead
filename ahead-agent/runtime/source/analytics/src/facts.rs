use codex_protocol::config_types::ModeKind;
use codex_protocol::config_types::Personality;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::config_types::ServiceTier;
pub use codex_protocol::error::CodexErrKind;
use codex_protocol::models::PermissionProfile;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::HookEventName;
use codex_protocol::protocol::HookExecutionMode;
use codex_protocol::protocol::HookHandlerType;
use codex_protocol::protocol::HookRunStatus;
use codex_protocol::protocol::HookSource;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SkillScope;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::protocol::ThreadSource;
use codex_protocol::protocol::TokenUsage;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct TrackEventsContext {
    pub model_slug: String,
    pub thread_id: String,
    pub turn_id: String,
    pub product_client_id: String,
}

#[derive(Clone)]
pub enum CodeModeToolCallFact {
    CellStarted {
        thread_id: String,
        turn_id: String,
        call_id: String,
        cell_id: String,
    },
    ChildStarted {
        thread_id: String,
        turn_id: String,
        call_id: String,
        cell_id: String,
    },
    CellClosed {
        thread_id: String,
        turn_id: String,
        cell_id: String,
    },
    SamplingResponseCompleted {
        thread_id: String,
        turn_id: String,
        response_id: String,
        tool_call_ids: Vec<String>,
    },
    Completed {
        thread_id: String,
        turn_id: String,
        turn_metadata: Arc<dyn TurnAnalyticsMetadata>,
        call_id: String,
        cell_id: Option<String>,
        tool_name: String,
        started_at_ms: u64,
        completed_at_ms: u64,
        status: CodeModeToolCallStatus,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeModeToolCallStatus {
    Completed,
    Failed,
    Interrupted,
}

#[derive(Clone)]
pub struct ControlToolCallFact {
    pub thread_id: String,
    pub turn_id: String,
    pub turn_metadata: Arc<dyn TurnAnalyticsMetadata>,
    pub call_id: String,
    pub cell_id: Option<String>,
    pub tool_name: String,
    pub started_at_ms: u64,
    pub completed_at_ms: u64,
    pub status: ControlToolCallStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlToolCallStatus {
    Completed,
    Failed,
    Rejected,
    Interrupted,
}

pub fn build_track_events_context(
    model_slug: String,
    thread_id: String,
    turn_id: String,
    product_client_id: String,
) -> TrackEventsContext {
    TrackEventsContext {
        model_slug,
        thread_id,
        turn_id,
        product_client_id,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageDetailSetting {
    High,
    Original,
}

/// Measurements for one successfully decoded image at the point where Codex prepares it for
/// durable conversation history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ImagePreparationMetadata {
    /// Set for images embedded in message content.
    pub message_role: Option<String>,
    /// Set to the originating call ID for tool-output images. This joins to the `item_id` on
    /// existing tool events for tool type and provenance.
    pub item_id: Option<String>,
    pub effective_detail: ImageDetailSetting,
    pub source_width: u32,
    pub source_height: u32,
    pub prepared_width: u32,
    pub prepared_height: u32,
}

#[derive(Clone)]
pub struct ImagePreparationFact {
    pub turn_id: String,
    pub metadata: ImagePreparationMetadata,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnSubmissionType {
    Default,
    Queued,
}

#[derive(Clone)]
pub struct TurnResolvedConfigFact {
    pub turn_id: String,
    pub thread_id: String,
    pub turn_metadata: Arc<dyn TurnAnalyticsMetadata>,
    pub num_input_images: usize,
    pub submission_type: Option<TurnSubmissionType>,
    pub ephemeral: bool,
    pub session_source: SessionSource,
    pub model: String,
    pub model_provider: String,
    pub permission_profile: PermissionProfile,
    pub permission_profile_cwd: PathBuf,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub reasoning_summary: Option<ReasoningSummary>,
    pub service_tier: Option<ServiceTier>,
    pub approval_policy: AskForApproval,
    pub sandbox_network_access: bool,
    pub collaboration_mode: ModeKind,
    pub personality: Option<Personality>,
    pub workspace_kind: Option<String>,
    pub is_first_turn: bool,
}

/// A live, read-only view of a turn's trusted analytics provenance.
///
/// Implementations must return `None` for unknown or ambiguous roots. The reducer
/// reads this when constructing each event because steering can invalidate a root
/// after the turn's configuration has been resolved.
pub trait TurnAnalyticsMetadata: Send + Sync {
    fn root_turn_id(&self) -> Option<String>;
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadInitializationMode {
    New,
    Forked,
    Resumed,
}

#[derive(Clone)]
pub struct TurnTokenUsageFact {
    pub turn_id: String,
    pub thread_id: String,
    pub token_usage: TokenUsage,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TurnProfile {
    pub before_first_sampling_ms: u64,
    pub sampling_ms: u64,
    pub compaction_ms: u64,
    pub between_sampling_overhead_ms: u64,
    pub tool_blocking_ms: u64,
    pub after_last_sampling_ms: u64,
    pub sampling_request_count: u32,
    pub sampling_retry_count: u32,
}

#[derive(Clone)]
pub struct TurnProfileFact {
    pub turn_id: String,
    pub profile: TurnProfile,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Completed,
    Failed,
    Interrupted,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnSteerResult {
    Accepted,
    Rejected,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnSteerRejectionReason {
    NoActiveTurn,
    ExpectedTurnMismatch,
    NonSteerableReview,
    NonSteerableCompact,
    EmptyInput,
    InputTooLarge,
}

#[derive(Clone)]
pub struct CodexTurnSteerEvent {
    pub expected_turn_id: Option<String>,
    pub accepted_turn_id: Option<String>,
    pub num_input_images: usize,
    pub result: TurnSteerResult,
    pub rejection_reason: Option<TurnSteerRejectionReason>,
    pub created_at: u64,
}

#[derive(Clone, Copy, Debug)]
pub enum AnalyticsJsonRpcError {
    TurnSteer(TurnSteerRequestError),
    Input(InputError),
}

#[derive(Clone, Copy, Debug)]
pub enum TurnSteerRequestError {
    NoActiveTurn,
    ExpectedTurnMismatch,
    NonSteerableReview,
    NonSteerableCompact,
}

#[derive(Clone, Copy, Debug)]
pub enum InputError {
    Empty,
    TooLarge,
}

impl From<TurnSteerRequestError> for TurnSteerRejectionReason {
    fn from(error: TurnSteerRequestError) -> Self {
        match error {
            TurnSteerRequestError::NoActiveTurn => Self::NoActiveTurn,
            TurnSteerRequestError::ExpectedTurnMismatch => Self::ExpectedTurnMismatch,
            TurnSteerRequestError::NonSteerableReview => Self::NonSteerableReview,
            TurnSteerRequestError::NonSteerableCompact => Self::NonSteerableCompact,
        }
    }
}

impl From<InputError> for TurnSteerRejectionReason {
    fn from(error: InputError) -> Self {
        match error {
            InputError::Empty => Self::EmptyInput,
            InputError::TooLarge => Self::InputTooLarge,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SkillInvocation {
    pub skill_name: String,
    pub location: SkillInvocationLocation,
    pub invocation_type: InvocationType,
}

#[derive(Clone, Debug)]
pub enum SkillInvocationLocation {
    Host {
        path: PathBuf,
        scope: SkillScope,
    },
    Resource {
        id: String,
        skill_id: Option<String>,
        scope: Option<SkillScope>,
    },
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InvocationType {
    Explicit,
    Implicit,
}

#[derive(Clone)]
pub struct SubAgentThreadStartedInput {
    pub session_id: String,
    pub thread_id: String,
    pub parent_thread_id: Option<String>,
    pub forked_from_thread_id: Option<String>,
    pub product_client_id: String,
    pub client_name: Option<String>,
    pub client_version: Option<String>,
    pub model: String,
    pub ephemeral: bool,
    pub thread_source: Option<ThreadSource>,
    pub subagent_source: SubAgentSource,
    pub created_at: u64,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionTrigger {
    Manual,
    Auto,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionReason {
    UserRequested,
    ContextLimit,
    ModelDownshift,
    CompHashChanged,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionImplementation {
    Responses,
    ResponsesCompactionV2,
    ResponsesCompact,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionPhase {
    StandaloneTurn,
    PreTurn,
    MidTurn,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionStrategy {
    Memento,
    PrefixCompaction,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionStatus {
    Completed,
    Failed,
    Interrupted,
}

#[derive(Clone)]
pub struct CodexCompactionEvent {
    pub thread_id: String,
    pub turn_id: String,
    pub trigger: CompactionTrigger,
    pub reason: CompactionReason,
    pub implementation: CompactionImplementation,
    pub phase: CompactionPhase,
    pub strategy: CompactionStrategy,
    pub status: CompactionStatus,
    pub codex_error_kind: Option<CodexErrKind>,
    pub codex_error_http_status_code: Option<u16>,
    pub active_context_tokens_before: i64,
    pub active_context_tokens_after: i64,
    pub retained_image_count: Option<usize>,
    pub compaction_summary_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub cache_write_input_tokens: Option<i64>,
    pub started_at: u64,
    pub completed_at: u64,
    pub duration_ms: Option<u64>,
}

pub struct HookRunFact {
    pub event_name: HookEventName,
    pub hook_source: HookSource,
    pub handler_type: HookHandlerType,
    pub execution_mode: HookExecutionMode,
    pub status: HookRunStatus,
}
