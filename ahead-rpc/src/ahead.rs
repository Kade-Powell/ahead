//! AHEAD Native Editor Protocol & DTOs
//!
//! Grounded in `docs/development/ahead-editor-contracts.ts` and `docs/development/ahead-editor-mvp.md`.
//! Serves as the typed RPC and storage bridge between the Floem UI, Proxy Session Host,
//! and governing agent runtimes.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub type Id = String;
pub type Sha256 = String;
pub type GitOid = String;
pub type Timestamp = String;
pub type Revision = u64;
pub type RepoPath = String;

/// The six descriptive work categories inferred from the human request.
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

    /// Suggests descriptive metadata from the human's starting request.
    /// The host may still accept an explicit classification when one exists.
    pub fn infer_from_request(request: &str) -> Self {
        let request = request.to_ascii_lowercase();
        let has = |terms: &[&str]| terms.iter().any(|term| request.contains(term));

        if has(&[
            "outage",
            "incident",
            "production down",
            "restore service",
            "rollback",
            "recovery",
        ]) {
            Self::OperationalStabilization
        } else if has(&[
            "bug",
            "broken",
            "crash",
            "error",
            "failure",
            "regression",
            "fix ",
        ]) {
            Self::CorrectiveDebugging
        } else if has(&[
            "decide",
            "decision",
            "tradeoff",
            "choose",
            "should we",
            "which approach",
        ]) {
            Self::Decision
        } else if has(&[
            "investigate",
            "explore",
            "research",
            "understand",
            "trace",
            "audit",
            "why ",
        ]) {
            Self::Investigation
        } else if has(&[
            "improve",
            "refactor",
            "cleanup",
            "clean up",
            "optimize",
            "performance",
            "maintain",
        ]) {
            Self::InternalImprovement
        } else {
            Self::ProductChange
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

impl AssistanceMode {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Learn => "Read-only guidance",
            Self::Assist => "Bounded workspace assistance",
        }
    }
}

/// Task-local intent. Sessions are durable containers, not Learn/Assist modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskIntent {
    Teaching,
    Assistance,
}

impl TaskIntent {
    pub fn assistance_mode(self) -> AssistanceMode {
        match self {
            Self::Teaching => AssistanceMode::Learn,
            Self::Assistance => AssistanceMode::Assist,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Teaching => "Teaching",
            Self::Assistance => "Assistance",
        }
    }

    /// Explicit teaching language creates a teaching task. Requests that
    /// combine teaching with active implementation stay assistance tasks.
    pub fn infer_from_request(request: &str) -> Self {
        let request = request.to_ascii_lowercase();
        let explicit_teaching = [
            "teach me",
            "help me learn",
            "i want to learn",
            "learn about",
            "help me understand",
            "walk me through",
            "explain why",
            "explain how",
        ]
        .iter()
        .any(|phrase| request.contains(phrase));
        let active_work = [
            "while we",
            "as we",
            "while debugging",
            "while fixing",
            "while implementing",
            "debug this",
            "fix this",
            "implement this",
            "add this",
            "change this",
        ]
        .iter()
        .any(|phrase| request.contains(phrase));

        if explicit_teaching && !active_work {
            Self::Teaching
        } else {
            Self::Assistance
        }
    }
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
            Self::Human { id, .. }
            | Self::Ai { id, .. }
            | Self::System { id, .. } => id,
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
            sha256:
                "0000000000000000000000000000000000000000000000000000000000000000"
                    .to_string(),
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
    pub title: String,
    pub owner_id: Id,
    pub lifecycle: SessionLifecycle,
    pub policy: SessionPolicySnapshot,
    pub revision: Revision,
    pub created_at: Timestamp,
}

/// An ACP agent in AHEAD's curated install catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalAcpAdapter {
    pub id: String,
    pub display_name: String,
    pub installed: bool,
}

