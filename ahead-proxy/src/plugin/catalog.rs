use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fs::OpenOptions,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
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
    proxy::{ProxyNotification, ProxyRpcHandler},
    style::LineStyle,
};
use fs4::fs_std::FileExt;
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
    proxy_rpc: Option<ProxyRpcHandler>,
    installing_servers: Arc<Mutex<HashSet<&'static str>>>,
    cancel_installs: Arc<AtomicBool>,
    plugins: HashMap<PluginId, PluginServerRpcHandler>,
    daps: HashMap<DapId, DapRpcHandler>,
    open_files: HashMap<PathBuf, OpenDocument>,
    diagnostics:
        HashMap<url::Url, HashMap<(PluginId, DiagnosticSource), Vec<Diagnostic>>>,
    extension_host: Option<Arc<ExtensionHost>>,
    language_extensions: Vec<LanguageServerExtension>,
    extension_servers: HashMap<PluginId, LanguageServerExtension>,
    configuration_generation: u64,
    /// Running servers by language id.
    lsp_servers: HashMap<String, PluginId>,
}

#[derive(Clone, Copy)]
struct BuiltinLanguageServer {
    command: &'static str,
    languages: &'static [&'static str],
    args: &'static [&'static str],
}

impl BuiltinLanguageServer {
    fn npm_package(&self) -> Option<(&'static str, &'static str, &'static str)> {
        match self.command {
            "basedpyright-langserver" => {
                Some(("basedpyright", "basedpyright", "langserver.index.js"))
            }
            "vtsls" => Some(("vtsls", "@vtsls/language-server", "bin/vtsls.js")),
            _ => None,
        }
    }

    fn cached_npm_command(
        &self,
        cache: &Path,
        node: &Path,
    ) -> Option<LanguageServerCommand> {
        let (directory, package_name, entrypoint) = self.npm_package()?;
        let package = cache
            .join("language-servers")
            .join(directory)
            .join("node_modules")
            .join(package_name);
        let script = valid_npm_entrypoint(&package, package_name, entrypoint)?;
        Some(LanguageServerCommand {
            command: node.to_str()?.into(),
            args: vec![script.to_str()?.into(), "--stdio".into()],
            env: Vec::new(),
            initialization_options: None,
            workspace_configuration: None,
        })
    }
}

