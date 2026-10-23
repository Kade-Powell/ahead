use std::{
    borrow::Cow,
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use ahead_core::encoding::{offset_utf8_to_utf16, offset_utf16_to_utf8};
use ahead_rpc::{
    RpcError,
    core::{CoreRpcHandler, ServerStatusParams},
    delta::AheadDelta,
    plugin::{PluginId, ServerId},
    style::{LineStyle, Style},
};
use anyhow::{Result, anyhow};
use crossbeam_channel::{Receiver, Sender};
use dyn_clone::DynClone;
use jsonrpc_lite::{Id, JsonRpc, Params};
use lsp_types::{
    CancelParams, CodeActionProviderCapability, DidChangeConfigurationParams,
    DidChangeTextDocumentParams, DidSaveTextDocumentParams, DocumentSelector,
    FoldingRangeProviderCapability, HoverProviderCapability,
    ImplementationProviderCapability, InitializeResult, LogMessageParams,
    MessageType, OneOf, Position, ProgressParams, PublishDiagnosticsParams, Range,
    Registration, RegistrationParams, SemanticTokens, SemanticTokensLegend,
    SemanticTokensServerCapabilities, ServerCapabilities, ShowMessageParams,
    TextDocumentContentChangeEvent, TextDocumentIdentifier,
    TextDocumentSaveRegistrationOptions, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncSaveOptions, Unregistration,
    UnregistrationParams, VersionedTextDocumentIdentifier,
    notification::{
        Cancel, DidChangeConfiguration, DidChangeTextDocument,
        DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument,
        DidSaveTextDocument, Initialized, LogMessage, Notification, Progress,
        PublishDiagnostics, ShowMessage,
    },
    request::{
        CallHierarchyIncomingCalls, CallHierarchyPrepare, CodeActionRequest,
        CodeActionResolveRequest, CodeLensRequest, CodeLensResolve, Completion,
        DocumentDiagnosticRequest, DocumentSymbolRequest, ExecuteCommand,
        FoldingRangeRequest, Formatting, GotoDefinition, GotoImplementation,
        GotoTypeDefinition, HoverRequest, Initialize, InlayHintRequest,
        InlineCompletionRequest, PrepareRenameRequest, References,
        RegisterCapability, Rename, Request, ResolveCompletionItem,
        SelectionRangeRequest, SemanticTokensFullRequest, SignatureHelpRequest,
        UnregisterCapability, WorkDoneProgressCreate, WorkspaceSymbolRequest,
    },
};
use parking_lot::{Condvar, Mutex};
use ropey::{LineType, Rope};
use serde::Serialize;
use serde_json::Value;

use super::{
    PluginCatalogRpcHandler, lsp::DocumentFilter,
    watched_files::WatchedFileRegistration,
};

pub(super) const SERVER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const SERVER_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
// Leave time for aggregation and IPC before the editor's five-second wait ends.
const CODE_ACTION_REQUEST_TIMEOUT: Duration = Duration::from_secs(4);

pub enum ResponseHandler<Resp, Error> {
    Chan(Sender<Result<Resp, Error>>),
    Callback(Box<dyn RpcCallback<Resp, Error>>),
}

impl<Resp, Error> ResponseHandler<Resp, Error> {
    pub fn invoke(self, result: Result<Resp, Error>) {
        match self {
            ResponseHandler::Chan(tx) => {
                if let Err(err) = tx.send(result) {
                    tracing::error!("{:?}", err);
                }
            }
            ResponseHandler::Callback(f) => f.call(result),
        }
    }
}

pub trait ClonableCallback<Resp, Error>:
    FnOnce(PluginId, Result<Resp, Error>) + Send + DynClone
{
}

impl<Resp, Error, F: Send + FnOnce(PluginId, Result<Resp, Error>) + DynClone>
    ClonableCallback<Resp, Error> for F
{
}

pub trait RpcCallback<Resp, Error>: Send {
    fn call(self: Box<Self>, result: Result<Resp, Error>);
}

impl<Resp, Error, F: Send + FnOnce(Result<Resp, Error>)> RpcCallback<Resp, Error>
    for F
{
    fn call(self: Box<F>, result: Result<Resp, Error>) {
        (*self)(result)
    }
}

#[allow(clippy::large_enum_variant)]
pub enum PluginHandlerNotification {
    Initialize,
    InitializeResult(InitializeResult),
    WorkspaceConfiguration(Option<Value>),
    Shutdown,
}

#[allow(clippy::large_enum_variant)]
pub enum PluginServerRpc {
    Shutdown,
    Handler(PluginHandlerNotification),
    FirstCapableRequest {
        remaining: VecDeque<PluginServerRpcHandler>,
        method: Cow<'static, str>,
        params: Value,
        language_id: Option<String>,
        path: Option<PathBuf>,
        callback: Box<dyn ClonableCallback<Value, RpcError>>,
    },
    ServerRequest {
        id: Id,
        method: Cow<'static, str>,
        params: Params,
        language_id: Option<String>,
        path: Option<PathBuf>,
        rh: ResponseHandler<Value, RpcError>,
    },
    ServerNotification {
        method: Cow<'static, str>,
        params: Params,
        language_id: Option<String>,
        path: Option<PathBuf>,
    },
    HostRequest {
        id: Id,
        method: String,
        params: Params,
        resp: ResponseSender,
    },
    HostNotification {
        method: String,
        params: Params,
        from: String,
    },
    DidSaveTextDocument {
        language_id: String,
        path: PathBuf,
        text_document: TextDocumentIdentifier,
        text: Rope,
    },
    DidChangeTextDocument {
        language_id: String,
        document: VersionedTextDocumentIdentifier,
        delta: AheadDelta,
        text: Rope,
        new_text: Rope,
        change: Arc<
            Mutex<(
                Option<TextDocumentContentChangeEvent>,
                Option<TextDocumentContentChangeEvent>,
            )>,
        >,
    },
    FormatSemanticTokens {
        tokens: SemanticTokens,
        text: Rope,
        f: Box<dyn RpcCallback<Vec<LineStyle>, RpcError>>,
    },
}

#[derive(Clone)]
pub struct PluginServerRpcHandler {
    pub plugin_id: PluginId,
    pub server_id: ServerId,
    rpc_tx: Sender<PluginServerRpc>,
    rpc_rx: Receiver<PluginServerRpc>,
    io_tx: Sender<Option<JsonRpc>>,
    id: Arc<AtomicU64>,
    server_requests: Arc<Mutex<ServerRequests>>,
    terminated: Arc<(Mutex<bool>, Condvar)>,
}

#[derive(Default)]
struct ServerRequests {
    stopped: bool,
    pending: HashMap<Id, PendingRequest>,
}

struct PendingRequest {
    deadline: Instant,
    response: ResponseHandler<Value, RpcError>,
}

#[derive(Clone)]
pub struct ResponseSender {
    tx: Sender<Result<Value, RpcError>>,
}
impl ResponseSender {
    pub fn new(tx: Sender<Result<Value, RpcError>>) -> Self {
        Self { tx }
    }

    pub fn send(&self, result: impl Serialize) {
        let result = serde_json::to_value(result).map_err(|e| RpcError {
            code: 0,
            message: e.to_string(),
        });
        if let Err(err) = self.tx.send(result) {
            tracing::error!("{:?}", err);
        }
    }

    pub fn send_null(&self) {
        if let Err(err) = self.tx.send(Ok(Value::Null)) {
            tracing::error!("{:?}", err);
        }
    }

    pub fn send_err(&self, code: i64, message: impl Into<String>) {
        if let Err(err) = self.tx.send(Err(RpcError {
            code,
            message: message.into(),
        })) {
            tracing::error!("{:?}", err);
        }
    }
}

pub trait PluginServerHandler {
    fn document_supported(
        &mut self,
        language_id: Option<&str>,
        path: Option<&Path>,
    ) -> bool;
    fn method_registered(&mut self, method: &str) -> bool;
    fn request_supported(&mut self, method: &str, _params: &Params) -> bool {
        self.method_registered(method)
    }
    fn handle_host_notification(
        &mut self,
        method: String,
        params: Params,
        from: String,
    );
    fn handle_host_request(
        &mut self,
        id: Id,
        method: String,
        params: Params,
        chan: ResponseSender,
    );
    fn handle_handler_notification(
        &mut self,
        notification: PluginHandlerNotification,
    );
    fn handle_did_save_text_document(
        &self,
        language_id: String,
        path: PathBuf,
        text_document: TextDocumentIdentifier,
        text: Rope,
    );
    fn handle_did_change_text_document(
        &mut self,
        language_id: String,
        document: VersionedTextDocumentIdentifier,
        delta: AheadDelta,
        text: Rope,
        new_text: Rope,
        change: Arc<
            Mutex<(
                Option<TextDocumentContentChangeEvent>,
                Option<TextDocumentContentChangeEvent>,
            )>,
        >,
    );
    fn format_semantic_tokens(
        &self,
        tokens: SemanticTokens,
        text: Rope,
        f: Box<dyn RpcCallback<Vec<LineStyle>, RpcError>>,
    );
}

impl PluginServerRpcHandler {
    pub fn new(
        server_id: ServerId,
        plugin_id: Option<PluginId>,
        io_tx: Sender<Option<JsonRpc>>,
    ) -> Self {
        let (rpc_tx, rpc_rx) = crossbeam_channel::unbounded();

        let rpc = Self {
            server_id,
            plugin_id: plugin_id.unwrap_or_else(PluginId::next),
            rpc_tx,
            rpc_rx,
            io_tx,
            id: Arc::new(AtomicU64::new(0)),
            server_requests: Arc::new(Mutex::new(ServerRequests::default())),
            terminated: Arc::new((Mutex::new(false), Condvar::new())),
        };

        rpc.initialize();
        rpc
    }

    #[cfg(test)]
    pub(crate) fn test_receiver(&self) -> Receiver<PluginServerRpc> {
        self.rpc_rx.clone()
    }

    fn initialize(&self) {
        self.handle_rpc(PluginServerRpc::Handler(
            PluginHandlerNotification::Initialize,
        ));
    }

    fn send_server_request(
        &self,
        id: Id,
        method: &str,
        params: Params,
        rh: ResponseHandler<Value, RpcError>,
    ) {
        let mut state = self.server_requests.lock();
        if state.stopped {
            drop(state);
            rh.invoke(Err(Self::stopped_error()));
            return;
        }
        state.pending.insert(
            id.clone(),
            PendingRequest {
                deadline: Instant::now()
                    + if method == CodeActionRequest::METHOD {
                        CODE_ACTION_REQUEST_TIMEOUT
                    } else {
                        SERVER_REQUEST_TIMEOUT
                    },
                response: rh,
            },
        );
        let msg = JsonRpc::request_with_params(id, method, params);
        let failed = self.io_tx.send(Some(msg)).is_err();
        drop(state);
        if failed {
            self.shutdown();
        }
    }

    fn send_server_notification(&self, method: &str, params: Params) {
        let msg = JsonRpc::notification_with_params(method, params);
        self.send_server_rpc(msg);
    }

    fn send_server_rpc(&self, msg: JsonRpc) {
        if let Err(err) = self.io_tx.send(Some(msg)) {
            tracing::error!("{:?}", err);
        }
    }

    pub fn handle_rpc(&self, rpc: PluginServerRpc) {
        let state = self.server_requests.lock();
        if state.stopped
            && !matches!(
                rpc,
                PluginServerRpc::Shutdown
                    | PluginServerRpc::Handler(PluginHandlerNotification::Shutdown)
            )
        {
            drop(state);
            Self::reject_stopped(rpc);
            return;
        }
        let result = self.rpc_tx.send(rpc);
        drop(state);
        if let Err(err) = result {
            Self::reject_stopped(err.0);
        }
    }

    fn stopped_error() -> RpcError {
        RpcError {
            code: -32097,
            message:
                "Language server stopped. Restart language servers and try again."
                    .into(),
        }
    }

    fn reject_stopped(rpc: PluginServerRpc) {
        match rpc {
            PluginServerRpc::FirstCapableRequest {
                remaining,
                method,
                params,
                language_id,
                path,
                callback,
            } => Self::request_first_capable(
                remaining,
                method,
                params,
                language_id,
                path,
                callback,
            ),
            PluginServerRpc::ServerRequest { rh, .. } => {
                rh.invoke(Err(Self::stopped_error()))
            }
            PluginServerRpc::FormatSemanticTokens { f, .. } => {
                f.call(Err(Self::stopped_error()))
            }
            PluginServerRpc::HostRequest { resp, .. } => {
                resp.send_err(-32097, Self::stopped_error().message)
            }
            _ => {}
        }
    }

    /// Send a notification.  
    /// The callback is called when the function is actually sent.
    pub fn server_notification<P: Serialize>(
        &self,
        method: impl Into<Cow<'static, str>>,
        params: P,
        language_id: Option<String>,
        path: Option<PathBuf>,
        check: bool,
    ) {
        let params = Params::from(serde_json::to_value(params).unwrap());
        let method = method.into();

        if check {
            if let Err(err) = self.rpc_tx.send(PluginServerRpc::ServerNotification {
                method,
                params,
                language_id,
                path,
            }) {
                tracing::error!("{:?}", err);
            }
        } else {
            self.send_server_notification(&method, params);
        }
    }

    pub fn update_workspace_configuration(&self, configuration: Option<Value>) {
        if let Err(error) = self.rpc_tx.send(PluginServerRpc::Handler(
            PluginHandlerNotification::WorkspaceConfiguration(configuration),
        )) {
            tracing::error!(?error, "updating language-server configuration");
        }
    }

    /// Make a request to plugin/language server and get the response.
    ///
    /// When check is true, the request will be in the handler mainloop to
    /// do checks like if the server has the capability of the request.
    ///
    /// When check is false, the request will be sent out straight away.
    pub fn server_request<P: Serialize>(
        &self,
        method: impl Into<Cow<'static, str>>,
        params: P,
        language_id: Option<String>,
        path: Option<PathBuf>,
        check: bool,
    ) -> Result<Value, RpcError> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.server_request_common(
            method.into(),
            params,
            language_id,
            path,
            check,
            ResponseHandler::Chan(tx),
        );
        rx.recv().unwrap_or_else(|_| {
            Err(RpcError {
                code: 0,
                message: "io error".to_string(),
            })
        })
    }

    pub fn server_request_async<P: Serialize>(
        &self,
        method: impl Into<Cow<'static, str>>,
        params: P,
        language_id: Option<String>,
        path: Option<PathBuf>,
        check: bool,
        f: impl RpcCallback<Value, RpcError> + 'static,
    ) {
        self.server_request_common(
            method.into(),
            params,
            language_id,
            path,
            check,
            ResponseHandler::Callback(Box::new(f)),
        );
    }

    pub(super) fn request_first_capable(
        mut remaining: VecDeque<Self>,
        method: Cow<'static, str>,
        params: Value,
        language_id: Option<String>,
        path: Option<PathBuf>,
        callback: Box<dyn ClonableCallback<Value, RpcError>>,
    ) {
        if let Some(server) = remaining.pop_front() {
            server.handle_rpc(PluginServerRpc::FirstCapableRequest {
                remaining,
                method,
                params,
                language_id,
                path,
                callback,
            });
        } else {
            callback(
                PluginId(0),
                Err(RpcError {
                    code: -32601,
                    message: format!(
                        "No selected language server supports {method} for this document"
                    ),
                }),
            );
        }
    }

    fn server_request_common<P: Serialize>(
        &self,
        method: Cow<'static, str>,
        params: P,
        language_id: Option<String>,
        path: Option<PathBuf>,
        check: bool,
        rh: ResponseHandler<Value, RpcError>,
    ) {
        let id = self.id.fetch_add(1, Ordering::Relaxed);
        let params = Params::from(serde_json::to_value(params).unwrap());
        if check {
            self.handle_rpc(PluginServerRpc::ServerRequest {
                id: Id::Num(id as i64),
                method,
                params,
                language_id,
                path,
                rh,
            });
        } else {
            self.send_server_request(Id::Num(id as i64), &method, params, rh);
        }
    }

    pub fn handle_server_response(&self, id: Id, result: Result<Value, RpcError>) {
        let handler = self.server_requests.lock().pending.remove(&id);
        if let Some(handler) = handler {
            handler.response.invoke(result);
        }
    }

    pub(super) fn expire_requests(&self, now: Instant) {
        let mut state = self.server_requests.lock();
        if state.stopped {
            return;
        }
        let expired_ids = state
            .pending
            .iter()
            .filter_map(|(id, request)| {
                (request.deadline <= now).then_some(id.clone())
            })
            .collect::<Vec<_>>();
        let expired = expired_ids
            .into_iter()
            .filter_map(|id| state.pending.remove(&id).map(|pending| (id, pending)))
            .collect::<Vec<_>>();
        drop(state);
        for (id, pending) in expired {
            self.send_server_notification(
                Cancel::METHOD,
                Params::from(serde_json::json!({ "id": id })),
            );
            pending.response.invoke(Err(RpcError { code: -32098, message: "Language server request timed out. Try again or restart language servers.".into() }));
        }
    }

    pub(super) fn shutdown_protocol(
        &self,
        deadline: Instant,
    ) -> Result<(), RpcError> {
        let id = Id::Num(self.id.fetch_add(1, Ordering::Relaxed) as i64);
        let (sender, receiver) = crossbeam_channel::bounded(1);
        self.server_requests.lock().pending.insert(
            id.clone(),
            PendingRequest {
                deadline,
                response: ResponseHandler::Chan(sender),
            },
        );
        self.send_server_rpc(JsonRpc::request_with_params(
            id.clone(),
            lsp_types::request::Shutdown::METHOD,
            Params::None(()),
        ));
        let result = receiver.recv_deadline(deadline).unwrap_or_else(|_| {
            Err(RpcError {
                code: -32098,
                message: "Language server shutdown timed out".into(),
            })
        });
        self.server_requests.lock().pending.remove(&id);
        self.send_server_notification(
            lsp_types::notification::Exit::METHOD,
            Params::None(()),
        );
        self.close_io();
        result.and_then(|value| {
            serde_json::from_value(value).map_err(|error| RpcError {
                code: -32603,
                message: format!("Invalid shutdown response: {error}"),
            })
        })
    }

    pub(super) fn close_io(&self) {
        if let Err(error) = self.io_tx.send(None) {
            tracing::debug!(?error, "language server writer already closed");
        }
    }

    pub fn shutdown(&self) {
        let mut state = self.server_requests.lock();
        if state.stopped {
            return;
        }
        state.stopped = true;
        let pending = std::mem::take(&mut state.pending);
        drop(state);
        for handler in pending.into_values() {
            handler.response.invoke(Err(Self::stopped_error()));
        }
        // to kill lsp
        self.handle_rpc(PluginServerRpc::Handler(
            PluginHandlerNotification::Shutdown,
        ));
        // to end PluginServerRpcHandler::mainloop
        self.handle_rpc(PluginServerRpc::Shutdown);
    }

    pub fn wait_for_shutdown(&self) -> bool {
        self.wait_for_shutdown_until(
            Instant::now() + SERVER_SHUTDOWN_TIMEOUT + Duration::from_secs(1),
        )
    }

    pub(super) fn wait_for_shutdown_until(&self, deadline: Instant) -> bool {
        let (terminated, changed) = &*self.terminated;
        let mut terminated = terminated.lock();
        changed.wait_while_for(
            &mut terminated,
            |terminated| !*terminated,
            deadline.saturating_duration_since(Instant::now()),
        );
        *terminated
    }

    pub fn mainloop<H>(&self, handler: &mut H)
    where
        H: PluginServerHandler,
    {
        for msg in &self.rpc_rx {
            if self.server_requests.lock().stopped
                && !matches!(
                    msg,
                    PluginServerRpc::Shutdown
                        | PluginServerRpc::Handler(
                            PluginHandlerNotification::Shutdown
                        )
                )
            {
                Self::reject_stopped(msg);
                continue;
            }
            match msg {
                PluginServerRpc::FirstCapableRequest {
                    remaining,
                    method,
                    params,
                    language_id,
                    path,
                    callback,
                } => {
                    let request_params = Params::from(params.clone());
                    if handler
                        .document_supported(language_id.as_deref(), path.as_deref())
                        && (handler.request_supported(&method, &request_params)
                            || (method == PrepareRenameRequest::METHOD
                                && handler.method_registered(Rename::METHOD)))
                    {
                        let plugin_id = self.plugin_id;
                        self.server_request_async(
                            method,
                            params,
                            language_id,
                            path,
                            true,
                            move |result| callback(plugin_id, result),
                        );
                    } else {
                        Self::request_first_capable(
                            remaining,
                            method,
                            params,
                            language_id,
                            path,
                            callback,
                        );
                    }
                }
                PluginServerRpc::ServerRequest {
                    id,
                    method,
                    params,
                    language_id,
                    path,
                    rh,
                } => {
                    let supported = handler
                        .document_supported(language_id.as_deref(), path.as_deref());
                    if supported && handler.request_supported(&method, &params) {
                        self.send_server_request(id, &method, params, rh);
                    } else if supported
                        && method == PrepareRenameRequest::METHOD
                        && handler.method_registered(Rename::METHOD)
                    {
                        // Rename preparation is optional; use the editor's word range.
                        rh.invoke(
                            serde_json::to_value(
                                lsp_types::PrepareRenameResponse::DefaultBehavior {
                                    default_behavior: true,
                                },
                            )
                            .map_err(|error| RpcError {
                                code: 0,
                                message: error.to_string(),
                            }),
                        );
                    } else if supported
                        && (method == ResolveCompletionItem::METHOD
                            || method == CodeActionResolveRequest::METHOD)
                    {
                        // LSP resolve is optional; preserve the server's original item.
                        rh.invoke(serde_json::to_value(params).map_err(|error| {
                            RpcError {
                                code: 0,
                                message: error.to_string(),
                            }
                        }));
                    } else {
                        rh.invoke(Err(RpcError {
                            code: 0,
                            message: "server not capable".to_string(),
                        }));
                    }
                }
                PluginServerRpc::ServerNotification {
                    method,
                    params,
                    language_id,
                    path,
                } => {
                    if handler
                        .document_supported(language_id.as_deref(), path.as_deref())
                        && handler.method_registered(&method)
                    {
                        self.send_server_notification(&method, params);
                    }
                }
                PluginServerRpc::HostRequest {
                    id,
                    method,
                    params,
                    resp,
                } => {
                    handler.handle_host_request(id, method, params, resp);
                }
                PluginServerRpc::HostNotification {
                    method,
                    params,
                    from,
                } => {
                    handler.handle_host_notification(method, params, from);
                }
                PluginServerRpc::DidSaveTextDocument {
                    language_id,
                    path,
                    text_document,
                    text,
                } => {
                    handler.handle_did_save_text_document(
                        language_id,
                        path,
                        text_document,
                        text,
                    );
                }
                PluginServerRpc::DidChangeTextDocument {
                    language_id,
                    document,
                    delta,
                    text,
                    new_text,
                    change,
                } => {
                    handler.handle_did_change_text_document(
                        language_id,
                        document,
                        delta,
                        text,
                        new_text,
                        change,
                    );
                }
                PluginServerRpc::FormatSemanticTokens { tokens, text, f } => {
                    handler.format_semantic_tokens(tokens, text, f);
                }
                PluginServerRpc::Handler(notification) => {
                    handler.handle_handler_notification(notification)
                }
                PluginServerRpc::Shutdown => {
                    let (terminated, changed) = &*self.terminated;
                    *terminated.lock() = true;
                    changed.notify_all();
                    return;
                }
            }
        }
    }
}