/// A tracked MCP process declaration awaiting workspace-local approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerDeclaration {
    pub id: String,
    pub declared: bool,
    pub declaration_toml: String,
    pub fingerprint: String,
    pub enabled: bool,
    pub approved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTask {
    pub id: Id,
    pub session_id: Id,
    pub intent: TaskIntent,
    pub work_kind: WorkKind,
    pub title: String,
    #[serde(default)]
    pub objective: String,
    pub parent_task_id: Option<Id>,
    pub learning_arc_id: Option<Id>,
    pub created_at: Timestamp,
    pub completed_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningRecord {
    pub id: Id,
    pub arc_id: Id,
    pub kind: String,
    pub content: String,
    pub source_refs: Vec<RepoPath>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningArc {
    pub id: Id,
    pub task_id: Id,
    pub mission: String,
    pub current_concept_id: Option<Id>,
    pub state: String,
    pub records: Vec<LearningRecord>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
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
    pub task: SessionTask,
    pub learning_arc: Option<LearningArc>,
    pub workflow: WorkflowState,
    pub participants: Vec<SessionParticipantRecord>,
}

/// Sidebar metadata; full session detail is loaded with `GetSession`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionListItem {
    pub id: Id,
    pub title: String,
    pub lifecycle: SessionLifecycle,
    pub created_at: Timestamp,
    /// Latest durable chat message, or creation time before the first turn.
    pub updated_at: Timestamp,
    pub backend: Option<String>,
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
    /// Who authored the anchored region: a participant id or `ahead`.
    pub actor_id: Id,
    pub path: RepoPath,
    pub range: DisplayRange,
    pub quote_hash: Sha256,
    pub surrounding_context: Option<String>,
}

/// Canonical actor id for edits authored by the AHEAD agent rather than a human.
pub const AHEAD_ACTOR_ID: &str = "ahead";

/// Presentation cue: highlights code without moving the human's caret
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentationCue {
    pub cue_id: Id,
    pub anchor: CodeAnchor,
    pub label: String,
    pub pointer_target: Option<DisplayPosition>,
    pub author_id: Id,
    pub display_duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// An agent request to present verified editor content, move its pointer, or
/// control speech. The editor resolves each code quote against the live buffer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AgentPresentationAction {
    Present {
        cue_id: Id,
        path: RepoPath,
        quote: String,
        label: String,
        note: String,
    },
    MovePointer {
        cue_id: Id,
        quote: String,
    },
    Clear {
        cue_id: Option<Id>,
    },
    Speak {
        cue_id: Option<Id>,
        text: String,
    },
    StopSpeaking,
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

/// Editor context attached to a governed turn: active document version,
/// visible range, selection, and explicit attachments. Mirrors
/// `EditorContext` in `ahead-editor-contracts.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnEditorContext {
    pub active_path: RepoPath,
    pub caret: DisplayPosition,
    pub selection: Option<DisplayRange>,
    pub file_content: String,
    pub visible_end: Option<DisplayPosition>,
    pub attached_anchor_ids: Vec<Id>,
    pub attached_files: Vec<TurnContextFile>,
    /// Human-selected excerpts from AHEAD's project or user memory.
    #[serde(default)]
    pub attached_memories: Vec<MemoryExcerpt>,
}

/// A file explicitly attached from the user's computer through the chat
/// composer. The content is captured at attach time so a turn remains
/// deterministic even if the file changes while the harness is running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnContextFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    Project,
    User,
}

impl MemoryScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::User => "user",
        }
    }
}

/// A bounded, explicitly selected excerpt from a human-readable memory file.
/// `source` is a portable label, never a host-specific absolute path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryExcerpt {
    pub scope: MemoryScope,
    pub source: String,
    pub line: usize,
    pub excerpt: String,
}

pub const MEMORY_DOCUMENT_MAX_BYTES: usize = 32 * 1024;

