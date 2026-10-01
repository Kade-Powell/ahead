use std::{
    borrow::Cow,
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    thread,
};

use ahead_extension_host::{
    ExtensionHost, LanguageServerCommand, LanguageServerExtension, WorktreeContext,
    discover_language_server_extensions,
};
use ahead_rpc::plugin::ServerId;
use ahead_rpc::{
    RpcError,
    dap_types::{self, DapId, DapServer, RunDebugConfig, SetBreakpointsResponse},
    delta::AheadDelta,
    plugin::PluginId,
    style::LineStyle,
};
use lsp_types::{
    Diagnostic, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentDiagnosticParams, DocumentDiagnosticReport,
    DocumentDiagnosticReportResult, PublishDiagnosticsParams, SemanticTokens,
    TextDocumentIdentifier, TextDocumentItem, VersionedTextDocumentIdentifier,
    notification::{DidCloseTextDocument, DidOpenTextDocument, Notification},
    request::{DocumentDiagnosticRequest, Request},
};
use parking_lot::Mutex;
use ropey::Rope;
use serde_json::Value;

use super::lsp::LspClient;
use super::{
    DiagnosticSource, PluginCatalogNotification, PluginCatalogRpcHandler,
    dap::{DapClient, DapRpcHandler},
    psp::{ClonableCallback, PluginServerRpc, PluginServerRpcHandler, RpcCallback},
};

pub struct PluginCatalog {
    workspace: Option<PathBuf>,
    plugin_rpc: PluginCatalogRpcHandler,
    plugins: HashMap<PluginId, PluginServerRpcHandler>,
    daps: HashMap<DapId, DapRpcHandler>,
    open_files: HashMap<PathBuf, OpenDocument>,
    diagnostics:
        HashMap<url::Url, HashMap<(PluginId, DiagnosticSource), Vec<Diagnostic>>>,
    extension_host: Option<ExtensionHost>,
    language_extensions: Vec<LanguageServerExtension>,
    /// Running servers by language id.
    lsp_servers: HashMap<String, PluginId>,
}

struct BuiltinLanguageServer {
    command: &'static str,
    languages: &'static [&'static str],
    args: &'static [&'static str],
}

fn builtin_language_server(language_id: &str) -> Option<BuiltinLanguageServer> {
    let (command, languages, args): (_, &[_], &[_]) =
        match builtin_protocol_language_id(language_id)? {
            "rust" => ("rust-analyzer", &["rust", "Rust"], &[]),
            "python" => (
                "basedpyright-langserver",
                &["python", "Python"],
                &["--stdio"],
            ),
            "javascript" | "javascriptreact" | "typescript" | "typescriptreact" => (
                "vtsls",
                &[
                    "javascript",
                    "javascriptreact",
                    "typescript",
                    "typescriptreact",
                    "JavaScript",
                    "JSX",
                    "TypeScript",
                    "TSX",
                ],
                &["--stdio"],
            ),
            _ => return None,
        };
    Some(BuiltinLanguageServer {
        command,
        languages,
        args,
    })
}

fn builtin_protocol_language_id(language: &str) -> Option<&'static str> {
    match language.to_ascii_lowercase().as_str() {
        "rust" => Some("rust"),
        "python" => Some("python"),
        "typescript" => Some("typescript"),
        "javascript" => Some("javascript"),
        "tsx" | "typescriptreact" => Some("typescriptreact"),
        "jsx" | "javascriptreact" => Some("javascriptreact"),
        _ => None,
    }
}

struct OpenDocument {
    language_id: String,
    generation: Arc<AtomicU64>,
}

