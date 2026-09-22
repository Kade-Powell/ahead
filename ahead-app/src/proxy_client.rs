//! Proxy-backed client for the AHEAD GPUI shell.
//!
//! Spawns `ahead-proxy --proxy` as a child with piped stdio, sends
//! `ProxyNotification::Initialize`, and pumps `CoreNotification` messages
//! back into GPUI-visible state: completions, hover, definitions, inline
//! predictions, diagnostics, diff branch, and DAP lifecycle.

use std::{
    collections::{HashMap, HashSet},
    io::{BufReader, BufWriter},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use ahead_rpc::{
    RpcError, RpcMessage,
    ahead::{
        AgentPlanEntry, AgentToolCall, AgentTurnRequestDto, AgentUsage,
        AheadNotification, AheadRequest, CodeAnchor, ConversationMessage,
        ExternalAcpAdapter, HarnessKind, SessionExportBundle, SessionView, WorkItem,
        WorkItemStatus,
    },
    core::{CoreNotification, CoreRequest, CoreResponse},
    dap_types::{
        DapId, RunDebugConfig, SourceBreakpoint, StackFrame, Stopped, ThreadId,
    },
    proxy::{ProxyMessage, ProxyResponse, ProxyRpc, ProxyRpcHandler},
    source_control::{DiffInfo, FileDiff},
    stdio::{read_msg, write_msg},
};
use crossbeam_channel::{Receiver, Sender, unbounded};
use lsp_types::{
    CompletionItem, CompletionResponse, Diagnostic, GotoDefinitionResponse, Hover,
    InlineCompletionResponse, Location, Position,
};

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

/// 1-indexed changed line ranges for the open file, computed from
/// `git diff --unified=0`; deletion hunks have zero live rows and use their
/// boundary line as the marker anchor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffHunk {
    pub start: u32,
    pub len: u32,
    pub kind: DiffHunkKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffHunkKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Clone, Debug, Default)]
pub struct DebugState {
    pub active: bool,
    pub stopped: bool,
    pub reason: String,
    pub frames: Vec<StackFrame>,
    pub breakpoints: HashMap<PathBuf, HashSet<u32>>,
}

pub struct ProxyClient {
    proxy_rpc: ProxyRpcHandler,
    pending_completion: Arc<Mutex<HashMap<usize, Sender<Vec<CompletionItem>>>>>,
    pending_hover: Arc<Mutex<HashMap<usize, Sender<Hover>>>>,
    pending_defs: Arc<Mutex<HashMap<usize, Sender<Vec<Location>>>>>,
    pending_inline: Arc<Mutex<HashMap<PathBuf, Sender<String>>>>,
    request_seq: AtomicUsize,
    diagnostics: Arc<Mutex<HashMap<PathBuf, ProxyDiagnostics>>>,
    diag_version: Arc<Mutex<usize>>,
    diff: Arc<Mutex<ProxyDiffState>>,
    lsp_servers: Arc<Mutex<HashMap<String, LspServerStatus>>>,
    /// Uncommitted attribution anchors per file, refreshed from the ahead host.
    anchors: Arc<Mutex<HashMap<PathBuf, Vec<CodeAnchor>>>>,
    debug: Arc<Mutex<DebugState>>,
    debug_configs: Arc<Mutex<Vec<RunDebugConfig>>>,
    /// Durable conversation messages for the active session, keyed by session.
    conversations: Arc<Mutex<HashMap<String, Vec<ConversationMessage>>>>,
    plans: Arc<Mutex<HashMap<String, Vec<AgentPlanEntry>>>>,
    tool_calls: Arc<Mutex<HashMap<String, Vec<AgentToolCall>>>>,
    usage: Arc<Mutex<HashMap<String, AgentUsage>>>,
    /// In-flight streamed turns, keyed by session id.
    streaming_turns: Arc<Mutex<HashMap<String, String>>>,
    workspace: PathBuf,
    _child: Mutex<Option<Child>>,
}

