use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use crossbeam_channel::{Receiver, Sender};
use lsp_types::{
    CancelParams, CompletionResponse, LogMessageParams, ProgressParams,
    PublishDiagnosticsParams, ShowMessageParams, SignatureHelp,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::{
    RequestId, RpcError, RpcMessage,
    dap_types::{
        self, DapId, DebugTerminalRequest, Scope, StackFrame, Stopped, ThreadId,
        Variable,
    },
    file::PathObject,
    plugin::PluginId,
    proxy::ProxyStatus,
    source_control::DiffInfo,
    terminal::TermId,
};

pub enum CoreRpc {
    Request(RequestId, CoreRequest),
    Notification(Box<CoreNotification>), // Box it since clippy complains
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChanged {
    Change(String),
    Delete,
}

#[expect(
    clippy::large_enum_variant,
    reason = "completion/signature responses carry payloads by value; consumed once per notification"
)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "method", content = "params")]
pub enum CoreNotification {
    ProxyStatus {
        status: ProxyStatus,
    },
    OpenFileChanged {
        path: PathBuf,
        content: FileChanged,
    },
    CompletionResponse {
        request_id: usize,
        input: String,
        resp: Option<CompletionResponse>,
        plugin_id: PluginId,
    },
    SignatureHelpResponse {
        request_id: usize,
        resp: SignatureHelp,
        plugin_id: PluginId,
    },
    OpenPaths {
        paths: Vec<PathObject>,
    },
    WorkspaceFileChange {
        generation: u64,
        paths: Vec<PathBuf>,
    },
    WorkspaceFileRename {
        generation: u64,
        old_path: PathBuf,
        new_path: PathBuf,
    },
    PublishDiagnostics {
        diagnostics: PublishDiagnosticsParams,
    },
    ServerStatus {
        params: ServerStatusParams,
    },
    LanguageExtensionIssues {
        issues: Vec<LanguageExtensionIssue>,
    },
    LanguageServerStatusesCleared,
    WorkDoneProgress {
        progress: ProgressParams,
    },
    ShowMessage {
        title: String,
        message: ShowMessageParams,
    },
    LogMessage {
        message: LogMessageParams,
        target: String,
    },
    LspCancel {
        params: CancelParams,
    },
    HomeDir {
        path: PathBuf,
    },
    DiffInfo {
        diff: DiffInfo,
    },
    UpdateTerminal {
        term_id: TermId,
        content: Vec<u8>,
    },
    TerminalLaunchFailed {
        term_id: TermId,
        error: String,
    },
    TerminalProcessId {
        term_id: TermId,
        process_id: Option<u32>,
    },
    TerminalProcessStopped {
        term_id: TermId,
        exit_code: Option<i32>,
    },
    RunInTerminal {
        request: DebugTerminalRequest,
    },
    Log {
        level: LogLevel,
        message: String,
        target: Option<String>,
    },
    DapStopped {
        dap_id: DapId,
        stopped: Stopped,
        stack_frames: HashMap<ThreadId, Vec<StackFrame>>,
        variables: Vec<(Scope, Vec<Variable>)>,
    },
    DapSessionState {
        dap_id: DapId,
        state: dap_types::DapSessionState,
    },
    DapError {
        dap_id: DapId,
        message: String,
    },
    DapBreakpointsResp {
        dap_id: DapId,
        path: PathBuf,
        breakpoints: Vec<dap_types::Breakpoint>,
    },
    AheadNotification {
        notification: crate::ahead::AheadNotification,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "method", content = "params")]
pub enum CoreRequest {
    ApplyWorkspaceEdit(lsp_types::ApplyWorkspaceEditParams),
    GitCredentialPrompt { prompt: String, secret: bool },
}

impl std::fmt::Debug for CoreRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApplyWorkspaceEdit(params) => formatter
                .debug_tuple("ApplyWorkspaceEdit")
                .field(params)
                .finish(),
            Self::GitCredentialPrompt { secret, .. } => formatter
                .debug_struct("GitCredentialPrompt")
                .field("prompt", &"<redacted>")
                .field("secret", secret)
                .finish(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "method", content = "params")]
pub enum CoreResponse {
    ApplyWorkspaceEdit(lsp_types::ApplyWorkspaceEditResponse),
    GitCredentialAnswer { answer: Option<String> },
}

impl std::fmt::Debug for CoreResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApplyWorkspaceEdit(response) => formatter
                .debug_tuple("ApplyWorkspaceEdit")
                .field(response)
                .finish(),
            Self::GitCredentialAnswer { answer } => formatter
                .debug_struct("GitCredentialAnswer")
                .field("answer", &answer.as_ref().map(|_| "<redacted>"))
                .finish(),
        }
    }
}