pub fn handle_plugin_server_message(
    server_rpc: &PluginServerRpcHandler,
    message: &str,
    from: &str,
) -> Option<JsonRpc> {
    match JsonRpc::parse(message) {
        Ok(value @ JsonRpc::Request(_)) => {
            let (tx, rx) = crossbeam_channel::bounded(1);
            let id = value.get_id().unwrap();
            let rpc = PluginServerRpc::HostRequest {
                id: id.clone(),
                method: value.get_method().unwrap().to_string(),
                params: value.get_params().unwrap_or(Params::None(())),
                resp: ResponseSender::new(tx),
            };
            server_rpc.handle_rpc(rpc);
            let result = rx.recv().unwrap_or_else(|_| {
                Err(RpcError {
                    code: -32603,
                    message: "language server request handler stopped".to_string(),
                })
            });
            let resp = match result {
                Ok(v) => JsonRpc::success(id, &v),
                Err(e) => JsonRpc::error(
                    id,
                    jsonrpc_lite::Error {
                        code: e.code,
                        message: e.message,
                        data: None,
                    },
                ),
            };
            Some(resp)
        }
        Ok(value @ JsonRpc::Notification(_)) => {
            let rpc = PluginServerRpc::HostNotification {
                method: value.get_method().unwrap().to_string(),
                params: value.get_params().unwrap_or(Params::None(())),
                from: from.to_string(),
            };
            server_rpc.handle_rpc(rpc);
            None
        }
        Ok(value @ JsonRpc::Success(_)) => {
            let result = value.get_result().unwrap().clone();
            server_rpc.handle_server_response(value.get_id().unwrap(), Ok(result));
            None
        }
        Ok(value @ JsonRpc::Error(_)) => {
            let error = value.get_error().unwrap();
            server_rpc.handle_server_response(
                value.get_id().unwrap(),
                Err(RpcError {
                    code: error.code,
                    message: error.message.clone(),
                }),
            );
            None
        }
        Err(err) => {
            eprintln!("parse error {err} message {message}");
            None
        }
    }
}