impl ProxyClient {
    pub fn new(workspace: PathBuf) -> Arc<Self> {
        let proxy_rpc = ProxyRpcHandler::new();
        let client = Arc::new(Self {
            proxy_rpc: proxy_rpc.clone(),
            pending_completion: Arc::new(Mutex::new(HashMap::new())),
            pending_hover: Arc::new(Mutex::new(HashMap::new())),
            pending_defs: Arc::new(Mutex::new(HashMap::new())),
            pending_inline: Arc::new(Mutex::new(HashMap::new())),
            request_seq: AtomicUsize::new(1),
            diagnostics: Arc::new(Mutex::new(HashMap::new())),
            diag_version: Arc::new(Mutex::new(0)),
            diff: Arc::new(Mutex::new(ProxyDiffState::default())),
            lsp_servers: Arc::new(Mutex::new(HashMap::new())),
            anchors: Arc::new(Mutex::new(HashMap::new())),
            debug: Arc::new(Mutex::new(DebugState::default())),
            debug_configs: Arc::new(Mutex::new(default_debug_configs())),
            conversations: Arc::new(Mutex::new(HashMap::new())),
            plans: Arc::new(Mutex::new(HashMap::new())),
            tool_calls: Arc::new(Mutex::new(HashMap::new())),
            usage: Arc::new(Mutex::new(HashMap::new())),
            streaming_turns: Arc::new(Mutex::new(HashMap::new())),
            workspace: workspace.clone(),
            _child: Mutex::new(None),
        });
        client.refresh_diff_local();
        client.refresh_debug_configs();
        if let Some(child) = Self::spawn_proxy(&client, proxy_rpc) {
            *client._child.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
            client.proxy_rpc.initialize(Some(workspace), 0, 0);
        }
        client
    }

