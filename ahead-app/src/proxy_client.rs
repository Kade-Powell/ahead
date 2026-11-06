//! Proxy-backed client for the AHEAD GPUI shell.
//!
//! Re-enters `ahead --proxy` as a child with piped stdio, sends
//! `ProxyNotification::Initialize`, and pumps `CoreNotification` messages
//! back into GPUI-visible state: completions, hover, definitions, inline
//! predictions, diagnostics, diff branch, and DAP lifecycle.

use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, BufWriter},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use ahead_rpc::{
    RequestId, RpcError, RpcMessage,
    ahead::{
        AgentAuthMethod, AgentBufferSnapshot, AgentBufferSymbol, AgentCommand,
        AgentConfigOption, AgentConfigOptionValue, AgentPatchPreview,
        AgentPlanEntry, AgentPresentationAction, AgentRuntimeState,
        AgentTerminalAuthLaunch, AgentToolCall, AgentTurnRequestDto, AgentUsage,
        AgentUserInputRequest, AheadNotification, AheadRequest, CodeAnchor,
        CodeComment, ConversationMessage, ConversationMessageCursor,
        ConversationMessagePage, ExternalAcpAdapter, HarnessKind,
        McpServerApprovalAction, McpServerDeclaration, MemoryDocument,
        MemoryExcerpt, MemoryScope, MemoryWriteResult, SessionExportBundle,
        SessionListItem, SessionParticipantRecord, SessionView,
        SharedBufferEditResult, SharedBufferSnapshot, SharedSessionOffer,
        SharedSessionUpdate, VoiceTranscriptSpeaker, WorkItem, WorkItemStatus,
    },
    core::{CoreNotification, CoreRequest, CoreResponse, LanguageExtensionIssue},
    dap_types::{
        DapId, DapSessionState, DebugTerminalRequest, DebugTerminalResponse,
        RunDebugConfig, SourceBreakpoint, StackFrame, Stopped, ThreadId,
    },
    delta::{AheadDelta, DeltaOp},
    plugin::PluginId,
    proxy::{
        ProxyMessage, ProxyRequest, ProxyResponse, ProxyRpc, ProxyRpcHandler,
        WorkspaceFilesRequest,
    },
    source_control::{
        DiffInfo, FileDiff, GitCommitSummary, GitFileState, GitWorkspaceStatus,
    },
    stdio::{read_msg, write_msg},
    style::SemanticStyles,
};
use crossbeam_channel::{Receiver, Sender, unbounded};
use lsp_types::{
    CodeAction, CompletionItem, CompletionResponse, Diagnostic,
    DocumentSymbolResponse, GotoDefinitionResponse, Hover, InlineCompletionResponse,
    Location, Position, PrepareRenameResponse, SignatureHelp, SignatureHelpContext,
    TextDocumentItem, TextEdit, WorkspaceEdit,
};
use ropey::Rope;

pub const CONVERSATION_PAGE_SIZE: usize = 50;

pub(crate) enum DebugTerminalEvent {
    Request(DebugTerminalRequest),
    Retire(DapId),
}
static NEXT_WORKSPACE_FILE_REQUEST_ID: AtomicU64 = AtomicU64::new(0);

pub(crate) fn next_workspace_file_request_id() -> u64 {
    NEXT_WORKSPACE_FILE_REQUEST_ID
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1)
}

fn common_rope_prefix_len(previous: &Rope, current: &Rope) -> usize {
    let mut previous_chunks = previous.chunks();
    let mut current_chunks = current.chunks();
    let mut previous_chunk = previous_chunks.next();
    let mut current_chunk = current_chunks.next();
    let mut prefix_len = 0;

    while let (Some(old), Some(new)) = (previous_chunk, current_chunk) {
        if old.is_empty() {
            previous_chunk = previous_chunks.next();
            continue;
        }
        if new.is_empty() {
            current_chunk = current_chunks.next();
            continue;
        }

        let compared_len = old.len().min(new.len());
        if old.as_bytes()[..compared_len] != new.as_bytes()[..compared_len] {
            for (old_char, new_char) in previous
                .chars_at(prefix_len)
                .zip(current.chars_at(prefix_len))
            {
                if old_char != new_char {
                    break;
                }
                prefix_len += old_char.len_utf8();
            }
            return prefix_len;
        }

        // Matching UTF-8 bytes ending at either chunk boundary also end at a
        // character boundary in the other chunk.
        prefix_len += compared_len;
        previous_chunk = if old.len() == compared_len {
            previous_chunks.next()
        } else {
            Some(&old[compared_len..])
        };
        current_chunk = if new.len() == compared_len {
            current_chunks.next()
        } else {
            Some(&new[compared_len..])
        };
    }

    prefix_len
}

fn common_rope_suffix_len(
    previous: &Rope,
    current: &Rope,
    prefix_len: usize,
) -> usize {
    let limit = (previous.len() - prefix_len).min(current.len() - prefix_len);
    let mut previous_chunks = previous.chunks_at(previous.len()).0.reversed();
    let mut current_chunks = current.chunks_at(current.len()).0.reversed();
    let mut previous_chunk = previous_chunks.next();
    let mut current_chunk = current_chunks.next();
    let mut suffix_len = 0;

    while suffix_len < limit {
        let (Some(old), Some(new)) = (previous_chunk, current_chunk) else {
            break;
        };
        if old.is_empty() {
            previous_chunk = previous_chunks.next();
            continue;
        }
        if new.is_empty() {
            current_chunk = current_chunks.next();
            continue;
        }

        let compared_len = old.len().min(new.len()).min(limit - suffix_len);
        let old_start = old.len() - compared_len;
        let new_start = new.len() - compared_len;
        if old.as_bytes()[old_start..] != new.as_bytes()[new_start..] {
            for (old_char, new_char) in previous
                .chars_at(previous.len() - suffix_len)
                .reversed()
                .zip(current.chars_at(current.len() - suffix_len).reversed())
            {
                if suffix_len == limit || old_char != new_char {
                    break;
                }
                suffix_len += old_char.len_utf8();
            }
            return suffix_len;
        }

        // A matching suffix ending at a chunk boundary starts on a UTF-8
        // boundary in both ropes, even when their chunk sizes differ.
        suffix_len += compared_len;
        previous_chunk = if old_start == 0 {
            previous_chunks.next()
        } else {
            Some(&old[..old_start])
        };
        current_chunk = if new_start == 0 {
            current_chunks.next()
        } else {
            Some(&new[..new_start])
        };
    }

    suffix_len
}

fn editor_text_delta(previous: &Rope, current: &Rope) -> Option<AheadDelta> {
    let prefix_len = common_rope_prefix_len(previous, current);
    if prefix_len == previous.len() && prefix_len == current.len() {
        return None;
    }
    let suffix_len = common_rope_suffix_len(previous, current, prefix_len);

    let previous_middle_end = previous.len() - suffix_len;
    let current_middle_end = current.len() - suffix_len;
    let mut operations = Vec::with_capacity(4);
    if prefix_len > 0 {
        operations.push(DeltaOp::Retain(prefix_len));
    }
    let deleted_len = previous_middle_end - prefix_len;
    if deleted_len > 0 {
        operations.push(DeltaOp::Delete(deleted_len));
    }
    if current_middle_end > prefix_len {
        operations.push(DeltaOp::Insert(
            current.slice(prefix_len..current_middle_end).to_string(),
        ));
    }
    if suffix_len > 0 {
        operations.push(DeltaOp::Retain(suffix_len));
    }
    Some(AheadDelta::new(previous.len(), operations))
}

#[derive(Clone, Debug)]
pub struct LspCompletion {
    pub plugin_id: PluginId,
    pub item: CompletionItem,
}

#[derive(Clone, Debug, Default)]
pub struct LspCompletionBatch {
    pub items: Vec<LspCompletion>,
    pub is_incomplete: bool,
}

pub use ahead_rpc::proxy::LspCodeAction;

