//! External-agent ACP client (compatibility tier).
//!
//! AHEAD speaks ACP v1 over stdio to its curated ACP agents. AHEAD never
//! reimplements the model/tool loop here; it only:
//!
//! - spawns the agent process over stdio,
//! - performs the `initialize` → `session/new|load` → `session/prompt`
//!   handshake,
//! - streams `session/update` notifications to the durable session and UI,
//! - answers `session/request_permission` from the current Learn/Assist mode,
//! - cancels an in-flight turn with `session/cancel`.
//!
//! Wire format: newline-delimited JSON-RPC 2.0 on stdio. The reader thread
//! owns response correlation so cancellation can be written from any thread
//! while a prompt request is outstanding.
//!
//! **Enforcement note:** an external agent performs its own shell/file effects,
//! so this tier cannot enforce AHEAD's Learn read-only, mechanical scope or edit
//! attribution. The built-in native tier owns that effect boundary instead.
//! See the harness decision in the docs.

use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use ahead_rpc::ahead::{
    AgentBufferSnapshot, AgentCommand, AgentConfigChoice, AgentConfigOption,
    AgentConfigOptionValue, AgentPresentationAction,
    validate_agent_buffer_snapshots,
};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::editor_tools::{
    parse_presentation_action, presentation_tool_definitions,
};

/// ACP protocol version advertised by this client.
pub const ACP_PROTOCOL_VERSION: u32 = 1;

const MAX_EDITOR_MCP_MESSAGE_BYTES: usize = 1_048_576;

const MCP_LATEST_LEGACY_PROTOCOL_VERSION: &str = "2025-11-25";
const MCP_MODERN_PROTOCOL_VERSION: &str = "2026-07-28";
const MCP_SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &[
    MCP_MODERN_PROTOCOL_VERSION,
    MCP_LATEST_LEGACY_PROTOCOL_VERSION,
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum McpProtocolError {
    InvalidParams,
    UnsupportedVersion(String),
}

fn mcp_legacy_protocol_version(requested: Option<&str>) -> &'static str {
    match requested {
        Some("2024-11-05") => "2024-11-05",
        Some("2025-03-26") => "2025-03-26",
        Some("2025-06-18") => "2025-06-18",
        Some("2025-11-25") => MCP_LATEST_LEGACY_PROTOCOL_VERSION,
        _ => MCP_LATEST_LEGACY_PROTOCOL_VERSION,
    }
}

fn mcp_request_is_modern(
    params: &Value,
    legacy_initialized: bool,
) -> std::result::Result<bool, McpProtocolError> {
    let Some(metadata) = params.get("_meta") else {
        return if legacy_initialized {
            Ok(false)
        } else {
            Err(McpProtocolError::InvalidParams)
        };
    };
    let Some(version) = metadata
        .get("io.modelcontextprotocol/protocolVersion")
        .and_then(Value::as_str)
    else {
        return Err(McpProtocolError::InvalidParams);
    };
    if metadata
        .get("io.modelcontextprotocol/clientCapabilities")
        .and_then(Value::as_object)
        .is_none()
    {
        return Err(McpProtocolError::InvalidParams);
    }
    if version == MCP_MODERN_PROTOCOL_VERSION {
        Ok(true)
    } else {
        Err(McpProtocolError::UnsupportedVersion(version.to_string()))
    }
}

fn mcp_protocol_error_response(id: Value, error: McpProtocolError) -> Value {
    let error = match error {
        McpProtocolError::InvalidParams => {
            json!({ "code": -32602, "message": "invalid MCP request metadata" })
        }
        McpProtocolError::UnsupportedVersion(requested) => json!({
            "code": -32022,
            "message": "unsupported MCP protocol version",
            "data": {
                "supported": MCP_SUPPORTED_PROTOCOL_VERSIONS,
                "requested": requested,
            },
        }),
    };
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

fn mcp_server_info() -> Value {
    json!({
        "name": "ahead-editor",
        "version": env!("CARGO_PKG_VERSION"),
    })
}

fn mcp_modern_complete_result(result: Value) -> Value {
    let Value::Object(mut result) = result else {
        return result;
    };
    result.insert("resultType".to_string(), json!("complete"));
    result.insert(
        "_meta".to_string(),
        json!({ "io.modelcontextprotocol/serverInfo": mcp_server_info() }),
    );
    Value::Object(result)
}

fn mcp_discovery_result() -> Value {
    mcp_modern_complete_result(json!({
        "supportedVersions": MCP_SUPPORTED_PROTOCOL_VERSIONS,
        "capabilities": { "tools": {} },
    }))
}

fn mcp_success_response(id: Value, modern: bool, result: Value) -> Value {
    if modern {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": mcp_modern_complete_result(result),
        })
    } else {
        json!({ "jsonrpc": "2.0", "id": id, "result": result })
    }
}

/// One streamed update from the harness, normalized for AHEAD storage and UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessEvent {
    AgentDelta {
        acp_session_id: String,
        text: String,
    },
    AgentThought {
        acp_session_id: String,
        text: String,
    },
    ContextCompacted {
        acp_session_id: String,
    },
    ToolCall {
        acp_session_id: String,
        call_id: String,
        title: String,
        status: String,
        kind: String,
    },
    FileChange {
        acp_session_id: String,
        status: String,
        changes: Vec<HarnessFileChange>,
    },
    Plan {
        acp_session_id: String,
        entries: Vec<HarnessPlanEntry>,
    },
    Usage {
        acp_session_id: String,
        total_tokens: u64,
        context_window: Option<u64>,
    },
    UserInputRequested {
        acp_session_id: String,
        request_id: String,
        is_blocking: bool,
        questions: Vec<HarnessUserInputQuestion>,
    },
    BufferSnapshotsRequested {
        acp_session_id: String,
        request_id: String,
    },
    EditorPresentationRequested {
        acp_session_id: String,
        request_id: String,
        action: AgentPresentationAction,
    },
    ModeChanged {
        acp_session_id: String,
        mode_id: String,
    },
    SessionTitle {
        acp_session_id: String,
        title: String,
    },
    AvailableCommands {
        acp_session_id: String,
        commands: Vec<AgentCommand>,
    },
    ConfigOptions {
        acp_session_id: String,
        options: Vec<AgentConfigOption>,
    },
}

/// A single plan step reported by the harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessPlanEntry {
    pub content: String,
    pub status: String,
    pub priority: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessUserInputOption {
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessUserInputQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    pub options: Vec<HarnessUserInputOption>,
    pub allows_other: bool,
    pub is_secret: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessFileChange {
    pub path: String,
    pub diff: String,
}

/// Callback invoked on the reader thread for every streamed event.
pub type HarnessSink = Arc<dyn Fn(HarnessEvent) + Send + Sync>;

/// One outstanding request awaiting a JSON-RPC response from the agent.
type PendingSender = mpsc::Sender<Result<Value, String>>;
/// Correlates JSON-RPC ids with the caller waiting on each response.
type PendingMap = Arc<Mutex<HashMap<u64, PendingSender>>>;

/// Configuration for launching a harness process.
#[derive(Debug, Clone)]
pub struct HarnessClientConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub cwd: PathBuf,
    pub default_config_options: HashMap<String, AgentConfigOptionValue>,
    pub default_config_options_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Default)]
struct AcpSessionCapabilities {
    load_session: bool,
    resume_session: bool,
}

#[derive(Clone)]
struct AcpEditorMcpServer {
    session_id: Option<String>,
    workspace: PathBuf,
    token: String,
}

#[derive(Default)]
struct AcpEditorMcpState {
    servers: HashMap<String, AcpEditorMcpServer>,
    connections: HashMap<String, String>,
    legacy_initialized_connections: HashSet<String>,
    cancelled_editor_requests: HashMap<(String, String), Instant>,
}

// Cancellation can reach the local bridge before its matching tool-call TCP
// connection; keep only a short, bounded set of those race tombstones.
const MAX_CANCELLED_EDITOR_REQUESTS: usize = 128;
const CANCELLED_EDITOR_REQUEST_TTL: Duration = Duration::from_secs(30);
// Bound worker threads when an ACP peer issues many slow editor calls.
const MAX_PENDING_STDIO_TOOL_CALLS: usize = 16;

enum EditorMcpStdioEvent {
    InputLine(String),
    InputTooLong,
    InputError(String),
    InputClosed,
    ToolCallFinished {
        id: Value,
        id_key: String,
        request_id: String,
        modern: bool,
        result: Value,
    },
}

type PendingEditorPresentations =
    Arc<Mutex<HashMap<(String, String), mpsc::Sender<(bool, String)>>>>;
type PendingEditorBufferSnapshots = Arc<
    Mutex<
        HashMap<
            (String, String),
            mpsc::Sender<std::result::Result<Vec<AgentBufferSnapshot>, String>>,
        >,
    >,
>;

fn register_editor_request(
    editor_mcp_state: &Arc<Mutex<AcpEditorMcpState>>,
    session_id: &str,
    request_id: &str,
    register: impl FnOnce() -> Result<()>,
) -> Result<bool> {
    let mut state = editor_mcp_state
        .lock()
        .map_err(|error| anyhow!("ACP editor MCP state lock: {error}"))?;
    let now = Instant::now();
    state
        .cancelled_editor_requests
        .retain(|_, expires_at| *expires_at > now);
    if state
        .cancelled_editor_requests
        .remove(&(session_id.to_string(), request_id.to_string()))
        .is_some()
    {
        return Ok(false);
    }
    register()?;
    Ok(true)
}

fn remember_cancelled_editor_request(
    state: &mut AcpEditorMcpState,
    session_id: &str,
    request_id: &str,
) {
    let now = Instant::now();
    state
        .cancelled_editor_requests
        .retain(|_, expires_at| *expires_at > now);
    if state.cancelled_editor_requests.len() >= MAX_CANCELLED_EDITOR_REQUESTS
        && let Some(oldest_key) = state
            .cancelled_editor_requests
            .iter()
            .min_by_key(|(_, expires_at)| **expires_at)
            .map(|(key, _)| (*key).clone())
    {
        state.cancelled_editor_requests.remove(&oldest_key);
    }
    state.cancelled_editor_requests.insert(
        (session_id.to_string(), request_id.to_string()),
        now + CANCELLED_EDITOR_REQUEST_TTL,
    );
}

fn claim_pending_editor_stdio_response(
    pending: &mut HashMap<String, String>,
    id_key: &str,
    request_id: &str,
) -> bool {
    if pending
        .get(id_key)
        .is_some_and(|active_id| active_id == request_id)
    {
        pending.remove(id_key);
        true
    } else {
        false
    }
}

impl HarnessClientConfig {
    /// Direct Codex ACP configuration for the opt-in harness test.
    /// Product sessions resolve an explicitly selected curated adapter.
    pub fn external_agent(cwd: PathBuf) -> Self {
        Self {
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "@agentclientprotocol/codex-acp".to_string(),
            ],
            env: HashMap::new(),
            cwd,
            default_config_options: HashMap::new(),
            default_config_options_path: None,
        }
    }

    pub fn new(command: impl Into<String>, cwd: PathBuf) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
            env: HashMap::new(),
            cwd,
            default_config_options: HashMap::new(),
            default_config_options_path: None,
        }
    }
}

/// Live ACP agent connection. Cheap to hold in an `Arc` and share: all
/// mutation is behind a mutex and the reader runs on its own thread.
pub struct HarnessClient {
    child: Mutex<Child>,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: PendingMap,
    next_id: AtomicU64,
    stopped: AtomicBool,
    /// AHEAD policy mode per ACP session id, used only to answer permission
    /// requests. This must stay separate from agent-advertised ACP modes,
    /// which may represent unrelated settings such as reasoning effort.
    policy_modes: Arc<Mutex<HashMap<String, String>>>,
    config_options: Arc<Mutex<HashMap<String, Vec<AgentConfigOption>>>>,
    default_config_options: Mutex<HashMap<String, AgentConfigOptionValue>>,
    default_config_options_path: Option<PathBuf>,
    sink: HarnessSink,
    editor_mcp_state: Arc<Mutex<AcpEditorMcpState>>,
    pending_editor_buffer_snapshots: PendingEditorBufferSnapshots,
    pending_editor_presentations: PendingEditorPresentations,
    editor_mcp_listener_addr: Option<String>,
    editor_mcp_command: Option<PathBuf>,
    editor_mcp_accepting: Arc<AtomicBool>,
    /// ACP mode ids advertised by each external agent session.
    agent_modes: Mutex<HashMap<String, Vec<String>>>,
    session_capabilities: Mutex<AcpSessionCapabilities>,
    /// In-flight prompt request id per ACP session. Cancellation removes the
    /// correlated response waiter so the controller settles immediately.
    prompt_requests: Mutex<HashMap<String, u64>>,
    /// Covers cancellation that races before the prompt request is registered.
    cancelled_sessions: Mutex<HashSet<String>>,
}

impl HarnessClient {
    /// Spawns the agent process and starts the reader thread. The `initialize`
    /// handshake must be performed separately via [`HarnessClient::initialize`].
    pub fn spawn(config: &HarnessClientConfig, sink: HarnessSink) -> Result<Self> {
        Self::spawn_inner(config, sink, true)
    }

    #[cfg(test)]
    pub(crate) fn spawn_without_editor_mcp(
        config: &HarnessClientConfig,
        sink: HarnessSink,
    ) -> Result<Self> {
        Self::spawn_inner(config, sink, false)
    }

    fn spawn_inner(
        config: &HarnessClientConfig,
        sink: HarnessSink,
        enable_editor_mcp_bridge: bool,
    ) -> Result<Self> {
        let (editor_mcp_listener, editor_mcp_listener_addr, editor_mcp_command) =
            if enable_editor_mcp_bridge {
                let editor_mcp_command = std::env::current_exe()
                    .context("AHEAD editor MCP server executable is unavailable")?;
                let editor_mcp_listener = TcpListener::bind(("127.0.0.1", 0))
                    .context("could not bind the local AHEAD editor MCP bridge")?;
                editor_mcp_listener.set_nonblocking(true).context(
                    "could not configure the local AHEAD editor MCP bridge",
                )?;
                let editor_mcp_listener_addr = editor_mcp_listener
                    .local_addr()
                    .context("could not read the AHEAD editor MCP bridge address")?
                    .to_string();
                (
                    Some(editor_mcp_listener),
                    Some(editor_mcp_listener_addr),
                    Some(editor_mcp_command),
                )
            } else {
                (None, None, None)
            };
        let editor_mcp_state = Arc::new(Mutex::new(AcpEditorMcpState::default()));
        let pending_editor_buffer_snapshots = Arc::new(Mutex::new(HashMap::new()));
        let pending_editor_presentations = Arc::new(Mutex::new(HashMap::new()));
        let editor_mcp_accepting =
            Arc::new(AtomicBool::new(enable_editor_mcp_bridge));

        let mut child = Command::new(&config.command)
            .args(&config.args)
            .envs(&config.env)
            .current_dir(&config.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| {
                format!("Failed to spawn ACP agent `{}`", config.command)
            })?;

        let stdin = Arc::new(Mutex::new(
            child.stdin.take().context("ACP agent stdin unavailable")?,
        ));
        let stdout = child
            .stdout
            .take()
            .context("ACP agent stdout unavailable")?;

        if let Some(editor_mcp_listener) = editor_mcp_listener {
            start_editor_mcp_listener(
                editor_mcp_listener,
                editor_mcp_state.clone(),
                pending_editor_buffer_snapshots.clone(),
                pending_editor_presentations.clone(),
                sink.clone(),
                editor_mcp_accepting.clone(),
            );
        }

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let policy_modes = Arc::new(Mutex::new(HashMap::new()));
        let config_options = Arc::new(Mutex::new(HashMap::new()));

        let harness = Self {
            child: Mutex::new(child),
            stdin: stdin.clone(),
            pending: pending.clone(),
            next_id: AtomicU64::new(1),
            stopped: AtomicBool::new(false),
            policy_modes: policy_modes.clone(),
            config_options: config_options.clone(),
            default_config_options: Mutex::new(
                config.default_config_options.clone(),
            ),
            default_config_options_path: config.default_config_options_path.clone(),
            sink: sink.clone(),
            editor_mcp_state,
            pending_editor_buffer_snapshots,
            pending_editor_presentations,
            editor_mcp_listener_addr,
            editor_mcp_command,
            editor_mcp_accepting,
            agent_modes: Mutex::new(HashMap::new()),
            session_capabilities: Mutex::new(AcpSessionCapabilities::default()),
            prompt_requests: Mutex::new(HashMap::new()),
            cancelled_sessions: Mutex::new(HashSet::new()),
        };

        Self::start_reader(
            stdout,
            stdin,
            pending,
            sink,
            policy_modes,
            config_options,
            harness.editor_mcp_state.clone(),
            harness.pending_editor_buffer_snapshots.clone(),
            harness.pending_editor_presentations.clone(),
        );
        Ok(harness)
    }