/// Snapshot of one human-readable memory file for a reviewed replacement.
/// `source` is a portable label, and `sha256` is the optimistic concurrency token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryDocument {
    pub scope: MemoryScope,
    pub source: String,
    pub content: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryWriteResult {
    pub indexed: bool,
    pub warning: Option<String>,
}

#[cfg(test)]
mod turn_editor_context_tests {
    use super::*;

    #[test]
    fn older_turn_contexts_default_to_no_memory_attachments() {
        let older_context = serde_json::json!({
            "active_path": "src/lib.rs",
            "caret": { "line": 0, "col": 0 },
            "selection": null,
            "file_content": "",
            "visible_end": null,
            "attached_anchor_ids": [],
            "attached_files": []
        });

        let context: TurnEditorContext = serde_json::from_value(older_context)
            .expect("older turn context should remain readable");
        assert!(context.attached_memories.is_empty());
    }
}

/// A current, unsaved editor buffer returned to the managed agent on demand.
/// `path` is workspace-relative; this is not a durable turn attachment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentBufferSnapshot {
    pub path: RepoPath,
    pub content: String,
}

pub const MAX_AGENT_BUFFER_SNAPSHOTS: usize = 128;
pub const MAX_AGENT_BUFFER_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;

pub fn validate_agent_buffer_snapshots(
    buffers: &[AgentBufferSnapshot],
) -> Result<(), String> {
    if buffers.len() > MAX_AGENT_BUFFER_SNAPSHOTS {
        return Err(format!(
            "AHEAD editor has more than {MAX_AGENT_BUFFER_SNAPSHOTS} searchable open buffers"
        ));
    }
    let total_bytes = buffers.iter().fold(0usize, |total, buffer| {
        total
            .saturating_add(buffer.path.len())
            .saturating_add(buffer.content.len())
    });
    if total_bytes > MAX_AGENT_BUFFER_SNAPSHOT_BYTES {
        return Err(format!(
            "AHEAD editor buffers exceed the {} MiB search snapshot limit",
            MAX_AGENT_BUFFER_SNAPSHOT_BYTES / (1024 * 1024)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod buffer_snapshot_tests {
    use super::*;

    #[test]
    fn rejects_oversized_batches_and_encodes_failures_separately() {
        let snapshot = AgentBufferSnapshot {
            path: "src/lib.rs".into(),
            content: "unsaved".into(),
        };
        assert!(validate_agent_buffer_snapshots(&[snapshot.clone()]).is_ok());
        assert!(
            validate_agent_buffer_snapshots(&vec![
                snapshot;
                MAX_AGENT_BUFFER_SNAPSHOTS + 1
            ])
            .is_err()
        );
        assert!(
            validate_agent_buffer_snapshots(&[AgentBufferSnapshot {
                path: "large.rs".into(),
                content: "x".repeat(MAX_AGENT_BUFFER_SNAPSHOT_BYTES),
            }])
            .is_err()
        );

        let legacy = serde_json::json!({
            "method": "agent_buffer_snapshots_response",
            "params": {
                "session_id": "session",
                "turn_id": "turn",
                "request_id": "request",
                "buffers": []
            }
        });
        let AheadRequest::AgentBufferSnapshotsResponse { buffers, .. } =
            serde_json::from_value(legacy).expect("deserialize older reply")
        else {
            panic!("expected buffer snapshot reply");
        };
        assert!(buffers.is_empty());
        let failed =
            serde_json::to_value(AheadRequest::AgentBufferSnapshotsFailed {
                session_id: "session".into(),
                turn_id: "turn".into(),
                request_id: "request".into(),
                message: "snapshot limit exceeded".into(),
            })
            .expect("serialize failure reply");
        assert_eq!(failed["method"], "agent_buffer_snapshots_failed");
    }
}

/// Which harness owns a durable AHEAD conversation.
///
/// AHEAD threads always use the managed, in-process AHEAD harness. External
/// ACP threads are compatibility conversations whose process and model are
/// selected by the external ACP agent configuration.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum HarnessKind {
    #[default]
    Ahead,
    ExternalAcp,
}

/// Explicit mechanical scope: paths plus the human contract artifact the
/// turn may work under. Mirrors `MechanicalScope` in contracts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnMechanicalScope {
    pub scope_id: Id,
    pub instruction: String,
    pub human_contract_artifact_id: Id,
    pub allowed_paths: Vec<RepoPath>,
    pub approved_by: Id,
}

/// Governed turn request for the built-in loop. `turn/start`-compatible
/// subset (`thread_id`, `cwd`, approval/sandbox overrides) plus AHEAD
/// gates (`expected_policy_sha256`, per-turn read-only mode, scope and editor
/// context).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTurnRequestDto {
    pub session_id: Id,
    pub thread_id: Id,
    /// Runtime tier for this durable conversation. AHEAD threads use the
    /// managed internal runtime; external ACP threads use the configured
    /// compatibility agent.
    #[serde(default)]
    pub harness: HarnessKind,
    /// Selected external ACP adapter. Managed AHEAD turns leave this empty.
    pub external_agent_id: Option<String>,
    /// Provider model selected in AHEAD settings for this turn.
    pub model: Option<String>,
    /// Provider id selected in AHEAD settings. The managed AHEAD runtime
    /// receives this as its native `modelProvider`; external ACP adapters may
    /// ignore it.
    pub model_provider: Option<String>,
    pub user_message: String,
    /// Host-assembled task and durable work state; separate from the human's
    /// message so retries preserve the exact prompt context.
    #[serde(default)]
    pub session_context: String,
    pub context: TurnEditorContext,
    pub invariants: Vec<String>,
    pub cwd: Option<String>,
    pub expected_policy_sha256: String,
    /// Applies a filesystem read-only sandbox for this managed turn.
    #[serde(default)]
    pub read_only: bool,
    pub scope: Option<TurnMechanicalScope>,
}

/// Work-item status in the side-panel checklist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkItemStatus {
    Open,
    InProgress,
    Done,
    Dropped,
}