#[cfg(test)]
mod parameterless_message_tests {
    use super::{
        PluginServerRpc, PluginServerRpcHandler, handle_plugin_server_message,
    };
    use ahead_rpc::plugin::ServerId;
    use crossbeam_channel::unbounded;
    use jsonrpc_lite::Params;
    use serde_json::Value;

    #[test]
    fn code_actions_expire_before_the_editor_wait_without_expiring_other_requests() {
        use lsp_types::request::{CodeActionRequest, Request};

        let (io_tx, io_rx) = unbounded();
        let rpc = PluginServerRpcHandler::new(
            ServerId {
                author: "ahead".into(),
                name: "test".into(),
            },
            None,
            io_tx,
        );
        let (actions_sender, actions_receiver) = unbounded();
        rpc.server_request_async(
            CodeActionRequest::METHOD,
            Value::Null,
            None,
            None,
            false,
            move |result: Result<Value, ahead_rpc::RpcError>| {
                actions_sender.send(result).expect("code-action result");
            },
        );
        let actions_request = io_rx.recv().expect("request").expect("frame");
        let (other_sender, other_receiver) = unbounded();
        rpc.server_request_async(
            "test/other",
            Value::Null,
            None,
            None,
            false,
            move |result: Result<Value, ahead_rpc::RpcError>| {
                other_sender.send(result).expect("other result");
            },
        );
        let other_request = io_rx.recv().expect("request").expect("frame");
        rpc.expire_requests(
            std::time::Instant::now()
                + super::CODE_ACTION_REQUEST_TIMEOUT
                + std::time::Duration::from_millis(100),
        );
        let cancellation = io_rx.recv().expect("cancel").expect("frame");
        assert_eq!(cancellation.get_method(), Some("$/cancelRequest"));
        assert_eq!(
            serde_json::to_value(cancellation.get_params()).expect("cancel params")
                ["id"],
            serde_json::to_value(actions_request.get_id()).expect("id")
        );
        assert_eq!(
            actions_receiver
                .recv()
                .expect("reply")
                .expect_err("timeout")
                .code,
            -32098
        );
        assert!(other_receiver.try_recv().is_err());
        rpc.handle_server_response(
            actions_request.get_id().expect("id").clone(),
            Ok(Value::Null),
        );
        assert!(actions_receiver.try_recv().is_err());
        rpc.handle_server_response(
            other_request.get_id().expect("id").clone(),
            Ok(Value::Bool(true)),
        );
        assert_eq!(
            other_receiver.recv().expect("reply").expect("success"),
            Value::Bool(true)
        );
        assert!(io_rx.try_recv().is_err());
        rpc.shutdown();
    }

    #[test]
    fn timed_out_requests_cancel_once_and_do_not_close_the_server() {
        let (io_tx, io_rx) = unbounded();
        let rpc = PluginServerRpcHandler::new(
            ServerId {
                author: "ahead".into(),
                name: "test".into(),
            },
            None,
            io_tx,
        );
        let (sender, receiver) = unbounded();
        let reentrant = rpc.clone();
        rpc.server_request_async(
            "test/slow",
            Value::Null,
            None,
            None,
            false,
            move |result: Result<Value, ahead_rpc::RpcError>| {
                assert_eq!(result.expect_err("timeout").code, -32098);
                reentrant.server_request_async(
                    "test/retry",
                    Value::Null,
                    None,
                    None,
                    false,
                    move |reply| sender.send(reply).expect("retry result"),
                );
            },
        );
        let first = io_rx.recv().expect("request").expect("frame");
        rpc.expire_requests(
            std::time::Instant::now()
                + super::SERVER_REQUEST_TIMEOUT
                + std::time::Duration::from_secs(1),
        );
        let cancellation = io_rx.recv().expect("cancellation").expect("frame");
        assert_eq!(cancellation.get_method(), Some("$/cancelRequest"));
        assert_eq!(
            serde_json::to_value(cancellation.get_params()).expect("cancel params")
                ["id"],
            serde_json::to_value(first.get_id()).expect("id")
        );
        let retry = io_rx.recv().expect("retry request").expect("frame");
        rpc.handle_server_response(
            first.get_id().expect("first id").clone(),
            Ok(Value::Null),
        );
        assert!(receiver.try_recv().is_err());
        rpc.handle_server_response(
            retry.get_id().expect("retry id").clone(),
            Ok(Value::Bool(true)),
        );
        assert_eq!(
            receiver.recv().expect("retry reply").expect("success"),
            Value::Bool(true)
        );
        assert!(io_rx.try_recv().is_err());
        rpc.shutdown();
    }

    #[test]
    fn shutdown_flushes_exit_before_closing_io_even_after_a_timeout() {
        for respond in [true, false] {
            let (io_tx, io_rx) = unbounded();
            let rpc = PluginServerRpcHandler::new(
                ServerId {
                    author: "ahead".into(),
                    name: "test".into(),
                },
                None,
                io_tx,
            );
            rpc.shutdown();
            let responder = rpc.clone();
            let server = std::thread::spawn(move || {
                let request =
                    io_rx.recv().expect("shutdown").expect("shutdown frame");
                assert_eq!(request.get_method(), Some("shutdown"));
                if respond {
                    responder.handle_server_response(
                        request.get_id().expect("id").clone(),
                        Ok(Value::Null),
                    );
                }
                let exit = io_rx.recv().expect("exit").expect("exit frame");
                assert_eq!(exit.get_method(), Some("exit"));
                assert!(io_rx.recv().expect("close writer").is_none());
            });
            let deadline = std::time::Instant::now()
                + if respond {
                    std::time::Duration::from_secs(2)
                } else {
                    std::time::Duration::ZERO
                };
            assert_eq!(rpc.shutdown_protocol(deadline).is_ok(), respond);
            server.join().expect("server finished");
        }
    }