    fn start_reader(
        stdout: std::process::ChildStdout,
        stdin: Arc<Mutex<ChildStdin>>,
        pending: PendingMap,
        sink: HarnessSink,
        policy_modes: Arc<Mutex<HashMap<String, String>>>,
        config_options: Arc<Mutex<HashMap<String, Vec<AgentConfigOption>>>>,
        editor_mcp_state: Arc<Mutex<AcpEditorMcpState>>,
        pending_editor_buffer_snapshots: PendingEditorBufferSnapshots,
        pending_editor_presentations: PendingEditorPresentations,
    ) {
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                Self::route_message(
                    msg,
                    &stdin,
                    &pending,
                    &sink,
                    &policy_modes,
                    &config_options,
                    &editor_mcp_state,
                    &pending_editor_buffer_snapshots,
                    &pending_editor_presentations,
                );
            }
            if let Ok(mut pending) = pending.lock() {
                for (_, sender) in pending.drain() {
                    drop(sender.send(Err(
                        "ACP agent closed its stdout during a request".to_string(),
                    )));
                }
            }
        });
    }

    fn route_message(
        msg: Value,
        stdin: &Arc<Mutex<ChildStdin>>,
        pending: &PendingMap,
        sink: &HarnessSink,
        policy_modes: &Arc<Mutex<HashMap<String, String>>>,
        config_options: &Arc<Mutex<HashMap<String, Vec<AgentConfigOption>>>>,
        editor_mcp_state: &Arc<Mutex<AcpEditorMcpState>>,
        pending_editor_buffer_snapshots: &PendingEditorBufferSnapshots,
        pending_editor_presentations: &PendingEditorPresentations,
    ) {
        let method = msg.get("method").and_then(Value::as_str);
        let id = msg.get("id").cloned();

        // Response to one of our requests.
        if let Some(id_num) = id.as_ref().and_then(Value::as_u64) {
            if method.is_none() {
                if let Some(tx) =
                    pending.lock().ok().and_then(|mut p| p.remove(&id_num))
                {
                    let outcome = if let Some(err) = msg.get("error") {
                        Err(err.to_string())
                    } else {
                        Ok(msg.get("result").cloned().unwrap_or(Value::Null))
                    };
                    drop(tx.send(outcome));
                }
                return;
            }
        }

        match method {
            Some("mcp/connect") => {
                let server_id = msg
                    .get("params")
                    .and_then(|params| params.get("serverId"))
                    .and_then(Value::as_str);
                let connection_id = server_id.and_then(|server_id| {
                    let mut state = editor_mcp_state.lock().ok()?;
                    if !state.servers.contains_key(server_id) {
                        return None;
                    }
                    let connection_id = uuid::Uuid::new_v4().to_string();
                    state
                        .connections
                        .insert(connection_id.clone(), server_id.to_string());
                    Some(connection_id)
                });
                let Some(id) = id else { return };
                let response = match connection_id {
                    Some(connection_id) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "connectionId": connection_id },
                    }),
                    None => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32602, "message": "unknown AHEAD editor MCP server" },
                    }),
                };
                if let Err(error) = write_json(stdin, &response) {
                    tracing::warn!(%error, "failed to answer ACP MCP connect request");
                }
            }
            Some("mcp/message") => {
                let Some(id) = id else { return };
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                let method = params
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let connection_id = params
                    .get("connectionId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let request_params =
                    params.get("params").cloned().unwrap_or(Value::Null);
                let legacy_initialized = match editor_mcp_state.lock() {
                    Ok(state) => state
                        .legacy_initialized_connections
                        .contains(&connection_id),
                    Err(error) => {
                        tracing::warn!(%error, "failed to read ACP MCP protocol state");
                        false
                    }
                };
                match method {
                    "initialize" => {
                        let protocol_version = mcp_legacy_protocol_version(
                            request_params
                                .get("protocolVersion")
                                .and_then(Value::as_str),
                        );
                        match editor_mcp_state.lock() {
                            Ok(mut state) => {
                                if state.connections.contains_key(&connection_id) {
                                    state
                                        .legacy_initialized_connections
                                        .insert(connection_id.clone());
                                }
                            }
                            Err(error) => {
                                tracing::warn!(%error, "failed to record ACP MCP initialization");
                            }
                        }
                        let result = json!({
                            "protocolVersion": protocol_version,
                            "capabilities": { "tools": { "listChanged": false } },
                            "serverInfo": mcp_server_info(),
                        });
                        if let Err(error) = write_json(
                            stdin,
                            &json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": result,
                            }),
                        ) {
                            tracing::warn!(%error, "failed to answer ACP MCP initialize request");
                        }
                    }
                    "server/discover" => {
                        let response =
                            match mcp_request_is_modern(&request_params, false) {
                                Ok(true) => json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "result": mcp_discovery_result(),
                                }),
                                Ok(false) => mcp_protocol_error_response(
                                    id,
                                    McpProtocolError::InvalidParams,
                                ),
                                Err(error) => mcp_protocol_error_response(id, error),
                            };
                        if let Err(error) = write_json(stdin, &response) {
                            tracing::warn!(%error, "failed to answer ACP MCP discovery request");
                        }
                    }
                    "tools/list" => {
                        let response = match mcp_request_is_modern(
                            &request_params,
                            legacy_initialized,
                        ) {
                            Ok(modern) => mcp_success_response(
                                id,
                                modern,
                                json!({ "tools": editor_mcp_tools() }),
                            ),
                            Err(error) => mcp_protocol_error_response(id, error),
                        };
                        if let Err(error) = write_json(stdin, &response) {
                            tracing::warn!(%error, "failed to answer ACP MCP tools/list request");
                        }
                    }
                    "tools/call" => {
                        let modern = match mcp_request_is_modern(
                            &request_params,
                            legacy_initialized,
                        ) {
                            Ok(modern) => modern,
                            Err(error) => {
                                if let Err(error) = write_json(
                                    stdin,
                                    &mcp_protocol_error_response(id, error),
                                ) {
                                    tracing::warn!(%error, "failed to reject invalid ACP MCP tool request");
                                }
                                return;
                            }
                        };
                        let stdin = stdin.clone();
                        let editor_mcp_state = editor_mcp_state.clone();
                        let pending_editor_buffer_snapshots =
                            pending_editor_buffer_snapshots.clone();
                        let pending_editor_presentations =
                            pending_editor_presentations.clone();
                        let sink = sink.clone();
                        thread::spawn(move || {
                            let result = handle_editor_tool_call(
                                &connection_id,
                                request_params,
                                &editor_mcp_state,
                                &pending_editor_buffer_snapshots,
                                &pending_editor_presentations,
                                &sink,
                            );
                            if let Err(error) = write_json(
                                &stdin,
                                &mcp_success_response(id, modern, result),
                            ) {
                                tracing::warn!(%error, "failed to answer ACP MCP tools/call request");
                            }
                        });
                    }
                    "ping" => {
                        let response = match mcp_request_is_modern(
                            &request_params,
                            legacy_initialized,
                        ) {
                            Ok(modern) => {
                                mcp_success_response(id, modern, json!({}))
                            }
                            Err(error) => mcp_protocol_error_response(id, error),
                        };
                        if let Err(error) = write_json(stdin, &response) {
                            tracing::warn!(%error, "failed to answer ACP MCP ping request");
                        }
                    }
                    other => {
                        if let Err(error) = write_json(
                            stdin,
                            &json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": { "code": -32601, "message": format!("unsupported MCP method: {other}") },
                            }),
                        ) {
                            tracing::warn!(%error, "failed to answer unsupported ACP MCP request");
                        }
                    }
                }
            }
            Some("mcp/disconnect") => {
                let connection_id = msg
                    .get("params")
                    .and_then(|params| params.get("connectionId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if let Ok(mut state) = editor_mcp_state.lock() {
                    state.connections.remove(connection_id);
                    state.legacy_initialized_connections.remove(connection_id);
                }
                if let Some(id) = id
                    && let Err(error) = write_json(
                        stdin,
                        &json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
                    )
                {
                    tracing::warn!(%error, "failed to answer ACP MCP disconnect request");
                }
            }
            Some("session/update") => {
                if let Some(update) = msg.get("params").and_then(|p| p.get("update"))
                {
                    let session_id = msg
                        .get("params")
                        .and_then(|p| p.get("sessionId"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if update.get("sessionUpdate").and_then(Value::as_str)
                        == Some("config_option_update")
                    {
                        if let Err(error) = publish_config_options(
                            &session_id,
                            update,
                            config_options,
                            sink,
                            true,
                        ) {
                            tracing::warn!(%error, "invalid ACP config option update");
                        }
                    } else {
                        Self::emit_update(session_id, update, sink);
                    }
                }
            }
            Some("session/request_permission") => {
                // Permission is governed by the current mode for this session.
                let session_id = msg
                    .get("params")
                    .and_then(|p| p.get("sessionId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let mode = policy_modes
                    .lock()
                    .ok()
                    .and_then(|m| m.get(&session_id).cloned())
                    .unwrap_or_default();
                let options = msg
                    .get("params")
                    .and_then(|p| p.get("options"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let response = Self::permission_response(&mode, &options);
                if let Some(id) = id {
                    if let Err(error) = write_json(
                        stdin,
                        &json!({ "jsonrpc": "2.0", "id": id, "result": response }),
                    ) {
                        tracing::warn!(
                            %error,
                            "failed to answer ACP permission request"
                        );
                    }
                }
            }
            Some(other) => {
                // Unknown client method: answer method-not-found so the
                // harness is not left waiting on a response.
                if let Some(id) = id {
                    if let Err(error) = write_json(
                        stdin,
                        &json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": { "code": -32601, "message": format!("method not found: {other}") },
                        }),
                    ) {
                        tracing::warn!(
                            %error,
                            "failed to answer unsupported ACP method"
                        );
                    }
                }
            }
            None => {}
        }
    }

    fn emit_update(session_id: String, update: &Value, sink: &HarnessSink) {
        let kind = update
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match kind {
            "agent_message_chunk" => {
                if let Some(text) = content_text(update.get("content")) {
                    sink(HarnessEvent::AgentDelta {
                        acp_session_id: session_id,
                        text,
                    });
                }
            }
            "agent_thought_chunk" => {
                if let Some(text) = content_text(update.get("content")) {
                    sink(HarnessEvent::AgentThought {
                        acp_session_id: session_id,
                        text,
                    });
                }
            }
            "tool_call" | "tool_call_update" => {
                let Some(call_id) = update
                    .get("toolCallId")
                    .or_else(|| update.get("tool_call_id"))
                    .and_then(Value::as_str)
                else {
                    tracing::warn!("ignored ACP tool update without toolCallId");
                    return;
                };
                sink(HarnessEvent::ToolCall {
                    acp_session_id: session_id,
                    call_id: call_id.to_string(),
                    title: update
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    status: update
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    kind: update
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                });
            }
            "plan" => {
                let entries = update
                    .get("entries")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .map(|e| HarnessPlanEntry {
                                content: e
                                    .get("content")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                status: e
                                    .get("status")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                priority: e
                                    .get("priority")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                sink(HarnessEvent::Plan {
                    acp_session_id: session_id,
                    entries,
                });
            }
            "usage_update" => {
                let total_tokens = update
                    .get("used")
                    .or_else(|| update.get("totalTokens"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let context_window = update
                    .get("size")
                    .or_else(|| update.get("contextWindow"))
                    .and_then(Value::as_u64);
                sink(HarnessEvent::Usage {
                    acp_session_id: session_id,
                    total_tokens,
                    context_window,
                });
            }
            "current_mode_update" => {
                let mode_id = update
                    .get("currentModeId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                sink(HarnessEvent::ModeChanged {
                    acp_session_id: session_id,
                    mode_id,
                });
            }
            "session_info_update" => {
                if let Some(title) = update.get("title").and_then(Value::as_str) {
                    sink(HarnessEvent::SessionTitle {
                        acp_session_id: session_id,
                        title: title.to_string(),
                    });
                }
            }
            "available_commands_update" => {
                let commands = update
                    .get("availableCommands")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|command| {
                                Some(AgentCommand {
                                    name: command
                                        .get("name")
                                        .and_then(Value::as_str)?
                                        .to_string(),
                                    description: command
                                        .get("description")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string(),
                                    input: command
                                        .get("input")
                                        .map(Value::to_string),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                sink(HarnessEvent::AvailableCommands {
                    acp_session_id: session_id,
                    commands,
                });
            }
            _ => {}
        }
    }

    /// Unknown and read-only sessions decline. Assistance accepts the first
    /// allow-style option; otherwise the request is cancelled.
    fn permission_response(mode: &str, options: &[Value]) -> Value {
        if !matches!(mode, "agent" | "assist") {
            return json!({ "outcome": { "outcome": "cancelled" } });
        }
        let allow = options.iter().find(|o| {
            let kind = o.get("kind").and_then(Value::as_str).unwrap_or_default();
            kind.starts_with("allow")
        });
        match allow {
            Some(option) => {
                let option_id =
                    option.get("optionId").cloned().unwrap_or(Value::Null);
                json!({
                    "outcome": {
                        "outcome": "selected",
                        "optionId": option_id,
                    }
                })
            }
            None => json!({ "outcome": { "outcome": "cancelled" } }),
        }
    }

    fn write_line(&self, value: &Value) -> Result<()> {
        write_json(&self.stdin, value)
    }

    fn send_request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<(u64, mpsc::Receiver<Result<Value, String>>)> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        let mut pending = self
            .pending
            .lock()
            .map_err(|e| anyhow!("pending lock: {e}"))?;
        anyhow::ensure!(
            !self.stopped.load(Ordering::SeqCst),
            "ACP agent shut down by AHEAD"
        );
        pending.insert(id, tx);
        drop(pending);
        if let Err(error) = self.write_line(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        })) {
            self.pending
                .lock()
                .map_err(|lock_error| anyhow!("pending lock: {lock_error}"))?
                .remove(&id);
            return Err(error);
        }
        Ok((id, rx))
    }

    fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        let (id, rx) = self.send_request(method, params)?;
        let result = match rx.recv_timeout(timeout) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) => Err(anyhow!("ACP `{method}` failed: {err}")),
            Err(_) => Err(anyhow!("ACP `{method}` timed out after {timeout:?}")),
        };
        if result.is_err() {
            self.pending
                .lock()
                .map_err(|error| anyhow!("pending lock: {error}"))?
                .remove(&id);
        }
        result
    }

    fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.write_line(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    /// Liveness probe for the spawned harness process.
    pub fn is_running(&self) -> bool {
        !self.stopped.load(Ordering::SeqCst)
            && self
                .child
                .lock()
                .map(|mut c| matches!(c.try_wait(), Ok(None)))
                .unwrap_or(false)
    }

    /// Performs the ACP `initialize` handshake and records session lifecycle
    /// capabilities for durable reopen.
    pub fn initialize(&self) -> Result<Value> {
        let result = self.request(
            "initialize",
            json!({
                "protocolVersion": ACP_PROTOCOL_VERSION,
                "clientCapabilities": {
                    "fs": { "readTextFile": false, "writeTextFile": false },
                    "terminal": false,
                    "session": { "configOptions": { "boolean": {} } },
                },
                "clientInfo": { "name": "ahead", "version": env!("CARGO_PKG_VERSION") },
            }),
            Duration::from_secs(60),
        )?;
        let capabilities = result
            .get("agentCapabilities")
            .map(session_capabilities)
            .unwrap_or_default();
        *self
            .session_capabilities
            .lock()
            .map_err(|error| anyhow!("ACP capability lock: {error}"))? =
            capabilities;
        Ok(result)
    }

    pub fn supports_editor_presentation(&self) -> bool {
        self.editor_mcp_accepting.load(Ordering::SeqCst)
    }

    fn declare_editor_mcp_server(
        &self,
        workspace: &Path,
    ) -> Result<(Vec<Value>, Option<String>)> {
        let (Some(listener_addr), Some(command)) = (
            self.editor_mcp_listener_addr.as_deref(),
            self.editor_mcp_command.as_deref(),
        ) else {
            return Ok((Vec::new(), None));
        };
        let server_id = format!("ahead-editor-{}", uuid::Uuid::new_v4());
        let token = uuid::Uuid::new_v4().to_string();
        self.editor_mcp_state
            .lock()
            .map_err(|error| anyhow!("ACP editor MCP state lock: {error}"))?
            .servers
            .insert(
                server_id.clone(),
                AcpEditorMcpServer {
                    session_id: None,
                    workspace: workspace.to_path_buf(),
                    token: token.clone(),
                },
            );
        Ok((
            vec![json!({
                "name": "AHEAD editor",
                "command": command.to_string_lossy(),
                "args": ["--ahead-editor-mcp"],
                "env": [
                    { "name": "AHEAD_EDITOR_MCP_ADDR", "value": listener_addr },
                    { "name": "AHEAD_EDITOR_MCP_SERVER_ID", "value": server_id },
                    { "name": "AHEAD_EDITOR_MCP_TOKEN", "value": token },
                ],
            })],
            Some(server_id),
        ))
    }

    fn bind_editor_mcp_server(
        &self,
        server_id: Option<&str>,
        session_id: &str,
    ) -> Result<()> {
        let Some(server_id) = server_id else {
            return Ok(());
        };
        let mut state = self
            .editor_mcp_state
            .lock()
            .map_err(|error| anyhow!("ACP editor MCP state lock: {error}"))?;
        let server = state
            .servers
            .get_mut(server_id)
            .context("AHEAD editor MCP server registration was lost")?;
        server.session_id = Some(session_id.to_string());
        Ok(())
    }

    fn remove_editor_mcp_server(&self, server_id: Option<&str>) {
        let Some(server_id) = server_id else {
            return;
        };
        if let Ok(mut state) = self.editor_mcp_state.lock() {
            state.servers.remove(server_id);
            state
                .connections
                .retain(|_, connected_server_id| connected_server_id != server_id);
            let live_connections =
                state.connections.keys().cloned().collect::<HashSet<_>>();
            state
                .legacy_initialized_connections
                .retain(|connection_id| live_connections.contains(connection_id));
        }
    }

    /// Creates a fresh harness conversation and returns its ACP session id.
    pub fn new_session(
        &self,
        cwd: &std::path::Path,
        mode_id: &str,
        model: Option<&str>,
        model_provider: Option<&str>,
    ) -> Result<String> {
        let (mcp_servers, editor_server_id) = self.declare_editor_mcp_server(cwd)?;
        let result = match self.request(
            "session/new",
            json!({ "cwd": cwd, "mcpServers": mcp_servers }),
            Duration::from_secs(60),
        ) {
            Ok(result) => result,
            Err(error) => {
                self.remove_editor_mcp_server(editor_server_id.as_deref());
                return Err(error);
            }
        };
        let Some(session_id) = result.get("sessionId").and_then(Value::as_str)
        else {
            self.remove_editor_mcp_server(editor_server_id.as_deref());
            bail!("ACP session/new returned no sessionId");
        };
        let session_id = session_id.to_string();
        self.bind_editor_mcp_server(editor_server_id.as_deref(), &session_id)?;
        self.record_agent_modes(&session_id, &result);
        publish_config_options(
            &session_id,
            &result,
            &self.config_options,
            &self.sink,
            false,
        )?;
        let selected_model_option =
            self.select_session_model(&session_id, &result, model, model_provider)?;
        self.apply_default_config_options(
            &session_id,
            selected_model_option.as_deref(),
        )?;
        self.set_session_mode(&session_id, mode_id)?;
        Ok(session_id)
    }

    /// Loads an existing harness conversation for durable reopen/resume.
    pub fn load_session(
        &self,
        acp_session_id: &str,
        cwd: &std::path::Path,
        mode_id: &str,
        model: Option<&str>,
        model_provider: Option<&str>,
    ) -> Result<()> {
        let capabilities = *self
            .session_capabilities
            .lock()
            .map_err(|error| anyhow!("ACP capability lock: {error}"))?;
        let (mcp_servers, editor_server_id) = self.declare_editor_mcp_server(cwd)?;
        let (method, params) = if capabilities.load_session {
            (
                "session/load",
                json!({
                    "sessionId": acp_session_id,
                    "cwd": cwd,
                    "mcpServers": mcp_servers,
                }),
            )
        } else if capabilities.resume_session {
            (
                "session/resume",
                json!({
                    "sessionId": acp_session_id,
                    "mcpServers": mcp_servers,
                }),
            )
        } else {
            self.remove_editor_mcp_server(editor_server_id.as_deref());
            bail!("ACP agent does not advertise session/load or session/resume")
        };
        let result = match self.request(method, params, Duration::from_secs(120)) {
            Ok(result) => result,
            Err(error) => {
                self.remove_editor_mcp_server(editor_server_id.as_deref());
                return Err(error);
            }
        };
        self.bind_editor_mcp_server(editor_server_id.as_deref(), acp_session_id)?;
        self.record_agent_modes(acp_session_id, &result);
        publish_config_options(
            acp_session_id,
            &result,
            &self.config_options,
            &self.sink,
            false,
        )?;
        let selected_model_option = self.select_session_model(
            acp_session_id,
            &result,
            model,
            model_provider,
        )?;
        self.apply_default_config_options(
            acp_session_id,
            selected_model_option.as_deref(),
        )?;
        self.set_session_mode(acp_session_id, mode_id)?;
        Ok(())
    }

    /// Sends a user prompt and blocks until the turn ends. Streamed output is
    /// delivered through the event sink as it arrives.
    pub fn prompt(&self, acp_session_id: &str, text: &str) -> Result<String> {
        if self
            .cancelled_sessions
            .lock()
            .map_err(|error| anyhow!("cancelled sessions lock: {error}"))?
            .remove(acp_session_id)
        {
            return Err(anyhow!("ACP prompt cancelled before it started"));
        }
        let (request_id, receiver) = self.send_request(
            "session/prompt",
            json!({
                "sessionId": acp_session_id,
                "prompt": [{ "type": "text", "text": text }],
            }),
        )?;
        self.prompt_requests
            .lock()
            .map_err(|error| anyhow!("prompt request lock: {error}"))?
            .insert(acp_session_id.to_string(), request_id);
        if self
            .cancelled_sessions
            .lock()
            .map_err(|error| anyhow!("cancelled sessions lock: {error}"))?
            .remove(acp_session_id)
        {
            if let Some(sender) = self
                .pending
                .lock()
                .map_err(|error| anyhow!("pending lock: {error}"))?
                .remove(&request_id)
            {
                drop(sender.send(Err("prompt cancelled by AHEAD".to_string())));
            }
        }
        let result = match receiver.recv_timeout(Duration::from_secs(60 * 30)) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(anyhow!("ACP `session/prompt` failed: {error}")),
            Err(_) => Err(anyhow!("ACP `session/prompt` timed out after 1800s")),
        };
        self.prompt_requests
            .lock()
            .map_err(|error| anyhow!("prompt request lock: {error}"))?
            .remove(acp_session_id);
        self.cancelled_sessions
            .lock()
            .map_err(|error| anyhow!("cancelled sessions lock: {error}"))?
            .remove(acp_session_id);
        if result.is_err() {
            self.pending
                .lock()
                .map_err(|error| anyhow!("pending lock: {error}"))?
                .remove(&request_id);
        }
        let result = result?;
        Ok(result
            .get("stopReason")
            .and_then(Value::as_str)
            .unwrap_or("end_turn")
            .to_string())
    }

    /// Requests cancellation of the in-flight turn (ACP notification).
    pub fn cancel(&self, acp_session_id: &str) -> Result<()> {
        self.cancelled_sessions
            .lock()
            .map_err(|error| anyhow!("cancelled sessions lock: {error}"))?
            .insert(acp_session_id.to_string());
        let notify_result =
            self.notify("session/cancel", json!({ "sessionId": acp_session_id }));
        let request_id = self
            .prompt_requests
            .lock()
            .map_err(|error| anyhow!("prompt request lock: {error}"))?
            .remove(acp_session_id);
        if let Some(request_id) = request_id {
            if let Some(sender) = self
                .pending
                .lock()
                .map_err(|error| anyhow!("pending lock: {error}"))?
                .remove(&request_id)
            {
                drop(sender.send(Err("prompt cancelled by AHEAD".to_string())));
            }
        }
        if let Ok(mut pending) = self.pending_editor_buffer_snapshots.lock() {
            pending.retain(|(session_id, _), _| session_id != acp_session_id);
        }
        self.cancel_editor_presentations(acp_session_id);
        notify_result
    }

    fn cancel_editor_presentations(&self, acp_session_id: &str) {
        Self::cancel_pending_editor_presentations(
            &self.pending_editor_presentations,
            acp_session_id,
        );
    }

    fn cancel_pending_editor_presentations(
        pending: &PendingEditorPresentations,
        acp_session_id: &str,
    ) {
        let requests = pending.lock().map(|mut requests| {
            let request_keys = requests
                .keys()
                .filter(|(session_id, _)| session_id == acp_session_id)
                .cloned()
                .collect::<Vec<_>>();
            request_keys
                .into_iter()
                .filter_map(|key| requests.remove(&key))
                .collect::<Vec<_>>()
        });
        if let Ok(senders) = requests {
            for sender in senders {
                drop(
                    sender
                        .send((false, "The AHEAD turn was cancelled.".to_string())),
                );
            }
        }
    }

    pub fn answer_editor_presentation(
        &self,
        acp_session_id: &str,
        request_id: &str,
        applied: bool,
        message: String,
    ) -> Result<()> {
        let sender = self
            .pending_editor_presentations
            .lock()
            .map_err(|error| anyhow!("pending editor presentation lock: {error}"))?
            .remove(&(acp_session_id.to_string(), request_id.to_string()))
            .with_context(|| {
                format!(
                    "AHEAD editor presentation `{request_id}` is no longer pending"
                )
            })?;
        sender.send((applied, message)).map_err(|_| {
            anyhow!("AHEAD editor presentation `{request_id}` was cancelled")
        })
    }

    pub fn answer_buffer_snapshots(
        &self,
        acp_session_id: &str,
        request_id: &str,
        buffers: Vec<AgentBufferSnapshot>,
        error: Option<String>,
    ) -> Result<()> {
        let sender = self
            .pending_editor_buffer_snapshots
            .lock()
            .map_err(|error| anyhow!("ACP editor buffer request lock: {error}"))?
            .remove(&(acp_session_id.to_string(), request_id.to_string()))
            .with_context(|| {
                format!("AHEAD buffer snapshot request `{request_id}` is no longer pending")
            })?;
        let response = match error {
            Some(error) => Err(error),
            None => validate_agent_buffer_snapshots(&buffers).map(|()| buffers),
        };
        sender.send(response).map_err(|_| {
            anyhow!("AHEAD buffer snapshot request `{request_id}` was cancelled")
        })
    }

    /// Records the AHEAD policy mode for permission requests. If an external
    /// agent independently advertises the same id, also select that ACP mode.
    /// Most ACP modes are orthogonal settings (Pi uses reasoning levels), so
    /// AHEAD policy names must never be sent to the agent blindly.
    pub fn set_session_mode(
        &self,
        acp_session_id: &str,
        mode_id: &str,
    ) -> Result<()> {
        if let Ok(mut policy_modes) = self.policy_modes.lock() {
            policy_modes.insert(acp_session_id.to_string(), mode_id.to_string());
        }
        let mode_is_advertised = self
            .agent_modes
            .lock()
            .map(|modes| {
                modes
                    .get(acp_session_id)
                    .is_some_and(|modes| modes.iter().any(|mode| mode == mode_id))
            })
            .unwrap_or(false);
        if mode_is_advertised {
            self.request(
                "session/set_mode",
                json!({ "sessionId": acp_session_id, "modeId": mode_id }),
                Duration::from_secs(30),
            )?;
        }
        Ok(())
    }

    fn record_agent_modes(&self, acp_session_id: &str, result: &Value) {
        if let Ok(mut sessions) = self.agent_modes.lock() {
            sessions.insert(acp_session_id.to_string(), advertised_mode_ids(result));
        }
    }

    fn select_session_model(
        &self,
        acp_session_id: &str,
        response: &Value,
        model: Option<&str>,
        model_provider: Option<&str>,
    ) -> Result<Option<String>> {
        let Some(model) = model.filter(|model| !model.trim().is_empty()) else {
            return Ok(None);
        };
        let (config_id, value) =
            advertised_model_option(response, model, model_provider)?;
        self.set_config_option(
            acp_session_id,
            &config_id,
            &AgentConfigOptionValue::Select(value),
        )?;
        Ok(Some(config_id))
    }

    fn apply_default_config_options(
        &self,
        acp_session_id: &str,
        except_config_id: Option<&str>,
    ) -> Result<()> {
        let defaults = self
            .default_config_options
            .lock()
            .map_err(|error| anyhow!("ACP config defaults lock: {error}"))?
            .clone();
        let offered_options = self
            .config_options
            .lock()
            .map_err(|error| anyhow!("ACP config option lock: {error}"))?
            .get(acp_session_id)
            .cloned()
            .unwrap_or_default();

        for option in offered_options {
            if except_config_id == Some(option.id.as_str()) {
                continue;
            }
            let Some(default_value) = defaults.get(&option.id) else {
                continue;
            };
            let value = match (default_value, &option.current_value) {
                (
                    AgentConfigOptionValue::Select(default),
                    AgentConfigOptionValue::Select(_),
                ) if option
                    .choices
                    .iter()
                    .any(|choice| choice.value == *default) =>
                {
                    AgentConfigOptionValue::Select(default.clone())
                }
                (
                    AgentConfigOptionValue::Boolean(default),
                    AgentConfigOptionValue::Boolean(_),
                ) => AgentConfigOptionValue::Boolean(*default),
                (
                    AgentConfigOptionValue::Select(_),
                    AgentConfigOptionValue::Select(_),
                ) => {
                    tracing::warn!(
                        config_id = %option.id,
                        "saved ACP option default is no longer advertised"
                    );
                    continue;
                }
                _ => continue,
            };
            if value != option.current_value {
                if let Err(error) =
                    self.set_config_option(acp_session_id, &option.id, &value)
                {
                    tracing::warn!(
                        config_id = %option.id,
                        %error,
                        "failed to apply saved ACP option default"
                    );
                    break;
                }
            }
        }
        Ok(())
    }

    pub fn set_config_option(
        &self,
        acp_session_id: &str,
        config_id: &str,
        value: &AgentConfigOptionValue,
    ) -> Result<()> {
        let options = self
            .config_options
            .lock()
            .map_err(|error| anyhow!("ACP config option lock: {error}"))?;
        let offered = options
            .get(acp_session_id)
            .into_iter()
            .flatten()
            .find(|option| option.id == config_id)
            .is_some_and(|option| match value {
                AgentConfigOptionValue::Select(value) => {
                    matches!(
                        &option.current_value,
                        AgentConfigOptionValue::Select(_)
                    ) && option.choices.iter().any(|choice| choice.value == *value)
                }
                AgentConfigOptionValue::Boolean(_) => {
                    matches!(
                        &option.current_value,
                        AgentConfigOptionValue::Boolean(_)
                    )
                }
            });
        if !offered {
            bail!(
                "ACP agent does not offer `{value:?}` for config option `{config_id}`"
            );
        }
        drop(options);
        let wire_value = match value {
            AgentConfigOptionValue::Select(value) => Value::String(value.clone()),
            AgentConfigOptionValue::Boolean(value) => Value::Bool(*value),
        };
        let response = self.request(
            "session/set_config_option",
            json!({
                "sessionId": acp_session_id,
                "configId": config_id,
                "value": wire_value,
            }),
            Duration::from_secs(30),
        )?;
        publish_config_options(
            acp_session_id,
            &response,
            &self.config_options,
            &self.sink,
            true,
        )?;
        if let Some(path) = self.default_config_options_path.as_deref() {
            let mut defaults = self
                .default_config_options
                .lock()
                .map_err(|error| anyhow!("ACP config defaults lock: {error}"))?;
            *defaults = crate::adapters::persist_default_config_option(
                path, config_id, value,
            )
            .context(
                "ACP option changed but its per-agent default could not be saved",
            )?;
        }
        Ok(())
    }

    /// Terminates the harness process.
    pub fn shutdown(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        self.editor_mcp_accepting.store(false, Ordering::SeqCst);
        if let Ok(mut pending) = self.pending.lock() {
            for (_, sender) in pending.drain() {
                drop(sender.send(Err("ACP agent shut down by AHEAD".into())));
            }
        }
        if let Ok(mut pending) = self.pending_editor_buffer_snapshots.lock() {
            pending.clear();
        }
        if let Ok(mut pending) = self.pending_editor_presentations.lock() {
            for (_, sender) in pending.drain() {
                drop(sender.send((
                    false,
                    "The AHEAD ACP agent was shut down.".to_string(),
                )));
            }
        }
        if let Ok(mut state) = self.editor_mcp_state.lock() {
            state.connections.clear();
            state.legacy_initialized_connections.clear();
            state.servers.clear();
        }
        if let Ok(mut child) = self.child.lock() {
            if matches!(child.try_wait(), Ok(None))
                && let Err(error) = child.kill()
            {
                tracing::warn!(%error, "failed to stop ACP agent");
            }
            if let Err(error) = child.wait() {
                tracing::warn!(%error, "failed to reap ACP agent");
            }
        }
    }
}

fn handle_editor_tool_call(
    connection_id: &str,
    params: Value,
    editor_mcp_state: &Arc<Mutex<AcpEditorMcpState>>,
    pending_editor_buffer_snapshots: &PendingEditorBufferSnapshots,
    pending_editor_presentations: &PendingEditorPresentations,
    sink: &HarnessSink,
) -> Value {
    let server_id = editor_mcp_state
        .lock()
        .ok()
        .and_then(|state| state.connections.get(connection_id).cloned());
    let Some(server_id) = server_id else {
        return editor_tool_result(
            false,
            "The AHEAD editor MCP connection is no longer active.".to_string(),
        );
    };
    let request_id = uuid::Uuid::new_v4().to_string();
    handle_editor_tool_call_for_server(
        &server_id,
        params,
        &request_id,
        editor_mcp_state,
        pending_editor_buffer_snapshots,
        pending_editor_presentations,
        sink,
    )
}

fn handle_editor_tool_call_for_server(
    server_id: &str,
    params: Value,
    request_id: &str,
    editor_mcp_state: &Arc<Mutex<AcpEditorMcpState>>,
    pending_editor_buffer_snapshots: &PendingEditorBufferSnapshots,
    pending_editor_presentations: &PendingEditorPresentations,
    sink: &HarnessSink,
) -> Value {
    let server = editor_mcp_state
        .lock()
        .ok()
        .and_then(|state| state.servers.get(server_id).cloned());
    let Some(server) = server else {
        return editor_tool_result(
            false,
            "The AHEAD editor MCP server is no longer active.".to_string(),
        );
    };
    let Some(session_id) = server.session_id else {
        return editor_tool_result(
            false,
            "The AHEAD editor MCP session is still starting.".to_string(),
        );
    };
    let Some(tool) = params.get("name").and_then(Value::as_str) else {
        return editor_tool_result(
            false,
            "AHEAD editor tool call has no name.".to_string(),
        );
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if tool == "read_editor_buffer" {
        let buffers = match request_editor_buffer_snapshots(
            &session_id,
            request_id,
            editor_mcp_state,
            pending_editor_buffer_snapshots,
            sink,
        ) {
            Ok(buffers) => buffers,
            Err(error) => return editor_tool_result(false, error.to_string()),
        };
        return match read_editor_buffer_from_snapshots(
            &arguments,
            &server.workspace,
            buffers,
        ) {
            Ok(content) => editor_tool_result(true, content),
            Err(error) => editor_tool_result(false, error.to_string()),
        };
    }
    let action =
        match parse_presentation_action(tool, arguments, &server.workspace, None) {
            Ok(action) => action,
            Err(error) => return editor_tool_result(false, error.to_string()),
        };

    let key = (session_id.clone(), request_id.to_string());
    let (sender, receiver) = mpsc::channel();
    let registered =
        register_editor_request(editor_mcp_state, &session_id, request_id, || {
            pending_editor_presentations
                .lock()
                .map_err(|error| anyhow!("ACP editor presentation lock: {error}"))?
                .insert(key.clone(), sender);
            Ok(())
        });
    match registered {
        Ok(true) => {}
        Ok(false) => {
            return editor_tool_result(
                false,
                "The ACP editor tool call was cancelled.".to_string(),
            );
        }
        Err(error) => {
            return editor_tool_result(false, error.to_string());
        }
    }
    sink(HarnessEvent::EditorPresentationRequested {
        acp_session_id: session_id,
        request_id: request_id.to_string(),
        action,
    });
    let response = receiver
        .recv_timeout(Duration::from_secs(15))
        .unwrap_or_else(|_| {
            (
                false,
                "AHEAD editor did not acknowledge the presentation.".to_string(),
            )
        });
    if let Ok(mut pending) = pending_editor_presentations.lock() {
        pending.remove(&key);
    }
    editor_tool_result(response.0, response.1)
}

fn request_editor_buffer_snapshots(
    session_id: &str,
    request_id: &str,
    editor_mcp_state: &Arc<Mutex<AcpEditorMcpState>>,
    pending_editor_buffer_snapshots: &PendingEditorBufferSnapshots,
    sink: &HarnessSink,
) -> Result<Vec<AgentBufferSnapshot>, String> {
    let key = (session_id.to_string(), request_id.to_string());
    let (sender, receiver) = mpsc::channel();
    let registered =
        register_editor_request(editor_mcp_state, session_id, request_id, || {
            pending_editor_buffer_snapshots
                .lock()
                .map_err(|error| anyhow!("ACP editor buffer request lock: {error}"))?
                .insert(key.clone(), sender);
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    if !registered {
        return Err("The ACP editor tool call was cancelled.".to_string());
    }
    sink(HarnessEvent::BufferSnapshotsRequested {
        acp_session_id: session_id.to_string(),
        request_id: request_id.to_string(),
    });
    let result = receiver.recv_timeout(Duration::from_secs(10));
    if let Ok(mut pending) = pending_editor_buffer_snapshots.lock() {
        pending.remove(&key);
    }
    match result {
        Ok(Ok(buffers)) => Ok(buffers),
        Ok(Err(error)) => Err(error),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            Err("AHEAD editor did not answer the buffer request".to_string())
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("AHEAD editor buffer request was cancelled".to_string())
        }
    }
}

fn cancel_editor_tool_call_for_server(
    server_id: &str,
    request_id: &str,
    editor_mcp_state: &Arc<Mutex<AcpEditorMcpState>>,
    pending_editor_buffer_snapshots: &PendingEditorBufferSnapshots,
    pending_editor_presentations: &PendingEditorPresentations,
) -> Value {
    let (presentation_sender, buffer_sender) = match editor_mcp_state.lock() {
        Ok(mut state) => {
            let Some(session_id) = state
                .servers
                .get(server_id)
                .and_then(|server| server.session_id.clone())
            else {
                return editor_tool_result(
                    false,
                    "The AHEAD editor MCP session is no longer active.".to_string(),
                );
            };
            let key = (session_id.clone(), request_id.to_string());
            let presentation_sender = match pending_editor_presentations.lock() {
                Ok(mut pending) => pending.remove(&key),
                Err(error) => {
                    return editor_tool_result(
                        false,
                        format!("ACP editor presentation lock: {error}"),
                    );
                }
            };
            let buffer_sender = match pending_editor_buffer_snapshots.lock() {
                Ok(mut pending) => pending.remove(&key),
                Err(error) => {
                    return editor_tool_result(
                        false,
                        format!("ACP editor buffer request lock: {error}"),
                    );
                }
            };
            if presentation_sender.is_none() && buffer_sender.is_none() {
                remember_cancelled_editor_request(
                    &mut state,
                    &session_id,
                    request_id,
                );
            }
            (presentation_sender, buffer_sender)
        }
        Err(error) => {
            return editor_tool_result(
                false,
                format!("ACP editor MCP state lock: {error}"),
            );
        }
    };

    if let Some(sender) = presentation_sender
        && let Err(error) = sender
            .send((false, "The ACP editor tool call was cancelled.".to_string()))
    {
        tracing::debug!(
            ?error,
            "cancelled ACP editor presentation was already closed"
        );
    }
    if let Some(sender) = buffer_sender
        && let Err(error) =
            sender.send(Err("The ACP editor tool call was cancelled.".to_string()))
    {
        tracing::debug!(
            ?error,
            "cancelled ACP editor buffer request was already closed"
        );
    }
    editor_tool_result(true, "ACP editor tool cancellation recorded.".to_string())
}

fn read_editor_buffer_from_snapshots(
    arguments: &Value,
    workspace: &Path,
    buffers: Vec<AgentBufferSnapshot>,
) -> Result<String> {
    let path = arguments
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .context("AHEAD read_editor_buffer requires a worktree-relative path")?;
    anyhow::ensure!(path.len() <= 512, "AHEAD editor path is too long");
    let relative = Path::new(path);
    anyhow::ensure!(
        !relative.is_absolute()
            && relative.components().all(|component| matches!(
                component,
                std::path::Component::Normal(_)
            )),
        "AHEAD editor path must stay inside the worktree"
    );
    anyhow::ensure!(
        !ahead_core::search::is_private_file(relative),
        "AHEAD cannot read a private editor buffer"
    );
    anyhow::ensure!(
        ahead_core::search::resolve_open_buffer_path(workspace, relative).is_some(),
        "AHEAD editor path is not a safe file in the worktree"
    );
    let buffer = buffers
        .iter()
        .find(|buffer| Path::new(&buffer.path) == relative)
        .with_context(|| {
            format!("`{path}` is not currently open in the AHEAD editor")
        })?;
    anyhow::ensure!(
        buffer.content.len() <= 262_144,
        "AHEAD editor buffer is larger than 256 KiB; use the agent's file tools for a focused excerpt"
    );
    Ok(format!(
        "Unsaved editor buffer `{path}`:\n{}",
        buffer.content
    ))
}

fn editor_tool_result(applied: bool, message: String) -> Value {
    json!({
        "content": [{ "type": "text", "text": message }],
        "isError": !applied,
    })
}

fn editor_mcp_tools() -> Vec<Value> {
    presentation_tool_definitions()
        .into_iter()
        .map(|(name, description, input_schema)| {
            json!({
                "name": name,
                "description": description,
                "inputSchema": input_schema,
            })
        })
        .collect()
}

fn start_editor_mcp_listener(
    listener: TcpListener,
    editor_mcp_state: Arc<Mutex<AcpEditorMcpState>>,
    pending_editor_buffer_snapshots: PendingEditorBufferSnapshots,
    pending_editor_presentations: PendingEditorPresentations,
    sink: HarnessSink,
    accepting: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        while accepting.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let editor_mcp_state = editor_mcp_state.clone();
                    let pending_editor_buffer_snapshots =
                        pending_editor_buffer_snapshots.clone();
                    let pending_editor_presentations =
                        pending_editor_presentations.clone();
                    let sink = sink.clone();
                    thread::spawn(move || {
                        if let Err(error) = handle_editor_mcp_bridge_connection(
                            stream,
                            &editor_mcp_state,
                            &pending_editor_buffer_snapshots,
                            &pending_editor_presentations,
                            &sink,
                        ) {
                            tracing::warn!(%error, "AHEAD editor MCP bridge request failed");
                        }
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(20));
                }
                Err(error) => {
                    tracing::warn!(%error, "AHEAD editor MCP bridge stopped");
                    break;
                }
            }
        }
    });
}