#[derive(Clone, Debug, Default)]
pub struct ProxyDiagnostics {
    pub items: Vec<Diagnostic>,
    pub version: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ProxyDiffState {
    pub branch: String,
    pub modified: Vec<PathBuf>,
    pub added: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LspServerStatus {
    pub name: String,
    pub ready: bool,
    pub quiescent: bool,
    pub message: Option<String>,
    pub on_type_formatting_triggers: Vec<String>,
}

impl LspServerStatus {
    pub fn is_ready(&self) -> bool {
        self.ready
    }

    pub fn is_error(&self) -> bool {
        self.quiescent && !self.ready
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointSummary {
    pub session_id: String,
    pub title: String,
    pub phase: String,
    pub exported_at: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct DurableSessionSummary {
    pub session: SessionListItem,
    pub harness: HarnessKind,
    pub external_agent_id: Option<String>,
    pub external_agent_name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DurableSessionState {
    pub view: SessionView,
    pub harness: HarnessKind,
    pub external_agent_id: Option<String>,
    pub work_items: Vec<WorkItem>,
    pub conversation: Vec<ConversationMessage>,
    pub conversation_has_older: bool,
    pub usage: Option<AgentUsage>,
    pub harness_warning: Option<String>,
}

pub(crate) struct ConversationState {
    pub warning: Option<String>,
    pub view: Option<SessionView>,
}

#[derive(Clone, Debug, Default)]
struct CachedConversation {
    messages: Vec<ConversationMessage>,
    has_older: bool,
    loaded_page: bool,
    revision: u64,
}

#[derive(Clone, Debug)]
pub struct DurableSessionRestore {
    pub sessions: Vec<DurableSessionSummary>,
    pub active: Option<DurableSessionState>,
}

fn harness_metadata(backend: Option<&str>) -> (HarnessKind, Option<String>) {
    let Some(backend) = backend else {
        return (HarnessKind::Ahead, None);
    };
    if backend == "external-agent-pending" {
        return (HarnessKind::ExternalAcp, None);
    }
    let Some(adapter_id) = backend.strip_prefix("external-agent:") else {
        return (HarnessKind::Ahead, None);
    };
    let adapter_id = adapter_id.strip_suffix("-fresh").unwrap_or(adapter_id);
    let adapter_id = (!adapter_id.is_empty() && adapter_id != "pending")
        .then(|| adapter_id.to_string());
    (HarnessKind::ExternalAcp, adapter_id)
}

#[derive(Clone, Debug, Default)]
pub struct DebugState {
    pub dap_id: Option<DapId>,
    pub thread_id: Option<ThreadId>,
    pub connection_closed: bool,
    pub state: DapSessionState,
    pub error: Option<String>,
    pub reason: String,
    pub frames: Vec<StackFrame>,
    pub thread_frames: HashMap<ThreadId, Vec<StackFrame>>,
    pub scopes: Vec<(
        ahead_rpc::dap_types::Scope,
        Vec<ahead_rpc::dap_types::Variable>,
    )>,
    pub breakpoints: HashMap<PathBuf, HashSet<u32>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferSnapshotRequest {
    pub session_id: String,
    pub turn_id: String,
    pub request_id: String,
    pub include_document_symbols: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorPresentationRequest {
    pub session_id: String,
    pub turn_id: String,
    pub request_id: String,
    pub action: AgentPresentationAction,
}

#[derive(Clone, Debug)]
pub(crate) struct CoreRequestEvent {
    pub request_id: RequestId,
    pub request: CoreRequest,
}

pub(crate) fn unavailable_core_request_response(
    request: CoreRequest,
    reason: &str,
) -> CoreResponse {
    match request {
        CoreRequest::ApplyWorkspaceEdit(_) => {
            CoreResponse::ApplyWorkspaceEdit(lsp_types::ApplyWorkspaceEditResponse {
                applied: false,
                failure_reason: Some(reason.to_string()),
                failed_change: None,
            })
        }
        CoreRequest::GitCredentialPrompt { .. } => {
            CoreResponse::GitCredentialAnswer { answer: None }
        }
    }
}

pub(crate) async fn await_editor_recovery<T>(
    receiver: async_channel::Receiver<Result<T, String>>,
    executor: &gpui_kit::BackgroundExecutor,
) -> Result<T, String> {
    let mut reply = std::pin::pin!(receiver.recv());
    let mut deadline = std::pin::pin!(executor.timer(Duration::from_secs(30)));
    std::future::poll_fn(|cx| {
        use std::task::Poll;
        if let Poll::Ready(result) = reply.as_mut().poll(cx) {
            return Poll::Ready(result.unwrap_or_else(|error| Err(error.to_string())));
        }
        if deadline.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err("Editor recovery was not confirmed within 30 seconds. Keep the buffers open and save them.".into()));
        }
        Poll::Pending
    }).await
}

fn bounded_agent_symbol(
    name: &str,
    range: lsp_types::Range,
    depth: u16,
) -> Option<AgentBufferSymbol> {
    let end_line =
        range
            .end
            .line
            .saturating_add(if range.end.character > 0 { 1 } else { 0 });
    if range.start.line >= end_line {
        return None;
    }

    let mut bounded_name = String::new();
    for character in name.trim().chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        if bounded_name.len() + character.len_utf8()
            > ahead_rpc::ahead::MAX_AGENT_BUFFER_SYMBOL_NAME_BYTES
        {
            break;
        }
        bounded_name.push(character);
    }
    let name = bounded_name.trim().to_string();
    (!name.is_empty()).then_some(AgentBufferSymbol {
        name,
        start_line: range.start.line,
        end_line,
        depth,
    })
}

fn agent_buffer_symbols(response: DocumentSymbolResponse) -> Vec<AgentBufferSymbol> {
    use ahead_rpc::ahead::MAX_AGENT_BUFFER_SYMBOLS_PER_SNAPSHOT;

    let mut symbols = Vec::new();
    match response {
        DocumentSymbolResponse::Flat(information) => {
            for symbol in information
                .into_iter()
                .take(MAX_AGENT_BUFFER_SYMBOLS_PER_SNAPSHOT.saturating_mul(4))
            {
                if let Some(symbol) =
                    bounded_agent_symbol(&symbol.name, symbol.location.range, 0)
                {
                    symbols.push(symbol);
                    if symbols.len() == MAX_AGENT_BUFFER_SYMBOLS_PER_SNAPSHOT {
                        break;
                    }
                }
            }
        }
        DocumentSymbolResponse::Nested(roots) => {
            let visit_limit = MAX_AGENT_BUFFER_SYMBOLS_PER_SNAPSHOT * 4;
            let mut pending = roots
                .into_iter()
                .take(visit_limit)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(|symbol| (symbol, 0u16))
                .collect::<Vec<_>>();
            let mut visited = 0;
            while let Some((symbol, depth)) = pending.pop() {
                visited += 1;
                if let Some(symbol_context) =
                    bounded_agent_symbol(&symbol.name, symbol.range, depth)
                {
                    symbols.push(symbol_context);
                }
                if symbols.len() == MAX_AGENT_BUFFER_SYMBOLS_PER_SNAPSHOT
                    || visited == visit_limit
                {
                    break;
                }
                if let Some(children) = symbol.children {
                    let children_limit =
                        visit_limit.saturating_sub(visited + pending.len());
                    let children = children
                        .into_iter()
                        .take(children_limit)
                        .collect::<Vec<_>>();
                    pending.extend(
                        children
                            .into_iter()
                            .rev()
                            .map(|child| (child, depth.saturating_add(1))),
                    );
                }
            }
        }
    }
    symbols
}

#[derive(Clone, Debug)]
pub(crate) struct InlinePredictionContext {
    pub session_id: Option<String>,
    pub model_context: Option<ahead_rpc::ahead::PredictionModelContext>,
    pub binding: Option<ahead_rpc::ahead::PredictionBinding>,
    session_revision: u64,
    configuration_revision: u64,
    buffer_revisions: HashMap<PathBuf, u64>,
}

pub(crate) struct InlinePrediction {
    pub text: String,
    pub range: Option<lsp_types::Range>,
    pub context: InlinePredictionContext,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceFileEvent {
    Changed(Vec<PathBuf>),
    Renamed {
        old_path: PathBuf,
        new_path: PathBuf,
    },
}

pub(crate) struct InlinePredictionRequest {
    pub context: InlinePredictionContext,
    receiver: async_channel::Receiver<InlinePrediction>,
    rpc: ProxyRpcHandler,
    request_id: RequestId,
}

impl InlinePredictionRequest {
    pub async fn recv(&self) -> Result<InlinePrediction, async_channel::RecvError> {
        self.receiver.recv().await
    }

    #[cfg(test)]
    fn try_recv(&self) -> Result<InlinePrediction, async_channel::TryRecvError> {
        self.receiver.try_recv()
    }
}

impl Drop for InlinePredictionRequest {
    fn drop(&mut self) {
        self.rpc.cancel_inline_completions(self.request_id);
    }
}

#[derive(Default)]
struct PredictionRevisions {
    next: u64,
    configuration: u64,
    sessions: HashMap<String, u64>,
    buffers: HashMap<PathBuf, u64>,
}

impl PredictionRevisions {
    fn route_is_current(&self, context: &InlinePredictionContext) -> bool {
        self.configuration == context.configuration_revision
            && context
                .session_id
                .as_ref()
                .map_or(0, |id| self.sessions.get(id).copied().unwrap_or(0))
                == context.session_revision
    }
}

pub struct ProxyClient {
    proxy_rpc: ProxyRpcHandler,
    core_request_tx: async_channel::Sender<CoreRequestEvent>,
    core_request_rx: Mutex<Option<async_channel::Receiver<CoreRequestEvent>>>,
    core_response_tx: Sender<(RequestId, CoreResponse)>,
    core_response_rx: Receiver<(RequestId, CoreResponse)>,
    pending_completion: Arc<
        Mutex<HashMap<usize, async_channel::Sender<Option<LspCompletionBatch>>>>,
    >,
    pending_signature_help:
        Arc<Mutex<HashMap<usize, async_channel::Sender<Option<SignatureHelp>>>>>,
    pending_hover: Arc<Mutex<HashMap<usize, Sender<Hover>>>>,
    pending_defs: Arc<Mutex<HashMap<usize, Sender<Vec<Location>>>>>,
    prediction_revisions: Mutex<PredictionRevisions>,
    prediction_subscribers: Mutex<Vec<async_channel::Sender<()>>>,
    request_seq: AtomicUsize,
    diagnostics: Arc<Mutex<HashMap<PathBuf, ProxyDiagnostics>>>,
    diag_version: Arc<Mutex<usize>>,
    diagnostic_subscribers: Mutex<Vec<async_channel::Sender<()>>>,
    diff: Arc<Mutex<ProxyDiffState>>,
    editor_snapshots: Mutex<HashMap<PathBuf, Rope>>,
    git_generation: AtomicU64,
    extension_syntax_generation: AtomicU64,
    lsp_servers: Arc<Mutex<HashMap<String, LspServerStatus>>>,
    language_extension_issues: Mutex<Vec<LanguageExtensionIssue>>,
    language_server_schemas:
        Arc<Mutex<HashMap<String, ahead_rpc::core::LanguageServerSchemas>>>,
    /// Uncommitted attribution anchors per file, refreshed from the ahead host.
    anchors: Arc<Mutex<HashMap<PathBuf, Vec<CodeAnchor>>>>,
    anchor_requests: Mutex<HashMap<PathBuf, usize>>,
    debug: Arc<Mutex<DebugState>>,
    debug_configs: Arc<Mutex<Vec<RunDebugConfig>>>,
    debug_config_error: Mutex<Option<String>>,
    debug_terminal_tx: async_channel::Sender<DebugTerminalEvent>,
    debug_terminal_rx: async_channel::Receiver<DebugTerminalEvent>,
    /// Durable conversation messages for the active session, keyed by session.
    conversations: Arc<Mutex<HashMap<String, CachedConversation>>>,
    shared_buffers: Arc<Mutex<HashMap<(String, String), SharedBufferSnapshot>>>,
    shared_buffer_changes:
        Mutex<HashMap<(String, String), (String, SharedBufferSnapshot)>>,
    shared_buffer_subscribers: Mutex<Vec<async_channel::Sender<()>>>,
    plans: Arc<Mutex<HashMap<String, Vec<AgentPlanEntry>>>>,
    tool_calls: Arc<Mutex<HashMap<String, Vec<AgentToolCall>>>>,
    patch_previews: Arc<Mutex<HashMap<String, Vec<AgentPatchPreview>>>>,
    thoughts: Arc<Mutex<HashMap<String, String>>>,
    available_commands: Arc<Mutex<HashMap<String, Vec<AgentCommand>>>>,
    config_options: Arc<Mutex<HashMap<String, Vec<AgentConfigOption>>>>,
    agent_auth_methods: Arc<Mutex<HashMap<String, Vec<AgentAuthMethod>>>>,
    usage: Arc<Mutex<HashMap<String, AgentUsage>>>,
    usage_update_subscribers: Mutex<Vec<async_channel::Sender<()>>>,
    pending_user_inputs: Arc<Mutex<HashMap<String, Vec<AgentUserInputRequest>>>>,
    pending_buffer_snapshot_requests: Arc<Mutex<Vec<BufferSnapshotRequest>>>,
    pending_editor_presentation_requests: Arc<Mutex<Vec<EditorPresentationRequest>>>,
    /// In-flight streamed turns, keyed by session id.
    streaming_turns: Arc<Mutex<HashMap<String, String>>>,
    workspace_file_generation: AtomicU64,
    workspace_file_subscribers: Mutex<Vec<async_channel::Sender<Vec<PathBuf>>>>,
    workspace_file_event_subscribers:
        Mutex<Vec<async_channel::Sender<WorkspaceFileEvent>>>,
    open_file_change_subscribers: Mutex<Vec<async_channel::Sender<PathBuf>>>,
    recovery_available: AtomicBool,
    // ponytail: show the latest proxy message in the status bar; queue them if burst loss matters.
    core_message: Mutex<Option<String>>,
    workspace: PathBuf,
    _child: Mutex<Option<Child>>,
}

impl ProxyClient {
    pub fn new(workspace: PathBuf) -> Arc<Self> {
        let client = Self::allocate(workspace.clone());
        client.refresh_debug_configs();
        if let Some(child) = Self::spawn_proxy(&client, client.proxy_rpc.clone()) {
            *client._child.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
            client.proxy_rpc.initialize(Some(workspace), 0, 0);
        } else {
            client.proxy_disconnected();
        }
        client
    }

    pub fn shutdown(&self) {
        self.invalidate_prediction_configuration();
        self.proxy_rpc.shutdown();
        let Some(mut child) = self
            ._child
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        else {
            return;
        };
        std::thread::spawn(move || {
            // The proxy owns LSP cleanup after app exit; do not block the GPUI
            // quit hook's short deadline or abandon a child when one window closes.
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while std::time::Instant::now() < deadline {
                match child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                    Err(error) => {
                        eprintln!("Could not inspect closing AHEAD proxy: {error}");
                        break;
                    }
                }
            }
            if let Err(error) = child.kill() {
                eprintln!("Could not stop unresponsive AHEAD proxy: {error}");
            }
            if let Err(error) = child.wait() {
                eprintln!("Could not reap closing AHEAD proxy: {error}");
            }
        });
    }

    fn allocate(workspace: PathBuf) -> Arc<Self> {
        let proxy_rpc = ProxyRpcHandler::new();
        let (debug_terminal_tx, debug_terminal_rx) = async_channel::unbounded();
        let (core_request_tx, core_request_rx) = async_channel::unbounded();
        let (core_response_tx, core_response_rx) = unbounded();
        Arc::new(Self {
            proxy_rpc,
            core_request_tx,
            core_request_rx: Mutex::new(Some(core_request_rx)),
            core_response_tx,
            core_response_rx,
            pending_completion: Arc::new(Mutex::new(HashMap::new())),
            pending_signature_help: Arc::new(Mutex::new(HashMap::new())),
            pending_hover: Arc::new(Mutex::new(HashMap::new())),
            pending_defs: Arc::new(Mutex::new(HashMap::new())),
            prediction_revisions: Mutex::new(PredictionRevisions::default()),
            prediction_subscribers: Mutex::new(Vec::new()),
            request_seq: AtomicUsize::new(1),
            diagnostics: Arc::new(Mutex::new(HashMap::new())),
            diag_version: Arc::new(Mutex::new(0)),
            diagnostic_subscribers: Mutex::new(Vec::new()),
            diff: Arc::new(Mutex::new(ProxyDiffState::default())),
            editor_snapshots: Mutex::new(HashMap::new()),
            git_generation: AtomicU64::new(0),
            extension_syntax_generation: AtomicU64::new(0),
            lsp_servers: Arc::new(Mutex::new(HashMap::new())),
            language_extension_issues: Mutex::new(Vec::new()),
            language_server_schemas: Arc::new(Mutex::new(HashMap::new())),
            anchors: Arc::new(Mutex::new(HashMap::new())),
            anchor_requests: Mutex::new(HashMap::new()),
            debug: Arc::new(Mutex::new(DebugState::default())),
            debug_configs: Arc::new(Mutex::new(default_debug_configs())),
            debug_config_error: Mutex::new(None),
            debug_terminal_tx,
            debug_terminal_rx,
            conversations: Arc::new(Mutex::new(HashMap::new())),
            shared_buffers: Arc::new(Mutex::new(HashMap::new())),
            shared_buffer_changes: Mutex::new(HashMap::new()),
            shared_buffer_subscribers: Mutex::new(Vec::new()),
            plans: Arc::new(Mutex::new(HashMap::new())),
            tool_calls: Arc::new(Mutex::new(HashMap::new())),
            patch_previews: Arc::new(Mutex::new(HashMap::new())),
            thoughts: Arc::new(Mutex::new(HashMap::new())),
            available_commands: Arc::new(Mutex::new(HashMap::new())),
            config_options: Arc::new(Mutex::new(HashMap::new())),
            agent_auth_methods: Arc::new(Mutex::new(HashMap::new())),
            usage: Arc::new(Mutex::new(HashMap::new())),
            usage_update_subscribers: Mutex::new(Vec::new()),
            pending_user_inputs: Arc::new(Mutex::new(HashMap::new())),
            pending_buffer_snapshot_requests: Arc::new(Mutex::new(Vec::new())),
            pending_editor_presentation_requests: Arc::new(Mutex::new(Vec::new())),
            streaming_turns: Arc::new(Mutex::new(HashMap::new())),
            workspace_file_generation: AtomicU64::new(0),
            workspace_file_subscribers: Mutex::new(Vec::new()),
            workspace_file_event_subscribers: Mutex::new(Vec::new()),
            open_file_change_subscribers: Mutex::new(Vec::new()),
            recovery_available: AtomicBool::new(false),
            core_message: Mutex::new(None),
            workspace,
            _child: Mutex::new(None),
        })
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(workspace: PathBuf) -> Arc<Self> {
        Self::allocate(workspace)
    }

    #[cfg(test)]
    pub(crate) fn rpc_for_test(&self) -> ProxyRpcHandler {
        self.proxy_rpc.clone()
    }

    fn proxy_bin() -> PathBuf {
        std::env::current_exe()
            .ok()
            .filter(|path| path.file_stem() == Some(std::ffi::OsStr::new("ahead")))
            .unwrap_or_else(|| PathBuf::from("target/debug/ahead"))
    }

    fn spawn_proxy(client: &Arc<Self>, proxy_rpc: ProxyRpcHandler) -> Option<Child> {
        let bin = Self::proxy_bin();
        if !bin.exists() {
            return None;
        }
        let mut child = Command::new(&bin)
            .arg("--proxy")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(if cfg!(debug_assertions) {
                Stdio::inherit()
            } else {
                Stdio::null()
            })
            .spawn()
            .ok()?;
        let stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;
        Self::bridge_stdio(client, proxy_rpc, stdin, stdout);
        Some(child)
    }

    fn bridge_stdio(
        client: &Arc<Self>,
        proxy_rpc: ProxyRpcHandler,
        stdin: std::process::ChildStdin,
        stdout: std::process::ChildStdout,
    ) {
        // App requests and Core RPC replies share stdin but use opposite RPC types.
        let app_tx = proxy_rpc.clone();
        let core_responses = client.core_response_rx.clone();
        let writer_client = client.clone();
        std::thread::spawn(move || {
            let mut writer = BufWriter::new(stdin);
            loop {
                let result = crossbeam_channel::select! {
                    recv(app_tx.rx()) -> message => match message {
                        Ok(ProxyRpc::Request(id, request)) => {
                            let wire: ProxyMessage = RpcMessage::Request(id, request);
                            write_msg(&mut writer, wire)
                        }
                        Ok(ProxyRpc::Notification(notification)) => {
                            let wire: ProxyMessage = RpcMessage::Notification(notification);
                            write_msg(&mut writer, wire)
                        }
                        Ok(ProxyRpc::Shutdown) | Err(_) => {
                            if cfg!(debug_assertions) {
                                eprintln!("AHEAD proxy writer stopped by RPC shutdown");
                            }
                            break;
                        },
                    },
                    recv(core_responses) -> message => match message {
                        Ok((id, response)) => {
                            let wire: ahead_rpc::core::CoreMessage = RpcMessage::Response(id, response);
                            write_msg(&mut writer, wire)
                        }
                        Err(_) => {
                            if cfg!(debug_assertions) {
                                eprintln!("AHEAD proxy writer stopped: core-response channel closed");
                            }
                            break;
                        },
                    },
                };
                if let Err(error) = result {
                    eprintln!("Could not write to AHEAD proxy: {error}");
                    writer_client.proxy_disconnected();
                    break;
                }
            }
        });
        // Core requests and notifications share stdout with app-request responses.
        let reader_client = client.clone();
        std::thread::spawn(move || {
            reader_client.read_proxy_messages(BufReader::new(stdout));
        });
    }

    fn read_proxy_messages(&self, mut reader: impl BufRead) {
        loop {
            let msg: std::io::Result<
                Option<RpcMessage<CoreRequest, CoreNotification, serde_json::Value>>,
            > = read_msg(&mut reader);
            let msg = match msg {
                Ok(message) => message,
                Err(error) => {
                    eprintln!("Could not read from AHEAD proxy: {error}");
                    self.proxy_disconnected();
                    return;
                }
            };
            let Some(msg) = msg else { continue };
            match msg {
                RpcMessage::Request(id, request) => {
                    self.route_core_request(id, request)
                }
                RpcMessage::Notification(notification) => {
                    self.route_core(notification)
                }
                RpcMessage::Response(id, value) => {
                    if let Err(error) =
                        route_proxy_response(&self.proxy_rpc, id, value)
                    {
                        eprintln!("Invalid AHEAD proxy response: {error}");
                        self.proxy_disconnected();
                        return;
                    }
                }
                RpcMessage::Error(id, error) => {
                    self.proxy_rpc.handle_response(id, Err(error));
                }
            }
        }
    }

    fn proxy_disconnected(&self) {
        self.invalidate_prediction_configuration();
        self.proxy_rpc.disconnect();
        let child_exit_status = self
            ._child
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
            .and_then(|child| match child.try_wait() {
                Ok(status) => status.map(|status| status.to_string()),
                Err(error) => {
                    eprintln!("Could not inspect disconnected AHEAD proxy: {error}");
                    None
                }
            });
        let retired_debugger = self
            .debug
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .dap_id;
        {
            let mut debug =
                self.debug.lock().unwrap_or_else(|error| error.into_inner());
            debug.connection_closed = true;
            debug.error = None;
            debug.state =
                DapSessionState::Failed("AHEAD proxy connection closed".into());
            debug.thread_id = None;
            debug.thread_frames.clear();
            debug.frames.clear();
            debug.scopes.clear();
            debug.reason = "AHEAD proxy connection closed".into();
        }
        if let Some(dap_id) = retired_debugger {
            drop(
                self.debug_terminal_tx
                    .try_send(DebugTerminalEvent::Retire(dap_id)),
            );
        }
        let message = child_exit_status.map_or_else(
            || {
                "AHEAD proxy connection closed. Restart AHEAD to restore stored sessions before retrying; an in-flight change may already have completed.".to_owned()
            },
            |status| {
                format!(
                    "AHEAD proxy connection closed (process exited: {status}). Restart AHEAD to restore stored sessions before retrying; an in-flight change may already have completed."
                )
            },
        );
        *self
            .core_message
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(message);
        for server in self
            .lsp_servers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values_mut()
        {
            server.ready = false;
            server.quiescent = true;
            server.message = Some(
                "AHEAD proxy connection closed. Restart AHEAD to reconnect.".into(),
            );
        }
        self.notify_editor_metadata();
    }

    fn route_core_request(&self, request_id: RequestId, request: CoreRequest) {
        if self.core_request_tx.receiver_count() == 0 {
            self.respond_core_request(
                request_id,
                unavailable_core_request_response(
                    request,
                    "No editor is available to handle the request",
                ),
            );
            return;
        }
        if let Err(error) = self.core_request_tx.try_send(CoreRequestEvent {
            request_id,
            request,
        }) {
            let event = match error {
                async_channel::TrySendError::Full(event)
                | async_channel::TrySendError::Closed(event) => event,
            };
            self.respond_core_request(
                event.request_id,
                unavailable_core_request_response(
                    event.request,
                    "The editor could not receive the request",
                ),
            );
        }
    }

    pub(crate) fn subscribe_core_requests(
        &self,
    ) -> Option<async_channel::Receiver<CoreRequestEvent>> {
        self.core_request_rx
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
    }

    pub(crate) fn respond_core_request(
        &self,
        request_id: RequestId,
        response: CoreResponse,
    ) {
        if let Err(error) = self.core_response_tx.send((request_id, response)) {
            eprintln!("Could not respond to AHEAD core request: {error}");
        }
    }

    pub fn diagnostics_for(&self, path: &PathBuf) -> Option<Vec<Diagnostic>> {
        self.diagnostics
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(path)
            .map(|d| d.items.clone())
    }

    pub fn subscribe_diagnostics(&self) -> async_channel::Receiver<()> {
        let (sender, receiver) = async_channel::bounded(1);
        self.diagnostic_subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(sender);
        receiver
    }

    pub(crate) fn subscribe_shared_buffers(&self) -> async_channel::Receiver<()> {
        let (sender, receiver) = async_channel::bounded(1);
        self.shared_buffer_subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(sender);
        receiver
    }

    pub(crate) fn shared_buffer_change_for_path(
        &self,
        workspace: &str,
        path: &str,
    ) -> Option<(String, String, SharedBufferSnapshot)> {
        self.shared_buffer_changes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .find(|((_, relative), _)| {
                std::path::Path::new(workspace).join(relative)
                    == std::path::Path::new(path)
            })
            .map(|((session_id, _), (previous, snapshot))| {
                (session_id.clone(), previous.clone(), snapshot.clone())
            })
    }

    pub fn all_diagnostics(&self) -> Vec<(PathBuf, Vec<Diagnostic>)> {
        self.diagnostics
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(_, diagnostics)| !diagnostics.items.is_empty())
            .map(|(path, diagnostics)| (path.clone(), diagnostics.items.clone()))
            .collect()
    }

    pub fn lsp_servers(&self) -> Vec<LspServerStatus> {
        let mut servers = self
            .lsp_servers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        servers.sort_by(|left, right| left.name.cmp(&right.name));
        servers
    }

    pub fn supports_on_type_formatting(&self, trigger_character: &str) -> bool {
        self.lsp_servers().iter().any(|server| {
            server.ready
                && server
                    .on_type_formatting_triggers
                    .iter()
                    .any(|trigger| trigger == trigger_character)
        })
    }

    pub fn language_extension_issues(&self) -> Vec<LanguageExtensionIssue> {
        self.language_extension_issues
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub fn language_server_schemas(
        &self,
    ) -> HashMap<String, ahead_rpc::core::LanguageServerSchemas> {
        self.language_server_schemas
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub fn restart_language_servers(&self) {
        self.proxy_rpc.notification(
            ahead_rpc::proxy::ProxyNotification::RestartLanguageServers {
                extension_id: None,
            },
        );
    }

    pub fn repair_language_server(&self, server_id: impl Into<String>) {
        self.proxy_rpc.notification(
            ahead_rpc::proxy::ProxyNotification::RepairLanguageServer {
                server_id: server_id.into(),
            },
        );
    }

    pub fn install_extension(
        &self,
        url: String,
        extension_id: String,
        version: String,
        expected_sha256: Option<String>,
        callback: impl FnOnce(Result<(), String>) + Send + 'static,
    ) {
        let proxy_rpc = self.proxy_rpc.clone();
        let restart_extension_id = extension_id.clone();
        self.proxy_rpc.install_language_extension(
            url,
            extension_id,
            version,
            expected_sha256,
            move |result| {
                if matches!(&result, Ok(ahead_rpc::proxy::ProxyResponse::Success {}))
                {
                    proxy_rpc.notification(
                        ahead_rpc::proxy::ProxyNotification::RestartLanguageServers {
                            extension_id: Some(restart_extension_id),
                        },
                    );
                }
                callback(match result {
                    Ok(ahead_rpc::proxy::ProxyResponse::Success {}) => Ok(()),
                    Ok(response) => Err(format!(
                        "unexpected extension install response: {response:?}"
                    )),
                    Err(error) => Err(error.message),
                });
            },
        );
    }

    pub fn uninstall_extension(
        &self,
        extension_id: String,
        callback: impl FnOnce(Result<(), String>) + Send + 'static,
    ) {
        self.proxy_rpc
            .uninstall_language_extension(extension_id, move |result| {
                callback(match result {
                    Ok(ahead_rpc::proxy::ProxyResponse::Success {}) => Ok(()),
                    Ok(response) => Err(format!(
                        "unexpected extension uninstall response: {response:?}"
                    )),
                    Err(error) => Err(error.message),
                });
            });
    }

    pub fn diff(&self) -> ProxyDiffState {
        self.diff.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn git_generation(&self) -> u64 {
        self.git_generation.load(Ordering::Acquire)
    }

    pub fn extension_syntax_generation(&self) -> u64 {
        self.extension_syntax_generation.load(Ordering::Acquire)
    }

    pub fn git_init(&self) {
        self.proxy_rpc.git_init();
    }

    pub fn trash_path(
        &self,
        path: PathBuf,
    ) -> async_channel::Receiver<Result<(), String>> {
        let (sender, receiver) = async_channel::bounded(1);
        self.proxy_rpc.trash_path(path, move |result| {
            let result = match result {
                Ok(ProxyResponse::Success {}) => Ok(()),
                Ok(other) => Err(format!("Unexpected Trash response: {other:?}")),
                Err(error) => Err(error.message),
            };
            if let Err(async_channel::TrySendError::Full(_)) =
                sender.try_send(result)
            {
                eprintln!("Duplicate Trash response");
            }
        });
        receiver
    }

    pub fn git_file_state(
        &self,
        path: PathBuf,
        content: String,
    ) -> async_channel::Receiver<Result<GitFileState, String>> {
        let (sender, receiver) = async_channel::bounded(1);
        self.proxy_rpc.request_async(
            ProxyRequest::GitFileState { path, content },
            move |response| {
                let result = match response {
                    Ok(ProxyResponse::GitFileState { state }) => Ok(state),
                    Ok(other) => Err(format!("Unexpected Git response: {other:?}")),
                    Err(error) => Err(error.message),
                };
                // A closed receiver means the buffer was closed or replaced.
                if let Err(async_channel::TrySendError::Full(_)) =
                    sender.try_send(result)
                {
                    eprintln!("Duplicate Git metadata response");
                }
            },
        );
        receiver
    }

    pub fn document_symbols(
        &self,
        path: PathBuf,
    ) -> async_channel::Receiver<Result<Vec<AgentBufferSymbol>, String>> {
        let (sender, receiver) = async_channel::bounded(1);
        self.proxy_rpc.request_async(
            ProxyRequest::GetDocumentSymbols { path },
            move |response| {
                let result = match response {
                    Ok(ProxyResponse::GetDocumentSymbols { resp }) => {
                        Ok(agent_buffer_symbols(resp))
                    }
                    Ok(other) => Err(format!(
                        "Unexpected document symbols response: {other:?}"
                    )),
                    Err(error) => Err(error.message),
                };
                if let Err(async_channel::TrySendError::Full(_)) =
                    sender.try_send(result)
                {
                    eprintln!("Duplicate document symbols response");
                }
            },
        );
        receiver
    }

    pub fn semantic_tokens(
        &self,
        path: PathBuf,
    ) -> async_channel::Receiver<Result<SemanticStyles, String>> {
        let (sender, receiver) = async_channel::bounded(1);
        self.proxy_rpc.get_semantic_tokens(path, move |response| {
            let result = match response {
                Ok(ProxyResponse::GetSemanticTokens { styles }) => Ok(styles),
                Ok(other) => {
                    Err(format!("Unexpected semantic tokens response: {other:?}"))
                }
                Err(error) => Err(error.message),
            };
            if let Err(async_channel::TrySendError::Full(_)) =
                sender.try_send(result)
            {
                eprintln!("Duplicate semantic tokens response");
            }
        });
        receiver
    }

    pub fn extension_syntax(
        &self,
        path: PathBuf,
    ) -> async_channel::Receiver<Result<ahead_rpc::style::ExtensionSyntax, String>>
    {
        let (sender, receiver) = async_channel::bounded(1);
        self.proxy_rpc.get_extension_syntax(path, move |response| {
            let result = match response {
                Ok(ProxyResponse::GetExtensionSyntax { syntax }) => Ok(syntax),
                Ok(other) => {
                    Err(format!("Unexpected extension syntax response: {other:?}"))
                }
                Err(error) => Err(error.message),
            };
            if let Err(async_channel::TrySendError::Full(_)) =
                sender.try_send(result)
            {
                eprintln!("Duplicate extension syntax response");
            }
        });
        receiver
    }

    pub fn git_status(
        &self,
    ) -> async_channel::Receiver<Result<GitWorkspaceStatus, String>> {
        let (sender, receiver) = async_channel::bounded(1);
        self.proxy_rpc
            .request_async(ProxyRequest::GitStatus {}, move |response| {
                let result = match response {
                    Ok(ProxyResponse::GitStatus { status }) => Ok(status),
                    Ok(other) => Err(format!("Unexpected Git response: {other:?}")),
                    Err(error) => Err(error.message),
                };
                if let Err(async_channel::TrySendError::Full(_)) =
                    sender.try_send(result)
                {
                    eprintln!("Duplicate Git status response");
                }
            });
        receiver
    }

    pub fn git_checkout(
        &self,
        reference: String,
    ) -> async_channel::Receiver<Result<(), String>> {
        self.git_operation(ProxyRequest::GitCheckout { reference })
    }

    pub fn git_create_branch(
        &self,
        reference: String,
    ) -> async_channel::Receiver<Result<(), String>> {
        self.git_operation(ProxyRequest::GitCreateBranch { reference })
    }

    pub fn git_delete_branch(
        &self,
        reference: String,
    ) -> async_channel::Receiver<Result<(), String>> {
        self.git_operation(ProxyRequest::GitDeleteBranch { reference })
    }

    pub fn git_push(
        &self,
        remote: Option<String>,
    ) -> async_channel::Receiver<Result<(), String>> {
        self.git_operation(ProxyRequest::GitPush { remote })
    }

    pub fn git_history(
        &self,
        limit: u32,
    ) -> async_channel::Receiver<Result<Vec<GitCommitSummary>, String>> {
        let (sender, receiver) = async_channel::bounded(1);
        self.proxy_rpc.request_async(
            ProxyRequest::GitHistory { limit },
            move |response| {
                let result = match response {
                    Ok(ProxyResponse::GitHistory { commits }) => Ok(commits),
                    Ok(other) => {
                        Err(format!("Unexpected Git history response: {other:?}"))
                    }
                    Err(error) => Err(error.message),
                };
                if let Err(async_channel::TrySendError::Full(_)) =
                    sender.try_send(result)
                {
                    eprintln!("Duplicate Git history response");
                }
            },
        );
        receiver
    }

    pub fn git_commit_patch(
        &self,
        id: String,
    ) -> async_channel::Receiver<Result<ahead_rpc::source_control::GitPatch, String>>
    {
        let (sender, receiver) = async_channel::bounded(1);
        self.proxy_rpc.request_async(
            ProxyRequest::GitCommitPatch { id },
            move |response| {
                let result = match response {
                    Ok(ProxyResponse::GitCommitPatch { patch }) => Ok(patch),
                    Ok(other) => Err(format!(
                        "Unexpected Git commit diff response: {other:?}"
                    )),
                    Err(error) => Err(error.message),
                };
                if let Err(async_channel::TrySendError::Full(_)) =
                    sender.try_send(result)
                {
                    eprintln!("Duplicate Git commit diff response");
                }
            },
        );
        receiver
    }

    pub fn git_worktree_file_patch(
        &self,
        path: PathBuf,
    ) -> async_channel::Receiver<Result<ahead_rpc::source_control::GitPatch, String>>
    {
        let (sender, receiver) = async_channel::bounded(1);
        self.proxy_rpc.request_async(
            ProxyRequest::GitWorktreeFilePatch { path },
            move |response| {
                let result = match response {
                    Ok(ProxyResponse::GitWorktreeFilePatch { patch }) => Ok(patch),
                    Ok(other) => Err(format!(
                        "Unexpected Git working-tree diff response: {other:?}"
                    )),
                    Err(error) => Err(error.message),
                };
                if let Err(async_channel::TrySendError::Full(_)) =
                    sender.try_send(result)
                {
                    eprintln!("Duplicate Git working-tree diff response");
                }
            },
        );
        receiver
    }

    pub fn git_worktree_patch(
        &self,
    ) -> async_channel::Receiver<Result<ahead_rpc::source_control::GitPatch, String>>
    {
        let (sender, receiver) = async_channel::bounded(1);
        self.proxy_rpc.request_async(
            ProxyRequest::GitWorktreePatch {},
            move |response| {
                let result = match response {
                    Ok(ProxyResponse::GitWorktreePatch { patch }) => Ok(patch),
                    Ok(other) => Err(format!(
                        "Unexpected Git working-tree diff response: {other:?}"
                    )),
                    Err(error) => Err(error.message),
                };
                if let Err(async_channel::TrySendError::Full(_)) =
                    sender.try_send(result)
                {
                    eprintln!("Duplicate Git working-tree diff response");
                }
            },
        );
        receiver
    }

    pub fn git_commit(
        &self,
        message: String,
    ) -> async_channel::Receiver<Result<(), String>> {
        self.git_operation(ProxyRequest::GitCommit {
            message,
            diffs: Vec::new(),
        })
    }

    pub fn git_stage_all(&self) -> async_channel::Receiver<Result<(), String>> {
        self.git_operation(ProxyRequest::GitStageAll {})
    }

    pub fn git_stage_paths(
        &self,
        paths: Vec<String>,
        staged: bool,
    ) -> async_channel::Receiver<Result<(), String>> {
        self.git_operation(ProxyRequest::GitStagePaths { paths, staged })
    }

    fn git_operation(
        &self,
        request: ProxyRequest,
    ) -> async_channel::Receiver<Result<(), String>> {
        let (sender, receiver) = async_channel::bounded(1);
        self.proxy_rpc.request_async(request, move |response| {
            let result = match response {
                Ok(ProxyResponse::Success {}) => Ok(()),
                Ok(other) => Err(format!("Unexpected Git response: {other:?}")),
                Err(error) => Err(error.message),
            };
            if let Err(async_channel::TrySendError::Full(_)) =
                sender.try_send(result)
            {
                eprintln!("Duplicate Git operation response");
            }
        });
        receiver
    }

    pub fn debug(&self) -> DebugState {
        self.debug.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn dap_scopes(
        &self,
        dap_id: DapId,
        frame_id: usize,
    ) -> async_channel::Receiver<
        Result<
            Vec<(
                ahead_rpc::dap_types::Scope,
                Vec<ahead_rpc::dap_types::Variable>,
            )>,
            String,
        >,
    > {
        let (sender, receiver) = async_channel::bounded(1);
        self.proxy_rpc
            .dap_get_scopes(dap_id, frame_id, move |response| {
                let result = match response {
                    Ok(ahead_rpc::proxy::ProxyResponse::DapGetScopesResponse {
                        scopes,
                    }) => Ok(scopes),
                    Ok(other) => Err(format!(
                        "Unexpected debugger scopes response: {other:?}"
                    )),
                    Err(error) => Err(error.message),
                };
                if let Err(error) = sender.try_send(result)
                    && !matches!(error, async_channel::TrySendError::Closed(_))
                {
                    eprintln!("Delivering debugger scopes: {error}");
                }
            });
        receiver
    }

    pub fn dap_variables(
        &self,
        dap_id: DapId,
        reference: usize,
    ) -> async_channel::Receiver<Result<Vec<ahead_rpc::dap_types::Variable>, String>>
    {
        let (sender, receiver) = async_channel::bounded(1);
        self.proxy_rpc
            .dap_variable(dap_id, reference, move |response| {
                let result = match response {
                    Ok(ahead_rpc::proxy::ProxyResponse::DapVariableResponse {
                        varialbes,
                    }) => Ok(varialbes),
                    Ok(other) => Err(format!(
                        "Unexpected debugger variables response: {other:?}"
                    )),
                    Err(error) => Err(error.message),
                };
                if let Err(error) = sender.try_send(result)
                    && !matches!(error, async_channel::TrySendError::Closed(_))
                {
                    eprintln!("Delivering debugger variables: {error}");
                }
            });
        receiver
    }

    pub fn stored_configs(&self) -> Vec<RunDebugConfig> {
        self.debug_configs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn debug_config_error(&self) -> Option<String> {
        self.debug_config_error
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn refresh_debug_configs(&self) {
        let run_toml = self.workspace.join(".ahead").join("run.toml");
        let (mut configs, error) = match std::fs::read_to_string(&run_toml) {
            Ok(text) => match parse_project_debug_configs(&text) {
                Ok(configs) if configs.is_empty() => (
                    Vec::new(),
                    Some(format!(
                        "{} has no [[configurations]] entries",
                        run_toml.display()
                    )),
                ),
                Ok(configs) => (configs, None),
                Err(error) => (
                    Vec::new(),
                    Some(format!("Could not parse {}: {error}", run_toml.display())),
                ),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (default_debug_configs_for_workspace(&self.workspace), None)
            }
            Err(error) => (
                Vec::new(),
                Some(format!("Could not read {}: {error}", run_toml.display())),
            ),
        };
        for config in &mut configs {
            if let Some(cwd) = config.cwd.as_deref() {
                let cwd = PathBuf::from(cwd);
                if !cwd.is_absolute() {
                    config.cwd = Some(
                        self.workspace.join(cwd).to_string_lossy().into_owned(),
                    );
                }
            }
        }
        *self
            .debug_configs
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = configs;
        *self
            .debug_config_error
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = error;
    }

    /// Refreshes uncommitted attribution anchors for the given absolute files.
    /// The host stores repository-relative paths, while the editor cache is
    /// keyed by the paths used by the open document.
    pub fn refresh_anchors(self: &Arc<Self>, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        let repo_paths: Vec<String> = paths
            .iter()
            .map(|path| {
                path.strip_prefix(&self.workspace)
                    .map(|relative| relative.to_string_lossy().to_string())
                    .unwrap_or_else(|_| path.to_string_lossy().to_string())
            })
            .collect();
        let request_id = self.next_id();
        let paths = paths.to_vec();
        self.anchor_requests
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .extend(paths.iter().cloned().map(|path| (path, request_id)));
        let client = Arc::downgrade(self);
        self.proxy_rpc.ahead_request(
            AheadRequest::ListAnchorsForPaths {
                paths: repo_paths.clone(),
            },
            move |result| {
                let Some(client) = client.upgrade() else {
                    return;
                };
                let result =
                    result.map_err(|error| error.message).and_then(|value| {
                        serde_json::from_value::<Vec<CodeAnchor>>(value)
                            .map_err(|error| error.to_string())
                    });
                let current = client
                    .anchor_requests
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let paths: Vec<_> = paths
                    .iter()
                    .zip(&repo_paths)
                    .filter(|(path, _)| current.get(*path) == Some(&request_id))
                    .collect();
                if paths.is_empty() {
                    return;
                }
                match result {
                    Ok(anchors) => {
                        let mut cache = client
                            .anchors
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        for (path, relative) in paths {
                            cache.insert(
                                path.clone(),
                                anchors
                                    .iter()
                                    .filter(|anchor| anchor.path == *relative)
                                    .cloned()
                                    .collect(),
                            );
                        }
                    }
                    Err(error) => {
                        *client
                            .core_message
                            .lock()
                            .unwrap_or_else(|error| error.into_inner()) = Some(
                            format!("Could not refresh file attribution: {error}"),
                        );
                    }
                }
                drop(current);
                client.notify_editor_metadata();
            },
        );
    }

    fn notify_editor_metadata(&self) {
        self.diagnostic_subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|sender| match sender.try_send(()) {
                Ok(()) | Err(async_channel::TrySendError::Full(())) => true,
                Err(async_channel::TrySendError::Closed(())) => false,
            });
    }

    /// Cached uncommitted attribution anchors for one file.
    pub fn anchors_for(&self, path: &PathBuf) -> Vec<CodeAnchor> {
        self.anchors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(path)
            .cloned()
            .unwrap_or_default()
    }

    pub fn workspace_file_generation(&self) -> u64 {
        self.workspace_file_generation.load(Ordering::SeqCst)
    }

    pub fn subscribe_workspace_file_changes(
        &self,
    ) -> async_channel::Receiver<Vec<PathBuf>> {
        let (sender, receiver) = async_channel::unbounded();
        self.workspace_file_subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(sender);
        receiver
    }

    pub(crate) fn subscribe_workspace_file_events(
        &self,
    ) -> async_channel::Receiver<WorkspaceFileEvent> {
        let (sender, receiver) = async_channel::unbounded();
        self.workspace_file_event_subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(sender);
        receiver
    }

    pub fn subscribe_open_file_changes(&self) -> async_channel::Receiver<PathBuf> {
        let (sender, receiver) = async_channel::unbounded();
        self.open_file_change_subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(sender);
        receiver
    }

    /// Enqueue without blocking the UI; await the handle from background work.
    pub fn workspace_files(&self, request_id: u64) -> WorkspaceFilesRequest {
        self.proxy_rpc.workspace_files(request_id)
    }

    pub fn cancel_workspace_files(&self, request_id: u64) {
        self.proxy_rpc.cancel_workspace_files(request_id);
    }

    /// Route one proxy-originated core notification into local state.
    pub fn route_core(&self, notif: CoreNotification) {
        match notif {
            CoreNotification::OpenFileChanged { path, .. } => {
                self.invalidate_prediction_buffer(&path);
                self.open_file_change_subscribers
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .retain(|sender| match sender.try_send(path.clone()) {
                        Ok(()) | Err(async_channel::TrySendError::Full(_)) => true,
                        Err(async_channel::TrySendError::Closed(_)) => false,
                    });
            }
            CoreNotification::RunInTerminal { request } => {
                let debug = self.debug();
                if debug.dap_id != Some(request.dap_id) || !debug.state.can_stop() {
                    self.reply_debug_terminal(
                        &request,
                        Err("debug session is stopping".into()),
                    );
                } else if let Err(error) = self
                    .debug_terminal_tx
                    .try_send(DebugTerminalEvent::Request(request))
                {
                    eprintln!(
                        "Could not forward debugger terminal request: {error}"
                    );
                }
            }
            CoreNotification::WorkspaceFileChange { generation, paths } => {
                if generation
                    < self
                        .workspace_file_generation
                        .fetch_max(generation, Ordering::SeqCst)
                {
                    return;
                }
                if paths.is_empty() {
                    self.invalidate_prediction_configuration();
                } else {
                    let affected_buffers: Vec<_> = {
                        let revisions = self
                            .prediction_revisions
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        paths
                            .iter()
                            .filter(|path| revisions.buffers.contains_key(*path))
                            .cloned()
                            .collect()
                    };
                    for path in affected_buffers {
                        self.invalidate_prediction_buffer(&path);
                    }
                }
                self.workspace_file_subscribers
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .retain(|sender| match sender.try_send(paths.clone()) {
                        Ok(()) | Err(async_channel::TrySendError::Full(_)) => true,
                        Err(async_channel::TrySendError::Closed(_)) => false,
                    });
                self.workspace_file_event_subscribers
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .retain(|sender| {
                        match sender
                            .try_send(WorkspaceFileEvent::Changed(paths.clone()))
                        {
                            Ok(()) | Err(async_channel::TrySendError::Full(_)) => {
                                true
                            }
                            Err(async_channel::TrySendError::Closed(_)) => false,
                        }
                    });
            }
            CoreNotification::WorkspaceFileRename {
                generation,
                old_path,
                new_path,
            } => {
                if generation
                    < self
                        .workspace_file_generation
                        .fetch_max(generation, Ordering::SeqCst)
                {
                    return;
                }
                self.workspace_file_event_subscribers
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .retain(|sender| {
                        match sender.try_send(WorkspaceFileEvent::Renamed {
                            old_path: old_path.clone(),
                            new_path: new_path.clone(),
                        }) {
                            Ok(()) | Err(async_channel::TrySendError::Full(_)) => {
                                true
                            }
                            Err(async_channel::TrySendError::Closed(_)) => false,
                        }
                    });
            }
            CoreNotification::CompletionResponse {
                request_id,
                resp,
                plugin_id,
                ..
            } => {
                if let Some(tx) = self
                    .pending_completion
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&request_id)
                {
                    let list = resp.map(|response| {
                        let (items, is_incomplete) = match response {
                            CompletionResponse::Array(items) => (items, false),
                            CompletionResponse::List(list) => {
                                (list.items, list.is_incomplete)
                            }
                        };
                        LspCompletionBatch {
                            items: items
                                .into_iter()
                                .map(|item| LspCompletion { plugin_id, item })
                                .collect(),
                            is_incomplete,
                        }
                    });
                    if let Err(error) = tx.try_send(list) {
                        eprintln!("Delivering completion list: {error}");
                    }
                }
            }
            CoreNotification::SignatureHelpResponse {
                request_id, resp, ..
            } => {
                if let Some(sender) = self
                    .pending_signature_help
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&request_id)
                {
                    let help = (!resp.signatures.is_empty()).then_some(resp);
                    if let Err(error) = sender.try_send(help) {
                        eprintln!("Delivering signature help: {error}");
                    }
                }
            }
            CoreNotification::PublishDiagnostics { diagnostics } => {
                let path = diagnostics.uri.to_file_path().unwrap_or_default();
                let mut version =
                    self.diag_version.lock().unwrap_or_else(|e| e.into_inner());
                *version += 1;
                self.diagnostics
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(
                        path,
                        ProxyDiagnostics {
                            items: diagnostics.diagnostics,
                            version: *version,
                        },
                    );
                self.notify_editor_metadata();
            }
            CoreNotification::DiffInfo { diff } => {
                let mut state = self.diff.lock().unwrap_or_else(|e| e.into_inner());
                state.branch = diff.head.clone();
                state.modified.clear();
                state.added.clear();
                for f in &diff.diffs {
                    match f {
                        FileDiff::Modified(p) => state.modified.push(p.clone()),
                        FileDiff::Added(p) => state.added.push(p.clone()),
                        _ => {}
                    }
                }
                drop(state);
                self.git_generation.fetch_add(1, Ordering::Release);
                self.notify_editor_metadata();
            }
            CoreNotification::ServerStatus { params } => {
                let ready = params.is_ok();
                let quiescent = params.is_quiescent();
                let message = params.message.clone();
                let on_type_formatting_triggers =
                    params.on_type_formatting_triggers.clone();
                let name = params
                    .server_name
                    .unwrap_or_else(|| "language-server".to_string());
                self.lsp_servers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(
                        name.clone(),
                        LspServerStatus {
                            name,
                            ready,
                            quiescent,
                            message,
                            on_type_formatting_triggers,
                        },
                    );
                self.notify_editor_metadata();
            }
            CoreNotification::LanguageExtensionIssues { issues } => {
                *self
                    .language_extension_issues
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = issues;
                self.extension_syntax_generation
                    .fetch_add(1, Ordering::Release);
                self.notify_editor_metadata();
            }
            CoreNotification::LanguageServerSchemas { server_id, schemas } => {
                self.language_server_schemas
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .insert(server_id, schemas);
                self.notify_editor_metadata();
            }
            CoreNotification::LanguageServerStatusesCleared => {
                self.lsp_servers
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clear();
                self.language_server_schemas
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clear();
                self.notify_editor_metadata();
            }
            CoreNotification::DapStopped {
                dap_id,
                stopped,
                stack_frames,
                variables,
            } => {
                let mut dbg = self.debug.lock().unwrap_or_else(|e| e.into_inner());
                if dbg.dap_id != Some(dap_id) || !dbg.state.can_stop() {
                    return;
                }
                dbg.state = DapSessionState::Stopped;
                dbg.error = None;
                dbg.thread_id = stopped.thread_id.or_else(|| {
                    stopped
                        .all_threads_stopped
                        .unwrap_or(false)
                        .then(|| stack_frames.keys().min().copied())
                        .flatten()
                });
                dbg.reason = stopped.reason.clone();
                dbg.frames = dbg
                    .thread_id
                    .and_then(|thread_id| stack_frames.get(&thread_id).cloned())
                    .unwrap_or_default();
                dbg.thread_frames = stack_frames;
                dbg.scopes = variables;
                drop(dbg);
                self.notify_editor_metadata();
            }
            CoreNotification::DapSessionState { dap_id, state } => {
                let mut debug =
                    self.debug.lock().unwrap_or_else(|error| error.into_inner());
                if debug.dap_id != Some(dap_id)
                    || !debug.state.is_active()
                    || (debug.state == DapSessionState::Stopping && state.can_stop())
                {
                    return;
                }
                let retire_terminal = matches!(
                    state,
                    DapSessionState::Terminated | DapSessionState::Failed(_)
                );
                debug.state = state;
                debug.error = None;
                debug.thread_id = None;
                debug.thread_frames.clear();
                debug.frames.clear();
                debug.scopes.clear();
                debug.reason.clear();
                drop(debug);
                if retire_terminal {
                    drop(
                        self.debug_terminal_tx
                            .try_send(DebugTerminalEvent::Retire(dap_id)),
                    );
                }
                self.notify_editor_metadata();
            }
            CoreNotification::DapError { dap_id, message } => {
                let mut debug =
                    self.debug.lock().unwrap_or_else(|error| error.into_inner());
                if debug.dap_id != Some(dap_id) || !debug.state.can_stop() {
                    return;
                }
                debug.error = Some(message);
                drop(debug);
                self.notify_editor_metadata();
            }
            CoreNotification::DapBreakpointsResp {
                dap_id,
                path,
                breakpoints,
            } => {
                let mut dbg = self.debug.lock().unwrap_or_else(|e| e.into_inner());
                if dbg.dap_id != Some(dap_id) || !dbg.state.can_stop() {
                    return;
                }
                dbg.breakpoints.insert(
                    self.breakpoint_path(&path),
                    breakpoints
                        .iter()
                        .filter_map(|b| b.line.map(|l| l as u32))
                        .collect(),
                );
                drop(dbg);
                self.notify_editor_metadata();
            }
            CoreNotification::AheadNotification { notification } => {
                self.route_ahead(notification);
            }
            CoreNotification::ShowMessage { title, message } => {
                *self
                    .core_message
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) =
                    Some(format!("{title}: {}", message.message));
            }
            _ => {}
        }
    }

    pub fn take_core_message(&self) -> Option<String> {
        self.core_message
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
    }

    /// Routes streamed AHEAD harness notifications into local conversation
    /// state. Deltas append to the in-flight message; turn state finalizes it.
    pub fn route_ahead(&self, notification: AheadNotification) {
        match notification {
            AheadNotification::PredictionContextChanged { session_id } => {
                if let Some(session_id) = session_id {
                    self.invalidate_prediction_session(&session_id);
                } else {
                    self.invalidate_prediction_configuration();
                }
            }
            AheadNotification::SessionUpdated { view } => {
                self.invalidate_prediction_session(&view.session.id);
            }
            AheadNotification::SharedBufferChanged {
                session_id,
                snapshot,
                previous_content,
            } => {
                self.invalidate_prediction_buffer(
                    &self.workspace.join(&snapshot.path),
                );
                self.shared_buffer_changes
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .insert(
                        (session_id.clone(), snapshot.path.clone()),
                        (previous_content, snapshot.clone()),
                    );
                self.shared_buffers
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .insert((session_id, snapshot.path.clone()), snapshot);
                self.shared_buffer_subscribers
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .retain(|sender| match sender.try_send(()) {
                        Ok(()) | Err(async_channel::TrySendError::Full(())) => true,
                        Err(async_channel::TrySendError::Closed(())) => false,
                    });
            }
            AheadNotification::ConversationMessageAdded { message } => {
                self.invalidate_prediction_session(&message.session_id);
                let mut conversations =
                    self.conversations.lock().unwrap_or_else(|e| e.into_inner());
                let conversation =
                    conversations.entry(message.session_id.clone()).or_default();
                conversation.revision = conversation.revision.wrapping_add(1);
                if let Some(existing) = conversation
                    .messages
                    .iter_mut()
                    .find(|existing| existing.id == message.id)
                {
                    *existing = message;
                } else {
                    conversation.messages.push(message);
                }
                conversation.messages.sort_by(|left, right| {
                    (left.sequence, &left.id).cmp(&(right.sequence, &right.id))
                });
            }
            AheadNotification::AgentMessageDelta {
                session_id,
                turn_id,
                delta,
            } => {
                if self.active_turn_id(&session_id).as_deref()
                    != Some(turn_id.as_str())
                {
                    return;
                }
                let mut conversations =
                    self.conversations.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(conversation) = conversations.get_mut(&session_id) {
                    conversation.revision = conversation.revision.wrapping_add(1);
                    if let Some(last) =
                        conversation.messages.iter_mut().rev().find(|m| {
                            m.role == "agent"
                                && m.status == "streaming"
                                && m.turn_id == turn_id
                        })
                    {
                        last.content.push_str(&delta);
                    }
                }
            }
            AheadNotification::AgentThoughtDelta {
                session_id,
                turn_id,
                delta,
            } => {
                if self.active_turn_id(&session_id).as_deref()
                    != Some(turn_id.as_str())
                {
                    return;
                }
                self.thoughts
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entry(session_id)
                    .or_default()
                    .push_str(&delta);
            }
            AheadNotification::AgentContextCompacted {
                session_id,
                turn_id,
            } => {
                if self.active_turn_id(&session_id).as_deref()
                    != Some(turn_id.as_str())
                {
                    return;
                }
                self.thoughts
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entry(session_id)
                    .or_default()
                    .push_str("\nContext compacted.\n");
            }
            AheadNotification::AgentTurnState {
                session_id,
                turn_id,
                message_id,
                state,
            } => {
                let mut streaming = self
                    .streaming_turns
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if state == "streaming" {
                    if streaming
                        .get(&session_id)
                        .is_none_or(|active| active == &turn_id)
                    {
                        streaming.insert(session_id.clone(), turn_id.clone());
                        self.plans
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&session_id);
                        self.tool_calls
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&session_id);
                        self.patch_previews
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&session_id);
                        self.thoughts
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&session_id);
                        self.pending_user_inputs
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&session_id);
                        self.pending_buffer_snapshot_requests
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .retain(|request| request.session_id != session_id);
                        self.pending_editor_presentation_requests
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .retain(|request| request.session_id != session_id);
                    }
                } else if streaming
                    .get(&session_id)
                    .is_some_and(|active| active == &turn_id)
                {
                    streaming.remove(&session_id);
                    self.thoughts
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&session_id);
                    self.pending_user_inputs
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&session_id);
                    self.pending_buffer_snapshot_requests
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .retain(|request| request.session_id != session_id);
                    self.pending_editor_presentation_requests
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .retain(|request| request.session_id != session_id);
                }
                let mut conversations =
                    self.conversations.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(list) = conversations.get_mut(&session_id) {
                    list.revision = list.revision.wrapping_add(1);
                    if let Some(message) =
                        list.messages.iter_mut().find(|m| m.id == message_id)
                    {
                        message.status = state;
                    }
                }
            }
            AheadNotification::AgentPlan {
                session_id,
                turn_id,
                entries,
            } => {
                if self.active_turn_id(&session_id).as_deref()
                    != Some(turn_id.as_str())
                {
                    return;
                }
                self.invalidate_prediction_session(&session_id);
                self.plans
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(session_id, entries);
            }
            AheadNotification::AgentToolCall {
                session_id,
                turn_id,
                call,
            } => {
                if self.active_turn_id(&session_id).as_deref()
                    != Some(turn_id.as_str())
                {
                    return;
                }
                let mut tool_calls =
                    self.tool_calls.lock().unwrap_or_else(|e| e.into_inner());
                let calls = tool_calls.entry(session_id).or_default();
                upsert_agent_tool_call(calls, call);
            }
            AheadNotification::AgentPatchProgress {
                session_id,
                turn_id,
                preview,
            } => {
                if self.active_turn_id(&session_id).as_deref()
                    != Some(turn_id.as_str())
                {
                    return;
                }
                let mut previews = self
                    .patch_previews
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let session_previews = previews.entry(session_id).or_default();
                if let Some(existing) = session_previews
                    .iter_mut()
                    .find(|existing| existing.call_id == preview.call_id)
                {
                    *existing = preview;
                } else {
                    session_previews.push(preview);
                }
            }
            AheadNotification::AgentUsage {
                session_id,
                turn_id,
                usage,
            } => {
                if self.active_turn_id(&session_id).as_deref()
                    != Some(turn_id.as_str())
                {
                    return;
                }
                self.usage
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(session_id, usage);
                self.notify_usage_updates();
            }
            AheadNotification::AgentUserInputRequested {
                session_id,
                turn_id,
                request,
            } => {
                if self.active_turn_id(&session_id).as_deref()
                    != Some(turn_id.as_str())
                {
                    return;
                }
                let mut pending = self
                    .pending_user_inputs
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let requests = pending.entry(session_id).or_default();
                if let Some(existing) = requests
                    .iter_mut()
                    .find(|existing| existing.request_id == request.request_id)
                {
                    *existing = request;
                } else {
                    requests.push(request);
                }
            }
            AheadNotification::AgentUserInputCancelled {
                session_id,
                turn_id,
                request_id,
            } => {
                if self.active_turn_id(&session_id).as_deref()
                    != Some(turn_id.as_str())
                {
                    return;
                }
                self.forget_answered_user_input(&session_id, &request_id);
            }
            AheadNotification::AgentBufferSnapshotsRequested {
                session_id,
                turn_id,
                request_id,
                include_document_symbols,
            } => {
                if self.active_turn_id(&session_id).as_deref()
                    != Some(turn_id.as_str())
                {
                    return;
                }
                self.pending_buffer_snapshot_requests
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(BufferSnapshotRequest {
                        session_id,
                        turn_id,
                        request_id,
                        include_document_symbols,
                    });
            }
            AheadNotification::AgentPresentationRequested {
                session_id,
                turn_id,
                request_id,
                action,
            } => {
                if self.active_turn_id(&session_id).as_deref()
                    != Some(turn_id.as_str())
                {
                    return;
                }
                self.pending_editor_presentation_requests
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(EditorPresentationRequest {
                        session_id,
                        turn_id,
                        request_id,
                        action,
                    });
            }
            AheadNotification::AgentCommandsAvailable {
                session_id,
                commands,
            } => {
                self.available_commands
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(session_id, commands);
            }
            AheadNotification::AgentConfigOptionsAvailable {
                session_id,
                options,
            } => {
                self.config_options
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(session_id, options);
            }
            AheadNotification::AgentAuthenticationRequired {
                session_id,
                methods,
            } => {
                self.agent_auth_methods
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .insert(session_id, methods);
            }
            AheadNotification::AgentAuthenticationCleared { session_id } => {
                self.agent_auth_methods
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&session_id);
            }
            _ => {}
        }
    }

    pub fn plan(&self, session_id: &str) -> Vec<AgentPlanEntry> {
        self.plans
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn tool_calls(&self, session_id: &str) -> Vec<AgentToolCall> {
        self.tool_calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn patch_previews(&self, session_id: &str) -> Vec<AgentPatchPreview> {
        self.patch_previews
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn thought(&self, session_id: &str) -> String {
        self.thoughts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn available_commands(&self, session_id: &str) -> Vec<AgentCommand> {
        self.available_commands
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn config_options(&self, session_id: &str) -> Vec<AgentConfigOption> {
        self.config_options
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn agent_auth_methods(&self, session_id: &str) -> Vec<AgentAuthMethod> {
        self.agent_auth_methods
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn usage(&self, session_id: &str) -> Option<AgentUsage> {
        self.usage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .cloned()
    }

    pub(crate) fn subscribe_usage_updates(&self) -> async_channel::Receiver<()> {
        let (sender, receiver) = async_channel::bounded(1);
        self.usage_update_subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(sender);
        receiver
    }

    fn notify_usage_updates(&self) {
        self.usage_update_subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|sender| match sender.try_send(()) {
                Ok(()) | Err(async_channel::TrySendError::Full(())) => true,
                Err(async_channel::TrySendError::Closed(())) => false,
            });
    }

    pub fn pending_user_input(
        &self,
        session_id: &str,
    ) -> Option<AgentUserInputRequest> {
        self.pending_user_inputs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .and_then(|requests| requests.first().cloned())
    }

    pub fn pending_user_input_count(&self, session_id: &str) -> usize {
        self.pending_user_inputs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .map_or(0, Vec::len)
    }

    fn forget_answered_user_input(&self, session_id: &str, request_id: &str) {
        let mut pending = self
            .pending_user_inputs
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(requests) = pending.get_mut(session_id) {
            requests.retain(|request| request.request_id != request_id);
            if requests.is_empty() {
                pending.remove(session_id);
            }
        }
    }

    pub fn take_buffer_snapshot_requests(&self) -> Vec<BufferSnapshotRequest> {
        std::mem::take(
            &mut *self
                .pending_buffer_snapshot_requests
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        )
    }

    pub fn answer_buffer_snapshot_request(
        &self,
        request: BufferSnapshotRequest,
        buffers: Vec<AgentBufferSnapshot>,
        error: Option<String>,
    ) -> Result<(), RpcError> {
        let reply = match error {
            Some(message) => AheadRequest::AgentBufferSnapshotsFailed {
                session_id: request.session_id,
                turn_id: request.turn_id,
                request_id: request.request_id,
                message,
            },
            None => AheadRequest::AgentBufferSnapshotsResponse {
                session_id: request.session_id,
                turn_id: request.turn_id,
                request_id: request.request_id,
                buffers,
            },
        };
        self.proxy_rpc.ahead_request_blocking(reply).map(|_| ())
    }

    pub fn take_editor_presentation_requests(
        &self,
    ) -> Vec<EditorPresentationRequest> {
        std::mem::take(
            &mut *self
                .pending_editor_presentation_requests
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        )
    }

    pub fn answer_editor_presentation_request(
        &self,
        request: EditorPresentationRequest,
        applied: bool,
        message: String,
    ) -> Result<(), RpcError> {
        self.proxy_rpc
            .ahead_request_blocking(AheadRequest::AgentPresentationResponse {
                session_id: request.session_id,
                turn_id: request.turn_id,
                request_id: request.request_id,
                applied,
                message,
            })
            .map(|_| ())
    }

    /// Reopens the most recently active durable session for this workspace, or starts
    /// one when the database has no active session yet.
    pub fn open_work_session(
        &self,
        title: &str,
        workspace: &str,
    ) -> Result<String, RpcError> {
        let proxy_running = self
            ._child
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(None)));
        if !proxy_running {
            return Err(RpcError {
                code: 0,
                message: "AHEAD proxy is not running or exited during startup"
                    .to_string(),
            });
        }
        let sessions = self.durable_sessions()?;
        if let Some(session) = sessions
            .into_iter()
            .filter(|session| {
                matches!(
                    session.session.lifecycle,
                    ahead_rpc::ahead::SessionLifecycle::Active
                )
            })
            .max_by(|left, right| {
                left.session.updated_at.cmp(&right.session.updated_at)
            })
        {
            return Ok(session.session.id);
        }

        let value =
            self.proxy_rpc
                .ahead_request_blocking(AheadRequest::StartWork {
                    work_kind: None,
                    title: title.to_string(),
                    starting_point: format!("Workspace: {workspace}"),
                    work_item: None,
                    harness: Some(HarnessKind::Ahead),
                    external_agent_id: None,
                    parent_session_id: None,
                })?;
        value
            .get("session")
            .and_then(|s| s.get("id"))
            .and_then(|id| id.as_str())
            .map(str::to_string)
            .ok_or_else(|| RpcError {
                code: 0,
                message: "AHEAD host returned no session id".to_string(),
            })
    }

    fn list_sessions_after_initialize(&self) -> Result<serde_json::Value, RpcError> {
        for attempt in 0..100 {
            match self
                .proxy_rpc
                .ahead_request_blocking(AheadRequest::ListSessions)
            {
                Ok(value) => return Ok(value),
                Err(error)
                    if attempt < 99 && error.message.contains("not initialized") =>
                {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("the session-list retry loop returns on its final attempt")
    }

    pub fn durable_sessions(&self) -> Result<Vec<DurableSessionSummary>, RpcError> {
        let value = self.list_sessions_after_initialize()?;
        let sessions = serde_json::from_value::<Vec<SessionListItem>>(value)
            .map_err(|error| RpcError {
                code: 0,
                message: format!("Invalid session list from AHEAD proxy: {error}"),
            })?;
        let adapters = self.external_acp_adapters().unwrap_or_else(|error| {
            eprintln!(
                "could not load external ACP agents for session labels: {}",
                error.message
            );
            Vec::new()
        });
        Ok(sessions
            .into_iter()
            .map(|session| {
                let (harness, external_agent_id) =
                    harness_metadata(session.backend.as_deref());
                let external_agent_name =
                    external_agent_id.as_ref().and_then(|id| {
                        adapters
                            .iter()
                            .find(|adapter| adapter.id == *id)
                            .map(|adapter| adapter.display_name.clone())
                    });
                DurableSessionSummary {
                    session,
                    harness,
                    external_agent_id,
                    external_agent_name,
                }
            })
            .collect())
    }

    pub fn recent_workspace_files(&self) -> Result<Vec<PathBuf>, RpcError> {
        let value = self
            .proxy_rpc
            .ahead_request_blocking(AheadRequest::ListRecentWorkspaceFiles)?;
        let paths =
            serde_json::from_value::<Vec<String>>(value).map_err(|error| {
                RpcError {
                    code: 0,
                    message: format!("Invalid recent workspace file list: {error}"),
                }
            })?;
        Ok(paths
            .into_iter()
            .map(|path| self.workspace.join(path))
            .collect())
    }

    pub fn record_recent_workspace_file(
        &self,
        path: &Path,
        opened_at: i64,
    ) -> Result<(), RpcError> {
        let workspace =
            std::fs::canonicalize(&self.workspace).map_err(|error| RpcError {
                code: 0,
                message: format!("Resolve AHEAD workspace for recent file: {error}"),
            })?;
        let path = std::fs::canonicalize(path).map_err(|error| RpcError {
            code: 0,
            message: format!("Resolve recent workspace file: {error}"),
        })?;
        let relative = path.strip_prefix(&workspace).map_err(|_| RpcError {
            code: 0,
            message: "Recent file is outside the AHEAD workspace".to_string(),
        })?;
        let relative = relative.to_str().ok_or_else(|| RpcError {
            code: 0,
            message: "Recent workspace file path is not valid UTF-8".to_string(),
        })?;
        let relative = relative.replace(std::path::MAIN_SEPARATOR, "/");
        self.proxy_rpc.ahead_request(
            AheadRequest::RecordRecentWorkspaceFile {
                path: relative,
                opened_at,
            },
            |result| {
                if let Err(error) = result {
                    eprintln!(
                        "AHEAD could not persist a recent workspace file: {}",
                        error.message
                    );
                }
            },
        );
        Ok(())
    }

    pub fn archive_session(&self, session_id: &str) -> Result<(), RpcError> {
        self.proxy_rpc
            .ahead_request_blocking(AheadRequest::ArchiveSession {
                session_id: session_id.to_string(),
            })?;
        Ok(())
    }

    pub fn durable_session_restore(
        &self,
    ) -> Result<DurableSessionRestore, RpcError> {
        let sessions = self.durable_sessions()?;
        let mut active = None;
        for summary in sessions.iter().filter(|summary| {
            matches!(
                summary.session.lifecycle,
                ahead_rpc::ahead::SessionLifecycle::Active
            )
        }) {
            let session_id = summary.session.id.clone();
            let Some(view) = self.session_view(&session_id) else {
                continue;
            };
            let conversation =
                self.conversation_page(&session_id, None, CONVERSATION_PAGE_SIZE)?;
            let runtime_state = self.agent_runtime_state(&session_id);
            let usage = runtime_state.as_ref().and_then(|state| state.usage.clone());
            if let Some(usage) = &usage {
                self.usage
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .entry(session_id.clone())
                    .or_insert_with(|| usage.clone());
            }
            active = Some(DurableSessionState {
                view,
                harness: summary.harness,
                external_agent_id: summary.external_agent_id.clone(),
                work_items: self.work_items(&session_id),
                conversation: conversation.messages,
                conversation_has_older: conversation.has_older,
                usage,
                harness_warning: self
                    .harness_warning(&session_id, runtime_state.as_ref()),
            });
            break;
        }
        Ok(DurableSessionRestore { sessions, active })
    }

    /// Starts a new durable AHEAD session without reopening an existing one.
    pub fn start_work_session(
        &self,
        title: &str,
        starting_point: &str,
        harness: HarnessKind,
        external_agent_id: Option<&str>,
        parent_session_id: Option<&str>,
    ) -> Result<String, RpcError> {
        let value =
            self.proxy_rpc
                .ahead_request_blocking(AheadRequest::StartWork {
                    work_kind: None,
                    title: title.to_string(),
                    starting_point: starting_point.to_string(),
                    work_item: None,
                    harness: Some(harness),
                    external_agent_id: external_agent_id.map(str::to_string),
                    parent_session_id: parent_session_id.map(str::to_string),
                })?;
        value
            .get("session")
            .and_then(|session| session.get("id"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| RpcError {
                code: 0,
                message: "AHEAD host returned no session id".into(),
            })
    }

    pub fn external_acp_adapters(
        &self,
    ) -> Result<Vec<ExternalAcpAdapter>, RpcError> {
        let value = self
            .proxy_rpc
            .ahead_request_blocking(AheadRequest::ListExternalAcpAdapters)?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: format!("Invalid ACP adapter list from AHEAD proxy: {error}"),
        })
    }

    pub fn set_external_acp_adapter_installed(
        &self,
        adapter_id: &str,
        installed: bool,
    ) -> Result<(), RpcError> {
        self.proxy_rpc
            .ahead_request_blocking(AheadRequest::SetExternalAcpAdapterInstalled {
                adapter_id: adapter_id.to_string(),
                installed,
            })
            .map(|_| ())
    }

    pub fn mcp_server_declarations(
        &self,
        workspace: &Path,
    ) -> Result<Vec<McpServerDeclaration>, RpcError> {
        let value = self.proxy_rpc.ahead_request_blocking(
            AheadRequest::ListMcpServerDeclarations {
                workspace: workspace.to_path_buf(),
            },
        )?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: format!(
                "Invalid MCP declaration list from AHEAD proxy: {error}"
            ),
        })
    }

    pub fn set_mcp_server_approval(
        &self,
        workspace: &Path,
        server_id: &str,
        expected_fingerprint: &str,
        action: McpServerApprovalAction,
    ) -> Result<(), RpcError> {
        self.proxy_rpc
            .ahead_request_blocking(AheadRequest::SetMcpServerApproval {
                workspace: workspace.to_path_buf(),
                server_id: server_id.to_string(),
                expected_fingerprint: expected_fingerprint.to_string(),
                action,
            })
            .map(|_| ())
    }

    pub fn session_view(&self, session_id: &str) -> Option<SessionView> {
        let value = self
            .proxy_rpc
            .ahead_request_blocking(AheadRequest::GetSession {
                session_id: session_id.to_string(),
            })
            .ok()?;
        serde_json::from_value(value).ok().flatten()
    }

    pub fn work_items(&self, session_id: &str) -> Vec<WorkItem> {
        let Ok(value) =
            self.proxy_rpc
                .ahead_request_blocking(AheadRequest::WorkItemList {
                    session_id: session_id.to_string(),
                })
        else {
            return Vec::new();
        };
        serde_json::from_value(value).unwrap_or_default()
    }

    pub fn advance_phase(
        &self,
        session_id: &str,
        expected_revision: u64,
        target_phase_id: &str,
    ) -> Result<SessionView, RpcError> {
        self.proxy_rpc
            .ahead_request_blocking(AheadRequest::AdvancePhase {
                session_id: session_id.to_string(),
                expected_revision,
                target_phase_id: target_phase_id.to_string(),
            })
            .and_then(|value| {
                serde_json::from_value(value).map_err(|error| RpcError {
                    code: 0,
                    message: error.to_string(),
                })
            })
    }

    pub fn set_work_item_status(
        &self,
        item_id: &str,
        status: WorkItemStatus,
    ) -> Result<WorkItem, RpcError> {
        self.proxy_rpc
            .ahead_request_blocking(AheadRequest::WorkItemSetStatus {
                item_id: item_id.to_string(),
                status,
            })
            .and_then(|value| {
                serde_json::from_value(value).map_err(|error| RpcError {
                    code: 0,
                    message: error.to_string(),
                })
            })
    }

    pub fn create_code_comment(
        &self,
        session_id: &str,
        path: String,
        range: ahead_rpc::ahead::DisplayRange,
        quote: String,
        source_sha256: String,
        body: String,
    ) -> Result<CodeComment, RpcError> {
        let value = self.proxy_rpc.ahead_request_blocking(
            AheadRequest::CreateCodeComment {
                session_id: session_id.to_string(),
                path,
                range,
                quote,
                source_sha256,
                body,
            },
        )?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: error.to_string(),
        })
    }

    pub(crate) fn record_prediction_acceptance(
        &self,
        session_id: String,
        path: String,
        range: ahead_rpc::ahead::DisplayRange,
        binding: ahead_rpc::ahead::PredictionBinding,
        model_context: Option<ahead_rpc::ahead::PredictionModelContext>,
        accepted_text_sha256: String,
    ) -> async_channel::Receiver<Result<(), RpcError>> {
        let (sender, receiver) = async_channel::bounded(1);
        self.proxy_rpc.ahead_request(
            AheadRequest::RecordPredictionAcceptance {
                session_id,
                path,
                range,
                binding,
                model_context,
                accepted_text_sha256,
            },
            move |result| {
                if let Err(error) = sender.try_send(result.map(|_| ()))
                    && !matches!(error, async_channel::TrySendError::Closed(_))
                {
                    eprintln!(
                        "Delivering accepted prediction record result: {error}"
                    );
                }
            },
        );
        receiver
    }

    pub fn prediction_acceptances(
        &self,
        session_id: &str,
    ) -> Result<Vec<ahead_rpc::ahead::PredictionAcceptance>, RpcError> {
        let value = self.proxy_rpc.ahead_request_blocking(
            AheadRequest::ListPredictionAcceptances {
                session_id: session_id.to_string(),
            },
        )?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: error.to_string(),
        })
    }

    pub fn code_comments(
        &self,
        session_id: &str,
    ) -> Result<Vec<CodeComment>, RpcError> {
        let value = self.proxy_rpc.ahead_request_blocking(
            AheadRequest::ListCodeComments {
                session_id: session_id.to_string(),
            },
        )?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: error.to_string(),
        })
    }

    pub fn resolve_code_comment(
        &self,
        session_id: &str,
        comment_id: &str,
    ) -> Result<CodeComment, RpcError> {
        let value = self.proxy_rpc.ahead_request_blocking(
            AheadRequest::ResolveCodeComment {
                session_id: session_id.to_string(),
                comment_id: comment_id.to_string(),
            },
        )?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: error.to_string(),
        })
    }

    /// Fetches one durable page and merges it into the loaded chat window.
    pub fn conversation_page(
        &self,
        session_id: &str,
        before: Option<ConversationMessageCursor>,
        limit: usize,
    ) -> Result<ConversationMessagePage, RpcError> {
        let is_older_page = before.is_some();
        let revision = self
            .conversations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(session_id)
            .map_or(0, |conversation| conversation.revision);
        let value = self.proxy_rpc.ahead_request_blocking(
            AheadRequest::ConversationMessagesPage {
                session_id: session_id.to_string(),
                before,
                limit,
            },
        )?;
        let page = serde_json::from_value::<ConversationMessagePage>(value)
            .map_err(|error| RpcError {
                code: 0,
                message: error.to_string(),
            })?;
        Ok(self.merge_conversation_page(session_id, is_older_page, revision, page))
    }

    fn merge_conversation_page(
        &self,
        session_id: &str,
        is_older_page: bool,
        revision: u64,
        page: ConversationMessagePage,
    ) -> ConversationMessagePage {
        let mut conversations = self
            .conversations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let conversation = conversations.entry(session_id.to_string()).or_default();
        if is_older_page || !conversation.loaded_page {
            conversation.has_older = page.has_older;
        }
        conversation.loaded_page = true;
        for message in page.messages {
            if let Some(existing) = conversation
                .messages
                .iter_mut()
                .find(|existing| existing.id == message.id)
            {
                if conversation.revision == revision {
                    *existing = message;
                }
            } else {
                conversation.messages.push(message);
            }
        }
        conversation.messages.sort_by(|left, right| {
            (left.sequence, &left.id).cmp(&(right.sequence, &right.id))
        });
        ConversationMessagePage {
            messages: conversation.messages.clone(),
            has_older: conversation.has_older,
        }
    }

    pub fn cached_conversation(
        &self,
        session_id: &str,
    ) -> Option<Vec<ConversationMessage>> {
        self.conversations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(session_id)
            .map(|conversation| conversation.messages.clone())
    }

    pub(crate) fn cached_conversation_page(
        &self,
        session_id: &str,
    ) -> Option<ConversationMessagePage> {
        self.conversations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(session_id)
            .map(|conversation| ConversationMessagePage {
                messages: conversation.messages.clone(),
                has_older: conversation.has_older,
            })
    }

    async fn ahead_response<T: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        request: AheadRequest,
    ) -> Result<T, RpcError> {
        let (sender, receiver) = async_channel::bounded(1);
        self.proxy_rpc.ahead_request(request, move |response| {
            let result = response.and_then(|value| {
                serde_json::from_value(value).map_err(|error| RpcError {
                    code: 0,
                    message: format!("Invalid agent response: {error}"),
                })
            });
            if let Err(async_channel::TrySendError::Full(_)) =
                sender.try_send(result)
            {
                eprintln!("Duplicate agent response");
            }
        });
        receiver.recv().await.map_err(|error| RpcError {
            code: 0,
            message: format!("Agent response disconnected: {error}"),
        })?
    }

    pub(crate) async fn conversation_state(
        &self,
        session_id: &str,
    ) -> Result<ConversationState, RpcError> {
        let revision = self
            .conversations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(session_id)
            .map_or(0, |conversation| conversation.revision);
        let page = self
            .ahead_response(AheadRequest::ConversationMessagesPage {
                session_id: session_id.into(),
                before: None,
                limit: CONVERSATION_PAGE_SIZE,
            })
            .await?;
        self.merge_conversation_page(session_id, false, revision, page);
        let runtime: Option<AgentRuntimeState> = self
            .ahead_response(AheadRequest::AgentRuntimeState {
                session_id: session_id.into(),
            })
            .await?;
        if let Some(runtime) = &runtime {
            if let Some(usage) = &runtime.usage {
                self.usage
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .entry(session_id.into())
                    .or_insert_with(|| usage.clone());
            }
            let streaming = self
                .streaming_turns
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let latest_turn = streaming.get(session_id).cloned().or_else(|| {
                self.cached_conversation(session_id)?
                    .iter()
                    .rev()
                    .find(|message| message.role == "agent")
                    .map(|message| message.turn_id.clone())
            });
            if runtime.turn_id == latest_turn {
                // Notifications received during the durable read are newer than this snapshot.
                self.plans
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .entry(session_id.into())
                    .or_insert_with(|| runtime.plan.clone());
                self.tool_calls
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .entry(session_id.into())
                    .or_insert_with(|| runtime.tool_calls.clone());
                self.available_commands
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .entry(session_id.into())
                    .or_insert_with(|| runtime.commands.clone());
            }
        }
        let backend: serde_json::Value = self
            .ahead_response(AheadRequest::HarnessStatus {
                session_id: session_id.into(),
            })
            .await?;
        let warning = Self::harness_warning_for_backend(
            backend.get("backend").and_then(serde_json::Value::as_str),
            runtime.as_ref(),
        );
        let view = self
            .ahead_response(AheadRequest::GetSession {
                session_id: session_id.into(),
            })
            .await?;
        Ok(ConversationState { warning, view })
    }

    pub fn agent_runtime_state(
        &self,
        session_id: &str,
    ) -> Option<AgentRuntimeState> {
        let value = self
            .proxy_rpc
            .ahead_request_blocking(AheadRequest::AgentRuntimeState {
                session_id: session_id.to_string(),
            })
            .ok()?;
        serde_json::from_value::<Option<AgentRuntimeState>>(value)
            .ok()
            .flatten()
    }

    pub fn agent_skills(
        &self,
        session_id: &str,
        model: Option<String>,
        model_provider: Option<String>,
    ) -> Result<ahead_rpc::ahead::AgentSkillCatalog, RpcError> {
        let value =
            self.proxy_rpc
                .ahead_request_blocking(AheadRequest::AgentSkills {
                    session_id: session_id.to_string(),
                    model,
                    model_provider,
                })?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: format!("Invalid managed-agent skill catalog: {error}"),
        })
    }

    pub fn agent_reasoning_options(
        &self,
        session_id: &str,
        model: String,
        model_provider: Option<String>,
    ) -> Result<ahead_rpc::ahead::AgentReasoningOptions, RpcError> {
        let value = self.proxy_rpc.ahead_request_blocking(
            AheadRequest::AgentReasoningOptions {
                session_id: session_id.to_string(),
                model,
                model_provider,
            },
        )?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: format!("Invalid managed-agent reasoning options: {error}"),
        })
    }

    /// Exports a readable, Git-distributable session checkpoint. Runtime
    /// databases and harness state stay outside the shared directory.
    pub fn export_checkpoint(&self, session_id: &str) -> Result<PathBuf, RpcError> {
        let bundle = self.session_export(session_id)?;
        if std::path::Path::new(session_id).components().count() != 1
            || !matches!(
                std::path::Path::new(session_id).components().next(),
                Some(std::path::Component::Normal(_))
            )
        {
            return Err(RpcError {
                code: 0,
                message: "Invalid session checkpoint id".into(),
            });
        }
        let directory = self
            .workspace
            .join(".ahead")
            .join("sessions")
            .join(session_id);
        std::fs::create_dir_all(&directory).map_err(|error| RpcError {
            code: 0,
            message: format!("Create shared session directory: {error}"),
        })?;
        let session_json =
            serde_json::to_vec_pretty(&bundle).map_err(|error| RpcError {
                code: 0,
                message: format!("Serialize session export: {error}"),
            })?;
        write_atomic(&directory.join("session.json"), &session_json)?;

        let mut session_md = format!(
            "# {}\n\n- Session ID: `{}`\n- Phase: {}\n- Exported: {}\n\n",
            bundle.session.title,
            bundle.session.id,
            bundle.workflow.phase.title,
            bundle.exported_at,
        );
        if !bundle.work_items.is_empty() {
            session_md.push_str("## Work items\n\n");
            for item in &bundle.work_items {
                session_md.push_str(&format!(
                    "- [{}] {}\n",
                    if matches!(item.status, ahead_rpc::ahead::WorkItemStatus::Done)
                    {
                        "x"
                    } else {
                        " "
                    },
                    item.title
                ));
            }
            session_md.push('\n');
        }
        if !bundle.conversation_summaries.is_empty() {
            session_md.push_str("## Conversation summaries\n\n");
            for summary in &bundle.conversation_summaries {
                session_md.push_str(&format!(
                    "### {}\n\n{}\n\n",
                    summary.phase, summary.summary_markdown
                ));
            }
        }
        write_atomic(&directory.join("session.md"), session_md.as_bytes())?;

        let mut conversation_jsonl = String::new();
        for message in &bundle.conversation_messages {
            let line = serde_json::to_string(message).map_err(|error| RpcError {
                code: 0,
                message: format!("Serialize conversation message: {error}"),
            })?;
            conversation_jsonl.push_str(&line);
            conversation_jsonl.push('\n');
        }
        write_atomic(
            &directory.join("conversation.jsonl"),
            conversation_jsonl.as_bytes(),
        )?;
        if !bundle.code_comments.is_empty() {
            let mut review = String::from("# Code comments\n\n");
            for comment in &bundle.code_comments {
                review.push_str(&format!(
                    "## {}:{} · {}\n\n{}\n\n```\n{}\n```\n\nSource revision: `{}`\n\n",
                    comment.path,
                    comment.range.start.line + 1,
                    comment.actor_id,
                    comment.body,
                    comment.quote,
                    comment.source_sha256,
                ));
            }
            write_atomic(&directory.join("code-comments.md"), review.as_bytes())?;
        }
        Ok(directory.join("session.json"))
    }

    fn session_export(
        &self,
        session_id: &str,
    ) -> Result<SessionExportBundle, RpcError> {
        let value =
            self.proxy_rpc
                .ahead_request_blocking(AheadRequest::SessionExport {
                    session_id: session_id.to_string(),
                })?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: format!("Invalid session export: {error}"),
        })
    }

    /// Lists shared checkpoints without opening a harness or changing local
    /// runtime state. Legacy JSON checkpoint files are accepted for discovery.
    pub fn checkpoints(&self) -> Vec<CheckpointSummary> {
        let shared = self.workspace.join(".ahead").join("sessions");
        let legacy = self.workspace.join(".ahead").join("checkpoints");
        let mut paths = Vec::new();
        if let Ok(entries) = std::fs::read_dir(shared) {
            for entry in entries.flatten() {
                let path = entry.path().join("session.json");
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        if let Ok(entries) = std::fs::read_dir(legacy) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                    paths.push(path);
                }
            }
        }
        let mut checkpoints = paths
            .into_iter()
            .filter_map(|path| {
                let payload = std::fs::read_to_string(&path).ok()?;
                let bundle =
                    serde_json::from_str::<SessionExportBundle>(&payload).ok()?;
                Some(CheckpointSummary {
                    session_id: bundle.session.id,
                    title: bundle.session.title,
                    phase: bundle.workflow.phase.title,
                    exported_at: bundle.exported_at,
                    path,
                })
            })
            .collect::<Vec<_>>();
        checkpoints.sort_by(|left, right| right.exported_at.cmp(&left.exported_at));
        checkpoints
    }

    /// Imports one checkpoint into the local store. This only restores durable
    /// readable history; it never starts or resumes the original harness.
    pub fn restore_checkpoint(
        &self,
        path: &PathBuf,
    ) -> Result<SessionView, RpcError> {
        let payload = std::fs::read_to_string(path).map_err(|error| RpcError {
            code: 0,
            message: format!("Read session checkpoint: {error}"),
        })?;
        let bundle = serde_json::from_str::<SessionExportBundle>(&payload).map_err(
            |error| RpcError {
                code: 0,
                message: format!("Invalid session checkpoint: {error}"),
            },
        )?;
        self.proxy_rpc
            .ahead_request_blocking(AheadRequest::SessionRestore { bundle })
            .and_then(|value| {
                serde_json::from_value(value).map_err(|error| RpcError {
                    code: 0,
                    message: format!("Invalid restored session: {error}"),
                })
            })
    }

    pub fn harness_warning(
        &self,
        session_id: &str,
        runtime_state: Option<&AgentRuntimeState>,
    ) -> Option<String> {
        Self::harness_warning_for_backend(
            self.harness_backend(session_id).as_deref(),
            runtime_state,
        )
    }

    fn harness_warning_for_backend(
        backend: Option<&str>,
        runtime_state: Option<&AgentRuntimeState>,
    ) -> Option<String> {
        let mut warnings = Vec::new();
        if let Some(backend) = backend {
            if backend.ends_with("-fresh") {
                warnings.push("The persisted agent thread was unavailable; a fresh thread is active.".to_string());
            } else if backend.starts_with("external-agent") {
                warnings.push("External ACP: AHEAD does not mediate shell/file effects or enforce teaching read-only, path scopes, or CodeAnchor attribution.".to_string());
            }
        }
        if let Some(state) = runtime_state {
            warnings.extend(state.warnings.iter().cloned());
        }
        (!warnings.is_empty()).then(|| warnings.join("\n"))
    }

    pub fn harness_backend(&self, session_id: &str) -> Option<String> {
        let value = self
            .proxy_rpc
            .ahead_request_blocking(AheadRequest::HarnessStatus {
                session_id: session_id.to_string(),
            })
            .ok()?;
        value
            .get("backend")
            .and_then(|backend| backend.as_str())
            .map(str::to_string)
    }

    pub fn harness_kind(&self, session_id: &str) -> HarnessKind {
        harness_metadata(self.harness_backend(session_id).as_deref()).0
    }

    pub fn external_agent_id(&self, session_id: &str) -> Option<String> {
        harness_metadata(self.harness_backend(session_id).as_deref()).1
    }

    /// Searches the current project's and user's AHEAD memory files.
    pub fn search_memory(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryExcerpt>, RpcError> {
        let value =
            self.proxy_rpc
                .ahead_request_blocking(AheadRequest::SearchMemory {
                    query: query.to_string(),
                    limit,
                })?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: format!("Invalid AHEAD memory search result: {error}"),
        })
    }

    /// Reads the current bounded memory document and its replacement token.
    pub fn read_memory(
        &self,
        scope: MemoryScope,
    ) -> Result<MemoryDocument, RpcError> {
        let value = self
            .proxy_rpc
            .ahead_request_blocking(AheadRequest::ReadMemory { scope })?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: format!("Invalid AHEAD memory document: {error}"),
        })
    }

    /// Appends explicitly selected content to one AHEAD memory source.
    pub fn write_memory(
        &self,
        scope: MemoryScope,
        message_id: &str,
        content: &str,
    ) -> Result<MemoryWriteResult, RpcError> {
        let value =
            self.proxy_rpc
                .ahead_request_blocking(AheadRequest::WriteMemory {
                    scope,
                    message_id: message_id.to_string(),
                    content: content.to_string(),
                })?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: format!("Invalid AHEAD memory write result: {error}"),
        })
    }

    /// Applies a reviewed replacement only while the original snapshot remains current.
    pub fn replace_memory(
        &self,
        scope: MemoryScope,
        expected_sha256: &str,
        content: &str,
    ) -> Result<MemoryWriteResult, RpcError> {
        let value =
            self.proxy_rpc
                .ahead_request_blocking(AheadRequest::ReplaceMemory {
                    scope,
                    expected_sha256: expected_sha256.to_string(),
                    content: content.to_string(),
                })?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: format!("Invalid AHEAD memory replacement result: {error}"),
        })
    }

    /// Starts a real streamed harness turn. Deltas arrive asynchronously via
    /// `route_ahead`; this returns once the turn has been accepted.
    pub async fn agent_turn(
        self: &Arc<Self>,
        request: AgentTurnRequestDto,
    ) -> Result<String, RpcError> {
        let value: serde_json::Value = self
            .ahead_response(AheadRequest::AgentTurnStart { request })
            .await?;
        value
            .get("turn_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| RpcError {
                code: 0,
                message: "AgentTurnStart returned no turn_id".to_string(),
            })
    }

    pub fn post_human_message(
        &self,
        session_id: String,
        content: String,
    ) -> Result<ConversationMessage, RpcError> {
        let value = self.proxy_rpc.ahead_request_blocking(
            AheadRequest::PostHumanMessage {
                session_id,
                content,
            },
        )?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: format!("Invalid human message response: {error}"),
        })
    }

    pub fn record_voice_transcript(
        &self,
        session_id: String,
        call_id: String,
        segment_id: String,
        speaker: VoiceTranscriptSpeaker,
        start_ms: u64,
        end_ms: u64,
        content: String,
    ) -> Result<ConversationMessage, RpcError> {
        let value = self.proxy_rpc.ahead_request_blocking(
            AheadRequest::RecordVoiceTranscript {
                session_id,
                call_id,
                segment_id,
                speaker,
                start_ms,
                end_ms,
                content,
            },
        )?;
        let message: ConversationMessage =
            serde_json::from_value(value).map_err(|error| RpcError {
                code: 0,
                message: format!("Invalid voice transcript response: {error}"),
            })?;
        self.route_ahead(AheadNotification::ConversationMessageAdded {
            message: message.clone(),
        });
        Ok(message)
    }

    pub fn start_voice_call(
        &self,
        session_id: String,
    ) -> Result<ahead_rpc::ahead::VoiceCallLease, RpcError> {
        let value = self
            .proxy_rpc
            .ahead_request_blocking(AheadRequest::StartVoiceCall { session_id })?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: format!("Invalid voice call lease from AHEAD proxy: {error}"),
        })
    }

    pub fn end_voice_call(
        &self,
        lease: ahead_rpc::ahead::VoiceCallLease,
    ) -> Result<bool, RpcError> {
        let value = self
            .proxy_rpc
            .ahead_request_blocking(AheadRequest::EndVoiceCall { lease })?;
        value
            .get("ended")
            .and_then(serde_json::Value::as_bool)
            .ok_or_else(|| RpcError {
                code: 0,
                message: "Invalid voice call end response from AHEAD proxy".into(),
            })
    }

    pub fn release_voice_call(&self, lease: ahead_rpc::ahead::VoiceCallLease) {
        self.proxy_rpc.ahead_request(
            AheadRequest::EndVoiceCall { lease },
            |result| {
                if let Err(error) = result {
                    eprintln!(
                        "Ending the AHEAD voice call lease: {}",
                        error.message
                    );
                }
            },
        );
    }

    pub fn share_session(
        &self,
        session_id: String,
        bind_address: String,
    ) -> Result<SharedSessionOffer, RpcError> {
        let value =
            self.proxy_rpc
                .ahead_request_blocking(AheadRequest::ShareSession {
                    session_id,
                    bind_address,
                })?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: format!("Invalid share offer: {error}"),
        })
    }

    pub fn workspace_participants(
        &self,
    ) -> Result<Vec<SessionParticipantRecord>, RpcError> {
        let value = self
            .proxy_rpc
            .ahead_request_blocking(AheadRequest::GetWorkspaceParticipants)?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: format!("Invalid team members: {error}"),
        })
    }

    pub fn stop_sharing_session(&self, session_id: String) -> Result<(), RpcError> {
        self.proxy_rpc
            .ahead_request_blocking(AheadRequest::StopSharingSession {
                session_id: session_id.clone(),
            })?;
        self.shared_buffer_changes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|(id, _), _| id != &session_id);
        self.shared_buffers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|(id, _), _| id != &session_id);
        Ok(())
    }

    pub fn publish_shared_terminal(
        &self,
        session_id: String,
        content: String,
    ) -> Result<(), RpcError> {
        self.proxy_rpc.ahead_request_blocking(
            AheadRequest::PublishSharedTerminal {
                session_id,
                content,
            },
        )?;
        Ok(())
    }

    pub fn publish_shared_presence(
        &self,
        session_id: String,
        path: Option<String>,
        line: Option<u32>,
    ) -> Result<(), RpcError> {
        self.proxy_rpc.ahead_request_blocking(
            AheadRequest::PublishSharedPresence {
                session_id,
                path,
                line,
            },
        )?;
        Ok(())
    }

    pub fn shared_presence(
        &self,
        session_id: String,
    ) -> Result<Vec<ahead_rpc::ahead::SharedPresence>, RpcError> {
        let value = self.proxy_rpc.ahead_request_blocking(
            AheadRequest::GetSharedPresence { session_id },
        )?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: format!("Invalid shared presence: {error}"),
        })
    }

    pub fn read_shared_buffer(
        &self,
        session_id: String,
        path: String,
    ) -> Result<SharedBufferSnapshot, RpcError> {
        let value = self.proxy_rpc.ahead_request_blocking(
            AheadRequest::ReadSharedBuffer {
                session_id: session_id.clone(),
                path,
            },
        )?;
        let snapshot: SharedBufferSnapshot =
            serde_json::from_value(value).map_err(|error| RpcError {
                code: 0,
                message: format!("Invalid shared buffer: {error}"),
            })?;
        self.shared_buffers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert((session_id, snapshot.path.clone()), snapshot.clone());
        Ok(snapshot)
    }

    pub fn replace_shared_buffer(
        &self,
        session_id: String,
        path: String,
        expected_revision: u64,
        content: String,
    ) -> Result<SharedBufferEditResult, RpcError> {
        let value = self.proxy_rpc.ahead_request_blocking(
            AheadRequest::ReplaceSharedBuffer {
                session_id: session_id.clone(),
                path,
                expected_revision,
                content,
            },
        )?;
        let edit: SharedBufferEditResult =
            serde_json::from_value(value).map_err(|error| RpcError {
                code: 0,
                message: format!("Invalid shared edit: {error}"),
            })?;
        self.shared_buffers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(
                (session_id, edit.snapshot.path.clone()),
                edit.snapshot.clone(),
            );
        Ok(edit)
    }

    pub fn cached_shared_buffer(
        &self,
        session_id: &str,
        path: &str,
    ) -> Option<SharedBufferSnapshot> {
        self.shared_buffers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&(session_id.to_string(), path.to_string()))
            .cloned()
    }

    pub fn add_session_participant(
        &self,
        session_id: String,
        user_handle: String,
        role: ahead_rpc::ahead::SessionRole,
    ) -> Result<SessionView, RpcError> {
        let value = self.proxy_rpc.ahead_request_blocking(
            AheadRequest::AddSessionParticipant {
                session_id,
                user_handle,
                role,
            },
        )?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: format!("Invalid updated session: {error}"),
        })
    }

    pub fn revoke_session_participant(
        &self,
        session_id: String,
        user_handle: String,
    ) -> Result<SessionView, RpcError> {
        let value = self.proxy_rpc.ahead_request_blocking(
            AheadRequest::RevokeSessionParticipant {
                session_id,
                user_handle,
            },
        )?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: format!("Invalid updated session: {error}"),
        })
    }

    pub fn join_shared_session(
        &self,
        offer: SharedSessionOffer,
    ) -> Result<SharedSessionUpdate, RpcError> {
        let value = self
            .proxy_rpc
            .ahead_request_blocking(AheadRequest::JoinSharedSession { offer })?;
        let update: SharedSessionUpdate =
            serde_json::from_value(value).map_err(|error| RpcError {
                code: 0,
                message: format!("Invalid joined session: {error}"),
            })?;
        for message in &update.messages {
            self.route_ahead(AheadNotification::ConversationMessageAdded {
                message: message.clone(),
            });
        }
        Ok(update)
    }

    pub fn poll_shared_session(
        &self,
        session_id: String,
        after_sequence: i64,
        active_path: Option<String>,
        active_line: Option<u32>,
    ) -> Result<SharedSessionUpdate, RpcError> {
        let value = self.proxy_rpc.ahead_request_blocking(
            AheadRequest::PollSharedSession {
                session_id,
                after_sequence,
                active_path,
                active_line,
            },
        )?;
        let update: SharedSessionUpdate =
            serde_json::from_value(value).map_err(|error| RpcError {
                code: 0,
                message: format!("Invalid shared session update: {error}"),
            })?;
        for message in &update.messages {
            self.route_ahead(AheadNotification::ConversationMessageAdded {
                message: message.clone(),
            });
        }
        Ok(update)
    }

    pub fn post_shared_human_message(
        &self,
        session_id: String,
        content: String,
    ) -> Result<ConversationMessage, RpcError> {
        let value = self.proxy_rpc.ahead_request_blocking(
            AheadRequest::PostSharedHumanMessage {
                session_id,
                content,
            },
        )?;
        let message: ConversationMessage =
            serde_json::from_value(value).map_err(|error| RpcError {
                code: 0,
                message: format!("Invalid shared message: {error}"),
            })?;
        self.route_ahead(AheadNotification::ConversationMessageAdded {
            message: message.clone(),
        });
        Ok(message)
    }

    pub fn start_shared_agent_turn(
        &self,
        session_id: String,
        content: String,
    ) -> Result<String, RpcError> {
        let value = self.proxy_rpc.ahead_request_blocking(
            AheadRequest::StartSharedAgentTurn {
                session_id,
                content,
            },
        )?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: format!("Invalid shared turn response: {error}"),
        })
    }

    pub fn leave_shared_session(&self, session_id: String) -> Result<(), RpcError> {
        self.proxy_rpc
            .ahead_request_blocking(AheadRequest::LeaveSharedSession {
                session_id,
            })?;
        Ok(())
    }

    /// Cancels the in-flight harness turn for a session.
    pub fn agent_cancel(
        &self,
        session_id: &str,
    ) -> async_channel::Receiver<Result<bool, RpcError>> {
        let (sender, receiver) = async_channel::bounded(1);
        self.proxy_rpc.ahead_request(
            AheadRequest::AgentTurnCancel {
                session_id: session_id.to_string(),
            },
            move |result| {
                let result = result.and_then(|value| {
                    value
                        .get("cancelled")
                        .and_then(serde_json::Value::as_bool)
                        .ok_or_else(|| RpcError {
                            code: 0,
                            message: "Invalid turn cancellation response".into(),
                        })
                });
                if let Err(async_channel::TrySendError::Full(_)) =
                    sender.try_send(result)
                {
                    eprintln!("Duplicate turn cancellation response");
                }
            },
        );
        receiver
    }

    pub fn set_agent_config_option(
        &self,
        session_id: &str,
        config_id: &str,
        value: &AgentConfigOptionValue,
    ) -> Result<(), RpcError> {
        self.proxy_rpc
            .ahead_request_blocking(AheadRequest::AgentConfigOptionSet {
                session_id: session_id.to_string(),
                config_id: config_id.to_string(),
                value: value.clone(),
            })
            .map(|_| ())
    }

    pub fn prepare_external_agent(&self, session_id: &str) -> Result<(), RpcError> {
        self.proxy_rpc
            .ahead_request_blocking(AheadRequest::AgentSessionPrepare {
                session_id: session_id.to_string(),
            })
            .map(|_| ())
    }

    pub fn authenticate_external_agent(
        &self,
        session_id: &str,
        method_id: &str,
    ) -> Result<(), RpcError> {
        self.proxy_rpc
            .ahead_request_blocking(AheadRequest::AgentAuthenticate {
                session_id: session_id.to_string(),
                method_id: method_id.to_string(),
            })
            .map(|_| ())
    }

    pub fn external_agent_terminal_auth_launch(
        &self,
        session_id: &str,
        method_id: &str,
    ) -> Result<AgentTerminalAuthLaunch, RpcError> {
        let value = self.proxy_rpc.ahead_request_blocking(
            AheadRequest::AgentTerminalAuthLaunch {
                session_id: session_id.to_string(),
                method_id: method_id.to_string(),
            },
        )?;
        serde_json::from_value(value).map_err(|error| RpcError {
            code: 0,
            message: format!("Invalid terminal authentication launch: {error}"),
        })
    }

    pub fn complete_external_agent_terminal_auth(
        &self,
        session_id: &str,
        succeeded: bool,
    ) -> Result<(), RpcError> {
        self.proxy_rpc
            .ahead_request_blocking(AheadRequest::AgentTerminalAuthComplete {
                session_id: session_id.to_string(),
                succeeded,
            })
            .map(|_| ())
    }

    /// Retries a durable failed/cancelled turn without silently replaying it
    /// after a process restart.
    pub async fn agent_retry(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<String, RpcError> {
        let value: serde_json::Value = self
            .ahead_response(AheadRequest::AgentTurnRetry {
                session_id: session_id.to_string(),
                turn_id: turn_id.to_string(),
            })
            .await?;
        value
            .get("turn_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| RpcError {
                code: 0,
                message: "AgentTurnRetry returned no turn_id".to_string(),
            })
    }

    pub fn answer_agent_user_input(
        &self,
        session_id: &str,
        request_id: &str,
        answers: HashMap<String, Vec<String>>,
    ) -> Result<(), RpcError> {
        self.proxy_rpc
            .ahead_request_blocking(AheadRequest::AgentUserInputAnswer {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                answers,
            })
            .map(|_| self.forget_answered_user_input(session_id, request_id))
    }

    pub fn is_streaming(&self, session_id: &str) -> bool {
        self.streaming_turns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(session_id)
    }

    pub fn active_turn_id(&self, session_id: &str) -> Option<String> {
        self.streaming_turns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .cloned()
    }

    /// Route one proxy-originated request-style response into pending slots.
    pub fn resolve_proxy_response(&self, resp: ProxyResponse) {
        match resp {
            ProxyResponse::HoverResponse { request_id, hover } => {
                if let Some(tx) = self
                    .pending_hover
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&request_id)
                {
                    let _ = tx.send(hover);
                }
            }
            ProxyResponse::GetDefinitionResponse {
                request_id,
                definition,
            } => {
                if let Some(tx) = self
                    .pending_defs
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&request_id)
                {
                    let locs = match definition {
                        GotoDefinitionResponse::Scalar(loc) => vec![loc],
                        GotoDefinitionResponse::Array(locs) => locs,
                        GotoDefinitionResponse::Link(links) => links
                            .into_iter()
                            .map(|l| Location {
                                uri: l.target_uri,
                                range: l.target_selection_range,
                            })
                            .collect(),
                    };
                    let _ = tx.send(locs);
                }
            }
            _ => {}
        }
    }

    fn next_id(&self) -> usize {
        self.request_seq.fetch_add(1, Ordering::Relaxed)
    }

    /// Fire a completion notification; the proxy answers via
    /// `CoreNotification::CompletionResponse` on the stdio pump.
    pub fn request_completions(
        self: &Arc<Self>,
        path: PathBuf,
        position: Position,
        input: String,
    ) -> (usize, async_channel::Receiver<Option<LspCompletionBatch>>) {
        let (tx, rx) = async_channel::unbounded();
        let id = self.next_id();
        self.pending_completion
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx);
        self.proxy_rpc.completion(id, path, input, position);
        (id, rx)
    }

    pub(crate) fn cancel_completion_request(&self, request_id: usize) {
        let was_pending = self
            .pending_completion
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&request_id)
            .is_some();
        if was_pending {
            self.proxy_rpc.cancel_completion(request_id);
        }
    }

    pub fn request_signature_help(
        self: &Arc<Self>,
        path: PathBuf,
        position: Position,
        context: SignatureHelpContext,
    ) -> (usize, async_channel::Receiver<Option<SignatureHelp>>) {
        let (sender, receiver) = async_channel::unbounded();
        let request_id = self.next_id();
        self.pending_signature_help
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(request_id, sender);
        self.proxy_rpc
            .signature_help(request_id, path, position, context);
        (request_id, receiver)
    }

    pub(crate) fn cancel_signature_help_request(&self, request_id: usize) {
        self.pending_signature_help
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&request_id);
    }

    pub fn resolve_completion(
        &self,
        completion: LspCompletion,
    ) -> async_channel::Receiver<Result<CompletionItem, RpcError>> {
        let (sender, receiver) = async_channel::bounded(1);
        self.proxy_rpc.completion_resolve(
            completion.plugin_id,
            completion.item,
            move |result| {
                let result = result.and_then(|response| match response {
                    ProxyResponse::CompletionResolveResponse { item } => Ok(*item),
                    _ => Err(RpcError {
                        code: 0,
                        message: "Unexpected completion resolve response".into(),
                    }),
                });
                if let Err(error) = sender.try_send(result) {
                    eprintln!("Delivering resolved completion: {error}");
                }
            },
        );
        receiver
    }

    pub fn request_code_actions(
        &self,
        path: PathBuf,
        position: Position,
        diagnostics: Vec<Diagnostic>,
    ) -> Receiver<Result<Vec<LspCodeAction>, String>> {
        let (sender, receiver) = unbounded();
        self.proxy_rpc.get_code_actions(
            path,
            position,
            diagnostics,
            move |result| {
                let result = match result {
                    Ok(ProxyResponse::GetCodeActionsResponse { actions }) => {
                        Ok(actions)
                    }
                    Ok(_) => {
                        Err("Unexpected response to code-action request".into())
                    }
                    Err(error) => Err(error.message),
                };
                if let Err(error) = sender.send(result) {
                    eprintln!("Delivering code-action response: {error}");
                }
            },
        );
        receiver
    }

    pub fn resolve_code_action(
        &self,
        action: CodeAction,
        plugin_id: PluginId,
    ) -> Receiver<Result<CodeAction, RpcError>> {
        let (sender, receiver) = unbounded();
        self.proxy_rpc
            .code_action_resolve(action, plugin_id, move |result| {
                let result = result.and_then(|response| match response {
                    ProxyResponse::CodeActionResolveResponse { item } => Ok(*item),
                    _ => Err(RpcError {
                        code: 0,
                        message: "Unexpected code-action resolve response".into(),
                    }),
                });
                if let Err(error) = sender.send(result) {
                    eprintln!("Delivering resolved code action: {error}");
                }
            });
        receiver
    }

    pub fn execute_lsp_command(
        &self,
        command: lsp_types::Command,
        plugin_id: PluginId,
    ) -> async_channel::Receiver<Result<(), RpcError>> {
        let (sender, receiver) = async_channel::bounded(1);
        self.proxy_rpc.execute_lsp_command(
            plugin_id,
            command.command,
            command.arguments.unwrap_or_default(),
            move |result| {
                let result = result.and_then(|response| match response {
                    ProxyResponse::ExecuteLspCommandResponse {} => Ok(()),
                    _ => Err(RpcError {
                        code: 0,
                        message: "Unexpected LSP command response".into(),
                    }),
                });
                if let Err(error) = sender.try_send(result) {
                    eprintln!("Delivering LSP command response: {error:?}");
                }
            },
        );
        receiver
    }

    pub fn sync_editor_snapshot(&self, path: PathBuf, content: String) {
        let rope = Rope::from(content.as_str());
        let previous = self
            .editor_snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(path.clone(), rope.clone());
        if previous.as_ref() != Some(&rope) {
            self.invalidate_prediction_buffer(&path);
        }
        self.proxy_rpc.editor_snapshot(path, content);
    }

    pub fn update_editor_text(&self, path: PathBuf, content: String) {
        self.update_editor_rope(path, Rope::from(content.as_str()));
    }

    pub fn update_editor_rope(&self, path: PathBuf, content: Rope) {
        let mut snapshots = self
            .editor_snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(previous) = snapshots.get_mut(&path) {
            if let Some(delta) = editor_text_delta(previous, &content) {
                self.invalidate_prediction_buffer(&path);
                self.proxy_rpc.editor_edit(path, delta);
                *previous = content;
            }
        } else {
            self.invalidate_prediction_buffer(&path);
            self.proxy_rpc
                .editor_snapshot(path.clone(), content.to_string());
            snapshots.insert(path, content);
        }
    }

    pub(crate) fn acknowledge_shared_buffer(
        &self,
        session_id: &str,
        snapshot: &SharedBufferSnapshot,
    ) {
        let key = (session_id.to_string(), snapshot.path.clone());
        let mut changes = self
            .shared_buffer_changes
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if changes
            .get(&key)
            .is_some_and(|(_, latest)| latest == snapshot)
        {
            changes.remove(&key);
        }
        drop(changes);
        self.sync_editor_snapshot(
            self.workspace.join(&snapshot.path),
            snapshot.content.clone(),
        );
    }

    pub(crate) fn editor_recovery_available(&self) -> bool {
        self.recovery_available.load(Ordering::Acquire)
    }

    pub(crate) fn enable_editor_recovery(&self) {
        self.recovery_available.store(true, Ordering::Release);
    }

    fn editor_recovery_request<T: serde::de::DeserializeOwned + Send + 'static>(
        self: &Arc<Self>,
        request: AheadRequest,
    ) -> async_channel::Receiver<Result<T, String>> {
        let (sender, receiver) = async_channel::bounded(1);
        let client = Arc::downgrade(self);
        self.proxy_rpc.ahead_request(request, move |result| {
            let result = result.map_err(|error| error.message).and_then(|value| {
                serde_json::from_value(value).map_err(|error| {
                    format!("Invalid editor recovery response: {error}")
                })
            });
            if let Err(error) = &result
                && let Some(client) = client.upgrade()
            {
                *client
                    .core_message
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) =
                    Some(format!("Editor recovery failed: {error}"));
            }
            if let Err(async_channel::TrySendError::Full(_)) =
                sender.try_send(result)
            {
                eprintln!("Duplicate editor recovery response");
            }
        });
        receiver
    }

    pub(crate) fn list_editor_recoveries(
        self: &Arc<Self>,
    ) -> async_channel::Receiver<
        Result<Vec<ahead_rpc::file::EditorRecoverySummary>, String>,
    > {
        self.editor_recovery_request(AheadRequest::ListEditorRecoveries)
    }

    pub(crate) fn read_editor_recovery(
        self: &Arc<Self>,
        buffer_id: String,
    ) -> async_channel::Receiver<
        Result<Option<ahead_rpc::file::EditorRecoverySnapshot>, String>,
    > {
        self.editor_recovery_request(AheadRequest::ReadEditorRecovery { buffer_id })
    }

    pub(crate) fn write_editor_recovery(
        self: &Arc<Self>,
        snapshot: ahead_rpc::file::EditorRecoverySnapshot,
    ) -> async_channel::Receiver<Result<bool, String>> {
        self.editor_recovery_request(AheadRequest::WriteEditorRecovery { snapshot })
    }

    pub(crate) fn discard_shared_remote_recovery(
        &self,
        path: PathBuf,
    ) -> Result<(), RpcError> {
        self.proxy_rpc.ahead_request_blocking(
            AheadRequest::DiscardSharedRemoteRecovery { path },
        )?;
        Ok(())
    }

    pub fn close_editor_buffer(&self, path: PathBuf) {
        self.invalidate_prediction_buffer(&path);
        self.prediction_revisions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .buffers
            .remove(&path);
        self.editor_snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&path);
        self.anchor_requests
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&path);
        self.anchors
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&path);
        self.proxy_rpc.close_editor_buffer(path);
    }

    pub fn save_editor_buffer(
        &self,
        path: PathBuf,
        content: String,
    ) -> async_channel::Receiver<Result<(), RpcError>> {
        let (sender, receiver) = async_channel::bounded(1);
        self.proxy_rpc.request_async(
            ProxyRequest::SaveEditorBuffer { path, content },
            move |result| {
                let result = result.and_then(|response| match response {
                    ProxyResponse::SaveResponse {} => Ok(()),
                    _ => Err(RpcError {
                        code: 0,
                        message: "Unexpected response while saving file".into(),
                    }),
                });
                if let Err(async_channel::TrySendError::Full(_)) =
                    sender.try_send(result)
                {
                    eprintln!("Duplicate editor save result");
                }
            },
        );
        receiver
    }

    /// Hover goes through `ProxyRequest::GetHover`, whose response is routed
    /// back through `resolve_proxy_response` by the app pump.
    pub fn request_hover(
        self: &Arc<Self>,
        path: PathBuf,
        position: Position,
    ) -> Receiver<Hover> {
        let (tx, rx) = unbounded();
        let id = self.next_id();
        self.pending_hover
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx.clone());
        let pending = self.pending_hover.clone();
        let this = self.clone();
        self.proxy_rpc.get_hover(id, path, position, move |res| {
            if let Ok(ProxyResponse::HoverResponse { hover, .. }) = res {
                let _ = tx.send(hover);
            } else if let Err(RpcError { message, .. }) = &res {
                let _ = message;
            }
            pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            let _ = &this;
        });
        rx
    }

    pub fn request_definition(
        self: &Arc<Self>,
        path: PathBuf,
        position: Position,
    ) -> Receiver<Vec<Location>> {
        let (tx, rx) = unbounded();
        let id = self.next_id();
        self.pending_defs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx.clone());
        let pending = self.pending_defs.clone();
        self.proxy_rpc
            .get_definition(id, path, position, move |res| {
                if let Ok(ProxyResponse::GetDefinitionResponse {
                    definition, ..
                }) = res
                {
                    let _ = tx.send(goto_locations(definition));
                }
                pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
            });
        rx
    }

    pub fn request_references(
        self: &Arc<Self>,
        path: PathBuf,
        position: Position,
    ) -> Receiver<Vec<Location>> {
        let (tx, rx) = unbounded();
        self.proxy_rpc.get_references(path, position, move |res| {
            let references = match res {
                Ok(ProxyResponse::GetReferencesResponse { references }) => {
                    references
                }
                _ => Vec::new(),
            };
            let _ = tx.send(references);
        });
        rx
    }

    pub fn request_document_formatting(
        self: &Arc<Self>,
        path: PathBuf,
    ) -> Receiver<Result<Vec<TextEdit>, String>> {
        let (sender, receiver) = unbounded();
        self.proxy_rpc.get_document_formatting(path, move |result| {
            let result = match result {
                Ok(ProxyResponse::GetDocumentFormatting { edits }) => Ok(edits),
                Ok(_) => Err("Unexpected response to document formatting".into()),
                Err(error) => Err(error.message),
            };
            let _ = sender.send(result);
        });
        receiver
    }

    pub fn request_document_on_type_formatting(
        self: &Arc<Self>,
        path: PathBuf,
        position: Position,
        trigger_character: String,
    ) -> Receiver<Result<Vec<TextEdit>, String>> {
        let (sender, receiver) = unbounded();
        self.proxy_rpc.get_document_on_type_formatting(
            path,
            position,
            trigger_character,
            move |result| {
                let result = match result {
                    Ok(ProxyResponse::GetDocumentOnTypeFormatting { edits }) => {
                        Ok(edits)
                    }
                    Ok(_) => Err("Unexpected response to on-type formatting".into()),
                    Err(error) => Err(error.message),
                };
                let _ = sender.send(result);
            },
        );
        receiver
    }

    pub fn request_prepare_rename(
        &self,
        path: PathBuf,
        position: Position,
    ) -> Receiver<Result<PrepareRenameResponse, String>> {
        let (sender, receiver) = unbounded();
        self.proxy_rpc
            .prepare_rename(path, position, move |result| {
                let result = match result {
                    Ok(ProxyResponse::PrepareRename { resp }) => Ok(resp),
                    Ok(_) => {
                        Err("Unexpected response to prepare-rename request".into())
                    }
                    Err(error) => Err(error.message),
                };
                let _ = sender.send(result);
            });
        receiver
    }

    pub fn request_rename(
        &self,
        path: PathBuf,
        position: Position,
        new_name: String,
    ) -> Receiver<Result<WorkspaceEdit, String>> {
        let (sender, receiver) = unbounded();
        self.proxy_rpc
            .rename(path, position, new_name, move |result| {
                let result = match result {
                    Ok(ProxyResponse::Rename { edit }) => Ok(edit),
                    Ok(_) => Err("Unexpected response to rename request".into()),
                    Err(error) => Err(error.message),
                };
                let _ = sender.send(result);
            });
        receiver
    }

    pub fn request_open_files_content(
        &self,
    ) -> Receiver<Result<Vec<TextDocumentItem>, String>> {
        let (sender, receiver) = unbounded();
        self.proxy_rpc.request_async(
            ProxyRequest::GetOpenFilesContent {},
            move |result| {
                let result = match result {
                    Ok(ProxyResponse::GetOpenFilesContentResponse { items }) => {
                        Ok(items)
                    }
                    Ok(_) => Err("Unexpected response to open-files request".into()),
                    Err(error) => Err(error.message),
                };
                let _ = sender.send(result);
            },
        );
        receiver
    }

    pub fn request_implementations(
        self: &Arc<Self>,
        path: PathBuf,
        position: Position,
    ) -> Receiver<Vec<Location>> {
        let (tx, rx) = unbounded();
        self.proxy_rpc
            .go_to_implementation(path, position, move |res| {
                let locations = match res {
                    Ok(ProxyResponse::GotoImplementationResponse {
                        resp, ..
                    }) => resp.map(goto_locations).unwrap_or_default(),
                    _ => Vec::new(),
                };
                let _ = tx.send(locations);
            });
        rx
    }

    fn notify_prediction_changes(&self) {
        self.prediction_subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|sender| match sender.try_send(()) {
                Ok(()) | Err(async_channel::TrySendError::Full(())) => true,
                Err(async_channel::TrySendError::Closed(())) => false,
            });
    }

    fn invalidate_prediction_buffer(&self, path: &Path) {
        let mut revisions = self
            .prediction_revisions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        revisions.next = revisions.next.wrapping_add(1);
        let next = revisions.next;
        revisions.buffers.insert(path.to_path_buf(), next);
        drop(revisions);
        self.notify_prediction_changes();
    }

    fn invalidate_prediction_session(&self, session_id: &str) {
        let mut revisions = self
            .prediction_revisions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        revisions.next = revisions.next.wrapping_add(1);
        let next = revisions.next;
        revisions.sessions.insert(session_id.to_string(), next);
        drop(revisions);
        self.notify_prediction_changes();
    }

    fn invalidate_prediction_configuration(&self) {
        let mut revisions = self
            .prediction_revisions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        revisions.next = revisions.next.wrapping_add(1);
        revisions.configuration = revisions.next;
        drop(revisions);
        self.notify_prediction_changes();
    }

    pub(crate) fn subscribe_prediction_changes(
        &self,
    ) -> async_channel::Receiver<()> {
        let (sender, receiver) = async_channel::bounded(1);
        self.prediction_subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(sender);
        receiver
    }

    pub(crate) fn inline_prediction_is_current(
        &self,
        context: &InlinePredictionContext,
    ) -> bool {
        let revisions = self
            .prediction_revisions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        revisions.route_is_current(context)
            && context.buffer_revisions.iter().all(|(path, revision)| {
                revisions.buffers.get(path) == Some(revision)
            })
    }

    pub(crate) fn inline_prediction_route_is_current(
        &self,
        context: &InlinePredictionContext,
    ) -> bool {
        self.prediction_revisions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .route_is_current(context)
    }

    pub(crate) fn request_inline(
        self: &Arc<Self>,
        path: PathBuf,
        position: Position,
        session_id: Option<String>,
        model_context: Option<ahead_rpc::ahead::PredictionModelContext>,
    ) -> InlinePredictionRequest {
        let mut context = {
            let revisions = self
                .prediction_revisions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            InlinePredictionContext {
                binding: None,
                session_revision: session_id.as_ref().map_or(0, |id| {
                    revisions.sessions.get(id).copied().unwrap_or(0)
                }),
                session_id: session_id.clone(),
                model_context: model_context.clone(),
                configuration_revision: revisions.configuration,
                buffer_revisions: revisions.buffers.clone(),
            }
        };
        let pending_context = context.clone();
        let (sender, receiver) = async_channel::bounded(1);
        let client = Arc::downgrade(self);
        let request_id = self.proxy_rpc.get_inline_completions(
            path,
            position,
            lsp_types::InlineCompletionTriggerKind::Automatic,
            session_id,
            model_context,
            move |response| {
                let Some(client) = client.upgrade() else {
                    return;
                };
                let Ok(ProxyResponse::GetInlineCompletions {
                    completions,
                    context_paths,
                    prediction,
                }) = response
                else {
                    return;
                };
                let consumed: Vec<_> = context_paths
                    .into_iter()
                    .map(|path| client.workspace.join(path))
                    .collect();
                if consumed
                    .iter()
                    .any(|path| !context.buffer_revisions.contains_key(path))
                {
                    return;
                }
                context
                    .buffer_revisions
                    .retain(|path, _| consumed.contains(path));
                context.binding = prediction;
                if !client.inline_prediction_is_current(&context) {
                    return;
                }
                let (text, range) = match completions {
                    InlineCompletionResponse::Array(items) => {
                        items.into_iter().next()
                    }
                    InlineCompletionResponse::List(list) => {
                        list.items.into_iter().next()
                    }
                }
                .map(|item| (item.insert_text, item.range))
                .unwrap_or_default();
                if let Err(error) = sender.try_send(InlinePrediction {
                    text,
                    range,
                    context,
                }) {
                    eprintln!("Inline prediction receiver closed: {error}");
                }
            },
        );
        InlinePredictionRequest {
            context: pending_context,
            receiver,
            rpc: self.proxy_rpc.clone(),
            request_id,
        }
    }

    // ---- DAP passthrough ----

    pub(crate) fn validate_inline_prediction(
        &self,
        path: PathBuf,
        position: Position,
        content_sha256: String,
        replacement: String,
        binding: ahead_rpc::ahead::PredictionBinding,
    ) -> async_channel::Receiver<Result<bool, RpcError>> {
        let (sender, receiver) = async_channel::bounded(1);
        self.proxy_rpc.request_async(
            ProxyRequest::ValidateInlinePrediction {
                path,
                position,
                content_sha256,
                replacement,
                binding,
            },
            move |response| {
                let result = response.and_then(|response| match response {
                    ProxyResponse::ValidateInlinePrediction { valid } => Ok(valid),
                    _ => Err(RpcError {
                        code: 0,
                        message: "Unexpected prediction validation response".into(),
                    }),
                });
                if let Err(error) = sender.try_send(result) {
                    if !matches!(error, async_channel::TrySendError::Closed(_)) {
                        eprintln!("Delivering prediction validation: {error}");
                    }
                }
            },
        );
        receiver
    }

    pub fn dap_start(
        &self,
        mut config: RunDebugConfig,
        breakpoints: HashMap<PathBuf, Vec<SourceBreakpoint>>,
    ) -> bool {
        let mut dbg = self.debug.lock().unwrap_or_else(|e| e.into_inner());
        if dbg.state.is_active() || dbg.connection_closed {
            return false;
        }
        if let Some(previous) = dbg.dap_id {
            self.proxy_rpc.dap_disconnect(previous);
        }
        config.dap_id = DapId::next();
        dbg.dap_id = Some(config.dap_id);
        dbg.thread_id = None;
        dbg.reason.clear();
        dbg.thread_frames.clear();
        dbg.frames.clear();
        dbg.scopes.clear();
        dbg.state = DapSessionState::Starting;
        dbg.error = None;
        self.proxy_rpc.dap_start(config, breakpoints);
        drop(dbg);
        self.notify_editor_metadata();
        true
    }

    fn stopped_debug_target(
        &self,
        selected_thread: Option<ThreadId>,
    ) -> Option<(DapId, ThreadId)> {
        let debug = self.debug.lock().unwrap_or_else(|error| error.into_inner());
        if debug.state != DapSessionState::Stopped {
            return None;
        }
        let thread_id = selected_thread
            .filter(|thread_id| debug.thread_frames.contains_key(thread_id))
            .or(debug.thread_id)?;
        Some((debug.dap_id?, thread_id))
    }

    pub fn dap_continue(&self) -> bool {
        self.dap_continue_for_thread(None)
    }

    pub fn dap_continue_for_thread(&self, thread_id: Option<ThreadId>) -> bool {
        let Some((dap_id, thread)) = self.stopped_debug_target(thread_id) else {
            return false;
        };
        self.proxy_rpc.dap_continue(dap_id, thread);
        true
    }

    pub fn dap_step_over(&self) -> bool {
        self.dap_step_over_for_thread(None)
    }

    pub fn dap_step_over_for_thread(&self, thread_id: Option<ThreadId>) -> bool {
        let Some((dap_id, thread)) = self.stopped_debug_target(thread_id) else {
            return false;
        };
        self.proxy_rpc.dap_step_over(dap_id, thread);
        true
    }

    pub fn dap_step_into(&self) -> bool {
        self.dap_step_into_for_thread(None)
    }

    pub fn dap_step_into_for_thread(&self, thread_id: Option<ThreadId>) -> bool {
        let Some((dap_id, thread)) = self.stopped_debug_target(thread_id) else {
            return false;
        };
        self.proxy_rpc.dap_step_into(dap_id, thread);
        true
    }

    pub fn dap_step_out(&self) -> bool {
        self.dap_step_out_for_thread(None)
    }

    pub fn dap_step_out_for_thread(&self, thread_id: Option<ThreadId>) -> bool {
        let Some((dap_id, thread)) = self.stopped_debug_target(thread_id) else {
            return false;
        };
        self.proxy_rpc.dap_step_out(dap_id, thread);
        true
    }

    pub fn dap_stop(&self) -> bool {
        let mut dbg = self.debug.lock().unwrap_or_else(|e| e.into_inner());
        let Some(dap_id) = dbg.dap_id.filter(|_| dbg.state.can_stop()) else {
            return false;
        };
        dbg.state = DapSessionState::Stopping;
        dbg.error = None;
        dbg.thread_id = None;
        dbg.thread_frames.clear();
        dbg.frames.clear();
        dbg.scopes.clear();
        dbg.reason.clear();
        self.proxy_rpc.dap_stop(dap_id);
        drop(dbg);
        self.notify_editor_metadata();
        true
    }

    pub(crate) fn subscribe_debug_terminals(
        &self,
    ) -> async_channel::Receiver<DebugTerminalEvent> {
        self.debug_terminal_rx.clone()
    }

    pub(crate) fn reply_debug_terminal(
        &self,
        request: &DebugTerminalRequest,
        result: Result<u32, String>,
    ) {
        let (shell_process_id, error) = match result {
            Ok(pid) => (Some(pid), None),
            Err(error) => (None, Some(error)),
        };
        self.proxy_rpc.dap_terminal_response(DebugTerminalResponse {
            dap_id: request.dap_id,
            generation: request.generation,
            request_seq: request.request_seq,
            shell_process_id,
            error,
        });
    }

    pub fn breakpoints_for(&self, path: &Path) -> HashSet<u32> {
        let path = self.breakpoint_path(path);
        self.debug()
            .breakpoints
            .get(&path)
            .cloned()
            .unwrap_or_default()
    }

    pub fn toggle_breakpoint(&self, path: &Path, line: u32) {
        let path = self.breakpoint_path(path);
        let mut dbg = self.debug.lock().unwrap_or_else(|e| e.into_inner());
        let breakpoints = dbg.breakpoints.entry(path.clone()).or_default();
        if !breakpoints.remove(&line) {
            breakpoints.insert(line);
        }
        if dbg.state.can_stop()
            && let Some(dap_id) = dbg.dap_id
        {
            let breakpoints = dbg
                .breakpoints
                .get(&path)
                .into_iter()
                .flat_map(|lines| lines.iter())
                .map(|line| SourceBreakpoint {
                    line: *line as usize,
                    ..Default::default()
                })
                .collect();
            self.proxy_rpc
                .dap_set_breakpoints(dap_id, path, breakpoints);
        }
        drop(dbg);
        self.notify_editor_metadata();
    }

    fn breakpoint_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace.join(path)
        }
    }
}