pub type CoreMessage = RpcMessage<CoreRequest, CoreNotification, CoreResponse>;

pub trait CoreHandler {
    fn handle_notification(&mut self, rpc: CoreNotification);
    fn handle_request(&mut self, id: RequestId, rpc: CoreRequest);
}

#[derive(Clone)]
pub struct CoreRpcHandler {
    tx: Sender<CoreRpc>,
    rx: Receiver<CoreRpc>,
    id: Arc<AtomicU64>,
    #[expect(
        clippy::type_complexity,
        reason = "message-map type alias would hide the channel triple; keep explicit"
    )]
    pending: Arc<Mutex<HashMap<u64, Sender<Result<CoreResponse, RpcError>>>>>,
}

impl CoreRpcHandler {
    pub fn new() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        Self {
            tx,
            rx,
            id: Arc::new(AtomicU64::new(0)),
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn mainloop<H>(&self, handler: &mut H)
    where
        H: CoreHandler,
    {
        for msg in &self.rx {
            match msg {
                CoreRpc::Request(id, rpc) => {
                    handler.handle_request(id, rpc);
                }
                CoreRpc::Notification(rpc) => {
                    handler.handle_notification(*rpc);
                }
                CoreRpc::Shutdown => {
                    return;
                }
            }
        }
    }

    pub fn rx(&self) -> &Receiver<CoreRpc> {
        &self.rx
    }

    pub fn handle_response(
        &self,
        id: RequestId,
        response: Result<CoreResponse, RpcError>,
    ) {
        let tx = { self.pending.lock().remove(&id) };
        if let Some(tx) = tx {
            if tx.send(response).is_err() {
                tracing::error!("Core RPC response receiver was dropped");
            }
        }
    }

    pub fn request(&self, request: CoreRequest) -> Result<CoreResponse, RpcError> {
        self.request_with_timeout(request, Duration::from_secs(30))
    }

    pub fn request_with_timeout(
        &self,
        request: CoreRequest,
        timeout: Duration,
    ) -> Result<CoreResponse, RpcError> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let id = self.id.fetch_add(1, Ordering::Relaxed);
        {
            let mut pending = self.pending.lock();
            pending.insert(id, tx);
        }
        if let Err(err) = self.tx.send(CoreRpc::Request(id, request)) {
            drop(self.pending.lock().remove(&id));
            return Err(RpcError {
                code: 0,
                message: format!("Could not send core request: {err}"),
            });
        }
        rx.recv_timeout(timeout).unwrap_or_else(|error| {
            drop(self.pending.lock().remove(&id));
            Err(RpcError {
                code: 0,
                message: match error {
                    crossbeam_channel::RecvTimeoutError::Timeout => {
                        format!(
                            "Core request timed out after {}s",
                            timeout.as_secs()
                        )
                    }
                    crossbeam_channel::RecvTimeoutError::Disconnected => {
                        "Core request channel closed".into()
                    }
                },
            })
        })
    }

    pub fn shutdown(&self) {
        if let Err(err) = self.tx.send(CoreRpc::Shutdown) {
            tracing::error!("{:?}", err);
        }
    }

    pub fn notification(&self, notification: CoreNotification) {
        if let Err(err) = self.tx.send(CoreRpc::Notification(Box::new(notification)))
        {
            tracing::error!("{:?}", err);
        }
    }

    pub fn workspace_file_change(&self, generation: u64, paths: Vec<PathBuf>) {
        self.notification(CoreNotification::WorkspaceFileChange {
            generation,
            paths,
        });
    }

    pub fn workspace_file_renamed(
        &self,
        generation: u64,
        old_path: PathBuf,
        new_path: PathBuf,
    ) {
        self.notification(CoreNotification::WorkspaceFileRename {
            generation,
            old_path,
            new_path,
        });
    }

    pub fn diff_info(&self, diff: DiffInfo) {
        self.notification(CoreNotification::DiffInfo { diff });
    }

    pub fn open_file_changed(&self, path: PathBuf, content: FileChanged) {
        self.notification(CoreNotification::OpenFileChanged { path, content });
    }

    pub fn completion_response(
        &self,
        request_id: usize,
        input: String,
        resp: Option<CompletionResponse>,
        plugin_id: PluginId,
    ) {
        self.notification(CoreNotification::CompletionResponse {
            request_id,
            input,
            resp,
            plugin_id,
        });
    }

