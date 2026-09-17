/**
 * AHEAD Editor proposal, 2026-09-17. Design vocabulary, not a shipped protocol.
 * Rust/serde must own the eventual wire schema; generate client types from it.
 * Self-contained editor contracts; no dependency on the retired framework.
 *
 * Check this draft:
 * rtk proxy tsc --noEmit --strict --skipLibCheck
 *   --target ES2022 --moduleResolution node docs/development/ahead-editor-contracts.ts
 */
export type Id = string; // Opaque UUID; never used as authorization or ordering.
export type Sha256 = string; // Lowercase, 64 hexadecimal characters.
export type GitOid = string; // Repository object format determines length.
export type Timestamp = string; // RFC 3339 UTC.
export type Revision = number; // Nonnegative safe integer; checked at ingress.
export type RepoPath = string; // UTF-8, relative, "/" separators; see design rules.
export type Base64 = string;

export type WorkKind =
  | "product-change" | "corrective-debugging" | "internal-improvement"
  | "investigation" | "decision" | "operational-stabilization";
export type AssistanceMode = "learn" | "assist";
export type Capability =
  | "read_context" | "present_code" | "record_draft"
  | "propose_edit" | "run_approved_check";
export type SessionRole = "owner" | "editor" | "reviewer" | "viewer";
export type Participant =
  | { id: Id; kind: "human"; subject: string; display_name: string }
  | { id: Id; kind: "ai"; backend_id: Id; on_behalf_of: Id; display_name: string }
  | { id: Id; kind: "system"; service: string; display_name: string };

export interface RepositoryIdentity {
  id: Id;
  // GitHub node ID when available; checkout location is never the identity.
  forge?: { host: string; repository_id: string; owner: string; name: string };
}

export interface ProjectRecord {
  id: Id;
  repository: RepositoryIdentity;
  title: string;
  tracker_connection_id?: Id;
  collaboration_space_id?: Id;
}

export interface LocalCheckout {
  // Machine-local only. Never included in collaboration broadcasts.
  project_id: Id;
  worktree_id: Id;
  absolute_root: string;
  branch_name: string;
  trusted: boolean;
}

export interface SessionPolicySnapshot {
  id: Id;
  sha256: Sha256;
  assistance: "maieutic-strict";
  work_item_required_before_phase: string | null;
  allowed_provider_ids: Id[];
  private_session: boolean;
  external_writes: "human-only";
  predictions: "off" | "explicit-mechanical-scope";
}

export type SessionLifecycle =
  | { kind: "active" }
  | { kind: "paused"; checkpoint_id: Id }
  | { kind: "completed"; checkpoint_id: Id }
  | { kind: "archived"; previous: "active" | "paused" | "completed" };

export interface WorkSession {
  id: Id;
  project_id: Id;
  worktree_id: Id;
  work_kind: WorkKind;
  mode: AssistanceMode;
  title: string;
  owner_id: Id;
  lifecycle: SessionLifecycle;
  policy: SessionPolicySnapshot;
  revision: Revision;
  created_at: Timestamp;
}

export interface SessionView {
  session: WorkSession;
  workflow: WorkflowState;
  participants: Array<{ participant: Participant; role: SessionRole }>;
}

export interface WorkflowState {
  revision: Revision;
  definition_version: string; // Built-in phase rules pinned for this session.
  phase: { id: string; title: string; visit: number };
  primary_work_item: GithubIssueRef | null;
  current_artifact_ids: Id[];
  approvals: Array<{
    artifact_revision_id: Id;
    content_sha256: Sha256;
    policy_sha256: Sha256;
    approved_by: Id; // Authenticated human assigned by the host.
    created_at: Timestamp;
  }>;
  // Derived by the host from the work kind, phase, artifacts and current policy.
  allowed_ai_capabilities: Capability[];
  blockers: string[];
  available_transitions: Array<{ id: string; title: string; to_phase: string }>;
}

