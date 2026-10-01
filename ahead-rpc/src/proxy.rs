use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use crate::{
    RequestId, RpcError, RpcMessage,
    buffer::BufferId,
    dap_types::{self, DapId, RunDebugConfig, SourceBreakpoint, ThreadId},
    delta::AheadDelta,
    file::{FileNodeItem, PathObject},
    file_line::FileLine,
    plugin::PluginId,
    source_control::{FileDiff, GitFileState},
    style::SemanticStyles,
    terminal::{TermId, TerminalProfile},
};
use crossbeam_channel::{Receiver, Sender};
use indexmap::IndexMap;
use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyItem, CodeAction, CodeActionResponse,
    CodeLens, CompletionItem, Diagnostic, DocumentSymbolResponse, FoldingRange,
    GotoDefinitionResponse, Hover, InlayHint, InlineCompletionResponse,
    InlineCompletionTriggerKind, Location, Position, PrepareRenameResponse,
    SelectionRange, SymbolInformation, TextDocumentItem, TextEdit, WorkspaceEdit,
    request::{GotoImplementationResponse, GotoTypeDefinitionResponse},
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

#[expect(
    clippy::large_enum_variant,
    reason = "RPC envelope dominated by small variants; boxing the large one adds indirection on every message"
)]
pub enum ProxyRpc {
    Request(RequestId, ProxyRequest),
    Notification(ProxyNotification),
    Shutdown,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum ProxyStatus {
    Connecting,
    Connected,
    Disconnected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMatch {
    pub line: usize,
    pub start: usize,
    pub end_line: usize,
    pub end: usize,
    pub line_content: String,
}

#[expect(
    clippy::large_enum_variant,
    reason = "request envelope carries buffer/search payloads by value; consumed once per dispatch"
)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "method", content = "params")]
pub enum ProxyRequest {
    NewBuffer {
        buffer_id: BufferId,
        path: PathBuf,
    },
    BufferHead {
        path: PathBuf,
    },
    GitFileState {
        path: PathBuf,
        content: String,
    },
    GlobalSearch {
        pattern: String,
        case_sensitive: bool,
        whole_word: bool,
        is_regex: bool,
    },
    WorkspaceFiles {
        request_id: u64,
    },
    CompletionResolve {
        plugin_id: PluginId,
        completion_item: Box<CompletionItem>,
    },
    CodeActionResolve {
        plugin_id: PluginId,
        action_item: Box<CodeAction>,
    },
    GetHover {
        request_id: usize,
        path: PathBuf,
        position: Position,
    },
    GetSignature {
        buffer_id: BufferId,
        position: Position,
    },
    GetSelectionRange {
        path: PathBuf,
        positions: Vec<Position>,
    },
    GitGetRemoteFileUrl {
        file: PathBuf,
    },
    GetReferences {
        path: PathBuf,
        position: Position,
    },
    GotoImplementation {
        path: PathBuf,
        position: Position,
    },
    GetDefinition {
        request_id: usize,
        path: PathBuf,
        position: Position,
    },
    ShowCallHierarchy {
        path: PathBuf,
        position: Position,
    },
    CallHierarchyIncoming {
        path: PathBuf,
        call_hierarchy_item: CallHierarchyItem,
    },
    GetTypeDefinition {
        request_id: usize,
        path: PathBuf,
        position: Position,
    },
    GetInlayHints {
        path: PathBuf,
    },
    GetInlineCompletions {
        path: PathBuf,
        position: Position,
        trigger_kind: InlineCompletionTriggerKind,
    },
    GetSemanticTokens {
        path: PathBuf,
    },
    LspFoldingRange {
        path: PathBuf,
    },
    PrepareRename {
        path: PathBuf,
        position: Position,
    },
    Rename {
        path: PathBuf,
        position: Position,
        new_name: String,
    },
    GetCodeActions {
        path: PathBuf,
        position: Position,
        diagnostics: Vec<Diagnostic>,
    },
    GetCodeLens {
        path: PathBuf,
    },
    GetCodeLensResolve {
        code_lens: CodeLens,
        path: PathBuf,
    },
    GetDocumentSymbols {
        path: PathBuf,
    },
    GetWorkspaceSymbols {
        /// The search query
        query: String,
    },
    GetDocumentFormatting {
        path: PathBuf,
    },
    GetOpenFilesContent {},
    ReadDir {
        path: PathBuf,
    },
    Save {
        rev: u64,
        path: PathBuf,
        /// Whether to create the parent directories if they do not exist.
        create_parents: bool,
    },
    SaveEditorBuffer {
        path: PathBuf,
        content: String,
    },
    SaveBufferAs {
        buffer_id: BufferId,
        path: PathBuf,
        rev: u64,
        content: String,
        /// Whether to create the parent directories if they do not exist.
        create_parents: bool,
    },
    CreateFile {
        path: PathBuf,
    },
    CreateDirectory {
        path: PathBuf,
    },
    TrashPath {
        path: PathBuf,
    },
    DuplicatePath {
        existing_path: PathBuf,
        new_path: PathBuf,
    },
    RenamePath {
        from: PathBuf,
        to: PathBuf,
    },
    TestCreateAtPath {
        path: PathBuf,
    },
    DapVariable {
        dap_id: DapId,
        reference: usize,
    },
    DapGetScopes {
        dap_id: DapId,
        frame_id: usize,
    },
    ReferencesResolve {
        items: Vec<Location>,
    },
    AheadRequest {
        request: crate::ahead::AheadRequest,
    },
    InstallLanguageExtension {
        url: String,
        extension_id: String,
    },
}