fn goto_locations(response: GotoDefinitionResponse) -> Vec<Location> {
    match response {
        GotoDefinitionResponse::Scalar(location) => vec![location],
        GotoDefinitionResponse::Array(locations) => locations,
        GotoDefinitionResponse::Link(links) => links
            .into_iter()
            .map(|link| Location {
                uri: link.target_uri,
                range: link.target_selection_range,
            })
            .collect(),
    }
}

fn upsert_agent_tool_call(calls: &mut Vec<AgentToolCall>, call: AgentToolCall) {
    if !call.id.is_empty()
        && let Some(existing) =
            calls.iter_mut().find(|existing| existing.id == call.id)
    {
        if !call.title.is_empty() {
            existing.title = call.title;
        }
        if !call.status.is_empty() {
            existing.status = call.status;
        }
        if !call.kind.is_empty() {
            existing.kind = call.kind;
        }
    } else {
        calls.push(call);
    }
}

fn route_proxy_response(
    proxy_rpc: &ProxyRpcHandler,
    id: ahead_rpc::RequestId,
    value: serde_json::Value,
) -> Result<(), serde_json::Error> {
    let response = serde_json::from_value::<ProxyResponse>(value)?;
    proxy_rpc.handle_response(id, Ok(response));
    Ok(())
}

fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), RpcError> {
    let Some(parent) = path.parent() else {
        return Err(RpcError {
            code: 0,
            message: format!("Checkpoint path has no parent: {}", path.display()),
        });
    };
    let temporary = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("file"),
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&temporary, contents).map_err(|error| RpcError {
        code: 0,
        message: format!("Write checkpoint temporary file: {error}"),
    })?;
    std::fs::rename(&temporary, path).map_err(|error| RpcError {
        code: 0,
        message: format!("Publish checkpoint file: {error}"),
    })
}

#[derive(serde::Deserialize)]
struct ProjectDebugConfigFile {
    #[serde(default)]
    configurations: Vec<RunDebugConfig>,
}

fn parse_project_debug_configs(source: &str) -> Result<Vec<RunDebugConfig>, String> {
    let file: ProjectDebugConfigFile =
        toml::from_str(source).map_err(|error| error.to_string())?;
    for config in &file.configurations {
        if config.name.trim().is_empty() {
            return Err("each configuration needs a non-empty name".into());
        }
        if config.program.trim().is_empty() {
            return Err(format!(
                "configuration {:?} needs a non-empty program",
                config.name
            ));
        }
        if config
            .debug_adapter
            .as_deref()
            .is_none_or(|adapter| adapter.trim().is_empty())
        {
            return Err(format!(
                "configuration {:?} needs a debug-adapter command",
                config.name
            ));
        }
        if config.prelaunch.is_some() {
            return Err(format!(
                "configuration {:?} uses unsupported prelaunch tasks",
                config.name
            ));
        }
    }
    Ok(file.configurations)
}