export type WorkflowAction =
  | { kind: "link_work_item"; issue: GithubIssueRef }
  | { kind: "record_artifact"; artifact_revision_id: Id }
  | {
      kind: "approve_artifact"; artifact_revision_id: Id;
      expected_content_sha256: Sha256; expected_policy_sha256: Sha256;
    }
  | { kind: "transition"; transition_id: string; reason?: string };

export interface StartWorkInput {
  project_id: Id;
  work_kind: WorkKind;
  title: string;
  human_starting_point: string;
  work_item?: GithubIssueRef;
  urgency: "normal" | "incident";
  mode: AssistanceMode;
}

export interface ArtifactRevision {
  id: Id;
  session_id: Id;
  phase: string;
  phase_visit: number;
  kind: string;
  author: Participant;
  content_sha256: Sha256;
  storage:
    | { kind: "draft"; document_id: Id }
    | { kind: "sealed"; blob_id: Id }; // Immutable content in the session store.
  derived_from: Id[];
}

export interface SessionCheckpoint {
  id: Id;
  session_id: Id;
  workflow_revision: Revision;
  worktree_snapshot_id: Id;
  sealed_artifact_ids: Id[];
  open_thread_ids: Id[];
  tracker_write_ids: Id[];
  next_human_action: string;
  ready_to_implement: boolean; // Derived; cannot create workflow approval.
  created_by: Id;
  created_at: Timestamp;
}

export interface GithubIssueRef {
  provider: "github";
  host: string; // github.com or an explicitly configured Enterprise host.
  repository_id: string;
  owner: string; // Display/routing cache; refresh after rename or transfer.
  repository: string;
  issue_node_id: string;
  number: number;
  url: string;
}

export interface GithubProjectField {
  field_id: string;
  name: string;
  value:
    | { kind: "text"; value: string }
    | { kind: "number"; value: number }
    | { kind: "date"; value: string }
    | { kind: "single_select"; option_id: string; label: string }
    | { kind: "iteration"; iteration_id: string; title: string }
    | { kind: "empty" };
}

export interface TrackerItem {
  ref: GithubIssueRef;
  title: string;
  body_markdown: string;
  state: "open" | "closed";
  labels: string[];
  assignees: string[];
  projects: Array<{
    project_id: string;
    item_id: string;
    fields: GithubProjectField[];
  }>;
  observed_at: Timestamp;
  remote_updated_at: Timestamp;
  etag?: string;
  content_sha256: Sha256;
}

export type TrackerOperation =
  | {
      kind: "create_issue"; project_id: Id;
      title: string; body_markdown: string; labels: string[];
    }
  | {
      kind: "update_issue"; issue: GithubIssueRef;
      title?: string; body_markdown?: string;
      state?: "open" | "closed"; labels?: string[]; assignees?: string[];
    }
  | {
      kind: "set_project_field"; issue: GithubIssueRef;
      project_id: string; item_id: string; field: GithubProjectField;
    };

export interface TrackerWrite {
  id: Id;
  session_id: Id;
  operation: TrackerOperation;
  expected_remote_sha256?: Sha256; // Required for updates.
  preview_sha256: Sha256;
  approved_by?: Id; // Human identity assigned by the host.
  status:
    | "draft" | "approved" | "sending" | "confirmed"
    | "conflict" | "failed" | "unknown";
  resulting_issue?: GithubIssueRef;
  failure?: { code: string; message: string };
}

export interface TextPosition {
  line: number; // Zero-based.
  character: number; // UTF-16 code units; never bytes or display columns.
}
export interface TextRange { start: TextPosition; end: TextPosition } // Half-open.

export interface DocumentVersion {
  document_id: Id;
  epoch: Id; // Changes when history is replaced by an external reset/rebase.
  content_sha256: Sha256;
  state_vector: Base64; // Pinned Yrs encoding; opaque to UI and agents.
}