    #[test]
    fn shutdown_rejects_pending_and_future_requests_without_calling_back_under_lock()
    {
        let (io_tx, _io_rx) = unbounded();
        let rpc = PluginServerRpcHandler::new(
            ServerId {
                author: "ahead".into(),
                name: "test".into(),
            },
            None,
            io_tx,
        );
        let (sender, receiver) = unbounded();
        let reentrant = rpc.clone();
        rpc.server_request_async(
            "test/pending",
            Value::Null,
            None,
            None,
            false,
            move |result: Result<Value, ahead_rpc::RpcError>| {
                assert_eq!(result.unwrap_err().code, -32097);
                reentrant.server_request_async(
                    "test/reentrant",
                    Value::Null,
                    None,
                    None,
                    true,
                    move |result: Result<Value, ahead_rpc::RpcError>| {
                        sender.send(result).unwrap()
                    },
                );
            },
        );
        rpc.shutdown();
        assert_eq!(
            receiver
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap()
                .unwrap_err()
                .code,
            -32097
        );
        assert_eq!(
            rpc.server_request("test/later", Value::Null, None, None, false)
                .unwrap_err()
                .code,
            -32097
        );
        rpc.shutdown();
        rpc.handle_server_response(jsonrpc_lite::Id::Num(0), Ok(Value::Null));
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn accepts_server_messages_without_params() {
        let (io_tx, _) = unbounded();
        let handler = PluginServerRpcHandler::new(
            ServerId {
                author: "ahead".to_string(),
                name: "test".to_string(),
            },
            None,
            io_tx,
        );
        let receiver = handler.rpc_rx.clone();
        let responder = std::thread::spawn(move || {
            assert!(matches!(
                receiver.recv().unwrap(),
                PluginServerRpc::Handler(_)
            ));
            match receiver.recv().unwrap() {
                PluginServerRpc::HostRequest { params, resp, .. } => {
                    assert_eq!(params, Params::None(()));
                    resp.send_null();
                }
                _ => panic!("expected server request"),
            }
        });
        let response = handle_plugin_server_message(
            &handler,
            r#"{"jsonrpc":"2.0","id":1,"method":"workspace/diagnostic/refresh"}"#,
            "test",
        )
        .unwrap();
        responder.join().unwrap();
        assert_eq!(response.get_result(), Some(&Value::Null));

        assert!(
            handle_plugin_server_message(
                &handler,
                r#"{"jsonrpc":"2.0","method":"test/noParams"}"#,
                "test",
            )
            .is_none()
        );
        match handler.rpc_rx.recv().unwrap() {
            PluginServerRpc::HostNotification { params, .. } => {
                assert_eq!(params, Params::None(()));
            }
            _ => panic!("expected server notification"),
        }
    }
}

struct SaveRegistration {
    include_text: bool,
    filters: Option<Vec<DocumentFilter>>,
}

#[derive(Default)]
struct ServerRegistrations {
    save: HashMap<String, SaveRegistration>,
    execute_command: HashMap<String, Vec<String>>,
}

pub struct PluginHostHandler {
    server_id: ServerId,
    server_display_name: String,
    #[allow(dead_code)]
    pwd: Option<PathBuf>,
    #[allow(dead_code)]
    pub(crate) workspace: Option<PathBuf>,
    document_selector: Vec<DocumentFilter>,
    core_rpc: CoreRpcHandler,
    catalog_rpc: PluginCatalogRpcHandler,
    pub server_rpc: PluginServerRpcHandler,
    pub server_capabilities: ServerCapabilities,
    server_registrations: ServerRegistrations,
    workspace_configuration: Option<Value>,
    initialized: bool,
    pending_workspace_configuration: bool,
}

impl PluginHostHandler {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        workspace: Option<PathBuf>,
        pwd: Option<PathBuf>,
        server_id: ServerId,
        server_display_name: String,
        document_selector: DocumentSelector,
        core_rpc: CoreRpcHandler,
        server_rpc: PluginServerRpcHandler,
        catalog_rpc: PluginCatalogRpcHandler,
        workspace_configuration: Option<Value>,
    ) -> Self {
        let document_selector = document_selector
            .iter()
            .map(DocumentFilter::from_lsp_filter_loose)
            .collect();
        Self {
            pwd,
            workspace,
            server_id,
            server_display_name,
            document_selector,
            core_rpc,
            catalog_rpc,
            server_rpc,
            server_capabilities: ServerCapabilities::default(),
            server_registrations: ServerRegistrations::default(),
            workspace_configuration,
            initialized: false,
            pending_workspace_configuration: false,
        }
    }

    pub(super) fn initialized(&mut self, result: InitializeResult) {
        self.server_capabilities = result.capabilities;
        self.initialized = true;
        if std::mem::take(&mut self.pending_workspace_configuration) {
            self.notify_workspace_configuration();
        }
        self.catalog_rpc.language_server_status(
            self.server_rpc.plugin_id,
            ServerStatusParams::ready(self.server_display_name.clone()),
        );
    }

    pub(super) fn update_workspace_configuration(
        &mut self,
        configuration: Option<Value>,
    ) {
        if self.workspace_configuration == configuration {
            return;
        }
        self.workspace_configuration = configuration.clone();
        if self.initialized {
            self.notify_workspace_configuration();
        } else {
            self.pending_workspace_configuration = true;
        }
    }

    fn notify_workspace_configuration(&self) {
        self.server_rpc.server_notification(
            DidChangeConfiguration::METHOD,
            DidChangeConfigurationParams {
                settings: self
                    .workspace_configuration
                    .clone()
                    .unwrap_or(Value::Null),
            },
            None,
            None,
            false,
        );
    }

    pub(super) fn initialization_failed(&self, message: String) {
        tracing::error!(server = self.server_display_name, %message, "language server initialization failed");
        self.catalog_rpc.language_server_status(
            self.server_rpc.plugin_id,
            ServerStatusParams::failed(self.server_display_name.clone(), message),
        );
    }

    pub fn document_supported(
        &self,
        language_id: Option<&str>,
        path: Option<&Path>,
    ) -> bool {
        match language_id {
            Some(language_id) => self
                .document_selector
                .iter()
                .any(|filter| filter.matches_file_document(language_id, path)),
            None => true,
        }
    }

    pub fn method_registered(&mut self, method: &str) -> bool {
        match method {
            Initialize::METHOD => true,
            Initialized::METHOD => true,
            Completion::METHOD => {
                self.server_capabilities.completion_provider.is_some()
            }
            ResolveCompletionItem::METHOD => self
                .server_capabilities
                .completion_provider
                .as_ref()
                .and_then(|c| c.resolve_provider)
                .unwrap_or(false),
            DidOpenTextDocument::METHOD | DidCloseTextDocument::METHOD => {
                match &self.server_capabilities.text_document_sync {
                    Some(TextDocumentSyncCapability::Kind(kind)) => {
                        kind != &TextDocumentSyncKind::NONE
                    }
                    Some(TextDocumentSyncCapability::Options(options)) => options
                        .open_close
                        .or_else(|| {
                            options
                                .change
                                .map(|kind| kind != TextDocumentSyncKind::NONE)
                        })
                        .unwrap_or(false),
                    None => false,
                }
            }
            DidChangeTextDocument::METHOD => {
                match &self.server_capabilities.text_document_sync {
                    Some(TextDocumentSyncCapability::Kind(kind)) => {
                        kind != &TextDocumentSyncKind::NONE
                    }
                    Some(TextDocumentSyncCapability::Options(options)) => options
                        .change
                        .map(|kind| kind != TextDocumentSyncKind::NONE)
                        .unwrap_or(false),
                    None => false,
                }
            }
            SignatureHelpRequest::METHOD => {
                self.server_capabilities.signature_help_provider.is_some()
            }
            ExecuteCommand::METHOD => {
                self.server_capabilities.execute_command_provider.is_some()
                    || !self.server_registrations.execute_command.is_empty()
            }
            HoverRequest::METHOD => self
                .server_capabilities
                .hover_provider
                .as_ref()
                .map(|c| match c {
                    HoverProviderCapability::Simple(is_capable) => *is_capable,
                    HoverProviderCapability::Options(_) => true,
                })
                .unwrap_or(false),
            GotoDefinition::METHOD => self
                .server_capabilities
                .definition_provider
                .as_ref()
                .map(|d| match d {
                    OneOf::Left(is_capable) => *is_capable,
                    OneOf::Right(_) => true,
                })
                .unwrap_or(false),
            GotoTypeDefinition::METHOD => {
                self.server_capabilities.type_definition_provider.is_some()
            }
            References::METHOD => self
                .server_capabilities
                .references_provider
                .as_ref()
                .map(|r| match r {
                    OneOf::Left(is_capable) => *is_capable,
                    OneOf::Right(_) => true,
                })
                .unwrap_or(false),
            GotoImplementation::METHOD => self
                .server_capabilities
                .implementation_provider
                .as_ref()
                .map(|r| match r {
                    ImplementationProviderCapability::Simple(is_capable) => {
                        *is_capable
                    }
                    ImplementationProviderCapability::Options(_) => true,
                })
                .unwrap_or(false),
            FoldingRangeRequest::METHOD => self
                .server_capabilities
                .folding_range_provider
                .as_ref()
                .map(|r| match r {
                    FoldingRangeProviderCapability::Simple(support) => *support,
                    FoldingRangeProviderCapability::FoldingProvider(_) => {
                        // todo
                        true
                    }
                    FoldingRangeProviderCapability::Options(_) => {
                        // todo
                        true
                    }
                })
                .unwrap_or(false),
            CodeActionRequest::METHOD => self
                .server_capabilities
                .code_action_provider
                .as_ref()
                .map(|a| match a {
                    CodeActionProviderCapability::Simple(is_capable) => *is_capable,
                    CodeActionProviderCapability::Options(_) => true,
                })
                .unwrap_or(false),
            Formatting::METHOD => self
                .server_capabilities
                .document_formatting_provider
                .as_ref()
                .map(|f| match f {
                    OneOf::Left(is_capable) => *is_capable,
                    OneOf::Right(_) => true,
                })
                .unwrap_or(false),
            SemanticTokensFullRequest::METHOD => {
                self.server_capabilities.semantic_tokens_provider.is_some()
            }
            DocumentDiagnosticRequest::METHOD => {
                self.server_capabilities.diagnostic_provider.is_some()
            }
            InlayHintRequest::METHOD => {
                self.server_capabilities.inlay_hint_provider.is_some()
            }
            InlineCompletionRequest::METHOD => self
                .server_capabilities
                .inline_completion_provider
                .is_some(),
            DocumentSymbolRequest::METHOD => {
                self.server_capabilities.document_symbol_provider.is_some()
            }
            WorkspaceSymbolRequest::METHOD => {
                self.server_capabilities.workspace_symbol_provider.is_some()
            }
            PrepareRenameRequest::METHOD => self
                .server_capabilities
                .rename_provider
                .as_ref()
                .is_some_and(|provider| {
                    matches!(provider,
                    OneOf::Right(options) if options.prepare_provider == Some(true))
                }),
            Rename::METHOD => self
                .server_capabilities
                .rename_provider
                .as_ref()
                .is_some_and(|provider| match provider {
                    OneOf::Left(enabled) => *enabled,
                    OneOf::Right(_) => true,
                }),
            SelectionRangeRequest::METHOD => {
                self.server_capabilities.selection_range_provider.is_some()
            }
            CodeActionResolveRequest::METHOD => self
                .server_capabilities
                .code_action_provider
                .as_ref()
                .is_some_and(|provider| match provider {
                    CodeActionProviderCapability::Simple(_) => false,
                    CodeActionProviderCapability::Options(options) => {
                        options.resolve_provider.unwrap_or(false)
                    }
                }),
            CodeLensRequest::METHOD => {
                self.server_capabilities.code_lens_provider.is_some()
            }
            CodeLensResolve::METHOD => self
                .server_capabilities
                .code_lens_provider
                .as_ref()
                .and_then(|x| x.resolve_provider)
                .unwrap_or(false),
            CallHierarchyPrepare::METHOD => {
                self.server_capabilities.call_hierarchy_provider.is_some()
            }
            CallHierarchyIncomingCalls::METHOD => {
                self.server_capabilities.call_hierarchy_provider.is_some()
            }
            _ => false,
        }
    }