fn default_debug_configs() -> Vec<RunDebugConfig> {
    vec![
        RunDebugConfig {
            ty: Some("lldb".into()),
            debug_adapter: Some(default_debug_adapter()),
            debug_adapter_args: None,
            name: "Debug current target".into(),
            program: "target/debug/ahead".into(),
            args: None,
            cwd: None,
            env: None,
            prelaunch: None,
            dap_id: DapId(0),
            tracing_output: false,
            config_source: Default::default(),
        },
        RunDebugConfig {
            ty: Some("lldb".into()),
            debug_adapter: Some(default_debug_adapter()),
            debug_adapter_args: None,
            name: "Debug tests".into(),
            program: "cargo".into(),
            args: Some(vec!["test".into(), "-p".into(), "ahead-app".into()]),
            cwd: None,
            env: None,
            prelaunch: None,
            dap_id: DapId(0),
            tracing_output: false,
            config_source: Default::default(),
        },
    ]
}

fn default_debug_configs_for_workspace(workspace: &Path) -> Vec<RunDebugConfig> {
    let manifest = workspace.join("Cargo.toml");
    let Ok(source) = std::fs::read_to_string(manifest) else {
        return Vec::new();
    };
    let Ok(manifest) = toml::from_str::<toml::Value>(&source) else {
        return Vec::new();
    };
    let is_ahead = manifest
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str)
        == Some("ahead");
    if is_ahead {
        default_debug_configs()
    } else {
        Vec::new()
    }
}

