use std::{
    borrow::Cow,
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

use ahead_rpc::{
    RpcError,
    dap_types::{self, DapId, DapServer, RunDebugConfig, SetBreakpointsResponse},
    delta::AheadDelta,
    plugin::{PluginId, ServerId},
    style::LineStyle,
};
use ropey::Rope;
use lsp_types::{
    DidOpenTextDocumentParams, SemanticTokens,
    TextDocumentIdentifier, TextDocumentItem, VersionedTextDocumentIdentifier,
    notification::{DidOpenTextDocument, Notification},
};
use parking_lot::Mutex;
use serde_json::Value;

use super::{
    PluginCatalogNotification, PluginCatalogRpcHandler,
    dap::{DapClient, DapRpcHandler},
    lsp::LspClient,
    psp::{ClonableCallback, PluginServerRpc, PluginServerRpcHandler, RpcCallback},
};

pub struct PluginCatalog {
    workspace: Option<PathBuf>,
    plugin_rpc: PluginCatalogRpcHandler,
    plugins: HashMap<PluginId, PluginServerRpcHandler>,
    daps: HashMap<DapId, DapRpcHandler>,
    open_files: HashMap<PathBuf, String>,
    /// Settings-configured language servers (`[language-servers.<lang>]`).
    lsp_commands: HashMap<String, LspServerCommand>,
    /// Running servers by language id.
    lsp_servers: HashMap<String, PluginId>,
}

/// A language server command from settings (decision 0007).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspServerCommand {
    pub command: String,
    pub args: Vec<String>,
}

impl PluginCatalog {
    pub fn new(
        workspace: Option<PathBuf>,
        plugin_rpc: PluginCatalogRpcHandler,
    ) -> Self {
        let lsp_commands = load_lsp_commands(workspace.as_deref());
        Self {
            workspace,
            plugin_rpc,
            plugins: HashMap::new(),
            daps: HashMap::new(),
            open_files: HashMap::new(),
            lsp_commands,
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
                self.open_files.insert(path, document.language_id.clone());
            }
            Err(err) => {
                tracing::error!("{:?}", err);
            }
        }
        self.ensure_lsp_server(&document.language_id.clone());

        let path = document.uri.to_file_path().ok();
        for (_, plugin) in self.plugins.iter() {
            plugin.server_notification(
                DidOpenTextDocument::METHOD,
                DidOpenTextDocumentParams {
                    text_document: document.clone(),
                },
                Some(document.language_id.clone()),
                path.clone(),
                true,
            );
        }
    }

    /// Starts the settings-configured language server for a language
    /// once, on first open (decision 0007). Later opens and edits reach
    /// it through the normal document-sync fan-out.
    fn ensure_lsp_server(&mut self, language_id: &str) {
        if self.lsp_servers.contains_key(language_id) {
            return;
        }
        let Some(server) = self.lsp_commands.get(language_id).cloned() else {
            return;
        };
        let selector = vec![lsp_types::DocumentFilter {
            language: Some(language_id.to_string()),
            scheme: None,
            pattern: None,
        }];
        let uri = if server.command.contains(std::path::MAIN_SEPARATOR) {
            match url::Url::from_file_path(&server.command) {
                Ok(uri) => uri,
                Err(_) => return,
            }
        } else {
            match url::Url::parse(&format!("urn:{}", server.command)) {
                Ok(uri) => uri,
                Err(_) => return,
            }
        };
        let plugin_rpc = self.plugin_rpc.clone();
        let workspace = self.workspace.clone();
        match LspClient::start(
            plugin_rpc,
            selector,
            workspace.clone(),
            ServerId {
                author: "ahead".to_string(),
                name: format!("lsp-{language_id}"),
            },
            format!("{language_id} language server"),
            Some(PluginId::next()),
            workspace,
            uri,
            server.args.clone(),
            None,
        ) {
            Ok((plugin_id, handler)) => {
                self.plugins.insert(plugin_id, handler);
                self.lsp_servers.insert(language_id.to_string(), plugin_id);
            }
            Err(err) => {
                tracing::error!("{:?}", err);
            }
        }
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
            DapLoaded(dap_rpc) => {
                self.daps.insert(dap_rpc.dap_id, dap_rpc);
            }
            DapDisconnected(dap_id) => {
                self.daps.remove(&dap_id);
            }
            DapStart {
                config,
                breakpoints,
            } => {
                let workspace = self.workspace.clone();
                let plugin_rpc = self.plugin_rpc.clone();
                let server = resolve_dap_server(&config, workspace);
                thread::spawn(move || {
                    match DapClient::start(
                        server,
                        config.clone(),
                        breakpoints,
                        plugin_rpc.clone(),
                    ) {
                        Ok(dap_rpc) => {
                            if let Err(err) =
                                plugin_rpc.dap_loaded(dap_rpc.clone())
                            {
                                tracing::error!("{:?}", err);
                            }

                            if let Err(err) = dap_rpc.launch(&config) {
                                tracing::error!("{:?}", err);
                            }
                        }
                        Err(err) => {
                            tracing::error!("{:?}", err);
                        }
                    }
                });
            }
            DapProcessId {
                dap_id,
                process_id,
                term_id,
            } => {
                if let Some(dap) = self.daps.get(&dap_id) {
                    if let Err(err) =
                        dap.termain_process_tx.send((term_id, process_id))
                    {
                        tracing::error!("{:?}", err);
                    }
                }
            }
            DapContinue { dap_id, thread_id } => {
                if let Some(dap) = self.daps.get(&dap_id).cloned() {
                    let plugin_rpc = self.plugin_rpc.clone();
                    thread::spawn(move || {
                        if dap.continue_thread(thread_id).is_ok() {
                            plugin_rpc.core_rpc.dap_continued(dap_id);
                        }
                    });
                }
            }
            DapPause { dap_id, thread_id } => {
                if let Some(dap) = self.daps.get(&dap_id).cloned() {
                    thread::spawn(move || {
                        if let Err(err) = dap.pause_thread(thread_id) {
                            tracing::error!("{:?}", err);
                        }
                    });
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
            }
        }
    }
}

