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
    RpcError, RpcMessage,
    ahead::{
        AgentBufferSnapshot, AgentCommand, AgentConfigOption,
        AgentConfigOptionValue, AgentPlanEntry, AgentPresentationAction,
        AgentRuntimeState, AgentToolCall, AgentTurnRequestDto, AgentUsage,
        AgentUserInputRequest, AheadNotification, AheadRequest, CodeAnchor,
        CodeComment, ConversationMessage, ConversationMessageCursor,
        ConversationMessagePage, ExternalAcpAdapter, HarnessKind,
        McpServerDeclaration, MemoryDocument, MemoryExcerpt, MemoryScope,
        MemoryWriteResult, SessionExportBundle, SessionListItem, SessionView,
        WorkItem, WorkItemStatus,
    },
    core::{CoreNotification, CoreRequest, CoreResponse},
    dap_types::{
        DapId, DapSessionState, DebugTerminalRequest, DebugTerminalResponse,
        RunDebugConfig, SourceBreakpoint, StackFrame, Stopped, ThreadId,
    },
    plugin::PluginId,
    proxy::{
        ProxyMessage, ProxyRequest, ProxyResponse, ProxyRpc, ProxyRpcHandler,
        WorkspaceFilesRequest,
    },
    source_control::{DiffInfo, FileDiff, GitFileState},
    stdio::{read_msg, write_msg},
};
use crossbeam_channel::{Receiver, Sender, unbounded};
use lsp_types::{
    CompletionItem, CompletionResponse, Diagnostic, GotoDefinitionResponse, Hover,
    InlineCompletionResponse, Location, Position,
};

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

#[derive(Clone, Debug)]
pub struct LspCompletion {
    pub plugin_id: PluginId,
    pub item: CompletionItem,
}

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
}