fn default_debug_adapter() -> String {
    std::env::var("AHEAD_DAP_ADAPTER").unwrap_or_else(|_| "lldb-dap".to_string())
}

#[allow(dead_code)]
fn _assert_proxy_types(_resp: ProxyResponse, _diff: DiffInfo, _stopped: Stopped) {}

#[allow(dead_code)]
fn _assert_core_types(_req: CoreRequest, _resp: CoreResponse) {}

#[cfg(test)]
mod tests {
    use super::{
        AgentPatchPreview, DeltaOp, ProxyClient, ProxyResponse, WorkspaceFileEvent,
        editor_text_delta, harness_metadata, route_proxy_response,
        upsert_agent_tool_call, write_atomic,
    };
    use ahead_rpc::ahead::{
        AgentAuthMethod, AgentConfigChoice, AgentConfigOption,
        AgentConfigOptionValue, AgentPlanEntry, AgentToolCall, AgentUsage,
        AgentUserInputRequest, AheadNotification, AheadRequest, ConversationMessage,
        HarnessKind, VoiceTranscriptSpeaker,
    };
    use ahead_rpc::core::{CoreNotification, CoreRequest, CoreResponse};
    use ahead_rpc::dap_types::{DapSessionState, DebugTerminalRequest};
    use ahead_rpc::proxy::{
        ProxyNotification, ProxyRequest, ProxyRpc, ProxyRpcHandler,
    };
    use crossbeam_channel::bounded;
    use lsp_types::{
        HoverContents, MarkedString, MessageType, Position, ShowMessageParams,
    };
    use std::{
        collections::HashMap, path::PathBuf, process::Command, time::Duration,
    };

    fn test_client() -> std::sync::Arc<ProxyClient> {
        ProxyClient::new_for_test(PathBuf::new())
    }

    fn inline_reply(text: &str, paths: &[&str]) -> ProxyResponse {
        inline_reply_with_range(text, None, paths)
    }

    fn inline_reply_with_range(
        text: &str,
        range: Option<lsp_types::Range>,
        paths: &[&str],
    ) -> ProxyResponse {
        ProxyResponse::GetInlineCompletions {
            prediction: None,
            completions: lsp_types::InlineCompletionResponse::Array(vec![
                lsp_types::InlineCompletionItem {
                    insert_text: text.into(),
                    filter_text: None,
                    range,
                    command: None,
                    insert_text_format: None,
                },
            ]),
            context_paths: paths.iter().map(PathBuf::from).collect(),
        }
    }

    #[test]
    fn inline_prediction_replies_keep_request_identity_and_consumed_revisions() {
        let client = ProxyClient::new_for_test(PathBuf::from("/project"));
        let rpc = client.rpc_for_test();
        client.sync_editor_snapshot("/project/src/helper.rs".into(), "first".into());
        client.sync_editor_snapshot(
            "/project/docs/other.md".into(),
            "unrelated".into(),
        );
        rpc.rx().try_iter().for_each(drop);
        let first = client.request_inline(
            "/project/src/main.rs".into(),
            Position::new(0, 0),
            Some("first-session".into()),
            None,
        );
        let second = client.request_inline(
            "/project/src/main.rs".into(),
            Position::new(0, 1),
            Some("second-session".into()),
            None,
        );
        let ProxyRpc::Request(
            first_id,
            ProxyRequest::GetInlineCompletions { session_id, .. },
        ) = rpc.rx().try_recv().unwrap()
        else {
            panic!("first inline request")
        };
        assert_eq!(session_id.as_deref(), Some("first-session"));
        let ProxyRpc::Request(
            second_id,
            ProxyRequest::GetInlineCompletions { session_id, .. },
        ) = rpc.rx().try_recv().unwrap()
        else {
            panic!("second inline request")
        };
        assert_eq!(session_id.as_deref(), Some("second-session"));
        client.update_editor_text(
            "/project/docs/other.md".into(),
            "changed unrelated".into(),
        );
        let range = lsp_types::Range::new(Position::new(0, 0), Position::new(0, 1));
        rpc.handle_response(
            second_id,
            Ok(inline_reply_with_range(
                "second",
                Some(range.clone()),
                &["src/helper.rs"],
            )),
        );
        let prediction = second.try_recv().expect("second reply");
        assert_eq!(prediction.text, "second");
        assert_eq!(prediction.range, Some(range));
        assert!(client.inline_prediction_is_current(&prediction.context));
        client.route_ahead(AheadNotification::PredictionContextChanged {
            session_id: Some("first-session".into()),
        });
        assert!(client.inline_prediction_is_current(&prediction.context));
        rpc.handle_response(first_id, Ok(inline_reply("first", &["src/helper.rs"])));
        assert!(
            first.try_recv().is_err(),
            "changed session discards its late reply"
        );
        client.update_editor_text(
            "/project/src/helper.rs".into(),
            "changed helper".into(),
        );
        assert!(!client.inline_prediction_is_current(&prediction.context));
    }

    #[test]
    fn inline_prediction_invalidates_changed_and_reopened_buffers_and_provider_settings()
     {
        for change in [
            "edit",
            "reopen",
            "provider",
            "session",
            "new-buffer",
            "disconnect",
            "shutdown",
        ] {
            let client = ProxyClient::new_for_test(PathBuf::from("/project"));
            let rpc = client.rpc_for_test();
            let path = PathBuf::from("/project/src/helper.rs");
            client.sync_editor_snapshot(path.clone(), "original".into());
            rpc.rx().try_iter().for_each(drop);
            let receiver = client.request_inline(
                "/project/src/main.rs".into(),
                Position::new(0, 0),
                Some("session".into()),
                None,
            );
            let ProxyRpc::Request(id, _) = rpc.rx().try_recv().unwrap() else {
                panic!("inline request")
            };
            match change {
                "edit" => client.update_editor_text(path, "changed".into()),
                "reopen" => {
                    client.close_editor_buffer(path.clone());
                    client.sync_editor_snapshot(path, "original".into());
                }
                "provider" => {
                    client.route_ahead(AheadNotification::PredictionContextChanged {
                        session_id: None,
                    })
                }
                "session" => {
                    client.route_ahead(AheadNotification::PredictionContextChanged {
                        session_id: Some("session".into()),
                    })
                }
                "new-buffer" => client.sync_editor_snapshot(
                    "/project/src/new.rs".into(),
                    "new".into(),
                ),
                "disconnect" => client.proxy_disconnected(),
                "shutdown" => client.shutdown(),
                _ => unreachable!(),
            }
            let consumed = if change == "new-buffer" {
                "src/new.rs"
            } else {
                "src/helper.rs"
            };
            rpc.handle_response(id, Ok(inline_reply("stale", &[consumed])));
            assert!(
                receiver.try_recv().is_err(),
                "{change} must discard the reply"
            );
        }
    }

    #[test]
    fn workspace_prediction_changes_only_invalidate_consumed_buffers() {
        for change in [
            "unrelated",
            "consumed",
            "rescan",
            "stale-event",
            "provider-refresh",
        ] {
            let client = ProxyClient::new_for_test(PathBuf::from("/project"));
            let rpc = client.rpc_for_test();
            let helper = PathBuf::from("/project/helper.rs");
            let unrelated = PathBuf::from("/project/unrelated.rs");
            client.sync_editor_snapshot(helper.clone(), "helper".into());
            client.sync_editor_snapshot(unrelated.clone(), "unrelated".into());
            client.route_core(CoreNotification::WorkspaceFileChange {
                generation: 2,
                paths: vec![unrelated.clone()],
            });
            client.route_ahead(AheadNotification::PredictionContextChanged {
                session_id: None,
            });
            rpc.rx().try_iter().for_each(drop);
            let request = client.request_inline(
                "/project/main.rs".into(),
                Position::new(0, 0),
                None,
                None,
            );
            let ProxyRpc::Request(id, _) =
                rpc.rx().try_recv().expect("prediction request")
            else {
                panic!("prediction request")
            };
            rpc.handle_response(id, Ok(inline_reply("suggestion", &["helper.rs"])));
            let prediction = request.try_recv().expect("prediction reply");
            let paths = match change {
                "unrelated" => vec![unrelated],
                "consumed" | "stale-event" => vec![helper],
                "provider-refresh" => {
                    vec![PathBuf::from("/project/.ahead/settings.toml")]
                }
                "rescan" => Vec::new(),
                _ => unreachable!(),
            };
            client.route_core(CoreNotification::WorkspaceFileChange {
                generation: if change == "stale-event" { 1 } else { 2 },
                paths,
            });
            assert_eq!(
                client.inline_prediction_is_current(&prediction.context),
                !matches!(change, "consumed" | "rescan"),
                "{change}"
            );
        }
    }

    #[test]
    fn editor_text_delta_is_utf8_safe_and_only_carries_changed_text() {
        let previous = "fn main() { let crab = \"🦀\"; let value = 1; }\n";
        let current = "fn main() { let crab = \"🦀\"; let value = 2; }\n";
        let previous_rope = ropey::Rope::from(previous);
        let current_rope = ropey::Rope::from(current);
        let delta =
            editor_text_delta(&previous_rope, &current_rope).expect("one edit");
        assert_eq!(delta.base_len(), previous.len());
        assert_eq!(
            delta.apply(&ropey::Rope::from(previous)).to_string(),
            current
        );
        let deleted_bytes = delta
            .ops()
            .iter()
            .map(|operation| match operation {
                DeltaOp::Delete(bytes) => *bytes,
                _ => 0,
            })
            .sum::<usize>();
        let inserted_bytes = delta
            .ops()
            .iter()
            .map(|operation| match operation {
                DeltaOp::Insert(text) => text.len(),
                _ => 0,
            })
            .sum::<usize>();
        assert_eq!(deleted_bytes, 1);
        assert_eq!(inserted_bytes, 1);
        assert!(editor_text_delta(&current_rope, &current_rope).is_none());

        let appended = "fn main() { let crab = \"🦀✨\"; let value = 2; }\n";
        let unicode_delta =
            editor_text_delta(&current_rope, &ropey::Rope::from(appended))
                .expect("Unicode insertion");
        assert_eq!(
            unicode_delta.apply(&ropey::Rope::from(current)).to_string(),
            appended
        );
        assert!(unicode_delta.ops().iter().any(
            |operation| matches!(operation, DeltaOp::Insert(text) if text == "✨")
        ));
    }

    #[test]
    fn editor_text_delta_handles_long_multichunk_unicode_prefix() {
        let prefix = "🦀 café\r\n".repeat(2_000);
        let previous = format!("{prefix}let value = 1;\n");
        let current = format!("{prefix}let value = 2;\n");
        let previous_rope = ropey::Rope::from(previous.as_str());
        let current_rope = ropey::Rope::from(current.as_str());

        assert_eq!(
            editor_text_delta(&previous_rope, &current_rope)
                .expect("one edit")
                .apply(&previous_rope)
                .to_string(),
            current
        );
        assert!(editor_text_delta(&previous_rope, &previous_rope).is_none());

        let inserted = format!("✨{previous}");
        let inserted_rope = ropey::Rope::from(inserted.as_str());
        assert_eq!(
            editor_text_delta(&previous_rope, &inserted_rope)
                .expect("leading Unicode insertion")
                .apply(&previous_rope)
                .to_string(),
            inserted
        );

        let mut resegmented_rope = previous_rope.clone();
        resegmented_rope.insert(0, "x");
        resegmented_rope.remove(0..1);
        assert!(editor_text_delta(&previous_rope, &resegmented_rope).is_none());

        let old = ropey::Rope::from(format!("é{prefix}"));
        let new = ropey::Rope::from(format!("©{prefix}"));
        assert_eq!(
            editor_text_delta(&old, &new)
                .expect("changed leading Unicode character")
                .apply(&old),
            new
        );
        let old = ropey::Rope::from("é");
        let new = ropey::Rope::from("©");
        assert_eq!(
            editor_text_delta(&old, &new)
                .expect("shared trailing UTF-8 byte is not a character")
                .apply(&old),
            new
        );
    }

    #[test]
    fn editor_text_updates_use_deltas_until_a_full_resync_or_close() {
        let client = test_client();
        let receiver = client.proxy_rpc.rx().clone();
        let path = PathBuf::from("/tmp/disposable-editor.ts");
        let initial = "const value = 1;\n";
        let edited = "const value = 2;\n";

        client.sync_editor_snapshot(path.clone(), initial.into());
        assert!(matches!(
            receiver.try_recv(),
            Ok(ProxyRpc::Notification(ProxyNotification::EditorSnapshot {
                path: snapshot_path,
                content,
            })) if snapshot_path == path && content == initial
        ));

        client.update_editor_rope(path.clone(), ropey::Rope::from(edited));
        assert!(matches!(
            receiver.try_recv(),
            Ok(ProxyRpc::Notification(ProxyNotification::EditorEdit {
                path: edit_path,
                delta,
            })) if edit_path == path
                && delta.apply(&ropey::Rope::from(initial)).to_string() == edited
        ));

        let reloaded = "const value = 3;\n";
        client.sync_editor_snapshot(path.clone(), reloaded.into());
        assert!(matches!(
            receiver.try_recv(),
            Ok(ProxyRpc::Notification(ProxyNotification::EditorSnapshot {
                path: snapshot_path,
                content,
            })) if snapshot_path == path && content == reloaded
        ));

        client.close_editor_buffer(path.clone());
        assert!(matches!(
            receiver.try_recv(),
            Ok(ProxyRpc::Notification(ProxyNotification::CloseEditorBuffer {
                path: closed_path,
            })) if closed_path == path
        ));

        client.update_editor_text(path.clone(), initial.into());
        assert!(matches!(
            receiver.try_recv(),
            Ok(ProxyRpc::Notification(ProxyNotification::EditorSnapshot {
                path: snapshot_path,
                content,
            })) if snapshot_path == path && content == initial
        ));
    }

    #[test]
    fn document_symbol_context_flattens_nested_and_flat_lsp_results() {
        let nested = serde_json::from_value::<lsp_types::DocumentSymbolResponse>(
            serde_json::json!([{
                "name": "module",
                "kind": 2,
                "range": {
                    "start": {"line": 0, "character": 0},
                    "end": {"line": 12, "character": 0}
                },
                "selectionRange": {
                    "start": {"line": 0, "character": 0},
                    "end": {"line": 1, "character": 0}
                },
                "children": [{
                    "name": "inner\nfn",
                    "kind": 12,
                    "range": {
                        "start": {"line": 3, "character": 0},
                        "end": {"line": 7, "character": 1}
                    },
                    "selectionRange": {
                        "start": {"line": 3, "character": 3},
                        "end": {"line": 3, "character": 9}
                    }
                }]
            }]),
        )
        .expect("nested LSP document symbols");
        let nested = super::agent_buffer_symbols(nested);
        assert_eq!(
            nested,
            vec![
                ahead_rpc::ahead::AgentBufferSymbol {
                    name: "module".into(),
                    start_line: 0,
                    end_line: 12,
                    depth: 0,
                },
                ahead_rpc::ahead::AgentBufferSymbol {
                    name: "inner fn".into(),
                    start_line: 3,
                    end_line: 8,
                    depth: 1,
                },
            ]
        );

        let flat = serde_json::from_value::<lsp_types::DocumentSymbolResponse>(
            serde_json::json!([{
                "name": "flat_fn",
                "kind": 12,
                "location": {
                    "uri": "file:///workspace/src/lib.rs",
                    "range": {
                        "start": {"line": 2, "character": 0},
                        "end": {"line": 5, "character": 0}
                    }
                }
            }, {
                "name": "one_line_fn",
                "kind": 12,
                "location": {
                    "uri": "file:///workspace/src/lib.rs",
                    "range": {
                        "start": {"line": 6, "character": 0},
                        "end": {"line": 6, "character": 22}
                    }
                }
            }]),
        )
        .expect("flat LSP document symbols");
        assert_eq!(
            super::agent_buffer_symbols(flat),
            vec![
                ahead_rpc::ahead::AgentBufferSymbol {
                    name: "flat_fn".into(),
                    start_line: 2,
                    end_line: 5,
                    depth: 0,
                },
                ahead_rpc::ahead::AgentBufferSymbol {
                    name: "one_line_fn".into(),
                    start_line: 6,
                    end_line: 7,
                    depth: 0,
                },
            ]
        );
    }

    #[test]
    fn document_symbols_use_the_existing_lsp_proxy_route() {
        let client = test_client();
        let response =
            client.document_symbols(PathBuf::from("/workspace/src/lib.rs"));
        let ProxyRpc::Request(request_id, ProxyRequest::GetDocumentSymbols { path }) =
            client
                .proxy_rpc
                .rx()
                .try_recv()
                .expect("document-symbol request")
        else {
            panic!("expected document-symbol proxy request");
        };
        assert_eq!(path, PathBuf::from("/workspace/src/lib.rs"));

        client.proxy_rpc.handle_response(
            request_id,
            Ok(ProxyResponse::GetDocumentSymbols {
                resp: lsp_types::DocumentSymbolResponse::Nested(Vec::new()),
            }),
        );
        assert_eq!(
            response.try_recv().expect("document-symbol response"),
            Ok(Vec::new())
        );
    }

    #[test]
    fn semantic_tokens_use_the_existing_lsp_proxy_route() {
        let client = test_client();
        let path = PathBuf::from("/workspace/src/lib.rs");
        let response = client.semantic_tokens(path.clone());
        let ProxyRpc::Request(
            request_id,
            ProxyRequest::GetSemanticTokens { path: request_path },
        ) = client
            .proxy_rpc
            .rx()
            .try_recv()
            .expect("semantic-token request")
        else {
            panic!("expected semantic-token proxy request");
        };
        assert_eq!(request_path, path);

        client.proxy_rpc.handle_response(
            request_id,
            Ok(ProxyResponse::GetSemanticTokens {
                styles: ahead_rpc::style::SemanticStyles {
                    rev: 1,
                    path: path.clone(),
                    len: 2,
                    styles: Vec::new(),
                },
            }),
        );
        let styles = response
            .try_recv()
            .expect("semantic-token response")
            .expect("semantic-token styles");
        assert_eq!(styles.path, path);
        assert_eq!(styles.len, 2);
    }

    #[test]
    fn extension_syntax_uses_the_proxy_route() {
        let client = test_client();
        let path = PathBuf::from("/workspace/page.html");
        let response = client.extension_syntax(path.clone());
        let ProxyRpc::Request(
            request_id,
            ProxyRequest::GetExtensionSyntax { path: request_path },
        ) = client
            .proxy_rpc
            .rx()
            .try_recv()
            .expect("extension-syntax request")
        else {
            panic!("expected extension-syntax proxy request");
        };
        assert_eq!(request_path, path);

        client.proxy_rpc.handle_response(
            request_id,
            Ok(ProxyResponse::GetExtensionSyntax {
                syntax: ahead_rpc::style::ExtensionSyntax {
                    rev: 1,
                    path: path.clone(),
                    len: 4,
                    language_name: Some("HTML".into()),
                    captures: vec![ahead_rpc::style::SyntaxCapture {
                        start: 0,
                        end: 4,
                        scope: "tag".into(),
                    }],
                    comments: None,
                },
            }),
        );
        let syntax = response
            .try_recv()
            .expect("extension-syntax response")
            .expect("extension syntax");
        assert_eq!(syntax.path, path);
        assert_eq!(syntax.len, 4);
        assert_eq!(syntax.language_name.as_deref(), Some("HTML"));
        assert_eq!(syntax.captures[0].scope, "tag");
    }

    #[test]
    fn git_commit_requests_staged_changes_and_reports_result() {
        use ahead_rpc::proxy::{ProxyRequest, ProxyRpc};

        let client = test_client();
        let response = client.git_commit("Commit staged changes".into());
        let ProxyRpc::Request(id, ProxyRequest::GitCommit { message, diffs }) =
            client.proxy_rpc.rx().try_recv().expect("commit request")
        else {
            panic!("expected Git commit request");
        };
        assert_eq!(message, "Commit staged changes");
        assert!(diffs.is_empty());
        client
            .proxy_rpc
            .handle_response(id, Ok(ProxyResponse::Success {}));
        assert_eq!(response.try_recv().expect("commit result"), Ok(()));

        let response = client.git_commit("Retry".into());
        let ProxyRpc::Request(id, _) =
            client.proxy_rpc.rx().try_recv().expect("retry request")
        else {
            panic!("expected retry request");
        };
        client.proxy_rpc.handle_response(
            id,
            Err(ahead_rpc::RpcError {
                code: 0,
                message: "Stage changes before committing".into(),
            }),
        );
        assert_eq!(
            response.try_recv().expect("commit error"),
            Err("Stage changes before committing".to_string())
        );

        let response = client.git_stage_all();
        let ProxyRpc::Request(id, ProxyRequest::GitStageAll {}) =
            client.proxy_rpc.rx().try_recv().expect("stage request")
        else {
            panic!("expected Git stage request");
        };
        client
            .proxy_rpc
            .handle_response(id, Ok(ProxyResponse::Success {}));
        assert_eq!(response.try_recv().expect("stage result"), Ok(()));

        let response = client.git_stage_paths(vec!["src/main.rs".into()], true);
        let ProxyRpc::Request(id, ProxyRequest::GitStagePaths { paths, staged }) =
            client
                .proxy_rpc
                .rx()
                .try_recv()
                .expect("stage path request")
        else {
            panic!("expected Git path stage request");
        };
        assert_eq!(paths, ["src/main.rs"]);
        assert!(staged);
        client
            .proxy_rpc
            .handle_response(id, Ok(ProxyResponse::Success {}));
        assert_eq!(response.try_recv().expect("stage path result"), Ok(()));
    }

    #[test]
    fn debug_terminal_request_and_retirement_reach_the_native_owner() {
        use ahead_rpc::dap_types::RunInTerminalArguments;
        use ahead_rpc::proxy::{ProxyNotification, ProxyRpc};

        let client = test_client();
        let events = client.subscribe_debug_terminals();
        let notifications = client.proxy_rpc.rx();
        let config = super::default_debug_configs().remove(0);
        assert!(client.dap_start(config, HashMap::new()));
        let dap_id = client.debug().dap_id.expect("active debugger");
        assert!(matches!(
            notifications.try_recv().expect("start"),
            ProxyRpc::Notification(ProxyNotification::DapStart { .. })
        ));
        let request = DebugTerminalRequest {
            dap_id,
            generation: 5,
            request_seq: 9,
            arguments: RunInTerminalArguments {
                kind: Some("integrated".into()),
                title: None,
                cwd: None,
                args: vec!["/bin/echo".into(), "hello".into()],
                env: None,
            },
        };
        client.route_core(CoreNotification::RunInTerminal {
            request: request.clone(),
        });
        assert!(matches!(
            events.try_recv().expect("native terminal request"),
            super::DebugTerminalEvent::Request(received) if received == request
        ));
        client.reply_debug_terminal(&request, Ok(42));
        assert!(matches!(
            notifications.try_recv().expect("terminal response"),
            ProxyRpc::Notification(ProxyNotification::DapTerminalResponse { response })
                if response.dap_id == dap_id
                    && response.generation == 5
                    && response.request_seq == 9
                    && response.shell_process_id == Some(42)
                    && response.error.is_none()
        ));
        client.route_core(CoreNotification::DapSessionState {
            dap_id,
            state: DapSessionState::Terminated,
        });
        assert!(matches!(
            events.try_recv().expect("terminal retirement"),
            super::DebugTerminalEvent::Retire(retired) if retired == dap_id
        ));
    }