export interface CodeAnchor {
  id: Id;
  worktree_id: Id;
  document_id: Id; // Stable across host-mediated rename.
  epoch: Id;
  path_at_creation: RepoPath;
  created_version: DocumentVersion;
  start_relative: Base64; // Yrs sticky position, including association.
  end_relative: Base64;
  snapshot_range: TextRange;
  quote_sha256: Sha256;
  context_before: string;
  context_after: string;
  snapshot_id: Id; // Historical content remains accessible if range is deleted.
}

export type AnchorResolution =
  | { kind: "live"; path: RepoPath; range: TextRange; version: DocumentVersion }
  | { kind: "deleted"; snapshot_id: Id; range: TextRange }
  | { kind: "ambiguous"; candidate_ranges: TextRange[] }
  | { kind: "unavailable"; reason: "epoch_changed" | "document_missing" | "history_expired" };

export type ThreadTarget =
  | { kind: "session" }
  | { kind: "code"; anchor_id: Id }
  | { kind: "message"; message_id: Id; range?: TextRange }
  | { kind: "artifact"; artifact_revision_id: Id; range?: TextRange }
  | { kind: "finding"; finding_id: Id };

export interface DiscussionThread {
  id: Id;
  session_id: Id;
  target: ThreadTarget;
  // Canonical issue comes from session -> run -> work_item.
  // Extra related issues are explicit; changing a primary link is audited.
  related_issues: GithubIssueRef[];
  status: "open" | "resolved";
  resolved_by?: Id;
  resolved_at?: Timestamp;
  revision: Revision;
}

export interface ConversationMessage {
  id: Id;
  thread_id: Id;
  author: Participant;
  source: "typed" | "voice_transcript" | "agent" | "system";
  body_markdown: string;
  evidence_anchor_ids: Id[];
  supersedes_message_id?: Id; // Correction/revision, not overwritten history.
  agent_turn_id?: Id;
  created_at: Timestamp;
}

export interface EditorContext {
  session_id: Id;
  context_id: Id;
  active_document?: {
    path: RepoPath;
    version: DocumentVersion;
    visible_range: TextRange;
    selections: TextRange[];
    unsaved: boolean;
  };
  attached_anchor_ids: Id[];
  attached_artifact_ids: Id[];
}

export type PresentationAction =
  | { kind: "reveal_file"; path: RepoPath }
  | { kind: "focus"; anchor_id: Id }
  | { kind: "point"; anchor_id: Id }
  | { kind: "clear" };

export interface PresentationCue {
  id: Id;
  session_id: Id;
  turn_id: Id;
  generation: number; // Presentation generation, independent of the coding task.
  recipient_participant_id: Id; // Navigation is personal unless explicitly followed.
  action: PresentationAction;
  speech_segment_id?: Id; // Also discard when this segment's output generation expires.
}

export interface PresentationResult {
  cue_id: Id;
  generation: number;
  status: "displayed" | "stale" | "suppressed" | "not_found";
  resolved?: Extract<AnchorResolution, { kind: "live" }>;
}

export interface VoiceSession {
  id: Id;
  session_id: Id;
  participant_id: Id;
  transport_epoch: Id; // New on reconnect; reject frames from older connections.
  duplex: "full";
  lifecycle: "connecting" | "active" | "reconnecting" | "closed";
  // These states are independent: input stays live while output is playing.
  input_state: "streaming" | "muted" | "device_unavailable";
  output_state: "idle" | "buffering" | "playing" | "muted";
  input_utterance_id?: Id;
  output_generation: number; // Interruption invalidates output, not capture or tasks.
  route:
    | { kind: "native_realtime"; model_route_id: Id }
    | {
        kind: "streaming_pipeline"; stt_route_id: Id;
        conversation_route_id: Id; tts_route_id: Id;
      };
  input_stream: AudioStreamDescriptor;
  output_stream: AudioStreamDescriptor;
  active_agent_turn_ids: Id[];
  retain_raw_audio: false;
}

export interface AudioStreamDescriptor {
  id: Id;
  codec: "pcm_s16le" | "opus";
  sample_rate_hz: number;
  channels: 1 | 2;
  frame_duration_ms: number;
}