fn handle_editor_mcp_bridge_connection(
    mut stream: TcpStream,
    editor_mcp_state: &Arc<Mutex<AcpEditorMcpState>>,
    pending_editor_buffer_snapshots: &PendingEditorBufferSnapshots,
    pending_editor_presentations: &PendingEditorPresentations,
    sink: &HarnessSink,
) -> Result<()> {
    // macOS inherits the listener's nonblocking mode; this worker uses blocking IO.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(20)))?;
    let mut line = String::new();
    let bytes =
        read_editor_mcp_line(&mut BufReader::new(stream.try_clone()?), &mut line)?;
    anyhow::ensure!(bytes > 0, "AHEAD editor MCP bridge request was empty");
    anyhow::ensure!(
        line.len() <= MAX_EDITOR_MCP_MESSAGE_BYTES,
        "AHEAD editor MCP bridge request exceeds 1 MiB"
    );
    let request: Value = serde_json::from_str(&line)?;
    let server_id = request.get("serverId").and_then(Value::as_str);
    let token = request.get("token").and_then(Value::as_str);
    let registered = server_id.zip(token).and_then(|(server_id, token)| {
        editor_mcp_state.lock().ok().and_then(|state| {
            state
                .servers
                .get(server_id)
                .filter(|server| server.token == token)
                .map(|_| server_id.to_string())
        })
    });
    let result = match registered {
        Some(server_id) => {
            if let Some(request_id) =
                request.get("cancelRequestId").and_then(Value::as_str)
            {
                cancel_editor_tool_call_for_server(
                    &server_id,
                    request_id,
                    editor_mcp_state,
                    pending_editor_buffer_snapshots,
                    pending_editor_presentations,
                )
            } else if let Some(request_id) =
                request.get("requestId").and_then(Value::as_str)
            {
                handle_editor_tool_call_for_server(
                    &server_id,
                    request.get("params").cloned().unwrap_or(Value::Null),
                    request_id,
                    editor_mcp_state,
                    pending_editor_buffer_snapshots,
                    pending_editor_presentations,
                    sink,
                )
            } else {
                editor_tool_result(
                    false,
                    "AHEAD editor MCP bridge request has no request ID.".to_string(),
                )
            }
        }
        None => editor_tool_result(
            false,
            "AHEAD rejected the editor MCP bridge credentials.".to_string(),
        ),
    };
    let mut response = serde_json::to_vec(&json!({ "result": result }))?;
    response.push(b'\n');
    stream.write_all(&response)?;
    stream.flush()?;
    Ok(())
}