impl LspServerStatus {
    pub fn is_ready(&self) -> bool {
        self.ready
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
    pub harness_warning: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct CachedConversation {
    messages: Vec<ConversationMessage>,
    has_older: bool,
    loaded_page: bool,
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
    pub breakpoints: HashMap<PathBuf, HashSet<u32>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferSnapshotRequest {
    pub session_id: String,
    pub turn_id: String,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorPresentationRequest {
    pub session_id: String,
    pub turn_id: String,
    pub request_id: String,
    pub action: AgentPresentationAction,
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

pub struct ProxyClient {
    proxy_rpc: ProxyRpcHandler,
    pending_completion: Arc<Mutex<HashMap<usize, Sender<Vec<LspCompletion>>>>>,
    pending_hover: Arc<Mutex<HashMap<usize, Sender<Hover>>>>,
    pending_defs: Arc<Mutex<HashMap<usize, Sender<Vec<Location>>>>>,
    pending_inline: Arc<Mutex<HashMap<PathBuf, Sender<String>>>>,
    request_seq: AtomicUsize,
    diagnostics: Arc<Mutex<HashMap<PathBuf, ProxyDiagnostics>>>,
    diag_version: Arc<Mutex<usize>>,
    diagnostic_subscribers: Mutex<Vec<async_channel::Sender<()>>>,
    diff: Arc<Mutex<ProxyDiffState>>,
    git_generation: AtomicU64,
    lsp_servers: Arc<Mutex<HashMap<String, LspServerStatus>>>,
    /// Uncommitted attribution anchors per file, refreshed from the ahead host.
    anchors: Arc<Mutex<HashMap<PathBuf, Vec<CodeAnchor>>>>,
    anchor_requests: Mutex<HashMap<PathBuf, usize>>,
    debug: Arc<Mutex<DebugState>>,
    debug_configs: Arc<Mutex<Vec<RunDebugConfig>>>,
    debug_terminal_tx: async_channel::Sender<DebugTerminalEvent>,
    debug_terminal_rx: async_channel::Receiver<DebugTerminalEvent>,
    /// Durable conversation messages for the active session, keyed by session.
    conversations: Arc<Mutex<HashMap<String, CachedConversation>>>,
    plans: Arc<Mutex<HashMap<String, Vec<AgentPlanEntry>>>>,
    tool_calls: Arc<Mutex<HashMap<String, Vec<AgentToolCall>>>>,
    thoughts: Arc<Mutex<HashMap<String, String>>>,
    available_commands: Arc<Mutex<HashMap<String, Vec<AgentCommand>>>>,
    config_options: Arc<Mutex<HashMap<String, Vec<AgentConfigOption>>>>,
    usage: Arc<Mutex<HashMap<String, AgentUsage>>>,
    pending_user_inputs: Arc<Mutex<HashMap<String, AgentUserInputRequest>>>,
    pending_buffer_snapshot_requests: Arc<Mutex<Vec<BufferSnapshotRequest>>>,
    pending_editor_presentation_requests: Arc<Mutex<Vec<EditorPresentationRequest>>>,
    /// In-flight streamed turns, keyed by session id.
    streaming_turns: Arc<Mutex<HashMap<String, String>>>,
    workspace_file_generation: AtomicU64,
    workspace_file_subscribers: Mutex<Vec<async_channel::Sender<()>>>,
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
        Arc::new(Self {
            proxy_rpc,
            pending_completion: Arc::new(Mutex::new(HashMap::new())),
            pending_hover: Arc::new(Mutex::new(HashMap::new())),
            pending_defs: Arc::new(Mutex::new(HashMap::new())),
            pending_inline: Arc::new(Mutex::new(HashMap::new())),
            request_seq: AtomicUsize::new(1),
            diagnostics: Arc::new(Mutex::new(HashMap::new())),
            diag_version: Arc::new(Mutex::new(0)),
            diagnostic_subscribers: Mutex::new(Vec::new()),
            diff: Arc::new(Mutex::new(ProxyDiffState::default())),
            git_generation: AtomicU64::new(0),
            lsp_servers: Arc::new(Mutex::new(HashMap::new())),
            anchors: Arc::new(Mutex::new(HashMap::new())),
            anchor_requests: Mutex::new(HashMap::new()),
            debug: Arc::new(Mutex::new(DebugState::default())),
            debug_configs: Arc::new(Mutex::new(default_debug_configs())),
            debug_terminal_tx,
            debug_terminal_rx,
            conversations: Arc::new(Mutex::new(HashMap::new())),
            plans: Arc::new(Mutex::new(HashMap::new())),
            tool_calls: Arc::new(Mutex::new(HashMap::new())),
            thoughts: Arc::new(Mutex::new(HashMap::new())),
            available_commands: Arc::new(Mutex::new(HashMap::new())),
            config_options: Arc::new(Mutex::new(HashMap::new())),
            usage: Arc::new(Mutex::new(HashMap::new())),
            pending_user_inputs: Arc::new(Mutex::new(HashMap::new())),
            pending_buffer_snapshot_requests: Arc::new(Mutex::new(Vec::new())),
            pending_editor_presentation_requests: Arc::new(Mutex::new(Vec::new())),
            streaming_turns: Arc::new(Mutex::new(HashMap::new())),
            workspace_file_generation: AtomicU64::new(0),
            workspace_file_subscribers: Mutex::new(Vec::new()),
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
            .stderr(Stdio::null())
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
        // App -> proxy: drain ProxyRpcHandler::rx() and frame onto stdin.
        let app_tx = proxy_rpc.clone();
        let writer_client = client.clone();
        std::thread::spawn(move || {
            let mut writer = BufWriter::new(stdin);
            for msg in app_tx.rx() {
                let wire: ProxyMessage = match msg {
                    ProxyRpc::Request(id, req) => RpcMessage::Request(id, req),
                    ProxyRpc::Notification(n) => RpcMessage::Notification(n),
                    ProxyRpc::Shutdown => break,
                };
                if let Err(error) = write_msg(&mut writer, wire) {
                    eprintln!("Could not write to AHEAD proxy: {error}");
                    writer_client.proxy_disconnected();
                    break;
                }
            }
        });
        // Proxy -> app: core notifications and proxy request responses share
        // this stdio stream. Keep response payloads as JSON until we know
        // which protocol owns them; CoreResponse is uninhabited.
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
                RpcMessage::Request(_id, _req) => {}
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
        self.proxy_rpc.disconnect();
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
            debug.frames.clear();
            debug.reason = "AHEAD proxy connection closed".into();
        }
        if let Some(dap_id) = retired_debugger {
            drop(
                self.debug_terminal_tx
                    .try_send(DebugTerminalEvent::Retire(dap_id)),
            );
        }
        *self.core_message.lock().unwrap_or_else(|error| error.into_inner()) = Some(
            "AHEAD proxy connection closed. Restart AHEAD to restore stored sessions before retrying; an in-flight change may already have completed.".into(),
        );
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

    pub fn restart_language_servers(&self) {
        self.proxy_rpc.notification(
            ahead_rpc::proxy::ProxyNotification::RestartLanguageServers {},
        );
    }

    pub fn install_language_extension(
        &self,
        url: String,
        extension_id: String,
        callback: impl FnOnce(Result<(), String>) + Send + 'static,
    ) {
        self.proxy_rpc.install_language_extension(
            url,
            extension_id,
            move |result| {
                callback(match result {
                    Ok(ahead_rpc::proxy::ProxyResponse::Success {}) => Ok(()),
                    Ok(response) => Err(format!(
                        "unexpected language-extension response: {response:?}"
                    )),
                    Err(error) => Err(error.message),
                });
            },
        );
    }

    pub fn diff(&self) -> ProxyDiffState {
        self.diff.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn git_generation(&self) -> u64 {
        self.git_generation.load(Ordering::Acquire)
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

    pub fn debug(&self) -> DebugState {
        self.debug.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn stored_configs(&self) -> Vec<RunDebugConfig> {
        self.debug_configs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn refresh_debug_configs(&self) {
        let run_toml = self.workspace.join(".ahead").join("run.toml");
        if let Ok(text) = std::fs::read_to_string(&run_toml) {
            let mut configs = default_debug_configs();
            for line in text.lines() {
                let line = line.trim().trim_matches('"').trim_matches('\'');
                if line.ends_with(".rs") || line.contains("target/debug") {
                    configs.push(RunDebugConfig {
                        ty: Some("lldb".into()),
                        debug_adapter: Some(default_debug_adapter()),
                        debug_adapter_args: None,
                        name: format!("Run {line}"),
                        program: line.to_string(),
                        args: None,
                        cwd: None,
                        env: None,
                        prelaunch: None,
                        dap_id: DapId(0),
                        tracing_output: false,
                        config_source: Default::default(),
                    });
                }
            }
            *self.debug_configs.lock().unwrap_or_else(|e| e.into_inner()) = configs;
        }
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

    pub fn subscribe_workspace_file_changes(&self) -> async_channel::Receiver<()> {
        let (sender, receiver) = async_channel::bounded(1);
        self.workspace_file_subscribers
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
            CoreNotification::WorkspaceFileChange { generation } => {
                if generation
                    < self
                        .workspace_file_generation
                        .fetch_max(generation, Ordering::SeqCst)
                {
                    return;
                }
                self.workspace_file_subscribers
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .retain(|sender| match sender.try_send(()) {
                        Ok(()) | Err(async_channel::TrySendError::Full(())) => true,
                        Err(async_channel::TrySendError::Closed(())) => false,
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
                    let list = match resp {
                        CompletionResponse::Array(v) => v,
                        CompletionResponse::List(l) => l.items,
                    };
                    if let Err(error) = tx.send(
                        list.into_iter()
                            .map(|item| LspCompletion { plugin_id, item })
                            .collect(),
                    ) {
                        eprintln!("Delivering completion list: {error}");
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
                        },
                    );
                self.notify_editor_metadata();
            }
            CoreNotification::DapStopped {
                dap_id,
                stopped,
                stack_frames,
                ..
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
                dbg.frames = stack_frames.values().flatten().cloned().collect();
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
                debug.frames.clear();
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
            AheadNotification::ConversationMessageAdded { message } => {
                let mut conversations =
                    self.conversations.lock().unwrap_or_else(|e| e.into_inner());
                let conversation =
                    conversations.entry(message.session_id.clone()).or_default();
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
                self.pending_user_inputs
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(session_id, request);
            }
            AheadNotification::AgentBufferSnapshotsRequested {
                session_id,
                turn_id,
                request_id,
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

    pub fn usage(&self, session_id: &str) -> Option<AgentUsage> {
        self.usage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .cloned()
    }

    pub fn pending_user_input(
        &self,
        session_id: &str,
    ) -> Option<AgentUserInputRequest> {
        self.pending_user_inputs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .cloned()
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
            active = Some(DurableSessionState {
                view,
                harness: summary.harness,
                external_agent_id: summary.external_agent_id.clone(),
                work_items: self.work_items(&session_id),
                conversation: conversation.messages,
                conversation_has_older: conversation.has_older,
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
        enabled: bool,
    ) -> Result<(), RpcError> {
        self.proxy_rpc
            .ahead_request_blocking(AheadRequest::SetMcpServerApproval {
                workspace: workspace.to_path_buf(),
                server_id: server_id.to_string(),
                expected_fingerprint: expected_fingerprint.to_string(),
                enabled,
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
                *existing = message;
            } else {
                conversation.messages.push(message);
            }
        }
        conversation.messages.sort_by(|left, right| {
            (left.sequence, &left.id).cmp(&(right.sequence, &right.id))
        });
        Ok(ConversationMessagePage {
            messages: conversation.messages.clone(),
            has_older: conversation.has_older,
        })
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
        let mut warnings = Vec::new();
        if let Some(backend) = self.harness_backend(session_id) {
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
    pub fn agent_turn(
        self: &Arc<Self>,
        request: AgentTurnRequestDto,
    ) -> Result<String, RpcError> {
        self.proxy_rpc
            .ahead_request_blocking(AheadRequest::AgentTurnStart { request })
            .and_then(|value| {
                value
                    .get("turn_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .ok_or_else(|| RpcError {
                        code: 0,
                        message: "AgentTurnStart returned no turn_id".to_string(),
                    })
            })
    }

    /// Cancels the in-flight harness turn for a session.
    pub fn agent_cancel(&self, session_id: &str) -> Result<bool, RpcError> {
        self.proxy_rpc
            .ahead_request_blocking(AheadRequest::AgentTurnCancel {
                session_id: session_id.to_string(),
            })
            .map(|value| {
                value
                    .get("cancelled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            })
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

    /// Retries a durable failed/cancelled turn without silently replaying it
    /// after a process restart.
    pub fn agent_retry(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<String, RpcError> {
        self.proxy_rpc
            .ahead_request_blocking(AheadRequest::AgentTurnRetry {
                session_id: session_id.to_string(),
                turn_id: turn_id.to_string(),
            })
            .and_then(|value| {
                value
                    .get("turn_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .ok_or_else(|| RpcError {
                        code: 0,
                        message: "AgentTurnRetry returned no turn_id".to_string(),
                    })
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
            .map(|_| {
                self.pending_user_inputs
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(session_id);
            })
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
            ProxyResponse::GetInlineCompletions { completions } => {
                let mut pending = self
                    .pending_inline
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let keys: Vec<PathBuf> = pending.keys().cloned().collect();
                if let Some(key) = keys.first() {
                    if let Some(tx) = pending.remove(key) {
                        let text = match completions {
                            InlineCompletionResponse::Array(items) => items
                                .first()
                                .map(|c| c.insert_text.clone())
                                .unwrap_or_default(),
                            InlineCompletionResponse::List(list) => list
                                .items
                                .first()
                                .map(|c| c.insert_text.clone())
                                .unwrap_or_default(),
                        };
                        let _ = tx.send(text);
                    }
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
    ) -> Receiver<Vec<LspCompletion>> {
        let (tx, rx) = unbounded();
        let id = self.next_id();
        self.pending_completion
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx);
        let pending = self.pending_completion.clone();
        self.proxy_rpc.completion(id, path, input, position);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            if let Some(tx) = pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id)
            {
                let _ = tx.send(Vec::new());
            }
        });
        rx
    }

    pub fn resolve_completion(
        &self,
        completion: LspCompletion,
    ) -> Receiver<Result<CompletionItem, RpcError>> {
        let (sender, receiver) = crossbeam_channel::bounded(1);
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
                if let Err(error) = sender.send(result) {
                    eprintln!("Delivering resolved completion: {error}");
                }
            },
        );
        receiver
    }

    pub fn sync_editor_snapshot(&self, path: PathBuf, content: String) {
        self.proxy_rpc.editor_snapshot(path, content);
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

    pub fn close_editor_buffer(&self, path: PathBuf) {
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
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1500));
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

    pub fn request_inline(
        self: &Arc<Self>,
        path: PathBuf,
        position: Position,
    ) -> Receiver<String> {
        let (tx, rx) = unbounded();
        self.pending_inline
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(path.clone(), tx.clone());
        let pending = self.pending_inline.clone();
        let key = path.clone();
        self.proxy_rpc.get_inline_completions(
            path,
            position,
            lsp_types::InlineCompletionTriggerKind::Automatic,
            move |res| {
                if let Ok(ProxyResponse::GetInlineCompletions { completions }) = res
                {
                    let text = match completions {
                        InlineCompletionResponse::Array(items) => items
                            .first()
                            .map(|c| c.insert_text.clone())
                            .unwrap_or_default(),
                        InlineCompletionResponse::List(list) => list
                            .items
                            .first()
                            .map(|c| c.insert_text.clone())
                            .unwrap_or_default(),
                    };
                    let _ = tx.send(text);
                }
                pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&key);
            },
        );
        rx
    }

    // ---- DAP passthrough ----

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
        dbg.frames.clear();
        dbg.state = DapSessionState::Starting;
        dbg.error = None;
        self.proxy_rpc.dap_start(config, breakpoints);
        drop(dbg);
        self.notify_editor_metadata();
        true
    }

    fn stopped_debug_target(&self) -> Option<(DapId, ThreadId)> {
        let debug = self.debug.lock().unwrap_or_else(|error| error.into_inner());
        if debug.state != DapSessionState::Stopped {
            return None;
        }
        Some((debug.dap_id?, debug.thread_id?))
    }

    pub fn dap_continue(&self) -> bool {
        let Some((dap_id, thread)) = self.stopped_debug_target() else {
            return false;
        };
        self.proxy_rpc.dap_continue(dap_id, thread);
        true
    }

    pub fn dap_step_over(&self) -> bool {
        let Some((dap_id, thread)) = self.stopped_debug_target() else {
            return false;
        };
        self.proxy_rpc.dap_step_over(dap_id, thread);
        true
    }

    pub fn dap_step_into(&self) -> bool {
        let Some((dap_id, thread)) = self.stopped_debug_target() else {
            return false;
        };
        self.proxy_rpc.dap_step_into(dap_id, thread);
        true
    }

    pub fn dap_step_out(&self) -> bool {
        let Some((dap_id, thread)) = self.stopped_debug_target() else {
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
        dbg.frames.clear();
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
        ProxyClient, ProxyResponse, harness_metadata, route_proxy_response,
        upsert_agent_tool_call, write_atomic,
    };
    use ahead_rpc::ahead::{
        AgentConfigChoice, AgentConfigOption, AgentConfigOptionValue,
        AgentPlanEntry, AgentToolCall, AgentUsage, AgentUserInputRequest,
        AheadNotification, HarnessKind,
    };
    use ahead_rpc::core::CoreNotification;
    use ahead_rpc::dap_types::{DapSessionState, DebugTerminalRequest};
    use ahead_rpc::proxy::ProxyRpcHandler;
    use crossbeam_channel::bounded;
    use lsp_types::{
        HoverContents, MarkedString, MessageType, Position, ShowMessageParams,
    };
    use std::{collections::HashMap, path::PathBuf};

    fn test_client() -> std::sync::Arc<ProxyClient> {
        ProxyClient::new_for_test(PathBuf::new())
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
        let (sender, receiver) = bounded(1);
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
            resp: lsp_types::CompletionResponse::Array(vec![item.clone()]),
            plugin_id,
        });
        let completion = receiver.try_recv().expect("items").remove(0);
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
            client.route_core(CoreNotification::DapStopped {
                dap_id,
                stopped: stopped.clone(),
                stack_frames: HashMap::new(),
                variables: Vec::new(),
            });
            assert!(updates.try_recv().is_ok());
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
    fn workspace_change_notifies_for_content_without_reindexing() {
        let client = test_client();
        let receiver = client.subscribe_workspace_file_changes();
        client.route_core(CoreNotification::WorkspaceFileChange { generation: 2 });
        assert_eq!(client.workspace_file_generation(), 2);
        assert_eq!(receiver.try_recv(), Ok(()));
        client.route_core(CoreNotification::WorkspaceFileChange { generation: 2 });
        assert_eq!(receiver.try_recv(), Ok(()));
        client.route_core(CoreNotification::WorkspaceFileChange { generation: 1 });
        assert!(receiver.try_recv().is_err());
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
    fn language_server_status_wakes_views_and_restart_uses_the_shared_proxy() {
        let client = test_client();
        let updates = client.subscribe_diagnostics();
        for params in [
            ahead_rpc::core::ServerStatusParams::starting("vtsls".into()),
            ahead_rpc::core::ServerStatusParams::ready("vtsls".into()),
            ahead_rpc::core::ServerStatusParams::failed(
                "vtsls".into(),
                "process exited".into(),
            ),
        ] {
            let ready = params.is_ok();
            client.route_core(CoreNotification::ServerStatus { params });
            assert_eq!(updates.try_recv(), Ok(()));
            assert_eq!(client.lsp_servers().len(), 1);
            assert_eq!(client.lsp_servers()[0].is_ready(), ready);
        }
        client.restart_language_servers();
        assert!(matches!(
            client
                .proxy_rpc
                .rx()
                .try_recv()
                .expect("restart notification"),
            ahead_rpc::proxy::ProxyRpc::Notification(
                ahead_rpc::proxy::ProxyNotification::RestartLanguageServers {}
            )
        ));
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
            client.route_ahead(
                ahead_rpc::ahead::AheadNotification::AgentBufferSnapshotsRequested {
                    session_id: "session".into(),
                    turn_id: turn_id.into(),
                    request_id: format!("request-{turn_id}"),
                },
            );
        }
        let requests = client.take_buffer_snapshot_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].request_id, "request-active");
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
    fn ignores_stale_turn_events_after_a_new_turn_starts() {
        let client = test_client();
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
                total_tokens: 42,
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
        assert!(client.pending_user_input("session").is_none());
    }
}