impl PluginCatalog {
    pub fn new(
        workspace: Option<PathBuf>,
        plugin_rpc: PluginCatalogRpcHandler,
    ) -> Self {
        let language_extensions =
            ahead_core::directory::Directory::plugins_directory()
                .and_then(|root| match discover_language_server_extensions(&root) {
                    Ok(extensions) => Some(extensions),
                    Err(error) => {
                        tracing::error!(
                            ?error,
                            "discovering installed language extensions"
                        );
                        None
                    }
                })
                .unwrap_or_default();
        let extension_host = match ExtensionHost::new() {
            Ok(host) => Some(host),
            Err(error) => {
                tracing::error!(?error, "creating language extension host");
                None
            }
        };
        Self {
            workspace,
            plugin_rpc,
            plugins: HashMap::new(),
            daps: HashMap::new(),
            open_files: HashMap::new(),
            diagnostics: HashMap::new(),
            extension_host,
            language_extensions,
            lsp_servers: HashMap::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn handle_server_request(
        &mut self,
        plugin_id: Option<PluginId>,
        request_sent: Option<Arc<AtomicUsize>>,
        method: Cow<'static, str>,
        params: Value,
        language_id: Option<String>,
        path: Option<PathBuf>,
        check: bool,
        f: Box<dyn ClonableCallback<Value, RpcError>>,
    ) {
        if let Some(plugin_id) = plugin_id {
            if let Some(plugin) = self.plugins.get(&plugin_id) {
                plugin.server_request_async(
                    method,
                    params,
                    language_id,
                    path,
                    check,
                    move |result| {
                        f(plugin_id, result);
                    },
                );
            } else {
                f(
                    plugin_id,
                    Err(RpcError {
                        code: 0,
                        message: "plugin doesn't exist".to_string(),
                    }),
                );
            }
            return;
        }

        if let Some(request_sent) = request_sent {
            // if there are no plugins installed the callback of the client is not called
            // so check if plugins list is empty
            if self.plugins.is_empty() {
                // Add a request
                request_sent.fetch_add(1, Ordering::Relaxed);

                // make a direct callback with an "error"
                f(
                    ahead_rpc::plugin::PluginId(0),
                    Err(RpcError {
                        code: 0,
                        message: "no available plugin could make a callback, because the plugins list is empty".to_string(),
                    }),
                );
                return;
            } else {
                request_sent.fetch_add(self.plugins.len(), Ordering::Relaxed);
            }
        }
        for (plugin_id, plugin) in self.plugins.iter() {
            let f = dyn_clone::clone_box(&*f);
            let plugin_id = *plugin_id;
            plugin.server_request_async(
                method.clone(),
                params.clone(),
                language_id.clone(),
                path.clone(),
                check,
                move |result| {
                    f(plugin_id, result);
                },
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn handle_server_notification(
        &mut self,
        plugin_id: Option<PluginId>,
        method: impl Into<Cow<'static, str>>,
        params: Value,
        language_id: Option<String>,
        path: Option<PathBuf>,
        check: bool,
    ) {
        if let Some(plugin_id) = plugin_id {
            if let Some(plugin) = self.plugins.get(&plugin_id) {
                plugin.server_notification(method, params, language_id, path, check);
            }

            return;
        }

        // Otherwise send it to all plugins
        let method = method.into();
        for (_, plugin) in self.plugins.iter() {
            plugin.server_notification(
                method.clone(),
                params.clone(),
                language_id.clone(),
                path.clone(),
                check,
            );
        }
    }

    pub fn handle_did_open_text_document(&mut self, document: TextDocumentItem) {
        match document.uri.to_file_path() {
            Ok(path) => {
                if let Some(previous) = self.open_files.insert(
                    path,
                    OpenDocument {
                        language_id: document.language_id.clone(),
                        generation: Arc::new(AtomicU64::new(0)),
                    },
                ) {
                    previous.generation.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(err) => {
                tracing::error!("{:?}", err);
            }
        }
        self.ensure_lsp_server(&document.language_id.clone());

        let path = document.uri.to_file_path().ok();
        let lsp_plugin_id = self.lsp_servers.get(&document.language_id).copied();
        let protocol_language_id = self
            .language_extensions
            .iter()
            .find(|extension| extension.supports_language(&document.language_id))
            .map(|extension| extension.protocol_language_id(&document.language_id))
            .or_else(|| {
                builtin_protocol_language_id(&document.language_id)
                    .map(str::to_owned)
            });
        for (plugin_id, plugin) in self.plugins.iter() {
            let text_document = if Some(*plugin_id) == lsp_plugin_id {
                let mut document = document.clone();
                if let Some(protocol_language_id) = &protocol_language_id {
                    document.language_id = protocol_language_id.clone();
                }
                document
            } else {
                document.clone()
            };
            plugin.server_notification(
                DidOpenTextDocument::METHOD,
                DidOpenTextDocumentParams { text_document },
                Some(document.language_id.clone()),
                path.clone(),
                true,
            );
        }
        if let Some(plugin_id) = lsp_plugin_id {
            self.refresh_diagnostics(plugin_id);
        }
    }

    pub fn handle_did_close_text_document(&mut self, path: PathBuf) {
        let Some(document) = self.open_files.remove(&path) else {
            return;
        };
        document.generation.fetch_add(1, Ordering::Relaxed);
        let Ok(uri) = url::Url::from_file_path(&path) else {
            tracing::error!(?path, "invalid closed document path");
            return;
        };
        for plugin in self.plugins.values() {
            plugin.server_notification(
                DidCloseTextDocument::METHOD,
                DidCloseTextDocumentParams {
                    text_document: TextDocumentIdentifier::new(uri.clone()),
                },
                Some(document.language_id.clone()),
                Some(path.clone()),
                true,
            );
        }
        self.diagnostics.remove(&uri);
        self.plugin_rpc
            .core_rpc
            .publish_diagnostics(PublishDiagnosticsParams::new(
                uri,
                Vec::new(),
                None,
            ));
    }

    pub fn publish_diagnostics(
        &mut self,
        plugin_id: PluginId,
        source: DiagnosticSource,
        mut diagnostics: PublishDiagnosticsParams,
    ) {
        if !self.plugins.contains_key(&plugin_id) {
            return;
        }
        // Rust Analyzer pushes rustc errors separately from its pulled native
        // diagnostics. Like Zed, replace only this server/source's contribution.
        let sources = self.diagnostics.entry(diagnostics.uri.clone()).or_default();
        if diagnostics.diagnostics.is_empty() {
            sources.remove(&(plugin_id, source));
        } else {
            sources.insert((plugin_id, source), diagnostics.diagnostics);
        }
        diagnostics.diagnostics = sources.values().flatten().cloned().collect();
        diagnostics.diagnostics.sort_by(|left, right| {
            (left.range.start, left.range.end, &left.message).cmp(&(
                right.range.start,
                right.range.end,
                &right.message,
            ))
        });
        // The merged sources need not describe the same document version.
        diagnostics.version = None;
        if sources.is_empty() {
            self.diagnostics.remove(&diagnostics.uri);
        }
        self.plugin_rpc.core_rpc.publish_diagnostics(diagnostics);
    }

    pub fn refresh_diagnostics(&self, plugin_id: PluginId) {
        let Some(plugin) = self.plugins.get(&plugin_id) else {
            return;
        };
        for (path, document) in &self.open_files {
            let Ok(uri) = url::Url::from_file_path(path) else {
                continue;
            };
            let catalog_rpc = self.plugin_rpc.clone();
            let text_document = TextDocumentIdentifier::new(uri.clone());
            let generation = document.generation.load(Ordering::Relaxed);
            let current_generation = document.generation.clone();
            plugin.server_request_async(
                DocumentDiagnosticRequest::METHOD,
                DocumentDiagnosticParams {
                    text_document,
                    identifier: None,
                    previous_result_id: None,
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                },
                Some(document.language_id.clone()),
                Some(path.clone()),
                true,
                move |result| match result {
                    Ok(value) => match serde_json::from_value::<
                        DocumentDiagnosticReportResult,
                    >(value)
                    {
                        Ok(report) => {
                            if current_generation.load(Ordering::Relaxed)
                                == generation
                                && let Some(diagnostics) =
                                    full_document_diagnostics(report)
                            {
                                catalog_rpc.publish_diagnostics(
                                    plugin_id,
                                    DiagnosticSource::Pulled,
                                    PublishDiagnosticsParams {
                                        uri,
                                        diagnostics,
                                        version: None,
                                    },
                                );
                            }
                        }
                        Err(error) => {
                            tracing::warn!(?error, "decoding document diagnostics")
                        }
                    },
                    Err(error) => {
                        tracing::debug!(?error, "pulling document diagnostics")
                    }
                },
            );
        }
    }

    fn ensure_lsp_server(&mut self, language_id: &str) {
        if self.lsp_servers.contains_key(language_id) {
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        if !self
            .language_extensions
            .iter()
            .any(|extension| extension.supports_language(language_id))
            && let Some(root) = ahead_core::directory::Directory::plugins_directory()
        {
            match discover_language_server_extensions(&root) {
                Ok(extensions) => self.language_extensions = extensions,
                Err(error) => {
                    self.report_language_server_error(language_id, &error);
                    return;
                }
            }
        }
        let extension = self
            .language_extensions
            .iter()
            .find(|extension| extension.supports_language(language_id));
        let (server, server_id, mut languages) = if let Some(extension) = extension {
            let result = (|| -> anyhow::Result<LanguageServerCommand> {
                let host = self.extension_host.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("Language extension host is unavailable")
                })?;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                runtime.block_on(host.language_server_command_for_extension(
                    extension,
                    WorktreeContext {
                        root: workspace.clone(),
                    },
                ))
            })();
            match result {
                Ok(server) => (
                    server,
                    extension.server_id.clone(),
                    extension.language_ids(),
                ),
                Err(error) => {
                    self.report_language_server_error(language_id, &error);
                    return;
                }
            }
        } else if let Some(builtin) = builtin_language_server(language_id) {
            (
                LanguageServerCommand {
                    command: builtin.command.into(),
                    args: builtin.args.iter().map(|arg| (*arg).into()).collect(),
                    env: Vec::new(),
                    initialization_options: None,
                    workspace_configuration: None,
                },
                builtin.command.to_string(),
                builtin
                    .languages
                    .iter()
                    .map(|language| (*language).into())
                    .collect(),
            )
        } else {
            return;
        };
        if !languages.iter().any(|language| language == language_id) {
            languages.push(language_id.to_string());
        }
        let selector = languages
            .iter()
            .map(|language| lsp_types::DocumentFilter {
                language: Some(language.clone()),
                scheme: None,
                pattern: None,
            })
            .collect();
        let uri = if server.command.contains(std::path::MAIN_SEPARATOR) {
            url::Url::from_file_path(&server.command).map_err(|_| {
                anyhow::anyhow!("Invalid language server path: {}", server.command)
            })
        } else {
            url::Url::parse(&format!("urn:{}", server.command))
                .map_err(anyhow::Error::from)
        };
        let result = uri.and_then(|uri| {
            self.plugin_rpc.core_rpc.server_status(
                ahead_rpc::core::ServerStatusParams::starting(server_id.clone()),
            );
            LspClient::start(
                self.plugin_rpc.clone(),
                selector,
                Some(workspace.clone()),
                ServerId {
                    author: "ahead".to_string(),
                    name: format!("lsp-{server_id}"),
                },
                server_id.clone(),
                Some(PluginId::next()),
                Some(workspace),
                uri,
                server.args,
                server.env,
                server.initialization_options,
                server.workspace_configuration,
            )
        });
        match result {
            Ok((plugin_id, handler)) => {
                self.plugins.insert(plugin_id, handler);
                for language in languages {
                    self.lsp_servers.insert(language, plugin_id);
                }
            }
            Err(error) => {
                self.report_language_server_error(
                    &server_id,
                    &anyhow::anyhow!(
                        "{error:#}. Install {server_id} on PATH or install a matching Zed language extension in Settings, then restart language servers."
                    ),
                );
            }
        }
    }

    fn report_language_server_error(
        &self,
        server_name: &str,
        error: &anyhow::Error,
    ) {
        tracing::error!(server_name, ?error, "starting language server");
        self.plugin_rpc.core_rpc.server_status(
            ahead_rpc::core::ServerStatusParams::failed(
                server_name.into(),
                format!("{error:#}"),
            ),
        );
    }

    pub fn handle_did_save_text_document(
        &mut self,
        language_id: String,
        path: PathBuf,
        text_document: TextDocumentIdentifier,
        text: Rope,
    ) {
        for (_, plugin) in self.plugins.iter() {
            plugin.handle_rpc(PluginServerRpc::DidSaveTextDocument {
                language_id: language_id.clone(),
                path: path.clone(),
                text_document: text_document.clone(),
                text: text.clone(),
            });
        }
    }

    pub fn handle_did_change_text_document(
        &mut self,
        language_id: String,
        document: VersionedTextDocumentIdentifier,
        delta: AheadDelta,
        text: Rope,
        new_text: Rope,
    ) {
        if let Ok(path) = document.uri.to_file_path()
            && let Some(open_document) = self.open_files.get(&path)
        {
            open_document.generation.fetch_add(1, Ordering::Relaxed);
        }
        let change = Arc::new(Mutex::new((None, None)));
        for (_, plugin) in self.plugins.iter() {
            plugin.handle_rpc(PluginServerRpc::DidChangeTextDocument {
                language_id: language_id.clone(),
                document: document.clone(),
                delta: delta.clone(),
                text: text.clone(),
                new_text: new_text.clone(),
                change: change.clone(),
            });
        }
        // ponytail: pull per edit; debounce by document if LSP traffic becomes costly.
        for plugin_id in self.plugins.keys().copied() {
            self.refresh_diagnostics(plugin_id);
        }
    }

    pub fn format_semantic_tokens(
        &self,
        plugin_id: PluginId,
        tokens: SemanticTokens,
        text: Rope,
        f: Box<dyn RpcCallback<Vec<LineStyle>, RpcError>>,
    ) {
        if let Some(plugin) = self.plugins.get(&plugin_id) {
            plugin.handle_rpc(PluginServerRpc::FormatSemanticTokens {
                tokens,
                text,
                f,
            });
        } else {
            f.call(Err(RpcError {
                code: 0,
                message: "plugin doesn't exist".to_string(),
            }));
        }
    }

    pub fn dap_variable(
        &self,
        dap_id: DapId,
        reference: usize,
        f: Box<dyn RpcCallback<Vec<dap_types::Variable>, RpcError>>,
    ) {
        if let Some(dap) = self.daps.get(&dap_id) {
            dap.variables_async(
                reference,
                |result: Result<dap_types::VariablesResponse, RpcError>| {
                    f.call(result.map(|resp| resp.variables))
                },
            );
        } else {
            f.call(Err(RpcError {
                code: 0,
                message: "plugin doesn't exist".to_string(),
            }));
        }
    }

    pub fn dap_get_scopes(
        &self,
        dap_id: DapId,
        frame_id: usize,
        f: Box<
            dyn RpcCallback<
                    Vec<(dap_types::Scope, Vec<dap_types::Variable>)>,
                    RpcError,
                >,
        >,
    ) {
        if let Some(dap) = self.daps.get(&dap_id) {
            let local_dap = dap.clone();
            dap.scopes_async(
                frame_id,
                move |result: Result<dap_types::ScopesResponse, RpcError>| {
                    match result {
                        Ok(resp) => {
                            let scopes = resp.scopes.clone();
                            if let Some(scope) = resp.scopes.first() {
                                let scope = scope.to_owned();
                                thread::spawn(move || {
                                    local_dap.variables_async(
                                        scope.variables_reference,
                                        move |result: Result<
                                            dap_types::VariablesResponse,
                                            RpcError,
                                        >| {
                                            let resp: Vec<(
                                                dap_types::Scope,
                                                Vec<dap_types::Variable>,
                                            )> = scopes
                                                .iter()
                                                .enumerate()
                                                .map(|(index, s)| {
                                                    (
                                                        s.clone(),
                                                        if index == 0 {
                                                            result
                                                                .as_ref()
                                                                .map(|resp| {
                                                                    resp.variables
                                                                        .clone()
                                                                })
                                                                .unwrap_or_default()
                                                        } else {
                                                            Vec::new()
                                                        },
                                                    )
                                                })
                                                .collect();
                                            f.call(Ok(resp));
                                        },
                                    );
                                });
                            } else {
                                f.call(Ok(Vec::new()));
                            }
                        }
                        Err(e) => {
                            f.call(Err(e));
                        }
                    }
                },
            );
        } else {
            f.call(Err(RpcError {
                code: 0,
                message: "plugin doesn't exist".to_string(),
            }));
        }
    }

    pub fn handle_notification(&mut self, notification: PluginCatalogNotification) {
        use PluginCatalogNotification::*;
        match notification {
            RestartLanguageServers { documents } => {
                for plugin in self.plugins.values() {
                    self.plugin_rpc.core_rpc.server_status(
                        ahead_rpc::core::ServerStatusParams::starting(
                            plugin
                                .server_id
                                .name
                                .strip_prefix("lsp-")
                                .unwrap_or(&plugin.server_id.name)
                                .to_owned(),
                        ),
                    );
                    plugin.shutdown();
                }
                for plugin in self.plugins.values() {
                    if !plugin.wait_for_shutdown() {
                        self.report_language_server_error(plugin.server_id.name.strip_prefix("lsp-").unwrap_or(&plugin.server_id.name), &anyhow::anyhow!("Language server did not stop before its shutdown deadline. Try restarting again."));
                        return;
                    }
                }
                for plugin_id in self.plugins.keys().copied().collect::<Vec<_>>() {
                    self.remove_language_server(plugin_id);
                }
                for document in documents {
                    self.handle_did_open_text_document(document);
                }
            }
            LanguageServerStopped { plugin_id, message } => {
                if let Some(plugin) = self.plugins.get(&plugin_id) {
                    let name = plugin
                        .server_id
                        .name
                        .strip_prefix("lsp-")
                        .unwrap_or(&plugin.server_id.name)
                        .to_owned();
                    self.remove_language_server(plugin_id);
                    self.plugin_rpc.core_rpc.server_status(
                        ahead_rpc::core::ServerStatusParams::failed(name, message),
                    );
                }
            }
            LanguageServerStatus { plugin_id, params } => {
                if self.plugins.contains_key(&plugin_id) {
                    self.plugin_rpc.core_rpc.server_status(params);
                }
            }
            DapStart {
                config,
                breakpoints,
            } => {
                // Keep disconnecting adapters owned until cleanup finishes, so
                // catalog shutdown can still wait for their processes.
                self.daps.retain(|_, dap| {
                    !dap.wait_for_shutdown_until(std::time::Instant::now())
                });
                let server = resolve_dap_server(&config, self.workspace.clone());
                let dap_id = config.dap_id;
                match DapClient::start(
                    server,
                    config,
                    breakpoints,
                    self.plugin_rpc.clone(),
                ) {
                    Ok(dap_rpc) => {
                        if let Some(previous) =
                            self.daps.insert(dap_rpc.dap_id, dap_rpc)
                        {
                            previous.shutdown();
                        }
                    }
                    Err(error) => self.plugin_rpc.core_rpc.dap_session_state(
                        dap_id,
                        ahead_rpc::dap_types::DapSessionState::Failed(format!(
                            "Could not start debugger: {error}"
                        )),
                    ),
                }
            }
            DapTerminalResponse { response } => {
                if let Some(dap) = self.daps.get(&response.dap_id)
                    && let Err(error) = dap.answer_terminal_request(response)
                {
                    tracing::error!(?error, "answering debug terminal request");
                }
            }
            DapContinue { dap_id, thread_id } => {
                if let Some(dap) = self.daps.get(&dap_id) {
                    dap.continue_thread(thread_id);
                }
            }
            DapPause { dap_id, thread_id } => {
                if let Some(dap) = self.daps.get(&dap_id) {
                    dap.pause_thread(thread_id);
                }
            }
            DapStepOver { dap_id, thread_id } => {
                if let Some(dap) = self.daps.get(&dap_id).cloned() {
                    dap.next(thread_id);
                }
            }
            DapStepInto { dap_id, thread_id } => {
                if let Some(dap) = self.daps.get(&dap_id).cloned() {
                    dap.step_in(thread_id);
                }
            }
            DapStepOut { dap_id, thread_id } => {
                if let Some(dap) = self.daps.get(&dap_id).cloned() {
                    dap.step_out(thread_id);
                }
            }
            DapStop { dap_id } => {
                if let Some(dap) = self.daps.get(&dap_id) {
                    dap.stop();
                }
            }
            DapDisconnect { dap_id } => {
                if let Some(dap) = self.daps.get(&dap_id).cloned() {
                    thread::spawn(move || {
                        if let Err(err) = dap.disconnect() {
                            tracing::error!("{:?}", err);
                        }
                        dap.shutdown();
                    });
                }
            }
            DapRestart {
                dap_id,
                breakpoints,
            } => {
                if let Some(dap) = self.daps.get(&dap_id) {
                    dap.restart(breakpoints);
                }
            }
            DapSetBreakpoints {
                dap_id,
                path,
                breakpoints,
            } => {
                if let Some(dap) = self.daps.get(&dap_id) {
                    let core_rpc = self.plugin_rpc.core_rpc.clone();
                    dap.set_breakpoints_async(
                        path.clone(),
                        breakpoints,
                        move |result: Result<SetBreakpointsResponse, RpcError>| {
                            match result {
                                Ok(resp) => {
                                    core_rpc.dap_breakpoints_resp(
                                        dap_id,
                                        path,
                                        resp.breakpoints.unwrap_or_default(),
                                    );
                                }
                                Err(err) => {
                                    tracing::error!("{:?}", err);
                                }
                            }
                        },
                    );
                }
            }
            Shutdown => {
                for (_, plugin) in self.plugins.iter() {
                    plugin.shutdown();
                }
                for dap in self.daps.values() {
                    dap.shutdown();
                }
                let deadline = std::time::Instant::now()
                    + super::psp::SERVER_SHUTDOWN_TIMEOUT
                    + std::time::Duration::from_secs(1);
                for plugin in self.plugins.values() {
                    if !plugin.wait_for_shutdown_until(deadline) {
                        tracing::error!(
                            ?plugin.plugin_id,
                            "language server did not stop before the catalog shutdown deadline"
                        );
                    }
                }
                for dap in self.daps.values() {
                    if !dap.wait_for_shutdown_until(deadline) {
                        tracing::error!(?dap.dap_id, "debug adapter did not stop before the catalog shutdown deadline");
                    }
                }
            }
        }
    }

    fn remove_language_server(&mut self, plugin_id: PluginId) {
        self.plugins.remove(&plugin_id);
        self.lsp_servers.retain(|_, id| *id != plugin_id);
        self.diagnostics.retain(|uri, sources| {
            let before = sources.len();
            sources.retain(|(id, _), _| *id != plugin_id);
            if before != sources.len() {
                self.plugin_rpc.core_rpc.publish_diagnostics(
                    PublishDiagnosticsParams::new(
                        uri.clone(),
                        sources.values().flatten().cloned().collect(),
                        None,
                    ),
                );
            }
            !sources.is_empty()
        });
    }
}

fn full_document_diagnostics(
    report: DocumentDiagnosticReportResult,
) -> Option<Vec<Diagnostic>> {
    match report {
        DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(
            report,
        )) => Some(report.full_document_diagnostic_report.items),
        _ => None,
    }
}

#[cfg(test)]
mod document_diagnostic_tests {
    use super::*;

    #[test]
    fn completion_without_a_running_server_finishes_with_an_empty_list() {
        use ahead_rpc::core::{CoreNotification, CoreRpc, CoreRpcHandler};
        let core = CoreRpcHandler::new();
        let rpc = PluginCatalogRpcHandler::new(core.clone());
        let mut catalog = PluginCatalog::new(None, rpc.clone());
        let path = std::env::temp_dir().join("ahead-completion-test.ts");
        rpc.completion(3, &path, String::new(), lsp_types::Position::default());
        rpc.shutdown();
        rpc.mainloop(&mut catalog);
        let CoreRpc::Notification(notification) =
            core.rx().try_recv().expect("completion finished")
        else {
            panic!("notification");
        };
        let CoreNotification::CompletionResponse {
            request_id, resp, ..
        } = *notification
        else {
            panic!("completion");
        };
        assert_eq!(request_id, 3);
        assert!(
            matches!(resp, lsp_types::CompletionResponse::Array(items) if items.is_empty())
        );
    }

    #[test]
    fn closing_a_document_clears_its_diagnostics_and_invalidates_pending_pulls() {
        use ahead_rpc::core::{CoreNotification, CoreRpc, CoreRpcHandler};
        let core_rpc = CoreRpcHandler::new();
        let mut catalog =
            PluginCatalog::new(None, PluginCatalogRpcHandler::new(core_rpc.clone()));
        let uri = url::Url::parse("file:///fixture/main.ts").expect("URI");
        let path = uri.to_file_path().expect("path");
        let generation = Arc::new(AtomicU64::new(7));
        catalog.open_files.insert(
            path.clone(),
            OpenDocument {
                language_id: "typescript".into(),
                generation: generation.clone(),
            },
        );
        catalog.diagnostics.insert(
            uri.clone(),
            HashMap::from([(
                (PluginId(1), DiagnosticSource::Pulled),
                vec![Diagnostic::default()],
            )]),
        );
        catalog.handle_did_close_text_document(path.clone());
        assert!(!catalog.open_files.contains_key(&path));
        assert_eq!(generation.load(Ordering::Relaxed), 8);
        assert!(!catalog.diagnostics.contains_key(&uri));
        let CoreRpc::Notification(notification) =
            core_rpc.rx().try_recv().expect("diagnostic update")
        else {
            panic!("expected notification");
        };
        let CoreNotification::PublishDiagnostics { diagnostics } = *notification
        else {
            panic!("expected diagnostics");
        };
        assert_eq!(diagnostics.uri, uri);
        assert!(diagnostics.diagnostics.is_empty());
        catalog.handle_did_close_text_document(path);
        assert!(core_rpc.rx().try_recv().is_err(), "close is idempotent");
    }

    #[test]
    fn diagnostic_updates_preserve_other_sources_and_servers() {
        use ahead_rpc::core::{CoreNotification, CoreRpc, CoreRpcHandler};

        let core_rpc = CoreRpcHandler::new();
        let mut catalog =
            PluginCatalog::new(None, PluginCatalogRpcHandler::new(core_rpc.clone()));
        let uri = url::Url::parse("file:///fixture/main.rs").expect("file URL");
        for id in [PluginId(1), PluginId(2)] {
            let (sender, _) = crossbeam_channel::unbounded();
            catalog.plugins.insert(
                id,
                PluginServerRpcHandler::new(
                    ServerId {
                        author: "ahead".into(),
                        name: format!("test-{}", id.0),
                    },
                    Some(id),
                    sender,
                ),
            );
        }
        for (plugin_id, source, message, expected) in [
            (
                1,
                DiagnosticSource::Pushed,
                Some("compiler"),
                vec!["compiler"],
            ),
            (1, DiagnosticSource::Pulled, None, vec!["compiler"]),
            (
                1,
                DiagnosticSource::Pulled,
                Some("syntax"),
                vec!["compiler", "syntax"],
            ),
            (
                2,
                DiagnosticSource::Pushed,
                Some("lint"),
                vec!["compiler", "lint", "syntax"],
            ),
            (1, DiagnosticSource::Pushed, None, vec!["lint", "syntax"]),
            (1, DiagnosticSource::Pulled, None, vec!["lint"]),
            (2, DiagnosticSource::Pushed, None, vec![]),
        ] {
            catalog.publish_diagnostics(
                PluginId(plugin_id),
                source,
                PublishDiagnosticsParams {
                    uri: uri.clone(),
                    diagnostics: message
                        .into_iter()
                        .map(|message| Diagnostic {
                            message: message.to_string(),
                            ..Diagnostic::default()
                        })
                        .collect(),
                    version: Some(1),
                },
            );
            let CoreRpc::Notification(notification) =
                core_rpc.rx().try_recv().expect("diagnostic update")
            else {
                panic!("expected notification");
            };
            let CoreNotification::PublishDiagnostics { diagnostics } = *notification
            else {
                panic!("expected merged diagnostics");
            };
            assert_eq!(diagnostics.uri, uri);
            assert_eq!(diagnostics.version, None);
            assert_eq!(
                diagnostics
                    .diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.message.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
        }
        assert!(catalog.diagnostics.is_empty());
        for id in [1, 2] {
            catalog.publish_diagnostics(
                PluginId(id),
                DiagnosticSource::Pushed,
                PublishDiagnosticsParams::new(
                    uri.clone(),
                    vec![Diagnostic {
                        message: format!("server {id}"),
                        ..Diagnostic::default()
                    }],
                    None,
                ),
            );
            core_rpc.rx().try_recv().expect("diagnostics");
        }
        catalog.remove_language_server(PluginId(1));
        let CoreRpc::Notification(notification) =
            core_rpc.rx().try_recv().expect("clear stopped server")
        else {
            panic!("notification");
        };
        let CoreNotification::PublishDiagnostics { diagnostics } = *notification
        else {
            panic!("diagnostics");
        };
        assert_eq!(diagnostics.diagnostics.len(), 1);
        assert_eq!(diagnostics.diagnostics[0].message, "server 2");
        catalog.publish_diagnostics(
            PluginId(1),
            DiagnosticSource::Pushed,
            PublishDiagnosticsParams::new(uri, vec![Diagnostic::default()], None),
        );
        catalog.handle_notification(
            PluginCatalogNotification::LanguageServerStatus {
                plugin_id: PluginId(1),
                params: ahead_rpc::core::ServerStatusParams::ready("stale".into()),
            },
        );
        catalog.handle_notification(
            PluginCatalogNotification::LanguageServerStopped {
                plugin_id: PluginId(1),
                message: "stale exit".into(),
            },
        );
        assert!(
            core_rpc.rx().try_recv().is_err(),
            "retired process must not replace live diagnostics or status"
        );
    }

    #[test]
    fn extracts_full_report_and_ignores_unchanged() {
        let full: DocumentDiagnosticReportResult = serde_json::from_value(serde_json::json!({
            "kind": "full",
            "items": [{
                "range": {"start": {"line": 4, "character": 7}, "end": {"line": 4, "character": 7}},
                "message": "Syntax Error: expected pattern"
            }]
        }))
        .expect("valid full diagnostic report");
        assert_eq!(full_document_diagnostics(full).unwrap().len(), 1);
        let unchanged: DocumentDiagnosticReportResult =
            serde_json::from_value(serde_json::json!({
                "kind": "unchanged", "resultId": "rust-analyzer"
            }))
            .expect("valid unchanged diagnostic report");
        assert!(full_document_diagnostics(unchanged).is_none());
    }
}

/// Builds the debug-adapter command from the user's run config, falling back
/// to the workspace when no working directory is configured.
fn resolve_dap_server(
    config: &RunDebugConfig,
    workspace: Option<PathBuf>,
) -> DapServer {
    let (program, args) = match config.debug_adapter.as_ref() {
        Some(program) => (
            program.clone(),
            config.debug_adapter_args.clone().unwrap_or_default(),
        ),
        None => (
            config.program.clone(),
            config.args.clone().unwrap_or_default(),
        ),
    };
    DapServer {
        program,
        args,
        cwd: config.cwd.clone().map(PathBuf::from).or(workspace),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_servers_cover_rust_python_and_the_shared_typescript_family() {
        assert_eq!(
            builtin_language_server("Rust").expect("Rust").command,
            "rust-analyzer"
        );
        let python = builtin_language_server("Python").expect("Python");
        assert_eq!(python.command, "basedpyright-langserver");
        assert_eq!(python.args, ["--stdio"]);
        for language in [
            "JavaScript",
            "TypeScript",
            "JSX",
            "TSX",
            "typescriptreact",
            "javascriptreact",
        ] {
            let server =
                builtin_language_server(language).expect("TypeScript family");
            assert_eq!(server.command, "vtsls");
            assert_eq!(server.args, ["--stdio"]);
            assert_eq!(
                server.languages,
                [
                    "javascript",
                    "javascriptreact",
                    "typescript",
                    "typescriptreact",
                    "JavaScript",
                    "JSX",
                    "TypeScript",
                    "TSX"
                ]
            );
            assert!(server.languages.contains(
                &builtin_protocol_language_id(language).expect("protocol ID")
            ));
        }
        assert!(builtin_language_server("Gleam").is_none());
    }

    fn test_config() -> RunDebugConfig {
        RunDebugConfig {
            ty: Some("lldb".to_string()),
            debug_adapter: None,
            debug_adapter_args: None,
            name: "Run".to_string(),
            program: "/usr/bin/codelldb".to_string(),
            args: Some(vec!["--port".to_string()]),
            cwd: Some("/work".to_string()),
            env: None,
            prelaunch: None,
            dap_id: DapId(0),
            tracing_output: false,
            config_source: Default::default(),
        }
    }

    #[test]
    fn resolve_dap_server_prefers_config_values() {
        let server = resolve_dap_server(&test_config(), Some(PathBuf::from("/ws")));
        assert_eq!(server.program, "/usr/bin/codelldb");
        assert_eq!(server.args, vec!["--port".to_string()]);
        assert_eq!(server.cwd, Some(PathBuf::from("/work")));
    }

    #[test]
    fn resolve_dap_server_keeps_target_args_out_of_adapter_command() {
        let mut config = test_config();
        config.debug_adapter = Some("lldb-dap".to_string());
        config.debug_adapter_args = Some(vec!["--stdio".to_string()]);
        config.args = Some(vec!["target-argument".to_string()]);

        let server = resolve_dap_server(&config, None);
        assert_eq!(server.program, "lldb-dap");
        assert_eq!(server.args, vec!["--stdio".to_string()]);
    }

    #[test]
    fn resolve_dap_server_falls_back_to_workspace() {
        let mut config = test_config();
        config.cwd = None;
        config.args = None;
        let server = resolve_dap_server(&config, Some(PathBuf::from("/ws")));
        assert_eq!(server.args, Vec::<String>::new());
        assert_eq!(server.cwd, Some(PathBuf::from("/ws")));
    }

    #[test]
    fn debugger_disconnect_retains_ownership_until_cleanup_then_prunes_the_session()
    {
        let plugin_rpc = super::PluginCatalogRpcHandler::new(
            ahead_rpc::core::CoreRpcHandler::new(),
        );
        let mut catalog = super::PluginCatalog::new(None, plugin_rpc.clone());
        let config = test_config();
        let client = super::DapClient::new(
            resolve_dap_server(&config, None),
            config.clone(),
            std::collections::HashMap::new(),
            plugin_rpc,
        )
        .expect("client");
        let rpc = client.dap_rpc.clone();
        catalog.daps.insert(config.dap_id, rpc.clone());
        catalog.handle_notification(
            super::PluginCatalogNotification::DapDisconnect {
                dap_id: config.dap_id,
            },
        );
        assert!(catalog.daps.contains_key(&config.dap_id));
        assert!(rpc.wait_for_shutdown_until(
            std::time::Instant::now() + std::time::Duration::from_secs(3)
        ));
        let mut next_config = config.clone();
        next_config.dap_id = DapId(4312);
        next_config.debug_adapter = Some("ahead-missing-debug-adapter-test".into());
        catalog.handle_notification(super::PluginCatalogNotification::DapStart {
            config: next_config.clone(),
            breakpoints: std::collections::HashMap::new(),
        });
        assert!(!catalog.daps.contains_key(&config.dap_id));
        assert!(catalog.daps.contains_key(&next_config.dap_id));
        catalog.handle_notification(super::PluginCatalogNotification::Shutdown);
        drop(client);
    }
}