fn read_editor_mcp_line(
    reader: &mut impl BufRead,
    line: &mut String,
) -> std::io::Result<usize> {
    reader
        .take(MAX_EDITOR_MCP_MESSAGE_BYTES as u64 + 1)
        .read_line(line)
}

fn read_editor_mcp_input(
    reader: &mut impl BufRead,
    sender: mpsc::Sender<EditorMcpStdioEvent>,
) {
    let mut line = String::new();
    loop {
        let event = match read_editor_mcp_line(reader, &mut line) {
            Ok(0) => EditorMcpStdioEvent::InputClosed,
            Ok(_) if line.len() > MAX_EDITOR_MCP_MESSAGE_BYTES => {
                EditorMcpStdioEvent::InputTooLong
            }
            Ok(_) => EditorMcpStdioEvent::InputLine(std::mem::take(&mut line)),
            Err(error) => EditorMcpStdioEvent::InputError(error.to_string()),
        };
        let terminal = !matches!(event, EditorMcpStdioEvent::InputLine(_));
        if let Err(error) = sender.send(event) {
            tracing::debug!(?error, "ACP editor MCP loop has closed");
            return;
        }
        if terminal {
            return;
        }
    }
}

pub fn run_editor_mcp_stdio() -> Result<()> {
    let address = std::env::var("AHEAD_EDITOR_MCP_ADDR")
        .context("AHEAD editor MCP bridge address is missing")?;
    let server_id = std::env::var("AHEAD_EDITOR_MCP_SERVER_ID")
        .context("AHEAD editor MCP server id is missing")?;
    let token = std::env::var("AHEAD_EDITOR_MCP_TOKEN")
        .context("AHEAD editor MCP bridge token is missing")?;

    let (event_sender, event_receiver) = mpsc::channel();
    let reader_sender = event_sender.clone();
    thread::Builder::new()
        .name("ahead-editor-mcp-stdin".to_string())
        .spawn(move || {
            let stdin = std::io::stdin();
            read_editor_mcp_input(&mut BufReader::new(stdin.lock()), reader_sender);
        })?;

    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    run_editor_mcp_stdio_loop(
        &address,
        &server_id,
        &token,
        event_receiver,
        event_sender,
        &mut writer,
    )
}