export type VoiceAudioChunk = {
  voice_session_id: Id;
  transport_epoch: Id;
  stream_id: Id;
  sequence: number;
  start_sample: number; // Monotonic position in this stream's negotiated sample rate.
  sample_count: number;
  payload: Base64; // JSON binding; a negotiated binary binding may carry raw bytes.
} & (
  | { direction: "input" }
  | {
      direction: "output";
      response: { id: Id; generation: number; speech_segment_id: Id };
    }
);

export interface VoiceTranscriptUpdate {
  voice_session_id: Id;
  transport_epoch: Id;
  utterance_id: Id;
  revision: number;
  speaker: "human" | "assistant";
  text: string; // Replaces the previous partial text for this utterance.
  final: boolean;
  response_id?: Id;
  output_generation?: number;
}

export interface SpeechSegment {
  id: Id;
  voice_session_id: Id;
  response_id: Id;
  agent_turn_id?: Id; // Voice can respond while an existing coding turn continues.
  output_generation: number;
  revision: number;
  text: string; // Replaces this segment's partial text, never a playback instruction.
  final: boolean;
  after_cue_id?: Id; // Playback waits for successful display or announces failure.
}

export interface VoicePlaybackReceipt {
  voice_session_id: Id;
  transport_epoch: Id;
  stream_id: Id;
  response_id: Id;
  output_generation: number;
  played_through_sample: number; // Output-stream position actually played, not queued.
  status: "started" | "progress" | "completed" | "interrupted";
}

export type VoiceControl =
  | { kind: "interrupt_speech"; response_id: Id; output_generation: number }
  | { kind: "set_input_muted"; muted: boolean }
  | { kind: "set_output_muted"; muted: boolean }
  | {
      kind: "steer_work"; agent_turn_id: Id; message_id: Id;
      context: EditorContext;
    }
  | { kind: "cancel_work"; agent_turn_id: Id };

export interface AgentBackend {
  id: Id;
  title: string;
  transport:
    | { kind: "ahead_codex_stdio"; pinned_revision: string }
    | { kind: "acp_stdio"; local_installation_id: Id };
  certification: "managed" | "external";
  capabilities: {
    text: boolean;
    tools: boolean;
    images: boolean;
    resume: boolean;
    cancel: boolean;
    editor_presentation: boolean;
    mediated_writes: boolean;
  };
}

export type CredentialReference =
  | { kind: "none" }
  | { kind: "keychain"; entry_id: string }
  | { kind: "aws_chain"; profile?: string; region: string };

export interface ModelRoute {
  id: Id;
  title: string;
  backend_id?: Id; // Present when the coding backend owns the route; direct audio/prediction need none.
  purpose: "reasoning" | "edit_prediction" | "realtime_voice" | "speech_to_text" | "text_to_speech";
  provider:
    | { kind: "codex_builtin"; provider_id: "openai" | "ollama" | "lmstudio" | "amazon-bedrock" }
    | {
        kind: "custom"; base_url: string;
        protocol: "responses" | "chat_completions" | "completions" | "anthropic_messages" | "gemini_generate_content";
      }
    | { kind: "realtime"; endpoint: string; protocol_adapter_id: Id }
    | { kind: "local_speech"; installation_id: Id };
  model_id: string;
  credentials: CredentialReference;
  data_location: "local" | "self_hosted" | "external";
  advertised_capabilities: string[];
  verified_capabilities: string[];
  verified_at?: Timestamp;
}

export interface AgentTurnRequest {
  session_id: Id;
  thread_id: Id;
  message_id: Id;
  backend_id: Id;
  model_route_id: Id;
  mode: AssistanceMode;
  expected_policy_sha256: Sha256;
  context: EditorContext;
}

export interface MechanicalScope {
  id: Id;
  session_id: Id;
  instruction: string;
  human_contract_artifact_id: Id;
  allowed_paths: RepoPath[];
  approved_by: Id;
  expires_at: Timestamp;
}