/// Loads `[language-servers.<lang>]` tables from the workspace config
/// over the user settings, mirroring the `prediction.rs` settings
/// layering (workspace wins).
fn load_lsp_commands(workspace: Option<&std::path::Path>) -> HashMap<String, LspServerCommand> {
    let mut configs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        let path = PathBuf::from(home).join(".ahead").join("settings.toml");
        if let Some(value) = read_toml(&path) {
            configs.push(value);
        }
    }
    if let Some(workspace) = workspace {
        for path in [
            workspace.join(".ahead/config.toml"),
            workspace.join(".ahead/config.local.toml"),
        ] {
            if let Some(value) = read_toml(&path) {
                configs.push(value);
            }
        }
    }
    let mut commands = HashMap::new();
    for config in &configs {
        if let Some(table) = config
            .get("language-servers")
            .and_then(Value::as_object)
        {
            for (language, entry) in table {
                let command = entry
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                if command.is_empty() {
                    continue;
                }
                let args = entry
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|args| {
                        args.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                commands.insert(
                    language.clone(),
                    LspServerCommand {
                        command: command.to_string(),
                        args,
                    },
                );
            }
        }
    }
    commands
}

fn read_toml(path: &std::path::Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    let table: toml::Table = text.parse().ok()?;
    serde_json::to_value(table).ok()
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
            debug_command: None,
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

    fn workspace_with_config(name: &str, config: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ahead-lsp-settings-{}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(dir.join(".ahead")).unwrap();
        std::fs::write(dir.join(".ahead").join("config.toml"), config).unwrap();
        dir
    }

    #[test]
    fn load_lsp_commands_reads_project_config() {
        let workspace = workspace_with_config(
            "project",
            "[language-servers.rust]\ncommand = \"rust-analyzer\"\nargs = [\"--x\"]\n",
        );
        let commands = load_lsp_commands(Some(&workspace));
        let rust = commands.get("rust").unwrap();
        assert_eq!(rust.command, "rust-analyzer");
        assert_eq!(rust.args, vec!["--x".to_string()]);
    }

    #[test]
    fn load_lsp_commands_local_config_overrides_shared_config() {
        let workspace = workspace_with_config(
            "local-override",
            "[language-servers.rust]\ncommand = \"rust-analyzer\"\nargs = [\"--shared\"]\n",
        );
        std::fs::write(
            workspace.join(".ahead/config.local.toml"),
            "[language-servers.rust]\ncommand = \"custom-rust\"\nargs = [\"--local\"]\n",
        )
        .unwrap();

        let commands = load_lsp_commands(Some(&workspace));
        let rust = commands.get("rust").unwrap();
        assert_eq!(rust.command, "custom-rust");
        assert_eq!(rust.args, vec!["--local".to_string()]);
    }

    #[test]
    fn load_lsp_commands_skips_empty_commands() {
        let workspace = workspace_with_config(
            "empty",
            "[language-servers.go]\ncommand = \"\"\n",
        );
        let commands = load_lsp_commands(Some(&workspace));
        assert!(!commands.contains_key("go"));
    }

    #[test]
    fn load_lsp_commands_missing_workspace_is_empty_or_user_only() {
        // Must not panic without a workspace; may still see user-level
        // settings from $HOME, so only assert absence of a bogus entry.
        let commands = load_lsp_commands(None);
        assert!(!commands.contains_key("definitely-not-a-language-xyz"));
    }
}