fn run_editor_mcp_stdio_loop(
    address: &str,
    server_id: &str,
    token: &str,
    event_receiver: mpsc::Receiver<EditorMcpStdioEvent>,
    event_sender: mpsc::Sender<EditorMcpStdioEvent>,
    writer: &mut impl Write,
) -> Result<()> {
    let mut legacy_initialized = false;
    let mut pending_calls = HashMap::<String, String>::new();

    loop {
        let event = event_receiver
            .recv()
            .context("AHEAD editor MCP event channel closed")?;
        let line = match event {
            EditorMcpStdioEvent::InputLine(line) => line,
            EditorMcpStdioEvent::InputTooLong => {
                cancel_pending_editor_mcp_calls(
                    &mut pending_calls,
                    address,
                    server_id,
                    token,
                );
                bail!("AHEAD editor MCP message exceeds 1 MiB");
            }
            EditorMcpStdioEvent::InputError(error) => {
                cancel_pending_editor_mcp_calls(
                    &mut pending_calls,
                    address,
                    server_id,
                    token,
                );
                bail!("could not read AHEAD editor MCP stdin: {error}");
            }
            EditorMcpStdioEvent::InputClosed => {
                cancel_pending_editor_mcp_calls(
                    &mut pending_calls,
                    address,
                    server_id,
                    token,
                );
                return Ok(());
            }
            EditorMcpStdioEvent::ToolCallFinished {
                id,
                id_key,
                request_id,
                modern,
                result,
            } => {
                if claim_pending_editor_stdio_response(
                    &mut pending_calls,
                    &id_key,
                    &request_id,
                ) {
                    write_mcp_stdio_response(
                        writer,
                        mcp_success_response(id, modern, result),
                    )?;
                }
                continue;
            }
        };
        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                write_mcp_stdio_response(
                    writer,
                    json!({
                        "jsonrpc": "2.0",
                        "id": Value::Null,
                        "error": { "code": -32700, "message": error.to_string() },
                    }),
                )?;
                continue;
            }
        };
        let id = message.get("id").cloned();
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            if let Some(id) = id {
                write_mcp_stdio_response(
                    writer,
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32600, "message": "expected an MCP method" },
                    }),
                )?;
            }
            continue;
        };

        match method {
            "initialize" => {
                if let Some(id) = id {
                    let protocol_version = mcp_legacy_protocol_version(
                        message
                            .get("params")
                            .and_then(|params| params.get("protocolVersion"))
                            .and_then(Value::as_str),
                    );
                    legacy_initialized = true;
                    write_mcp_stdio_response(
                        writer,
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "protocolVersion": protocol_version,
                                "capabilities": { "tools": { "listChanged": false } },
                                "serverInfo": mcp_server_info(),
                            },
                        }),
                    )?;
                }
            }
            "notifications/initialized" => {}
            "notifications/cancelled" => {
                let Some(cancelled_id) = message
                    .get("params")
                    .and_then(|params| params.get("requestId"))
                else {
                    continue;
                };
                let id_key = cancelled_id.to_string();
                let Some(request_id) = pending_calls.remove(&id_key) else {
                    continue;
                };
                if let Err(error) = forward_editor_tool_cancellation(
                    address,
                    server_id,
                    token,
                    &request_id,
                ) {
                    tracing::warn!(%error, "failed to cancel AHEAD editor MCP request");
                }
            }
            "server/discover" => {
                if let Some(id) = id {
                    let params =
                        message.get("params").cloned().unwrap_or(Value::Null);
                    let response = match mcp_request_is_modern(&params, false) {
                        Ok(true) => json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": mcp_discovery_result(),
                        }),
                        Ok(false) => mcp_protocol_error_response(
                            id,
                            McpProtocolError::InvalidParams,
                        ),
                        Err(error) => mcp_protocol_error_response(id, error),
                    };
                    write_mcp_stdio_response(writer, response)?;
                }
            }
            "tools/list" => {
                if let Some(id) = id {
                    let params =
                        message.get("params").cloned().unwrap_or(Value::Null);
                    let response =
                        match mcp_request_is_modern(&params, legacy_initialized) {
                            Ok(modern) => mcp_success_response(
                                id,
                                modern,
                                json!({ "tools": editor_mcp_tools() }),
                            ),
                            Err(error) => mcp_protocol_error_response(id, error),
                        };
                    write_mcp_stdio_response(writer, response)?;
                }
            }
            "tools/call" => {
                if let Some(id) = id {
                    let params =
                        message.get("params").cloned().unwrap_or(Value::Null);
                    let modern =
                        match mcp_request_is_modern(&params, legacy_initialized) {
                            Ok(modern) => modern,
                            Err(error) => {
                                write_mcp_stdio_response(
                                    writer,
                                    mcp_protocol_error_response(id, error),
                                )?;
                                continue;
                            }
                        };
                    let id_key = id.to_string();
                    if pending_calls.contains_key(&id_key) {
                        write_mcp_stdio_response(
                            writer,
                            json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": { "code": -32600, "message": "duplicate pending MCP request id" },
                            }),
                        )?;
                        continue;
                    }
                    if pending_calls.len() >= MAX_PENDING_STDIO_TOOL_CALLS {
                        write_mcp_stdio_response(
                            writer,
                            mcp_success_response(
                                id,
                                modern,
                                editor_tool_result(
                                    false,
                                    "AHEAD editor MCP has too many pending tool calls.".to_string(),
                                ),
                            ),
                        )?;
                        continue;
                    }

                    let request_id = uuid::Uuid::new_v4().to_string();
                    pending_calls.insert(id_key.clone(), request_id.clone());
                    let worker_id = id.clone();
                    let worker_id_key = id_key.clone();
                    let worker_request_id = request_id.clone();
                    let worker_address = address.to_string();
                    let worker_server_id = server_id.to_string();
                    let worker_token = token.to_string();
                    let worker_sender = event_sender.clone();
                    let spawn_result = thread::Builder::new()
                        .name("ahead-editor-mcp-tool".to_string())
                        .spawn(move || {
                            let result = forward_editor_tool_call(
                                &worker_address,
                                &worker_server_id,
                                &worker_token,
                                &worker_request_id,
                                params,
                            )
                            .unwrap_or_else(|error| {
                                editor_tool_result(false, error.to_string())
                            });
                            if let Err(error) = worker_sender.send(
                                EditorMcpStdioEvent::ToolCallFinished {
                                    id: worker_id,
                                    id_key: worker_id_key,
                                    request_id: worker_request_id,
                                    modern,
                                    result,
                                },
                            ) {
                                tracing::debug!(
                                    ?error,
                                    "ACP editor MCP loop has closed"
                                );
                            }
                        });
                    if let Err(error) = spawn_result {
                        pending_calls.remove(&id_key);
                        write_mcp_stdio_response(
                            writer,
                            mcp_success_response(
                                id,
                                modern,
                                editor_tool_result(
                                    false,
                                    format!(
                                        "could not start AHEAD editor MCP call: {error}"
                                    ),
                                ),
                            ),
                        )?;
                    }
                }
            }
            "ping" => {
                if let Some(id) = id {
                    let params =
                        message.get("params").cloned().unwrap_or(Value::Null);
                    let response =
                        match mcp_request_is_modern(&params, legacy_initialized) {
                            Ok(modern) => {
                                mcp_success_response(id, modern, json!({}))
                            }
                            Err(error) => mcp_protocol_error_response(id, error),
                        };
                    write_mcp_stdio_response(writer, response)?;
                }
            }
            other => {
                if let Some(id) = id {
                    write_mcp_stdio_response(
                        writer,
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": { "code": -32601, "message": format!("unsupported MCP method: {other}") },
                        }),
                    )?;
                }
            }
        }
    }
}

fn cancel_pending_editor_mcp_calls(
    pending_calls: &mut HashMap<String, String>,
    address: &str,
    server_id: &str,
    token: &str,
) {
    for (_, request_id) in pending_calls.drain() {
        if let Err(error) =
            forward_editor_tool_cancellation(address, server_id, token, &request_id)
        {
            tracing::warn!(%error, "failed to cancel AHEAD editor MCP request on shutdown");
        }
    }
}

fn write_mcp_stdio_response(writer: &mut impl Write, response: Value) -> Result<()> {
    serde_json::to_writer(&mut *writer, &response)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn forward_editor_tool_call(
    address: &str,
    server_id: &str,
    token: &str,
    request_id: &str,
    params: Value,
) -> Result<Value> {
    let mut stream = TcpStream::connect(address)
        .context("could not connect to the AHEAD editor MCP bridge")?;
    let mut request = serde_json::to_vec(&json!({
        "serverId": server_id,
        "token": token,
        "requestId": request_id,
        "params": params,
    }))?;
    request.push(b'\n');
    stream.write_all(&request)?;
    stream.flush()?;
    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response)?;
    let response: Value = serde_json::from_str(&response)
        .context("invalid response from the AHEAD editor MCP bridge")?;
    response
        .get("result")
        .cloned()
        .context("AHEAD editor MCP bridge returned no result")
}

fn forward_editor_tool_cancellation(
    address: &str,
    server_id: &str,
    token: &str,
    request_id: &str,
) -> Result<()> {
    let mut stream = TcpStream::connect(address).context(
        "could not connect to the AHEAD editor MCP bridge for cancellation",
    )?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut request = serde_json::to_vec(&json!({
        "serverId": server_id,
        "token": token,
        "cancelRequestId": request_id,
    }))?;
    request.push(b'\n');
    stream.write_all(&request)?;
    stream.flush()?;
    let mut response = String::new();
    let bytes = BufReader::new(stream).read_line(&mut response)?;
    anyhow::ensure!(
        bytes > 0,
        "AHEAD editor MCP bridge did not confirm cancellation"
    );
    let response: Value = serde_json::from_str(&response)
        .context("invalid response from the AHEAD editor MCP bridge cancellation")?;
    anyhow::ensure!(
        response
            .get("result")
            .is_some_and(|result| result["isError"] == false),
        "AHEAD editor MCP bridge rejected cancellation"
    );
    Ok(())
}

impl Drop for HarnessClient {
    fn drop(&mut self) {
        self.editor_mcp_accepting.store(false, Ordering::SeqCst);
        if let Ok(mut child) = self.child.lock() {
            if matches!(child.try_wait(), Ok(None))
                && let Err(error) = child.kill()
            {
                tracing::warn!(%error, "failed to stop ACP agent during drop");
            }
        }
    }
}

fn write_json(stdin: &Arc<Mutex<ChildStdin>>, value: &Value) -> Result<()> {
    let mut stdin = stdin.lock().map_err(|e| anyhow!("stdin lock: {e}"))?;
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    stdin.write_all(line.as_bytes())?;
    stdin.flush()?;
    Ok(())
}

