pub mod catalog;
pub mod dap;
pub mod lsp;
pub mod psp;
mod routing;
pub mod watched_files;

use std::{
    borrow::Cow,
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use ahead_extension_host::{LanguageServerCommand, LanguageServerExtension};
use ahead_rpc::{
    RequestId, RpcError,
    core::CoreRpcHandler,
    dap_types::{self, DapId, RunDebugConfig, SourceBreakpoint, ThreadId},
    delta::AheadDelta,
    plugin::PluginId,
    proxy::LspCodeAction,
    style::LineStyle,
};
use anyhow::{Result, anyhow};
use crossbeam_channel::{Receiver, Sender};
use dyn_clone::DynClone;
use lsp_types::{
    CallHierarchyClientCapabilities, CallHierarchyIncomingCall,
    CallHierarchyIncomingCallsParams, CallHierarchyItem, CallHierarchyPrepareParams,
    ClientCapabilities, CodeAction, CodeActionCapabilityResolveSupport,
    CodeActionClientCapabilities, CodeActionContext, CodeActionKind,
    CodeActionKindLiteralSupport, CodeActionLiteralSupport, CodeActionParams,
    CodeActionResponse, CodeLens, CodeLensParams, CompletionClientCapabilities,
    CompletionContext, CompletionItem, CompletionItemCapability,
    CompletionItemCapabilityResolveSupport, CompletionParams, CompletionResponse,
    CompletionTriggerKind, Diagnostic, DocumentFormattingParams,
    DocumentSymbolClientCapabilities, DocumentSymbolParams, DocumentSymbolResponse,
    ExecuteCommandParams, FoldingRange, FoldingRangeClientCapabilities,
    FoldingRangeParams, FormattingOptions, GotoCapability, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverClientCapabilities, HoverParams, InlayHint,
    InlayHintClientCapabilities, InlayHintParams,
    InlineCompletionClientCapabilities, InlineCompletionParams,
    InlineCompletionResponse, InlineCompletionTriggerKind, Location, MarkupKind,
    MessageActionItemCapabilities, ParameterInformationSettings,
    PartialResultParams, Position, PrepareRenameResponse,
    PublishDiagnosticsClientCapabilities, PublishDiagnosticsParams, Range,
    ReferenceContext, ReferenceParams, RenameParams, SelectionRange,
    SelectionRangeParams, SemanticTokens, SemanticTokensClientCapabilities,
    SemanticTokensParams, ShowMessageRequestClientCapabilities, SignatureHelp,
    SignatureHelpClientCapabilities, SignatureHelpContext, SignatureHelpParams,
    SignatureInformationSettings, SymbolInformation, TextDocumentClientCapabilities,
    TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams,
    TextDocumentSyncClientCapabilities, TextEdit, VersionedTextDocumentIdentifier,
    WindowClientCapabilities, WorkDoneProgressParams, WorkspaceClientCapabilities,
    WorkspaceEdit, WorkspaceSymbolClientCapabilities, WorkspaceSymbolParams,
    request::{
        CallHierarchyIncomingCalls, CallHierarchyPrepare, CodeActionRequest,
        CodeActionResolveRequest, CodeLensRequest, CodeLensResolve, Completion,
        DocumentSymbolRequest, ExecuteCommand, FoldingRangeRequest, Formatting,
        GotoDefinition, GotoImplementation, GotoImplementationResponse,
        GotoTypeDefinition, GotoTypeDefinitionParams, GotoTypeDefinitionResponse,
        HoverRequest, InlayHintRequest, InlineCompletionRequest,
        PrepareRenameRequest, References, Rename, Request, ResolveCompletionItem,
        SelectionRangeRequest, SemanticTokensFullRequest, SignatureHelpRequest,
        WorkspaceSymbolRequest,
    },
};
use parking_lot::Mutex;
use ropey::Rope;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use url::Url;

use self::{
    catalog::PluginCatalog,
    psp::{ClonableCallback, RpcCallback},
};
use crate::buffer::language_id_from_path;

pub type PluginName = String;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DiagnosticSource {
    Pushed,
    Pulled,
}

#[allow(clippy::large_enum_variant)]
pub enum PluginCatalogRpc {
    PublishDiagnostics {
        plugin_id: PluginId,
        source: DiagnosticSource,
        diagnostics: PublishDiagnosticsParams,
    },
    RefreshDiagnostics {
        plugin_id: PluginId,
    },
    ServerRequest {
        plugin_id: Option<PluginId>,
        request_sent: Option<Arc<AtomicUsize>>,
        method: Cow<'static, str>,
        params: Value,
        language_id: Option<String>,
        path: Option<PathBuf>,
        check: bool,
        f: Box<dyn ClonableCallback<Value, RpcError>>,
    },
    ServerNotification {
        plugin_id: Option<PluginId>,
        method: Cow<'static, str>,
        params: Value,
        language_id: Option<String>,
        path: Option<PathBuf>,
        check: bool,
    },
    FormatSemanticTokens {
        plugin_id: PluginId,
        tokens: SemanticTokens,
        text: Rope,
        f: Box<dyn RpcCallback<Vec<LineStyle>, RpcError>>,
    },
    DapVariable {
        dap_id: DapId,
        reference: usize,
        f: Box<dyn RpcCallback<Vec<dap_types::Variable>, RpcError>>,
    },
    DapGetScopes {
        dap_id: DapId,
        frame_id: usize,
        f: Box<
            dyn RpcCallback<
                    Vec<(dap_types::Scope, Vec<dap_types::Variable>)>,
                    RpcError,
                >,
        >,
    },
    DidOpenTextDocument {
        document: TextDocumentItem,
    },
    DidCloseTextDocument {
        path: PathBuf,
    },
    DidChangeTextDocument {
        language_id: String,
        document: VersionedTextDocumentIdentifier,
        delta: AheadDelta,
        text: Rope,
        new_text: Rope,
    },
    DidSaveTextDocument {
        language_id: String,
        path: PathBuf,
        text_document: TextDocumentIdentifier,
        text: Rope,
    },
    UninstallLanguageExtension {
        extension_id: String,
        f: Box<dyn RpcCallback<(), RpcError>>,
    },
    Handler(PluginCatalogNotification),
    Shutdown,
}

#[allow(clippy::large_enum_variant)]
pub enum PluginCatalogNotification {
    RegisterWatchedFiles {
        plugin_id: PluginId,
        id: String,
        registration: watched_files::WatchedFileRegistration,
    },
    UnregisterWatchedFiles {
        plugin_id: PluginId,
        id: String,
    },
    WatchedFilesChanged(notify::Event),
    RestartLanguageServers {
        documents: Vec<TextDocumentItem>,
        extension_id: Option<String>,
    },
    RepairBuiltinNpmServer {
        server_id: String,
    },
    RepairBuiltinNpmServerFinished {
        server_id: String,
        result: Result<(), String>,
    },
    BuiltinNpmServerUpdateAvailable {
        server_id: String,
        latest_version: String,
    },
    LanguageServerResolved {
        language_id: String,
        extension: LanguageServerExtension,
        generation: u64,
        result: Result<LanguageServerCommand, String>,
    },
    BuiltinServerResolved {
        language_id: String,
        server_name: &'static str,
        generation: u64,
        result: Result<LanguageServerCommand, String>,
    },
    RefreshWorkspaceConfigurations,
    ExtensionConfigurationResolved {
        plugin_id: PluginId,
        generation: u64,
        result: Result<LanguageServerCommand, String>,
    },
    LanguageServerStopped {
        plugin_id: PluginId,
        message: String,
    },
    LanguageServerStatus {
        plugin_id: PluginId,
        params: ahead_rpc::core::ServerStatusParams,
    },
    DapStart {
        config: RunDebugConfig,
        breakpoints: HashMap<PathBuf, Vec<SourceBreakpoint>>,
    },
    DapTerminalResponse {
        response: ahead_rpc::dap_types::DebugTerminalResponse,
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
    Shutdown,
}

#[derive(Clone)]
pub struct PluginCatalogRpcHandler {
    core_rpc: CoreRpcHandler,
    plugin_tx: Sender<PluginCatalogRpc>,
    plugin_rx: Arc<Mutex<Option<Receiver<PluginCatalogRpc>>>>,
    #[allow(dead_code)]
    id: Arc<AtomicU64>,
    #[allow(dead_code, clippy::type_complexity)]
    pending: Arc<Mutex<HashMap<u64, Sender<Result<Value, RpcError>>>>>,
}

impl PluginCatalogRpcHandler {
    pub fn new(core_rpc: CoreRpcHandler) -> Self {
        let (plugin_tx, plugin_rx) = crossbeam_channel::unbounded();
        Self {
            core_rpc,
            plugin_tx,
            plugin_rx: Arc::new(Mutex::new(Some(plugin_rx))),
            id: Arc::new(AtomicU64::new(0)),
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[allow(dead_code)]
    fn handle_response(&self, id: RequestId, result: Result<Value, RpcError>) {
        if let Some(chan) = { self.pending.lock().remove(&id) } {
            if let Err(err) = chan.send(result) {
                tracing::error!("{:?}", err);
            }
        }
    }

    pub fn mainloop(&self, plugin: &mut PluginCatalog) {
        let plugin_rx = self.plugin_rx.lock().take().unwrap();
        for msg in plugin_rx {
            match msg {
                PluginCatalogRpc::PublishDiagnostics {
                    plugin_id,
                    source,
                    diagnostics,
                } => plugin.publish_diagnostics(plugin_id, source, diagnostics),
                PluginCatalogRpc::RefreshDiagnostics { plugin_id } => {
                    plugin.refresh_diagnostics(plugin_id);
                }
                PluginCatalogRpc::ServerRequest {
                    plugin_id,
                    request_sent,
                    method,
                    params,
                    language_id,
                    path,
                    check,
                    f,
                } => {
                    plugin.handle_server_request(
                        plugin_id,
                        request_sent,
                        method,
                        params,
                        language_id,
                        path,
                        check,
                        f,
                    );
                }
                PluginCatalogRpc::ServerNotification {
                    plugin_id,
                    method,
                    params,
                    language_id,
                    path,
                    check,
                } => {
                    plugin.handle_server_notification(
                        plugin_id,
                        method,
                        params,
                        language_id,
                        path,
                        check,
                    );
                }
                PluginCatalogRpc::Handler(notification) => {
                    plugin.handle_notification(notification);
                }
                PluginCatalogRpc::FormatSemanticTokens {
                    plugin_id,
                    tokens,
                    text,
                    f,
                } => {
                    plugin.format_semantic_tokens(plugin_id, tokens, text, f);
                }
                PluginCatalogRpc::DidOpenTextDocument { document } => {
                    plugin.handle_did_open_text_document(document);
                }
                PluginCatalogRpc::DidCloseTextDocument { path } => {
                    plugin.handle_did_close_text_document(path);
                }
                PluginCatalogRpc::DidSaveTextDocument {
                    language_id,
                    path,
                    text_document,
                    text,
                } => {
                    plugin.handle_did_save_text_document(
                        language_id,
                        path,
                        text_document,
                        text,
                    );
                }
                PluginCatalogRpc::UninstallLanguageExtension { extension_id, f } => {
                    plugin.uninstall_language_extension(extension_id, f)
                }
                PluginCatalogRpc::DidChangeTextDocument {
                    language_id,
                    document,
                    delta,
                    text,
                    new_text,
                } => {
                    plugin.handle_did_change_text_document(
                        language_id,
                        document,
                        delta,
                        text,
                        new_text,
                    );
                }
                PluginCatalogRpc::DapVariable {
                    dap_id,
                    reference,
                    f,
                } => {
                    plugin.dap_variable(dap_id, reference, f);
                }
                PluginCatalogRpc::DapGetScopes {
                    dap_id,
                    frame_id,
                    f,
                } => {
                    plugin.dap_get_scopes(dap_id, frame_id, f);
                }
                PluginCatalogRpc::Shutdown => {
                    return;
                }
            }
        }
    }

    pub fn shutdown(&self) {
        if let Err(err) =
            self.catalog_notification(PluginCatalogNotification::Shutdown)
        {
            tracing::error!("{:?}", err);
        }
        if let Err(err) = self.plugin_tx.send(PluginCatalogRpc::Shutdown) {
            tracing::error!("{:?}", err);
        }
    }

    pub fn language_server_status(
        &self,
        plugin_id: PluginId,
        params: ahead_rpc::core::ServerStatusParams,
    ) {
        if let Err(error) = self.catalog_notification(
            PluginCatalogNotification::LanguageServerStatus { plugin_id, params },
        ) {
            tracing::error!(?error, "reporting language server status");
        }
    }

    pub fn refresh_diagnostics(&self, plugin_id: PluginId) {
        if let Err(error) = self
            .plugin_tx
            .send(PluginCatalogRpc::RefreshDiagnostics { plugin_id })
        {
            tracing::error!(?error, "requesting document diagnostics");
        }
    }

    pub fn publish_diagnostics(
        &self,
        plugin_id: PluginId,
        source: DiagnosticSource,
        diagnostics: PublishDiagnosticsParams,
    ) {
        if let Err(error) =
            self.plugin_tx.send(PluginCatalogRpc::PublishDiagnostics {
                plugin_id,
                source,
                diagnostics,
            })
        {
            tracing::error!(?error, "publishing language-server diagnostics");
        }
    }

    pub(crate) fn catalog_notification(
        &self,
        notification: PluginCatalogNotification,
    ) -> Result<()> {
        self.plugin_tx
            .send(PluginCatalogRpc::Handler(notification))
            .map_err(|e| anyhow!(e.to_string()))?;
        Ok(())
    }

    pub(crate) fn uninstall_language_extension(
        &self,
        extension_id: String,
        f: Box<dyn RpcCallback<(), RpcError>>,
    ) -> Result<()> {
        self.plugin_tx
            .send(PluginCatalogRpc::UninstallLanguageExtension { extension_id, f })
            .map_err(|error| anyhow!(error.to_string()))
    }

    #[cfg(test)]
    pub(crate) fn test_receiver(&self) -> Receiver<PluginCatalogRpc> {
        self.plugin_rx
            .lock()
            .as_ref()
            .expect("catalog receiver")
            .clone()
    }

    fn send_request_to_first_capable_plugin<P, Resp>(
        &self,
        method: &'static str,
        params: P,
        language_id: Option<String>,
        path: Option<PathBuf>,
        cb: impl FnOnce(PluginId, Result<Resp, RpcError>) + Clone + Send + 'static,
    ) where
        P: Serialize,
        Resp: DeserializeOwned,
    {
        self.send_request(
            None,
            None,
            method,
            params,
            language_id,
            path,
            true,
            move |plugin_id, result| {
                cb(
                    plugin_id,
                    result.and_then(|value| {
                        serde_json::from_value::<Resp>(value).map_err(|error| {
                            RpcError {
                                code: -32603,
                                message: format!(
                                    "Invalid language-server response: {error}"
                                ),
                            }
                        })
                    }),
                );
            },
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn send_request<P: Serialize>(
        &self,
        plugin_id: Option<PluginId>,
        request_sent: Option<Arc<AtomicUsize>>,
        method: impl Into<Cow<'static, str>>,
        params: P,
        language_id: Option<String>,
        path: Option<PathBuf>,
        check: bool,
        f: impl FnOnce(PluginId, Result<Value, RpcError>) + Send + DynClone + 'static,
    ) {
        let params = serde_json::to_value(params).unwrap();
        let rpc = PluginCatalogRpc::ServerRequest {
            plugin_id,
            request_sent,
            method: method.into(),
            params,
            language_id,
            path,
            check,
            f: Box::new(f),
        };
        if let Err(err) = self.plugin_tx.send(rpc) {
            tracing::error!("{:?}", err);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn send_notification<P: Serialize>(
        &self,
        plugin_id: Option<PluginId>,
        method: impl Into<Cow<'static, str>>,
        params: P,
        language_id: Option<String>,
        path: Option<PathBuf>,
        check: bool,
    ) {
        let params = serde_json::to_value(params).unwrap();
        let rpc = PluginCatalogRpc::ServerNotification {
            plugin_id,
            method: method.into(),
            params,
            language_id,
            path,
            check,
        };
        if let Err(err) = self.plugin_tx.send(rpc) {
            tracing::error!("{:?}", err);
        }
    }

    pub fn format_semantic_tokens(
        &self,
        plugin_id: PluginId,
        tokens: SemanticTokens,
        text: Rope,
        f: Box<dyn RpcCallback<Vec<LineStyle>, RpcError>>,
    ) {
        if let Err(err) =
            self.plugin_tx.send(PluginCatalogRpc::FormatSemanticTokens {
                plugin_id,
                tokens,
                text,
                f,
            })
        {
            tracing::error!("{:?}", err);
        }
    }

    pub fn did_save_text_document(&self, path: &Path, text: Rope) {
        let text_document =
            TextDocumentIdentifier::new(Url::from_file_path(path).unwrap());
        let language_id = language_id_from_path(path).unwrap_or_default();
        if let Err(err) =
            self.plugin_tx.send(PluginCatalogRpc::DidSaveTextDocument {
                language_id,
                text_document,
                path: path.into(),
                text,
            })
        {
            tracing::error!("{:?}", err);
        }
    }

    pub fn did_change_text_document(
        &self,
        path: &Path,
        rev: u64,
        delta: AheadDelta,
        text: Rope,
        new_text: Rope,
    ) {
        let document = VersionedTextDocumentIdentifier::new(
            Url::from_file_path(path).unwrap(),
            rev as i32,
        );
        let language_id = language_id_from_path(path).unwrap_or_default();
        if let Err(err) =
            self.plugin_tx
                .send(PluginCatalogRpc::DidChangeTextDocument {
                    language_id,
                    document,
                    delta,
                    text,
                    new_text,
                })
        {
            tracing::error!("{:?}", err);
        }
    }

    pub fn get_definition(
        &self,
        path: &Path,
        position: Position,
        cb: impl FnOnce(PluginId, Result<GotoDefinitionResponse, RpcError>)
        + Clone
        + Send
        + 'static,
    ) {
        let uri = Url::from_file_path(path).unwrap();
        let method = GotoDefinition::METHOD;
        let params = GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };

        let language_id = Some(language_id_from_path(path).unwrap_or_default());
        self.send_request_to_first_capable_plugin(
            method,
            params,
            language_id,
            Some(path.to_path_buf()),
            cb,
        );
    }

    pub fn get_type_definition(
        &self,
        path: &Path,
        position: Position,
        cb: impl FnOnce(PluginId, Result<GotoTypeDefinitionResponse, RpcError>)
        + Clone
        + Send
        + 'static,
    ) {
        let uri = Url::from_file_path(path).unwrap();
        let method = GotoTypeDefinition::METHOD;
        let params = GotoTypeDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };

        let language_id = Some(language_id_from_path(path).unwrap_or_default());
        self.send_request_to_first_capable_plugin(
            method,
            params,
            language_id,
            Some(path.to_path_buf()),
            cb,
        );
    }

    pub fn call_hierarchy_incoming(
        &self,
        path: &Path,
        item: CallHierarchyItem,
        cb: impl FnOnce(
            PluginId,
            Result<Option<Vec<CallHierarchyIncomingCall>>, RpcError>,
        ) + Clone
        + Send
        + 'static,
    ) {
        let method = CallHierarchyIncomingCalls::METHOD;
        let params = CallHierarchyIncomingCallsParams {
            item,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: Default::default(),
        };

        let language_id = Some(language_id_from_path(path).unwrap_or_default());
        self.send_request_to_first_capable_plugin(
            method,
            params,
            language_id,
            Some(path.to_path_buf()),
            cb,
        );
    }

    pub fn show_call_hierarchy(
        &self,
        path: &Path,
        position: Position,
        cb: impl FnOnce(PluginId, Result<Option<Vec<CallHierarchyItem>>, RpcError>)
        + Clone
        + Send
        + 'static,
    ) {
        let uri = Url::from_file_path(path).unwrap();
        let method = CallHierarchyPrepare::METHOD;
        let params = CallHierarchyPrepareParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        };

        let language_id = Some(language_id_from_path(path).unwrap_or_default());
        self.send_request_to_first_capable_plugin(
            method,
            params,
            language_id,
            Some(path.to_path_buf()),
            cb,
        );
    }

    pub fn get_references(
        &self,
        path: &Path,
        position: Position,
        cb: impl FnOnce(PluginId, Result<Vec<Location>, RpcError>)
        + Clone
        + Send
        + 'static,
    ) {
        let uri = Url::from_file_path(path).unwrap();
        let method = References::METHOD;
        let params = ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: ReferenceContext {
                include_declaration: true,
            },
        };

        let language_id = Some(language_id_from_path(path).unwrap_or_default());
        self.send_request_to_first_capable_plugin(
            method,
            params,
            language_id,
            Some(path.to_path_buf()),
            cb,
        );
    }

    pub fn get_lsp_folding_range(
        &self,
        path: &Path,
        cb: impl FnOnce(
            PluginId,
            std::result::Result<Option<Vec<FoldingRange>>, RpcError>,
        ) + Clone
        + Send
        + 'static,
    ) {
        let uri = Url::from_file_path(path).unwrap();
        let method = FoldingRangeRequest::METHOD;
        let params = FoldingRangeParams {
            text_document: TextDocumentIdentifier { uri },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };

        let language_id = Some(language_id_from_path(path).unwrap_or_default());
        self.send_request_to_first_capable_plugin(
            method,
            params,
            language_id,
            Some(path.to_path_buf()),
            cb,
        );
    }

    pub fn go_to_implementation(
        &self,
        path: &Path,
        position: Position,
        cb: impl FnOnce(PluginId, Result<Option<GotoImplementationResponse>, RpcError>)
        + Clone
        + Send
        + 'static,
    ) {
        let uri = Url::from_file_path(path).unwrap();
        let method = GotoImplementation::METHOD;
        let params = GotoTypeDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };

        let language_id = Some(language_id_from_path(path).unwrap_or_default());
        self.send_request_to_first_capable_plugin(
            method,
            params,
            language_id,
            Some(path.to_path_buf()),
            cb,
        );
    }

    pub fn get_code_actions(
        &self,
        path: &Path,
        position: Position,
        diagnostics: Vec<Diagnostic>,
        cb: impl FnOnce(Result<Vec<LspCodeAction>, RpcError>) + Send + 'static,
    ) {
        let uri = Url::from_file_path(path).unwrap();
        let method = CodeActionRequest::METHOD;
        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range {
                start: position,
                end: position,
            },
            context: CodeActionContext {
                diagnostics,
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        let language_id = Some(language_id_from_path(path).unwrap_or_default());
        struct Replies<Callback> {
            callback: Option<Callback>,
            results: Vec<(PluginId, Result<Option<CodeActionResponse>, RpcError>)>,
        }
        let request_sent = Arc::new(AtomicUsize::new(0));
        let replies = Arc::new(Mutex::new(Replies {
            callback: Some(cb),
            results: Vec::new(),
        }));
        self.send_request(
            None,
            Some(request_sent.clone()),
            method,
            params,
            language_id,
            Some(path.to_path_buf()),
            true,
            move |plugin_id, result| {
                let result = result.and_then(|value| {
                    serde_json::from_value::<Option<CodeActionResponse>>(value)
                        .map_err(|error| RpcError {
                            code: -32603,
                            message: format!("Invalid language-server code actions: {error}"),
                        })
                });
                let (callback, mut results) = {
                    let mut replies = replies.lock();
                    replies.results.push((plugin_id, result));
                    if replies.results.len() != request_sent.load(Ordering::Acquire) {
                        return;
                    }
                    let Some(callback) = replies.callback.take() else {
                        return;
                    };
                    (callback, std::mem::take(&mut replies.results))
                };
                results.sort_by_key(|(plugin_id, _)| plugin_id.0);
                let mut actions = Vec::new();
                let mut successful = false;
                let mut last_error = None;
                for (plugin_id, result) in results {
                    match result {
                        Ok(response) => {
                            successful = true;
                            actions.extend(response.into_iter().flatten().map(|action| {
                                LspCodeAction { plugin_id, action }
                            }));
                        }
                        Err(error) => {
                            tracing::debug!(?plugin_id, message = %error.message, "language-server code-action request failed");
                            last_error = Some(error);
                        }
                    }
                }
                if successful {
                    callback(Ok(actions));
                } else {
                    callback(Err(last_error.unwrap_or_else(|| RpcError {
                        code: -32603,
                        message: "No language server returned code actions".into(),
                    })));
                }
            },
        );
    }

    pub fn get_code_lens(
        &self,
        path: &Path,
        cb: impl FnOnce(PluginId, Result<Option<Vec<CodeLens>>, RpcError>)
        + Clone
        + Send
        + 'static,
    ) {
        let uri = Url::from_file_path(path).unwrap();
        let method = CodeLensRequest::METHOD;
        let params = CodeLensParams {
            text_document: TextDocumentIdentifier { uri },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let language_id = Some(language_id_from_path(path).unwrap_or_default());

        self.send_request_to_first_capable_plugin(
            method,
            params,
            language_id,
            Some(path.to_path_buf()),
            cb,
        );
    }

    pub fn get_code_lens_resolve(
        &self,
        path: &Path,
        code_lens: &CodeLens,
        cb: impl FnOnce(PluginId, Result<CodeLens, RpcError>) + Clone + Send + 'static,
    ) {
        let method = CodeLensResolve::METHOD;
        let language_id = Some(language_id_from_path(path).unwrap_or_default());

        self.send_request_to_first_capable_plugin(
            method,
            code_lens,
            language_id,
            Some(path.to_path_buf()),
            cb,
        );
    }

    pub fn get_inlay_hints(
        &self,
        path: &Path,
        range: Range,
        cb: impl FnOnce(PluginId, Result<Vec<InlayHint>, RpcError>)
        + Clone
        + Send
        + 'static,
    ) {
        let uri = Url::from_file_path(path).unwrap();
        let method = InlayHintRequest::METHOD;
        let params = InlayHintParams {
            text_document: TextDocumentIdentifier { uri },
            work_done_progress_params: WorkDoneProgressParams::default(),
            range,
        };
        let language_id = Some(language_id_from_path(path).unwrap_or_default());
        self.send_request_to_first_capable_plugin(
            method,
            params,
            language_id,
            Some(path.to_path_buf()),
            cb,
        );
    }

    pub fn get_inline_completions(
        &self,
        path: &Path,
        position: Position,
        trigger_kind: InlineCompletionTriggerKind,
        cb: impl FnOnce(PluginId, Result<InlineCompletionResponse, RpcError>)
        + Clone
        + Send
        + 'static,
    ) {
        let uri = Url::from_file_path(path).unwrap();
        let method = InlineCompletionRequest::METHOD;
        let params = InlineCompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            context: lsp_types::InlineCompletionContext {
                trigger_kind,
                selected_completion_info: None,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        let language_id = Some(language_id_from_path(path).unwrap_or_default());
        self.send_request_to_first_capable_plugin(
            method,
            params,
            language_id,
            Some(path.to_path_buf()),
            cb,
        );
    }

    pub fn get_document_symbols(
        &self,
        path: &Path,
        cb: impl FnOnce(PluginId, Result<DocumentSymbolResponse, RpcError>)
        + Clone
        + Send
        + 'static,
    ) {
        let uri = Url::from_file_path(path).unwrap();
        let method = DocumentSymbolRequest::METHOD;
        let params = DocumentSymbolParams {
            text_document: TextDocumentIdentifier { uri },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        let language_id = Some(language_id_from_path(path).unwrap_or_default());
        self.send_request_to_first_capable_plugin(
            method,
            params,
            language_id,
            Some(path.to_path_buf()),
            cb,
        );
    }

    pub fn get_workspace_symbols(
        &self,
        query: String,
        cb: impl FnOnce(PluginId, Result<Vec<SymbolInformation>, RpcError>)
        + Clone
        + Send
        + 'static,
    ) {
        let method = WorkspaceSymbolRequest::METHOD;
        let params = WorkspaceSymbolParams {
            query,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        self.send_request_to_first_capable_plugin(method, params, None, None, cb);
    }

    pub fn get_document_formatting(
        &self,
        path: &Path,
        cb: impl FnOnce(PluginId, Result<Vec<TextEdit>, RpcError>)
        + Clone
        + Send
        + 'static,
    ) {
        let uri = Url::from_file_path(path).unwrap();
        let method = Formatting::METHOD;
        let params = DocumentFormattingParams {
            text_document: TextDocumentIdentifier { uri },
            options: FormattingOptions {
                tab_size: 4,
                insert_spaces: true,
                ..Default::default()
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        let language_id = Some(language_id_from_path(path).unwrap_or_default());
        self.send_request_to_first_capable_plugin(
            method,
            params,
            language_id,
            Some(path.to_path_buf()),
            move |plugin_id, result: Result<Option<Vec<TextEdit>>, RpcError>| {
                cb(plugin_id, result.map(Option::unwrap_or_default));
            },
        );
    }

    pub fn prepare_rename(
        &self,
        path: &Path,
        position: Position,
        cb: impl FnOnce(PluginId, Result<PrepareRenameResponse, RpcError>)
        + Clone
        + Send
        + 'static,
    ) {
        let uri = Url::from_file_path(path).unwrap();
        let method = PrepareRenameRequest::METHOD;
        let params = TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri },
            position,
        };
        let language_id = Some(language_id_from_path(path).unwrap_or_default());
        self.send_request_to_first_capable_plugin(
            method,
            params,
            language_id,
            Some(path.to_path_buf()),
            cb,
        );
    }

    pub fn rename(
        &self,
        path: &Path,
        position: Position,
        new_name: String,
        cb: impl FnOnce(PluginId, Result<WorkspaceEdit, RpcError>)
        + Clone
        + Send
        + 'static,
    ) {
        let uri = Url::from_file_path(path).unwrap();
        let method = Rename::METHOD;
        let params = RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            new_name,
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        let language_id = Some(language_id_from_path(path).unwrap_or_default());
        self.send_request_to_first_capable_plugin(
            method,
            params,
            language_id,
            Some(path.to_path_buf()),
            cb,
        );
    }

    pub fn get_semantic_tokens(
        &self,
        path: &Path,
        cb: impl FnOnce(PluginId, Result<SemanticTokens, RpcError>)
        + Clone
        + Send
        + 'static,
    ) {
        let uri = Url::from_file_path(path).unwrap();
        let method = SemanticTokensFullRequest::METHOD;
        let params = SemanticTokensParams {
            text_document: TextDocumentIdentifier { uri },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        };
        let language_id = Some(language_id_from_path(path).unwrap_or_default());
        self.send_request_to_first_capable_plugin(
            method,
            params,
            language_id,
            Some(path.to_path_buf()),
            cb,
        );
    }

    pub fn get_selection_range(
        &self,
        path: &Path,
        positions: Vec<Position>,
        cb: impl FnOnce(PluginId, Result<Vec<SelectionRange>, RpcError>)
        + Clone
        + Send
        + 'static,
    ) {
        let uri = Url::from_file_path(path).unwrap();
        let method = SelectionRangeRequest::METHOD;
        let params = SelectionRangeParams {
            text_document: TextDocumentIdentifier { uri },
            positions,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: Default::default(),
        };
        let language_id = Some(language_id_from_path(path).unwrap_or_default());
        self.send_request_to_first_capable_plugin(
            method,
            params,
            language_id,
            Some(path.to_path_buf()),
            cb,
        );
    }

    pub fn hover(
        &self,
        path: &Path,
        position: Position,
        cb: impl FnOnce(PluginId, Result<Hover, RpcError>) + Clone + Send + 'static,
    ) {
        let uri = Url::from_file_path(path).unwrap();
        let method = HoverRequest::METHOD;
        let params = HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        let language_id = Some(language_id_from_path(path).unwrap_or_default());

        self.send_request_to_first_capable_plugin(
            method,
            params,
            language_id,
            Some(path.to_path_buf()),
            cb,
        );
    }

    pub fn completion(
        &self,
        request_id: usize,
        path: &Path,
        input: String,
        position: Position,
    ) {
        let uri = Url::from_file_path(path).unwrap();
        let method = Completion::METHOD;
        let params = CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: Some(CompletionContext {
                trigger_kind: CompletionTriggerKind::INVOKED,
                trigger_character: None,
            }),
        };

        let core_rpc = self.core_rpc.clone();
        let language_id = Some(language_id_from_path(path).unwrap_or_default());

        self.send_request_to_first_capable_plugin(
            method,
            params,
            language_id,
            Some(path.to_path_buf()),
            move |plugin_id, result: Result<Value, RpcError>| {
                let response = result
                    .and_then(|value| {
                        if value.is_null() {
                            Ok(None)
                        } else {
                            serde_json::from_value::<CompletionResponse>(value)
                                .map(Some)
                                .map_err(|error| RpcError {
                                    code: -32603,
                                    message: error.to_string(),
                                })
                        }
                    })
                    .unwrap_or_else(|error| {
                        tracing::debug!(?error, "completion request failed");
                        Some(CompletionResponse::Array(Vec::new()))
                    });
                core_rpc.completion_response(request_id, input, response, plugin_id);
            },
        );
    }

    pub fn completion_resolve(
        &self,
        plugin_id: PluginId,
        item: CompletionItem,
        cb: impl FnOnce(Result<CompletionItem, RpcError>) + Send + Clone + 'static,
    ) {
        let method = ResolveCompletionItem::METHOD;
        self.send_request(
            Some(plugin_id),
            None,
            method,
            item,
            None,
            None,
            true,
            move |_, result| {
                let result = match result {
                    Ok(value) => {
                        if let Ok(item) =
                            serde_json::from_value::<CompletionItem>(value)
                        {
                            Ok(item)
                        } else {
                            Err(RpcError {
                                code: 0,
                                message: "completion item deserialize error"
                                    .to_string(),
                            })
                        }
                    }
                    Err(e) => Err(e),
                };
                cb(result)
            },
        );
    }

    pub fn signature_help(
        &self,
        request_id: usize,
        path: &Path,
        position: Position,
        context: SignatureHelpContext,
    ) {
        let uri = Url::from_file_path(path).unwrap();
        let method = SignatureHelpRequest::METHOD;
        let params = SignatureHelpParams {
            context: Some(context),
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        };

        let core_rpc = self.core_rpc.clone();
        let language_id = Some(language_id_from_path(path).unwrap_or_default());
        self.send_request_to_first_capable_plugin(
            method,
            params,
            language_id,
            Some(path.to_path_buf()),
            move |plugin_id, result| {
                let response = result.unwrap_or_else(|error| {
                    tracing::error!("Signature help request failed: {error:?}");
                    SignatureHelp {
                        signatures: Vec::new(),
                        active_signature: None,
                        active_parameter: None,
                    }
                });
                core_rpc.signature_help_response(request_id, response, plugin_id);
            },
        );
    }

    pub fn action_resolve(
        &self,
        item: CodeAction,
        plugin_id: PluginId,
        cb: impl FnOnce(Result<CodeAction, RpcError>) + Send + Clone + 'static,
    ) {
        let method = CodeActionResolveRequest::METHOD;
        self.send_request(
            Some(plugin_id),
            None,
            method,
            item,
            None,
            None,
            true,
            move |_, result| {
                let result = match result {
                    Ok(value) => {
                        if let Ok(item) = serde_json::from_value::<CodeAction>(value)
                        {
                            Ok(item)
                        } else {
                            Err(RpcError {
                                code: 0,
                                message: "code_action item deserialize error"
                                    .to_string(),
                            })
                        }
                    }
                    Err(e) => Err(e),
                };
                cb(result)
            },
        );
    }

    pub fn execute_command(
        &self,
        plugin_id: PluginId,
        command: String,
        arguments: Vec<Value>,
        cb: impl FnOnce(Result<Value, RpcError>) + Clone + Send + 'static,
    ) {
        self.send_request(
            Some(plugin_id),
            None,
            ExecuteCommand::METHOD,
            ExecuteCommandParams {
                command,
                arguments,
                ..Default::default()
            },
            None,
            None,
            true,
            move |_, result| cb(result),
        );
    }

    pub fn did_open_document(
        &self,
        path: &Path,
        language_id: String,
        version: i32,
        text: String,
    ) {
        match Url::from_file_path(path) {
            Ok(path) => {
                if let Err(err) =
                    self.plugin_tx.send(PluginCatalogRpc::DidOpenTextDocument {
                        document: TextDocumentItem::new(
                            path,
                            language_id,
                            version,
                            text,
                        ),
                    })
                {
                    tracing::error!("{:?}", err);
                }
            }
            Err(_) => {
                tracing::error!("Failed to parse URL from file path: {path:?}");
            }
        }
    }

    pub fn did_close_document(&self, path: PathBuf) {
        if let Err(error) = self
            .plugin_tx
            .send(PluginCatalogRpc::DidCloseTextDocument { path })
        {
            tracing::error!(?error, "closing language-server document");
        }
    }

    pub fn dap_start(
        &self,
        config: RunDebugConfig,
        breakpoints: HashMap<PathBuf, Vec<SourceBreakpoint>>,
    ) -> Result<()> {
        self.catalog_notification(PluginCatalogNotification::DapStart {
            config,
            breakpoints,
        })
    }

    pub fn dap_terminal_response(
        &self,
        response: ahead_rpc::dap_types::DebugTerminalResponse,
    ) -> Result<()> {
        self.catalog_notification(PluginCatalogNotification::DapTerminalResponse {
            response,
        })
    }

    pub fn dap_continue(&self, dap_id: DapId, thread_id: ThreadId) -> Result<()> {
        self.catalog_notification(PluginCatalogNotification::DapContinue {
            dap_id,
            thread_id,
        })
    }

    pub fn dap_pause(&self, dap_id: DapId, thread_id: ThreadId) -> Result<()> {
        self.catalog_notification(PluginCatalogNotification::DapPause {
            dap_id,
            thread_id,
        })
    }

    pub fn dap_step_over(&self, dap_id: DapId, thread_id: ThreadId) -> Result<()> {
        self.catalog_notification(PluginCatalogNotification::DapStepOver {
            dap_id,
            thread_id,
        })
    }

    pub fn dap_step_into(&self, dap_id: DapId, thread_id: ThreadId) -> Result<()> {
        self.catalog_notification(PluginCatalogNotification::DapStepInto {
            dap_id,
            thread_id,
        })
    }

    pub fn dap_step_out(&self, dap_id: DapId, thread_id: ThreadId) -> Result<()> {
        self.catalog_notification(PluginCatalogNotification::DapStepOut {
            dap_id,
            thread_id,
        })
    }

    pub fn dap_stop(&self, dap_id: DapId) -> Result<()> {
        self.catalog_notification(PluginCatalogNotification::DapStop { dap_id })
    }

    pub fn dap_disconnect(&self, dap_id: DapId) -> Result<()> {
        self.catalog_notification(PluginCatalogNotification::DapDisconnect {
            dap_id,
        })
    }

    pub fn dap_restart(
        &self,
        dap_id: DapId,
        breakpoints: HashMap<PathBuf, Vec<SourceBreakpoint>>,
    ) -> Result<()> {
        self.catalog_notification(PluginCatalogNotification::DapRestart {
            dap_id,
            breakpoints,
        })
    }

    pub fn dap_set_breakpoints(
        &self,
        dap_id: DapId,
        path: PathBuf,
        breakpoints: Vec<SourceBreakpoint>,
    ) -> Result<()> {
        self.catalog_notification(PluginCatalogNotification::DapSetBreakpoints {
            dap_id,
            path,
            breakpoints,
        })
    }

    pub fn dap_variable(
        &self,
        dap_id: DapId,
        reference: usize,
        f: impl FnOnce(Result<Vec<dap_types::Variable>, RpcError>) + Send + 'static,
    ) {
        if let Err(err) = self.plugin_tx.send(PluginCatalogRpc::DapVariable {
            dap_id,
            reference,
            f: Box::new(f),
        }) {
            tracing::error!("{:?}", err);
        }
    }

    pub fn dap_get_scopes(
        &self,
        dap_id: DapId,
        frame_id: usize,
        f: impl FnOnce(
            Result<Vec<(dap_types::Scope, Vec<dap_types::Variable>)>, RpcError>,
        ) + Send
        + 'static,
    ) {
        if let Err(err) = self.plugin_tx.send(PluginCatalogRpc::DapGetScopes {
            dap_id,
            frame_id,
            f: Box::new(f),
        }) {
            tracing::error!("{:?}", err);
        }
    }
}

fn client_capabilities() -> ClientCapabilities {
    // https://github.com/rust-lang/rust-analyzer/blob/master/docs/dev/lsp-extensions.md#server-status
    let mut experimental = Map::new();
    experimental.insert("serverStatusNotification".into(), true.into());
    let command_vec = ["rust-analyzer.runSingle", "rust-analyzer.debugSingle"]
        .map(Value::from)
        .to_vec();

    let mut commands = Map::new();
    experimental.insert("serverStatusNotification".into(), true.into());
    commands.insert("commands".into(), command_vec.into());
    experimental.insert("commands".into(), commands.into());
    ClientCapabilities {
        text_document: Some(TextDocumentClientCapabilities {
            synchronization: Some(TextDocumentSyncClientCapabilities {
                did_save: Some(true),
                dynamic_registration: Some(true),
                ..Default::default()
            }),
            completion: Some(CompletionClientCapabilities {
                completion_item: Some(CompletionItemCapability {
                    snippet_support: Some(true),
                    resolve_support: Some(CompletionItemCapabilityResolveSupport {
                        properties: vec!["additionalTextEdits".to_string()],
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            signature_help: Some(SignatureHelpClientCapabilities {
                context_support: Some(true),
                signature_information: Some(SignatureInformationSettings {
                    documentation_format: Some(vec![
                        MarkupKind::Markdown,
                        MarkupKind::PlainText,
                    ]),
                    parameter_information: Some(ParameterInformationSettings {
                        label_offset_support: Some(true),
                    }),
                    active_parameter_support: Some(true),
                }),
                ..Default::default()
            }),
            hover: Some(HoverClientCapabilities {
                content_format: Some(vec![
                    MarkupKind::Markdown,
                    MarkupKind::PlainText,
                ]),
                ..Default::default()
            }),
            inlay_hint: Some(InlayHintClientCapabilities {
                ..Default::default()
            }),
            code_action: Some(CodeActionClientCapabilities {
                data_support: Some(true),
                resolve_support: Some(CodeActionCapabilityResolveSupport {
                    properties: vec!["edit".to_string()],
                }),
                code_action_literal_support: Some(CodeActionLiteralSupport {
                    code_action_kind: CodeActionKindLiteralSupport {
                        value_set: vec![
                            CodeActionKind::EMPTY.as_str().to_string(),
                            CodeActionKind::QUICKFIX.as_str().to_string(),
                            CodeActionKind::REFACTOR.as_str().to_string(),
                            CodeActionKind::REFACTOR_EXTRACT.as_str().to_string(),
                            CodeActionKind::REFACTOR_INLINE.as_str().to_string(),
                            CodeActionKind::REFACTOR_REWRITE.as_str().to_string(),
                            CodeActionKind::SOURCE.as_str().to_string(),
                            CodeActionKind::SOURCE_ORGANIZE_IMPORTS
                                .as_str()
                                .to_string(),
                            "quickassist".to_string(),
                            "source.fixAll".to_string(),
                        ],
                    },
                }),
                ..Default::default()
            }),
            semantic_tokens: Some(SemanticTokensClientCapabilities {
                ..Default::default()
            }),
            type_definition: Some(GotoCapability {
                // Note: This is explicitly specified rather than left to the Default because
                // of a bug in lsp-types https://github.com/gluon-lang/lsp-types/pull/244
                link_support: Some(false),
                ..Default::default()
            }),
            definition: Some(GotoCapability {
                ..Default::default()
            }),
            publish_diagnostics: Some(PublishDiagnosticsClientCapabilities {
                ..Default::default()
            }),
            diagnostic: Some(lsp_types::DiagnosticClientCapabilities {
                dynamic_registration: Some(false),
                related_document_support: Some(false),
            }),
            inline_completion: Some(InlineCompletionClientCapabilities {
                ..Default::default()
            }),
            call_hierarchy: Some(CallHierarchyClientCapabilities {
                dynamic_registration: Some(true),
            }),
            document_symbol: Some(DocumentSymbolClientCapabilities {
                hierarchical_document_symbol_support: Some(true),
                ..Default::default()
            }),
            folding_range: Some(FoldingRangeClientCapabilities {
                dynamic_registration: Some(false),
                range_limit: None,
                line_folding_only: Some(false),
                folding_range_kind: None,
                folding_range: None,
            }),
            ..Default::default()
        }),
        window: Some(WindowClientCapabilities {
            work_done_progress: Some(true),
            show_message: Some(ShowMessageRequestClientCapabilities {
                message_action_item: Some(MessageActionItemCapabilities {
                    additional_properties_support: Some(true),
                }),
            }),
            ..Default::default()
        }),
        workspace: Some(WorkspaceClientCapabilities {
            did_change_watched_files: Some(
                lsp_types::DidChangeWatchedFilesClientCapabilities {
                    dynamic_registration: Some(true),
                    relative_pattern_support: Some(true),
                },
            ),
            symbol: Some(WorkspaceSymbolClientCapabilities {
                ..Default::default()
            }),
            execute_command: Some(lsp_types::ExecuteCommandClientCapabilities {
                dynamic_registration: Some(true),
            }),
            configuration: Some(true),
            workspace_folders: Some(true),
            ..Default::default()
        }),
        experimental: Some(experimental.into()),
        ..Default::default()
    }
}

#[cfg(test)]
mod capability_tests {
    use super::client_capabilities;

    #[test]
    fn code_actions_collect_every_server_and_preserve_opaque_items() {
        use super::{PluginCatalogRpc, PluginCatalogRpcHandler};
        use ahead_rpc::{RpcError, plugin::PluginId};
        use serde_json::json;

        let first = json!({ "title": "Same title", "data": { "server": 1 } });
        let second = json!({
            "title": "Same title", "command": "fixture.command",
            "arguments": [{ "server": 2 }]
        });
        for replies in [
            vec![
                (2, Ok(json!([second.clone()]))),
                (1, Ok(json!([first.clone()]))),
            ],
            vec![
                (3, Ok(serde_json::Value::Null)),
                (2, Ok(json!([second.clone()]))),
                (4, Ok(json!([]))),
                (1, Ok(json!([first.clone()]))),
            ],
            vec![
                (5, Ok(json!(false))),
                (
                    6,
                    Err(RpcError {
                        code: -32097,
                        message: "server stopped".into(),
                    }),
                ),
                (2, Ok(json!([second.clone()]))),
                (1, Ok(json!([first.clone()]))),
            ],
        ] {
            let rpc =
                PluginCatalogRpcHandler::new(ahead_rpc::core::CoreRpcHandler::new());
            let (sender, receiver) = crossbeam_channel::unbounded();
            rpc.get_code_actions(
                &std::env::temp_dir().join("actions.py"),
                lsp_types::Position::new(0, 0),
                Vec::new(),
                move |result| sender.send(result).expect("action result"),
            );
            let PluginCatalogRpc::ServerRequest {
                f,
                request_sent: Some(request_sent),
                ..
            } = rpc.test_receiver().recv().expect("request")
            else {
                panic!("server request expected");
            };
            let count = replies.len();
            request_sent.store(count, std::sync::atomic::Ordering::Release);
            for (index, (id, reply)) in replies.into_iter().enumerate() {
                dyn_clone::clone_box(&*f)(PluginId(id), reply);
                if index + 1 < count {
                    assert!(
                        receiver.try_recv().is_err(),
                        "wait for complementary servers"
                    );
                }
            }
            let actions = receiver
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("reply")
                .expect("healthy servers retain their actions");
            assert_eq!(
                actions
                    .iter()
                    .map(|action| action.plugin_id)
                    .collect::<Vec<_>>(),
                [PluginId(1), PluginId(2)]
            );
            assert_eq!(
                serde_json::to_value(&actions[0].action).expect("action"),
                first
            );
            assert_eq!(
                serde_json::to_value(&actions[1].action).expect("command"),
                second
            );
            assert!(receiver.try_recv().is_err(), "one aggregate callback");
        }
    }

    #[test]
    fn code_actions_propagate_total_failure_but_accept_empty_success() {
        use super::{PluginCatalogRpc, PluginCatalogRpcHandler};
        use ahead_rpc::{RpcError, plugin::PluginId};
        use serde_json::json;

        for (replies, expected_error) in [
            (vec![Ok(serde_json::Value::Null), Ok(json!([]))], None),
            (vec![Ok(json!(false))], Some(-32603)),
            (
                vec![Err(RpcError {
                    code: -32097,
                    message: "server stopped".into(),
                })],
                Some(-32097),
            ),
        ] {
            let rpc =
                PluginCatalogRpcHandler::new(ahead_rpc::core::CoreRpcHandler::new());
            let (sender, receiver) = crossbeam_channel::unbounded();
            rpc.get_code_actions(
                &std::env::temp_dir().join("actions.py"),
                lsp_types::Position::new(0, 0),
                Vec::new(),
                move |result| sender.send(result).expect("action result"),
            );
            let PluginCatalogRpc::ServerRequest {
                f,
                request_sent: Some(request_sent),
                ..
            } = rpc.test_receiver().recv().expect("request")
            else {
                panic!("server request expected");
            };
            request_sent.store(replies.len(), std::sync::atomic::Ordering::Release);
            for (index, reply) in replies.into_iter().enumerate() {
                dyn_clone::clone_box(&*f)(PluginId(index as u64), reply);
            }
            let result = receiver
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("reply");
            match expected_error {
                Some(code) => {
                    assert_eq!(result.expect_err("all servers failed").code, code)
                }
                None => {
                    assert!(result.expect("valid no-actions response").is_empty())
                }
            }
            assert!(receiver.try_recv().is_err());
        }
    }

    #[test]
    fn concurrent_code_actions_deliver_one_complete_response() {
        use super::{PluginCatalogRpc, PluginCatalogRpcHandler};
        use ahead_rpc::plugin::PluginId;
        use std::sync::{
            Arc, Barrier,
            atomic::{AtomicUsize, Ordering},
        };

        let rpc =
            PluginCatalogRpcHandler::new(ahead_rpc::core::CoreRpcHandler::new());
        let (sender, receiver) = crossbeam_channel::unbounded();
        let callback_count = Arc::new(AtomicUsize::new(0));
        let completed = callback_count.clone();
        rpc.get_code_actions(
            &std::env::temp_dir().join("actions.py"),
            lsp_types::Position::new(0, 0),
            Vec::new(),
            move |result| {
                completed.fetch_add(1, Ordering::SeqCst);
                sender.send(result).expect("action result");
            },
        );
        let PluginCatalogRpc::ServerRequest {
            f,
            request_sent: Some(request_sent),
            ..
        } = rpc.test_receiver().recv().expect("request")
        else {
            panic!("server request expected");
        };
        request_sent.store(4, Ordering::Release);
        let barrier = Arc::new(Barrier::new(4));
        std::thread::scope(|scope| {
            for id in 1..=4 {
                let callback = dyn_clone::clone_box(&*f);
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    callback(
                        PluginId(id),
                        Ok(serde_json::json!([
                            { "title": "Same title", "data": { "server": id } }
                        ])),
                    );
                });
            }
        });
        let actions = receiver
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("reply")
            .expect("success");
        assert_eq!(
            actions
                .iter()
                .map(|action| action.plugin_id)
                .collect::<Vec<_>>(),
            [PluginId(1), PluginId(2), PluginId(3), PluginId(4)]
        );
        assert_eq!(callback_count.load(Ordering::SeqCst), 1);
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn formatting_accepts_null_and_empty_edits_without_hiding_errors() {
        use super::{PluginCatalogRpc, PluginCatalogRpcHandler};
        use ahead_rpc::{RpcError, plugin::PluginId};
        use serde_json::json;

        let edit = lsp_types::TextEdit {
            range: lsp_types::Range::new(
                lsp_types::Position::new(0, 0),
                lsp_types::Position::new(0, 1),
            ),
            new_text: "formatted".into(),
        };
        for (reply, expected) in [
            (Ok(serde_json::Value::Null), Ok(Vec::new())),
            (Ok(json!([])), Ok(Vec::new())),
            (Ok(json!([edit.clone()])), Ok(vec![edit])),
            (
                Err(RpcError {
                    code: -32601,
                    message: "server not capable".into(),
                }),
                Err("server not capable"),
            ),
        ] {
            let rpc =
                PluginCatalogRpcHandler::new(ahead_rpc::core::CoreRpcHandler::new());
            let (sender, receiver) = crossbeam_channel::bounded(1);
            rpc.get_document_formatting(
                &std::env::temp_dir().join("format.py"),
                move |id, result| {
                    sender.send((id, result)).expect("format result");
                },
            );
            let PluginCatalogRpc::ServerRequest {
                f,
                request_sent: None,
                ..
            } = rpc.test_receiver().recv().expect("request")
            else {
                panic!("server request expected");
            };
            f(PluginId(7), reply);
            let (id, result) = receiver
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("response");
            assert_eq!(id, PluginId(7));
            match expected {
                Ok(edits) => assert_eq!(result.expect("successful format"), edits),
                Err(message) => assert_eq!(
                    result.expect_err("unsupported format").message,
                    message
                ),
            }
        }
    }

    #[test]
    fn first_capable_requests_preserve_null_and_report_invalid_responses() {
        use super::{PluginCatalogRpc, PluginCatalogRpcHandler};
        for (reply, succeeds) in [
            (serde_json::Value::Null, true),
            (serde_json::json!(false), false),
        ] {
            let rpc =
                PluginCatalogRpcHandler::new(ahead_rpc::core::CoreRpcHandler::new());
            let (sender, receiver) = crossbeam_channel::bounded(1);
            rpc.send_request_to_first_capable_plugin::<_, Option<Vec<String>>>(
                "test/request",
                (),
                None,
                None,
                move |id, result| sender.send((id, result)).expect("response"),
            );
            let PluginCatalogRpc::ServerRequest {
                request_sent: None,
                f,
                ..
            } = rpc.test_receiver().recv().expect("request")
            else {
                panic!("single-server request expected");
            };
            f(ahead_rpc::plugin::PluginId(7), Ok(reply));
            let (id, result) = receiver
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("response");
            assert_eq!(id, ahead_rpc::plugin::PluginId(7));
            if succeeds {
                assert_eq!(result.expect("valid null"), None);
            } else {
                assert_eq!(result.expect_err("invalid response").code, -32603);
            }
        }
    }

    #[test]
    fn advertises_lsp_snippet_completions() {
        let capabilities = client_capabilities();
        let completion_item = capabilities
            .text_document
            .and_then(|text_document| text_document.completion)
            .and_then(|completion| completion.completion_item);
        assert_eq!(
            completion_item
                .and_then(|completion_item| completion_item.snippet_support),
            Some(true)
        );
    }

    #[test]
    fn advertises_signature_help_context_support() {
        let signature_help = client_capabilities()
            .text_document
            .and_then(|text_document| text_document.signature_help)
            .expect("signature-help capability");
        assert_eq!(signature_help.context_support, Some(true));
    }

    #[test]
    fn forwards_signature_help_retrigger_context_to_the_language_server() {
        use super::{PluginCatalogRpc, PluginCatalogRpcHandler};
        use lsp_types::{
            Position, SignatureHelp, SignatureHelpContext, SignatureHelpTriggerKind,
            SignatureInformation,
        };

        let rpc =
            PluginCatalogRpcHandler::new(ahead_rpc::core::CoreRpcHandler::new());
        let active_signature_help = SignatureHelp {
            signatures: vec![SignatureInformation {
                label: "render(value: usize)".into(),
                documentation: None,
                parameters: None,
                active_parameter: Some(0),
            }],
            active_signature: Some(0),
            active_parameter: Some(0),
        };
        let context = SignatureHelpContext {
            trigger_kind: SignatureHelpTriggerKind::CONTENT_CHANGE,
            trigger_character: None,
            is_retrigger: true,
            active_signature_help: Some(active_signature_help),
        };
        rpc.signature_help(
            7,
            &std::env::temp_dir().join("signature.rs"),
            Position::new(0, 12),
            context.clone(),
        );

        let PluginCatalogRpc::ServerRequest { method, params, .. } = rpc
            .test_receiver()
            .recv()
            .expect("signature-help server request")
        else {
            panic!("server request expected");
        };
        assert_eq!(method, "textDocument/signatureHelp");
        let params: lsp_types::SignatureHelpParams =
            serde_json::from_value(params).expect("signature-help params");
        assert_eq!(params.context, Some(context));
    }

    #[test]
    fn advertises_dynamic_execute_command_registration() {
        let capabilities = client_capabilities();
        assert_eq!(
            capabilities
                .workspace
                .and_then(|workspace| workspace.execute_command)
                .and_then(|execute_command| execute_command.dynamic_registration),
            Some(true)
        );
    }

    #[test]
    fn advertises_dynamic_file_watchers_and_relative_patterns() {
        let watched = client_capabilities()
            .workspace
            .and_then(|workspace| workspace.did_change_watched_files)
            .expect("file watch capability");
        assert_eq!(watched.dynamic_registration, Some(true));
        assert_eq!(watched.relative_pattern_support, Some(true));
    }
}