#[expect(
    clippy::large_enum_variant,
    reason = "notification envelope carries init/plugin payloads by value; consumed once per dispatch"
)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "method", content = "params")]
pub enum ProxyNotification {
    Initialize {
        workspace: Option<PathBuf>,
        window_id: usize,
        tab_id: usize,
    },
    OpenFileChanged {
        path: PathBuf,
    },
    OpenPaths {
        paths: Vec<PathObject>,
    },
    Shutdown {},
    CancelWorkspaceFiles {
        request_id: u64,
    },
    Completion {
        request_id: usize,
        path: PathBuf,
        input: String,
        position: Position,
    },
    SignatureHelp {
        request_id: usize,
        path: PathBuf,
        position: Position,
    },
    Update {
        path: PathBuf,
        delta: AheadDelta,
        rev: u64,
    },
    EditorSnapshot {
        path: PathBuf,
        content: String,
    },
    CloseEditorBuffer {
        path: PathBuf,
    },
    NewTerminal {
        term_id: TermId,
        profile: TerminalProfile,
    },
    GitCommit {
        message: String,
        diffs: Vec<FileDiff>,
    },
    GitCheckout {
        reference: String,
    },
    GitDiscardFilesChanges {
        files: Vec<PathBuf>,
    },
    GitDiscardWorkspaceChanges {},
    GitInit {},
    LspCancel {
        id: i32,
    },
    RestartLanguageServers {},
    TerminalWrite {
        term_id: TermId,
        content: String,
    },
    TerminalResize {
        term_id: TermId,
        width: usize,
        height: usize,
    },
    TerminalClose {
        term_id: TermId,
    },
    DapStart {
        config: RunDebugConfig,
        breakpoints: HashMap<PathBuf, Vec<SourceBreakpoint>>,
    },
    DapTerminalResponse {
        response: crate::dap_types::DebugTerminalResponse,
    },
    DapContinue {
        dap_id: DapId,
        thread_id: ThreadId,
    },
    DapStepOver {
        dap_id: DapId,
        thread_id: ThreadId,
    },
    DapStepInto {
        dap_id: DapId,
        thread_id: ThreadId,
    },
    DapStepOut {
        dap_id: DapId,
        thread_id: ThreadId,
    },
    DapPause {
        dap_id: DapId,
        thread_id: ThreadId,
    },
    DapStop {
        dap_id: DapId,
    },
    DapDisconnect {
        dap_id: DapId,
    },
    DapRestart {
        dap_id: DapId,
        breakpoints: HashMap<PathBuf, Vec<SourceBreakpoint>>,
    },
    DapSetBreakpoints {
        dap_id: DapId,
        path: PathBuf,
        breakpoints: Vec<SourceBreakpoint>,
    },
    AheadNotification {
        notification: crate::ahead::AheadNotification,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "method", content = "params")]