    #[test]
    fn git_worktree_file_diff_is_requested_through_the_proxy() {
        use ahead_rpc::proxy::{ProxyRequest, ProxyRpc};

        let client = test_client();
        let path = PathBuf::from("deleted.rs");
        let response = client.git_worktree_file_patch(path.clone());
        let ProxyRpc::Request(
            id,
            ProxyRequest::GitWorktreeFilePatch {
                path: requested_path,
            },
        ) = client
            .proxy_rpc
            .rx()
            .try_recv()
            .expect("working diff request")
        else {
            panic!("expected working-tree file diff request");
        };
        assert_eq!(requested_path, path);
        client.proxy_rpc.handle_response(
            id,
            Ok(ProxyResponse::GitWorktreeFilePatch {
                patch: ahead_rpc::source_control::GitPatch {
                    id: "deleted.rs".into(),
                    patch: "-removed\n".into(),
                    truncated: false,
                },
            }),
        );
        let patch = response
            .try_recv()
            .expect("working diff result")
            .expect("patch response");
        assert_eq!(patch.id, "deleted.rs");
        assert!(patch.patch.contains("-removed"));
    }

    #[test]
    fn git_workspace_diff_is_requested_through_the_proxy() {
        use ahead_rpc::proxy::{ProxyRequest, ProxyRpc};

        let client = test_client();
        let response = client.git_worktree_patch();
        let ProxyRpc::Request(id, ProxyRequest::GitWorktreePatch {}) = client
            .proxy_rpc
            .rx()
            .try_recv()
            .expect("aggregate working diff request")
        else {
            panic!("expected aggregate working-tree diff request");
        };
        client.proxy_rpc.handle_response(
            id,
            Ok(ProxyResponse::GitWorktreePatch {
                patch: ahead_rpc::source_control::GitPatch {
                    id: "working-tree".into(),
                    patch: "diff --git a/one.rs b/one.rs\n+one\ndiff --git a/two.rs b/two.rs\n+two\n"
                        .into(),
                    truncated: false,
                },
            }),
        );
        let patch = response
            .try_recv()
            .expect("aggregate diff result")
            .expect("patch response");
        assert!(patch.patch.contains("one.rs"));
        assert!(patch.patch.contains("two.rs"));
    }

    #[test]
    fn git_branch_switch_is_requested_as_an_acknowledged_operation() {
        use ahead_rpc::proxy::{ProxyRequest, ProxyRpc};

        let client = test_client();
        let response = client.git_checkout("feature".into());
        let ProxyRpc::Request(id, ProxyRequest::GitCheckout { reference }) = client
            .proxy_rpc
            .rx()
            .try_recv()
            .expect("branch switch request")
        else {
            panic!("expected Git branch switch request");
        };
        assert_eq!(reference, "feature");
        client
            .proxy_rpc
            .handle_response(id, Ok(ProxyResponse::Success {}));
        assert!(response.try_recv().expect("branch switch result").is_ok());
    }

    #[test]
    fn git_branch_creation_is_requested_as_an_acknowledged_operation() {
        use ahead_rpc::proxy::{ProxyRequest, ProxyRpc};

        let client = test_client();
        let response = client.git_create_branch("feature".into());
        let ProxyRpc::Request(id, ProxyRequest::GitCreateBranch { reference }) =
            client
                .proxy_rpc
                .rx()
                .try_recv()
                .expect("branch creation request")
        else {
            panic!("expected Git branch creation request");
        };
        assert_eq!(reference, "feature");
        client
            .proxy_rpc
            .handle_response(id, Ok(ProxyResponse::Success {}));
        assert!(response.try_recv().expect("branch creation result").is_ok());
    }

    #[test]
    fn git_branch_deletion_is_requested_as_an_acknowledged_operation() {
        use ahead_rpc::proxy::{ProxyRequest, ProxyRpc};

        let client = test_client();
        let response = client.git_delete_branch("feature".into());
        let ProxyRpc::Request(id, ProxyRequest::GitDeleteBranch { reference }) =
            client
                .proxy_rpc
                .rx()
                .try_recv()
                .expect("branch deletion request")
        else {
            panic!("expected Git branch deletion request");
        };
        assert_eq!(reference, "feature");
        client
            .proxy_rpc
            .handle_response(id, Ok(ProxyResponse::Success {}));
        assert!(response.try_recv().expect("branch deletion result").is_ok());
    }

    #[test]
    fn git_push_is_requested_as_an_acknowledged_operation() {
        use ahead_rpc::proxy::{ProxyRequest, ProxyRpc};

        let client = test_client();
        let remote = Some("origin".to_string());
        let response = client.git_push(remote.clone());
        let ProxyRpc::Request(id, ProxyRequest::GitPush { remote: requested }) =
            client.proxy_rpc.rx().try_recv().expect("Git push request")
        else {
            panic!("expected Git push request");
        };
        assert_eq!(requested, remote);
        client
            .proxy_rpc
            .handle_response(id, Ok(ProxyResponse::Success {}));
        assert!(response.try_recv().expect("Git push result").is_ok());
    }

    #[test]
    fn git_metadata_notifications_invalidate_identical_status_and_wake_views() {
        let client = test_client();
        let updates = client.subscribe_diagnostics();
        for expected_generation in 1..=2 {
            client.route_core(CoreNotification::DiffInfo {
                diff: Default::default(),
            });
            assert_eq!(client.git_generation(), expected_generation);
            assert!(updates.try_recv().is_ok());
        }
        let result =
            client.git_file_state(PathBuf::from("main.ts"), "unsaved\n".into());
        assert!(result.try_recv().is_err(), "request is asynchronous");
        let super::ProxyRpc::Request(
            id,
            super::ProxyRequest::GitFileState { path, content },
        ) = client.proxy_rpc.rx().try_recv().expect("metadata request")
        else {
            panic!("metadata request")
        };
        assert_eq!(path, PathBuf::from("main.ts"));
        assert_eq!(content, "unsaved\n");
        client.proxy_rpc.handle_response(
            id,
            Err(ahead_rpc::RpcError {
                code: 1,
                message: "unreadable Git repository".into(),
            }),
        );
        assert_eq!(
            result.try_recv().expect("error reply"),
            Err("unreadable Git repository".into())
        );
    }

    #[test]
    fn record_voice_transcript_persists_and_updates_the_local_conversation() {
        let client = test_client();
        let responder_client = client.clone();
        let responder = std::thread::spawn(move || {
            let ProxyRpc::Request(
                request_id,
                ProxyRequest::AheadRequest { request },
            ) = responder_client
                .proxy_rpc
                .rx()
                .recv_timeout(Duration::from_secs(2))
                .expect("voice transcript request")
            else {
                panic!("expected voice transcript RPC");
            };
            assert!(matches!(
                request,
                AheadRequest::RecordVoiceTranscript {
                    session_id,
                    call_id,
                    segment_id,
                    speaker: VoiceTranscriptSpeaker::Human,
                    start_ms: 120,
                    end_ms: 740,
                    ref content,
                } if session_id == "session-1"
                    && call_id == "call-1"
                    && segment_id == "00000000-0000-4000-8000-000000000001"
                    && content == "Explain this function"
            ));
            let message = ConversationMessage {
                id: "message-1".into(),
                session_id: "session-1".into(),
                turn_id: "voice:call-1".into(),
                sequence: 1,
                role: "human".into(),
                actor_id: "owner".into(),
                content: "Explain this function".into(),
                status: "complete".into(),
                created_at: "2026-10-07T00:00:00Z".into(),
                voice_timing: Some(ahead_rpc::ahead::VoiceTranscriptTiming {
                    call_id: "call-1".into(),
                    start_ms: 120,
                    end_ms: 740,
                }),
            };
            responder_client.proxy_rpc.handle_response(
                request_id,
                Ok(ProxyResponse::AheadResponse {
                    response: serde_json::to_value(message).expect("encode message"),
                }),
            );
        });

        let message = client
            .record_voice_transcript(
                "session-1".into(),
                "call-1".into(),
                "00000000-0000-4000-8000-000000000001".into(),
                VoiceTranscriptSpeaker::Human,
                120,
                740,
                "Explain this function".into(),
            )
            .expect("record voice transcript");
        responder.join().expect("RPC responder");

        assert_eq!(client.cached_conversation("session-1"), Some(vec![message]));
    }

    #[test]
    fn attribution_refresh_is_nonblocking_and_rejects_stale_or_closed_results() {
        let directory = tempfile::tempdir().expect("workspace");
        let client = ProxyClient::new_for_test(directory.path().to_owned());
        let path = directory.path().join("main.ts");
        let updates = client.subscribe_diagnostics();
        let request_id = || {
            let super::ProxyRpc::Request(
                id,
                super::ProxyRequest::AheadRequest {
                    request: super::AheadRequest::ListAnchorsForPaths { paths },
                },
            ) = client.proxy_rpc.rx().try_recv().expect("queued request")
            else {
                panic!("expected anchor request");
            };
            assert_eq!(paths, vec!["main.ts"]);
            id
        };
        client.refresh_anchors(std::slice::from_ref(&path));
        let old = request_id();
        client.refresh_anchors(std::slice::from_ref(&path));
        let current = request_id();
        let anchors = serde_json::json!([{
            "id": "anchor", "session_id": "session", "actor_id": "ahead",
            "path": "main.ts", "range": {"start": {"line": 1, "col": 0}, "end": {"line": 1, "col": 4}},
            "quote_hash": "hash", "surrounding_context": null
        }]);
        client.proxy_rpc.handle_response(
            current,
            Ok(ProxyResponse::AheadResponse {
                response: anchors.clone(),
            }),
        );
        assert_eq!(client.anchors_for(&path).len(), 1);
        assert!(updates.try_recv().is_ok());
        client.proxy_rpc.handle_response(
            old,
            Ok(ProxyResponse::AheadResponse {
                response: serde_json::json!([]),
            }),
        );
        assert_eq!(client.anchors_for(&path).len(), 1);
        assert!(updates.try_recv().is_err());
        client.refresh_anchors(std::slice::from_ref(&path));
        let closing = request_id();
        client.close_editor_buffer(path.clone());
        client.proxy_rpc.handle_response(
            closing,
            Ok(ProxyResponse::AheadResponse { response: anchors }),
        );
        assert!(client.anchors_for(&path).is_empty());
        assert!(updates.try_recv().is_err());
        client.proxy_rpc.rx().try_iter().for_each(drop);
        client.refresh_anchors(std::slice::from_ref(&path));
        let failed = request_id();
        client.proxy_rpc.handle_response(
            failed,
            Err(ahead_rpc::RpcError {
                code: 0,
                message: "test storage error".into(),
            }),
        );
        assert!(
            client
                .take_core_message()
                .expect("visible error")
                .contains("test storage error")
        );
        assert!(updates.try_recv().is_ok());
    }