    pub fn request_supported(&mut self, method: &str, params: &Params) -> bool {
        if method != ExecuteCommand::METHOD {
            return self.method_registered(method);
        }
        let command = match params {
            Params::Map(params) => params.get("command").and_then(Value::as_str),
            Params::Array(_) | Params::None(_) => None,
        };
        command.is_some_and(|command| {
            self.server_capabilities
                .execute_command_provider
                .as_ref()
                .is_some_and(|provider| {
                    provider.commands.iter().any(|allowed| allowed == command)
                })
                || self.server_registrations.execute_command.values().any(
                    |commands| commands.iter().any(|allowed| allowed == command),
                )
        })
    }

    fn check_save_capability(&self, language_id: &str, path: &Path) -> (bool, bool) {
        let (mut should_send, mut include_text) = if self
            .document_supported(Some(language_id), Some(path))
        {
            self.server_capabilities
                .text_document_sync
                .as_ref()
                .and_then(|sync| match sync {
                    TextDocumentSyncCapability::Kind(_) => None,
                    TextDocumentSyncCapability::Options(options) => Some(options),
                })
                .and_then(|o| o.save.as_ref())
                .map(|o| match o {
                    TextDocumentSyncSaveOptions::Supported(is_supported) => {
                        (*is_supported, false)
                    }
                    TextDocumentSyncSaveOptions::SaveOptions(options) => {
                        (true, options.include_text.unwrap_or(false))
                    }
                })
                .unwrap_or((false, false))
        } else {
            (false, false)
        };

        for options in self.server_registrations.save.values() {
            let applies = options.filters.as_ref().map_or_else(
                || self.document_supported(Some(language_id), Some(path)),
                |filters| {
                    filters.iter().any(|filter| {
                        filter.matches_file_document(language_id, Some(path))
                    })
                },
            );
            if applies {
                should_send = true;
                include_text |= options.include_text;
            }
        }

        (should_send, include_text)
    }

    fn register_capabilities(
        &mut self,
        registrations: Vec<Registration>,
    ) -> Result<()> {
        for registration in registrations {
            self.register_capability(registration)?;
        }
        Ok(())
    }

    fn register_capability(&mut self, registration: Registration) -> Result<()> {
        match registration.method.as_str() {
            DidChangeWatchedFiles::METHOD => {
                let options = serde_json::from_value::<
                    lsp_types::DidChangeWatchedFilesRegistrationOptions,
                >(
                    registration.register_options.ok_or_else(|| {
                        anyhow!("file watchers require registration options")
                    })?,
                )?;
                let watched = WatchedFileRegistration::new(
                    self.workspace.as_deref(),
                    options,
                )?;
                self.catalog_rpc.catalog_notification(
                    super::PluginCatalogNotification::RegisterWatchedFiles {
                        plugin_id: self.server_rpc.plugin_id,
                        id: registration.id,
                        registration: watched,
                    },
                )?;
            }
            DidSaveTextDocument::METHOD => {
                let options =
                    registration
                        .register_options
                        .filter(|options| !options.is_null())
                        .map(
                            serde_json::from_value::<
                                TextDocumentSaveRegistrationOptions,
                            >,
                        )
                        .transpose()?;
                let (filters, include_text) =
                    options.map_or((None, false), |options| {
                        (
                            options
                                .text_document_registration_options
                                .document_selector
                                .map(|selector| {
                                    selector
                                        .iter()
                                        .map(DocumentFilter::from_lsp_filter_loose)
                                        .collect()
                                }),
                            options.include_text.unwrap_or(false),
                        )
                    });
                self.server_registrations.save.insert(
                    registration.id,
                    SaveRegistration {
                        include_text,
                        filters,
                    },
                );
            }
            ExecuteCommand::METHOD => {
                if let Some(options) = registration.register_options {
                    let options = serde_json::from_value::<
                        lsp_types::ExecuteCommandOptions,
                    >(options)?;
                    self.server_registrations
                        .execute_command
                        .insert(registration.id, options.commands);
                }
            }
            _ => {
                eprintln!(
                    "don't handle register capability for {}",
                    registration.method
                );
            }
        }
        Ok(())
    }

    fn unregister_capabilities(&mut self, unregisterations: Vec<Unregistration>) {
        for unregistration in unregisterations {
            if unregistration.method == DidChangeWatchedFiles::METHOD {
                if let Err(error) = self.catalog_rpc.catalog_notification(
                    super::PluginCatalogNotification::UnregisterWatchedFiles {
                        plugin_id: self.server_rpc.plugin_id,
                        id: unregistration.id,
                    },
                ) {
                    tracing::error!(
                        ?error,
                        "unregistering language-server file watchers"
                    );
                }
            } else if unregistration.method == DidSaveTextDocument::METHOD {
                self.server_registrations.save.remove(&unregistration.id);
            } else if unregistration.method == ExecuteCommand::METHOD {
                self.server_registrations
                    .execute_command
                    .remove(&unregistration.id);
            }
        }
    }

    pub fn handle_request(
        &mut self,
        _id: Id,
        method: String,
        params: Params,
        resp: ResponseSender,
    ) {
        if let Err(err) = self.process_request(method, params, resp.clone()) {
            resp.send_err(0, err.to_string());
        }
    }

    pub fn process_request(
        &mut self,
        method: String,
        params: Params,
        resp: ResponseSender,
    ) -> Result<()> {
        match method.as_str() {
            WorkDoneProgressCreate::METHOD => {
                resp.send_null();
            }
            RegisterCapability::METHOD => {
                let params: RegistrationParams =
                    serde_json::from_value(serde_json::to_value(params)?)?;
                self.register_capabilities(params.registrations)?;
                resp.send_null();
            }
            UnregisterCapability::METHOD => {
                let params: UnregistrationParams =
                    serde_json::from_value(serde_json::to_value(params)?)?;
                self.unregister_capabilities(params.unregisterations);
                resp.send_null();
            }
            "workspace/configuration" => {
                let params = serde_json::to_value(params)?;
                let result = workspace_configuration_response(
                    self.workspace_configuration.as_ref(),
                    &params,
                );
                resp.send(result);
            }
            "workspace/applyEdit" => {
                let params: lsp_types::ApplyWorkspaceEditParams =
                    serde_json::from_value(serde_json::to_value(params)?)?;
                let response = match self.core_rpc.request(
                    ahead_rpc::core::CoreRequest::ApplyWorkspaceEdit(params),
                ) {
                    Ok(ahead_rpc::core::CoreResponse::ApplyWorkspaceEdit(
                        response,
                    )) => response,
                    Ok(_) => lsp_types::ApplyWorkspaceEditResponse {
                        applied: false,
                        failure_reason: Some(
                            "The editor returned an unexpected response".into(),
                        ),
                        failed_change: None,
                    },
                    Err(error) => lsp_types::ApplyWorkspaceEditResponse {
                        applied: false,
                        failure_reason: Some(error.message),
                        failed_change: None,
                    },
                };
                resp.send(response);
            }
            "workspace/diagnostic/refresh" => {
                resp.send_null();
                self.catalog_rpc
                    .refresh_diagnostics(self.server_rpc.plugin_id);
            }
            _ => return Err(anyhow!("request not supported")),
        }

        Ok(())
    }