pub enum ProxyResponse {
    GitFileState {
        state: GitFileState,
    },
    GitGetRemoteFileUrl {
        file_url: String,
    },
    NewBufferResponse {
        content: String,
        read_only: bool,
    },
    BufferHeadResponse {
        version: String,
        content: String,
    },
    ReadDirResponse {
        items: Vec<FileNodeItem>,
    },
    CompletionResolveResponse {
        item: Box<CompletionItem>,
    },
    CodeActionResolveResponse {
        item: Box<CodeAction>,
    },
    HoverResponse {
        request_id: usize,
        hover: Hover,
    },
    GetDefinitionResponse {
        request_id: usize,
        definition: GotoDefinitionResponse,
    },
    ShowCallHierarchyResponse {
        items: Option<Vec<CallHierarchyItem>>,
    },
    CallHierarchyIncomingResponse {
        items: Option<Vec<CallHierarchyIncomingCall>>,
    },
    GetTypeDefinition {
        request_id: usize,
        definition: GotoTypeDefinitionResponse,
    },
    GetReferencesResponse {
        references: Vec<Location>,
    },
    GetCodeActionsResponse {
        plugin_id: PluginId,
        resp: CodeActionResponse,
    },
    LspFoldingRangeResponse {
        plugin_id: PluginId,
        resp: Option<Vec<FoldingRange>>,
    },
    GetCodeLensResponse {
        plugin_id: PluginId,
        resp: Option<Vec<CodeLens>>,
    },
    GetCodeLensResolveResponse {
        plugin_id: PluginId,
        resp: CodeLens,
    },
    GotoImplementationResponse {
        plugin_id: PluginId,
        resp: Option<GotoImplementationResponse>,
    },
    GetDocumentFormatting {
        edits: Vec<TextEdit>,
    },
    GetDocumentSymbols {
        resp: DocumentSymbolResponse,
    },
    GetWorkspaceSymbols {
        symbols: Vec<SymbolInformation>,
    },
    GetSelectionRange {
        ranges: Vec<SelectionRange>,
    },
    GetInlayHints {
        hints: Vec<InlayHint>,
    },
    GetInlineCompletions {
        completions: InlineCompletionResponse,
    },
    GetSemanticTokens {
        styles: SemanticStyles,
    },
    PrepareRename {
        resp: PrepareRenameResponse,
    },
    Rename {
        edit: WorkspaceEdit,
    },
    GetOpenFilesContentResponse {
        items: Vec<TextDocumentItem>,
    },
    GlobalSearchResponse {
        matches: IndexMap<PathBuf, Vec<SearchMatch>>,
    },
    WorkspaceFilesResponse {
        generation: u64,
        files: Vec<PathBuf>,
    },
    DapVariableResponse {
        varialbes: Vec<dap_types::Variable>,
    },
    DapGetScopesResponse {
        scopes: Vec<(dap_types::Scope, Vec<dap_types::Variable>)>,
    },
    CreatePathResponse {
        path: PathBuf,
    },
    Success {},
    SaveResponse {},
    ReferencesResolveResponse {
        items: Vec<FileLine>,
    },
    AheadResponse {
        response: serde_json::Value,
    },
}

pub type ProxyMessage = RpcMessage<ProxyRequest, ProxyNotification, ProxyResponse>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadDirResponse {
    pub items: HashMap<PathBuf, FileNodeItem>,
}

pub trait ProxyCallback: Send + FnOnce(Result<ProxyResponse, RpcError>) {}

impl<F: Send + FnOnce(Result<ProxyResponse, RpcError>)> ProxyCallback for F {}

enum ResponseHandler {
    Callback(Box<dyn ProxyCallback>),
    Chan(Sender<Result<ProxyResponse, RpcError>>),
}

pub struct WorkspaceFilesRequest {
    request_id: u64,
    tx: Sender<ProxyRpc>,
    receiver: Receiver<Result<ProxyResponse, RpcError>>,
}

impl WorkspaceFilesRequest {
    pub fn wait(self) -> Result<(u64, Vec<PathBuf>), RpcError> {
        let result = match self.receiver.recv_timeout(Duration::from_secs(30)) {
            Ok(result) => result?,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                self.cancel();
                return Err(RpcError {
                    code: 0,
                    message: "proxy request timed out after 30s".to_string(),
                });
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return Err(RpcError {
                    code: 0,
                    message: "proxy connection closed".to_string(),
                });
            }
        };
        match result {
            ProxyResponse::WorkspaceFilesResponse { generation, files } => {
                Ok((generation, files))
            }
            _ => Err(RpcError {
                code: 0,
                message: "proxy returned the wrong workspace file response"
                    .to_string(),
            }),
        }
    }

    fn cancel(&self) {
        if let Err(error) = self.tx.send(ProxyRpc::Notification(
            ProxyNotification::CancelWorkspaceFiles {
                request_id: self.request_id,
            },
        )) {
            tracing::error!("{:?}", error);
        }
    }
}

impl ResponseHandler {
    fn invoke(self, result: Result<ProxyResponse, RpcError>) {
        match self {
            ResponseHandler::Callback(f) => f(result),
            ResponseHandler::Chan(tx) => {
                if let Err(err) = tx.send(result) {
                    tracing::error!("{:?}", err);
                }
            }
        }
    }
}

pub trait ProxyHandler {
    fn handle_notification(&mut self, rpc: ProxyNotification);
    fn handle_request(&mut self, id: RequestId, rpc: ProxyRequest);
}