fn content_text(content: Option<&Value>) -> Option<String> {
    let content = content?;
    match content.get("type").and_then(Value::as_str) {
        Some("text") => content
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string),
        Some("resource") => content
            .get("resource")
            .and_then(|r| r.get("text"))
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => content
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

fn advertised_mode_ids(result: &Value) -> Vec<String> {
    result
        .get("modes")
        .and_then(|modes| modes.get("availableModes"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|mode| mode.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

fn parse_config_options(payload: &Value) -> Option<Vec<AgentConfigOption>> {
    let options = payload.get("configOptions")?.as_array()?;
    Some(
        options
            .iter()
            .filter_map(|option| {
                let (current_value, choices) = match option
                    .get("type")
                    .and_then(Value::as_str)?
                {
                    "select" => {
                        let choices = option
                            .get("options")?
                            .as_array()?
                            .iter()
                            .flat_map(|entry| {
                                entry
                                    .get("options")
                                    .and_then(Value::as_array)
                                    .into_iter()
                                    .flatten()
                                    .map(|choice| {
                                        (
                                            entry
                                                .get("name")
                                                .and_then(Value::as_str),
                                            choice,
                                        )
                                    })
                                    .chain(std::iter::once((None, entry)))
                            })
                            .filter_map(|(group, choice)| {
                                let name = choice.get("name")?.as_str()?;
                                Some(AgentConfigChoice {
                                    value: choice
                                        .get("value")?
                                        .as_str()?
                                        .to_string(),
                                    name: group
                                        .map(|group| format!("{group} · {name}"))
                                        .unwrap_or_else(|| name.to_string()),
                                })
                            })
                            .collect();
                        (
                            AgentConfigOptionValue::Select(
                                option.get("currentValue")?.as_str()?.to_string(),
                            ),
                            choices,
                        )
                    }
                    "boolean" => (
                        AgentConfigOptionValue::Boolean(
                            option.get("currentValue")?.as_bool()?,
                        ),
                        Vec::new(),
                    ),
                    _ => return None,
                };
                Some(AgentConfigOption {
                    id: option.get("id")?.as_str()?.to_string(),
                    name: option.get("name")?.as_str()?.to_string(),
                    category: option
                        .get("category")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    current_value,
                    choices,
                })
            })
            .collect(),
    )
}

fn publish_config_options(
    session_id: &str,
    payload: &Value,
    state: &Arc<Mutex<HashMap<String, Vec<AgentConfigOption>>>>,
    sink: &HarnessSink,
    required: bool,
) -> Result<()> {
    let Some(options) = parse_config_options(payload) else {
        if required {
            bail!("ACP agent returned no configOptions list");
        }
        return Ok(());
    };
    let mut state = state
        .lock()
        .map_err(|error| anyhow!("ACP config option lock: {error}"))?;
    // A notification can arrive after the session response is read but before
    // this thread handles it; keep that newer state instead of reverting it.
    if !required && state.contains_key(session_id) {
        return Ok(());
    }
    state.insert(session_id.to_string(), options.clone());
    drop(state);
    sink(HarnessEvent::ConfigOptions {
        acp_session_id: session_id.to_string(),
        options,
    });
    Ok(())
}

fn advertised_model_option(
    result: &Value,
    model: &str,
    model_provider: Option<&str>,
) -> Result<(String, String)> {
    let option = result
        .get("configOptions")
        .and_then(Value::as_array)
        .and_then(|options| {
            options.iter().find(|option| {
                option.get("type").and_then(Value::as_str) == Some("select")
                    && (option.get("category").and_then(Value::as_str)
                        == Some("model")
                        || option.get("id").and_then(Value::as_str) == Some("model"))
            })
        })
        .context("ACP agent did not advertise a model config option")?;
    let config_id = option
        .get("id")
        .and_then(Value::as_str)
        .context("ACP model config option has no id")?;
    let qualified = model_provider
        .filter(|provider| !provider.is_empty() && !model.contains('/'))
        .map(|provider| format!("{provider}/{model}"));
    let values = option
        .get("options")
        .and_then(Value::as_array)
        .context("ACP model config option has no values")?;
    let offered = values
        .iter()
        .flat_map(|entry| {
            entry
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .chain(std::iter::once(entry))
        })
        .filter_map(|entry| entry.get("value").and_then(Value::as_str))
        .collect::<Vec<_>>();
    let requested = qualified.as_deref().unwrap_or(model);
    let selected = offered.iter().copied().find(|value| *value == requested).ok_or_else(|| {
        let available = offered.iter().take(8).copied().collect::<Vec<_>>().join(", ");
        let remaining = offered.len().saturating_sub(8);
        anyhow!(
            "ACP agent does not offer model `{requested}`; advertised choices: {available}{}",
            if remaining > 0 {
                format!(" (+{remaining} more)")
            } else {
                String::new()
            }
        )
    })?;
    Ok((config_id.to_string(), selected.to_string()))
}

fn session_capabilities(agent_capabilities: &Value) -> AcpSessionCapabilities {
    AcpSessionCapabilities {
        load_session: agent_capabilities
            .get("loadSession")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        resume_session: agent_capabilities
            .get("sessionCapabilities")
            .and_then(|capabilities| capabilities.get("resume"))
            .is_some_and(|resume| !resume.is_null())
            || agent_capabilities
                .get("resumeSession")
                .and_then(Value::as_bool)
                .unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_mcp_negotiates_a_zed_supported_legacy_protocol_version() {
        assert_eq!(
            mcp_legacy_protocol_version(Some("2024-11-05")),
            "2024-11-05"
        );
        assert_eq!(
            mcp_legacy_protocol_version(Some("2025-03-26")),
            "2025-03-26"
        );
        assert_eq!(
            mcp_legacy_protocol_version(Some("2025-06-18")),
            "2025-06-18"
        );
        assert_eq!(
            mcp_legacy_protocol_version(Some("2025-11-25")),
            "2025-11-25"
        );
        assert_eq!(mcp_legacy_protocol_version(None), "2025-11-25");
        assert_eq!(
            mcp_legacy_protocol_version(Some("2026-07-28")),
            "2025-11-25"
        );
    }

    #[test]
    fn editor_mcp_accepts_modern_per_request_metadata_and_rejects_bad_versions() {
        let modern_request = json!({
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
            },
        });
        assert_eq!(mcp_request_is_modern(&modern_request, false), Ok(true));
        assert_eq!(mcp_request_is_modern(&json!({}), true), Ok(false));
        assert_eq!(
            mcp_request_is_modern(&json!({}), false),
            Err(McpProtocolError::InvalidParams)
        );
        assert_eq!(
            mcp_request_is_modern(
                &json!({
                    "_meta": {
                        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    },
                }),
                false,
            ),
            Err(McpProtocolError::InvalidParams)
        );
        assert_eq!(
            mcp_request_is_modern(
                &json!({
                    "_meta": {
                        "io.modelcontextprotocol/protocolVersion": "2030-01-01",
                        "io.modelcontextprotocol/clientCapabilities": {},
                    },
                }),
                false,
            ),
            Err(McpProtocolError::UnsupportedVersion(
                "2030-01-01".to_string()
            ))
        );
        let unsupported = mcp_protocol_error_response(
            json!(9),
            McpProtocolError::UnsupportedVersion("2030-01-01".to_string()),
        );
        assert_eq!(unsupported["id"], 9);
        assert_eq!(unsupported["error"]["code"], -32022);
        assert_eq!(unsupported["error"]["data"]["requested"], "2030-01-01");
        assert!(
            unsupported["error"]["data"]["supported"]
                .as_array()
                .is_some_and(|versions| versions.iter().any(|version| {
                    version.as_str() == Some(MCP_MODERN_PROTOCOL_VERSION)
                }))
        );
    }

    #[test]
    fn editor_mcp_discovery_and_modern_results_advertise_server_metadata() {
        let discovery = mcp_discovery_result();
        assert_eq!(discovery["resultType"], "complete");
        assert!(
            discovery["supportedVersions"]
                .as_array()
                .is_some_and(|versions| versions.iter().any(|version| {
                    version.as_str() == Some(MCP_MODERN_PROTOCOL_VERSION)
                }))
        );
        assert_eq!(
            discovery["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            "ahead-editor"
        );

        let result = mcp_modern_complete_result(json!({ "tools": [] }));
        assert_eq!(result["resultType"], "complete");
        assert_eq!(result["tools"], json!([]));
        assert_eq!(
            result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            "ahead-editor"
        );

        let modern_response =
            mcp_success_response(json!(7), true, json!({ "tools": [] }));
        assert_eq!(modern_response["id"], 7);
        assert_eq!(modern_response["result"]["resultType"], "complete");

        let legacy_response =
            mcp_success_response(json!(8), false, json!({ "tools": [] }));
        assert_eq!(legacy_response["id"], 8);
        assert!(legacy_response["result"].get("resultType").is_none());
    }

    #[test]
    fn acp_editor_mcp_lists_the_shared_presentation_tools() {
        let tools = editor_mcp_tools();
        for name in [
            "read_editor_buffer",
            "present_code",
            "move_code_pointer",
            "clear_presentation",
            "speak_text",
            "stop_speaking",
        ] {
            assert!(tools.iter().any(|tool| tool["name"] == name));
        }
    }

    #[test]
    fn acp_presentation_tool_returns_the_editor_acknowledgement() {
        let workspace = tempfile::tempdir().expect("create workspace");
        std::fs::create_dir_all(workspace.path().join("src"))
            .expect("create source directory");
        std::fs::write(
            workspace.path().join("src/main.rs"),
            "fn main() { let answer = 42; }",
        )
        .expect("write source file");

        let session_id = "acp-session".to_string();
        let server_id = "editor-server".to_string();
        let mut state = AcpEditorMcpState::default();
        state.servers.insert(
            server_id.clone(),
            AcpEditorMcpServer {
                session_id: Some(session_id.clone()),
                workspace: workspace.path().to_path_buf(),
                token: "test-token".to_string(),
            },
        );
        let state = Arc::new(Mutex::new(state));
        let pending_presentations: PendingEditorPresentations =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_snapshots: PendingEditorBufferSnapshots =
            Arc::new(Mutex::new(HashMap::new()));
        let (events, received_events) = mpsc::channel();
        let sink: HarnessSink = Arc::new(move |event| {
            events.send(event).expect("receive editor request");
        });

        let call_state = state.clone();
        let call_presentations = pending_presentations.clone();
        let call_snapshots = pending_snapshots.clone();
        let call_server_id = server_id.clone();
        let call_sink = sink.clone();
        let call = thread::spawn(move || {
            handle_editor_tool_call_for_server(
                &call_server_id,
                json!({
                    "name": "present_code",
                    "arguments": {
                        "path": "src/main.rs",
                        "quote": "let answer = 42;",
                        "label": "Answer",
                        "note": "This local binding holds the return value.",
                    },
                }),
                "presentation-call",
                &call_state,
                &call_snapshots,
                &call_presentations,
                &call_sink,
            )
        });

        let HarnessEvent::EditorPresentationRequested {
            acp_session_id,
            request_id,
            action,
        } = received_events
            .recv_timeout(Duration::from_secs(2))
            .expect("ACP tool call should request an editor presentation")
        else {
            panic!("ACP emitted a different event for present_code");
        };
        assert_eq!(acp_session_id, session_id);
        let AgentPresentationAction::Present {
            path,
            quote,
            label,
            note,
            ..
        } = action
        else {
            panic!("ACP emitted a different presentation action");
        };
        assert_eq!(path, "src/main.rs");
        assert_eq!(quote, "let answer = 42;");
        assert_eq!(label, "Answer");
        assert_eq!(note, "This local binding holds the return value.");

        let responder = pending_presentations
            .lock()
            .expect("lock pending presentations")
            .remove(&(session_id.clone(), request_id))
            .expect("presentation responder should be registered");
        responder
            .send((true, "Rendered cue `cue-1`.".to_string()))
            .expect("deliver rendered acknowledgement");
        let response = call.join().expect("join ACP editor-tool call");
        assert_eq!(response["isError"], false);
        assert_eq!(response["content"][0]["text"], "Rendered cue `cue-1`.");

        let pointer_state = state.clone();
        let pointer_presentations = pending_presentations.clone();
        let pointer_snapshots = pending_snapshots.clone();
        let pointer_server_id = server_id.clone();
        let pointer_sink = sink.clone();
        let pointer_call = thread::spawn(move || {
            handle_editor_tool_call_for_server(
                &pointer_server_id,
                json!({
                    "name": "move_code_pointer",
                    "arguments": {
                        "cue_id": "cue-1",
                        "quote": "fn main()",
                    },
                }),
                "pointer-call",
                &pointer_state,
                &pointer_snapshots,
                &pointer_presentations,
                &pointer_sink,
            )
        });
        let HarnessEvent::EditorPresentationRequested {
            acp_session_id,
            request_id,
            action,
        } = received_events
            .recv_timeout(Duration::from_secs(2))
            .expect("ACP pointer call should request an editor presentation")
        else {
            panic!("ACP emitted a different event for move_code_pointer");
        };
        assert_eq!(acp_session_id, session_id);
        assert_eq!(
            action,
            AgentPresentationAction::MovePointer {
                cue_id: "cue-1".to_string(),
                quote: "fn main()".to_string(),
            }
        );
        let responder = pending_presentations
            .lock()
            .expect("lock pending presentations")
            .remove(&(session_id.clone(), request_id))
            .expect("pointer responder should be registered");
        responder
            .send((true, "Moved pointer for cue `cue-1`.".to_string()))
            .expect("deliver pointer acknowledgement");
        let response = pointer_call.join().expect("join ACP pointer-tool call");
        assert_eq!(response["isError"], false);
        assert_eq!(
            response["content"][0]["text"],
            "Moved pointer for cue `cue-1`."
        );
    }

    #[test]
    fn cancelling_acp_turn_settles_pending_editor_presentation() {
        let pending: PendingEditorPresentations =
            Arc::new(Mutex::new(HashMap::new()));
        let (sender, receiver) = mpsc::channel();
        pending
            .lock()
            .expect("lock pending editor presentations")
            .insert(("acp-session".to_string(), "request".to_string()), sender);

        HarnessClient::cancel_pending_editor_presentations(&pending, "acp-session");
        let (applied, message) = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("cancel should settle the pending editor tool call");
        assert!(!applied);
        assert_eq!(message, "The AHEAD turn was cancelled.");
        assert!(
            pending
                .lock()
                .expect("lock pending presentations")
                .is_empty()
        );
    }

    #[test]
    fn cancelling_mcp_tool_call_settles_pending_editor_requests() {
        let editor_mcp_state = Arc::new(Mutex::new(AcpEditorMcpState::default()));
        editor_mcp_state
            .lock()
            .expect("lock editor MCP state")
            .servers
            .insert(
                "server".to_string(),
                AcpEditorMcpServer {
                    session_id: Some("session".to_string()),
                    workspace: PathBuf::new(),
                    token: "secret".to_string(),
                },
            );

        let pending_presentations: PendingEditorPresentations =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_snapshots: PendingEditorBufferSnapshots =
            Arc::new(Mutex::new(HashMap::new()));
        let (presentation_sender, presentation_receiver) = mpsc::channel();
        let (snapshot_sender, snapshot_receiver) = mpsc::channel();
        pending_presentations
            .lock()
            .expect("lock pending presentations")
            .insert(
                ("session".to_string(), "call-id".to_string()),
                presentation_sender,
            );
        pending_snapshots
            .lock()
            .expect("lock pending snapshots")
            .insert(
                ("session".to_string(), "call-id".to_string()),
                snapshot_sender,
            );

        let result = cancel_editor_tool_call_for_server(
            "server",
            "call-id",
            &editor_mcp_state,
            &pending_snapshots,
            &pending_presentations,
        );

        assert_eq!(result["isError"], false);
        assert_eq!(
            presentation_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("presentation should be cancelled"),
            (false, "The ACP editor tool call was cancelled.".to_string())
        );
        assert!(matches!(
            snapshot_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("buffer snapshot should be cancelled"),
            Err(message) if message == "The ACP editor tool call was cancelled."
        ));
        assert!(
            editor_mcp_state
                .lock()
                .expect("lock editor MCP state")
                .cancelled_editor_requests
                .is_empty()
        );
    }

    #[test]
    fn mcp_cancellation_before_editor_request_registration_is_remembered() {
        let editor_mcp_state = Arc::new(Mutex::new(AcpEditorMcpState::default()));
        editor_mcp_state
            .lock()
            .expect("lock editor MCP state")
            .servers
            .insert(
                "server".to_string(),
                AcpEditorMcpServer {
                    session_id: Some("session".to_string()),
                    workspace: PathBuf::new(),
                    token: "secret".to_string(),
                },
            );
        let pending_presentations: PendingEditorPresentations =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_snapshots: PendingEditorBufferSnapshots =
            Arc::new(Mutex::new(HashMap::new()));

        let result = cancel_editor_tool_call_for_server(
            "server",
            "call-id",
            &editor_mcp_state,
            &pending_snapshots,
            &pending_presentations,
        );
        assert_eq!(result["isError"], false);

        let (sender, receiver) = mpsc::channel();
        let mut pending_sender = Some(sender);
        let registered =
            register_editor_request(&editor_mcp_state, "session", "call-id", || {
                pending_presentations
                    .lock()
                    .map_err(|error| anyhow!("pending presentation lock: {error}"))?
                    .insert(
                        ("session".to_string(), "call-id".to_string()),
                        pending_sender.take().expect("sender has not been consumed"),
                    );
                Ok(())
            })
            .expect("check the remembered cancellation");

        assert!(!registered);
        assert!(receiver.recv_timeout(Duration::from_millis(1)).is_err());
        assert!(
            pending_presentations
                .lock()
                .expect("lock pending presentations")
                .is_empty()
        );
    }

    #[test]
    fn cancelled_mcp_call_cannot_claim_a_reused_jsonrpc_id() {
        let mut pending = HashMap::from([("1".to_string(), "old-call".to_string())]);
        pending.remove("1");
        pending.insert("1".to_string(), "new-call".to_string());

        assert!(!claim_pending_editor_stdio_response(
            &mut pending,
            "1",
            "old-call",
        ));
        assert_eq!(pending.get("1"), Some(&"new-call".to_string()));
        assert!(claim_pending_editor_stdio_response(
            &mut pending,
            "1",
            "new-call",
        ));
        assert!(pending.is_empty());
    }

    #[test]
    fn editor_mcp_bounds_stdin_frames_and_stops_after_invalid_input() {
        let mut input = vec![b' '; MAX_EDITOR_MCP_MESSAGE_BYTES - 3];
        input.extend_from_slice(b"{}\n");
        input.extend_from_slice("{\"message\":\"héllo\"}\n".as_bytes());
        let (sender, receiver) = mpsc::channel();
        read_editor_mcp_input(&mut std::io::Cursor::new(input), sender);
        assert!(
            matches!(receiver.recv().expect("boundary-sized line"), EditorMcpStdioEvent::InputLine(line)
            if line.len() == MAX_EDITOR_MCP_MESSAGE_BYTES && line.trim() == "{}")
        );
        assert!(
            matches!(receiver.recv().expect("following UTF-8 line"), EditorMcpStdioEvent::InputLine(line)
            if line == "{\"message\":\"héllo\"}\n")
        );
        assert!(matches!(
            receiver.recv().expect("EOF"),
            EditorMcpStdioEvent::InputClosed
        ));
        assert!(receiver.try_recv().is_err());

        for mut invalid in [vec![b' '; MAX_EDITOR_MCP_MESSAGE_BYTES + 1], vec![0xff]]
        {
            let oversized = invalid.len() > MAX_EDITOR_MCP_MESSAGE_BYTES;
            invalid.extend_from_slice(b"\n{\"method\":\"tools/call\"}\n");
            let mut reader = std::io::Cursor::new(invalid);
            let (sender, receiver) = mpsc::channel();
            read_editor_mcp_input(&mut reader, sender);
            if oversized {
                assert!(matches!(
                    receiver.recv().expect("oversized frame"),
                    EditorMcpStdioEvent::InputTooLong
                ));
                assert_eq!(
                    reader.position(),
                    MAX_EDITOR_MCP_MESSAGE_BYTES as u64 + 1,
                    "must stop reading at the bound, without waiting for a newline"
                );
            } else {
                assert!(matches!(
                    receiver.recv().expect("invalid UTF-8"),
                    EditorMcpStdioEvent::InputError(_)
                ));
            }
            assert!(
                receiver.try_recv().is_err(),
                "invalid input must not enqueue its trailing command"
            );
        }
    }

    #[test]
    fn editor_mcp_rejects_an_oversized_socket_frame_before_eof() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind bridge");
        let mut client = TcpStream::connect(listener.local_addr().expect("address"))
            .expect("connect bridge");
        client
            .set_write_timeout(Some(Duration::from_secs(2)))
            .expect("bound fixture write");
        let (server, _) = listener.accept().expect("accept bridge");
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            let sink: HarnessSink =
                Arc::new(|_| panic!("oversized input must not reach editor tools"));
            let result = handle_editor_mcp_bridge_connection(
                server,
                &Arc::new(Mutex::new(AcpEditorMcpState::default())),
                &Arc::new(Mutex::new(HashMap::new())),
                &Arc::new(Mutex::new(HashMap::new())),
                &sink,
            );
            sender.send(result).expect("report rejected frame");
        });
        client
            .write_all(&vec![b' '; MAX_EDITOR_MCP_MESSAGE_BYTES + 1])
            .expect("write unterminated frame");
        let result = receiver.recv_timeout(Duration::from_secs(1));
        // Release the old unbounded reader too, so a regression fails without leaking a worker.
        drop(client);
        worker.join().expect("bridge finished");
        let error = result
            .expect("reject before newline or socket EOF")
            .expect_err("oversized frame");
        assert!(error.to_string().contains("exceeds 1 MiB"));
    }

    #[test]
    fn editor_mcp_bridge_waits_for_a_fragmented_request_on_nonblocking_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind bridge");
        let mut client = TcpStream::connect(listener.local_addr().expect("address"))
            .expect("connect bridge");
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("bound response wait");
        let (server, _) = listener.accept().expect("accept bridge");
        // Exercise macOS's inherited mode on every platform.
        server
            .set_nonblocking(true)
            .expect("set accepted socket mode");
        client
            .write_all(b"{\"serverId\":")
            .expect("write first fragment");
        let (finished_sender, finished_receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            let sink: HarnessSink = Arc::new(|_| {
                panic!("unregistered requests must not reach the editor")
            });
            let result = handle_editor_mcp_bridge_connection(
                server,
                &Arc::new(Mutex::new(AcpEditorMcpState::default())),
                &Arc::new(Mutex::new(HashMap::new())),
                &Arc::new(Mutex::new(HashMap::new())),
                &sink,
            );
            finished_sender.send(result).expect("report bridge result");
        });
        let before_tail = finished_receiver.recv_timeout(Duration::from_millis(50));
        client
            .write_all(
                b"\"unknown\",\"token\":\"invalid\",\"requestId\":\"split\"}\n",
            )
            .expect("write delayed fragment");
        let mut response = String::new();
        BufReader::new(client)
            .read_line(&mut response)
            .expect("read response");
        worker.join().expect("bridge worker finished");
        assert!(matches!(before_tail, Err(mpsc::RecvTimeoutError::Timeout)));
        finished_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("receive bridge result")
            .expect("read complete fragmented request");
        let response: Value =
            serde_json::from_str(&response).expect("parse response");
        assert_eq!(response["result"]["isError"], true);
        assert!(
            response
                .to_string()
                .contains("rejected the editor MCP bridge credentials")
        );
    }

    #[test]
    fn stdio_mcp_loop_reads_cancellation_while_editor_tool_call_is_pending() {
        let workspace = tempfile::tempdir().expect("create workspace");
        std::fs::create_dir_all(workspace.path().join("src"))
            .expect("create source directory");
        std::fs::write(workspace.path().join("src/main.rs"), "fn main() {}\n")
            .expect("write source file");

        let listener =
            TcpListener::bind("127.0.0.1:0").expect("bind local editor MCP bridge");
        listener
            .set_nonblocking(true)
            .expect("make local editor MCP bridge nonblocking");
        let address = listener
            .local_addr()
            .expect("read bridge address")
            .to_string();
        let accepting = Arc::new(AtomicBool::new(true));
        let editor_mcp_state = Arc::new(Mutex::new(AcpEditorMcpState::default()));
        editor_mcp_state
            .lock()
            .expect("lock editor MCP state")
            .servers
            .insert(
                "server".to_string(),
                AcpEditorMcpServer {
                    session_id: Some("session".to_string()),
                    workspace: workspace.path().to_path_buf(),
                    token: "secret".to_string(),
                },
            );
        let pending_snapshots: PendingEditorBufferSnapshots =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_presentations: PendingEditorPresentations =
            Arc::new(Mutex::new(HashMap::new()));
        let (harness_event_sender, harness_event_receiver) = mpsc::channel();
        let sink: HarnessSink = Arc::new(move |event| {
            harness_event_sender
                .send(event)
                .expect("test should receive editor request");
        });
        start_editor_mcp_listener(
            listener,
            editor_mcp_state,
            pending_snapshots,
            pending_presentations.clone(),
            sink,
            accepting.clone(),
        );

        let (event_sender, event_receiver) = mpsc::channel();
        let loop_event_sender = event_sender.clone();
        let loop_address = address.clone();
        let loop_thread = thread::spawn(move || {
            let mut output = Vec::new();
            let result = run_editor_mcp_stdio_loop(
                &loop_address,
                "server",
                "secret",
                event_receiver,
                loop_event_sender,
                &mut output,
            );
            (result, output)
        });
        event_sender
            .send(EditorMcpStdioEvent::InputLine(
                json!({
                    "jsonrpc": "2.0",
                    "id": 7,
                    "method": "tools/call",
                    "params": {
                        "_meta": {
                            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                            "io.modelcontextprotocol/clientCapabilities": {},
                        },
                        "name": "present_code",
                        "arguments": {
                            "path": "src/main.rs",
                            "quote": "fn main() {}",
                            "label": "Main",
                            "note": "Entry point",
                        },
                    },
                })
                .to_string(),
            ))
            .expect("send pending editor tool call");
        let HarnessEvent::EditorPresentationRequested {
            acp_session_id,
            request_id,
            ..
        } = harness_event_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("tool call should reach the editor")
        else {
            panic!("expected an editor presentation request");
        };
        assert_eq!(acp_session_id, "session");

        event_sender
            .send(EditorMcpStdioEvent::InputLine(
                json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/cancelled",
                    "params": { "requestId": 7 },
                })
                .to_string(),
            ))
            .expect("send request cancellation");
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut cancellation_processed = false;
        while Instant::now() < deadline {
            cancellation_processed = pending_presentations
                .lock()
                .expect("lock pending presentations")
                .is_empty();
            if cancellation_processed {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        if !cancellation_processed
            && let Some(sender) = pending_presentations
                .lock()
                .expect("lock pending presentations")
                .remove(&(acp_session_id, request_id))
        {
            sender
                .send((false, "test cleanup".to_string()))
                .expect("release pending editor call");
        }
        event_sender
            .send(EditorMcpStdioEvent::InputClosed)
            .expect("close test MCP input");
        drop(event_sender);
        let (loop_result, output) = loop_thread.join().expect("join stdio MCP loop");
        accepting.store(false, Ordering::SeqCst);

        loop_result.expect("run stdio MCP loop");
        assert!(cancellation_processed, "cancellation did not reach editor");
        assert!(output.is_empty(), "cancelled tool call emitted a response");
    }

    #[test]
    fn acp_reads_unsaved_text_from_an_open_editor_snapshot() {
        let workspace = tempfile::tempdir().expect("create workspace");
        std::fs::create_dir_all(workspace.path().join("src"))
            .expect("create source directory");
        std::fs::write(workspace.path().join("src/lib.rs"), "disk value")
            .expect("write disk source");
        let buffer = AgentBufferSnapshot {
            path: "src/lib.rs".to_string(),
            content: "unsaved editor value".to_string(),
        };

        let text = read_editor_buffer_from_snapshots(
            &json!({ "path": "src/lib.rs" }),
            workspace.path(),
            vec![buffer],
        )
        .expect("read open editor snapshot");
        assert!(text.contains("unsaved editor value"));
        assert!(!text.contains("disk value"));
        assert!(
            read_editor_buffer_from_snapshots(
                &json!({ "path": ".env" }),
                workspace.path(),
                Vec::new(),
            )
            .is_err()
        );
    }

    #[test]
    fn extracts_text_from_content_blocks() {
        assert_eq!(
            content_text(Some(&json!({ "type": "text", "text": "hello" }))),
            Some("hello".to_string())
        );
        assert_eq!(
            content_text(Some(&json!({
                "type": "resource",
                "resource": { "text": "embedded" }
            }))),
            Some("embedded".to_string())
        );
    }

    #[test]
    fn learn_permission_is_declined_and_assist_selects_allow() {
        let options = vec![
            json!({ "optionId": "reject", "kind": "reject_once", "name": "Reject" }),
            json!({ "optionId": "allow", "kind": "allow_once", "name": "Allow" }),
        ];
        let learn = HarnessClient::permission_response("read-only", &options);
        assert_eq!(learn["outcome"]["outcome"], "cancelled");

        let assist = HarnessClient::permission_response("agent", &options);
        assert_eq!(assist["outcome"]["outcome"], "selected");
        assert_eq!(assist["outcome"]["optionId"], "allow");

        let unknown = HarnessClient::permission_response("", &options);
        assert_eq!(unknown["outcome"]["outcome"], "cancelled");
    }

    #[test]
    fn routes_agent_message_chunk_to_delta() {
        let seen: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_cb = seen.clone();
        let sink: HarnessSink = Arc::new(move |event| {
            if let Ok(mut s) = seen_cb.lock() {
                s.push(event);
            }
        });
        let update = json!({
            "sessionUpdate": "agent_message_chunk",
            "content": { "type": "text", "text": "PONG" }
        });
        HarnessClient::emit_update("s1".to_string(), &update, &sink);
        let events = seen.lock().unwrap();
        assert_eq!(
            events.as_slice(),
            &[HarnessEvent::AgentDelta {
                acp_session_id: "s1".to_string(),
                text: "PONG".to_string()
            }]
        );
    }

    #[test]
    fn parses_plan_and_usage_updates() {
        let seen: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_cb = seen.clone();
        let sink: HarnessSink = Arc::new(move |event| {
            if let Ok(mut s) = seen_cb.lock() {
                s.push(event);
            }
        });
        HarnessClient::emit_update(
            "s1".to_string(),
            &json!({
                "sessionUpdate": "plan",
                "entries": [
                    { "content": "Inspect", "status": "completed", "priority": "high" }
                ]
            }),
            &sink,
        );
        HarnessClient::emit_update(
            "s1".to_string(),
            &json!({ "sessionUpdate": "usage_update", "used": 18510, "size": 272000 }),
            &sink,
        );
        let events = seen.lock().unwrap();
        assert!(matches!(events[0], HarnessEvent::Plan { .. }));
        assert!(matches!(
            events[1],
            HarnessEvent::Usage {
                total_tokens: 18510,
                ..
            }
        ));
    }

    #[test]
    fn preserves_acp_tool_call_identity_for_updates() {
        let seen: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_cb = seen.clone();
        let sink: HarnessSink = Arc::new(move |event| {
            if let Ok(mut events) = seen_cb.lock() {
                events.push(event);
            }
        });
        HarnessClient::emit_update(
            "s1".to_string(),
            &json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "tool-7",
                "status": "completed"
            }),
            &sink,
        );

        assert!(matches!(
            seen.lock().unwrap().as_slice(),
            [HarnessEvent::ToolCall { call_id, status, .. }]
                if call_id == "tool-7" && status == "completed"
        ));
    }

    #[test]
    fn ignores_tool_updates_without_required_identity() {
        let seen: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_cb = seen.clone();
        let sink: HarnessSink = Arc::new(move |event| {
            if let Ok(mut events) = seen_cb.lock() {
                events.push(event);
            }
        });
        HarnessClient::emit_update(
            "s1".to_string(),
            &json!({
                "sessionUpdate": "tool_call_update",
                "status": "completed"
            }),
            &sink,
        );

        assert!(seen.lock().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_before_prompt_registration_settles_immediately() {
        let sink: HarnessSink = Arc::new(|_| {});
        let client = HarnessClient::spawn_without_editor_mcp(
            &HarnessClientConfig::new("/bin/cat", std::env::temp_dir()),
            sink,
        )
        .unwrap();

        client.cancel("session-race").unwrap();
        let started = std::time::Instant::now();
        let error = client.prompt("session-race", "ignored").unwrap_err();
        assert!(error.to_string().contains("before it started"));
        assert!(started.elapsed() < Duration::from_secs(1));
        client.shutdown();
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_settles_an_in_flight_prompt_without_agent_response() {
        let mut config = HarnessClientConfig::new("sh", std::env::temp_dir());
        config.args = vec![
            "-c".into(),
            r#"
IFS= read -r line || exit 1
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{}}}'
IFS= read -r line || exit 1
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s1"}}'
IFS= read -r line || exit 1
case "$line" in
  *'"method":"session/prompt"'*) ;;
  *) exit 2 ;;
esac
IFS= read -r line || exit 1
case "$line" in
  *'"method":"session/cancel"'*) exit 0 ;;
  *) exit 3 ;;