    fn proxy_bin() -> PathBuf {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("ahead-proxy")))
            .unwrap_or_else(|| PathBuf::from("target/debug/ahead-proxy"))
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
        std::thread::spawn(move || {
            let mut writer = BufWriter::new(stdin);
            for msg in app_tx.rx() {
                let wire: ProxyMessage = match msg {
                    ProxyRpc::Request(id, req) => RpcMessage::Request(id, req),
                    ProxyRpc::Notification(n) => RpcMessage::Notification(n),
                    ProxyRpc::Shutdown => break,
                };
                if write_msg(&mut writer, wire).is_err() {
                    break;
                }
            }
        });
        // Proxy -> app: core notifications and proxy request responses share
        // this stdio stream. Keep response payloads as JSON until we know
        // which protocol owns them; CoreResponse is uninhabited.
        let reader_client = client.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let msg: std::io::Result<
                    Option<
                        RpcMessage<CoreRequest, CoreNotification, serde_json::Value>,
                    >,
                > = read_msg(&mut reader);
                let msg = match msg {
                    Ok(m) => m,
                    Err(_) => break,
                };
                let Some(msg) = msg else { continue };
                match msg {
                    RpcMessage::Request(_id, _req) => {}
                    RpcMessage::Notification(n) => reader_client.route_core(n),
                    RpcMessage::Response(id, value) => {
                        route_proxy_response(&reader_client.proxy_rpc, id, value);
                    }
                    RpcMessage::Error(id, error) => {
                        reader_client.proxy_rpc.handle_response(id, Err(error));
                    }
                }
            }
        });
    }

    pub fn diagnostics_for(&self, path: &PathBuf) -> Option<Vec<Diagnostic>> {
        self.diagnostics
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(path)
            .map(|d| d.items.clone())
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

    pub fn diff(&self) -> ProxyDiffState {
        self.diff.lock().unwrap_or_else(|e| e.into_inner()).clone()
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
                        debug_command: None,
                        dap_id: DapId(0),
                        tracing_output: false,
                        config_source: Default::default(),
                    });
                }
            }
            *self.debug_configs.lock().unwrap_or_else(|e| e.into_inner()) = configs;
        }
    }

    /// Best-effort local refresh so gutter markers and the branch chip are
    /// never empty even when the proxy child is unavailable.
    pub fn refresh_diff_local(&self) {
        let branch = std::process::Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(&self.workspace)
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                } else {
                    None
                }
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "main".to_string());
        let mut state = self.diff.lock().unwrap_or_else(|e| e.into_inner());
        state.branch = branch;
    }

    /// Hunks for one open file from `git diff --unified=0 -- <path>`.
    pub fn hunks_for(&self, path: &PathBuf) -> Vec<DiffHunk> {
        let rel = path
            .strip_prefix(&self.workspace)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string_lossy().to_string());
        let out = std::process::Command::new("git")
            .args(["diff", "--unified=0", "--", &rel])
            .current_dir(&self.workspace)
            .output();
        let Ok(out) = out else { return Vec::new() };
        if !out.status.success() {
            return Vec::new();
        }
        parse_unified_hunks(&String::from_utf8_lossy(&out.stdout))
    }

    /// Refreshes uncommitted attribution anchors for the given absolute files.
    /// The host stores repository-relative paths, while the editor cache is
    /// keyed by the paths used by the open document.
    pub fn refresh_anchors(&self, paths: &[PathBuf]) {
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
        let Ok(value) = self.proxy_rpc.ahead_request_blocking(
            AheadRequest::ListAnchorsForPaths { paths: repo_paths },
        ) else {
            return;
        };
        let Ok(anchors) = serde_json::from_value::<Vec<CodeAnchor>>(value) else {
            return;
        };
        let mut cache = self.anchors.lock().unwrap_or_else(|e| e.into_inner());
        for path in paths {
            let relative = path
                .strip_prefix(&self.workspace)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| path.to_string_lossy().to_string());
            cache.insert(
                path.clone(),
                anchors
                    .iter()
                    .filter(|anchor| anchor.path == relative)
                    .cloned()
                    .collect(),
            );
        }
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

    /// Route one proxy-originated core notification into local state.
    pub fn route_core(&self, notif: CoreNotification) {
        match notif {
            CoreNotification::CompletionResponse {
                request_id, resp, ..
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
                    let _ = tx.send(list);
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
            }
            CoreNotification::DapStopped {
                stopped,
                stack_frames,
                ..
            } => {
                let mut dbg = self.debug.lock().unwrap_or_else(|e| e.into_inner());
                dbg.active = true;
                dbg.stopped = true;
                dbg.reason = stopped.reason.clone();
                dbg.frames = stack_frames.values().flatten().cloned().collect();
            }
            CoreNotification::DapContinued { .. } => {
                let mut dbg = self.debug.lock().unwrap_or_else(|e| e.into_inner());
                dbg.stopped = false;
            }
            CoreNotification::DapBreakpointsResp {
                path, breakpoints, ..
            } => {
                let mut dbg = self.debug.lock().unwrap_or_else(|e| e.into_inner());
                dbg.breakpoints.insert(
                    self.breakpoint_path(&path),
                    breakpoints
                        .iter()
                        .filter_map(|b| b.line.map(|l| l as u32))
                        .collect(),
                );
            }
            CoreNotification::AheadNotification { notification } => {
                self.route_ahead(notification);
            }
            _ => {}
        }
    }

    /// Routes streamed AHEAD harness notifications into local conversation
    /// state. Deltas append to the in-flight message; turn state finalizes it.
    pub fn route_ahead(&self, notification: AheadNotification) {
        match notification {
            AheadNotification::ConversationMessageAdded { message } => {
                let mut conversations =
                    self.conversations.lock().unwrap_or_else(|e| e.into_inner());
                let list =
                    conversations.entry(message.session_id.clone()).or_default();
                if let Some(existing) = list.iter_mut().find(|m| m.id == message.id)
                {
                    *existing = message;
                } else {
                    list.push(message);
                }
            }
            AheadNotification::AgentMessageDelta {
                session_id,
                turn_id,
                delta,
            } => {
                let mut conversations =
                    self.conversations.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(list) = conversations.get_mut(&session_id) {
                    if let Some(last) = list.iter_mut().rev().find(|m| {
                        m.role == "agent"
                            && m.status == "streaming"
                            && m.turn_id == turn_id
                    }) {
                        last.content.push_str(&delta);
                    }
                }
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
                    streaming.insert(session_id.clone(), turn_id.clone());
                } else if streaming
                    .get(&session_id)
                    .is_some_and(|active| active == &turn_id)
                {
                    streaming.remove(&session_id);
                }
                let mut conversations =
                    self.conversations.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(list) = conversations.get_mut(&session_id) {
                    if let Some(message) =
                        list.iter_mut().find(|m| m.id == message_id)
                    {
                        message.status = state;
                    }
                }
            }
            AheadNotification::AgentPlan {
                session_id,
                entries,
                ..
            } => {
                self.plans
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(session_id, entries);
            }
            AheadNotification::AgentToolCall {
                session_id, call, ..
            } => {
                self.tool_calls
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entry(session_id)
                    .or_default()
                    .push(call);
            }
            AheadNotification::AgentUsage {
                session_id, usage, ..
            } => {
                self.usage
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(session_id, usage);
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

    pub fn usage(&self, session_id: &str) -> Option<AgentUsage> {
        self.usage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .cloned()
    }

    /// Reopens the newest active durable session for this workspace, or starts
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
        let value = self
            .proxy_rpc
            .ahead_request_blocking(AheadRequest::ListSessions)?;
        let sessions =
            serde_json::from_value::<Vec<SessionView>>(value).map_err(|error| {
                RpcError {
                    code: 0,
                    message: format!(
                        "Invalid session list from AHEAD proxy: {error}"
                    ),
                }
            })?;
        if let Some(session) = sessions
            .into_iter()
            .filter(|session| {
                matches!(
                    session.session.lifecycle,
                    ahead_rpc::ahead::SessionLifecycle::Active
                )
            })
            .max_by(|left, right| {
                left.session.created_at.cmp(&right.session.created_at)
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

    /// Starts a new durable AHEAD session without reopening an existing one.
    pub fn start_work_session(
        &self,
        title: &str,
        starting_point: &str,
        harness: HarnessKind,
        external_agent_id: Option<&str>,
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

    /// Durable conversation for a session, refreshing from the host first so a
    /// reopened session shows the persisted history.
    pub fn conversation(&self, session_id: &str) -> Vec<ConversationMessage> {
        if let Ok(value) = self.proxy_rpc.ahead_request_blocking(
            AheadRequest::ConversationMessages {
                session_id: session_id.to_string(),
            },
        ) {
            if let Ok(messages) =
                serde_json::from_value::<Vec<ConversationMessage>>(value)
            {
                self.conversations
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(session_id.to_string(), messages);
            }
        }
        self.conversations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Exports a readable, Git-distributable session checkpoint. Runtime
    /// databases and harness state stay outside the shared directory.
    pub fn export_checkpoint(&self, session_id: &str) -> Result<PathBuf, RpcError> {
        let bundle = self.session_export(session_id)?;
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

    pub fn harness_warning(&self, session_id: &str) -> Option<String> {
        let backend = self.harness_backend(session_id)?;
        if backend.starts_with("external-agent") {
            Some(
                "External ACP: shell and edits are observed, not AHEAD-mediated."
                    .to_string(),
            )
        } else if backend.ends_with("-fresh") {
            Some("The persisted agent thread was unavailable; a fresh thread is active.".to_string())
        } else {
            None
        }
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
        self.harness_backend(session_id)
            .filter(|backend| backend.starts_with("external-agent"))
            .map(|_| HarnessKind::ExternalAcp)
            .unwrap_or(HarnessKind::Ahead)
    }

    pub fn external_agent_id(&self, session_id: &str) -> Option<String> {
        self.harness_backend(session_id)
            .and_then(|backend| {
                backend
                    .strip_prefix("external-agent:")
                    .map(|id| id.strip_suffix("-fresh").unwrap_or(id).to_string())
            })
            .filter(|id| !id.is_empty())
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
                                range: l.target_range,
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
    ) -> Receiver<Vec<CompletionItem>> {
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
                    let locs = match definition {
                        GotoDefinitionResponse::Scalar(loc) => vec![loc],
                        GotoDefinitionResponse::Array(locs) => locs,
                        GotoDefinitionResponse::Link(links) => links
                            .into_iter()
                            .map(|l| Location {
                                uri: l.target_uri,
                                range: l.target_range,
                            })
                            .collect(),
                    };
                    let _ = tx.send(locs);
                }
                pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
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
        config: RunDebugConfig,
        breakpoints: HashMap<PathBuf, Vec<SourceBreakpoint>>,
    ) {
        self.proxy_rpc.dap_start(config, breakpoints);
        let mut dbg = self.debug.lock().unwrap_or_else(|e| e.into_inner());
        dbg.active = true;
        dbg.stopped = false;
    }

    pub fn dap_continue(&self, dap_id: DapId, thread: ThreadId) {
        self.proxy_rpc.dap_continue(dap_id, thread);
    }

    pub fn dap_step_over(&self, dap_id: DapId, thread: ThreadId) {
        self.proxy_rpc.dap_step_over(dap_id, thread);
    }

    pub fn dap_step_into(&self, dap_id: DapId, thread: ThreadId) {
        self.proxy_rpc.dap_step_into(dap_id, thread);
    }

    pub fn dap_step_out(&self, dap_id: DapId, thread: ThreadId) {
        self.proxy_rpc.dap_step_out(dap_id, thread);
    }

    pub fn dap_stop(&self, dap_id: DapId) {
        self.proxy_rpc.dap_stop(dap_id);
        let mut dbg = self.debug.lock().unwrap_or_else(|e| e.into_inner());
        dbg.active = false;
        dbg.stopped = false;
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
        if dbg.active {
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
                .dap_set_breakpoints(DapId(0), path, breakpoints);
        }
    }

    fn breakpoint_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace.join(path)
        }
    }
}

fn route_proxy_response(
    proxy_rpc: &ProxyRpcHandler,
    id: ahead_rpc::RequestId,
    value: serde_json::Value,
) {
    match serde_json::from_value::<ProxyResponse>(value) {
        Ok(response) => proxy_rpc.handle_response(id, Ok(response)),
        Err(error) => eprintln!("invalid proxy response: {error}"),
    }
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
            debug_command: None,
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
            debug_command: None,
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

pub fn parse_unified_hunks(text: &str) -> Vec<DiffHunk> {
    let mut hunks = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with("@@") {
            continue;
        }
        // Format: @@ -old,count +new,count @@
        let plus = line.split('+').nth(1).unwrap_or("");
        let range = plus.split_whitespace().next().unwrap_or("");
        let mut parts = range.split(',');
        let start: u32 = parts.next().unwrap_or("0").parse().unwrap_or(0);
        let new_len: u32 = parts.next().unwrap_or("1").parse().unwrap_or(1);
        if start == 0 && new_len > 0 {
            continue;
        }
        let old_part = line.split('-').nth(1).unwrap_or("");
        let old_range = old_part.split_whitespace().next().unwrap_or("");
        let old_len: u32 = old_range
            .split(',')
            .nth(1)
            .unwrap_or("1")
            .parse()
            .unwrap_or(1);
        let kind = if old_len == 0 {
            DiffHunkKind::Added
        } else if new_len == 0 {
            DiffHunkKind::Deleted
        } else {
            DiffHunkKind::Modified
        };
        hunks.push(DiffHunk {
            start: start.max(1),
            len: if matches!(kind, DiffHunkKind::Deleted) {
                0
            } else {
                new_len.max(1)
            },
            kind,
        });
    }
    hunks
}

#[allow(dead_code)]
fn _assert_core_types(_req: CoreRequest, _resp: CoreResponse) {}

#[cfg(test)]
mod tests {
    use super::{
        ProxyResponse, parse_unified_hunks, route_proxy_response, write_atomic,
    };
    use ahead_rpc::proxy::ProxyRpcHandler;
    use crossbeam_channel::bounded;
    use lsp_types::{HoverContents, MarkedString, Position};
    use std::path::PathBuf;

    #[test]
    fn parses_added_modified_and_deleted_hunks() {
        let diff = "@@ -10,0 +11,3 @@\n+aaa\n+bbb\n+ccc\n@@ -20,2 +23,2 @@\n-old\n+new\n@@ -30,2 +32,0 @@\n-old\n-older\n";
        let hunks = parse_unified_hunks(diff);
        assert_eq!(hunks.len(), 3);
        assert_eq!(hunks[0].start, 11);
        assert_eq!(hunks[0].len, 3);
        assert!(matches!(hunks[0].kind, super::DiffHunkKind::Added));
        assert_eq!(hunks[1].start, 23);
        assert!(matches!(hunks[1].kind, super::DiffHunkKind::Modified));
        assert_eq!(hunks[2].start, 32);
        assert_eq!(hunks[2].len, 0);
        assert!(matches!(hunks[2].kind, super::DiffHunkKind::Deleted));
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
        route_proxy_response(&rpc, 0, serde_json::to_value(response).unwrap());

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
}