export interface ProposedTextEdit {
  path: RepoPath;
  document_id: Id;
  expected_version: DocumentVersion;
  range: TextRange;
  old_text_sha256: Sha256;
  replacement: string;
}

export type ProposedFileChange =
  | { kind: "edit"; edit: ProposedTextEdit }
  | {
      kind: "create"; path: RepoPath; expected_absent: true;
      content: string; git_mode: "100644" | "100755";
    };

export interface ChangeProposal {
  id: Id;
  session_id: Id;
  turn_id: Id;
  scope_id: Id;
  author_id: Id;
  proposal_sha256: Sha256;
  policy_sha256: Sha256;
  rationale: string;
  changes: ProposedFileChange[]; // Rename/delete require a separate human action.
  state: "proposed" | "accepted" | "rejected" | "stale" | "applied" | "failed";
}

export interface AcceptChangeInput {
  proposal_id: Id;
  expected_proposal_sha256: Sha256;
  expected_policy_sha256: Sha256;
  // Approver inferred from authenticated human connection, not this payload.
}

export interface PredictionRequest {
  request_id: Id;
  session_id: Id;
  scope_id: Id;
  model_route_id: Id;
  expected_policy_sha256: Sha256;
  context: PredictionContextSnapshot;
  deadline_ms: number;
}

export interface PredictionWorkContext {
  mode: AssistanceMode;
  work_kind: WorkKind;
  workflow_version: string;
  workflow_revision: Revision;
  phase: string;
  phase_visit: number;
  primary_work_item: GithubIssueRef | null;
  outcome: string;
  invariants: string[];
  // Compact excerpts retain authorship and the exact source revision.
  work_excerpts: Array<{
    kind: "issue" | "human_intent" | "decision" | "plan_step" | "discussion";
    source_id: Id;
    source_revision: string;
    text: string;
    author_kind: Participant["kind"];
    human_approved: boolean;
  }>;
}

export interface PredictionCodeExcerpt {
  path: RepoPath;
  version: DocumentVersion;
  language_id: string;
  range: TextRange;
  text: string; // Current in-memory content, including authorized unsaved edits.
  reason: "current_file" | "open_file" | "recent_file" | "definition" | "reference" | "test";
}

export interface PredictionContextSnapshot {
  id: Id;
  sha256: Sha256; // Canonical snapshot, excluding id/hash; request also binds policy/route.
  project_id: Id;
  worktree_id: Id;
  work: PredictionWorkContext;
  active_document: {
    path: RepoPath;
    version: DocumentVersion;
    language_id: string;
    cursor: TextPosition;
    selections: TextRange[];
    prefix: string;
    suffix: string;
  };
  code_excerpts: PredictionCodeExcerpt[];
  recent_edits: Array<{
    path: RepoPath;
    before_sha256: Sha256;
    after_version: DocumentVersion;
    diff: string;
    origin: "human" | "accepted_prediction" | "accepted_proposal" | "collaborator";
    applied_at: Timestamp;
  }>;
  diagnostics: Array<{
    document_id: Id; version: DocumentVersion; range: TextRange;
    severity: "error" | "warning" | "information" | "hint";
    message: string;
  }>;
  budget: {
    max_input_tokens: number;
    max_output_tokens: number;
    omitted_excerpt_count: number;
  };
}

export interface EditPrediction {
  request_id: Id;
  context_id: Id;
  context_sha256: Sha256;
  scope_id: Id;
  expires_at: Timestamp;
  edit: ProposedTextEdit; // Single current-file replacement; ghost text.
}

export interface FileSnapshot {
  document_id: Id;
  path: RepoPath;
  git_mode: "100644" | "100755";
  version: DocumentVersion;
  blob_sha256: Sha256;
}