    pub fn signature_help_response(
        &self,
        request_id: usize,
        resp: SignatureHelp,
        plugin_id: PluginId,
    ) {
        self.notification(CoreNotification::SignatureHelpResponse {
            request_id,
            resp,
            plugin_id,
        });
    }

    pub fn run_in_terminal(&self, request: DebugTerminalRequest) {
        self.notification(CoreNotification::RunInTerminal { request });
    }

    pub fn log(&self, level: LogLevel, message: String, target: Option<String>) {
        self.notification(CoreNotification::Log {
            level,
            message,
            target,
        });
    }

    pub fn publish_diagnostics(&self, diagnostics: PublishDiagnosticsParams) {
        self.notification(CoreNotification::PublishDiagnostics { diagnostics });
    }

    pub fn server_status(&self, params: ServerStatusParams) {
        self.notification(CoreNotification::ServerStatus { params });
    }

    pub fn language_extension_issues(&self, issues: Vec<LanguageExtensionIssue>) {
        self.notification(CoreNotification::LanguageExtensionIssues { issues });
    }

    pub fn clear_language_server_statuses(&self) {
        self.notification(CoreNotification::LanguageServerStatusesCleared);
    }

    pub fn work_done_progress(&self, progress: ProgressParams) {
        self.notification(CoreNotification::WorkDoneProgress { progress });
    }

    pub fn show_message(&self, title: String, message: ShowMessageParams) {
        self.notification(CoreNotification::ShowMessage { title, message });
    }

    pub fn log_message(&self, message: LogMessageParams, target: String) {
        self.notification(CoreNotification::LogMessage { message, target });
    }

    pub fn cancel(&self, params: CancelParams) {
        self.notification(CoreNotification::LspCancel { params });
    }

    pub fn terminal_process_id(&self, term_id: TermId, process_id: Option<u32>) {
        self.notification(CoreNotification::TerminalProcessId {
            term_id,
            process_id,
        });
    }

    pub fn terminal_process_stopped(&self, term_id: TermId, exit_code: Option<i32>) {
        self.notification(CoreNotification::TerminalProcessStopped {
            term_id,
            exit_code,
        });
    }

    pub fn terminal_launch_failed(&self, term_id: TermId, error: String) {
        self.notification(CoreNotification::TerminalLaunchFailed { term_id, error });
    }

    pub fn update_terminal(&self, term_id: TermId, content: Vec<u8>) {
        self.notification(CoreNotification::UpdateTerminal { term_id, content });
    }

    pub fn dap_stopped(
        &self,
        dap_id: DapId,
        stopped: Stopped,
        stack_frames: HashMap<ThreadId, Vec<StackFrame>>,
        variables: Vec<(Scope, Vec<Variable>)>,
    ) {
        self.notification(CoreNotification::DapStopped {
            dap_id,
            stopped,
            stack_frames,
            variables,
        });
    }

    pub fn dap_session_state(
        &self,
        dap_id: DapId,
        state: dap_types::DapSessionState,
    ) {
        self.notification(CoreNotification::DapSessionState { dap_id, state });
    }

    pub fn dap_error(&self, dap_id: DapId, message: String) {
        self.notification(CoreNotification::DapError { dap_id, message });
    }

    pub fn dap_breakpoints_resp(
        &self,
        dap_id: DapId,
        path: PathBuf,
        breakpoints: Vec<dap_types::Breakpoint>,
    ) {
        self.notification(CoreNotification::DapBreakpointsResp {
            dap_id,
            path,
            breakpoints,
        });
    }

    pub fn home_dir(&self, path: PathBuf) {
        self.notification(CoreNotification::HomeDir { path });
    }

    pub fn ahead_notification(&self, notification: crate::ahead::AheadNotification) {
        self.notification(CoreNotification::AheadNotification { notification });
    }
}