    pub fn handle_notification(
        &mut self,
        method: String,
        params: Params,
        from: String,
    ) -> Result<()> {
        match method.as_str() {
            PublishDiagnostics::METHOD => {
                let diagnostics: PublishDiagnosticsParams =
                    serde_json::from_value(serde_json::to_value(params)?)?;
                self.catalog_rpc.publish_diagnostics(
                    self.server_rpc.plugin_id,
                    super::DiagnosticSource::Pushed,
                    diagnostics,
                );
            }
            Progress::METHOD => {
                let progress: ProgressParams =
                    serde_json::from_value(serde_json::to_value(params)?)?;
                self.catalog_rpc.core_rpc.work_done_progress(progress);
            }
            ShowMessage::METHOD => {
                let message: ShowMessageParams =
                    serde_json::from_value(serde_json::to_value(params)?)?;
                let title = format!("Language server: {}", self.server_display_name);
                self.catalog_rpc.core_rpc.show_message(title, message);
            }
            LogMessage::METHOD => {
                let message: LogMessageParams =
                    serde_json::from_value(serde_json::to_value(params)?)?;
                self.catalog_rpc.core_rpc.log_message(
                    message,
                    format!(
                        "ahead_proxy::plugin::psp::{}::{}::LogMessage",
                        self.server_id.author, self.server_id.name
                    ),
                );
            }
            Cancel::METHOD => {
                let params: CancelParams =
                    serde_json::from_value(serde_json::to_value(params)?)?;
                self.catalog_rpc.core_rpc.cancel(params);
            }
            "experimental/serverStatus" => {
                let mut param: ServerStatusParams =
                    serde_json::from_value(serde_json::to_value(params)?)?;
                param.server_name = Some(self.server_display_name.clone());
                if !param.is_ok() {
                    if let Some(msg) = &param.message {
                        self.core_rpc.show_message(
                            from,
                            ShowMessageParams {
                                typ: MessageType::ERROR,
                                message: msg.clone(),
                            },
                        );
                    }
                }
                self.catalog_rpc
                    .language_server_status(self.server_rpc.plugin_id, param);
            }
            _ => {
                self.core_rpc.log(
                    ahead_rpc::core::LogLevel::Warn,
                    format!("host notification {method} not handled"),
                    Some(format!(
                        "ahead_proxy::plugin::psp::{}::{}::{method}",
                        self.server_id.author, self.server_id.name
                    )),
                );
            }
        }
        Ok(())
    }

    pub fn handle_did_save_text_document(
        &self,
        language_id: String,
        path: PathBuf,
        text_document: TextDocumentIdentifier,
        text: Rope,
    ) {
        let (should_send, include_text) =
            self.check_save_capability(language_id.as_str(), &path);
        if !should_send {
            return;
        }
        let params = DidSaveTextDocumentParams {
            text_document,
            text: if include_text {
                Some(text.to_string())
            } else {
                None
            },
        };
        self.server_rpc.server_notification(
            DidSaveTextDocument::METHOD,
            params,
            Some(language_id),
            Some(path),
            false,
        );
    }

    pub fn handle_did_change_text_document(
        &mut self,
        lanaguage_id: String,
        document: VersionedTextDocumentIdentifier,
        delta: AheadDelta,
        text: Rope,
        new_text: Rope,
        change: Arc<
            Mutex<(
                Option<TextDocumentContentChangeEvent>,
                Option<TextDocumentContentChangeEvent>,
            )>,
        >,
    ) {
        let kind = match &self.server_capabilities.text_document_sync {
            Some(TextDocumentSyncCapability::Kind(kind)) => *kind,
            Some(TextDocumentSyncCapability::Options(options)) => {
                options.change.unwrap_or(TextDocumentSyncKind::NONE)
            }
            None => TextDocumentSyncKind::NONE,
        };

        let mut existing = change.lock();
        let change = match kind {
            TextDocumentSyncKind::FULL => {
                if let Some(c) = existing.0.as_ref() {
                    c.clone()
                } else {
                    let change = TextDocumentContentChangeEvent {
                        range: None,
                        range_length: None,
                        text: new_text.to_string(),
                    };
                    existing.0 = Some(change.clone());
                    change
                }
            }
            TextDocumentSyncKind::INCREMENTAL => {
                if let Some(c) = existing.1.as_ref() {
                    c.clone()
                } else {
                    let change =
                        get_document_content_change(&text, &delta, &new_text);
                    existing.1 = Some(change.clone());
                    change
                }
            }
            TextDocumentSyncKind::NONE => return,
            _ => return,
        };

        let path = document.uri.to_file_path().ok();

        let params = DidChangeTextDocumentParams {
            text_document: document,
            content_changes: vec![change],
        };

        self.server_rpc.server_notification(
            DidChangeTextDocument::METHOD,
            params,
            Some(lanaguage_id),
            path,
            false,
        );
    }

    pub fn format_semantic_tokens(
        &self,
        tokens: SemanticTokens,
        text: Rope,
        f: Box<dyn RpcCallback<Vec<LineStyle>, RpcError>>,
    ) {
        let result = format_semantic_styles(
            &text,
            self.server_capabilities.semantic_tokens_provider.as_ref(),
            &tokens,
        )
        .ok_or_else(|| RpcError {
            code: 0,
            message: "can't get styles".to_string(),
        });
        f.call(result);
    }
}

fn workspace_configuration_response(
    settings: Option<&Value>,
    params: &Value,
) -> Value {
    // The Zed extension WIT callback accepts a worktree, not a scope URI, so
    // this extension-provided configuration is intentionally scope-invariant.
    let empty_settings = Value::Object(Default::default());
    let settings = settings.unwrap_or(&empty_settings);
    let items = params
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| match item.get("section").and_then(Value::as_str) {
            // VS Code-style servers may dereference nested fields on an absent section.
            Some(section) => settings
                .get(section)
                .cloned()
                .unwrap_or_else(|| Value::Object(Default::default())),
            None => settings.clone(),
        })
        .collect();
    Value::Array(items)
}

#[cfg(test)]
mod save_capability_tests {
    use super::*;

    #[test]
    fn document_selectors_and_dynamic_save_registrations_match_file_scheme() {
        let make_host = |scheme: &str| {
            let core_rpc = CoreRpcHandler::new();
            let catalog_rpc = PluginCatalogRpcHandler::new(core_rpc.clone());
            let (io_tx, _io_rx) = crossbeam_channel::unbounded();
            let server_id = ServerId {
                author: "ahead".into(),
                name: "scheme-test".into(),
            };
            let server_rpc =
                PluginServerRpcHandler::new(server_id.clone(), None, io_tx);
            PluginHostHandler::new(
                None,
                None,
                server_id,
                "Scheme Test".into(),
                vec![lsp_types::DocumentFilter {
                    language: Some("rust".into()),
                    scheme: Some(scheme.into()),
                    pattern: None,
                }],
                core_rpc,
                server_rpc,
                catalog_rpc,
                None,
            )
        };
        let file_path = Path::new("/workspace/src/main.rs");
        let host = make_host("FILE");
        assert!(host.document_supported(Some("rust"), Some(file_path)));
        assert!(
            !make_host("untitled").document_supported(Some("rust"), Some(file_path))
        );

        let mut host = make_host("file");
        host.register_capability(Registration {
            id: "untitled-save".into(),
            method: DidSaveTextDocument::METHOD.into(),
            register_options: Some(serde_json::json!({
                "documentSelector": [{"language": "rust", "scheme": "untitled"}],
                "includeText": true
            })),
        })
        .expect("register non-file save filter");
        assert_eq!(
            host.check_save_capability("rust", file_path),
            (false, false)
        );

        host.register_capability(Registration {
            id: "file-save".into(),
            method: DidSaveTextDocument::METHOD.into(),
            register_options: Some(serde_json::json!({
                "documentSelector": [{"language": "rust", "scheme": "file"}],
                "includeText": true
            })),
        })
        .expect("register file save filter");
        assert_eq!(host.check_save_capability("rust", file_path), (true, true));
    }

    #[test]
    fn dynamic_file_watchers_validate_before_ack_and_route_registration_by_owner() {
        let project = tempfile::tempdir().expect("project");
        let core = CoreRpcHandler::new();
        let catalog = PluginCatalogRpcHandler::new(core.clone());
        let receiver = catalog.test_receiver();
        let (sender, _io) = crossbeam_channel::unbounded();
        let server_id = ServerId {
            author: "ahead".into(),
            name: "test".into(),
        };
        let server = PluginServerRpcHandler::new(
            server_id.clone(),
            Some(PluginId(42)),
            sender,
        );
        let mut host = PluginHostHandler::new(
            Some(project.path().to_owned()),
            None,
            server_id,
            "test".into(),
            Vec::new(),
            core,
            server,
            catalog,
            None,
        );
        let (sender, response) = crossbeam_channel::unbounded();
        host.handle_request(Id::Num(1), RegisterCapability::METHOD.into(), Params::from(serde_json::json!({
            "registrations": [{ "id": "config", "method": DidChangeWatchedFiles::METHOD, "registerOptions": { "watchers": [{ "globPattern": "**/*.toml" }] } }]
        })), ResponseSender::new(sender));
        assert_eq!(
            response.recv().expect("ack").expect("valid registration"),
            Value::Null
        );
        assert!(
            matches!(receiver.try_recv(), Ok(super::super::PluginCatalogRpc::Handler(super::super::PluginCatalogNotification::RegisterWatchedFiles { plugin_id: PluginId(42), id, .. })) if id == "config")
        );
        let (sender, response) = crossbeam_channel::unbounded();
        host.handle_request(Id::Num(2), RegisterCapability::METHOD.into(), Params::from(serde_json::json!({
            "registrations": [{ "id": "bad", "method": DidChangeWatchedFiles::METHOD, "registerOptions": { "watchers": [{ "globPattern": "[invalid" }] } }]
        })), ResponseSender::new(sender));
        assert!(
            response
                .recv()
                .expect("invalid registration response")
                .is_err()
        );
        assert!(
            receiver.try_recv().is_err(),
            "invalid registration must not be installed"
        );
        host.unregister_capabilities(vec![Unregistration {
            id: "config".into(),
            method: DidChangeWatchedFiles::METHOD.into(),
        }]);
        assert!(
            matches!(receiver.try_recv(), Ok(super::super::PluginCatalogRpc::Handler(super::super::PluginCatalogNotification::UnregisterWatchedFiles { plugin_id: PluginId(42), id })) if id == "config")
        );
    }

    #[test]
    fn execute_command_is_limited_to_server_advertised_commands() {
        let core_rpc = CoreRpcHandler::new();
        let catalog_rpc = PluginCatalogRpcHandler::new(core_rpc.clone());
        let server_id = ServerId {
            author: "ahead".into(),
            name: "test".into(),
        };
        let (sender, _receiver) = crossbeam_channel::unbounded();
        let server_rpc =
            PluginServerRpcHandler::new(server_id.clone(), None, sender);
        let mut host = PluginHostHandler::new(
            None,
            None,
            server_id,
            "Test LSP".into(),
            Vec::new(),
            core_rpc,
            server_rpc,
            catalog_rpc,
            None,
        );
        let capabilities = serde_json::from_value(serde_json::json!({
            "executeCommandProvider": {"commands": ["allowed.command"]}
        }))
        .expect("valid execute-command capability");
        host.initialized(InitializeResult {
            capabilities,
            server_info: None,
            offset_encoding: None,
        });
        let request = |command: &str| {
            Params::from(serde_json::json!({"command": command, "arguments": []}))
        };

        assert!(
            host.request_supported(
                ExecuteCommand::METHOD,
                &request("allowed.command")
            )
        );
        assert!(!host.request_supported(
            ExecuteCommand::METHOD,
            &request("unadvertised.command")
        ));
        assert!(!host.request_supported(
            ExecuteCommand::METHOD,
            &Params::from(serde_json::json!({"arguments": []}))
        ));
    }