/// A checklist item owned by a session. Accumulates close-out docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkItem {
    pub id: Id,
    pub session_id: Id,
    pub title: String,
    pub status: WorkItemStatus,
    pub position: i64,
    pub created_by: Id,
    pub created_at: Timestamp,
    pub closed_at: Option<Timestamp>,
}

/// Append-only event on a work item (note, decision, artifact link, close).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkItemEvent {
    pub id: Id,
    pub item_id: Id,
    pub session_id: Id,
    pub kind: String,
    pub body_markdown: String,
    pub actor_id: Id,
    pub anchor_id: Option<Id>,
    pub artifact_id: Option<Id>,
    pub created_at: Timestamp,
}

/// Close-out doc required (or explicitly skipped with reason) at close.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkItemCloseout {
    pub item_id: Id,
    pub session_id: Id,
    pub summary_markdown: String,
    pub issue_ref: Option<String>,
    pub follow_ups_json: String,
    pub conversation_summary_id: Option<Id>,
    pub closed_at: Timestamp,
}

/// Tracker publish payload (issue update preview).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerPublishPayload {
    pub title: Option<String>,
    pub body_markdown: Option<String>,
    pub state: Option<String>,
}

/// Frozen review snapshot: code tree hash, implementers, findings and a
/// reviewer attestation. Whether the attestation is independent or self-review
/// is recorded for the repository/team policy to interpret; it is not merge
/// authorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSnapshotDto {
    pub snapshot_id: Id,
    pub session_id: Id,
    pub code_tree_sha: Sha256,
    pub implementer_ids: Vec<Id>,
    pub findings: Vec<String>,
    pub is_approved: bool,
    pub approved_by: Option<Id>,
    pub approved_at: Option<Timestamp>,
    #[serde(default)]
    pub reviewer_is_implementer: Option<bool>,
}
/// Durable conversation message in a work session's readable history.
/// `status` is one of `streaming`, `complete`, `cancelled` or `failed`; a
/// streamed turn updates the same row in place as deltas arrive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub id: Id,
    pub session_id: Id,
    pub turn_id: Id,
    pub sequence: i64,
    /// `human` or `agent`.
    pub role: String,
    pub actor_id: Id,
    pub content: String,
    pub status: String,
    pub created_at: Timestamp,
}