#[derive(Clone)]
pub struct ProxyRpcHandler {
    tx: Sender<ProxyRpc>,
    rx: Receiver<ProxyRpc>,
    id: Arc<AtomicU64>,
    pending: Arc<Mutex<Option<HashMap<u64, ResponseHandler>>>>,
}

impl ProxyRpcHandler {
    pub fn new() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        Self {
            tx,
            rx,
            id: Arc::new(AtomicU64::new(0)),
            pending: Arc::new(Mutex::new(Some(HashMap::new()))),
        }
    }

    pub fn rx(&self) -> &Receiver<ProxyRpc> {
        &self.rx
    }

    pub fn mainloop<H>(&self, handler: &mut H)
    where
        H: ProxyHandler,
    {
        use ProxyRpc::*;
        for msg in &self.rx {
            match msg {
                Request(id, request) => {
                    handler.handle_request(id, request);
                }
                Notification(notification) => {
                    handler.handle_notification(notification);
                }
                Shutdown => {
                    return;
                }
            }
        }
    }

    fn request_common(&self, request: ProxyRequest, rh: ResponseHandler) {
        let id = self.id.fetch_add(1, Ordering::Relaxed);

        let mut pending = self.pending.lock();
        let Some(handlers) = pending.as_mut() else {
            drop(pending);
            rh.invoke(Err(Self::connection_closed_error()));
            return;
        };
        handlers.insert(id, rh);
        let sent = self.tx.send(ProxyRpc::Request(id, request));
        drop(pending);
        if let Err(error) = sent {
            tracing::error!("{error:?}");
            self.disconnect();
        }
    }

    fn request(&self, request: ProxyRequest) -> Result<ProxyResponse, RpcError> {
        self.request_with_timeout(request, Duration::from_secs(30))
    }

    fn request_with_timeout(
        &self,
        request: ProxyRequest,
        timeout: Duration,
    ) -> Result<ProxyResponse, RpcError> {
        // These writes are not cancelled by a local deadline. Keep their pending
        // slot until the real reply or disconnect, rather than invite a duplicate.
        let wait_for_outcome = matches!(
            &request,
            ProxyRequest::AheadRequest {
                request: crate::ahead::AheadRequest::StartWork { .. }
                    | crate::ahead::AheadRequest::SetExternalAcpAdapterInstalled { .. }
            }
        );
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.request_common(request, ResponseHandler::Chan(tx));
        if wait_for_outcome {
            return rx
                .recv()
                .unwrap_or_else(|_| Err(Self::connection_closed_error()));
        }
        rx.recv_timeout(timeout).unwrap_or_else(|error| {
            Err(match error {
                crossbeam_channel::RecvTimeoutError::Timeout => RpcError {
                    code: 0,
                    message: format!(
                        "proxy request timed out after {}s",
                        timeout.as_secs()
                    ),
                },
                crossbeam_channel::RecvTimeoutError::Disconnected => {
                    Self::connection_closed_error()
                }
            })
        })
    }

    fn connection_closed_error() -> RpcError {
        RpcError {
            code: 0,
            message: "proxy connection closed. Restart AHEAD to reconnect and restore stored sessions; an in-flight change may already have completed.".into(),
        }
    }

    pub fn disconnect(&self) {
        let pending = self.pending.lock().take();
        if let Some(pending) = pending {
            if let Err(error) = self.tx.send(ProxyRpc::Shutdown) {
                tracing::error!("{error:?}");
            }
            // Callbacks can submit another request, so invoke them outside the lock.
            for handler in pending.into_values() {
                handler.invoke(Err(Self::connection_closed_error()));
            }
        }
    }

    pub fn workspace_files(&self, request_id: u64) -> WorkspaceFilesRequest {
        let (sender, receiver) = crossbeam_channel::bounded(1);
        self.request_common(
            ProxyRequest::WorkspaceFiles { request_id },
            ResponseHandler::Chan(sender),
        );
        WorkspaceFilesRequest {
            request_id,
            tx: self.tx.clone(),
            receiver,
        }
    }

    pub fn cancel_workspace_files(&self, request_id: u64) {
        self.notification(ProxyNotification::CancelWorkspaceFiles { request_id });
    }

    pub fn request_async(
        &self,
        request: ProxyRequest,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_common(request, ResponseHandler::Callback(Box::new(f)));
    }

    pub fn handle_response(
        &self,
        id: RequestId,
        result: Result<ProxyResponse, RpcError>,
    ) {
        let handler = {
            self.pending
                .lock()
                .as_mut()
                .and_then(|pending| pending.remove(&id))
        };
        if let Some(handler) = handler {
            handler.invoke(result);
        }
    }

    pub fn notification(&self, notification: ProxyNotification) {
        let pending = self.pending.lock();
        if pending.is_none() {
            return;
        }
        let sent = self.tx.send(ProxyRpc::Notification(notification));
        drop(pending);
        if let Err(error) = sent {
            tracing::error!("{error:?}");
            self.disconnect();
        }
    }

    pub fn lsp_cancel(&self, id: i32) {
        self.notification(ProxyNotification::LspCancel { id });
    }

    pub fn git_init(&self) {
        self.notification(ProxyNotification::GitInit {});
    }

    pub fn git_commit(&self, message: String, diffs: Vec<FileDiff>) {
        self.notification(ProxyNotification::GitCommit { message, diffs });
    }

    pub fn git_checkout(&self, reference: String) {
        self.notification(ProxyNotification::GitCheckout { reference });
    }

    pub fn shutdown(&self) {
        self.notification(ProxyNotification::Shutdown {});
        self.disconnect();
    }

    pub fn initialize(
        &self,
        workspace: Option<PathBuf>,
        window_id: usize,
        tab_id: usize,
    ) {
        self.notification(ProxyNotification::Initialize {
            workspace,
            window_id,
            tab_id,
        });
    }

    pub fn completion(
        &self,
        request_id: usize,
        path: PathBuf,
        input: String,
        position: Position,
    ) {
        self.notification(ProxyNotification::Completion {
            request_id,
            path,
            input,
            position,
        });
    }

    pub fn signature_help(
        &self,
        request_id: usize,
        path: PathBuf,
        position: Position,
    ) {
        self.notification(ProxyNotification::SignatureHelp {
            request_id,
            path,
            position,
        });
    }

    pub fn new_terminal(&self, term_id: TermId, profile: TerminalProfile) {
        self.notification(ProxyNotification::NewTerminal { term_id, profile });
    }

    pub fn terminal_close(&self, term_id: TermId) {
        self.notification(ProxyNotification::TerminalClose { term_id });
    }

    pub fn terminal_resize(&self, term_id: TermId, width: usize, height: usize) {
        self.notification(ProxyNotification::TerminalResize {
            term_id,
            width,
            height,
        });
    }

    pub fn terminal_write(&self, term_id: TermId, content: String) {
        self.notification(ProxyNotification::TerminalWrite { term_id, content });
    }

    pub fn new_buffer(
        &self,
        buffer_id: BufferId,
        path: PathBuf,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(ProxyRequest::NewBuffer { buffer_id, path }, f);
    }

    pub fn install_language_extension(
        &self,
        url: String,
        extension_id: String,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(
            ProxyRequest::InstallLanguageExtension { url, extension_id },
            f,
        );
    }

    pub fn get_buffer_head(&self, path: PathBuf, f: impl ProxyCallback + 'static) {
        self.request_async(ProxyRequest::BufferHead { path }, f);
    }

    pub fn create_file(&self, path: PathBuf, f: impl ProxyCallback + 'static) {
        self.request_async(ProxyRequest::CreateFile { path }, f);
    }

    pub fn create_directory(&self, path: PathBuf, f: impl ProxyCallback + 'static) {
        self.request_async(ProxyRequest::CreateDirectory { path }, f);
    }

    pub fn trash_path(&self, path: PathBuf, f: impl ProxyCallback + 'static) {
        self.request_async(ProxyRequest::TrashPath { path }, f);
    }

    pub fn duplicate_path(
        &self,
        existing_path: PathBuf,
        new_path: PathBuf,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(
            ProxyRequest::DuplicatePath {
                existing_path,
                new_path,
            },
            f,
        );
    }

    pub fn rename_path(
        &self,
        from: PathBuf,
        to: PathBuf,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(ProxyRequest::RenamePath { from, to }, f);
    }

    pub fn test_create_at_path(
        &self,
        path: PathBuf,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(ProxyRequest::TestCreateAtPath { path }, f);
    }

    pub fn save_buffer_as(
        &self,
        buffer_id: BufferId,
        path: PathBuf,
        rev: u64,
        content: String,
        create_parents: bool,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(
            ProxyRequest::SaveBufferAs {
                buffer_id,
                path,
                rev,
                content,
                create_parents,
            },
            f,
        );
    }

    pub fn global_search(
        &self,
        pattern: String,
        case_sensitive: bool,
        whole_word: bool,
        is_regex: bool,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(
            ProxyRequest::GlobalSearch {
                pattern,
                case_sensitive,
                whole_word,
                is_regex,
            },
            f,
        );
    }

    pub fn save(
        &self,
        rev: u64,
        path: PathBuf,
        create_parents: bool,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(
            ProxyRequest::Save {
                rev,
                path,
                create_parents,
            },
            f,
        );
    }

    pub fn get_open_files_content(&self) -> Result<ProxyResponse, RpcError> {
        self.request(ProxyRequest::GetOpenFilesContent {})
    }

    pub fn read_dir(&self, path: PathBuf, f: impl ProxyCallback + 'static) {
        self.request_async(ProxyRequest::ReadDir { path }, f);
    }

    pub fn completion_resolve(
        &self,
        plugin_id: PluginId,
        completion_item: CompletionItem,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(
            ProxyRequest::CompletionResolve {
                plugin_id,
                completion_item: Box::new(completion_item),
            },
            f,
        );
    }

    pub fn code_action_resolve(
        &self,
        action_item: CodeAction,
        plugin_id: PluginId,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(
            ProxyRequest::CodeActionResolve {
                action_item: Box::new(action_item),
                plugin_id,
            },
            f,
        );
    }

    pub fn get_hover(
        &self,
        request_id: usize,
        path: PathBuf,
        position: Position,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(
            ProxyRequest::GetHover {
                request_id,
                path,
                position,
            },
            f,
        );
    }

    pub fn get_definition(
        &self,
        request_id: usize,
        path: PathBuf,
        position: Position,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(
            ProxyRequest::GetDefinition {
                request_id,
                path,
                position,
            },
            f,
        );
    }

    pub fn show_call_hierarchy(
        &self,
        path: PathBuf,
        position: Position,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(ProxyRequest::ShowCallHierarchy { path, position }, f);
    }

    pub fn call_hierarchy_incoming(
        &self,
        path: PathBuf,
        call_hierarchy_item: CallHierarchyItem,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(
            ProxyRequest::CallHierarchyIncoming {
                path,
                call_hierarchy_item,
            },
            f,
        );
    }

    pub fn get_type_definition(
        &self,
        request_id: usize,
        path: PathBuf,
        position: Position,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(
            ProxyRequest::GetTypeDefinition {
                request_id,
                path,
                position,
            },
            f,
        );
    }

    pub fn get_lsp_folding_range(
        &self,
        path: PathBuf,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(ProxyRequest::LspFoldingRange { path }, f);
    }

    pub fn get_references(
        &self,
        path: PathBuf,
        position: Position,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(ProxyRequest::GetReferences { path, position }, f);
    }

    pub fn references_resolve(
        &self,
        items: Vec<Location>,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(ProxyRequest::ReferencesResolve { items }, f);
    }

    pub fn go_to_implementation(
        &self,
        path: PathBuf,
        position: Position,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(ProxyRequest::GotoImplementation { path, position }, f);
    }

    pub fn get_code_actions(
        &self,
        path: PathBuf,
        position: Position,
        diagnostics: Vec<Diagnostic>,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(
            ProxyRequest::GetCodeActions {
                path,
                position,
                diagnostics,
            },
            f,
        );
    }

    pub fn get_code_lens(&self, path: PathBuf, f: impl ProxyCallback + 'static) {
        self.request_async(ProxyRequest::GetCodeLens { path }, f);
    }

    pub fn get_code_lens_resolve(
        &self,
        code_lens: CodeLens,
        path: PathBuf,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(ProxyRequest::GetCodeLensResolve { code_lens, path }, f);
    }

    pub fn get_document_formatting(
        &self,
        path: PathBuf,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(ProxyRequest::GetDocumentFormatting { path }, f);
    }

    pub fn get_semantic_tokens(
        &self,
        path: PathBuf,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(ProxyRequest::GetSemanticTokens { path }, f);
    }

    pub fn get_document_symbols(
        &self,
        path: PathBuf,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(ProxyRequest::GetDocumentSymbols { path }, f);
    }

    pub fn get_workspace_symbols(
        &self,
        query: String,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(ProxyRequest::GetWorkspaceSymbols { query }, f);
    }

    pub fn prepare_rename(
        &self,
        path: PathBuf,
        position: Position,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(ProxyRequest::PrepareRename { path, position }, f);
    }

    pub fn git_get_remote_file_url(
        &self,
        file: PathBuf,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(ProxyRequest::GitGetRemoteFileUrl { file }, f);
    }

    pub fn rename(
        &self,
        path: PathBuf,
        position: Position,
        new_name: String,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(
            ProxyRequest::Rename {
                path,
                position,
                new_name,
            },
            f,
        );
    }

    pub fn get_inlay_hints(&self, path: PathBuf, f: impl ProxyCallback + 'static) {
        self.request_async(ProxyRequest::GetInlayHints { path }, f);
    }

    pub fn get_inline_completions(
        &self,
        path: PathBuf,
        position: Position,
        trigger_kind: InlineCompletionTriggerKind,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(
            ProxyRequest::GetInlineCompletions {
                path,
                position,
                trigger_kind,
            },
            f,
        );
    }

    pub fn update(&self, path: PathBuf, delta: AheadDelta, rev: u64) {
        self.notification(ProxyNotification::Update { path, delta, rev });
    }

    pub fn editor_snapshot(&self, path: PathBuf, content: String) {
        self.notification(ProxyNotification::EditorSnapshot { path, content });
    }

    pub fn close_editor_buffer(&self, path: PathBuf) {
        self.notification(ProxyNotification::CloseEditorBuffer { path });
    }

    pub fn git_discard_files_changes(&self, files: Vec<PathBuf>) {
        self.notification(ProxyNotification::GitDiscardFilesChanges { files });
    }

    pub fn git_discard_workspace_changes(&self) {
        self.notification(ProxyNotification::GitDiscardWorkspaceChanges {});
    }

    pub fn get_selection_range(
        &self,
        path: PathBuf,
        positions: Vec<Position>,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(ProxyRequest::GetSelectionRange { path, positions }, f);
    }

    pub fn dap_start(
        &self,
        config: RunDebugConfig,
        breakpoints: HashMap<PathBuf, Vec<SourceBreakpoint>>,
    ) {
        self.notification(ProxyNotification::DapStart {
            config,
            breakpoints,
        });
    }

    pub fn dap_terminal_response(
        &self,
        response: crate::dap_types::DebugTerminalResponse,
    ) {
        self.notification(ProxyNotification::DapTerminalResponse { response });
    }

    pub fn dap_restart(
        &self,
        dap_id: DapId,
        breakpoints: HashMap<PathBuf, Vec<SourceBreakpoint>>,
    ) {
        self.notification(ProxyNotification::DapRestart {
            dap_id,
            breakpoints,
        });
    }

    pub fn dap_continue(&self, dap_id: DapId, thread_id: ThreadId) {
        self.notification(ProxyNotification::DapContinue { dap_id, thread_id });
    }

    pub fn dap_step_over(&self, dap_id: DapId, thread_id: ThreadId) {
        self.notification(ProxyNotification::DapStepOver { dap_id, thread_id });
    }

    pub fn dap_step_into(&self, dap_id: DapId, thread_id: ThreadId) {
        self.notification(ProxyNotification::DapStepInto { dap_id, thread_id });
    }

    pub fn dap_step_out(&self, dap_id: DapId, thread_id: ThreadId) {
        self.notification(ProxyNotification::DapStepOut { dap_id, thread_id });
    }

    pub fn dap_pause(&self, dap_id: DapId, thread_id: ThreadId) {
        self.notification(ProxyNotification::DapPause { dap_id, thread_id });
    }

    pub fn dap_stop(&self, dap_id: DapId) {
        self.notification(ProxyNotification::DapStop { dap_id });
    }

    pub fn dap_disconnect(&self, dap_id: DapId) {
        self.notification(ProxyNotification::DapDisconnect { dap_id });
    }

    pub fn dap_set_breakpoints(
        &self,
        dap_id: DapId,
        path: PathBuf,
        breakpoints: Vec<SourceBreakpoint>,
    ) {
        self.notification(ProxyNotification::DapSetBreakpoints {
            dap_id,
            path,
            breakpoints,
        });
    }

    pub fn dap_variable(
        &self,
        dap_id: DapId,
        reference: usize,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(ProxyRequest::DapVariable { dap_id, reference }, f);
    }

    pub fn dap_get_scopes(
        &self,
        dap_id: DapId,
        frame_id: usize,
        f: impl ProxyCallback + 'static,
    ) {
        self.request_async(ProxyRequest::DapGetScopes { dap_id, frame_id }, f);
    }

    pub fn ahead_request(
        &self,
        request: crate::ahead::AheadRequest,
        f: impl FnOnce(Result<serde_json::Value, RpcError>) + Send + 'static,
    ) {
        self.request_async(ProxyRequest::AheadRequest { request }, move |res| {
            match res {
                Ok(ProxyResponse::AheadResponse { response }) => f(Ok(response)),
                Ok(_) => f(Err(RpcError {
                    code: 0,
                    message: "Unexpected response variant for AheadRequest".into(),
                })),
                Err(err) => f(Err(err)),
            }
        });
    }

    /// Blocking variant for call sites that can afford to wait (conversation
    /// history refresh, turn start/cancel from a background task).
    pub fn ahead_request_blocking(
        &self,
        request: crate::ahead::AheadRequest,
    ) -> Result<serde_json::Value, RpcError> {
        match self.request(ProxyRequest::AheadRequest { request })? {
            ProxyResponse::AheadResponse { response } => Ok(response),
            _ => Err(RpcError {
                code: 0,
                message: "Unexpected response variant for AheadRequest".into(),
            }),
        }
    }

    pub fn ahead_notification(&self, notification: crate::ahead::AheadNotification) {
        self.notification(ProxyNotification::AheadNotification { notification });
    }
}