impl Default for CoreRpcHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogLevel {
    Info = 0,
    Warn = 1,
    Error = 2,
    Debug = 3,
    Trace = 4,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerStatusParams {
    #[serde(default)]
    pub server_name: Option<String>,
    health: String,
    quiescent: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LanguageExtensionIssue {
    pub name: String,
    pub message: String,
}

impl ServerStatusParams {
    pub fn starting(server_name: String) -> Self {
        Self {
            server_name: Some(server_name),
            health: "starting".into(),
            quiescent: false,
            message: None,
        }
    }
    pub fn ready(server_name: String) -> Self {
        Self {
            server_name: Some(server_name),
            health: "ok".to_string(),
            quiescent: true,
            message: None,
        }
    }

    pub fn failed(server_name: String, message: String) -> Self {
        Self {
            server_name: Some(server_name),
            health: "error".to_string(),
            quiescent: true,
            message: Some(message),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.health.as_str() == "ok"
    }

    pub fn is_quiescent(&self) -> bool {
        self.quiescent
    }
}

#[cfg(test)]
mod tests {
    use super::{CoreMessage, CoreRequest, CoreResponse, CoreRpcHandler};
    use crate::RpcMessage;
    use std::time::Duration;

    #[test]
    fn workspace_edit_core_messages_round_trip_through_stdio() {
        let request: CoreMessage = RpcMessage::Request(
            7,
            CoreRequest::ApplyWorkspaceEdit(lsp_types::ApplyWorkspaceEditParams {
                label: Some("Apply edit".into()),
                edit: lsp_types::WorkspaceEdit::default(),
            }),
        );
        let mut request_bytes = Vec::new();
        crate::stdio::write_msg(&mut request_bytes, request)
            .expect("write Core RPC request");
        let decoded_request: Option<CoreMessage> =
            crate::stdio::read_msg(&mut std::io::Cursor::new(request_bytes))
                .expect("read Core RPC request");
        assert!(matches!(
            decoded_request,
            Some(RpcMessage::Request(
                7,
                CoreRequest::ApplyWorkspaceEdit(params)
            )) if params.label.as_deref() == Some("Apply edit")
        ));

        let response: CoreMessage = RpcMessage::Response(
            7,
            CoreResponse::ApplyWorkspaceEdit(
                lsp_types::ApplyWorkspaceEditResponse {
                    applied: true,
                    failure_reason: None,
                    failed_change: None,
                },
            ),
        );
        let mut response_bytes = Vec::new();
        crate::stdio::write_msg(&mut response_bytes, response)
            .expect("write Core RPC response");
        let decoded_response: Option<CoreMessage> =
            crate::stdio::read_msg(&mut std::io::Cursor::new(response_bytes))
                .expect("read Core RPC response");
        assert!(matches!(
            decoded_response,
            Some(RpcMessage::Response(
                7,
                CoreResponse::ApplyWorkspaceEdit(response)
            )) if response.applied
        ));
    }

    #[test]
    fn timed_out_core_requests_release_their_pending_slot() {
        let rpc = CoreRpcHandler::new();
        let result = rpc.request_with_timeout(
            CoreRequest::ApplyWorkspaceEdit(lsp_types::ApplyWorkspaceEditParams {
                label: None,
                edit: lsp_types::WorkspaceEdit::default(),
            }),
            Duration::from_millis(1),
        );

        assert!(result.is_err());
        assert!(rpc.pending.lock().is_empty());
    }

    #[test]
    fn git_credential_prompt_round_trips_through_core_rpc() {
        let request: CoreMessage = RpcMessage::Request(
            19,
            CoreRequest::GitCredentialPrompt {
                prompt: "Password for https://example.test".into(),
                secret: true,
            },
        );
        let mut bytes = Vec::new();
        crate::stdio::write_msg(&mut bytes, request)
            .expect("write Git credential request");
        let decoded: Option<CoreMessage> =
            crate::stdio::read_msg(&mut std::io::Cursor::new(bytes))
                .expect("read Git credential request");
        assert!(matches!(
            decoded,
            Some(RpcMessage::Request(
                19,
                CoreRequest::GitCredentialPrompt { prompt, secret: true }
            )) if prompt == "Password for https://example.test"
        ));

        let response: CoreMessage = RpcMessage::Response(
            19,
            CoreResponse::GitCredentialAnswer {
                answer: Some("temporary-secret".into()),
            },
        );
        let mut bytes = Vec::new();
        crate::stdio::write_msg(&mut bytes, response)
            .expect("write Git credential answer");
        let decoded: Option<CoreMessage> =
            crate::stdio::read_msg(&mut std::io::Cursor::new(bytes))
                .expect("read Git credential answer");
        assert!(matches!(
            decoded,
            Some(RpcMessage::Response(
                19,
                CoreResponse::GitCredentialAnswer {
                    answer: Some(answer)
                }
            )) if answer == "temporary-secret"
        ));
    }

    #[test]
    fn git_credential_response_debug_redacts_the_answer() {
        let response = CoreResponse::GitCredentialAnswer {
            answer: Some("disposable-secret".into()),
        };

        let debug = format!("{response:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("disposable-secret"));

        let request = CoreRequest::GitCredentialPrompt {
            prompt: "Password for https://private.example.test".into(),
            secret: true,
        };
        let debug = format!("{request:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("private.example.test"));
    }
}