    #[test]
    fn dynamic_execute_commands_are_allowlisted_and_unregistered_by_id() {
        let core_rpc = CoreRpcHandler::new();
        let catalog_rpc = PluginCatalogRpcHandler::new(core_rpc.clone());
        let server_id = ServerId {
            author: "ahead".into(),
            name: "test".into(),
        };
        let (sender, _receiver) = crossbeam_channel::unbounded();
        let server_rpc =
            PluginServerRpcHandler::new(server_id.clone(), None, sender);
        let mut host = PluginHostHandler::new(
            None,
            None,
            server_id,
            "Test LSP".into(),
            Vec::new(),
            core_rpc,
            server_rpc,
            catalog_rpc,
            None,
        );
        let request = |command: &str| {
            Params::from(serde_json::json!({"command": command, "arguments": []}))
        };

        host.register_capability(Registration {
            id: "commands-one".into(),
            method: ExecuteCommand::METHOD.into(),
            register_options: Some(serde_json::json!({
                "commands": ["dynamic.only", "shared.command"]
            })),
        })
        .expect("first dynamic execute-command registration");
        host.register_capability(Registration {
            id: "commands-two".into(),
            method: ExecuteCommand::METHOD.into(),
            register_options: Some(serde_json::json!({
                "commands": ["shared.command"]
            })),
        })
        .expect("overlapping dynamic execute-command registration");

        assert!(host.method_registered(ExecuteCommand::METHOD));
        assert!(
            host.request_supported(ExecuteCommand::METHOD, &request("dynamic.only"))
        );
        assert!(
            host.request_supported(
                ExecuteCommand::METHOD,
                &request("shared.command")
            )
        );
        assert!(
            !host.request_supported(
                ExecuteCommand::METHOD,
                &request("unknown.command")
            )
        );

        host.unregister_capabilities(vec![Unregistration {
            id: "commands-one".into(),
            method: ExecuteCommand::METHOD.into(),
        }]);
        assert!(
            !host
                .request_supported(ExecuteCommand::METHOD, &request("dynamic.only"))
        );
        assert!(
            host.request_supported(
                ExecuteCommand::METHOD,
                &request("shared.command")
            )
        );

        host.unregister_capabilities(vec![Unregistration {
            id: "commands-two".into(),
            method: ExecuteCommand::METHOD.into(),
        }]);
        assert!(!host.method_registered(ExecuteCommand::METHOD));
        assert!(
            !host.request_supported(
                ExecuteCommand::METHOD,
                &request("shared.command")
            )
        );
    }

    #[test]
    fn initialization_and_custom_status_use_the_same_server_identity() {
        use crate::plugin::{PluginCatalogNotification, PluginCatalogRpc};
        let core_rpc = CoreRpcHandler::new();
        let catalog_rpc = PluginCatalogRpcHandler::new(core_rpc.clone());
        let notifications = catalog_rpc
            .plugin_rx
            .lock()
            .take()
            .expect("catalog receiver");
        let server_id = ServerId {
            author: "ahead".into(),
            name: "lsp-test".into(),
        };
        let (sender, _receiver) = crossbeam_channel::unbounded();
        let server_rpc =
            PluginServerRpcHandler::new(server_id.clone(), None, sender);
        let mut host = PluginHostHandler::new(
            None,
            None,
            server_id,
            "Test LSP".into(),
            Vec::new(),
            core_rpc.clone(),
            server_rpc,
            catalog_rpc,
            None,
        );
        let next_status = || {
            let PluginCatalogRpc::Handler(
                PluginCatalogNotification::LanguageServerStatus { params, .. },
            ) = notifications.try_recv().expect("status notification")
            else {
                panic!("expected server status")
            };
            assert_eq!(params.server_name.as_deref(), Some("Test LSP"));
            params
        };
        host.initialized(InitializeResult {
            capabilities: ServerCapabilities::default(),
            server_info: None,
            offset_encoding: None,
        });
        assert!(next_status().is_ok());
        host.handle_notification(
            "experimental/serverStatus".into(),
            Params::from(serde_json::json!({"health": "ok", "quiescent": false})),
            "Test LSP".into(),
        )
        .expect("server status");
        assert!(!next_status().is_quiescent());
        host.initialization_failed("invalid capabilities".into());
        let failed = next_status();
        assert!(!failed.is_ok());
        assert_eq!(failed.message.as_deref(), Some("invalid capabilities"));
    }

    #[test]
    fn incremental_snapshot_changes_replace_the_old_document_range() {
        use ahead_rpc::delta::DeltaOp;
        let snapshots = [
            "import { calculate } from './math.ts';\n// café 日本語\nconst result = calculate(21);\nconsole.log(result.doubled);\n",
            "import { calculate } from './math.ts';\nconst result = calculate(21);\nresult.dou",
            "import { calculate } from './math.ts';\nconst result = calculate(21);\nresult.doubled",
            "α🙂target",
            "",
        ];
        for pair in snapshots.windows(2) {
            let old_text = Rope::from(pair[0]);
            let new_text = Rope::from(pair[1]);
            let delta = AheadDelta::new(
                old_text.len(),
                vec![
                    DeltaOp::Delete(old_text.len()),
                    DeltaOp::Insert(pair[1].to_owned()),
                ],
            );
            let change = get_document_content_change(&old_text, &delta, &new_text);
            let range = change.range.expect("incremental range");
            assert_eq!(range.start, Position::new(0, 0));
            let last_line = pair[0].rsplit('\n').next().expect("last line");
            assert_eq!(
                range.end,
                Position::new(
                    pair[0].bytes().filter(|byte| *byte == b'\n').count() as u32,
                    last_line.encode_utf16().count() as u32
                )
            );
            assert_eq!(change.text, pair[1]);
        }
    }

    #[test]
    fn incremental_selection_replacement_keeps_a_precise_utf16_range() {
        use ahead_rpc::delta::DeltaOp;

        let old_text = Rope::from("a🙂bcdef");
        let delta = AheadDelta::new(
            old_text.len(),
            vec![
                DeltaOp::Retain(1),
                DeltaOp::Delete("🙂b".len()),
                DeltaOp::Insert("🦀".into()),
                DeltaOp::Retain("cdef".len()),
            ],
        );
        let new_text = delta.apply(&old_text);
        let change = get_document_content_change(&old_text, &delta, &new_text);
        let range = change.range.expect("incremental replacement range");

        assert_eq!(range.start, Position::new(0, 1));
        assert_eq!(range.end, Position::new(0, 4));
        assert_eq!(change.text, "🦀");
        assert_eq!(new_text.to_string(), "a🦀cdef");
    }

    #[test]
    fn dynamic_save_registrations_merge_null_and_unregister_by_id() {
        let core_rpc = CoreRpcHandler::new();
        let catalog_rpc = PluginCatalogRpcHandler::new(core_rpc.clone());
        let server_id = ServerId {
            author: "ahead".into(),
            name: "rust".into(),
        };
        let (io_tx, _io_rx) = crossbeam_channel::unbounded();
        let server_rpc = PluginServerRpcHandler::new(server_id.clone(), None, io_tx);
        let mut host = PluginHostHandler::new(
            None,
            None,
            server_id,
            "Rust Analyzer".into(),
            vec![lsp_types::DocumentFilter {
                language: Some("rust".into()),
                scheme: None,
                pattern: None,
            }],
            core_rpc,
            server_rpc,
            catalog_rpc,
            None,
        );
        let rust_path = Path::new("/workspace/src/main.rs");
        assert_eq!(
            host.check_save_capability("rust", rust_path),
            (false, false)
        );
        host.register_capability(Registration {
            id: "save".into(),
            method: DidSaveTextDocument::METHOD.into(),
            register_options: Some(serde_json::json!({
                "documentSelector": [{"pattern": "**/*.rs"}, {"pattern": "**/Cargo.toml"}],
                "includeText": false
            })),
        })
        .expect("valid dynamic registration");
        host.register_capability(Registration {
            id: "save-text".into(),
            method: DidSaveTextDocument::METHOD.into(),
            register_options: Some(serde_json::json!({
                "documentSelector": [{"language": "rust"}],
                "includeText": true
            })),
        })
        .expect("valid overlapping dynamic registration");
        assert_eq!(host.check_save_capability("rust", rust_path), (true, true));
        assert_eq!(
            host.check_save_capability("toml", Path::new("/workspace/Cargo.toml")),
            (true, false)
        );
        assert_eq!(
            host.check_save_capability("json", Path::new("/workspace/data.json")),
            (false, false)
        );

        let (unregister_sender, unregister_receiver) = crossbeam_channel::bounded(1);
        host.process_request(
            UnregisterCapability::METHOD.into(),
            Params::from(serde_json::json!({
                "unregisterations": [{
                    "id": "save-text",
                    "method": DidSaveTextDocument::METHOD
                }]
            })),
            ResponseSender::new(unregister_sender),
        )
        .expect("unregister request");
        assert_eq!(
            unregister_receiver
                .recv()
                .expect("unregister response")
                .expect("unregister response is successful"),
            Value::Null
        );
        assert_eq!(host.check_save_capability("rust", rust_path), (true, false));

        host.unregister_capabilities(vec![Unregistration {
            id: "save".into(),
            method: "textDocument/didChange".into(),
        }]);
        assert_eq!(
            host.check_save_capability("toml", Path::new("/workspace/Cargo.toml")),
            (true, false)
        );
        host.unregister_capabilities(vec![Unregistration {
            id: "save".into(),
            method: DidSaveTextDocument::METHOD.into(),
        }]);
        assert_eq!(
            host.check_save_capability("rust", rust_path),
            (false, false)
        );

        host.register_capability(Registration {
            id: "save-all".into(),
            method: DidSaveTextDocument::METHOD.into(),
            register_options: Some(Value::Null),
        })
        .expect("null options use the client document selector");
        assert_eq!(host.check_save_capability("rust", rust_path), (true, false));
        assert_eq!(
            host.check_save_capability("python", Path::new("/workspace/main.py")),
            (false, false)
        );
        host.unregister_capabilities(vec![Unregistration {
            id: "save-all".into(),
            method: DidSaveTextDocument::METHOD.into(),
        }]);
        assert_eq!(
            host.check_save_capability("rust", rust_path),
            (false, false)
        );

        host.server_capabilities.text_document_sync =
            Some(TextDocumentSyncCapability::Options(
                lsp_types::TextDocumentSyncOptions {
                    save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                    ..Default::default()
                },
            ));
        assert_eq!(host.check_save_capability("rust", rust_path), (true, false));
        for kind in [
            TextDocumentSyncKind::NONE,
            TextDocumentSyncKind::FULL,
            TextDocumentSyncKind::INCREMENTAL,
        ] {
            host.server_capabilities.text_document_sync =
                Some(TextDocumentSyncCapability::Kind(kind));
            assert_eq!(
                host.method_registered(DidCloseTextDocument::METHOD),
                kind != TextDocumentSyncKind::NONE
            );
        }
    }
}