impl Default for ProxyRpcHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{ProxyRequest, ProxyResponse, ProxyRpc, ProxyRpcHandler};
    use crate::ahead::{AheadRequest, HarnessKind};
    use std::time::Duration;

    fn creation_request() -> AheadRequest {
        AheadRequest::StartWork {
            work_kind: None,
            title: "Delayed session".into(),
            starting_point: "Keep one session".into(),
            work_item: None,
            harness: Some(HarnessKind::Ahead),
            external_agent_id: None,
            parent_session_id: None,
        }
    }

    #[test]
    fn setup_requests_wait_past_read_deadline_for_their_actual_reply() {
        for request in [
            creation_request(),
            AheadRequest::SetExternalAcpAdapterInstalled {
                adapter_id: "pi-acp".into(),
                installed: true,
            },
        ] {
            let rpc = ProxyRpcHandler::new();
            let worker_rpc = rpc.clone();
            let (sender, receiver) = crossbeam_channel::bounded(1);
            let worker = std::thread::spawn(move || {
                sender
                    .send(worker_rpc.request_with_timeout(
                        ProxyRequest::AheadRequest { request },
                        Duration::ZERO,
                    ))
                    .expect("report result");
            });
            let ProxyRpc::Request(id, _) = rpc
                .rx()
                .recv_timeout(Duration::from_secs(1))
                .expect("request")
            else {
                panic!("expected one request");
            };
            assert!(matches!(
                receiver.recv_timeout(Duration::from_millis(20)),
                Err(crossbeam_channel::RecvTimeoutError::Timeout)
            ));
            rpc.handle_response(
                id,
                Ok(ProxyResponse::AheadResponse {
                    response: serde_json::json!({"session": {"id": "created-once"}}),
                }),
            );
            let result = receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("reply")
                .expect("success");
            assert!(matches!(result, ProxyResponse::AheadResponse { .. }));
            assert!(rpc.rx().is_empty());
            worker.join().expect("worker");
        }
    }

    #[test]
    fn read_requests_keep_their_deadline() {
        let rpc = ProxyRpcHandler::new();
        let result = rpc.request_with_timeout(
            ProxyRequest::AheadRequest {
                request: AheadRequest::ListSessions,
            },
            Duration::ZERO,
        );
        assert!(result.err().expect("timeout").message.contains("timed out"));
    }

    #[test]
    fn disconnect_releases_waiting_setup_request() {
        let rpc = ProxyRpcHandler::new();
        let worker_rpc = rpc.clone();
        let (sender, receiver) = crossbeam_channel::bounded(1);
        let worker = std::thread::spawn(move || {
            sender
                .send(worker_rpc.ahead_request_blocking(creation_request()))
                .expect("report result");
        });
        assert!(matches!(
            rpc.rx()
                .recv_timeout(Duration::from_secs(1))
                .expect("request"),
            ProxyRpc::Request(_, _)
        ));
        rpc.disconnect();
        let error = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("disconnect")
            .expect_err("failed request");
        assert!(error.message.contains("Restart AHEAD"));
        worker.join().expect("worker");
    }

    #[test]
    fn disconnect_drains_once_and_rejects_reentrant_and_future_requests() {
        let rpc = ProxyRpcHandler::new();
        let callback_rpc = rpc.clone();
        let (sender, receiver) = crossbeam_channel::unbounded();
        let nested_sender = sender.clone();
        rpc.ahead_request(creation_request(), move |result| {
            sender.send(result).expect("first result");
            callback_rpc.ahead_request(AheadRequest::ListSessions, move |result| {
                nested_sender.send(result).expect("nested result");
            });
        });
        let ProxyRpc::Request(id, _) = rpc.rx().try_recv().expect("request") else {
            panic!("expected request");
        };
        rpc.disconnect();
        rpc.disconnect();
        rpc.handle_response(id, Ok(ProxyResponse::Success {}));
        for _ in 0..2 {
            let error = receiver
                .try_recv()
                .expect("one result")
                .expect_err("closed");
            assert!(error.message.contains("connection closed"));
        }
        assert!(receiver.try_recv().is_err());
        assert!(matches!(rpc.rx().try_recv(), Ok(ProxyRpc::Shutdown)));
        assert!(rpc.rx().is_empty());
        let error = rpc
            .ahead_request_blocking(creation_request())
            .expect_err("no new writes");
        assert!(error.message.contains("Restart AHEAD"));
        rpc.notification(super::ProxyNotification::Shutdown {});
        assert!(rpc.rx().is_empty());
    }
}