/// Stable keyset cursor into one session's visible conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationMessageCursor {
    pub sequence: i64,
    pub message_id: Id,
}

/// One bounded, chronological window of durable conversation history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationMessagePage {
    pub messages: Vec<ConversationMessage>,
    pub has_older: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPlanEntry {
    pub content: String,
    pub status: String,
    pub priority: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentToolCall {
    /// Stable runtime identifier used to update one tool card from pending to
    /// its terminal state. Mirrors Zed's `ToolCallId`-keyed upsert model.
    #[serde(default)]
    pub id: String,
    pub title: String,
    pub status: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCommand {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub input: Option<String>,
}

/// Non-sensitive metadata for one skill available to the managed agent.
/// Skill instructions are loaded by the owning runtime after explicit selection.
/// The source distinguishes same-named skills without exposing their paths.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSkillSource {
    #[default]
    User,
    Project,
    System,
    Admin,
}

impl AgentSkillSource {
    /// Stable scope qualifier inserted in slash commands when names collide.
    pub fn slash_prefix(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::System => "system",
            Self::Admin => "admin",
        }
    }

    /// Human-readable origin for the skill picker.
    pub fn display_label(self) -> &'static str {
        match self {
            Self::User => "User",
            Self::Project => "Project",
            Self::System => "AHEAD system",
            Self::Admin => "Admin",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSkill {
    pub name: String,
    pub description: String,
    /// Defaults to the user scope for older RPC payloads.
    #[serde(default)]
    pub source: AgentSkillSource,
}

/// Path-free skill metadata and validation status for the managed-agent picker.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSkillCatalog {
    #[serde(default)]
    pub skills: Vec<AgentSkill>,
    /// Number of discovered non-system skill packages rejected during loading.
    #[serde(default)]
    pub skipped_count: usize,
}

#[cfg(test)]
mod agent_skill_tests {
    use super::{AgentSkill, AgentSkillSource};

    #[test]
    fn source_round_trips_and_old_payloads_default_to_user() {
        let legacy: AgentSkill = serde_json::from_value(serde_json::json!({
            "name": "review",
            "description": "Review the current file"
        }))
        .expect("deserialize legacy skill metadata");
        assert_eq!(legacy.source, AgentSkillSource::User);

        let skill = AgentSkill {
            name: "review".into(),
            description: "Review the current file".into(),
            source: AgentSkillSource::Project,
        };
        let encoded = serde_json::to_value(skill).expect("serialize skill metadata");
        assert_eq!(encoded["source"], "project");
    }

    #[test]
    fn slash_prefixes_disambiguate_every_skill_source() {
        assert_eq!(AgentSkillSource::User.slash_prefix(), "user");
        assert_eq!(AgentSkillSource::Project.slash_prefix(), "project");
        assert_eq!(AgentSkillSource::System.slash_prefix(), "system");
        assert_eq!(AgentSkillSource::Admin.slash_prefix(), "admin");
    }
}

/// ACP configuration options advertised for one external-agent session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentConfigOption {
    pub id: String,
    pub name: String,
    pub category: Option<String>,
    pub current_value: AgentConfigOptionValue,
    pub choices: Vec<AgentConfigChoice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum AgentConfigOptionValue {
    Select(String),
    Boolean(bool),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentConfigChoice {
    pub value: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentUsage {
    pub total_tokens: u64,
    pub context_window: Option<u64>,
}

/// Durable, non-sensitive UI projection of the latest streamed agent turn.
/// Ephemeral reasoning and pending user-input payloads are intentionally not
/// retained here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AgentRuntimeState {
    #[serde(default)]
    pub turn_id: Option<Id>,
    #[serde(default)]
    pub plan: Vec<AgentPlanEntry>,
    #[serde(default)]
    pub tool_calls: Vec<AgentToolCall>,
    #[serde(default)]
    pub usage: Option<AgentUsage>,
    #[serde(default)]
    pub commands: Vec<AgentCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentUserInputOption {
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentUserInputQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    pub options: Vec<AgentUserInputOption>,
    pub allows_other: bool,
    pub is_secret: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentUserInputRequest {
    pub request_id: String,
    /// Blocking requests pause the model turn until the client answers.
    #[serde(default)]
    pub is_blocking: bool,
    pub questions: Vec<AgentUserInputQuestion>,
}

/// Retained phase conversation summary (messages are never deleted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationSummary {
    pub id: Id,
    pub session_id: Id,
    pub phase: String,
    pub summary_markdown: String,
    pub message_id_range: String,
    pub created_at: Timestamp,
}

/// Full session export bundle: everything needed to reconstruct the
/// visible session (conversation, evidence, code versions, issue links)
/// on a clean machine without the original database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionExportBundle {
    pub format_version: String,
    pub exported_at: Timestamp,
    pub session: WorkSession,
    pub task: SessionTask,
    pub learning_arc: Option<LearningArc>,
    pub workflow: WorkflowState,
    pub participants: Vec<SessionParticipantRecord>,
    /// Readable conversation history, separate from model replay history.
    #[serde(default)]
    pub conversation_messages: Vec<ConversationMessage>,
    /// Latest non-sensitive plan, tool cards, usage and command metadata.
    /// Runtime bindings, credentials and pending input are not portable.
    pub agent_runtime_state: AgentRuntimeState,
    pub anchors: Vec<CodeAnchor>,
    pub work_items: Vec<WorkItem>,
    pub work_item_events: Vec<WorkItemEvent>,
    pub work_item_closeouts: Vec<WorkItemCloseout>,
    pub conversation_summaries: Vec<ConversationSummary>,
    pub anchors_note: String,
}

#[cfg(test)]
mod task_tests {
    use super::{TaskIntent, WorkKind};

    #[test]
    fn request_inference_keeps_teaching_separate_from_work_kind() {
        assert_eq!(
            TaskIntent::infer_from_request(
                "Teach me why this retry path behaves this way"
            ),
            TaskIntent::Teaching
        );
        assert_eq!(
            TaskIntent::infer_from_request(
                "Teach me while we debug this retry path"
            ),
            TaskIntent::Assistance
        );
        assert_eq!(
            WorkKind::infer_from_request("The parser crashes on empty input"),
            WorkKind::CorrectiveDebugging
        );
        assert_eq!(
            WorkKind::infer_from_request("Add retries to the request client"),
            WorkKind::ProductChange
        );
        assert_eq!(
            WorkKind::infer_from_request("Should we use a queue or a stream?"),
            WorkKind::Decision
        );
    }
}

/// RPC requests from UI (ahead-app) to Session Host (ahead-proxy)
#[expect(
    clippy::large_enum_variant,
    reason = "AgentTurnStart/SessionRestore carry DTOs by value for zero-copy serde; boxing adds indirection per turn"
)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum AheadRequest {
    StartWork {
        work_kind: Option<WorkKind>,
        title: String,
        starting_point: String,
        work_item: Option<GithubIssueRef>,
        #[serde(default)]
        harness: Option<HarnessKind>,
        #[serde(default)]
        external_agent_id: Option<String>,
    },
    /// Lists AHEAD's supported external ACP agents and their install state.
    ListExternalAcpAdapters,
    SetExternalAcpAdapterInstalled {
        adapter_id: String,
        installed: bool,
    },
    /// Lists tracked MCP declarations and their ignored local approval state.
    ListMcpServerDeclarations {
        workspace: PathBuf,
    },
    /// Enables only a current reviewed declaration, or revokes a local approval.
    SetMcpServerApproval {
        workspace: PathBuf,
        server_id: String,
        expected_fingerprint: String,
        enabled: bool,
    },
    GetSession {
        session_id: Id,
    },
    ListSessions,
    /// Returns the current workspace's recently opened indexed files.
    ListRecentWorkspaceFiles,
    /// Records a workspace-relative file as recently opened.
    RecordRecentWorkspaceFile {
        path: RepoPath,
        opened_at: i64,
    },
    ListEditorRecoveries,
    ReadEditorRecovery {
        buffer_id: String,
    },
    WriteEditorRecovery {
        snapshot: crate::file::EditorRecoverySnapshot,
    },
    /// Searches the current workspace and AHEAD user memory sources.
    SearchMemory {
        query: String,
        limit: usize,
    },
    /// Reads one bounded current AHEAD memory file without exposing its host path.
    ReadMemory {
        scope: MemoryScope,
    },
    /// Appends user-selected content to the exact project or user memory file.
    WriteMemory {
        scope: MemoryScope,
        message_id: Id,
        content: String,
    },
    /// Replaces a reviewed memory document only if its source snapshot is unchanged.
    ReplaceMemory {
        scope: MemoryScope,
        expected_sha256: String,
        content: String,
    },
    /// Hides a durable session from the editor thread rail without deleting
    /// its transcript, work items, anchors, or checkpoints.
    ArchiveSession {
        session_id: Id,
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
    /// Uncommitted attribution anchors on the given paths, across sessions.
    ListAnchorsForPaths {
        paths: Vec<RepoPath>,
    },
    /// Begins a real streamed harness turn. Deltas arrive as
    /// `AheadNotification::AgentMessageDelta`; completion/cancellation arrives
    /// as `AheadNotification::AgentTurnState`.
    AgentTurnStart {
        request: AgentTurnRequestDto,
    },
    /// Cancel the in-flight harness turn for a work session.
    AgentTurnCancel {
        session_id: Id,
    },
    /// Connect an external agent and load its advertised config before a turn.
    AgentSessionPrepare {
        session_id: Id,
    },
    /// Change an advertised option on a live external ACP session.
    AgentConfigOptionSet {
        session_id: Id,
        config_id: String,
        value: AgentConfigOptionValue,
    },
    /// Retries a durable failed/cancelled turn after an explicit user action.
    AgentTurnRetry {
        session_id: Id,
        turn_id: Id,
    },
    /// Answers a blocking question raised by the built-in agent loop.
    AgentUserInputAnswer {
        session_id: Id,
        request_id: String,
        answers: HashMap<String, Vec<String>>,
    },
    /// Answers one managed-runtime request for live editor buffers.
    AgentBufferSnapshotsResponse {
        session_id: Id,
        turn_id: Id,
        request_id: String,
        buffers: Vec<AgentBufferSnapshot>,
    },
    /// A separate method so older proxies reject failures instead of treating
    /// an empty buffer list as a successful snapshot.
    AgentBufferSnapshotsFailed {
        session_id: Id,
        turn_id: Id,
        request_id: String,
        message: String,
    },
    /// Acknowledges whether the editor displayed or cleared a managed-agent
    /// presentation after verifying its live source range.
    AgentPresentationResponse {
        session_id: Id,
        turn_id: Id,
        request_id: String,
        applied: bool,
        message: String,
    },
    /// Diagnostic state for the external/managed harness binding.
    HarnessStatus {
        session_id: Id,
    },
    /// Loads a bounded page before the exclusive `(sequence, message_id)` key.
    /// Returned messages are in chronological order.
    ConversationMessagesPage {
        session_id: Id,
        before: Option<ConversationMessageCursor>,
        limit: usize,
    },
    /// Latest durable presentation state for streamed agent cards.
    AgentRuntimeState {
        session_id: Id,
    },
    /// Discovers managed-runtime skills without persisting their metadata.
    AgentSkills {
        session_id: Id,
        model: Option<String>,
        model_provider: Option<String>,
    },
    WorkItemCreate {
        session_id: Id,
        title: String,
    },
    WorkItemList {
        session_id: Id,
    },
    WorkItemSetStatus {
        item_id: Id,
        status: WorkItemStatus,
    },
    WorkItemNote {
        item_id: Id,
        kind: String,
        body_markdown: String,
    },
    WorkItemClose {
        item_id: Id,
        summary_markdown: String,
        issue_ref: Option<String>,
        follow_ups_json: Option<String>,
        skip_reason: Option<String>,
    },
    ConversationSummarize {
        session_id: Id,
        phase: String,
        summary_markdown: String,
        message_id_range: String,
    },
    ConversationList {
        session_id: Id,
    },
    SessionExport {
        session_id: Id,
    },
    SessionRestore {
        bundle: SessionExportBundle,
    },
    ReviewCapture {
        session_id: Id,
        code_tree_sha: Sha256,
        implementer_ids: Vec<Id>,
        findings: Vec<String>,
    },
    ReviewAttest {
        snapshot_id: Id,
        reviewer_id: Id,
    },
    ReviewGet {
        session_id: Id,
    },
    TrackerStage {
        session_id: Id,
        issue: GithubIssueRef,
        payload: TrackerPublishPayload,
        observed_body_sha: String,
    },
    TrackerAuthorize {
        outbox_id: Id,
    },
    TrackerPublish {
        outbox_id: Id,
    },
    TrackerGet {
        outbox_id: Id,
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
#[expect(
    clippy::large_enum_variant,
    reason = "SessionUpdated carries SessionView by value; notifications are consumed once, boxing adds indirection"
)]
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
    /// A streamed chunk of the agent's reply for an in-flight turn.
    AgentMessageDelta {
        session_id: Id,
        turn_id: Id,
        delta: String,
    },
    /// Ephemeral reasoning/thought text for the active turn. It is shown in
    /// the chat but intentionally not added to the durable transcript.
    AgentThoughtDelta {
        session_id: Id,
        turn_id: Id,
        delta: String,
    },
    /// The native runtime compacted conversation context for this turn.
    AgentContextCompacted {
        session_id: Id,
        turn_id: Id,
    },
    /// Turn lifecycle transition (`streaming`, `complete`, `cancelled`,
    /// `failed`) with the durable message it belongs to.
    AgentTurnState {
        session_id: Id,
        turn_id: Id,
        message_id: Id,
        state: String,
    },
    /// A durable conversation message was appended or updated.
    ConversationMessageAdded {
        message: ConversationMessage,
    },
    AgentPlan {
        session_id: Id,
        turn_id: Id,
        entries: Vec<AgentPlanEntry>,
    },
    AgentToolCall {
        session_id: Id,
        turn_id: Id,
        call: AgentToolCall,
    },
    AgentUsage {
        session_id: Id,
        turn_id: Id,
        usage: AgentUsage,
    },
    AgentUserInputRequested {
        session_id: Id,
        turn_id: Id,
        request: AgentUserInputRequest,
    },
    AgentBufferSnapshotsRequested {
        session_id: Id,
        turn_id: Id,
        request_id: String,
    },
    AgentPresentationRequested {
        session_id: Id,
        turn_id: Id,
        request_id: String,
        action: AgentPresentationAction,
    },
    AgentModeChanged {
        session_id: Id,
        mode_id: String,
    },
    /// Slash commands advertised by the selected external ACP agent.
    AgentCommandsAvailable {
        session_id: Id,
        commands: Vec<AgentCommand>,
    },
    /// Complete ACP config state for one external-agent session.
    AgentConfigOptionsAvailable {
        session_id: Id,
        options: Vec<AgentConfigOption>,
    },
}