export interface WorktreeSnapshot {
  id: Id;
  worktree_id: Id;
  parent_git_oid: GitOid | null;
  merge_base_oid: GitOid | null;
  files: FileSnapshot[];
  non_text_files: Array<{
    path: RepoPath; git_mode: string; content_sha256: Sha256;
    review_support: "manual" | "unsupported";
  }>;
  deleted_paths: RepoPath[];
  manifest_sha256: Sha256;
  last_durable_sequence: Revision;
  created_at: Timestamp;
}

export interface ReviewFinding {
  id: Id;
  session_id: Id;
  snapshot_id: Id;
  thread_id: Id;
  author_id: Id;
  severity: "critical" | "major" | "minor" | "suggestion";
  title: string;
  body_markdown: string;
  anchor_id?: Id;
}

export interface FindingDisposition {
  finding_id: Id;
  reviewed_snapshot_id: Id;
  human_id: Id;
  disposition: "fixed" | "accepted_risk" | "not_applicable" | "follow_up";
  rationale: string;
  follow_up_issue?: GithubIssueRef;
}

export interface ReviewAttestation {
  id: Id;
  session_id: Id;
  snapshot_id: Id;
  workflow_revision: Revision;
  policy_sha256: Sha256;
  reviewer_id: Id;
  implementer_ids: Id[];
  outcome: "approved" | "changes_requested";
  created_at: Timestamp;
}

export interface Presence {
  session_id: Id;
  participant_id: Id;
  document_id?: Id;
  selection?: { start_relative: Base64; end_relative: Base64 };
  following_participant_id?: Id;
  expires_at: Timestamp;
}

export interface DocumentUpdate {
  document_id: Id;
  epoch: Id;
  update: Base64;
}

export interface CollaborationTransaction {
  transaction_id: Id;
  session_id: Id;
  policy_sha256: Sha256;
  updates: DocumentUpdate[];
  cause:
    | { kind: "human_edit" }
    | { kind: "accepted_proposal"; proposal_id: Id }
    | { kind: "external_file_change"; observation_id: Id };
  // Server assigns actor and durable sequence after validating every document.
}

export type SessionEvent =
  | { kind: "session_started"; session: WorkSession; workflow: WorkflowState }
  | { kind: "workflow_updated"; action: WorkflowAction; state: WorkflowState }
  | { kind: "message_recorded"; message: ConversationMessage }
  | { kind: "thread_updated"; thread: DiscussionThread }
  | { kind: "documents_updated"; transaction: CollaborationTransaction }
  | { kind: "checkpoint_created"; checkpoint: SessionCheckpoint }
  | { kind: "proposal_updated"; proposal: ChangeProposal }
  | { kind: "tracker_write_updated"; write: TrackerWrite }
  | { kind: "review_recorded"; attestation: ReviewAttestation };

export interface DurableEvent {
  api_version: "ahead.editor/v0-draft";
  event_id: Id;
  session_id: Id;
  sequence: Revision;
  actor: Participant;
  server_time: Timestamp;
  causation_id?: Id;
  payload: SessionEvent;
}

export interface CommandEnvelope<T> {
  api_version: "ahead.editor/v0-draft";
  request_id: Id;
  session_id: Id;
  expected_revision?: Revision;
  payload: T;
}

export interface WorkflowActionInput {
  session_id: Id;
  expected_workflow_revision: Revision;
  action: WorkflowAction;
}

export interface SubscribeInput {
  session_id: Id;
  after_sequence: Revision;
}

export interface ToolAuthorization {
  tool_name: string;
  mapped_capability: Capability;
  allowed: boolean;
  reason: string;
  policy_sha256: Sha256;
  workflow_revision: Revision;
}

export interface ProtocolError {
  code:
    | "invalid_request" | "unauthorized" | "forbidden"
    | "policy_changed" | "version_conflict" | "anchor_unavailable"
    | "capability_unavailable" | "provider_unavailable" | "rate_limited"
    | "tracker_conflict" | "external_outcome_unknown" | "resync_required";
  message: string;
  retryable: boolean;
  current_revision?: Revision;
  retry_after_ms?: number;
}