#[cfg(test)]
mod workspace_configuration_tests {
    use super::workspace_configuration_response;
    use serde_json::json;

    #[test]
    fn changed_configuration_updates_requests_and_notifies_after_initialize() {
        use super::{PluginHostHandler, PluginServerRpcHandler};
        use ahead_rpc::{core::CoreRpcHandler, plugin::ServerId};
        use lsp_types::notification::{DidChangeConfiguration, Notification};

        let core_rpc = CoreRpcHandler::new();
        let catalog_rpc = super::PluginCatalogRpcHandler::new(core_rpc.clone());
        let server_id = ServerId {
            author: "ahead".into(),
            name: "test-lsp".into(),
        };
        let (io_tx, io_rx) = crossbeam_channel::unbounded();
        let server_rpc = PluginServerRpcHandler::new(server_id.clone(), None, io_tx);
        let mut host = PluginHostHandler::new(
            None,
            None,
            server_id,
            "Test LSP".into(),
            Vec::new(),
            core_rpc,
            server_rpc,
            catalog_rpc,
            None,
        );
        let first = json!({ "test": { "value": 1 } });
        host.update_workspace_configuration(Some(first.clone()));
        assert_eq!(
            workspace_configuration_response(
                host.workspace_configuration.as_ref(),
                &json!({ "items": [{}] }),
            ),
            json!([first])
        );
        assert!(io_rx.try_recv().is_err());

        host.initialized(
            serde_json::from_value(json!({ "capabilities": {} }))
                .expect("initialize result"),
        );
        let changed = io_rx
            .recv()
            .expect("configuration notification")
            .expect("frame");
        assert_eq!(changed.get_method(), Some(DidChangeConfiguration::METHOD));
        assert_eq!(
            serde_json::to_value(changed.get_params()).expect("params")["settings"],
            json!({ "test": { "value": 1 } })
        );
        host.update_workspace_configuration(Some(first));
        assert!(io_rx.try_recv().is_err());
        host.update_workspace_configuration(None);
        let cleared = io_rx.recv().expect("configuration clear").expect("frame");
        assert_eq!(
            serde_json::to_value(cleared.get_params()).expect("params")["settings"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn client_advertises_workspace_configuration_support() {
        assert_eq!(
            super::super::client_capabilities()
                .workspace
                .and_then(|workspace| workspace.configuration),
            Some(true)
        );
    }

    #[test]
    fn responds_with_requested_sections_and_empty_objects_for_missing_settings() {
        let settings = json!({
            "rust-analyzer": { "check": { "command": "clippy" } },
            "zed": { "channel": "stable" }
        });
        let params = json!({
            "items": [
                {
                    "scopeUri": "file:///workspace/src/lib.rs",
                    "section": "rust-analyzer"
                },
                {
                    "scopeUri": "file:///workspace/tests/lib.rs",
                    "section": "rust-analyzer"
                },
                {},
                { "section": "missing" },
                { "section": null }
            ]
        });

        assert_eq!(
            workspace_configuration_response(Some(&settings), &params),
            json!([
                { "check": { "command": "clippy" } },
                { "check": { "command": "clippy" } },
                settings,
                {},
                settings
            ])
        );
    }

    #[test]
    fn responds_with_empty_configuration_when_no_items_or_settings_exist() {
        assert_eq!(
            workspace_configuration_response(None, &json!({ "items": [] })),
            json!([])
        );
        assert_eq!(
            workspace_configuration_response(None, &json!({ "items": [{}] })),
            json!([{}])
        );
    }
}

/// Byte offset of a line start, clamped to the document.
fn offset_of_line(text: &Rope, line: usize) -> usize {
    let lines = text.len_lines(LineType::LF_CR);
    text.line_to_byte_idx(line.min(lines), LineType::LF_CR)
}

/// Converts a UTF-8 byte offset to an LSP position, clamping into range.
fn offset_to_position(text: &Rope, offset: usize) -> Position {
    let offset = offset.min(text.len());
    let line = text.byte_to_line_idx(offset, LineType::LF_CR);
    let line_start = text.line_to_byte_idx(line, LineType::LF_CR);
    let utf16_col = offset_utf8_to_utf16(
        text.slice(line_start..).char_indices(),
        offset - line_start,
    );
    Position {
        line: line as u32,
        character: utf16_col as u32,
    }
}

fn get_document_content_change(
    text: &Rope,
    delta: &AheadDelta,
    new_text: &Rope,
) -> TextDocumentContentChangeEvent {
    if let Some((range, replacement)) = delta.contiguous_replacement(text, new_text)
    {
        return TextDocumentContentChangeEvent {
            range: Some(Range {
                start: offset_to_position(text, range.start),
                end: offset_to_position(text, range.end),
            }),
            range_length: None,
            text: replacement,
        };
    }

    // Invalid or mismatched deltas must still leave incremental servers with
    // the complete new snapshot rather than applying a partial edit.
    TextDocumentContentChangeEvent {
        range: Some(Range::new(
            Position::new(0, 0),
            offset_to_position(text, text.len()),
        )),
        range_length: None,
        text: new_text.to_string(),
    }
}

fn format_semantic_styles(
    text: &Rope,
    semantic_tokens_provider: Option<&SemanticTokensServerCapabilities>,
    tokens: &SemanticTokens,
) -> Option<Vec<LineStyle>> {
    let semantic_tokens_provider = semantic_tokens_provider?;
    let semantic_legends = semantic_tokens_legend(semantic_tokens_provider);

    let mut highlights = Vec::new();
    let mut line = 0;
    let mut start = 0;
    let mut last_start = 0;
    for semantic_token in &tokens.data {
        if semantic_token.delta_line > 0 {
            line += semantic_token.delta_line as usize;
            start = offset_of_line(text, line);
        }

        let sub_text = text.slice(start..).char_indices();
        start += offset_utf16_to_utf8(sub_text, semantic_token.delta_start as usize);

        let sub_text = text.slice(start..).char_indices();
        let end =
            start + offset_utf16_to_utf8(sub_text, semantic_token.length as usize);

        let kind = semantic_legends.token_types[semantic_token.token_type as usize]
            .as_str()
            .to_string();
        if start < last_start {
            continue;
        }
        last_start = start;
        highlights.push(LineStyle {
            start,
            end,
            style: Style {
                fg_color: Some(kind),
            },
        });
    }

    Some(highlights)
}

fn semantic_tokens_legend(
    semantic_tokens_provider: &SemanticTokensServerCapabilities,
) -> &SemanticTokensLegend {
    match semantic_tokens_provider {
        SemanticTokensServerCapabilities::SemanticTokensOptions(options) => {
            &options.legend
        }
        SemanticTokensServerCapabilities::SemanticTokensRegistrationOptions(
            options,
        ) => &options.semantic_tokens_options.legend,
    }
}

#[cfg(test)]
mod workspace_apply_edit_tests {
    use super::{
        PluginCatalogRpcHandler, PluginHostHandler, PluginServerRpcHandler,
        ResponseSender,
    };
    use ahead_rpc::{
        core::{CoreRequest, CoreResponse, CoreRpc, CoreRpcHandler},
        plugin::ServerId,
    };
    use crossbeam_channel::bounded;
    use lsp_types::{ApplyWorkspaceEditParams, WorkspaceEdit};

    #[test]
    fn server_apply_edit_relays_to_editor_and_returns_its_result() {
        let core_rpc = CoreRpcHandler::new();
        let core_messages = core_rpc.rx().clone();
        let catalog_rpc = PluginCatalogRpcHandler::new(core_rpc.clone());
        let server_id = ServerId {
            author: "ahead".into(),
            name: "test-lsp".into(),
        };
        let (io_sender, _) = crossbeam_channel::unbounded();
        let server_rpc =
            PluginServerRpcHandler::new(server_id.clone(), None, io_sender);
        let mut host = PluginHostHandler::new(
            None,
            None,
            server_id,
            "Test LSP".into(),
            Vec::new(),
            core_rpc.clone(),
            server_rpc,
            catalog_rpc,
            None,
        );
        let params = ApplyWorkspaceEditParams {
            label: Some("Add import".into()),
            edit: WorkspaceEdit::default(),
        };
        let rpc_params = jsonrpc_lite::Params::from(
            serde_json::to_value(params).expect("serialize workspace edit"),
        );
        let (response_sender, response_receiver) = bounded(1);
        let worker = std::thread::spawn(move || {
            host.process_request(
                "workspace/applyEdit".into(),
                rpc_params,
                ResponseSender::new(response_sender),
            )
        });

        let CoreRpc::Request(request_id, request) = core_messages
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("workspace edit core request")
        else {
            panic!("expected a core request");
        };
        assert!(matches!(
            request,
            CoreRequest::ApplyWorkspaceEdit(params)
                if params.label.as_deref() == Some("Add import")
        ));
        core_rpc.handle_response(
            request_id,
            Ok(CoreResponse::ApplyWorkspaceEdit(
                lsp_types::ApplyWorkspaceEditResponse {
                    applied: false,
                    failure_reason: Some("stale document version".into()),
                    failed_change: None,
                },
            )),
        );
        worker
            .join()
            .expect("server request worker")
            .expect("process server request");

        let response = response_receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("LSP response")
            .expect("serialize LSP response");
        assert_eq!(response["applied"], false);
        assert_eq!(response["failureReason"], "stale document version");
    }
}