esac
"#
            .into(),
        ];
        let client = Arc::new(
            HarnessClient::spawn_without_editor_mcp(&config, Arc::new(|_| {}))
                .expect("spawn ACP cancellation fixture"),
        );
        client.initialize().expect("initialize ACP fixture");
        let session_id = client
            .new_session(&config.cwd, "agent", None, None)
            .expect("create ACP session");
        let prompt_client = client.clone();
        let (finished, result) = mpsc::channel();
        let prompt = thread::spawn(move || {
            let _ = finished.send(prompt_client.prompt(&session_id, "hello"));
        });

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !client
            .prompt_requests
            .lock()
            .expect("prompt request lock")
            .contains_key("s1")
        {
            assert!(
                std::time::Instant::now() < deadline,
                "ACP prompt was not registered"
            );
            thread::sleep(Duration::from_millis(1));
        }

        client.cancel("s1").expect("cancel ACP prompt");
        let error = result
            .recv_timeout(Duration::from_secs(1))
            .expect("cancel should settle the pending prompt")
            .expect_err("cancelled prompt must not succeed");
        assert!(error.to_string().contains("cancelled by AHEAD"));
        prompt.join().expect("join ACP prompt worker");
        client.shutdown();
    }

    #[cfg(unix)]
    #[test]
    fn shutdown_settles_requests_and_rejects_new_work() {
        let mut config = HarnessClientConfig::new("sh", std::env::temp_dir());
        config.args =
            vec!["-c".into(), "while IFS= read -r line; do :; done".into()];
        let client =
            HarnessClient::spawn_without_editor_mcp(&config, Arc::new(|_| {}))
                .expect("spawn silent ACP fixture");
        let (_, initialization) =
            client.send_request("initialize", json!({})).unwrap();
        let (_, prompt) = client
            .send_request("session/prompt", json!({"sessionId": "s1"}))
            .unwrap();

        client.shutdown();

        for response in [initialization, prompt] {
            let error = response
                .recv_timeout(Duration::from_secs(1))
                .expect("shutdown must settle every request")
                .expect_err("shutdown must not acknowledge a request as successful");
            assert_eq!(error, "ACP agent shut down by AHEAD");
        }
        assert!(!client.is_running());
        assert!(client.child.lock().unwrap().try_wait().unwrap().is_some());
        assert!(client.pending.lock().unwrap().is_empty());
        assert!(
            client
                .initialize()
                .unwrap_err()
                .to_string()
                .contains("shut down")
        );
        assert!(
            client
                .prompt("s1", "too late")
                .unwrap_err()
                .to_string()
                .contains("shut down")
        );
        client.shutdown();
    }

    #[test]
    fn keeps_agent_modes_distinct_from_ahead_policy_modes() {
        let result = json!({
            "modes": {
                "currentModeId": "medium",
                "availableModes": [
                    { "id": "off", "name": "Thinking: off" },
                    { "id": "medium", "name": "Thinking: medium" }
                ]
            }
        });

        assert_eq!(advertised_mode_ids(&result), ["off", "medium"]);
        assert!(
            !advertised_mode_ids(&result)
                .iter()
                .any(|mode| mode == "agent")
        );
    }

    #[test]
    fn explicit_model_requires_an_advertised_acp_choice() {
        assert!(advertised_model_option(&json!({}), "missing", None).is_err());
        let mut choices = vec![json!({
            "value": "provider/available",
            "name": "Available"
        })];
        choices.extend((1..10).map(|index| {
            let value = format!("provider/model-{index}");
            json!({ "value": value, "name": value })
        }));
        let response = json!({
            "configOptions": [{
                "id": "model",
                "category": "model",
                "type": "select",
                "options": choices
            }]
        });
        let error =
            advertised_model_option(&response, "unavailable", Some("provider"))
                .expect_err("unavailable models must be rejected")
                .to_string();
        assert!(error.contains("provider/available"));
        assert!(error.contains("(+2 more)"));
        assert!(!error.contains("provider/model-8"));
        assert_eq!(
            advertised_model_option(&response, "available", Some("provider"))
                .expect("advertised provider/model is selectable"),
            ("model".to_string(), "provider/available".to_string())
        );
        assert!(
            advertised_model_option(&response, "available", Some("other")).is_err()
        );
    }

    #[test]
    fn parses_ordered_flat_and_grouped_select_options() {
        let options = parse_config_options(&json!({
            "configOptions": [
                {
                    "id": "model",
                    "name": "Model",
                    "category": "model",
                    "type": "select",
                    "currentValue": "fast",
                    "options": [
                        { "value": "fast", "name": "Fast" },
                        { "group": "other", "name": "Other", "options": [
                            { "value": "deep", "name": "Deep" }
                        ] }
                    ]
                },
                { "id": "unknown", "name": "Unknown", "type": "future", "currentValue": "x" }
            ]
        }))
        .expect("configOptions list");
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].id, "model");
        assert_eq!(options[0].category.as_deref(), Some("model"));
        assert_eq!(
            options[0].current_value,
            AgentConfigOptionValue::Select("fast".to_string())
        );
        assert_eq!(
            options[0]
                .choices
                .iter()
                .map(|choice| choice.value.as_str())
                .collect::<Vec<_>>(),
            ["fast", "deep"]
        );
        assert_eq!(options[0].choices[1].name, "Other · Deep");
    }

    #[test]
    fn initial_config_response_does_not_overwrite_newer_update() {
        let state = Arc::new(Mutex::new(HashMap::new()));
        let seen: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_cb = seen.clone();
        let sink: HarnessSink = Arc::new(move |event| {
            seen_cb.lock().expect("event lock").push(event);
        });
        let option = |current| {
            json!({
                "configOptions": [{
                    "id": "model", "name": "Model", "type": "select",
                    "currentValue": current,
                    "options": [{ "value": "old", "name": "Old" }, { "value": "new", "name": "New" }]
                }]
            })
        };

        publish_config_options("s1", &option("new"), &state, &sink, true)
            .expect("publish update");
        publish_config_options("s1", &option("old"), &state, &sink, false)
            .expect("publish initial response");
        assert_eq!(
            state.lock().expect("state lock")["s1"][0].current_value,
            AgentConfigOptionValue::Select("new".to_string())
        );
        assert_eq!(seen.lock().expect("event lock").len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn agent_config_update_before_new_session_response_remains_authoritative() {
        let mut config = HarnessClientConfig::new("sh", std::env::temp_dir());
        config.args = vec![
            "-c".into(),
            r#"
IFS= read -r line || exit 1
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{}}}'
IFS= read -r line || exit 1
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"config_option_update","configOptions":[{"id":"model","name":"Model","type":"select","currentValue":"provider/new","options":[{"value":"provider/old","name":"Old"},{"value":"provider/new","name":"New"}]}]}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s1","configOptions":[{"id":"model","name":"Model","type":"select","currentValue":"provider/old","options":[{"value":"provider/old","name":"Old"},{"value":"provider/new","name":"New"}]}]}}'
"#
            .into(),
        ];
        let events: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let client = HarnessClient::spawn_without_editor_mcp(
            &config,
            Arc::new(move |event| captured.lock().expect("event lock").push(event)),
        )
        .expect("spawn ACP fixture");
        client.initialize().expect("initialize fixture");
        let session_id = client
            .new_session(&config.cwd, "agent", None, None)
            .expect("create session with advertised options");
        let published_options = events
            .lock()
            .expect("event lock")
            .iter()
            .filter_map(|event| match event {
                HarnessEvent::ConfigOptions { options, .. } => Some(options.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        client.shutdown();

        assert_eq!(session_id, "s1");
        assert_eq!(published_options.len(), 1);
        assert_eq!(
            published_options
                .first()
                .and_then(|options| options.first())
                .expect("model option")
                .current_value,
            AgentConfigOptionValue::Select("provider/new".to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    fn selects_external_model_through_acp_config_option() {
        let mut config = HarnessClientConfig::new("sh", std::env::temp_dir());
        config.default_config_options.insert(
            "chosen-model".to_string(),
            AgentConfigOptionValue::Select("openai/gpt-4.1".to_string()),
        );
        config.args = vec![
            "-c".into(),
            r#"
IFS= read -r line || exit 1
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{}}}'
IFS= read -r line || exit 1
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s1","configOptions":[{"id":"chosen-model","name":"Model","category":"model","type":"select","currentValue":"old","options":[{"group":"available","name":"Available","options":[{"value":"openai/gpt-4.1-mini","name":"Mini"},{"value":"openai/gpt-4.1","name":"Other"}]}]}]}}'
IFS= read -r line || exit 1
case "$line" in
  *'"method":"session/set_config_option"'*'"configId":"chosen-model"'*'"value":"openai/gpt-4.1-mini"'*) ;;
  *) exit 2 ;;
esac
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"configOptions":[{"id":"chosen-model","name":"Model","category":"model","type":"select","currentValue":"openai/gpt-4.1-mini","options":[{"group":"available","name":"Available","options":[{"value":"openai/gpt-4.1-mini","name":"Mini"},{"value":"openai/gpt-4.1","name":"Other"}]}]}]}}'
"#
            .into(),
        ];
        let seen: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_cb = seen.clone();
        let client = HarnessClient::spawn_without_editor_mcp(
            &config,
            Arc::new(move |event| {
                seen_cb.lock().expect("event lock").push(event);
            }),
        )
        .expect("spawn ACP fixture");
        client.initialize().expect("initialize fixture");
        let session_id = client
            .new_session(&config.cwd, "agent", Some("gpt-4.1-mini"), Some("openai"))
            .expect("select advertised model");
        assert_eq!(session_id, "s1");
        assert!(matches!(
            seen.lock().expect("event lock").as_slice(),
            [HarnessEvent::ConfigOptions { options: first, .. }, HarnessEvent::ConfigOptions { options: second, .. }]
                if first.len() == 1 && second.len() == 1
        ));
        client.shutdown();
    }

    #[cfg(unix)]
    #[test]
    fn applies_saved_agent_defaults_to_advertised_session_options() {
        let mut config = HarnessClientConfig::new("sh", std::env::temp_dir());
        config.default_config_options = HashMap::from([
            (
                "model".to_string(),
                AgentConfigOptionValue::Select("provider/new".to_string()),
            ),
            ("verbose".to_string(), AgentConfigOptionValue::Boolean(true)),
        ]);
        config.args = vec![
            "-c".into(),
            r#"
IFS= read -r line || exit 1
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{}}}'
IFS= read -r line || exit 1
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s1","configOptions":[{"id":"model","name":"Model","type":"select","currentValue":"provider/old","options":[{"value":"provider/old","name":"Old"},{"value":"provider/new","name":"New"}]},{"id":"verbose","name":"Verbose","type":"boolean","currentValue":false}]}}'
IFS= read -r line || exit 1
case "$line" in
  *'"method":"session/set_config_option"'*'"configId":"model"'*'"value":"provider/new"'*) ;;
  *) exit 2 ;;