fn valid_npm_entrypoint(
    package: &Path,
    expected_name: &str,
    entrypoint: &str,
) -> Option<PathBuf> {
    let script = package.join(entrypoint);
    let manifest = package.join("package.json");
    if !std::fs::symlink_metadata(&script).ok()?.is_file()
        || !std::fs::symlink_metadata(&manifest).ok()?.is_file()
    {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(manifest)
        .ok()?
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 64 * 1024 {
        return None;
    }
    let manifest: Value = serde_json::from_slice(&bytes).ok()?;
    (manifest.get("name")?.as_str()? == expected_name
        && !manifest.get("version")?.as_str()?.is_empty())
    .then_some(script)
}

fn install_builtin_npm_package(
    cache: &Path,
    npm: &Path,
    builtin: BuiltinLanguageServer,
    cancelled: &AtomicBool,
) -> anyhow::Result<()> {
    let (directory, package, entrypoint) = builtin
        .npm_package()
        .ok_or_else(|| anyhow::anyhow!("{} has no npm package", builtin.command))?;
    let root = cache.join("language-servers");
    std::fs::create_dir_all(&root)?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let lock = options.open(root.join(format!(".{directory}.lock")))?;
    anyhow::ensure!(lock.metadata()?.is_file(), "invalid {package} cache lock");
    loop {
        anyhow::ensure!(
            !cancelled.load(Ordering::Relaxed),
            "npm install of {package} was cancelled"
        );
        anyhow::ensure!(
            Instant::now() < deadline,
            "npm install of {package} timed out waiting for its cache lock"
        );
        if FileExt::try_lock_exclusive(&lock)? {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let staging = tempfile::Builder::new()
        .prefix(&format!(".{directory}-install-"))
        .tempdir_in(&root)?;
    anyhow::ensure!(
        !cancelled.load(Ordering::Relaxed),
        "npm install of {package} was cancelled"
    );
    let mut child = Command::new(npm)
        .args(["install", "--prefix"])
        .arg(staging.path())
        .arg(format!("{package}@latest"))
        .args([
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--no-package-lock",
            "--save-exact",
            "--fetch-retries=2",
            "--fetch-timeout=10000",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if cancelled.load(Ordering::Relaxed) => {
                break Err(anyhow::anyhow!(
                    "npm install of {package} was cancelled"
                ));
            }
            Ok(None) if Instant::now() >= deadline => {
                break Err(anyhow::anyhow!(
                    "npm install of {package} timed out after 120 seconds"
                ));
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(error) => break Err(error.into()),
        }
    };
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            if let Err(kill_error) = child.kill() {
                tracing::warn!(?kill_error, package, "stopping npm install");
            }
            if let Err(wait_error) = child.wait() {
                tracing::warn!(?wait_error, package, "reaping npm install");
            }
            return Err(error);
        }
    };
    anyhow::ensure!(
        status.success(),
        "npm could not install {package} ({})",
        status
    );
    anyhow::ensure!(
        !cancelled.load(Ordering::Relaxed),
        "npm install of {package} was cancelled"
    );
    let installed = staging.path().join("node_modules").join(package);
    anyhow::ensure!(
        valid_npm_entrypoint(&installed, package, entrypoint).is_some(),
        "npm finished without a valid {package} language-server package"
    );
    let destination = root.join(directory);
    let backup = root.join(format!(".{directory}-backup-{}", uuid::Uuid::new_v4()));
    let replaced = destination.exists();
    if replaced {
        std::fs::rename(&destination, &backup)?;
    }
    if let Err(error) = std::fs::rename(staging.path(), &destination) {
        if replaced {
            std::fs::rename(&backup, &destination)?;
        }
        return Err(error.into());
    }
    if replaced {
        if let Err(error) = std::fs::remove_dir_all(&backup) {
            tracing::warn!(?error, path = %backup.display(), "removing replaced language-server cache");
        }
    }
    Ok(())
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

fn report_extension_discovery_errors(
    plugin_rpc: &PluginCatalogRpcHandler,
    errors: Vec<(PathBuf, anyhow::Error)>,
) {
    let issues = errors
        .into_iter()
        .map(|(directory, error)| {
            tracing::error!(path = %directory.display(), ?error, "loading language extension");
            ahead_rpc::core::LanguageExtensionIssue {
                name: directory
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("unknown")
                    .to_owned(),
                message: format!("{error:#}"),
            }
        })
        .collect();
    plugin_rpc.core_rpc.language_extension_issues(issues);
}

impl PluginCatalog {
    pub fn new(
        workspace: Option<PathBuf>,
        plugin_rpc: PluginCatalogRpcHandler,
    ) -> Self {
        let language_extensions =
            ahead_core::directory::Directory::plugins_directory()
                .and_then(|root| match discover_language_server_extensions(&root) {
                    Ok(discovery) => {
                        report_extension_discovery_errors(
                            &plugin_rpc,
                            discovery.errors,
                        );
                        Some(discovery.extensions)
                    }
                    Err(error) => {
                        tracing::error!(
                            ?error,
                            "discovering installed language extensions"
                        );
                        report_extension_discovery_errors(
                            &plugin_rpc,
                            vec![(root, error)],
                        );
                        None
                    }
                })
                .unwrap_or_default();
        let extension_host = match ExtensionHost::new() {
            Ok(host) => Some(Arc::new(host)),
            Err(error) => {
                tracing::error!(?error, "creating language extension host");
                None
            }
        };
        Self {
            workspace,
            plugin_rpc,
            proxy_rpc: None,
            installing_servers: Arc::new(Mutex::new(HashSet::new())),
            cancel_installs: Arc::new(AtomicBool::new(false)),
            plugins: HashMap::new(),
            daps: HashMap::new(),
            open_files: HashMap::new(),
            diagnostics: HashMap::new(),
            extension_host,
            language_extensions,
            extension_servers: HashMap::new(),
            configuration_generation: 0,
            lsp_servers: HashMap::new(),
        }
    }

    pub fn with_proxy_rpc(mut self, proxy_rpc: ProxyRpcHandler) -> Self {
        self.proxy_rpc = Some(proxy_rpc);
        self
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
        for plugin in self.plugins.values() {
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
        match ahead_core::workspace_trust::is_trusted(&workspace) {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                self.report_language_server_error(
                    language_id,
                    &anyhow::Error::from(error),
                );
                return;
            }
        }
        if !self
            .language_extensions
            .iter()
            .any(|extension| extension.supports_language(language_id))
        {
            if let Err(error) = self.reload_language_extensions() {
                self.report_language_server_error(language_id, &error);
                return;
            }
        }
        let extension = self
            .language_extensions
            .iter()
            .find(|extension| extension.supports_language(language_id))
            .cloned();
        let (server, server_id, mut languages) =
            if let Some(extension) = extension.as_ref() {
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
                // ponytail: use system Node for cached packages until AHEAD
                // provisions its own runtime alongside managed server downloads.
                let has_user_binary = which::which(builtin.command).is_ok();
                let cached_command = if !has_user_binary {
                    ahead_core::directory::Directory::cache_directory().and_then(
                        |cache| {
                            which::which("node").ok().and_then(|node| {
                                builtin.cached_npm_command(&cache, &node)
                            })
                        },
                    )
                } else {
                    None
                };
                if !has_user_binary
                    && cached_command.is_none()
                    && builtin.npm_package().is_some()
                {
                    self.start_builtin_install(builtin);
                    return;
                }
                (
                    cached_command.unwrap_or_else(|| LanguageServerCommand {
                        command: builtin.command.into(),
                        args: builtin.args.iter().map(|arg| (*arg).into()).collect(),
                        env: Vec::new(),
                        initialization_options: None,
                        workspace_configuration: None,
                    }),
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
                if let Some(extension) = extension {
                    self.extension_servers.insert(plugin_id, extension);
                }
                for language in languages {
                    self.lsp_servers.insert(language, plugin_id);
                }
            }
            Err(error) => {
                self.report_language_server_error(&server_id, &error);
            }
        }
    }

    fn start_builtin_install(&self, builtin: BuiltinLanguageServer) {
        let prerequisites = (|| -> anyhow::Result<_> {
            let proxy_rpc = self.proxy_rpc.clone().ok_or_else(|| {
                anyhow::anyhow!("managed server installer is unavailable")
            })?;
            let cache = ahead_core::directory::Directory::cache_directory()
                .ok_or_else(|| {
                    anyhow::anyhow!("AHEAD cache directory is unavailable")
                })?;
            which::which("node").map_err(|_| {
                anyhow::anyhow!("Node.js is required for {}", builtin.command)
            })?;
            let npm = which::which("npm").map_err(|_| {
                anyhow::anyhow!("npm is required to install {}", builtin.command)
            })?;
            Ok((proxy_rpc, cache, npm))
        })();
        let (proxy_rpc, cache, npm) = match prerequisites {
            Ok(prerequisites) => prerequisites,
            Err(error) => {
                self.report_language_server_error(builtin.command, &error);
                return;
            }
        };
        if !self.installing_servers.lock().insert(builtin.command) {
            return;
        }
        let mut status =
            ahead_rpc::core::ServerStatusParams::starting(builtin.command.into());
        status.message = Some("Installing from npm into AHEAD's cache…".into());
        self.plugin_rpc.core_rpc.server_status(status);
        let core_rpc = self.plugin_rpc.core_rpc.clone();
        let installing = self.installing_servers.clone();
        let cancelled = self.cancel_installs.clone();
        thread::spawn(move || {
            let result =
                install_builtin_npm_package(&cache, &npm, builtin, &cancelled);
            installing.lock().remove(builtin.command);
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            match result {
                Ok(()) => proxy_rpc
                    .notification(ProxyNotification::RestartLanguageServers {}),
                Err(error) => {
                    tracing::error!(
                        server = builtin.command,
                        ?error,
                        "installing language server"
                    );
                    core_rpc.server_status(
                        ahead_rpc::core::ServerStatusParams::failed(
                            builtin.command.into(),
                            format!("{error:#}"),
                        ),
                    );
                }
            }
        });
    }

    fn reload_language_extensions(&mut self) -> anyhow::Result<()> {
        let Some(root) = ahead_core::directory::Directory::plugins_directory()
        else {
            self.plugin_rpc
                .core_rpc
                .language_extension_issues(Vec::new());
            self.language_extensions.clear();
            return Ok(());
        };
        let discovery = match discover_language_server_extensions(&root) {
            Ok(discovery) => discovery,
            Err(error) => {
                report_extension_discovery_errors(
                    &self.plugin_rpc,
                    vec![(root, anyhow::anyhow!("{error:#}"))],
                );
                return Err(error);
            }
        };
        report_extension_discovery_errors(&self.plugin_rpc, discovery.errors);
        self.language_extensions = discovery.extensions;
        Ok(())
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

    fn refresh_workspace_configurations(&mut self) {
        let (Some(workspace), Some(host)) =
            (self.workspace.as_ref(), self.extension_host.as_ref())
        else {
            return;
        };
        self.configuration_generation =
            self.configuration_generation.wrapping_add(1);
        let generation = self.configuration_generation;
        for (plugin_id, extension) in &self.extension_servers {
            if !self.plugins.contains_key(plugin_id) {
                continue;
            }
            let host = host.clone();
            let workspace = workspace.clone();
            let extension = extension.clone();
            let plugin_id = *plugin_id;
            let catalog_rpc = self.plugin_rpc.clone();
            thread::spawn(move || {
                let result = (|| -> anyhow::Result<Option<Value>> {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?;
                    runtime.block_on(host.workspace_configuration_for_extension(
                        &extension,
                        WorktreeContext { root: workspace },
                    ))
                })()
                .map_err(|error| format!("{error:#}"));
                if let Err(error) = catalog_rpc.catalog_notification(
                    PluginCatalogNotification::WorkspaceConfigurationResolved {
                        plugin_id,
                        generation,
                        result,
                    },
                ) {
                    tracing::error!(?error, "reporting language-server settings");
                }
            });
        }
    }

    pub fn handle_did_save_text_document(
        &mut self,
        language_id: String,
        path: PathBuf,
        text_document: TextDocumentIdentifier,
        text: Rope,
    ) {
        for plugin in self.plugins.values() {
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
        for plugin in self.plugins.values() {
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
            RefreshWorkspaceConfigurations => {
                self.refresh_workspace_configurations();
            }
            WorkspaceConfigurationResolved {
                plugin_id,
                generation,
                result,
            } => {
                if generation != self.configuration_generation {
                    return;
                }
                let (Some(server), Some(extension)) = (
                    self.plugins.get(&plugin_id),
                    self.extension_servers.get(&plugin_id),
                ) else {
                    return;
                };
                match result {
                    Ok(configuration) => {
                        server.update_workspace_configuration(configuration)
                    }
                    Err(message) => {
                        tracing::error!(server = %extension.server_id, %message, "refreshing language-server settings");
                        self.plugin_rpc.core_rpc.show_message(
                            "Language server settings".into(),
                            lsp_types::ShowMessageParams {
                                typ: lsp_types::MessageType::ERROR,
                                message: format!(
                                    "{}: {message}",
                                    extension.server_id
                                ),
                            },
                        );
                    }
                }
            }
            RestartLanguageServers { documents } => {
                self.plugin_rpc.core_rpc.clear_language_server_statuses();
                let installing = self
                    .installing_servers
                    .lock()
                    .iter()
                    .copied()
                    .collect::<Vec<_>>();
                for name in installing {
                    let mut status =
                        ahead_rpc::core::ServerStatusParams::starting(name.into());
                    status.message =
                        Some("Installing from npm into AHEAD's cache…".into());
                    self.plugin_rpc.core_rpc.server_status(status);
                }
                for plugin in self.plugins.values() {
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
                if let Err(error) = self.reload_language_extensions() {
                    tracing::error!(
                        ?error,
                        "reloading installed language extensions"
                    );
                    self.plugin_rpc.core_rpc.show_message(
                        "Language extensions".into(),
                        lsp_types::ShowMessageParams {
                            typ: lsp_types::MessageType::ERROR,
                            message: format!(
                                "Could not reload language extensions: {error:#}"
                            ),
                        },
                    );
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
                self.cancel_installs.store(true, Ordering::Relaxed);
                let install_deadline = Instant::now() + Duration::from_secs(1);
                while !self.installing_servers.lock().is_empty()
                    && Instant::now() < install_deadline
                {
                    thread::sleep(Duration::from_millis(10));
                }
                if !self.installing_servers.lock().is_empty() {
                    tracing::warn!(
                        "npm installs did not stop before shutdown deadline"
                    );
                }
                for plugin in self.plugins.values() {
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
        self.extension_servers.remove(&plugin_id);
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

    fn discard_initial_extension_issues(core: &ahead_rpc::core::CoreRpcHandler) {
        use ahead_rpc::core::{CoreNotification, CoreRpc};

        if let Ok(message) = core.rx().try_recv() {
            assert!(matches!(
                message,
                CoreRpc::Notification(notification)
                    if matches!(&*notification, CoreNotification::LanguageExtensionIssues { .. })
            ));
        }
    }

    #[test]
    fn extension_discovery_errors_are_not_server_statuses() {
        use ahead_rpc::core::{CoreNotification, CoreRpc, CoreRpcHandler};

        let core = CoreRpcHandler::new();
        let rpc = PluginCatalogRpcHandler::new(core.clone());
        report_extension_discovery_errors(
            &rpc,
            vec![(
                PathBuf::from("broken-extension"),
                anyhow::anyhow!("bad manifest"),
            )],
        );
        let CoreRpc::Notification(notification) =
            core.rx().try_recv().expect("issue")
        else {
            panic!("expected notification");
        };
        let CoreNotification::LanguageExtensionIssues { issues } = *notification
        else {
            panic!("extension issue must not be a language-server status");
        };
        assert_eq!(issues[0].name, "broken-extension");
        assert!(issues[0].message.contains("bad manifest"));
        assert!(core.rx().try_recv().is_err());

        report_extension_discovery_errors(&rpc, Vec::new());
        assert!(matches!(
            core.rx().try_recv(),
            Ok(CoreRpc::Notification(notification))
                if matches!(&*notification, CoreNotification::LanguageExtensionIssues { issues } if issues.is_empty())
        ));
    }

    #[test]
    fn restart_clears_old_language_server_statuses() {
        use ahead_rpc::core::{CoreNotification, CoreRpc, CoreRpcHandler};

        let core = CoreRpcHandler::new();
        let rpc = PluginCatalogRpcHandler::new(core.clone());
        let mut catalog = PluginCatalog::new(None, rpc);
        catalog.handle_notification(
            PluginCatalogNotification::RestartLanguageServers {
                documents: Vec::new(),
            },
        );
        assert!(core.rx().try_iter().any(|event| matches!(
            event,
            CoreRpc::Notification(notification)
                if matches!(&*notification, CoreNotification::LanguageServerStatusesCleared)
        )));
    }

    #[cfg(unix)]
    #[test]
    fn builtin_npm_install_publishes_only_a_complete_cached_package()
    -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let cache = tempfile::tempdir()?;
        let npm = cache.path().join("fake-npm");
        std::fs::write(
            &npm,
            r#"#!/bin/sh
mkdir -p "$3/node_modules/basedpyright"
printf '{"name":"basedpyright","version":"1.0.0"}' > "$3/node_modules/basedpyright/package.json"
: > "$3/node_modules/basedpyright/langserver.index.js"
"#,
        )?;
        std::fs::set_permissions(&npm, std::fs::Permissions::from_mode(0o700))?;
        let builtin = builtin_language_server("python").expect("Python server");
        let cancelled = AtomicBool::new(false);
        install_builtin_npm_package(cache.path(), &npm, builtin, &cancelled)?;
        let cached =
            builtin.cached_npm_command(cache.path(), Path::new("/usr/bin/node"));
        assert!(cached.is_some());

        std::fs::write(
            &npm,
            r#"#!/bin/sh
mkdir -p "$3/node_modules/basedpyright"
printf '{}' > "$3/node_modules/basedpyright/package.json"
: > "$3/node_modules/basedpyright/langserver.index.js"
"#,
        )?;
        assert!(
            install_builtin_npm_package(cache.path(), &npm, builtin, &cancelled)
                .is_err()
        );
        assert!(
            builtin
                .cached_npm_command(cache.path(), Path::new("/usr/bin/node"))
                .is_some()
        );

        std::fs::write(&npm, "#!/bin/sh\nexit 7\n")?;
        assert!(
            install_builtin_npm_package(cache.path(), &npm, builtin, &cancelled)
                .is_err()
        );
        assert!(
            builtin
                .cached_npm_command(cache.path(), Path::new("/usr/bin/node"))
                .is_some()
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_builtin_npm_installs_serialize_cache_publication()
    -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Barrier;

        let cache = tempfile::tempdir()?;
        let npm = cache.path().join("fake-npm");
        std::fs::write(
            &npm,
            r#"#!/bin/sh
mkdir "$3/../busy" || exit 27
sleep 1
mkdir -p "$3/node_modules/basedpyright"
printf '{"name":"basedpyright","version":"1.0.0"}' > "$3/node_modules/basedpyright/package.json"
: > "$3/node_modules/basedpyright/langserver.index.js"
rmdir "$3/../busy"
"#,
        )?;
        std::fs::set_permissions(&npm, std::fs::Permissions::from_mode(0o700))?;
        let start = Arc::new(Barrier::new(3));
        let workers = (0..2)
            .map(|_| {
                let cache = cache.path().to_path_buf();
                let npm = npm.clone();
                let start = start.clone();
                thread::spawn(move || {
                    start.wait();
                    install_builtin_npm_package(
                        &cache,
                        &npm,
                        builtin_language_server("python").expect("Python server"),
                        &AtomicBool::new(false),
                    )
                })
            })
            .collect::<Vec<_>>();
        start.wait();
        for worker in workers {
            worker.join().expect("installer thread")?;
        }
        assert!(
            builtin_language_server("python")
                .expect("Python server")
                .cached_npm_command(cache.path(), Path::new("/usr/bin/node"))
                .is_some()
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn shutdown_stops_an_active_builtin_npm_install() -> anyhow::Result<()> {
        use ahead_rpc::core::CoreRpcHandler;
        use std::os::unix::fs::PermissionsExt;

        let cache = tempfile::tempdir()?;
        let npm = cache.path().join("fake-npm");
        std::fs::write(&npm, "#!/bin/sh\n: > \"$3/../started\"\nexec sleep 10\n")?;
        std::fs::set_permissions(&npm, std::fs::Permissions::from_mode(0o700))?;
        let builtin = builtin_language_server("python").expect("Python server");
        let mut catalog = PluginCatalog::new(
            None,
            PluginCatalogRpcHandler::new(CoreRpcHandler::new()),
        );
        let cancelled = catalog.cancel_installs.clone();
        let installing = catalog.installing_servers.clone();
        installing.lock().insert(builtin.command);
        let cache_path = cache.path().to_path_buf();
        let worker = thread::spawn(move || {
            let result =
                install_builtin_npm_package(&cache_path, &npm, builtin, &cancelled);
            installing.lock().remove(builtin.command);
            result
        });
        let started = cache.path().join("language-servers/started");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !started.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let was_running = started.exists();
        let shutdown_started = Instant::now();
        catalog.handle_notification(PluginCatalogNotification::Shutdown);
        let result = worker.join().expect("installer thread");
        assert!(was_running, "fake npm did not start");
        assert!(format!("{result:?}").contains("cancelled"));
        assert!(shutdown_started.elapsed() < Duration::from_secs(5));
        assert!(!cache.path().join("language-servers/basedpyright").exists());
        Ok(())
    }

    #[test]
    fn builtin_servers_use_only_complete_app_cached_packages() -> anyhow::Result<()>
    {
        let cache = tempfile::tempdir()?;
        let node = Path::new("/usr/bin/node");
        for (language, package_name, script) in [
            ("python", "basedpyright", "langserver.index.js"),
            ("typescript", "@vtsls/language-server", "bin/vtsls.js"),
        ] {
            let builtin =
                builtin_language_server(language).expect("built-in server");
            assert!(builtin.cached_npm_command(cache.path(), node).is_none());
            let package = cache
                .path()
                .join("language-servers")
                .join(if language == "python" {
                    "basedpyright"
                } else {
                    "vtsls"
                })
                .join("node_modules")
                .join(package_name);
            std::fs::create_dir_all(&package)?;
            std::fs::write(package.join("package.json"), "{}")?;
            assert!(builtin.cached_npm_command(cache.path(), node).is_none());
            let script = package.join(script);
            std::fs::create_dir_all(script.parent().expect("script directory"))?;
            std::fs::write(&script, "")?;
            assert!(builtin.cached_npm_command(cache.path(), node).is_none());
            std::fs::write(
                package.join("package.json"),
                format!(r#"{{"name":"{package_name}","version":"1.0.0"}}"#),
            )?;
            let command = builtin
                .cached_npm_command(cache.path(), node)
                .expect("complete cached package");
            assert_eq!(command.command, node.to_string_lossy().into_owned());
            assert_eq!(
                command.args,
                vec![script.to_string_lossy().into_owned(), "--stdio".into()]
            );
        }
        Ok(())
    }

    #[test]
    #[ignore = "set AHEAD_ZED_EXTENSION_SOURCE to an installed HTML extension and put its server on PATH"]
    fn installed_zed_html_extension_starts_a_real_server() -> anyhow::Result<()> {
        use ahead_rpc::core::{CoreNotification, CoreRpc, CoreRpcHandler};
        use anyhow::{Context as _, bail};

        let source = PathBuf::from(std::env::var("AHEAD_ZED_EXTENSION_SOURCE")?);
        let project = tempfile::tempdir()?;
        let extension_root = project.path().join("extensions");
        let installed = extension_root.join("html");
        std::fs::create_dir_all(&installed)?;
        for name in ["extension.toml", "extension.wasm"] {
            std::fs::copy(source.join(name), installed.join(name))?;
        }
        let language_directory = installed.join("languages/html");
        std::fs::create_dir_all(&language_directory)?;
        std::fs::copy(
            source.join("languages/html/config.toml"),
            language_directory.join("config.toml"),
        )?;
        let extension = discover_language_server_extensions(&extension_root)?
            .extensions
            .into_iter()
            .find(|extension| extension.server_id == "vscode-html-language-server")
            .context("installed HTML extension has no language server")?;
        let workspace = project.path().join("workspace");
        std::fs::create_dir(&workspace)?;
        let file = workspace.join("index.html");
        std::fs::write(&file, "<di")?;
        let language = ahead_extension_host::language_id_for_path(
            &extension_root,
            &file,
            Some("<di"),
        )?
        .context("installed extension did not detect HTML")?;
        assert_eq!(language, "HTML");
        let core = CoreRpcHandler::new();
        let rpc = PluginCatalogRpcHandler::new(core.clone());
        let mut catalog = PluginCatalog::new(Some(workspace), rpc.clone());
        catalog.language_extensions = vec![extension];
        catalog.handle_did_open_text_document(TextDocumentItem::new(
            url::Url::from_file_path(&file)
                .map_err(|_| anyhow::anyhow!("invalid HTML file path"))?,
            language,
            1,
            "<di".into(),
        ));
        if !catalog.lsp_servers.contains_key("HTML") {
            bail!("installed HTML extension did not launch its language server");
        }
        let loop_rpc = rpc.clone();
        let loop_thread = std::thread::spawn(move || {
            loop_rpc.mainloop(&mut catalog);
        });
        let outcome = (|| -> anyhow::Result<()> {
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_secs(20);
            loop {
                let timeout =
                    deadline.saturating_duration_since(std::time::Instant::now());
                let CoreRpc::Notification(notification) =
                    core.rx().recv_timeout(timeout)?
                else {
                    continue;
                };
                if let CoreNotification::ServerStatus { params } = &*notification
                    && params.server_name.as_deref()
                        == Some("vscode-html-language-server")
                {
                    if params.is_ok() {
                        return Ok(());
                    }
                    if let Some(message) = &params.message {
                        bail!("installed HTML server failed: {message}");
                    }
                }
            }
        })();
        rpc.shutdown();
        loop_thread.join().expect("catalog loop");
        outcome
    }

    #[test]
    fn completion_without_a_running_server_finishes_with_an_empty_list() {
        use ahead_rpc::core::{CoreNotification, CoreRpc, CoreRpcHandler};
        let core = CoreRpcHandler::new();
        let rpc = PluginCatalogRpcHandler::new(core.clone());
        let mut catalog = PluginCatalog::new(None, rpc.clone());
        discard_initial_extension_issues(&core);
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
        discard_initial_extension_issues(&core_rpc);
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
        discard_initial_extension_issues(&core_rpc);
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