    #[gpui_kit::test]
    fn preview_replacement_and_close_release_the_proxy_buffer(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("test project");
        let first = directory.path().join("main.ts");
        let second = directory.path().join("other.py");
        std::fs::write(&first, "const count = 1;\n").expect("TS source");
        std::fs::write(&second, "count = 1\n").expect("Python source");
        let client = test_client();
        let (panel, cx) = cx.add_window_view(|window, cx| {
            crate::code_panel::CodePanel::new(
                first.to_str().expect("path"),
                window,
                cx,
            )
            .with_proxy(
                client.clone(),
                directory.path().to_str().expect("workspace"),
                window,
                cx,
            )
        });
        client.proxy_rpc.rx().try_iter().for_each(drop);
        panel.update_in(cx, |panel, window, cx| {
            panel.open_file(second.to_str().expect("path"), true, window, cx)
        });
        let notifications = client
            .proxy_rpc
            .rx()
            .try_iter()
            .filter_map(|rpc| match rpc {
                super::ProxyRpc::Notification(
                    ahead_rpc::proxy::ProxyNotification::CloseEditorBuffer { path },
                ) => Some(("close", path)),
                super::ProxyRpc::Notification(
                    ahead_rpc::proxy::ProxyNotification::EditorSnapshot {
                        path, ..
                    },
                ) => Some(("open", path)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(notifications.first(), Some(&("close", first)));
        assert!(
            notifications
                .iter()
                .skip(1)
                .all(|(method, path)| *method == "open" && path == &second)
        );
        assert!(notifications.len() >= 2);
        panel.update(cx, |panel, _| panel.release_buffer());
        assert!(
            matches!(client.proxy_rpc.rx().try_recv(), Ok(super::ProxyRpc::Notification(ahead_rpc::proxy::ProxyNotification::CloseEditorBuffer { path })) if path == second)
        );
        panel.update(cx, |panel, _| panel.release_buffer());
        assert!(client.proxy_rpc.rx().try_recv().is_err());
        assert!(panel.update(cx, |panel, _| panel.proxy.is_none()));
    }

    #[test]
    fn completion_resolve_preserves_server_identity_and_original_item() {
        let client = test_client();
        let (sender, receiver) = async_channel::bounded(1);
        client
            .pending_completion
            .lock()
            .expect("completion map")
            .insert(42, sender);
        let item = lsp_types::CompletionItem {
            label: "calculate".into(),
            data: Some(serde_json::json!({"opaque": "server-owned token"})),
            ..Default::default()
        };
        let plugin_id = ahead_rpc::plugin::PluginId(17);
        client.route_core(CoreNotification::CompletionResponse {
            request_id: 42,
            input: "calc".into(),
            resp: Some(lsp_types::CompletionResponse::Array(vec![item.clone()])),
            plugin_id,
        });
        let completion = receiver
            .try_recv()
            .expect("completion response")
            .expect("non-null completion response")
            .items
            .remove(0);
        assert_eq!(completion.plugin_id, plugin_id);
        assert_eq!(completion.item, item);
        let resolved = client.resolve_completion(completion);
        let super::ProxyRpc::Request(
            id,
            super::ProxyRequest::CompletionResolve {
                plugin_id: target,
                completion_item,
            },
        ) = client.proxy_rpc.rx().try_recv().expect("resolve request")
        else {
            panic!("expected completion resolve");
        };
        assert_eq!(target, plugin_id);
        assert_eq!(*completion_item, item);
        let mut response = item;
        response.additional_text_edits = Some(vec![lsp_types::TextEdit {
            range: lsp_types::Range::default(),
            new_text: "import { calculate } from './math';\n".into(),
        }]);
        client.proxy_rpc.handle_response(
            id,
            Ok(ProxyResponse::CompletionResolveResponse {
                item: Box::new(response.clone()),
            }),
        );
        assert_eq!(
            resolved.try_recv().expect("reply").expect("success"),
            response
        );
        let failed = client.resolve_completion(super::LspCompletion {
            plugin_id,
            item: response,
        });
        client.read_proxy_messages(std::io::Cursor::new(Vec::<u8>::new()));
        assert!(failed.try_recv().expect("failure reply").is_err());
    }

    #[test]
    fn completion_null_is_distinct_from_an_empty_result() {
        let client = test_client();
        let (null_sender, null_receiver) = async_channel::bounded(1);
        let (empty_sender, empty_receiver) = async_channel::bounded(1);
        {
            let mut pending =
                client.pending_completion.lock().expect("completion map");
            pending.insert(51, null_sender);
            pending.insert(52, empty_sender);
        }

        client.route_core(CoreNotification::CompletionResponse {
            request_id: 51,
            input: "calc".into(),
            resp: None,
            plugin_id: ahead_rpc::plugin::PluginId(17),
        });
        client.route_core(CoreNotification::CompletionResponse {
            request_id: 52,
            input: "calc".into(),
            resp: Some(lsp_types::CompletionResponse::Array(Vec::new())),
            plugin_id: ahead_rpc::plugin::PluginId(17),
        });

        assert!(null_receiver.try_recv().expect("null response").is_none());
        assert!(
            empty_receiver
                .try_recv()
                .expect("empty response")
                .is_some_and(|batch| batch.items.is_empty() && !batch.is_incomplete)
        );
    }

    #[test]
    fn completion_list_preserves_the_server_incomplete_flag() {
        let client = test_client();
        let (sender, receiver) = async_channel::bounded(1);
        client
            .pending_completion
            .lock()
            .expect("completion map")
            .insert(53, sender);
        client.route_core(CoreNotification::CompletionResponse {
            request_id: 53,
            input: "Calcul".into(),
            resp: Some(lsp_types::CompletionResponse::List(
                lsp_types::CompletionList {
                    is_incomplete: true,
                    items: vec![lsp_types::CompletionItem {
                        label: "Calculation".into(),
                        ..Default::default()
                    }],
                },
            )),
            plugin_id: ahead_rpc::plugin::PluginId(17),
        });

        let batch = receiver
            .try_recv()
            .expect("completion reply")
            .expect("completion list");
        assert!(batch.is_incomplete);
        assert_eq!(batch.items.len(), 1);
        assert_eq!(batch.items[0].plugin_id, ahead_rpc::plugin::PluginId(17));
    }

    #[test]
    fn proxy_eof_fails_pending_requests_and_rejects_retries_until_restart() {
        let client = test_client();
        let (sender, receiver) = bounded(1);
        client.proxy_rpc.ahead_request(
            super::AheadRequest::ListSessions,
            move |result| {
                sender.send(result).expect("report result");
            },
        );
        client.read_proxy_messages(std::io::Cursor::new(Vec::<u8>::new()));
        let error = receiver
            .try_recv()
            .expect("pending response")
            .expect_err("disconnected");
        assert!(error.message.contains("connection closed"));
        assert!(
            client
                .take_core_message()
                .expect("visible status")
                .contains("Restart AHEAD")
        );

        let error = client
            .start_work_session(
                "Do not duplicate",
                "request",
                HarnessKind::Ahead,
                None,
                None,
            )
            .expect_err("closed connection cannot submit a new session");
        assert!(error.message.contains("may already have completed"));
        let queued = client.proxy_rpc.rx().try_iter().collect::<Vec<_>>();
        assert_eq!(queued.len(), 2);
        assert!(matches!(&queued[0], super::ProxyRpc::Request(_, _)));
        assert!(matches!(&queued[1], super::ProxyRpc::Shutdown));
    }

    #[cfg(unix)]
    #[test]
    fn proxy_disconnect_reports_an_unexpected_child_exit_status() {
        let client = test_client();
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 23"])
            .spawn()
            .expect("spawn test proxy child");
        let status = child.wait().expect("wait for test proxy child");
        assert_eq!(status.code(), Some(23));
        *client
            ._child
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(child);

        client.proxy_disconnected();

        assert!(
            client
                .take_core_message()
                .expect("proxy disconnect status")
                .contains("process exited: exit status: 23")
        );
    }

    #[test]
    fn debugger_round_trips_session_ids_and_fences_retired_sessions() {
        use ahead_rpc::proxy::ProxyNotification;

        let client = test_client();
        let notifications = client.proxy_rpc.rx();
        let updates = client.subscribe_diagnostics();
        let config = super::default_debug_configs().remove(0);
        let mut previous = None;
        for iteration in 0..2 {
            assert!(client.dap_start(config.clone(), HashMap::new()));
            let dap_id = client.debug().dap_id.expect("active ID");
            if let Some(previous) = previous {
                assert_ne!(dap_id, previous);
                assert!(matches!(
                    notifications.try_recv().expect("retire old session"),
                    super::ProxyRpc::Notification(ProxyNotification::DapDisconnect { dap_id })
                        if dap_id == previous
                ));
            }
            let super::ProxyRpc::Notification(notification) =
                notifications.try_recv().expect("start notification")
            else {
                panic!("start notification");
            };
            let wire = serde_json::to_vec(&notification).expect("serialize start");
            let ProxyNotification::DapStart {
                config: received, ..
            } = serde_json::from_slice(&wire).expect("deserialize start")
            else {
                panic!("start round trip");
            };
            assert_eq!(
                received.dap_id, dap_id,
                "the proxy must receive the editor's ID"
            );
            let terminal = CoreNotification::RunInTerminal {
                request: DebugTerminalRequest {
                    dap_id: received.dap_id,
                    generation: 3,
                    request_seq: 7,
                    arguments: ahead_rpc::dap_types::RunInTerminalArguments {
                        kind: Some("integrated".into()),
                        title: Some("Debug target".into()),
                        cwd: Some("/tmp".into()),
                        args: vec!["python".into(), "main.py".into()],
                        env: None,
                    },
                },
            };
            let CoreNotification::RunInTerminal { request: received } =
                serde_json::from_slice(
                    &serde_json::to_vec(&terminal).expect("serialize terminal"),
                )
                .expect("deserialize terminal")
            else {
                panic!("terminal round trip");
            };
            assert_eq!(received.dap_id, dap_id);
            assert_eq!(received.arguments.args, ["python", "main.py"]);
            assert!(updates.try_recv().is_ok());
            assert!(!client.dap_start(config.clone(), HashMap::new()));
            assert!(!client.dap_step_over(), "running is not stopped");
            assert!(notifications.try_recv().is_err());

            let stopped: ahead_rpc::dap_types::Stopped = serde_json::from_value(
                serde_json::json!({"reason": "breakpoint", "threadId": 42}),
            )
            .expect("stopped event");
            let thread_id = stopped.thread_id.expect("stopped thread");
            let frame: ahead_rpc::dap_types::StackFrame =
                serde_json::from_value(serde_json::json!({
                    "id": 7,
                    "name": "main",
                    "source": {"path": "/tmp/main.rs"},
                    "line": 12,
                    "column": 1
                }))
                .expect("stack frame");
            let scope: ahead_rpc::dap_types::Scope =
                serde_json::from_value(serde_json::json!({
                    "name": "Locals",
                    "variablesReference": 1,
                    "expensive": false
                }))
                .expect("scope");
            let variable: ahead_rpc::dap_types::Variable =
                serde_json::from_value(serde_json::json!({
                    "name": "count",
                    "value": "3",
                    "type": "i32",
                    "variablesReference": 0
                }))
                .expect("variable");
            client.route_core(CoreNotification::DapStopped {
                dap_id,
                stopped: stopped.clone(),
                stack_frames: HashMap::from([(thread_id, vec![frame.clone()])]),
                variables: vec![(scope.clone(), vec![variable.clone()])],
            });
            assert!(updates.try_recv().is_ok());
            assert_eq!(client.debug().frames, vec![frame]);
            assert_eq!(client.debug().scopes, vec![(scope, vec![variable])]);
            if let Some(previous) = previous {
                client.route_core(CoreNotification::DapSessionState {
                    dap_id: previous,
                    state: DapSessionState::Running,
                });
                assert_eq!(client.debug().state, DapSessionState::Stopped);
            }
            assert!(client.dap_continue());
            assert!(client.dap_step_over());
            assert!(client.dap_step_into());
            assert!(client.dap_step_out());
            for expected_method in [
                "dap_continue",
                "dap_step_over",
                "dap_step_into",
                "dap_step_out",
            ] {
                let super::ProxyRpc::Notification(notification) =
                    notifications.try_recv().expect("control notification")
                else {
                    panic!("control notification");
                };
                let wire =
                    serde_json::to_value(&notification).expect("control wire");
                assert_eq!(wire["method"], expected_method);
                assert_eq!(
                    wire["params"]["dap_id"],
                    serde_json::to_value(dap_id).expect("ID")
                );
                assert_eq!(wire["params"]["thread_id"], 42);
            }
            let path = PathBuf::from("src/main.rs");
            client.toggle_breakpoint(&path, 7 + iteration);
            let super::ProxyRpc::Notification(
                ProxyNotification::DapSetBreakpoints { dap_id: target, .. },
            ) = notifications.try_recv().expect("breakpoint notification")
            else {
                panic!("breakpoint notification");
            };
            assert_eq!(target, dap_id);
            if let Some(previous) = previous {
                let before = client.breakpoints_for(&path);
                client.route_core(CoreNotification::DapBreakpointsResp {
                    dap_id: previous,
                    path: path.clone(),
                    breakpoints: Vec::new(),
                });
                assert_eq!(client.breakpoints_for(&path), before);
            }
            assert_eq!(client.debug().thread_id, Some(thread_id));
            client.route_core(CoreNotification::DapSessionState {
                dap_id,
                state: DapSessionState::Running,
            });
            assert_eq!(client.debug().state, DapSessionState::Running);
            assert!(client.debug().thread_id.is_none());
            assert!(!client.dap_continue());
            assert!(client.dap_stop());
            assert!(matches!(
                notifications.try_recv().expect("stop notification"),
                super::ProxyRpc::Notification(ProxyNotification::DapStop { dap_id: target })
                    if target == dap_id
            ));
            assert!(!client.dap_stop());
            client.route_core(CoreNotification::DapStopped {
                dap_id,
                stopped,
                stack_frames: HashMap::new(),
                variables: Vec::new(),
            });
            assert_eq!(
                client.debug().state,
                DapSessionState::Stopping,
                "late events cannot revive a stopping session"
            );
            assert!(client.debug().thread_id.is_none());
            assert!(
                !client.dap_start(config.clone(), HashMap::new()),
                "wait for cleanup before a new start"
            );
            client.route_core(CoreNotification::DapSessionState {
                dap_id,
                state: DapSessionState::Terminated,
            });
            assert!(!client.debug().state.is_active());
            previous = Some(dap_id);
        }
        assert!(client.dap_start(config.clone(), HashMap::new()));
        client.route_core(CoreNotification::DapError {
            dap_id: client.debug().dap_id.expect("new session"),
            message: "Earlier command failure".into(),
        });
        client.read_proxy_messages(std::io::Cursor::new(Vec::<u8>::new()));
        assert!(!client.debug().state.is_active());
        assert!(
            client.debug().error.is_none(),
            "connection failure supersedes command errors"
        );
        assert!(client.debug().reason.contains("connection closed"));
        assert!(client.debug().connection_closed);
        assert!(!client.dap_start(config, HashMap::new()));
    }

    #[test]
    fn debugger_never_invents_a_stopped_thread_zero() {
        let client = test_client();
        assert!(
            client
                .dap_start(super::default_debug_configs().remove(0), HashMap::new())
        );
        let notifications = client.proxy_rpc.rx();
        notifications.try_recv().expect("start");
        let dap_id = client.debug().dap_id.expect("session ID");
        let thread_id =
            serde_json::from_value(serde_json::json!(17)).expect("thread ID");
        for all_threads_stopped in [false, true] {
            client.route_core(CoreNotification::DapStopped {
                dap_id,
                stopped: serde_json::from_value(serde_json::json!({
                    "reason": "pause", "allThreadsStopped": all_threads_stopped,
                }))
                .expect("stopped event"),
                stack_frames: HashMap::from([(thread_id, Vec::new())]),
                variables: Vec::new(),
            });
            assert_eq!(client.dap_step_over(), all_threads_stopped);
            if all_threads_stopped {
                assert!(matches!(
                    notifications.try_recv().expect("step request"),
                    super::ProxyRpc::Notification(ahead_rpc::proxy::ProxyNotification::DapStepOver {
                        thread_id: target, ..
                    }) if target == thread_id
                ));
            } else {
                assert!(notifications.try_recv().is_err());
            }
        }
    }

    #[test]
    fn malformed_proxy_reply_closes_connection_instead_of_leaving_setup_pending() {
        let client = test_client();
        let (sender, receiver) = bounded(1);
        client.proxy_rpc.ahead_request(
            super::AheadRequest::ListSessions,
            move |result| {
                sender.send(result).expect("report result");
            },
        );
        let invalid =
            serde_json::json!({"id": 0, "result": {"unexpected": "payload"}});
        let valid = serde_json::json!({
            "id": 0,
            "result": ProxyResponse::AheadResponse { response: serde_json::Value::Null },
        });
        client.read_proxy_messages(std::io::Cursor::new(format!(
            "{invalid}\n{valid}\n"
        )));
        assert!(
            receiver
                .try_recv()
                .expect("pending response")
                .expect_err("invalid frame closes transport")
                .message
                .contains("connection closed")
        );
        assert!(
            client
                .take_core_message()
                .expect("visible status")
                .contains("Restart AHEAD")
        );
    }

    #[test]
    fn core_message_reaches_the_status_queue() {
        let client = test_client();
        client.route_core(CoreNotification::ShowMessage {
            title: "Provider settings".into(),
            message: ShowMessageParams {
                typ: MessageType::WARNING,
                message: "Invalid settings.toml".into(),
            },
        });
        assert_eq!(
            client.take_core_message().as_deref(),
            Some("Provider settings: Invalid settings.toml")
        );
        assert!(client.take_core_message().is_none());
    }

    #[test]
    fn server_workspace_edit_requests_are_delivered_and_replied_to() {
        let client = test_client();
        let requests = client
            .subscribe_core_requests()
            .expect("one editor request subscriber");
        let params = lsp_types::ApplyWorkspaceEditParams {
            label: Some("Add import".into()),
            edit: lsp_types::WorkspaceEdit::default(),
        };
        let mut request =
            serde_json::to_value(CoreRequest::ApplyWorkspaceEdit(params))
                .expect("serialize core request");
        request["id"] = serde_json::json!(17);
        client.read_proxy_messages(std::io::Cursor::new(format!("{request}\n")));

        let event = requests.try_recv().expect("routed editor request");
        assert_eq!(event.request_id, 17);
        assert!(matches!(
            event.request,
            CoreRequest::ApplyWorkspaceEdit(params)
                if params.label.as_deref() == Some("Add import")
        ));

        client.respond_core_request(
            event.request_id,
            CoreResponse::ApplyWorkspaceEdit(
                lsp_types::ApplyWorkspaceEditResponse {
                    applied: false,
                    failure_reason: Some("stale document version".into()),
                    failed_change: None,
                },
            ),
        );
        let (reply_id, reply) = client
            .core_response_rx
            .try_recv()
            .expect("queued core response");
        assert_eq!(reply_id, 17);
        assert!(matches!(
            reply,
            CoreResponse::ApplyWorkspaceEdit(response)
                if !response.applied
                    && response.failure_reason.as_deref() == Some("stale document version")
        ));
    }

    #[test]
    fn git_credentials_are_transient_core_requests() {
        let client = test_client();
        let requests = client
            .subscribe_core_requests()
            .expect("one editor request subscriber");
        client.route_core_request(
            23,
            CoreRequest::GitCredentialPrompt {
                prompt: "Password for https://example.test".into(),
                secret: true,
            },
        );

        let event = requests.try_recv().expect("Git credential prompt");
        assert_eq!(event.request_id, 23);
        assert!(matches!(
            event.request,
            CoreRequest::GitCredentialPrompt { prompt, secret: true }
                if prompt == "Password for https://example.test"
        ));

        client.respond_core_request(
            event.request_id,
            CoreResponse::GitCredentialAnswer {
                answer: Some("disposable-test-token".into()),
            },
        );
        let (reply_id, reply) = client
            .core_response_rx
            .try_recv()
            .expect("queued Git credential reply");
        assert_eq!(reply_id, 23);
        assert!(matches!(
            reply,
            CoreResponse::GitCredentialAnswer {
                answer: Some(answer)
            } if answer == "disposable-test-token"
        ));
    }

    #[test]
    fn external_config_update_replaces_complete_session_state() {
        let client = test_client();
        let option = AgentConfigOption {
            id: "model".to_string(),
            name: "Model".to_string(),
            category: Some("model".to_string()),
            current_value: AgentConfigOptionValue::Select("fast".to_string()),
            choices: vec![AgentConfigChoice {
                value: "fast".to_string(),
                name: "Fast".to_string(),
            }],
        };
        client.route_ahead(AheadNotification::AgentConfigOptionsAvailable {
            session_id: "session".to_string(),
            options: vec![option.clone()],
        });
        assert_eq!(client.config_options("session"), vec![option]);
        client.route_ahead(AheadNotification::AgentConfigOptionsAvailable {
            session_id: "session".to_string(),
            options: Vec::new(),
        });
        assert!(client.config_options("session").is_empty());
    }

    #[test]
    fn external_auth_methods_are_cached_until_the_agent_clears_them() {
        let client = test_client();
        let method = AgentAuthMethod {
            id: "terminal-login".to_string(),
            name: "Sign in in terminal".to_string(),
            description: Some("Complete sign-in in the integrated terminal".into()),
            method_type: ahead_rpc::ahead::AgentAuthMethodType::Terminal,
        };
        client.route_ahead(AheadNotification::AgentAuthenticationRequired {
            session_id: "session".to_string(),
            methods: vec![method.clone()],
        });
        assert_eq!(client.agent_auth_methods("session"), vec![method]);

        client.route_ahead(AheadNotification::AgentAuthenticationCleared {
            session_id: "session".to_string(),
        });
        assert!(client.agent_auth_methods("session").is_empty());
    }

    #[test]
    fn workspace_change_notifies_for_content_without_reindexing() {
        let client = test_client();
        let receiver = client.subscribe_workspace_file_changes();
        let event_receiver = client.subscribe_workspace_file_events();
        let path = PathBuf::from("src/main.rs");
        client.route_core(CoreNotification::WorkspaceFileChange {
            generation: 2,
            paths: vec![path.clone()],
        });
        assert_eq!(client.workspace_file_generation(), 2);
        assert_eq!(receiver.try_recv(), Ok(vec![path.clone()]));
        assert_eq!(
            event_receiver.try_recv(),
            Ok(WorkspaceFileEvent::Changed(vec![path.clone()]))
        );
        client.route_core(CoreNotification::WorkspaceFileChange {
            generation: 2,
            paths: vec![path.clone()],
        });
        assert_eq!(receiver.try_recv(), Ok(vec![path]));
        assert!(matches!(
            event_receiver.try_recv(),
            Ok(WorkspaceFileEvent::Changed(_))
        ));
        client.route_core(CoreNotification::WorkspaceFileChange {
            generation: 1,
            paths: Vec::new(),
        });
        assert!(receiver.try_recv().is_err());
        assert!(event_receiver.try_recv().is_err());
    }

    #[test]
    fn workspace_rename_is_delivered_as_an_ordered_event() {
        let client = test_client();
        let receiver = client.subscribe_workspace_file_events();
        let old_path = PathBuf::from("src/old.rs");
        let new_path = PathBuf::from("src/new.rs");
        client.route_core(CoreNotification::WorkspaceFileRename {
            generation: 1,
            old_path: old_path.clone(),
            new_path: new_path.clone(),
        });

        assert_eq!(
            receiver.try_recv(),
            Ok(WorkspaceFileEvent::Renamed { old_path, new_path })
        );
    }

    #[test]
    fn open_file_change_notifies_only_open_buffer_subscribers() {
        let client = test_client();
        let workspace_receiver = client.subscribe_workspace_file_changes();
        let open_file_receiver = client.subscribe_open_file_changes();
        let path = PathBuf::from("ignored/generated.rs");
        client.route_core(CoreNotification::OpenFileChanged {
            path: path.clone(),
            content: ahead_rpc::core::FileChanged::Change("latest".into()),
        });
        assert_eq!(open_file_receiver.try_recv(), Ok(path));
        assert!(workspace_receiver.try_recv().is_err());
    }

    #[test]
    fn diagnostic_updates_notify_editor_subscribers() {
        let client = test_client();
        let receiver = client.subscribe_diagnostics();
        client.route_core(CoreNotification::PublishDiagnostics {
            diagnostics: lsp_types::PublishDiagnosticsParams {
                uri: lsp_types::Url::parse("file:///private/tmp/ahead-test.rs")
                    .expect("valid test URI"),
                diagnostics: Vec::new(),
                version: None,
            },
        });
        assert_eq!(receiver.try_recv(), Ok(()));
        assert_eq!(
            client.diagnostics_for(&PathBuf::from("/private/tmp/ahead-test.rs")),
            Some(Vec::new())
        );
    }

    #[test]
    fn repair_language_server_targets_the_named_managed_server() {
        let client = test_client();
        client.repair_language_server("vtsls");
        assert!(matches!(
            client.proxy_rpc.rx().try_recv(),
            Ok(super::ProxyRpc::Notification(
                ahead_rpc::proxy::ProxyNotification::RepairLanguageServer {
                    server_id,
                }
            )) if server_id == "vtsls"
        ));
    }

    #[test]
    fn successful_extension_install_reloads_without_a_settings_view() {
        use ahead_rpc::proxy::{ProxyNotification, ProxyRequest, ProxyRpc};

        let client = test_client();
        client.install_extension(
            "https://example.invalid/html.tar.gz".into(),
            "html".into(),
            "0.3.2".into(),
            Some("a".repeat(64)),
            |_| {},
        );
        let ProxyRpc::Request(
            request_id,
            ProxyRequest::InstallLanguageExtension {
                extension_id,
                version,
                expected_sha256,
                ..
            },
        ) = client.proxy_rpc.rx().try_recv().expect("install request")
        else {
            panic!("expected language-extension install request");
        };
        assert_eq!(extension_id, "html");
        assert_eq!(version, "0.3.2");
        assert_eq!(expected_sha256.as_deref(), Some("a".repeat(64).as_str()));
        client
            .proxy_rpc
            .handle_response(request_id, Ok(ProxyResponse::Success {}));
        let ProxyRpc::Notification(ProxyNotification::RestartLanguageServers {
            extension_id,
        }) = client
            .proxy_rpc
            .rx()
            .try_recv()
            .expect("restart notification")
        else {
            panic!("expected extension-scoped restart");
        };
        assert_eq!(extension_id.as_deref(), Some("html"));
    }

    #[test]
    fn language_server_status_wakes_views_and_restart_uses_the_shared_proxy() {
        let client = test_client();
        let updates = client.subscribe_diagnostics();
        let syntax_generation = client.extension_syntax_generation();
        client.route_core(CoreNotification::LanguageExtensionIssues {
            issues: Vec::new(),
        });
        assert_eq!(client.extension_syntax_generation(), syntax_generation + 1);
        assert_eq!(updates.try_recv(), Ok(()));
        let mut installing =
            ahead_rpc::core::ServerStatusParams::starting("vtsls".into());
        installing.message = Some("Installing from npm".into());
        for params in [
            installing,
            ahead_rpc::core::ServerStatusParams::ready("vtsls".into())
                .with_on_type_formatting_triggers(vec!["}".into()]),
            ahead_rpc::core::ServerStatusParams::failed(
                "vtsls".into(),
                "process exited".into(),
            ),
        ] {
            let ready = params.is_ok();
            let error = !ready && params.is_quiescent();
            client.route_core(CoreNotification::ServerStatus { params });
            assert_eq!(updates.try_recv(), Ok(()));
            assert_eq!(client.lsp_servers().len(), 1);
            assert_eq!(client.lsp_servers()[0].is_ready(), ready);
            assert_eq!(client.lsp_servers()[0].is_error(), error);
            assert_eq!(client.supports_on_type_formatting("}"), ready);
            assert!(!client.supports_on_type_formatting(";"));
        }
        client.restart_language_servers();
        assert!(matches!(
            client
                .proxy_rpc
                .rx()
                .try_recv()
                .expect("restart notification"),
            ahead_rpc::proxy::ProxyRpc::Notification(
                ahead_rpc::proxy::ProxyNotification::RestartLanguageServers {
                    extension_id: None,
                }
            )
        ));
        client.route_core(CoreNotification::LanguageServerStatusesCleared);
        assert_eq!(updates.try_recv(), Ok(()));
        assert!(client.lsp_servers().is_empty());
        client.route_core(CoreNotification::ServerStatus {
            params: ahead_rpc::core::ServerStatusParams::ready("vtsls".into()),
        });
        updates.try_recv().expect("ready update");
        client.proxy_disconnected();
        assert_eq!(updates.try_recv(), Ok(()));
        let servers = client.lsp_servers();
        assert!(!servers[0].is_ready());
        assert!(
            servers[0]
                .message
                .as_ref()
                .expect("failure reason")
                .contains("proxy connection closed")
        );
    }

    #[test]
    fn extension_discovery_issues_do_not_appear_as_language_servers() {
        use ahead_rpc::core::LanguageExtensionIssue;

        let client = test_client();
        let updates = client.subscribe_diagnostics();
        let issue = LanguageExtensionIssue {
            name: "broken-extension".into(),
            message: "bad manifest".into(),
        };
        client.route_core(CoreNotification::LanguageExtensionIssues {
            issues: vec![issue.clone()],
        });
        assert_eq!(updates.try_recv(), Ok(()));
        assert_eq!(client.language_extension_issues(), vec![issue]);
        assert!(client.lsp_servers().is_empty());

        client.route_core(CoreNotification::LanguageExtensionIssues {
            issues: Vec::new(),
        });
        assert_eq!(updates.try_recv(), Ok(()));
        assert!(client.language_extension_issues().is_empty());
    }

    #[test]
    fn language_server_schemas_are_cached_and_cleared_with_server_statuses() {
        let client = test_client();
        let updates = client.subscribe_diagnostics();
        let schemas = ahead_rpc::core::LanguageServerSchemas {
            initialization_options: Some(serde_json::json!({
                "type": "object",
                "properties": { "enabled": { "type": "boolean" } }
            })),
            settings: Some(serde_json::json!({
                "type": "object",
                "properties": { "mode": { "enum": ["safe", "strict"] } }
            })),
        };

        client.route_core(CoreNotification::LanguageServerSchemas {
            server_id: "example-lsp".into(),
            schemas: schemas.clone(),
        });

        assert_eq!(updates.try_recv(), Ok(()));
        assert_eq!(
            client.language_server_schemas().get("example-lsp"),
            Some(&schemas)
        );

        client.route_core(CoreNotification::LanguageServerStatusesCleared);
        assert_eq!(updates.try_recv(), Ok(()));
        assert!(client.language_server_schemas().is_empty());
    }

    #[test]
    fn queues_live_buffer_request_only_for_active_turn() {
        let client = test_client();
        client.route_ahead(ahead_rpc::ahead::AheadNotification::AgentTurnState {
            session_id: "session".into(),
            turn_id: "active".into(),
            message_id: "message".into(),
            state: "streaming".into(),
        });
        assert!(client.is_streaming("session"));
        for turn_id in ["stale", "active"] {
            let include_document_symbols = turn_id == "active";
            client.route_ahead(
                ahead_rpc::ahead::AheadNotification::AgentBufferSnapshotsRequested {
                    session_id: "session".into(),
                    turn_id: turn_id.into(),
                    request_id: format!("request-{turn_id}"),
                    include_document_symbols,
                },
            );
        }
        let requests = client.take_buffer_snapshot_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].request_id, "request-active");
        assert!(requests[0].include_document_symbols);
        assert!(client.take_buffer_snapshot_requests().is_empty());

        client.route_ahead(ahead_rpc::ahead::AheadNotification::AgentTurnState {
            session_id: "session".into(),
            turn_id: "active".into(),
            message_id: "message".into(),
            state: "completed".into(),
        });
        assert!(!client.is_streaming("session"));
    }

    #[test]
    fn terminal_turn_clears_transient_thinking() {
        let client = test_client();
        for state in ["completed", "cancelled", "failed"] {
            let turn_id = format!("turn-{state}");
            client.route_ahead(AheadNotification::AgentTurnState {
                session_id: "session".into(),
                turn_id: turn_id.clone(),
                message_id: "message".into(),
                state: "streaming".into(),
            });
            client.route_ahead(AheadNotification::AgentThoughtDelta {
                session_id: "session".into(),
                turn_id: turn_id.clone(),
                delta: "Stream retry: Reconnecting".into(),
            });
            assert!(!client.thought("session").is_empty());
            client.route_ahead(AheadNotification::AgentTurnState {
                session_id: "session".into(),
                turn_id,
                message_id: "message".into(),
                state: state.into(),
            });
            assert!(client.thought("session").is_empty(), "{state}");
        }
    }

    #[test]
    fn derives_durable_harness_metadata_from_the_stored_backend() {
        assert_eq!(harness_metadata(None), (HarnessKind::Ahead, None));
        assert_eq!(
            harness_metadata(Some("ahead-pending")),
            (HarnessKind::Ahead, None)
        );
        assert_eq!(
            harness_metadata(Some("external-agent-pending")),
            (HarnessKind::ExternalAcp, None)
        );
        assert_eq!(
            harness_metadata(Some("external-agent:pi-acp-fresh")),
            (HarnessKind::ExternalAcp, Some("pi-acp".to_string()))
        );
    }

    #[test]
    fn routes_proxy_response_to_pending_callback() {
        let rpc = ProxyRpcHandler::new();
        let (tx, rx) = bounded(1);
        rpc.get_hover(
            7,
            PathBuf::from("src/lib.rs"),
            Position::new(0, 0),
            move |result| {
                tx.send(result).unwrap();
            },
        );

        let response = ProxyResponse::HoverResponse {
            request_id: 7,
            hover: lsp_types::Hover {
                contents: HoverContents::Scalar(MarkedString::String("ok".into())),
                range: None,
            },
        };
        route_proxy_response(&rpc, 0, serde_json::to_value(response).unwrap())
            .expect("valid response");

        let response = rx.recv().unwrap().unwrap();
        let ProxyResponse::HoverResponse { hover, .. } = response else {
            panic!("expected hover response");
        };
        assert!(
            matches!(&hover.contents, HoverContents::Scalar(MarkedString::String(text)) if text == "ok")
        );
    }

    #[test]
    fn publishes_checkpoint_files_atomically() {
        let directory = std::env::temp_dir()
            .join(format!("ahead-checkpoint-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("session.json");
        write_atomic(&path, br#"{"format_version":"ahead.editor/v0-draft"}"#)
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"format_version":"ahead.editor/v0-draft"}"#
        );
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn updates_tool_calls_by_runtime_id() {
        let mut calls = Vec::new();
        upsert_agent_tool_call(
            &mut calls,
            AgentToolCall {
                id: "call-1".into(),
                title: "cargo check".into(),
                status: "in_progress".into(),
                kind: "commandExecution".into(),
            },
        );
        upsert_agent_tool_call(
            &mut calls,
            AgentToolCall {
                id: "call-1".into(),
                title: String::new(),
                status: "completed".into(),
                kind: String::new(),
            },
        );

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].title, "cargo check");
        assert_eq!(calls[0].status, "completed");
    }

    #[test]
    fn patch_previews_upsert_by_call_and_clear_for_new_turns() {
        let client = test_client();
        client.route_ahead(AheadNotification::AgentTurnState {
            session_id: "session".into(),
            turn_id: "turn-1".into(),
            message_id: "message-1".into(),
            state: "streaming".into(),
        });
        let preview = |diff: &str| AgentPatchPreview {
            call_id: "patch-1".into(),
            files: vec![ahead_rpc::ahead::AgentPatchFilePreview {
                path: "src/lib.rs".into(),
                diff: diff.into(),
            }],
            truncated: false,
        };

        client.route_ahead(AheadNotification::AgentPatchProgress {
            session_id: "session".into(),
            turn_id: "turn-1".into(),
            preview: preview("-old\n+new"),
        });
        client.route_ahead(AheadNotification::AgentPatchProgress {
            session_id: "session".into(),
            turn_id: "turn-1".into(),
            preview: preview("-old\n+newer"),
        });
        client.route_ahead(AheadNotification::AgentPatchProgress {
            session_id: "session".into(),
            turn_id: "stale-turn".into(),
            preview: preview("stale"),
        });

        let previews = client.patch_previews("session");
        assert_eq!(previews.len(), 1);
        assert_eq!(previews[0].files[0].diff, "-old\n+newer");

        client.route_ahead(AheadNotification::AgentTurnState {
            session_id: "session".into(),
            turn_id: "turn-1".into(),
            message_id: "message-1".into(),
            state: "completed".into(),
        });
        client.route_ahead(AheadNotification::AgentTurnState {
            session_id: "session".into(),
            turn_id: "turn-2".into(),
            message_id: "message-2".into(),
            state: "streaming".into(),
        });
        assert!(client.patch_previews("session").is_empty());
    }

    #[test]
    fn failed_turn_keeps_its_harness_error_visible_in_chat() {
        let client = test_client();
        client.route_ahead(AheadNotification::ConversationMessageAdded {
            message: ConversationMessage {
                id: "agent-message".into(),
                session_id: "session".into(),
                turn_id: "turn".into(),
                sequence: 1,
                role: "agent".into(),
                actor_id: "ahead".into(),
                content: String::new(),
                status: "streaming".into(),
                created_at: "2026-10-01T00:00:00Z".into(),
                voice_timing: None,
            },
        });
        client.route_ahead(AheadNotification::AgentTurnState {
            session_id: "session".into(),
            turn_id: "turn".into(),
            message_id: "agent-message".into(),
            state: "streaming".into(),
        });
        client.route_ahead(AheadNotification::AgentMessageDelta {
            session_id: "session".into(),
            turn_id: "turn".into(),
            delta: "\n\nHarness error: AHEAD agent history was not saved".into(),
        });
        client.route_ahead(AheadNotification::AgentTurnState {
            session_id: "session".into(),
            turn_id: "turn".into(),
            message_id: "agent-message".into(),
            state: "failed".into(),
        });

        let conversations = client.conversations.lock().expect("chat state");
        let message = &conversations["session"].messages[0];
        assert_eq!(message.status, "failed");
        assert!(message.content.contains("history was not saved"));
    }

    #[test]
    fn ignores_stale_turn_events_after_a_new_turn_starts() {
        let client = test_client();
        let usage_updates = client.subscribe_usage_updates();
        client
            .streaming_turns
            .lock()
            .unwrap()
            .insert("session".into(), "current".into());

        client.route_ahead(AheadNotification::AgentTurnState {
            session_id: "session".into(),
            turn_id: "stale".into(),
            message_id: "stale-message".into(),
            state: "streaming".into(),
        });
        client.route_ahead(AheadNotification::AgentMessageDelta {
            session_id: "session".into(),
            turn_id: "stale".into(),
            delta: "late text".into(),
        });
        client.route_ahead(AheadNotification::AgentThoughtDelta {
            session_id: "session".into(),
            turn_id: "stale".into(),
            delta: "late thought".into(),
        });
        client.route_ahead(AheadNotification::AgentContextCompacted {
            session_id: "session".into(),
            turn_id: "stale".into(),
        });
        client.route_ahead(AheadNotification::AgentPlan {
            session_id: "session".into(),
            turn_id: "stale".into(),
            entries: vec![AgentPlanEntry {
                content: "old plan".into(),
                status: "in_progress".into(),
                priority: "normal".into(),
            }],
        });
        client.route_ahead(AheadNotification::AgentToolCall {
            session_id: "session".into(),
            turn_id: "stale".into(),
            call: AgentToolCall {
                id: "old-call".into(),
                title: "old tool".into(),
                status: "in_progress".into(),
                kind: "commandExecution".into(),
            },
        });
        client.route_ahead(AheadNotification::AgentUsage {
            session_id: "session".into(),
            turn_id: "stale".into(),
            usage: AgentUsage {
                used_tokens: 42,
                context_window: Some(100),
            },
        });
        client.route_ahead(AheadNotification::AgentUserInputRequested {
            session_id: "session".into(),
            turn_id: "stale".into(),
            request: AgentUserInputRequest {
                request_id: "old-input".into(),
                is_blocking: true,
                questions: Vec::new(),
            },
        });

        assert_eq!(client.active_turn_id("session").as_deref(), Some("current"));
        assert!(client.conversations.lock().unwrap().is_empty());
        assert!(client.thought("session").is_empty());
        assert!(client.plan("session").is_empty());
        assert!(client.tool_calls("session").is_empty());
        assert!(client.usage("session").is_none());
        assert!(usage_updates.try_recv().is_err());
        assert!(client.pending_user_input("session").is_none());
    }

    #[test]
    fn accepted_agent_usage_updates_the_cache_and_notifies_subscribers() {
        let client = test_client();
        let usage_updates = client.subscribe_usage_updates();
        client.route_ahead(AheadNotification::AgentTurnState {
            session_id: "session".into(),
            turn_id: "turn".into(),
            message_id: "message".into(),
            state: "streaming".into(),
        });

        client.route_ahead(AheadNotification::AgentUsage {
            session_id: "session".into(),
            turn_id: "turn".into(),
            usage: AgentUsage {
                used_tokens: 81_000,
                context_window: Some(1_050_000),
            },
        });

        assert_eq!(usage_updates.try_recv(), Ok(()));
        assert_eq!(
            client.usage("session"),
            Some(AgentUsage {
                used_tokens: 81_000,
                context_window: Some(1_050_000),
            })
        );
    }

    #[test]
    fn parallel_agent_input_requests_remain_queued_until_each_is_answered() {
        use ahead_rpc::{
            ahead::AheadRequest,
            proxy::{ProxyRequest, ProxyResponse, ProxyRpc},
        };

        let client = test_client();
        client
            .streaming_turns
            .lock()
            .unwrap()
            .insert("session".into(), "turn".into());
        for request_id in ["first", "second", "second"] {
            client.route_ahead(AheadNotification::AgentUserInputRequested {
                session_id: "session".into(),
                turn_id: "turn".into(),
                request: AgentUserInputRequest {
                    request_id: request_id.into(),
                    is_blocking: true,
                    questions: Vec::new(),
                },
            });
        }
        assert_eq!(client.pending_user_input_count("session"), 2);
        assert_eq!(
            client.pending_user_input("session").unwrap().request_id,
            "first"
        );
        let outgoing = client.proxy_rpc.rx().clone();
        let responding_client = client.clone();
        let responder = std::thread::spawn(move || {
            let ProxyRpc::Request(
                id,
                ProxyRequest::AheadRequest {
                    request:
                        AheadRequest::AgentUserInputAnswer {
                            session_id,
                            request_id,
                            ..
                        },
                },
            ) = outgoing
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("answer request")
            else {
                panic!("expected an agent input answer request");
            };
            assert_eq!(session_id, "session");
            assert_eq!(request_id, "first");
            responding_client.proxy_rpc.handle_response(
                id,
                Ok(ProxyResponse::AheadResponse {
                    response: serde_json::json!({}),
                }),
            );
        });
        client
            .answer_agent_user_input("session", "first", HashMap::new())
            .expect("answer first request");
        responder.join().expect("answer responder");
        assert_eq!(client.pending_user_input_count("session"), 1);
        assert_eq!(
            client.pending_user_input("session").unwrap().request_id,
            "second"
        );
        client.route_ahead(AheadNotification::AgentUserInputCancelled {
            session_id: "session".into(),
            turn_id: "turn".into(),
            request_id: "second".into(),
        });
        assert!(client.pending_user_input("session").is_none());
        client.route_ahead(AheadNotification::AgentTurnState {
            session_id: "session".into(),
            turn_id: "turn".into(),
            message_id: "agent-message".into(),
            state: "completed".into(),
        });
        assert!(client.pending_user_input("session").is_none());
    }

    #[test]
    fn project_debug_config_parses_launch_and_adapter_settings() {
        let configs = super::parse_project_debug_configs(
            r#"
[[configurations]]
name = "Python main"
type = "python"
debug-adapter = "python"
debug-adapter-args = ["-m", "debugpy.adapter"]
program = "main.py"
args = ["--verbose"]
cwd = "."

[configurations.env]
APP_MODE = "test"

[[configurations]]
name = "Node tests"
type = "node"
debug-adapter = "js-debug-adapter"
program = "tests/run.mjs"
"#,
        )
        .expect("typed project launch configurations");

        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].name, "Python main");
        assert_eq!(configs[0].debug_adapter.as_deref(), Some("python"));
        let expected_adapter_args =
            vec!["-m".to_string(), "debugpy.adapter".to_string()];
        assert_eq!(
            configs[0].debug_adapter_args.as_ref(),
            Some(&expected_adapter_args)
        );
        assert_eq!(configs[0].program, "main.py");
        assert_eq!(
            configs[0]
                .env
                .as_ref()
                .and_then(|environment| environment.get("APP_MODE"))
                .map(String::as_str),
            Some("test")
        );
        assert_eq!(configs[1].name, "Node tests");
    }

    #[test]
    fn project_debug_config_rejects_missing_adapter_commands() {
        let error = super::parse_project_debug_configs(
            r#"
[[configurations]]
name = "Unconfigured"
program = "main.py"
"#,
        )
        .expect_err("project configs must select an adapter");

        assert!(error.contains("debug-adapter"));
    }

    #[test]
    fn refresh_debug_configs_resolves_relative_working_directories() {
        let workspace = tempfile::tempdir().expect("workspace");
        let ahead_directory = workspace.path().join(".ahead");
        std::fs::create_dir_all(&ahead_directory).expect("create .ahead");
        std::fs::write(
            ahead_directory.join("run.toml"),
            r#"
[[configurations]]
name = "Python main"
debug-adapter = "python"
program = "main.py"
cwd = "."
"#,
        )
        .expect("write project config");

        let client = ProxyClient::allocate(workspace.path().to_path_buf());
        client.refresh_debug_configs();

        let configs = client.stored_configs();
        assert_eq!(configs.len(), 1);
        let cwd = configs[0]
            .cwd
            .as_deref()
            .expect("resolved working directory");
        assert_eq!(std::path::Path::new(cwd), workspace.path());
        assert!(client.debug_config_error().is_none());
    }
}