esac
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"configOptions":[{"id":"model","name":"Model","type":"select","currentValue":"provider/new","options":[{"value":"provider/old","name":"Old"},{"value":"provider/new","name":"New"}]},{"id":"verbose","name":"Verbose","type":"boolean","currentValue":false}]}}'
IFS= read -r line || exit 1
case "$line" in
  *'"method":"session/set_config_option"'*'"configId":"verbose"'*'"value":true'*) ;;
  *) exit 3 ;;
esac
printf '%s\n' '{"jsonrpc":"2.0","id":4,"result":{"configOptions":[{"id":"model","name":"Model","type":"select","currentValue":"provider/new","options":[{"value":"provider/old","name":"Old"},{"value":"provider/new","name":"New"}]},{"id":"verbose","name":"Verbose","type":"boolean","currentValue":true}]}}'
"#
                .into(),
        ];
        let events: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let client = HarnessClient::spawn_without_editor_mcp(
            &config,
            Arc::new(move |event| captured.lock().expect("event lock").push(event)),
        )
        .expect("spawn ACP fixture");
        client.initialize().expect("initialize fixture");
        let session_id = client
            .new_session(&config.cwd, "agent", None, None)
            .expect("create session with saved defaults");

        let final_options = events
            .lock()
            .expect("event lock")
            .iter()
            .rev()
            .find_map(|event| match event {
                HarnessEvent::ConfigOptions { options, .. } => Some(options.clone()),
                _ => None,
            })
            .expect("final config options");
        client.shutdown();

        assert_eq!(session_id, "s1");
        let model_option = final_options.first().expect("model option");
        let verbose_option = final_options.get(1).expect("verbose option");
        assert_eq!(
            model_option.current_value,
            AgentConfigOptionValue::Select("provider/new".to_string())
        );
        assert_eq!(
            verbose_option.current_value,
            AgentConfigOptionValue::Boolean(true)
        );
    }

    #[cfg(unix)]
    #[test]
    fn updates_and_republishes_a_live_acp_config_option() {
        let temporary = tempfile::tempdir().expect("temporary ACP settings");
        let defaults_path = temporary.path().join("pi-acp.config-options.json");
        let mut config = HarnessClientConfig::new("sh", std::env::temp_dir());
        config.default_config_options_path = Some(defaults_path.clone());
        config.args = vec![
            "-c".into(),
            r#"
IFS= read -r line || exit 1
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{}}}'
IFS= read -r line || exit 1
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s1","configOptions":[{"id":"model","name":"Model","category":"model","type":"select","currentValue":"provider/old","options":[{"value":"provider/old","name":"Old"},{"value":"provider/new","name":"New"}]}]}}'
IFS= read -r line || exit 1
case "$line" in
  *'"method":"session/set_config_option"'*'"configId":"model"'*'"value":"provider/new"'*) ;;
  *) exit 2 ;;
esac
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"configOptions":[{"id":"model","name":"Model","category":"model","type":"select","currentValue":"provider/new","options":[{"value":"provider/old","name":"Old"},{"value":"provider/new","name":"New"}]}]}}'
"#
            .into(),
        ];
        let events: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let client = HarnessClient::spawn_without_editor_mcp(
            &config,
            Arc::new(move |event| captured.lock().expect("event lock").push(event)),
        )
        .expect("spawn ACP fixture");
        client.initialize().expect("initialize fixture");
        let session_id = client
            .new_session(&config.cwd, "agent", None, None)
            .expect("create session with advertised options");

        let rejected = client.set_config_option(
            &session_id,
            "model",
            &AgentConfigOptionValue::Select("provider/missing".to_string()),
        );
        let updated = client.set_config_option(
            &session_id,
            "model",
            &AgentConfigOptionValue::Select("provider/new".to_string()),
        );
        let current_values = events
            .lock()
            .expect("event lock")
            .iter()
            .filter_map(|event| match event {
                HarnessEvent::ConfigOptions { options, .. } => {
                    options.first().map(|option| option.current_value.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        client.shutdown();

        assert!(rejected.is_err());
        updated.expect("set advertised option");
        let persisted: HashMap<String, AgentConfigOptionValue> =
            serde_json::from_slice(
                &std::fs::read(defaults_path).expect("read saved defaults"),
            )
            .expect("parse saved defaults");
        assert_eq!(
            persisted.get("model"),
            Some(&AgentConfigOptionValue::Select("provider/new".to_string()))
        );
        assert_eq!(
            current_values,
            [
                AgentConfigOptionValue::Select("provider/old".to_string()),
                AgentConfigOptionValue::Select("provider/new".to_string())
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn advertises_and_sets_boolean_acp_config_options() {
        let mut config = HarnessClientConfig::new("sh", std::env::temp_dir());
        config.args = vec![
            "-c".into(),
            r#"
IFS= read -r line || exit 1
case "$line" in *'"boolean":{}'*) ;; *) exit 10 ;; esac
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{}}}'
IFS= read -r line || exit 1
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s1","configOptions":[{"id":"verbose","name":"Verbose output","type":"boolean","currentValue":false}]}}'
IFS= read -r line || exit 1
case "$line" in *'"method":"session/set_config_option"'*'"configId":"verbose"'*'"value":true'*) ;; *) exit 11 ;; esac
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"configOptions":[{"id":"verbose","name":"Verbose output","type":"boolean","currentValue":true}]}}'
"#
                .into(),
        ];
        let events: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let client = HarnessClient::spawn_without_editor_mcp(
            &config,
            Arc::new(move |event| captured.lock().expect("event lock").push(event)),
        )
        .expect("spawn ACP boolean fixture");
        client
            .initialize()
            .expect("initialize with boolean support");
        let session_id = client
            .new_session(&config.cwd, "agent", None, None)
            .expect("create session with boolean option");

        let rejected = client.set_config_option(
            &session_id,
            "verbose",
            &AgentConfigOptionValue::Select("true".to_string()),
        );
        let updated = client.set_config_option(
            &session_id,
            "verbose",
            &AgentConfigOptionValue::Boolean(true),
        );
        let boolean_values = events
            .lock()
            .expect("event lock")
            .iter()
            .filter_map(|event| match event {
                HarnessEvent::ConfigOptions { options, .. } => {
                    options.first().map(|option| option.current_value.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        client.shutdown();

        assert!(rejected.is_err());
        updated.expect("set advertised boolean option");
        assert_eq!(
            boolean_values,
            [
                AgentConfigOptionValue::Boolean(false),
                AgentConfigOptionValue::Boolean(true)
            ]
        );
    }

    #[test]
    fn records_advertised_session_lifecycle_capabilities() {
        let capabilities = session_capabilities(&json!({
            "loadSession": false,
            "sessionCapabilities": { "resume": {} }
        }));
        assert!(!capabilities.load_session);
        assert!(capabilities.resume_session);
    }

    #[test]
    fn closed_agent_releases_pending_request() {
        let mut config = HarnessClientConfig::new(
            "sh",
            std::env::current_dir().expect("current directory"),
        );
        config.args = vec!["-c".into(), "sleep 0.1".into()];
        let client =
            HarnessClient::spawn_without_editor_mcp(&config, Arc::new(|_| {}))
                .expect("spawn fixture agent");
        let started = std::time::Instant::now();
        let result = client.initialize();
        assert!(result.is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}
