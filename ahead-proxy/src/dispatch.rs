use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use ahead_core::{
    encoding::offset_utf16_to_utf8_str,
    search::{
        WorkspaceFileIndex, is_binary_content, is_private_file,
        resolve_open_buffer_path,
    },
};
use ahead_extension_host::install_extension_from_url;
use ahead_rpc::{
    RequestId, RpcError,
    ahead::{
        AheadRequest as AgentHostRequest, SharedBufferEditResult,
        SharedBufferSnapshot,
    },
    buffer::BufferId,
    core::{CoreNotification, CoreRpcHandler, FileChanged},
    delta::{AheadDelta, DeltaOp},
    file::{EditorRecoverySnapshot, FileNodeItem},
    file_line::FileLine,
    proxy::{
        ProxyHandler, ProxyNotification, ProxyRequest, ProxyResponse,
        ProxyRpcHandler, SearchMatch,
    },
    source_control::{
        BlameCommit, BlameHunk, DiffHunkKind, DiffInfo, FileDiff, GitFileState,
    },
    style::{LineStyle, SemanticStyles},
    terminal::TermId,
};
use alacritty_terminal::{event::WindowSize, event_loop::Msg};
use anyhow::{Context, Result, anyhow};
use crossbeam_channel::Sender;
use git2::{
    DiffOptions, ErrorCode::NotFound, Oid, Repository, build::CheckoutBuilder,
};
use indexmap::IndexMap;
use lsp_types::{
    CancelParams, MessageType, NumberOrString, Position, Range, ShowMessageParams,
    TextDocumentItem,
    notification::{Cancel, Notification},
};
use parking_lot::Mutex;
use ropey::Rope;
use sha2::{Digest, Sha256};
use url::Url;

use crate::{
    ahead::prediction::OpenBufferContext,
    buffer::{Buffer, get_mod_time, language_id_from_path_with_content, load_file},
    plugin::{PluginCatalogRpcHandler, catalog::PluginCatalog},
    terminal::{Terminal, TerminalSender},
    watcher::{FileWatcher, Notify, WatchToken},
};

const OPEN_FILE_EVENT_TOKEN: WatchToken = WatchToken(1);
const WORKSPACE_EVENT_TOKEN: WatchToken = WatchToken(2);
const AHEAD_GITIGNORE: &str = include_str!("../../.ahead/.gitignore");
const MAX_FIM_OPEN_BUFFERS: usize = 4;
const MAX_FIM_OPEN_BUFFER_BYTES: usize = 4096;
const MAX_SHARED_BUFFER_BYTES: usize = 1024 * 1024;

fn is_provider_settings_path(workspace: &Path, path: &Path) -> bool {
    path.strip_prefix(workspace).is_ok_and(|relative| {
        relative.parent() == Some(Path::new(".ahead"))
            && matches!(
                relative.file_name().and_then(|name| name.to_str()),
                Some("settings.toml" | "config.toml" | "config.local.toml")
            )
    })
}

fn ensure_ahead_gitignore(workspace: &Path) -> io::Result<()> {
    let ahead_dir = workspace.join(".ahead");
    fs::create_dir_all(&ahead_dir)?;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(ahead_dir.join(".gitignore"))
    {
        Ok(mut file) => file.write_all(AHEAD_GITIGNORE.as_bytes()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

pub struct Dispatcher {
    workspace: Option<PathBuf>,
    pub proxy_rpc: ProxyRpcHandler,
    core_rpc: CoreRpcHandler,
    catalog_rpc: PluginCatalogRpcHandler,
    catalog_stopped: Option<crossbeam_channel::Receiver<()>>,
    buffers: HashMap<PathBuf, Buffer>,
    shared_remote_pending: HashMap<PathBuf, (String, SharedBufferSnapshot)>,
    shared_remote_recovery: HashMap<PathBuf, SharedRemoteRecovery>,
    terminals: HashMap<TermId, TerminalSender>,
    file_watcher: FileWatcher,
    file_index: Option<Arc<WorkspaceFileIndex>>,
    workspace_file_jobs: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
    shown_prediction_errors: Arc<Mutex<HashSet<String>>>,
    window_id: usize,
    tab_id: usize,
    ahead_storage_error: Option<String>,
    pub ahead_host:
        Option<Arc<parking_lot::RwLock<crate::ahead::host::AheadSessionHost>>>,
    shared_session_server: Option<crate::ahead::share::SharedSessionServer>,
    shared_session_client:
        Arc<Mutex<Option<crate::ahead::share::SharedSessionClient>>>,
    #[cfg(feature = "test-support")]
    share_test_api_base: Option<String>,
}

struct SharedRemoteRecovery {
    buffer_id: String,
    revision: u64,
    relative_path: PathBuf,
}

impl ProxyHandler for Dispatcher {
    fn handle_notification(&mut self, rpc: ProxyNotification) {
        use ProxyNotification::*;
        match rpc {
            Initialize {
                workspace,
                window_id,
                tab_id,
            } => {
                self.window_id = window_id;
                self.tab_id = tab_id;
                for cancelled in self.workspace_file_jobs.lock().values() {
                    cancelled.store(true, Ordering::SeqCst);
                }
                self.workspace_file_jobs.lock().clear();
                self.shown_prediction_errors.lock().clear();
                self.workspace = workspace;
                self.file_index = self.workspace.as_ref().map(|workspace| {
                    Arc::new(WorkspaceFileIndex::new(workspace.clone()))
                });
                let notifier = FileWatchNotifier::new(
                    self.workspace.clone(),
                    self.core_rpc.clone(),
                    self.proxy_rpc.clone(),
                    self.catalog_rpc.clone(),
                    self.file_index.clone(),
                );
                let git_metadata_paths = notifier.git_metadata_paths.clone();
                self.file_watcher.notify(notifier);
                if let Some(workspace) = self.workspace.as_ref() {
                    self.file_watcher
                        .watch(workspace, true, WORKSPACE_EVENT_TOKEN);
                    // Linked worktrees and opened subdirectories keep Git
                    // metadata outside the workspace's recursive file watch.
                    for path in &git_metadata_paths {
                        if !path.starts_with(workspace) {
                            self.file_watcher.watch(
                                path,
                                true,
                                WORKSPACE_EVENT_TOKEN,
                            );
                        }
                    }
                }

                let plugin_rpc = self.catalog_rpc.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                let workspace = self.workspace.clone();
                let (stopped, catalog_stopped) = crossbeam_channel::bounded(1);
                self.catalog_stopped = Some(catalog_stopped);
                thread::spawn(move || {
                    let mut plugin =
                        PluginCatalog::new(workspace, plugin_rpc.clone())
                            .with_proxy_rpc(proxy_rpc);
                    plugin_rpc.mainloop(&mut plugin);
                    if let Err(error) = stopped.send(()) {
                        tracing::debug!(
                            ?error,
                            "proxy stopped waiting for its catalog"
                        );
                    }
                });

                let session_store = if let Some(ws) = self.workspace.as_ref() {
                    let ahead_dir = ws.join(".ahead");
                    if let Err(error) = ensure_ahead_gitignore(ws) {
                        tracing::warn!(
                            "Could not initialize {}: {error}",
                            ahead_dir.join(".gitignore").display()
                        );
                        self.core_rpc.show_message(
                            "AHEAD project setup incomplete".to_string(),
                            lsp_types::ShowMessageParams {
                                typ: lsp_types::MessageType::WARNING,
                                message: format!(
                                    "Could not create {}. Private AHEAD files may not be ignored: {error}",
                                    ahead_dir.join(".gitignore").display()
                                ),
                            },
                        );
                    }
                    let db_path = ahead_dir.join("session.db");
                    crate::ahead::store::SessionStore::open(&db_path).with_context(
                        || format!("Could not open {}", db_path.display()),
                    )
                } else {
                    crate::ahead::store::SessionStore::in_memory()
                };
                self.shared_session_server = None;
                *self.shared_session_client.lock() = None;
                self.ahead_host = None;
                self.ahead_storage_error = None;
                match session_store {
                    Ok(session_store) => {
                        let ahead_host = Arc::new(parking_lot::RwLock::new(
                            crate::ahead::host::AheadSessionHost::new(session_store),
                        ));
                        {
                            let host = ahead_host.read();
                            if let Some(ws) = self.workspace.clone() {
                                host.set_workspace(ws);
                                host.sync_memory_sources();
                            }
                            if let Some(index) = self.file_index.clone() {
                                host.set_file_index(index);
                            }
                            // Streamed harness output flows to the UI over the core
                            // notification channel; the sink is installed before any
                            // turn can start.
                            let core_rpc = self.core_rpc.clone();
                            host.set_notification_sink(Arc::new(
                                move |notification| {
                                    core_rpc.ahead_notification(notification);
                                },
                            ));
                        }
                        self.ahead_host = Some(ahead_host);
                    }
                    Err(error) => {
                        let message = format!(
                            "Agent sessions are disabled because session storage could not be opened. {error:#}"
                        );
                        tracing::error!("{message}");
                        self.core_rpc.show_message(
                            "AHEAD session storage unavailable".to_string(),
                            ShowMessageParams {
                                typ: MessageType::ERROR,
                                message: message.clone(),
                            },
                        );
                        self.ahead_storage_error = Some(message);
                    }
                }

                self.core_rpc.notification(CoreNotification::ProxyStatus {
                    status: ahead_rpc::proxy::ProxyStatus::Connected,
                });

                // send home directory for initinal filepicker dir
                let dirs = directories::UserDirs::new();

                if let Some(dirs) = dirs {
                    self.core_rpc.home_dir(dirs.home_dir().into());
                }
            }
            OpenPaths { paths } => {
                self.core_rpc
                    .notification(CoreNotification::OpenPaths { paths });
            }
            OpenFileChanged { path } => {
                if path.exists() {
                    if let Some(buffer) = self.buffers.get(&path) {
                        if get_mod_time(&buffer.path) == buffer.mod_time {
                            return;
                        }
                        match load_file(&buffer.path) {
                            Ok(content) => {
                                self.core_rpc.open_file_changed(
                                    path,
                                    FileChanged::Change(content),
                                );
                            }
                            Err(err) => {
                                tracing::event!(
                                    tracing::Level::ERROR,
                                    "Failed to re-read file after change notification: {err}"
                                );
                            }
                        }
                    }
                } else {
                    self.close_editor_buffer(&path);
                    self.core_rpc.open_file_changed(path, FileChanged::Delete);
                }
            }
            Completion {
                request_id,
                path,
                input,
                position,
            } => {
                self.catalog_rpc
                    .completion(request_id, &path, input, position);
            }
            SignatureHelp {
                request_id,
                path,
                position,
            } => {
                self.catalog_rpc.signature_help(request_id, &path, position);
            }
            Shutdown {} => {
                self.shared_session_server = None;
                *self.shared_session_client.lock() = None;
                let deadline = std::time::Instant::now() + Duration::from_secs(8);
                for cancelled in self.workspace_file_jobs.lock().values() {
                    cancelled.store(true, Ordering::SeqCst);
                }
                self.catalog_rpc.shutdown();
                for sender in self.terminals.values() {
                    sender.send(Msg::Shutdown);
                }
                if let Some(host) = &self.ahead_host {
                    host.read().harness().shutdown_harness();
                }
                if let Some(stopped) = self.catalog_stopped.take() {
                    if let Err(error) = stopped.recv_deadline(deadline) {
                        tracing::error!(
                            ?error,
                            "language catalog shutdown did not finish"
                        );
                    }
                }
                self.proxy_rpc.disconnect();
            }
            RestartLanguageServers {} => {
                let documents = self
                    .buffers
                    .iter_mut()
                    .filter_map(|(path, buffer)| {
                        let uri = Url::from_file_path(path).ok()?;
                        let text = buffer.get_document();
                        buffer.language_id =
                            language_id_from_path_with_content(path, Some(&text))
                                .unwrap_or_default();
                        Some(TextDocumentItem {
                            uri,
                            language_id: buffer.language_id.to_string(),
                            version: buffer.rev as i32,
                            text,
                        })
                    })
                    .collect();
                if let Err(error) = self.catalog_rpc.catalog_notification(crate::plugin::PluginCatalogNotification::RestartLanguageServers { documents }) {
                    tracing::error!(?error, "restarting language servers");
                }
            }
            CancelWorkspaceFiles { request_id } => {
                if let Some(cancelled) =
                    self.workspace_file_jobs.lock().get(&request_id)
                {
                    cancelled.store(true, Ordering::SeqCst);
                }
            }
            Update { path, delta, rev } => {
                if self.shared_remote_pending.contains_key(&path) {
                    tracing::debug!(
                        ?path,
                        "host update waits for shared change acknowledgement"
                    );
                    return;
                }
                let Some(buffer) = self.buffers.get_mut(&path) else {
                    tracing::warn!(path = %path.display(), "ignoring update for unopened editor buffer");
                    return;
                };
                let old_text = buffer.rope.clone();
                if buffer.update(&delta, rev).is_none() {
                    tracing::warn!(path = %path.display(), rev, "ignoring stale editor update");
                    return;
                }
                self.catalog_rpc.did_change_text_document(
                    &path,
                    rev,
                    delta,
                    old_text,
                    buffer.rope.clone(),
                );
            }
            EditorSnapshot { path, content } => {
                let acknowledging_shared_change =
                    self.shared_remote_pending.contains_key(&path);
                if let Some(pending) = self.shared_remote_pending.get(&path) {
                    if pending.1.content != content {
                        tracing::debug!(
                            ?path,
                            "host snapshot waits for shared change acknowledgement"
                        );
                        return;
                    }
                }
                match self.sync_editor_snapshot(path.clone(), content) {
                    Ok(_) if acknowledging_shared_change => {
                        self.shared_remote_pending.remove(&path);
                    }
                    Err(error) => {
                        tracing::error!(?error, "synchronizing editor buffer");
                    }
                    Ok(_) => {}
                }
            }
            CloseEditorBuffer { path } => self.close_editor_buffer(&path),
            NewTerminal { term_id, profile } => {
                let mut terminal = match Terminal::new(term_id, profile, 50, 10) {
                    Ok(terminal) => terminal,
                    Err(e) => {
                        self.core_rpc.terminal_launch_failed(term_id, e.to_string());
                        return;
                    }
                };

                #[allow(unused)]
                let mut child_id = None;

                #[cfg(target_os = "windows")]
                {
                    child_id = terminal.pty.child_watcher().pid().map(|x| x.get());
                }
                #[cfg(not(target_os = "windows"))]
                {
                    child_id = Some(terminal.pty.child().id());
                }

                self.core_rpc.terminal_process_id(term_id, child_id);
                let tx = terminal.tx.clone();
                let poller = terminal.poller.clone();
                let sender = TerminalSender::new(tx, poller);
                self.terminals.insert(term_id, sender);
                let rpc = self.core_rpc.clone();
                thread::spawn(move || {
                    terminal.run(rpc);
                });
            }
            TerminalWrite { term_id, content } => {
                if let Some(tx) = self.terminals.get(&term_id) {
                    tx.send(Msg::Input(content.into_bytes().into()));
                }
            }
            TerminalResize {
                term_id,
                width,
                height,
            } => {
                if let Some(tx) = self.terminals.get(&term_id) {
                    let size = WindowSize {
                        num_lines: height as u16,
                        num_cols: width as u16,
                        cell_width: 1,
                        cell_height: 1,
                    };

                    tx.send(Msg::Resize(size));
                }
            }
            TerminalClose { term_id } => {
                if let Some(tx) = self.terminals.remove(&term_id) {
                    tx.send(Msg::Shutdown);
                }
            }
            DapStart {
                config,
                breakpoints,
            } => {
                if let Err(err) = self.catalog_rpc.dap_start(config, breakpoints) {
                    tracing::error!("{:?}", err);
                }
            }
            DapTerminalResponse { response } => {
                if let Err(err) = self.catalog_rpc.dap_terminal_response(response) {
                    tracing::error!("{:?}", err);
                }
            }
            DapContinue { dap_id, thread_id } => {
                if let Err(err) = self.catalog_rpc.dap_continue(dap_id, thread_id) {
                    tracing::error!("{:?}", err);
                }
            }
            DapPause { dap_id, thread_id } => {
                if let Err(err) = self.catalog_rpc.dap_pause(dap_id, thread_id) {
                    tracing::error!("{:?}", err);
                }
            }
            DapStepOver { dap_id, thread_id } => {
                if let Err(err) = self.catalog_rpc.dap_step_over(dap_id, thread_id) {
                    tracing::error!("{:?}", err);
                }
            }
            DapStepInto { dap_id, thread_id } => {
                if let Err(err) = self.catalog_rpc.dap_step_into(dap_id, thread_id) {
                    tracing::error!("{:?}", err);
                }
            }
            DapStepOut { dap_id, thread_id } => {
                if let Err(err) = self.catalog_rpc.dap_step_out(dap_id, thread_id) {
                    tracing::error!("{:?}", err);
                }
            }
            DapStop { dap_id } => {
                if let Err(err) = self.catalog_rpc.dap_stop(dap_id) {
                    tracing::error!("{:?}", err);
                }
            }
            DapDisconnect { dap_id } => {
                if let Err(err) = self.catalog_rpc.dap_disconnect(dap_id) {
                    tracing::error!("{:?}", err);
                }
            }
            DapRestart {
                dap_id,
                breakpoints,
            } => {
                if let Err(err) = self.catalog_rpc.dap_restart(dap_id, breakpoints) {
                    tracing::error!("{:?}", err);
                }
            }
            DapSetBreakpoints {
                dap_id,
                path,
                breakpoints,
            } => {
                if let Err(err) =
                    self.catalog_rpc
                        .dap_set_breakpoints(dap_id, path, breakpoints)
                {
                    tracing::error!("{:?}", err);
                }
            }
            GitCheckout { reference } => {
                if let Some(workspace) = self.workspace.as_ref() {
                    match git_checkout(workspace, &reference) {
                        Ok(()) => (),
                        Err(e) => eprintln!("{e:?}"),
                    }
                }
            }
            GitDiscardFilesChanges { files } => {
                if let Some(workspace) = self.workspace.as_ref() {
                    match git_discard_files_changes(
                        workspace,
                        files.iter().map(AsRef::as_ref),
                    ) {
                        Ok(()) => (),
                        Err(e) => eprintln!("{e:?}"),
                    }
                }
            }
            GitDiscardWorkspaceChanges {} => {
                if let Some(workspace) = self.workspace.as_ref() {
                    match git_discard_workspace_changes(workspace) {
                        Ok(()) => (),
                        Err(e) => eprintln!("{e:?}"),
                    }
                }
            }
            GitInit {} => {
                if let Some(workspace) = self.workspace.as_ref() {
                    match git_init(workspace) {
                        Ok(()) => (),
                        Err(e) => eprintln!("{e:?}"),
                    }
                }
            }
            LspCancel { id } => {
                self.catalog_rpc.send_notification(
                    None,
                    Cancel::METHOD,
                    CancelParams {
                        id: NumberOrString::Number(id),
                    },
                    None,
                    None,
                    false,
                );
            }
            AheadNotification { notification } => {
                self.core_rpc.ahead_notification(notification);
            }
        }
    }

    fn handle_request(&mut self, id: RequestId, rpc: ProxyRequest) {
        use ProxyRequest::*;
        match rpc {
            InstallLanguageExtension {
                url,
                extension_id,
                version,
                expected_sha256,
            } => {
                let proxy_rpc = self.proxy_rpc.clone();
                thread::spawn(move || {
                    let result =
                        ahead_core::directory::Directory::plugins_directory()
                            .ok_or_else(|| {
                                anyhow!("AHEAD plugin directory is unavailable")
                            })
                            .and_then(|root| {
                                install_extension_from_url(
                                    &url,
                                    &root,
                                    &extension_id,
                                    &version,
                                    expected_sha256.as_deref(),
                                )
                            })
                            .map(|_| ProxyResponse::Success {})
                            .map_err(|error| RpcError {
                                code: 0,
                                message: error.to_string(),
                            });
                    proxy_rpc.handle_response(id, result);
                });
            }
            GitStageAll {} => {
                let workspace = self.workspace.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                thread::spawn(move || {
                    let result = workspace
                        .ok_or_else(|| anyhow!("no workspace set"))
                        .and_then(|workspace| git_stage_all(&workspace))
                        .map(|_| ProxyResponse::Success {})
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                    proxy_rpc.handle_response(id, result);
                });
            }
            AheadRequest { request } => {
                if let AgentHostRequest::PublishSharedTerminal {
                    session_id, ..
                } = &request
                {
                    let session_id = session_id.clone();
                    let result = (|| -> Result<ProxyResponse> {
                        anyhow::ensure!(
                            self.shared_session_server.as_ref().is_some_and(
                                |server| server.offer().session_id == session_id
                            ),
                            "No active shared session"
                        );
                        let host = self
                            .ahead_host
                            .as_ref()
                            .context("AHEAD session host is not initialized")?;
                        Ok(ProxyResponse::AheadResponse {
                            response: host.read().handle_request(request)?,
                        })
                    })()
                    .map_err(|error| RpcError {
                        code: 0,
                        message: error.to_string(),
                    });
                    self.respond_rpc(id, result);
                    return;
                }
                if let AgentHostRequest::PublishSharedPresence {
                    session_id,
                    path,
                    line,
                } = &request
                {
                    let result = (|| -> Result<ProxyResponse> {
                        anyhow::ensure!(
                            self.shared_session_server
                                .as_ref()
                                .is_some_and(|server| server.offer().session_id
                                    == *session_id),
                            "No active shared session"
                        );
                        if let Some(path) = path {
                            self.read_shared_buffer(path)?;
                        }
                        self.shared_session_server
                            .as_ref()
                            .context("No active shared session")?
                            .set_owner_location(path.clone(), *line);
                        Ok(ProxyResponse::AheadResponse {
                            response: serde_json::json!({ "published": true }),
                        })
                    })()
                    .map_err(|error| RpcError {
                        code: 0,
                        message: error.to_string(),
                    });
                    self.respond_rpc(id, result);
                    return;
                }
                if let AgentHostRequest::DiscardSharedRemoteRecovery { path } =
                    &request
                {
                    let result = (|| -> Result<ProxyResponse> {
                        self.discard_shared_remote_recovery(path)?;
                        Ok(ProxyResponse::AheadResponse {
                            response: serde_json::json!({ "discarded": true }),
                        })
                    })()
                    .map_err(|error| RpcError {
                        code: 0,
                        message: error.to_string(),
                    });
                    self.respond_rpc(id, result);
                    return;
                }
                if let AgentHostRequest::GetSharedPresence { session_id } = &request
                {
                    let result = (|| -> Result<ProxyResponse> {
                        let server = self
                            .shared_session_server
                            .as_ref()
                            .context("No active shared session")?;
                        anyhow::ensure!(
                            server.offer().session_id == *session_id,
                            "Wrong shared session"
                        );
                        Ok(ProxyResponse::AheadResponse {
                            response: serde_json::to_value(server.presence())?,
                        })
                    })()
                    .map_err(|error| RpcError {
                        code: 0,
                        message: error.to_string(),
                    });
                    self.respond_rpc(id, result);
                    return;
                }
                if let AgentHostRequest::ReadSharedBuffer { session_id, path } =
                    &request
                {
                    if self.shared_session_client.lock().as_ref().is_some_and(
                        |connection| connection.session_id() == session_id,
                    ) {
                        let clients = self.shared_session_client.clone();
                        let proxy_rpc = self.proxy_rpc.clone();
                        let session_id = session_id.clone();
                        let path = path.clone();
                        thread::spawn(move || {
                            let result = (|| -> Result<ProxyResponse> {
                                let mut clients = clients.lock();
                                let connection = clients
                                    .as_mut()
                                    .context("No shared session is joined")?;
                                anyhow::ensure!(
                                    connection.session_id() == session_id,
                                    "Wrong joined session"
                                );
                                let snapshot = connection.read_buffer(path)?;
                                Ok(ProxyResponse::AheadResponse {
                                    response: serde_json::to_value(snapshot)?,
                                })
                            })()
                            .map_err(|error| RpcError {
                                code: 0,
                                message: error.to_string(),
                            });
                            proxy_rpc.handle_response(id, result);
                        });
                    } else {
                        let result = (|| -> Result<ProxyResponse> {
                            self.ahead_host
                                .as_ref()
                                .context("AHEAD session host is not initialized")?
                                .read()
                                .can_host_share(session_id)?;
                            let snapshot = self.read_shared_buffer(path)?;
                            Ok(ProxyResponse::AheadResponse {
                                response: serde_json::to_value(snapshot)?,
                            })
                        })()
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                        self.respond_rpc(id, result);
                    }
                    return;
                }
                if let AgentHostRequest::ReplaceSharedBuffer {
                    session_id,
                    path,
                    expected_revision,
                    content,
                } = &request
                {
                    if self.shared_session_client.lock().as_ref().is_some_and(
                        |connection| connection.session_id() == session_id,
                    ) {
                        let clients = self.shared_session_client.clone();
                        let proxy_rpc = self.proxy_rpc.clone();
                        let session_id = session_id.clone();
                        let path = path.clone();
                        let expected_revision = *expected_revision;
                        let content = content.clone();
                        thread::spawn(move || {
                            let result = (|| -> Result<ProxyResponse> {
                                let mut clients = clients.lock();
                                let connection = clients
                                    .as_mut()
                                    .context("No shared session is joined")?;
                                anyhow::ensure!(
                                    connection.session_id() == session_id,
                                    "Wrong joined session"
                                );
                                let edit = connection.replace_buffer(
                                    path,
                                    expected_revision,
                                    content,
                                )?;
                                Ok(ProxyResponse::AheadResponse {
                                    response: serde_json::to_value(edit)?,
                                })
                            })()
                            .map_err(|error| RpcError {
                                code: 0,
                                message: error.to_string(),
                            });
                            proxy_rpc.handle_response(id, result);
                        });
                    } else {
                        let result = (|| -> Result<ProxyResponse> {
                            self.ahead_host
                                .as_ref()
                                .context("AHEAD session host is not initialized")?
                                .read()
                                .can_host_share(session_id)?;
                            let edit = self.replace_shared_buffer(
                                session_id,
                                path,
                                *expected_revision,
                                content.clone(),
                            )?;
                            Ok(ProxyResponse::AheadResponse {
                                response: serde_json::to_value(edit)?,
                            })
                        })()
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                        self.respond_rpc(id, result);
                    }
                    return;
                }
                let shared_comment_session = match &request {
                    AgentHostRequest::CreateCodeComment { session_id, .. }
                    | AgentHostRequest::ListCodeComments { session_id }
                    | AgentHostRequest::ResolveCodeComment { session_id, .. } => {
                        Some(session_id.clone())
                    }
                    _ => None,
                };
                if let Some(session_id) = shared_comment_session
                    && self.shared_session_client.lock().as_ref().is_some_and(
                        |connection| connection.session_id() == session_id,
                    )
                {
                    let clients = self.shared_session_client.clone();
                    let proxy_rpc = self.proxy_rpc.clone();
                    thread::spawn(move || {
                        let result = (|| -> Result<ProxyResponse> {
                            let mut clients = clients.lock();
                            let connection = clients
                                .as_mut()
                                .context("No shared session is joined")?;
                            anyhow::ensure!(
                                connection.session_id() == session_id,
                                "Wrong joined session"
                            );
                            let response = match request {
                                AgentHostRequest::ListCodeComments { .. } => {
                                    serde_json::to_value(
                                        connection.list_code_comments()?,
                                    )?
                                }
                                AgentHostRequest::CreateCodeComment {
                                    path,
                                    range,
                                    quote,
                                    source_sha256,
                                    body,
                                    ..
                                } => serde_json::to_value(
                                    connection.create_code_comment(
                                        path,
                                        range,
                                        quote,
                                        source_sha256,
                                        body,
                                    )?,
                                )?,
                                AgentHostRequest::ResolveCodeComment {
                                    comment_id,
                                    ..
                                } => serde_json::to_value(
                                    connection.resolve_code_comment(comment_id)?,
                                )?,
                                _ => anyhow::bail!(
                                    "Unsupported shared comment request"
                                ),
                            };
                            Ok(ProxyResponse::AheadResponse { response })
                        })()
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                        proxy_rpc.handle_response(id, result);
                    });
                    return;
                }
                if let AgentHostRequest::JoinSharedSession { offer } = &request {
                    let host = self.ahead_host.clone();
                    let clients = self.shared_session_client.clone();
                    let proxy_rpc = self.proxy_rpc.clone();
                    let offer = offer.clone();
                    thread::spawn(move || {
                        let result = (|| -> Result<ProxyResponse> {
                            let host = host
                                .context("AHEAD session host is not initialized")?;
                            let token = host.read().share_join_token()?;
                            let (
                                client,
                                view,
                                messages,
                                code_comments,
                                terminal_output,
                                presence,
                            ) = crate::ahead::share::SharedSessionClient::connect(
                                &offer, &token,
                            )?;
                            let actor_id = client.actor_id().to_string();
                            *clients.lock() = Some(client);
                            Ok(ProxyResponse::AheadResponse {
                                response: serde_json::to_value(
                                    ahead_rpc::ahead::SharedSessionUpdate {
                                        view,
                                        messages,
                                        code_comments,
                                        terminal_output,
                                        presence,
                                        actor_id,
                                    },
                                )?,
                            })
                        })()
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                        proxy_rpc.handle_response(id, result);
                    });
                    return;
                }
                if let AgentHostRequest::PollSharedSession {
                    session_id,
                    after_sequence,
                    active_path,
                    active_line,
                } = &request
                {
                    let clients = self.shared_session_client.clone();
                    let proxy_rpc = self.proxy_rpc.clone();
                    let session_id = session_id.clone();
                    let after_sequence = *after_sequence;
                    let active_path = active_path.clone();
                    let active_line = *active_line;
                    thread::spawn(move || {
                        let result = (|| -> Result<ProxyResponse> {
                            let mut client = clients.lock();
                            let connection = client
                                .as_mut()
                                .context("No shared session is joined")?;
                            anyhow::ensure!(
                                connection.session_id() == session_id,
                                "Wrong joined session"
                            );
                            let (
                                view,
                                messages,
                                code_comments,
                                terminal_output,
                                presence,
                            ) = connection.poll(
                                after_sequence,
                                active_path,
                                active_line,
                            )?;
                            Ok(ProxyResponse::AheadResponse {
                                response: serde_json::to_value(
                                    ahead_rpc::ahead::SharedSessionUpdate {
                                        view,
                                        messages,
                                        code_comments,
                                        terminal_output,
                                        presence,
                                        actor_id: connection.actor_id().to_string(),
                                    },
                                )?,
                            })
                        })()
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                        proxy_rpc.handle_response(id, result);
                    });
                    return;
                }
                if let AgentHostRequest::PostSharedHumanMessage {
                    session_id,
                    content,
                } = &request
                {
                    let clients = self.shared_session_client.clone();
                    let proxy_rpc = self.proxy_rpc.clone();
                    let session_id = session_id.clone();
                    let content = content.clone();
                    thread::spawn(move || {
                        let result = (|| -> Result<ProxyResponse> {
                            let mut client = clients.lock();
                            let connection = client
                                .as_mut()
                                .context("No shared session is joined")?;
                            anyhow::ensure!(
                                connection.session_id() == session_id,
                                "Wrong joined session"
                            );
                            let message = connection.post_human_message(content)?;
                            Ok(ProxyResponse::AheadResponse {
                                response: serde_json::to_value(message)?,
                            })
                        })()
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                        proxy_rpc.handle_response(id, result);
                    });
                    return;
                }
                if let AgentHostRequest::StartSharedAgentTurn {
                    session_id,
                    content,
                } = &request
                {
                    let clients = self.shared_session_client.clone();
                    let proxy_rpc = self.proxy_rpc.clone();
                    let session_id = session_id.clone();
                    let content = content.clone();
                    thread::spawn(move || {
                        let result = (|| -> Result<ProxyResponse> {
                            let mut client = clients.lock();
                            let connection = client
                                .as_mut()
                                .context("No shared session is joined")?;
                            anyhow::ensure!(
                                connection.session_id() == session_id,
                                "Wrong joined session"
                            );
                            let turn_id = connection.start_agent_turn(content)?;
                            Ok(ProxyResponse::AheadResponse {
                                response: serde_json::to_value(turn_id)?,
                            })
                        })()
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                        proxy_rpc.handle_response(id, result);
                    });
                    return;
                }
                if let AgentHostRequest::LeaveSharedSession { session_id } = &request
                {
                    let clients = self.shared_session_client.clone();
                    let proxy_rpc = self.proxy_rpc.clone();
                    let session_id = session_id.clone();
                    thread::spawn(move || {
                        let result = (|| -> Result<ProxyResponse> {
                            let mut client = clients.lock();
                            anyhow::ensure!(
                                client.as_ref().is_some_and(|connection| {
                                    connection.session_id() == session_id
                                }),
                                "This shared session is not joined"
                            );
                            *client = None;
                            Ok(ProxyResponse::AheadResponse {
                                response: serde_json::json!({ "left": true }),
                            })
                        })()
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                        proxy_rpc.handle_response(id, result);
                    });
                    return;
                }
                if let AgentHostRequest::AddSessionParticipant {
                    session_id,
                    user_handle,
                    role,
                } = &request
                {
                    let host = self.ahead_host.clone();
                    let proxy_rpc = self.proxy_rpc.clone();
                    let session_id = session_id.clone();
                    let user_handle = user_handle.clone();
                    let role = *role;
                    thread::spawn(move || {
                        let result = (|| -> Result<ProxyResponse> {
                            let host = host
                                .context("AHEAD session host is not initialized")?;
                            host.read().can_host_share(&session_id)?;
                            let token = host.read().share_join_token()?;
                            let user = crate::ahead::share::resolve_github_user(
                                &token,
                                &user_handle,
                            )?;
                            let view =
                                host.read().add_session_participant_verified(
                                    &session_id,
                                    user,
                                    role,
                                )?;
                            Ok(ProxyResponse::AheadResponse {
                                response: serde_json::to_value(view)?,
                            })
                        })()
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                        proxy_rpc.handle_response(id, result);
                    });
                    return;
                }
                if let AgentHostRequest::RevokeSessionParticipant {
                    session_id,
                    user_handle,
                } = &request
                {
                    let result = (|| -> Result<ProxyResponse> {
                        let host = self
                            .ahead_host
                            .clone()
                            .context("AHEAD session host is not initialized")?;
                        let view = host
                            .read()
                            .revoke_session_participant(session_id, user_handle)?;
                        if let Some(server) = &self.shared_session_server
                            && server.offer().session_id == *session_id
                        {
                            server.revoke_actor(user_handle);
                        }
                        Ok(ProxyResponse::AheadResponse {
                            response: serde_json::to_value(view)?,
                        })
                    })()
                    .map_err(|error| RpcError {
                        code: 0,
                        message: error.to_string(),
                    });
                    self.respond_rpc(id, result);
                    return;
                }
                if let AgentHostRequest::ShareSession {
                    session_id,
                    bind_address,
                } = &request
                {
                    let result = (|| -> Result<ProxyResponse> {
                        let host = self
                            .ahead_host
                            .clone()
                            .context("AHEAD session host is not initialized")?;
                        host.read().can_host_share(session_id)?;
                        if let Some(server) = &self.shared_session_server {
                            anyhow::ensure!(
                                server.offer().session_id == *session_id,
                                "Stop sharing the current session first"
                            );
                            return Ok(ProxyResponse::AheadResponse {
                                response: serde_json::to_value(server.offer())?,
                            });
                        }
                        #[cfg(feature = "test-support")]
                        let server = if let Some(api_base) =
                            &self.share_test_api_base
                        {
                            crate::ahead::share::SharedSessionServer::start_with_api(
                                host,
                                self.proxy_rpc.clone(),
                                session_id.clone(),
                                bind_address,
                                api_base,
                            )?
                        } else {
                            crate::ahead::share::SharedSessionServer::start(
                                host,
                                self.proxy_rpc.clone(),
                                session_id.clone(),
                                bind_address,
                            )?
                        };
                        #[cfg(not(feature = "test-support"))]
                        let server =
                            crate::ahead::share::SharedSessionServer::start(
                                host,
                                self.proxy_rpc.clone(),
                                session_id.clone(),
                                bind_address,
                            )?;
                        let response = serde_json::to_value(server.offer())?;
                        self.shared_session_server = Some(server);
                        Ok(ProxyResponse::AheadResponse { response })
                    })()
                    .map_err(|error| RpcError {
                        code: 0,
                        message: error.to_string(),
                    });
                    self.respond_rpc(id, result);
                    return;
                }
                if let AgentHostRequest::StopSharingSession { session_id } = &request
                {
                    let result = (|| -> Result<ProxyResponse> {
                        let server = self
                            .shared_session_server
                            .as_ref()
                            .context("This session is not shared")?;
                        anyhow::ensure!(
                            server.offer().session_id == *session_id,
                            "A different session is shared"
                        );
                        self.ensure_no_pending_shared_changes(session_id)?;
                        self.shared_session_server = None;
                        if let Some(host) = &self.ahead_host {
                            host.read().clear_shared_terminal(session_id);
                        }
                        Ok(ProxyResponse::AheadResponse {
                            response: serde_json::json!({ "stopped": true }),
                        })
                    })()
                    .map_err(|error| RpcError {
                        code: 0,
                        message: error.to_string(),
                    });
                    self.respond_rpc(id, result);
                    return;
                }
                if let AgentHostRequest::ArchiveSession { session_id } = &request {
                    let result = self
                        .ensure_no_pending_shared_changes(session_id)
                        .and_then(|_| {
                            self.ahead_host
                                .as_ref()
                                .context("AHEAD session host is not initialized")
                                .and_then(|host| {
                                    host.read().handle_request(request.clone())
                                })
                        })
                        .map(|response| ProxyResponse::AheadResponse { response })
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                    if result.is_ok()
                        && self.shared_session_server.as_ref().is_some_and(
                            |server| server.offer().session_id == *session_id,
                        )
                    {
                        self.shared_session_server = None;
                    }
                    self.respond_rpc(id, result);
                    return;
                }
                let background = matches!(
                    &request,
                    AgentHostRequest::SearchMemory { .. }
                        | AgentHostRequest::WriteMemory { .. }
                        | AgentHostRequest::AgentTurnStart { .. }
                        | AgentHostRequest::AgentTurnRetry { .. }
                        | AgentHostRequest::AgentTurnCancel { .. }
                        | AgentHostRequest::AgentSessionPrepare { .. }
                        | AgentHostRequest::AgentConfigOptionSet { .. }
                );
                let host = self.ahead_host.clone();
                let storage_error = self.ahead_storage_error.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                let handle = move || {
                    let response = if let Some(host) = host {
                        host.read()
                            .handle_request(request)
                            .map(|response| ProxyResponse::AheadResponse {
                                response,
                            })
                            .map_err(|error| RpcError {
                                code: 0,
                                message: error.to_string(),
                            })
                    } else {
                        Err(RpcError {
                            code: 0,
                            message: storage_error.unwrap_or_else(|| {
                                "Ahead session host not initialized".to_string()
                            }),
                        })
                    };
                    proxy_rpc.handle_response(id, response);
                };
                if background {
                    thread::spawn(handle);
                } else {
                    handle();
                }
            }
            NewBuffer { buffer_id, path } => {
                let buffer = Buffer::new(buffer_id, path.clone());
                let content = buffer.rope.to_string();
                let read_only = buffer.read_only;
                self.catalog_rpc.did_open_document(
                    &path,
                    buffer.language_id.to_string(),
                    buffer.rev as i32,
                    content.clone(),
                );
                self.file_watcher.watch(&path, false, OPEN_FILE_EVENT_TOKEN);
                self.buffers.insert(path, buffer);
                self.respond_rpc(
                    id,
                    Ok(ProxyResponse::NewBufferResponse { content, read_only }),
                );
            }
            GitCommit { message, diffs } => {
                let host = self.ahead_host.as_ref().map(|host| host.read());
                let result = self
                    .workspace
                    .as_ref()
                    .ok_or_else(|| anyhow!("no workspace set"))
                    .and_then(|workspace| {
                        git_commit_attributed(
                            workspace,
                            &message,
                            diffs,
                            host.as_deref(),
                        )
                    })
                    .map(|_| ProxyResponse::Success {})
                    .map_err(|error| RpcError {
                        code: 0,
                        message: error.to_string(),
                    });
                self.respond_rpc(id, result);
            }
            GitFileState { path, content } => {
                let workspace = self.workspace.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                thread::spawn(move || {
                    let result = workspace
                        .ok_or_else(|| anyhow::anyhow!("no workspace set"))
                        .and_then(|workspace| {
                            git_file_state(&workspace, &path, &content)
                        })
                        .map(|state| ProxyResponse::GitFileState { state })
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                    proxy_rpc.handle_response(id, result);
                });
            }
            BufferHead { path } => {
                let result = if let Some(workspace) = self.workspace.as_ref() {
                    let result = file_get_head(workspace, &path);
                    if let Ok((_blob_id, content)) = result {
                        Ok(ProxyResponse::BufferHeadResponse {
                            version: "head".to_string(),
                            content,
                        })
                    } else {
                        Err(RpcError {
                            code: 0,
                            message: "can't get file head".to_string(),
                        })
                    }
                } else {
                    Err(RpcError {
                        code: 0,
                        message: "no workspace set".to_string(),
                    })
                };
                self.respond_rpc(id, result);
            }
            WorkspaceFiles { request_id } => {
                let index = self.file_index.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                let cancelled = Arc::new(AtomicBool::new(false));
                self.workspace_file_jobs
                    .lock()
                    .insert(request_id, cancelled.clone());
                let jobs = self.workspace_file_jobs.clone();
                thread::spawn(move || {
                    let response = match index {
                        None => Err(RpcError {
                            code: 0,
                            message: "no workspace set".to_string(),
                        }),
                        Some(index) => {
                            match index.snapshot_with_generation_while(|| {
                                !cancelled.load(Ordering::SeqCst)
                            }) {
                                Some((generation, files)) => {
                                    Ok(ProxyResponse::WorkspaceFilesResponse {
                                        generation,
                                        files: files.as_ref().clone(),
                                    })
                                }
                                None => Err(RpcError {
                                    code: 0,
                                    message: "workspace file request cancelled"
                                        .to_string(),
                                }),
                            }
                        }
                    };
                    jobs.lock().remove(&request_id);
                    proxy_rpc.handle_response(id, response);
                });
            }
            GlobalSearch {
                pattern,
                case_sensitive,
                whole_word,
                is_regex,
            } => {
                static WORKER_ID: AtomicU64 = AtomicU64::new(0);
                let our_id = WORKER_ID.fetch_add(1, Ordering::SeqCst) + 1;

                let workspace = self.workspace.clone();
                let file_index = self.file_index.clone();
                let buffers = self
                    .buffers
                    .iter()
                    .map(|(path, buffer)| (path.clone(), buffer.rope.clone()))
                    .collect::<HashMap<_, _>>();
                let proxy_rpc = self.proxy_rpc.clone();

                // Perform the search on another thread to avoid blocking the proxy thread
                thread::spawn(move || {
                    let overrides = buffers
                        .into_iter()
                        .map(|(path, rope)| (path, rope.to_string()))
                        .collect::<HashMap<_, _>>();
                    let indexed_files = if let Some(index) = file_index.as_ref() {
                        let Some((_, files)) =
                            index.snapshot_with_generation_while(|| {
                                WORKER_ID.load(Ordering::SeqCst) == our_id
                            })
                        else {
                            proxy_rpc.handle_response(
                                id,
                                Err(RpcError {
                                    code: 0,
                                    message: "expired search job".to_string(),
                                }),
                            );
                            return;
                        };
                        Some(files)
                    } else {
                        None
                    };
                    let paths: Box<dyn Iterator<Item = PathBuf> + '_> =
                        if let Some(root) = workspace.as_ref() {
                            let disk_paths: Box<dyn Iterator<Item = PathBuf> + '_> =
                                if let Some(files) = indexed_files.as_ref() {
                                    Box::new(files.iter().cloned())
                                } else {
                                    Box::new(ahead_core::search::workspace_paths(
                                        root,
                                    ))
                                };
                            Box::new(
                                ahead_core::search::new_open_buffer_paths(
                                    root, &overrides,
                                )
                                .into_iter()
                                .chain(disk_paths),
                            )
                        } else {
                            let mut open_paths =
                                overrides.keys().cloned().collect::<Vec<_>>();
                            open_paths.sort_unstable();
                            Box::new(open_paths.into_iter())
                        };
                    proxy_rpc.handle_response(
                        id,
                        search_in_path(
                            workspace.as_deref().map_or(
                                ahead_core::search::SearchScope::BuffersOnly,
                                ahead_core::search::SearchScope::Workspace,
                            ),
                            our_id,
                            &WORKER_ID,
                            paths,
                            &overrides,
                            &pattern,
                            case_sensitive,
                            whole_word,
                            is_regex,
                        ),
                    );
                });
            }
            CompletionResolve {
                plugin_id,
                completion_item,
            } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc.completion_resolve(
                    plugin_id,
                    *completion_item,
                    move |result| {
                        let result = result.map(|item| {
                            ProxyResponse::CompletionResolveResponse {
                                item: Box::new(item),
                            }
                        });
                        proxy_rpc.handle_response(id, result);
                    },
                );
            }
            GetHover {
                request_id,
                path,
                position,
            } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc.hover(&path, position, move |_, result| {
                    let result = result.map(|hover| ProxyResponse::HoverResponse {
                        request_id,
                        hover,
                    });
                    proxy_rpc.handle_response(id, result);
                });
            }
            GetSignature { .. } => {}
            GetReferences { path, position } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc.get_references(
                    &path,
                    position,
                    move |_, result| {
                        let result = result.map(|references| {
                            ProxyResponse::GetReferencesResponse { references }
                        });
                        proxy_rpc.handle_response(id, result);
                    },
                );
            }
            GitGetRemoteFileUrl { file } => {
                if let Some(workspace) = self.workspace.as_ref() {
                    match git_get_remote_file_url(workspace, &file) {
                        Ok(s) => self.proxy_rpc.handle_response(
                            id,
                            Ok(ProxyResponse::GitGetRemoteFileUrl { file_url: s }),
                        ),
                        Err(e) => eprintln!("{e:?}"),
                    }
                }
            }
            GetDefinition {
                request_id,
                path,
                position,
            } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc.get_definition(
                    &path,
                    position,
                    move |_, result| {
                        let result = result.map(|definition| {
                            ProxyResponse::GetDefinitionResponse {
                                request_id,
                                definition,
                            }
                        });
                        proxy_rpc.handle_response(id, result);
                    },
                );
            }
            GetTypeDefinition {
                request_id,
                path,
                position,
            } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc.get_type_definition(
                    &path,
                    position,
                    move |_, result| {
                        let result = result.map(|definition| {
                            ProxyResponse::GetTypeDefinition {
                                request_id,
                                definition,
                            }
                        });
                        proxy_rpc.handle_response(id, result);
                    },
                );
            }
            ShowCallHierarchy { path, position } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc.show_call_hierarchy(
                    &path,
                    position,
                    move |_, result| {
                        let result = result.map(|items| {
                            ProxyResponse::ShowCallHierarchyResponse { items }
                        });
                        proxy_rpc.handle_response(id, result);
                    },
                );
            }
            CallHierarchyIncoming {
                path,
                call_hierarchy_item,
            } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc.call_hierarchy_incoming(
                    &path,
                    call_hierarchy_item,
                    move |_, result| {
                        let result = result.map(|items| {
                            ProxyResponse::CallHierarchyIncomingResponse { items }
                        });
                        proxy_rpc.handle_response(id, result);
                    },
                );
            }
            GetInlayHints { path } => {
                let proxy_rpc = self.proxy_rpc.clone();
                let buffer = self.buffers.get(&path).unwrap();
                let range = Range {
                    start: Position::new(0, 0),
                    end: buffer.offset_to_position(buffer.len()),
                };
                self.catalog_rpc
                    .get_inlay_hints(&path, range, move |_, result| {
                        let result = result
                            .map(|hints| ProxyResponse::GetInlayHints { hints });
                        proxy_rpc.handle_response(id, result);
                    });
            }
            GetInlineCompletions {
                path,
                position,
                trigger_kind,
            } => {
                let proxy_rpc = self.proxy_rpc.clone();
                let ahead_host = self.ahead_host.clone();
                let core_rpc = self.core_rpc.clone();
                let shown_prediction_errors = self.shown_prediction_errors.clone();
                let buffer_snapshot = self.buffers.clone();
                let workspace = self.workspace.clone();
                let path_buf = path.clone();
                self.catalog_rpc.get_inline_completions(
                    &path,
                    position,
                    trigger_kind,
                    move |_, result| {
                        let mut completions_opt = result.ok();
                        let has_items = completions_opt
                            .as_ref()
                            .map(|completions| match completions {
                                lsp_types::InlineCompletionResponse::Array(
                                    items,
                                ) => !items.is_empty(),
                                lsp_types::InlineCompletionResponse::List(list) => {
                                    !list.items.is_empty()
                                }
                            })
                            .unwrap_or(false);

                        if !has_items {
                            if let Some(host_lock) = ahead_host {
                                if let Some(workspace) = workspace.as_deref()
                                    && is_prediction_target_visible(
                                        workspace,
                                        &path_buf,
                                    )
                                    && let Some(buffer) =
                                        buffer_snapshot.get(&path_buf)
                                {
                                    let active_text = buffer.rope.to_string();
                                    let (prefix, suffix) =
                                        prediction_prefix_suffix(
                                            &buffer.rope,
                                            position,
                                        );
                                    let open_buffers =
                                        relevant_open_buffer_contexts(
                                            &buffer_snapshot,
                                            workspace,
                                            &path_buf,
                                            &active_text,
                                        );
                                    let request =
                                        ahead_rpc::ahead::PredictionRequest {
                                            request_id: uuid::Uuid::new_v4()
                                                .to_string(),
                                            session_id: "active".to_string(),
                                            path: path_buf
                                                .to_string_lossy()
                                                .to_string(),
                                            cursor:
                                                ahead_rpc::ahead::DisplayPosition {
                                                    line: position.line,
                                                    col: position.character,
                                                },
                                            prefix,
                                            suffix,
                                            work_context:
                                                "ahead-editor-flow".to_string(),
                                        };
                                    let host = host_lock.read();
                                    match host.request_prediction(request, &open_buffers) {
                                        Ok(prediction) => {
                                            shown_prediction_errors.lock().clear();
                                            if !prediction.replacement.is_empty() {
                                                completions_opt = Some(
                                                    lsp_types::InlineCompletionResponse::Array(
                                                        vec![lsp_types::InlineCompletionItem {
                                                            insert_text: prediction.replacement,
                                                            filter_text: None,
                                                            range: None,
                                                            command: None,
                                                            insert_text_format: None,
                                                        }],
                                                    ),
                                                );
                                            }
                                        }
                                        Err(error) => {
                                            let message = format!("{error:#}");
                                            if shown_prediction_errors.lock().insert(message.clone()) {
                                                core_rpc.show_message(
                                                    "AHEAD edit prediction unavailable".into(),
                                                    ShowMessageParams {
                                                        typ: MessageType::WARNING,
                                                        message,
                                                    },
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        let final_result = Ok(ProxyResponse::GetInlineCompletions {
                            completions:
                                completions_opt.unwrap_or_else(|| {
                                    lsp_types::InlineCompletionResponse::Array(
                                        Vec::new(),
                                    )
                                }),
                        });
                        proxy_rpc.handle_response(id, final_result);
                    },
                );
            }
            GetSemanticTokens { path } => {
                let buffer = self.buffers.get(&path).unwrap();
                let text = buffer.rope.clone();
                let rev = buffer.rev;
                let len = buffer.len();
                let local_path = path.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                let catalog_rpc = self.catalog_rpc.clone();

                let handle_tokens =
                    move |result: Result<Vec<LineStyle>, RpcError>| match result {
                        Ok(styles) => {
                            proxy_rpc.handle_response(
                                id,
                                Ok(ProxyResponse::GetSemanticTokens {
                                    styles: SemanticStyles {
                                        rev,
                                        path: local_path,
                                        styles,
                                        len,
                                    },
                                }),
                            );
                        }
                        Err(e) => {
                            proxy_rpc.handle_response(id, Err(e));
                        }
                    };

                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc.get_semantic_tokens(
                    &path,
                    move |plugin_id, result| match result {
                        Ok(result) => {
                            catalog_rpc.format_semantic_tokens(
                                plugin_id,
                                result,
                                text,
                                Box::new(handle_tokens),
                            );
                        }
                        Err(e) => {
                            proxy_rpc.handle_response(id, Err(e));
                        }
                    },
                );
            }
            GetCodeActions {
                path,
                position,
                diagnostics,
            } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc.get_code_actions(
                    &path,
                    position,
                    diagnostics,
                    move |plugin_id, result| {
                        let result = result.map(|resp| {
                            ProxyResponse::GetCodeActionsResponse { plugin_id, resp }
                        });
                        proxy_rpc.handle_response(id, result);
                    },
                );
            }
            GetDocumentSymbols { path } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc
                    .get_document_symbols(&path, move |_, result| {
                        let result = result
                            .map(|resp| ProxyResponse::GetDocumentSymbols { resp });
                        proxy_rpc.handle_response(id, result);
                    });
            }
            GetWorkspaceSymbols { query } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc
                    .get_workspace_symbols(query, move |_, result| {
                        let result = result.map(|symbols| {
                            ProxyResponse::GetWorkspaceSymbols { symbols }
                        });
                        proxy_rpc.handle_response(id, result);
                    });
            }
            GetDocumentFormatting { path } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc
                    .get_document_formatting(&path, move |_, result| {
                        let result = result.map(|edits| {
                            ProxyResponse::GetDocumentFormatting { edits }
                        });
                        proxy_rpc.handle_response(id, result);
                    });
            }
            PrepareRename { path, position } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc.prepare_rename(
                    &path,
                    position,
                    move |_, result| {
                        let result =
                            result.map(|resp| ProxyResponse::PrepareRename { resp });
                        proxy_rpc.handle_response(id, result);
                    },
                );
            }
            Rename {
                path,
                position,
                new_name,
            } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc.rename(
                    &path,
                    position,
                    new_name,
                    move |_, result| {
                        let result =
                            result.map(|edit| ProxyResponse::Rename { edit });
                        proxy_rpc.handle_response(id, result);
                    },
                );
            }
            GetOpenFilesContent {} => {
                let items = self
                    .buffers
                    .iter()
                    .map(|(path, buffer)| TextDocumentItem {
                        uri: Url::from_file_path(path).unwrap(),
                        language_id: buffer.language_id.to_string(),
                        version: buffer.rev as i32,
                        text: buffer.get_document(),
                    })
                    .collect();
                let resp = ProxyResponse::GetOpenFilesContentResponse { items };
                self.proxy_rpc.handle_response(id, Ok(resp));
            }
            ReadDir { path } => {
                let proxy_rpc = self.proxy_rpc.clone();
                thread::spawn(move || {
                    let result = fs::read_dir(path)
                        .map(|entries| {
                            let mut items = entries
                                .into_iter()
                                .filter_map(|entry| {
                                    entry
                                        .map(|e| FileNodeItem {
                                            path: e.path(),
                                            is_dir: e.path().is_dir(),
                                            open: false,
                                            read: false,
                                            children: HashMap::new(),
                                            children_open_count: 0,
                                        })
                                        .ok()
                                })
                                .collect::<Vec<FileNodeItem>>();

                            items.sort();

                            ProxyResponse::ReadDirResponse { items }
                        })
                        .map_err(|e| RpcError {
                            code: 0,
                            message: e.to_string(),
                        });
                    proxy_rpc.handle_response(id, result);
                });
            }
            Save {
                rev,
                path,
                create_parents,
            } => {
                let result = if self.shared_remote_pending.contains_key(&path) {
                    Err(RpcError {
                        code: 0,
                        message: "Resolve the incoming shared edit before saving"
                            .into(),
                    })
                } else {
                    self.save_buffer(&path, rev, create_parents)
                        .and_then(|response| {
                            self.clear_shared_remote_recovery(&path)?;
                            Ok(response)
                        })
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        })
                };
                self.respond_rpc(id, result);
            }
            SaveEditorBuffer { path, content } => {
                let result =
                    self.save_editor_buffer(path, content).map_err(|error| {
                        RpcError {
                            code: 0,
                            message: error.to_string(),
                        }
                    });
                self.respond_rpc(id, result);
            }
            SaveBufferAs {
                buffer_id,
                path,
                rev,
                content,
                create_parents,
            } => {
                if self.shared_remote_pending.contains_key(&path) {
                    self.respond_rpc(
                        id,
                        Err(RpcError {
                            code: 0,
                            message:
                                "Resolve the incoming shared edit before saving"
                                    .into(),
                        }),
                    );
                    return;
                }
                let mut buffer = Buffer::new(buffer_id, path.clone());
                buffer.rope = Rope::from(content);
                buffer.rev = rev;
                let result = buffer
                    .save(rev, create_parents)
                    .and_then(|()| self.clear_shared_remote_recovery(&path))
                    .map(|_| ProxyResponse::Success {})
                    .map_err(|e| RpcError {
                        code: 0,
                        message: e.to_string(),
                    });
                self.buffers.insert(path, buffer);
                self.respond_rpc(id, result);
            }
            CreateFile { path } => {
                let result = path
                    .parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|()| {
                        std::fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(path)
                    })
                    .map(|_| ProxyResponse::Success {})
                    .map_err(|e| RpcError {
                        code: 0,
                        message: e.to_string(),
                    });
                self.respond_rpc(id, result);
            }
            CreateDirectory { path } => {
                let result = std::fs::create_dir_all(path)
                    .map(|_| ProxyResponse::Success {})
                    .map_err(|e| RpcError {
                        code: 0,
                        message: e.to_string(),
                    });
                self.respond_rpc(id, result);
            }
            TrashPath { path } => {
                let workspace = self.workspace.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                thread::spawn(move || {
                    let result = workspace
                        .ok_or_else(|| anyhow!("no workspace set"))
                        .and_then(|workspace| {
                            workspace_trash_target(&workspace, &path)
                        })
                        .and_then(|path| trash::delete(path).map_err(Into::into))
                        .map(|_| ProxyResponse::Success {})
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                    proxy_rpc.handle_response(id, result);
                });
            }
            DuplicatePath {
                existing_path,
                new_path,
            } => {
                // We first check if the destination already exists, because copy can overwrite it
                // and that's not the default behavior we want for when a user duplicates a document.
                let result = if new_path.exists() {
                    Err(RpcError {
                        code: 0,
                        message: format!("{new_path:?} already exists"),
                    })
                } else {
                    if let Some(parent) = new_path.parent() {
                        if let Err(error) = std::fs::create_dir_all(parent) {
                            let result = Err(RpcError {
                                code: 0,
                                message: error.to_string(),
                            });
                            self.respond_rpc(id, result);
                            return;
                        }
                    }
                    std::fs::copy(existing_path, new_path)
                        .map(|_| ProxyResponse::Success {})
                        .map_err(|e| RpcError {
                            code: 0,
                            message: e.to_string(),
                        })
                };
                self.respond_rpc(id, result);
            }
            RenamePath { from, to } => {
                // We first check if the destination already exists, because rename can overwrite it
                // and that's not the default behavior we want for when a user renames a document.
                let result = if to.exists() {
                    Err(format!("{} already exists", to.display()))
                } else {
                    Ok(())
                };

                let result = result.and_then(|_| {
                    if let Some(parent) = to.parent() {
                        fs::create_dir_all(parent).map_err(|e| {
                            if let io::ErrorKind::AlreadyExists = e.kind() {
                                format!(
                                    "{} has a parent that is not a directory",
                                    to.display()
                                )
                            } else {
                                e.to_string()
                            }
                        })
                    } else {
                        Ok(())
                    }
                });

                let result = result
                    .and_then(|_| fs::rename(&from, &to).map_err(|e| e.to_string()));

                let result = result
                    .map(|_| {
                        let to = to.canonicalize().unwrap_or(to);

                        let (is_dir, is_file) = to
                            .metadata()
                            .map(|metadata| (metadata.is_dir(), metadata.is_file()))
                            .unwrap_or((false, false));

                        if is_dir {
                            // Update all buffers in which a file the renamed directory is an
                            // ancestor of is open to use the file's new path.
                            // This could be written more nicely if `HashMap::extract_if` were
                            // stable.
                            let child_buffers: Vec<_> = self
                                .buffers
                                .keys()
                                .filter_map(|path| {
                                    path.strip_prefix(&from).ok().map(|suffix| {
                                        (path.clone(), suffix.to_owned())
                                    })
                                })
                                .collect();

                            for (path, suffix) in child_buffers {
                                if let Some(mut buffer) = self.buffers.remove(&path)
                                {
                                    let new_path = to.join(suffix);
                                    buffer.path = new_path;

                                    self.buffers.insert(buffer.path.clone(), buffer);
                                }
                            }
                        } else if is_file {
                            // If the renamed file is open in a buffer, update it to use the new
                            // path.
                            let buffer = self.buffers.remove(&from);

                            if let Some(mut buffer) = buffer {
                                buffer.path.clone_from(&to);
                                self.buffers.insert(to.clone(), buffer);
                            }
                        }

                        ProxyResponse::CreatePathResponse { path: to }
                    })
                    .map_err(|message| RpcError { code: 0, message });

                self.respond_rpc(id, result);
            }
            TestCreateAtPath { path } => {
                // This performs a best effort test to see if an attempt to create an item at
                // `path` or rename an item to `path` will succeed.
                // Currently the only conditions that are tested are that `path` doesn't already
                // exist and that `path` doesn't have a parent that exists and is not a directory.
                let result = if path.exists() {
                    Err(format!("{} already exists", path.display()))
                } else {
                    Ok(path)
                };

                let result = result
                    .and_then(|path| {
                        let parent_is_dir = path
                            .ancestors()
                            .skip(1)
                            .find(|parent| parent.exists())
                            .is_none_or(|parent| parent.is_dir());

                        if parent_is_dir {
                            Ok(ProxyResponse::Success {})
                        } else {
                            Err(format!(
                                "{} has a parent that is not a directory",
                                path.display()
                            ))
                        }
                    })
                    .map_err(|message| RpcError { code: 0, message });

                self.respond_rpc(id, result);
            }
            GetSelectionRange { positions, path } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc.get_selection_range(
                    path.as_path(),
                    positions,
                    move |_, result| {
                        let result = result.map(|ranges| {
                            ProxyResponse::GetSelectionRange { ranges }
                        });
                        proxy_rpc.handle_response(id, result);
                    },
                );
            }
            CodeActionResolve {
                action_item,
                plugin_id,
            } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc.action_resolve(
                    *action_item,
                    plugin_id,
                    move |result| {
                        let result = result.map(|item| {
                            ProxyResponse::CodeActionResolveResponse {
                                item: Box::new(item),
                            }
                        });
                        proxy_rpc.handle_response(id, result);
                    },
                );
            }
            DapVariable { dap_id, reference } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc
                    .dap_variable(dap_id, reference, move |result| {
                        proxy_rpc.handle_response(
                            id,
                            result.map(|resp| ProxyResponse::DapVariableResponse {
                                varialbes: resp,
                            }),
                        );
                    });
            }
            DapGetScopes { dap_id, frame_id } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc
                    .dap_get_scopes(dap_id, frame_id, move |result| {
                        proxy_rpc.handle_response(
                            id,
                            result.map(|resp| ProxyResponse::DapGetScopesResponse {
                                scopes: resp,
                            }),
                        );
                    });
            }
            GetCodeLens { path } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc
                    .get_code_lens(&path, move |plugin_id, result| {
                        let result = result.map(|resp| {
                            ProxyResponse::GetCodeLensResponse { plugin_id, resp }
                        });
                        proxy_rpc.handle_response(id, result);
                    });
            }
            LspFoldingRange { path } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc.get_lsp_folding_range(
                    &path,
                    move |plugin_id, result| {
                        let result = result.map(|resp| {
                            ProxyResponse::LspFoldingRangeResponse {
                                plugin_id,
                                resp,
                            }
                        });
                        proxy_rpc.handle_response(id, result);
                    },
                );
            }
            GetCodeLensResolve { code_lens, path } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc.get_code_lens_resolve(
                    &path,
                    &code_lens,
                    move |plugin_id, result| {
                        let result = result.map(|resp| {
                            ProxyResponse::GetCodeLensResolveResponse {
                                plugin_id,
                                resp,
                            }
                        });
                        proxy_rpc.handle_response(id, result);
                    },
                );
            }
            GotoImplementation { path, position } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc.go_to_implementation(
                    &path,
                    position,
                    move |plugin_id, result| {
                        let result = result.map(|resp| {
                            ProxyResponse::GotoImplementationResponse {
                                plugin_id,
                                resp,
                            }
                        });
                        proxy_rpc.handle_response(id, result);
                    },
                );
            }
            ReferencesResolve { items } => {
                let items: Vec<FileLine> = items
                    .into_iter()
                    .filter_map(|location| {
                        let Ok(path) = location.uri.to_file_path() else {
                            tracing::error!(
                                "get file path fail: {:?}",
                                location.uri
                            );
                            return None;
                        };
                        let buffer = self.get_buffer_or_insert(path.clone());
                        let line_num = location.range.start.line as usize;
                        let content = buffer.line_to_cow(line_num).to_string();
                        Some(FileLine {
                            path,
                            position: location.range.start,
                            content,
                        })
                    })
                    .collect();
                let resp = ProxyResponse::ReferencesResolveResponse { items };
                self.proxy_rpc.handle_response(id, Ok(resp));
            }
        }
    }
}

fn prediction_prefix_suffix(rope: &Rope, position: Position) -> (String, String) {
    let line_index = position.line as usize;
    if line_index >= rope.len_lines(ropey::LineType::LF_CR) {
        return (String::new(), String::new());
    }
    let line_start = rope.line_to_byte_idx(line_index, ropey::LineType::LF_CR);
    let line = rope.line(line_index, ropey::LineType::LF_CR).to_string();
    let line_content = line.trim_end_matches(['\r', '\n']);
    let column = offset_utf16_to_utf8_str(line_content, position.character as usize);
    let cursor_offset = line_start + column;
    (
        rope.slice(..cursor_offset).to_string(),
        rope.slice(cursor_offset..).to_string(),
    )
}

fn is_prediction_target_visible(workspace: &Path, path: &Path) -> bool {
    let Ok(workspace_root) = workspace.canonicalize() else {
        return false;
    };
    let relative = if path.is_absolute() {
        path.strip_prefix(&workspace_root)
            .or_else(|_| path.strip_prefix(workspace))
    } else {
        Ok(path)
    };
    let Ok(relative) = relative else {
        return false;
    };
    resolve_open_buffer_path(&workspace_root, relative).is_some_and(|resolved| {
        ahead_core::search::is_agent_visible_path(&workspace_root, &resolved)
    })
}

fn relevant_open_buffer_contexts(
    buffers: &HashMap<PathBuf, Buffer>,
    workspace: &Path,
    active_path: &Path,
    active_text: &str,
) -> Vec<OpenBufferContext> {
    let Ok(workspace_root) = workspace.canonicalize() else {
        return Vec::new();
    };
    let active_relative = active_path
        .strip_prefix(&workspace_root)
        .or_else(|_| active_path.strip_prefix(workspace));
    let Ok(active_relative) = active_relative else {
        return Vec::new();
    };
    let Some(active_path) =
        resolve_open_buffer_path(&workspace_root, active_relative)
    else {
        return Vec::new();
    };
    let Ok(active_relative) = active_path.strip_prefix(&workspace_root) else {
        return Vec::new();
    };
    let active_text = active_text.to_lowercase();

    let mut candidates = buffers
        .values()
        .filter_map(|buffer| {
            let relative = buffer
                .path
                .strip_prefix(&workspace_root)
                .or_else(|_| buffer.path.strip_prefix(workspace))
                .ok()?;
            let path = resolve_open_buffer_path(&workspace_root, relative)?;
            if !ahead_core::search::is_agent_visible_path(&workspace_root, &path) {
                return None;
            }
            let relative = path.strip_prefix(&workspace_root).ok()?.to_path_buf();
            if relative == active_relative {
                return None;
            }
            let stem = relative.file_stem()?.to_string_lossy().to_lowercase();
            let mentioned = stem.chars().count() >= 3 && active_text.contains(&stem);
            let sibling = relative.parent() == active_relative.parent();
            (mentioned || sibling).then_some((mentioned, relative, buffer))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(
        |(left_mentioned, left_path, _), (right_mentioned, right_path, _)| {
            right_mentioned
                .cmp(left_mentioned)
                .then_with(|| left_path.cmp(right_path))
        },
    );

    candidates
        .into_iter()
        .take(MAX_FIM_OPEN_BUFFERS)
        .map(|(_, path, buffer)| {
            let contents = buffer.rope.to_string();
            let excerpt = if contents.len() > MAX_FIM_OPEN_BUFFER_BYTES {
                let mut end = MAX_FIM_OPEN_BUFFER_BYTES;
                while !contents.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}\n[open-buffer excerpt truncated]", &contents[..end])
            } else {
                contents
            };
            OpenBufferContext {
                path: path
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/"),
                relevant_excerpt: excerpt,
            }
        })
        .collect()
}

impl Dispatcher {
    fn ensure_no_pending_shared_changes(&self, session_id: &str) -> Result<()> {
        let paths: Vec<_> = self
            .shared_remote_pending
            .iter()
            .filter_map(|(path, (pending_session, _))| {
                (pending_session == session_id).then(|| path.display().to_string())
            })
            .collect();
        anyhow::ensure!(
            paths.is_empty(),
            "Open and save collaborator edits before ending this share: {}",
            paths.join(", ")
        );
        Ok(())
    }

    fn close_editor_buffer(&mut self, path: &Path) {
        if self.shared_remote_pending.contains_key(path) {
            return;
        }
        if self.buffers.remove(path).is_some() {
            self.catalog_rpc.did_close_document(path.to_owned());
            let watched_path =
                path.canonicalize().unwrap_or_else(|_| path.to_owned());
            self.file_watcher
                .unwatch(&watched_path, OPEN_FILE_EVENT_TOKEN);
        }
    }

    fn sync_editor_snapshot(
        &mut self,
        path: PathBuf,
        content: String,
    ) -> Result<u64> {
        if let Some(buffer) = self.buffers.get_mut(&path) {
            if buffer.get_document() == content {
                return Ok(buffer.rev);
            }
            let old_text = buffer.rope.clone();
            let delta = AheadDelta::new(
                old_text.len(),
                vec![DeltaOp::Delete(old_text.len()), DeltaOp::Insert(content)],
            );
            let rev = buffer
                .rev
                .checked_add(1)
                .context("buffer revision overflow")?;
            buffer
                .update(&delta, rev)
                .context("editor snapshot revision mismatch")?;
            self.catalog_rpc.did_change_text_document(
                &path,
                rev,
                delta,
                old_text,
                buffer.rope.clone(),
            );
            Ok(rev)
        } else {
            let mut buffer = Buffer::new(BufferId::next(), path.clone());
            buffer.language_id =
                language_id_from_path_with_content(&path, Some(&content))
                    .unwrap_or_default();
            buffer.rope = Rope::from(content.clone());
            buffer.rev = u64::from(!content.is_empty());
            let rev = buffer.rev;
            self.catalog_rpc.did_open_document(
                &path,
                buffer.language_id.clone(),
                rev as i32,
                content,
            );
            self.file_watcher.watch(&path, false, OPEN_FILE_EVENT_TOKEN);
            self.buffers.insert(path, buffer);
            Ok(rev)
        }
    }

    fn shared_buffer_path(&self, relative: &str) -> Result<PathBuf> {
        anyhow::ensure!(relative.len() <= 1024, "Shared buffer path is too long");
        let relative = Path::new(relative);
        anyhow::ensure!(
            !is_private_file(relative)
                && !relative.components().any(|component| {
                    component.as_os_str().to_string_lossy().starts_with('.')
                }),
            "This file cannot be shared"
        );
        let workspace = self.workspace.as_ref().context("No workspace set")?;
        resolve_open_buffer_path(workspace, relative)
            .context("Shared file is outside the workspace or follows a link")
    }

    fn read_shared_buffer(
        &mut self,
        relative: &str,
    ) -> Result<SharedBufferSnapshot> {
        let path = self.shared_buffer_path(relative)?;
        if !self.buffers.contains_key(&path) {
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
                Err(error) => return Err(error.into()),
            };
            anyhow::ensure!(
                bytes.len() <= MAX_SHARED_BUFFER_BYTES
                    && !bytes.contains(&0)
                    && !is_binary_content(&bytes),
                "Shared file must be text smaller than 1 MiB"
            );
            let content = String::from_utf8(bytes)
                .context("Shared file must be UTF-8 text")?;
            self.sync_editor_snapshot(path.clone(), content)?;
        }
        let buffer = self
            .buffers
            .get(&path)
            .context("Shared buffer was closed")?;
        let content = buffer.get_document();
        anyhow::ensure!(
            content.len() <= MAX_SHARED_BUFFER_BYTES,
            "Shared file is larger than 1 MiB"
        );
        Ok(SharedBufferSnapshot {
            path: relative.to_string(),
            revision: buffer.rev,
            content,
        })
    }

    fn replace_shared_buffer(
        &mut self,
        session_id: &str,
        relative: &str,
        expected_revision: u64,
        content: String,
    ) -> Result<SharedBufferEditResult> {
        anyhow::ensure!(
            content.len() <= MAX_SHARED_BUFFER_BYTES && !content.contains('\0'),
            "Shared buffer must be text smaller than 1 MiB"
        );
        let current = self.read_shared_buffer(relative)?;
        if current.revision != expected_revision {
            return Ok(SharedBufferEditResult {
                applied: false,
                snapshot: current,
            });
        }
        let path = self.shared_buffer_path(relative)?;
        anyhow::ensure!(
            self.buffers
                .get(&path)
                .is_some_and(|buffer| !buffer.read_only),
            "Shared buffer is read-only"
        );
        self.persist_shared_remote_recovery(&path, &content)?;
        self.sync_editor_snapshot(path, content)?;
        let snapshot = self.read_shared_buffer(relative)?;
        let path = self.shared_buffer_path(relative)?;
        self.shared_remote_pending
            .insert(path, (session_id.to_string(), snapshot.clone()));
        self.core_rpc.ahead_notification(
            ahead_rpc::ahead::AheadNotification::SharedBufferChanged {
                session_id: session_id.to_string(),
                snapshot: snapshot.clone(),
                previous_content: current.content,
            },
        );
        Ok(SharedBufferEditResult {
            applied: true,
            snapshot,
        })
    }

    fn persist_shared_remote_recovery(
        &mut self,
        path: &Path,
        content: &str,
    ) -> Result<()> {
        let host = self
            .ahead_host
            .as_ref()
            .context("AHEAD session host is not initialized")?;
        let workspace = self.workspace.as_ref().context("No workspace set")?;
        let relative_path = path
            .strip_prefix(workspace)
            .context("Shared file is outside the workspace")?
            .to_path_buf();
        let (buffer_id, revision) =
            if let Some(record) = self.shared_remote_recovery.get(path) {
                (
                    record.buffer_id.clone(),
                    record
                        .revision
                        .checked_add(1)
                        .context("recovery revision overflow")?,
                )
            } else {
                (uuid::Uuid::new_v4().to_string(), 1)
            };
        let saved_sha256 = match fs::read(path) {
            Ok(bytes) => Some(format!("{:x}", Sha256::digest(bytes))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let snapshot = EditorRecoverySnapshot {
            buffer_id: buffer_id.clone(),
            revision,
            path: relative_path.clone(),
            contents: Some(content.to_string()),
            saved_sha256,
        };
        let stored: bool =
            serde_json::from_value(host.read().handle_request(
                AgentHostRequest::WriteEditorRecovery { snapshot },
            )?)?;
        anyhow::ensure!(stored, "Could not persist the incoming shared edit");
        self.shared_remote_recovery.insert(
            path.to_path_buf(),
            SharedRemoteRecovery {
                buffer_id,
                revision,
                relative_path,
            },
        );
        Ok(())
    }

    fn clear_shared_remote_recovery(&mut self, path: &Path) -> Result<()> {
        let Some(record) = self.shared_remote_recovery.get(path) else {
            return Ok(());
        };
        let host = self
            .ahead_host
            .as_ref()
            .context("AHEAD session host is not initialized")?;
        let snapshot = EditorRecoverySnapshot {
            buffer_id: record.buffer_id.clone(),
            revision: record
                .revision
                .checked_add(1)
                .context("recovery revision overflow")?,
            path: record.relative_path.clone(),
            contents: None,
            saved_sha256: None,
        };
        let stored: bool =
            serde_json::from_value(host.read().handle_request(
                AgentHostRequest::WriteEditorRecovery { snapshot },
            )?)?;
        anyhow::ensure!(stored, "Could not clear the saved shared edit recovery");
        self.shared_remote_recovery.remove(path);
        Ok(())
    }

    fn discard_shared_remote_recovery(&mut self, path: &Path) -> Result<()> {
        if self.shared_remote_recovery.contains_key(path) {
            anyhow::ensure!(
                !self.shared_remote_pending.contains_key(path),
                "Apply the incoming shared edit before discarding it"
            );
            anyhow::ensure!(
                self.shared_session_server.is_none(),
                "Stop sharing before discarding a collaborator edit"
            );
            self.clear_shared_remote_recovery(path)?;
        }
        Ok(())
    }

    fn save_buffer(
        &mut self,
        path: &Path,
        rev: u64,
        create_parents: bool,
    ) -> Result<ProxyResponse> {
        let buffer = self
            .buffers
            .get_mut(path)
            .context("file buffer is not open")?;
        buffer.save(rev, create_parents)?;
        self.catalog_rpc
            .did_save_text_document(path, buffer.rope.clone());
        Ok(ProxyResponse::SaveResponse {})
    }

    fn save_editor_buffer(
        &mut self,
        path: PathBuf,
        content: String,
    ) -> Result<ProxyResponse> {
        let acknowledging_shared_change =
            self.shared_remote_pending.contains_key(&path);
        if let Some(pending) = self.shared_remote_pending.get(&path) {
            anyhow::ensure!(
                pending.1.content == content,
                "Shared file changed; resolve the incoming edit before saving"
            );
        }
        let rev = self.sync_editor_snapshot(path.clone(), content)?;
        let response = self.save_buffer(&path, rev, false)?;
        self.clear_shared_remote_recovery(&path)?;
        if acknowledging_shared_change {
            self.shared_remote_pending.remove(&path);
        }
        Ok(response)
    }

    pub fn new(core_rpc: CoreRpcHandler, proxy_rpc: ProxyRpcHandler) -> Self {
        let plugin_rpc = PluginCatalogRpcHandler::new(core_rpc.clone());

        let file_watcher = FileWatcher::new();

        Self {
            workspace: None,
            proxy_rpc,
            core_rpc,
            catalog_rpc: plugin_rpc,
            catalog_stopped: None,
            buffers: HashMap::new(),
            shared_remote_pending: HashMap::new(),
            shared_remote_recovery: HashMap::new(),
            terminals: HashMap::new(),
            file_watcher,
            file_index: None,
            workspace_file_jobs: Arc::new(Mutex::new(HashMap::new())),
            shown_prediction_errors: Arc::new(Mutex::new(HashSet::new())),
            window_id: 1,
            tab_id: 1,
            ahead_host: None,
            shared_session_server: None,
            shared_session_client: Arc::new(Mutex::new(None)),
            #[cfg(feature = "test-support")]
            share_test_api_base: None,
            ahead_storage_error: None,
        }
    }

    #[cfg(feature = "test-support")]
    pub fn set_share_test_api_base(&mut self, api_base: String) {
        self.share_test_api_base = Some(api_base);
    }

    fn respond_rpc(&self, id: RequestId, result: Result<ProxyResponse, RpcError>) {
        self.proxy_rpc.handle_response(id, result);
    }

    fn get_buffer_or_insert(&mut self, path: PathBuf) -> &mut Buffer {
        self.buffers
            .entry(path.clone())
            .or_insert(Buffer::new(BufferId::next(), path))
    }
}

struct FileWatchNotifier {
    core_rpc: CoreRpcHandler,
    proxy_rpc: ProxyRpcHandler,
    catalog_rpc: PluginCatalogRpcHandler,
    workspace: Option<PathBuf>,
    file_index: Option<Arc<WorkspaceFileIndex>>,
    workspace_fs_change_handler: Arc<Mutex<Option<Sender<(bool, bool, bool)>>>>,
    git_metadata_paths: Vec<PathBuf>,
}

impl Notify for FileWatchNotifier {
    fn notify(&self, events: Vec<(WatchToken, notify::Event)>) {
        self.handle_fs_events(events);
    }
}

impl FileWatchNotifier {
    fn new(
        workspace: Option<PathBuf>,
        core_rpc: CoreRpcHandler,
        proxy_rpc: ProxyRpcHandler,
        catalog_rpc: PluginCatalogRpcHandler,
        file_index: Option<Arc<WorkspaceFileIndex>>,
    ) -> Self {
        let git_metadata_paths = workspace
            .as_ref()
            .and_then(|workspace| Repository::discover(workspace).ok())
            .map(|repo| {
                let mut paths = vec![repo.commondir().to_path_buf()];
                if !repo.path().starts_with(repo.commondir()) {
                    paths.push(repo.path().to_path_buf());
                }
                paths
            })
            .unwrap_or_default();
        let notifier = Self {
            workspace,
            file_index,
            core_rpc,
            proxy_rpc,
            catalog_rpc,
            workspace_fs_change_handler: Arc::new(Mutex::new(None)),
            git_metadata_paths,
        };

        if let Some(workspace) = notifier.workspace.clone() {
            let core_rpc = notifier.core_rpc.clone();
            thread::spawn(move || {
                if let Some(diff) = git_diff_new(&workspace) {
                    core_rpc.diff_info(diff.clone());
                }
            });
        }

        notifier
    }

    fn handle_fs_events(&self, events: Vec<(WatchToken, notify::Event)>) {
        for (token, event) in events {
            match token {
                OPEN_FILE_EVENT_TOKEN => self.handle_open_file_fs_event(event),
                WORKSPACE_EVENT_TOKEN => self.handle_workspace_fs_event(event),
                _ => {}
            }
        }
    }

    fn handle_open_file_fs_event(&self, event: notify::Event) {
        if event.kind.is_modify() || event.kind.is_remove() {
            for path in event.paths {
                #[cfg(windows)]
                if let Some(path_str) = path.to_str() {
                    const PREFIX: &str = r"\\?\";
                    if let Some(path_str) = path_str.strip_prefix(PREFIX) {
                        let path = PathBuf::from(&path_str);
                        self.proxy_rpc.notification(
                            ProxyNotification::OpenFileChanged { path },
                        );
                        continue;
                    }
                }
                self.proxy_rpc
                    .notification(ProxyNotification::OpenFileChanged { path });
            }
        }
    }

    fn handle_workspace_fs_event(&self, event: notify::Event) {
        let ignore_rule_change = event.paths.iter().any(|path| {
            matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some(".gitignore" | ".ignore")
            ) || path.ends_with(".git/info/exclude")
        });
        let file_set_change = match &event.kind {
            notify::EventKind::Create(_)
            | notify::EventKind::Remove(_)
            | notify::EventKind::Modify(notify::event::ModifyKind::Name(_)) => true,
            notify::EventKind::Modify(_) => ignore_rule_change,
            _ => return,
        };
        let search_relevant = ignore_rule_change
            || event.paths.is_empty()
            || self.file_index.as_ref().is_none_or(|index| {
                event
                    .paths
                    .iter()
                    .any(|path| index.affects_search(path, file_set_change))
            });
        let settings_relevant = self.workspace.as_ref().is_some_and(|workspace| {
            event
                .paths
                .iter()
                .any(|path| is_provider_settings_path(workspace, path))
        });
        let notify_relevant = search_relevant || settings_relevant;
        // Git can change without changing the file-status list (for example,
        // amend on the same branch). Invalidate buffer metadata on those events.
        let git_relevant = search_relevant
            || event.paths.iter().any(|path| {
                path.components()
                    .any(|component| component.as_os_str() == ".git")
                    || self
                        .git_metadata_paths
                        .iter()
                        .any(|root| path.starts_with(root))
            });
        if search_relevant {
            if let Some(index) = &self.file_index {
                if file_set_change {
                    index.invalidate();
                } else {
                    index.mark_content_changed();
                }
            }
        }

        let mut handler = self.workspace_fs_change_handler.lock();
        if let Some(sender) = handler.as_mut() {
            if let Err(err) =
                sender.send((notify_relevant, git_relevant, settings_relevant))
            {
                tracing::error!("{:?}", err);
            }
            return;
        }
        let (sender, receiver) = crossbeam_channel::unbounded();
        if let Err(err) =
            sender.send((notify_relevant, git_relevant, settings_relevant))
        {
            tracing::error!("{:?}", err);
        }

        let local_handler = self.workspace_fs_change_handler.clone();
        let core_rpc = self.core_rpc.clone();
        let catalog_rpc = self.catalog_rpc.clone();
        let file_index = self.file_index.clone();
        let workspace = self.workspace.clone().unwrap();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(500));

            {
                local_handler.lock().take();
            }

            let (notify_relevant, git_relevant, settings_relevant) =
                receiver.into_iter().fold(
                    (false, false, false),
                    |(notify, git, settings),
                     (next_notify, next_git, next_settings)| {
                        (
                            notify || next_notify,
                            git || next_git,
                            settings || next_settings,
                        )
                    },
                );
            if notify_relevant {
                let generation = file_index
                    .as_ref()
                    .map(|index| index.generation())
                    .unwrap_or_default();
                core_rpc.workspace_file_change(generation);
            }
            if git_relevant {
                core_rpc.diff_info(git_diff_new(&workspace).unwrap_or_default());
            }
            if settings_relevant
                && let Err(error) = catalog_rpc.catalog_notification(
                    crate::plugin::PluginCatalogNotification::RefreshWorkspaceConfigurations,
                )
            {
                tracing::error!(?error, "refreshing language-server settings");
            }
        });
        *handler = Some(sender);
    }
}

#[derive(Clone, Debug)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub header: String,
}

fn git_init(workspace_path: &Path) -> Result<()> {
    if Repository::discover(workspace_path).is_err() {
        Repository::init(workspace_path)?;
    };
    Ok(())
}

fn git_stage_all(workspace_path: &Path) -> Result<()> {
    let output = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(workspace_path)
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "Could not stage changes: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

fn git_commit(
    workspace_path: &Path,
    message: &str,
    diffs: Vec<FileDiff>,
    agent_session_ids: &[String],
) -> Result<()> {
    anyhow::ensure!(!message.trim().is_empty(), "Enter a commit message");
    let repo = Repository::discover(workspace_path)?;
    let mut index = repo.index()?;
    stage_diffs(&mut index, workspace_path, diffs)?;
    index.write()?;
    let tree = index.write_tree()?;
    let tree = repo.find_tree(tree)?;

    match repo.signature() {
        Ok(signature) => {
            let parents = match repo.head() {
                Ok(head) => vec![
                    head.peel_to_commit()
                        .context("HEAD does not point to a commit")?,
                ],
                Err(error)
                    if matches!(
                        error.code(),
                        NotFound | git2::ErrorCode::UnbornBranch
                    ) =>
                {
                    Vec::new()
                }
                Err(error) => return Err(error.into()),
            };
            let parents_refs = parents.iter().collect::<Vec<_>>();

            // Committer is always the human driving the commit. Author is
            // `ahead` when the changeset carries agent-authored regions, so
            // attribution is visible in `git log --author=ahead` / blame.
            let author = if !agent_session_ids.is_empty() {
                git2::Signature::now("ahead", "ahead@ahead.local")
                    .unwrap_or_else(|_| signature.clone())
            } else {
                signature.clone()
            };
            let commit_message = if agent_session_ids.is_empty() {
                message.to_string()
            } else {
                format!(
                    "{message}\n\nAhead-Session: {}",
                    agent_session_ids.join("\nAhead-Session: ")
                )
            };

            repo.commit(
                Some("HEAD"),
                &author,
                &signature,
                &commit_message,
                &tree,
                &parents_refs,
            )?;
            Ok(())
        }
        Err(e) => match e.code() {
            NotFound => Err(anyhow!(
                "No user.name and/or user.email configured for this git repository."
            )),
            _ => Err(anyhow!(
                "Error while creating commit's signature: {}",
                e.message()
            )),
        },
    }
}

fn stage_diffs(
    index: &mut git2::Index,
    workspace_path: &Path,
    diffs: Vec<FileDiff>,
) -> Result<()> {
    for diff in diffs {
        match diff {
            FileDiff::Modified(p) | FileDiff::Added(p) => {
                index.add_path(p.strip_prefix(workspace_path)?)?;
            }
            FileDiff::Renamed(old, new) => {
                index.add_path(new.strip_prefix(workspace_path)?)?;
                index.remove_path(old.strip_prefix(workspace_path)?)?;
            }
            FileDiff::Deleted(p) => {
                index.remove_path(p.strip_prefix(workspace_path)?)?;
            }
        }
    }
    Ok(())
}

fn git_commit_attributed(
    workspace_path: &Path,
    message: &str,
    diffs: Vec<FileDiff>,
    host: Option<&crate::ahead::host::AheadSessionHost>,
) -> Result<()> {
    anyhow::ensure!(!message.trim().is_empty(), "Enter a commit message");
    let repo = Repository::discover(workspace_path)?;
    let mut index = repo.index()?;
    stage_diffs(&mut index, workspace_path, diffs)?;
    index.write()?;
    let head_tree = match repo.head() {
        Ok(head) => Some(head.peel_to_tree()?),
        Err(error)
            if matches!(
                error.code(),
                git2::ErrorCode::NotFound | git2::ErrorCode::UnbornBranch
            ) =>
        {
            None
        }
        Err(error) => return Err(error.into()),
    };
    let staged = repo.diff_tree_to_index(head_tree.as_ref(), Some(&index), None)?;
    let mut committed_paths: Vec<String> = staged
        .deltas()
        .flat_map(|delta| [delta.old_file().path(), delta.new_file().path()])
        .flatten()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    committed_paths.sort();
    committed_paths.dedup();
    anyhow::ensure!(
        !committed_paths.is_empty(),
        "Stage changes before committing"
    );
    let committed_contents: HashMap<String, String> = committed_paths
        .iter()
        .filter_map(|path| {
            let entry = index.get_path(Path::new(path), 0)?;
            let blob = repo.find_blob(entry.id).ok()?;
            let content = std::str::from_utf8(blob.content()).ok()?;
            Some((path.clone(), content.to_string()))
        })
        .collect();
    let mut agent_session_ids = host
        .map(|host| host.anchors_for_paths(&committed_paths))
        .transpose()?
        .into_iter()
        .flatten()
        .filter(|anchor| {
            anchor.actor_id == ahead_rpc::ahead::AHEAD_ACTOR_ID
                && committed_contents.get(&anchor.path).is_some_and(|content| {
                    crate::ahead::store::anchor_matches_content(anchor, content)
                })
        })
        .map(|anchor| anchor.session_id)
        .collect::<Vec<_>>();
    agent_session_ids.sort();
    agent_session_ids.dedup();
    git_commit(workspace_path, message, Vec::new(), &agent_session_ids)?;
    if let Some(host) = host {
        if let Err(error) =
            host.clear_committed_anchors(&committed_paths, &committed_contents)
        {
            tracing::warn!("failed to clear committed anchors: {error}");
        }
    }
    Ok(())
}

fn git_checkout(workspace_path: &Path, reference: &str) -> Result<()> {
    let repo = Repository::discover(workspace_path)?;
    let (object, reference) = repo.revparse_ext(reference)?;
    repo.checkout_tree(&object, None)?;
    repo.set_head(reference.unwrap().name().unwrap())?;
    Ok(())
}

fn git_discard_files_changes<'a>(
    workspace_path: &Path,
    files: impl Iterator<Item = &'a Path>,
) -> Result<()> {
    let repo = Repository::discover(workspace_path)?;

    let mut checkout_b = CheckoutBuilder::new();
    checkout_b.update_only(false).force();

    let mut had_path = false;
    for path in files {
        // Remove the workspace path so it is relative to the folder
        if let Ok(path) = path.strip_prefix(workspace_path) {
            had_path = true;
            checkout_b.path(path);
        }
    }

    if !had_path {
        // If there we no paths then we do nothing
        // because the default behavior of checkout builder is to select all files
        // if it is not given a path
        return Ok(());
    }

    repo.checkout_index(None, Some(&mut checkout_b))?;

    Ok(())
}

fn git_discard_workspace_changes(workspace_path: &Path) -> Result<()> {
    let repo = Repository::discover(workspace_path)?;
    let mut checkout_b = CheckoutBuilder::new();
    checkout_b.force();

    repo.checkout_index(None, Some(&mut checkout_b))?;

    Ok(())
}

fn git_delta_format(
    workspace_path: &Path,
    delta: &git2::DiffDelta,
) -> Option<(git2::Delta, git2::Oid, PathBuf)> {
    match delta.status() {
        git2::Delta::Added | git2::Delta::Untracked => Some((
            git2::Delta::Added,
            delta.new_file().id(),
            delta.new_file().path().map(|p| workspace_path.join(p))?,
        )),
        git2::Delta::Deleted => Some((
            git2::Delta::Deleted,
            delta.old_file().id(),
            delta.old_file().path().map(|p| workspace_path.join(p))?,
        )),
        git2::Delta::Modified => Some((
            git2::Delta::Modified,
            delta.new_file().id(),
            delta.new_file().path().map(|p| workspace_path.join(p))?,
        )),
        _ => None,
    }
}

fn workspace_trash_target(workspace: &Path, path: &Path) -> Result<PathBuf> {
    anyhow::ensure!(
        path.is_absolute(),
        "Trash requires an absolute workspace path"
    );
    let workspace = fs::canonicalize(workspace)?;
    let name = path.file_name().context("Cannot trash a filesystem root")?;
    // Resolve the parent, not the entry: trashing a symlink must not trash its target.
    let parent =
        fs::canonicalize(path.parent().context("Trash path has no parent")?)?;
    let target = parent.join(name);
    anyhow::ensure!(
        target != workspace && target.starts_with(&workspace),
        "Cannot trash the workspace root or a path outside this workspace"
    );
    fs::symlink_metadata(&target)?;
    Ok(target)
}

fn git_file_state(
    workspace: &Path,
    path: &Path,
    content: &str,
) -> Result<GitFileState> {
    let repo = match Repository::discover(workspace) {
        Ok(repo) => repo,
        Err(error) if error.code() == git2::ErrorCode::NotFound => {
            return Ok(GitFileState::default());
        }
        Err(error) => return Err(error.into()),
    };
    let workdir = repo
        .workdir()
        .context("Git repository has no working directory")?;
    let workdir = fs::canonicalize(workdir)?;
    let path = fs::canonicalize(path)?;
    let relative = path.strip_prefix(&workdir)?;
    let head = match repo.head() {
        Ok(head) => Some(head.peel_to_commit()?),
        Err(error)
            if matches!(
                error.code(),
                git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
            ) =>
        {
            None
        }
        Err(error) => return Err(error.into()),
    };
    let tree = head.as_ref().map(|commit| commit.tree()).transpose()?;
    let blob = match tree.as_ref().map(|tree| tree.get_path(relative)) {
        Some(Ok(entry)) => Some(repo.find_blob(entry.id())?),
        Some(Err(error)) if error.code() != git2::ErrorCode::NotFound => {
            return Err(error.into());
        }
        _ => None,
    };
    if blob.as_ref().is_some_and(|blob| blob.is_binary())
        || (blob.is_none() && repo.status_should_ignore(relative)?)
    {
        return Ok(GitFileState::default());
    }

    // Like Zed's uncommitted diff, compare HEAD with the live buffer, not the
    // on-disk file or index: staging must not erase uncommitted gutter marks.
    let mut options = DiffOptions::new();
    options.context_lines(0);
    let patch = git2::Patch::from_buffers(
        blob.as_ref().map(|blob| blob.content()).unwrap_or_default(),
        Some(relative),
        content.as_bytes(),
        Some(relative),
        Some(&mut options),
    )?;
    let mut state = GitFileState::default();
    for index in 0..patch.num_hunks() {
        let (hunk, line_count) = patch.hunk(index)?;
        let mut old_lines = Vec::new();
        for line_index in 0..line_count {
            let line = patch.line_in_hunk(index, line_index)?;
            if line.origin() == '-' {
                old_lines.push(
                    String::from_utf8_lossy(line.content())
                        .trim_end_matches(['\r', '\n'])
                        .to_string(),
                );
            }
        }
        state.hunks.push(ahead_rpc::source_control::DiffHunk {
            start: hunk.new_start().max(1),
            len: hunk.new_lines(),
            old_start: hunk.old_start(),
            old_lines,
            kind: if hunk.old_lines() == 0 {
                DiffHunkKind::Added
            } else if hunk.new_lines() == 0 {
                DiffHunkKind::Deleted
            } else {
                DiffHunkKind::Modified
            },
        });
    }
    if blob.is_none() {
        if !content.is_empty() {
            state.blame.push(BlameHunk {
                start: 1,
                len: content.lines().count(),
                commit: None,
            });
        }
        return Ok(state);
    }
    let mut blame_options = git2::BlameOptions::new();
    if let Some(head) = &head {
        blame_options.newest_commit(head.id());
    }
    // ponytail: compute whole-file blame per coalesced revision; cache the HEAD
    // baseline if large-file/many-tab profiling shows this worker is costly.
    let committed = repo.blame_file(relative, Some(&mut blame_options))?;
    let live = committed.blame_buffer(content.as_bytes())?;
    let mut commits = HashMap::new();
    for hunk in live.iter() {
        let id = hunk.final_commit_id();
        let commit = if id.is_zero() {
            None
        } else {
            if let std::collections::hash_map::Entry::Vacant(entry) =
                commits.entry(id)
            {
                let commit = repo.find_commit(id)?;
                let author = commit.author();
                entry.insert(BlameCommit {
                    id: id.to_string(),
                    author: author.name().unwrap_or("Unknown author").to_string(),
                    timestamp: author.when().seconds(),
                    subject: commit
                        .summary()
                        .unwrap_or("No commit subject")
                        .to_string(),
                });
            }
            commits.get(&id).cloned()
        };
        state.blame.push(BlameHunk {
            start: hunk.final_start_line(),
            len: hunk.lines_in_hunk(),
            commit,
        });
    }
    Ok(state)
}

fn git_diff_new(workspace_path: &Path) -> Option<DiffInfo> {
    let repo = Repository::discover(workspace_path).ok()?;
    let name = match repo.head() {
        Ok(head) => head.shorthand()?.to_string(),
        _ => "(No branch)".to_owned(),
    };

    let mut branches = Vec::new();
    for branch in repo.branches(None).ok()? {
        branches.push(branch.ok()?.0.name().ok()??.to_string());
    }

    let mut tags = Vec::new();
    if let Ok(git_tags) = repo.tag_names(None) {
        for tag in git_tags.into_iter().flatten() {
            tags.push(tag.to_owned());
        }
    }

    let mut deltas = Vec::new();
    let mut diff_options = DiffOptions::new();
    let diff = repo
        .diff_index_to_workdir(
            None,
            Some(
                diff_options
                    .include_untracked(true)
                    .recurse_untracked_dirs(true),
            ),
        )
        .ok()?;
    for delta in diff.deltas() {
        if let Some(delta) = git_delta_format(workspace_path, &delta) {
            deltas.push(delta);
        }
    }

    let oid = match repo.revparse_single("HEAD^{tree}") {
        Ok(obj) => obj.id(),
        _ => Oid::zero(),
    };

    let cached_diff = repo
        .diff_tree_to_index(repo.find_tree(oid).ok().as_ref(), None, None)
        .ok();

    if let Some(cached_diff) = cached_diff {
        for delta in cached_diff.deltas() {
            if let Some(delta) = git_delta_format(workspace_path, &delta) {
                deltas.push(delta);
            }
        }
    }
    let mut renames = Vec::new();
    let mut renamed_deltas = HashSet::new();

    for (added_index, delta) in deltas.iter().enumerate() {
        if delta.0 == git2::Delta::Added {
            for (deleted_index, d) in deltas.iter().enumerate() {
                if d.0 == git2::Delta::Deleted && d.1 == delta.1 {
                    renames.push((added_index, deleted_index));
                    renamed_deltas.insert(added_index);
                    renamed_deltas.insert(deleted_index);
                    break;
                }
            }
        }
    }

    let mut file_diffs = Vec::new();
    for (added_index, deleted_index) in renames.iter() {
        file_diffs.push(FileDiff::Renamed(
            deltas[*added_index].2.clone(),
            deltas[*deleted_index].2.clone(),
        ));
    }
    for (i, delta) in deltas.iter().enumerate() {
        if renamed_deltas.contains(&i) {
            continue;
        }
        let diff = match delta.0 {
            git2::Delta::Added => FileDiff::Added(delta.2.clone()),
            git2::Delta::Deleted => FileDiff::Deleted(delta.2.clone()),
            git2::Delta::Modified => FileDiff::Modified(delta.2.clone()),
            _ => continue,
        };
        file_diffs.push(diff);
    }
    file_diffs.sort_by_key(|d| match d {
        FileDiff::Modified(p)
        | FileDiff::Added(p)
        | FileDiff::Renamed(p, _)
        | FileDiff::Deleted(p) => p.clone(),
    });
    Some(DiffInfo {
        head: name,
        branches,
        tags,
        diffs: file_diffs,
    })
}

fn file_get_head(workspace_path: &Path, path: &Path) -> Result<(String, String)> {
    let repo = Repository::discover(workspace_path)?;
    let head = repo.head()?;
    let tree = head.peel_to_tree()?;
    let tree_entry = tree.get_path(path.strip_prefix(workspace_path)?)?;
    let blob = repo.find_blob(tree_entry.id())?;
    let id = blob.id().to_string();
    let content = std::str::from_utf8(blob.content())
        .with_context(|| "content bytes to string")?
        .to_string();
    Ok((id, content))
}

fn git_get_remote_file_url(workspace_path: &Path, file: &Path) -> Result<String> {
    let repo = Repository::discover(workspace_path)?;
    let head = repo.head()?;
    let target_remote = repo.find_remote(
        repo.branch_upstream_remote(head.name().unwrap())?
            .as_str()
            .unwrap(),
    )?;

    // Grab URL part of remote
    let remote = target_remote
        .url()
        .ok_or(anyhow!("Failed to convert remote to str"))?;

    let remote_url = match Url::parse(remote) {
        Ok(url) => url,
        Err(_) => {
            // Parse URL as ssh
            Url::parse(&format!("ssh://{}", remote.replacen(':', "/", 1)))?
        }
    };

    // Get host part
    let host = remote_url
        .host_str()
        .ok_or(anyhow!("Couldn't find remote host"))?;
    // Get namespace (e.g. organisation/project in case of GitHub, org/team/team/team/../project on GitLab)
    let namespace = if let Some(stripped) = remote_url.path().strip_suffix(".git") {
        stripped
    } else {
        remote_url.path()
    };

    let commit = head.peel_to_commit()?.id();

    let file_path = file
        .strip_prefix(workspace_path)?
        .to_str()
        .ok_or(anyhow!("Couldn't convert file path to str"))?;

    let url = format!("https://{host}{namespace}/blob/{commit}/{file_path}",);

    Ok(url)
}

fn search_in_path(
    scope: ahead_core::search::SearchScope<'_>,
    id: u64,
    current_id: &AtomicU64,
    paths: impl Iterator<Item = PathBuf>,
    overrides: &HashMap<PathBuf, String>,
    pattern: &str,
    case_sensitive: bool,
    whole_word: bool,
    is_regex: bool,
) -> Result<ProxyResponse, RpcError> {
    let options = ahead_core::search::FileSearchOptions {
        pattern: pattern.to_string(),
        case_sensitive,
        whole_word,
        is_regex,
        max_results: 5_000,
    };
    let matches = ahead_core::search::search_paths_with_overrides(
        scope,
        paths,
        overrides,
        &options,
        || current_id.load(Ordering::SeqCst) == id,
    )
    .map_err(|error| RpcError {
        code: 0,
        message: error.to_string(),
    })?
    .into_iter()
    .map(|(path, line_matches)| {
        (
            path,
            line_matches
                .into_iter()
                .map(|line_match| SearchMatch {
                    line: line_match.line,
                    start: line_match.start,
                    end_line: line_match.end_line,
                    end: line_match.end,
                    line_content: line_match.line_content,
                })
                .collect(),
        )
    })
    .collect::<IndexMap<_, _>>();
    Ok(ProxyResponse::GlobalSearchResponse { matches })
}

#[cfg(test)]
mod tests {
    use super::{
        AHEAD_GITIGNORE, Buffer, Dispatcher, FileWatchNotifier,
        ensure_ahead_gitignore, git_commit, is_prediction_target_visible,
        is_provider_settings_path, prediction_prefix_suffix,
        relevant_open_buffer_contexts, search_in_path,
    };
    use crate::plugin::PluginCatalogRpcHandler;
    use ahead_core::search::WorkspaceFileIndex;
    use ahead_rpc::{
        ahead::{AheadRequest, DisplayPosition, DisplayRange},
        buffer::BufferId,
        core::{CoreNotification, CoreRpc, CoreRpcHandler},
        file::{EditorRecoverySnapshot, EditorRecoverySummary},
        proxy::{
            ProxyHandler, ProxyNotification, ProxyRequest, ProxyResponse, ProxyRpc,
            ProxyRpcHandler,
        },
        source_control::FileDiff,
    };
    use git2::Repository;
    use ropey::Rope;
    use std::{
        collections::HashMap,
        fs,
        sync::Arc,
        sync::atomic::{AtomicBool, AtomicU64, Ordering},
        time::Duration,
    };

    fn git_metadata_fixture() -> (tempfile::TempDir, Repository, std::path::PathBuf)
    {
        let directory = tempfile::tempdir().expect("Git fixture");
        let repo = Repository::init(directory.path()).expect("init fixture");
        let path = directory.path().join("main.ts");
        fs::write(&path, "first\nsecond\nthird\n").expect("source");
        let mut index = repo.index().expect("index");
        index
            .add_path(std::path::Path::new("main.ts"))
            .expect("add source");
        index.write().expect("write index");
        let tree_id = index.write_tree().expect("tree");
        let signature =
            git2::Signature::now("Ada", "ada@example.invalid").expect("signature");
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            "Initial source",
            &repo.find_tree(tree_id).expect("tree"),
            &[],
        )
        .expect("commit");
        (directory, repo, path)
    }

    #[test]
    fn trash_target_rejects_workspace_root_external_paths_and_symlink_escapes() {
        let workspace = tempfile::tempdir().expect("workspace");
        let external = tempfile::tempdir().expect("external directory");
        let file = workspace.path().join("main.py");
        fs::write(&file, "keep").expect("source");
        assert_eq!(
            super::workspace_trash_target(workspace.path(), &file).expect("child"),
            fs::canonicalize(&file).expect("canonical file")
        );
        assert!(
            super::workspace_trash_target(workspace.path(), workspace.path())
                .is_err()
        );
        assert!(
            super::workspace_trash_target(workspace.path(), external.path())
                .is_err()
        );
        assert!(
            super::workspace_trash_target(
                workspace.path(),
                std::path::Path::new("main.py")
            )
            .is_err()
        );
        #[cfg(unix)]
        {
            let link = workspace.path().join("linked");
            std::os::unix::fs::symlink(external.path(), &link)
                .expect("directory link");
            let outside = external.path().join("keep.py");
            fs::write(&outside, "keep external").expect("external source");
            assert!(
                super::workspace_trash_target(
                    workspace.path(),
                    &link.join("keep.py")
                )
                .is_err()
            );
            assert_eq!(
                super::workspace_trash_target(workspace.path(), &link)
                    .expect("link entry"),
                fs::canonicalize(workspace.path())
                    .expect("root")
                    .join("linked")
            );
            assert_eq!(
                fs::read_to_string(outside).expect("external source retained"),
                "keep external"
            );
        }
        assert_eq!(fs::read_to_string(file).expect("source retained"), "keep");
    }

    #[test]
    fn shared_buffers_use_live_host_text_and_reject_stale_or_private_edits() {
        let workspace = tempfile::tempdir().expect("workspace");
        fs::write(workspace.path().join("main.rs"), "old\n").expect("source");
        fs::create_dir(workspace.path().join(".ahead")).expect("private directory");
        fs::write(
            workspace.path().join(".ahead/settings.toml"),
            "token = 'secret'",
        )
        .expect("private settings");
        let mut dispatcher =
            Dispatcher::new(CoreRpcHandler::new(), ProxyRpcHandler::new());
        dispatcher.workspace = Some(workspace.path().to_path_buf());
        let host = Arc::new(parking_lot::RwLock::new(
            crate::ahead::host::AheadSessionHost::in_memory().expect("session host"),
        ));
        host.read().set_workspace(workspace.path().to_path_buf());
        dispatcher.ahead_host = Some(host.clone());
        let initial = dispatcher
            .read_shared_buffer("main.rs")
            .expect("open source");
        let edited = dispatcher
            .replace_shared_buffer(
                "session",
                "main.rs",
                initial.revision,
                "guest edit\n".into(),
            )
            .expect("edit source");
        assert!(edited.applied);
        assert_eq!(edited.snapshot.content, "guest edit\n");
        let recoveries: Vec<EditorRecoverySummary> = serde_json::from_value(
            host.read()
                .handle_request(AheadRequest::ListEditorRecoveries)
                .expect("list incoming edit recovery"),
        )
        .expect("recovery summaries");
        assert_eq!(recoveries.len(), 1);
        let recovery: Option<EditorRecoverySnapshot> = serde_json::from_value(
            host.read()
                .handle_request(AheadRequest::ReadEditorRecovery {
                    buffer_id: recoveries[0].buffer_id.clone(),
                })
                .expect("read incoming edit recovery"),
        )
        .expect("recovery snapshot");
        assert_eq!(
            recovery
                .expect("recoverable shared edit")
                .contents
                .as_deref(),
            Some("guest edit\n")
        );
        let stale = dispatcher
            .replace_shared_buffer(
                "session",
                "main.rs",
                initial.revision,
                "stale edit\n".into(),
            )
            .expect("report conflict");
        assert!(!stale.applied);
        assert_eq!(stale.snapshot, edited.snapshot);
        assert!(
            dispatcher
                .ensure_no_pending_shared_changes("session")
                .is_err()
        );
        let path = workspace.path().join("main.rs");
        assert!(dispatcher.discard_shared_remote_recovery(&path).is_err());
        dispatcher.handle_notification(ProxyNotification::EditorSnapshot {
            path: path.clone(),
            content: "host edit\n".into(),
        });
        assert_eq!(
            dispatcher
                .read_shared_buffer("main.rs")
                .expect("remote preserved")
                .content,
            "guest edit\n"
        );
        dispatcher.handle_notification(ProxyNotification::EditorSnapshot {
            path: path.clone(),
            content: "guest edit\n".into(),
        });
        dispatcher
            .ensure_no_pending_shared_changes("session")
            .expect("host acknowledged edit");
        dispatcher.handle_notification(ProxyNotification::EditorSnapshot {
            path,
            content: "host edit\n".into(),
        });
        assert_eq!(
            dispatcher
                .read_shared_buffer("main.rs")
                .expect("live text")
                .content,
            "host edit\n"
        );
        dispatcher
            .discard_shared_remote_recovery(&workspace.path().join("main.rs"))
            .expect("discard acknowledged remote edit");
        let recoveries: Vec<EditorRecoverySummary> = serde_json::from_value(
            host.read()
                .handle_request(AheadRequest::ListEditorRecoveries)
                .expect("list discarded recovery"),
        )
        .expect("recovery summaries");
        assert!(recoveries.is_empty());
        assert_eq!(
            fs::read_to_string(workspace.path().join("main.rs"))
                .expect("disk was not written"),
            "old\n"
        );
        let before_save = dispatcher
            .read_shared_buffer("main.rs")
            .expect("read host text");
        let next_edit = dispatcher
            .replace_shared_buffer(
                "session",
                "main.rs",
                before_save.revision,
                "another guest edit\n".into(),
            )
            .expect("edit before save");
        dispatcher.close_editor_buffer(&workspace.path().join("main.rs"));
        assert!(
            dispatcher
                .ensure_no_pending_shared_changes("session")
                .is_err()
        );
        assert_eq!(
            dispatcher
                .read_shared_buffer("main.rs")
                .expect("remote text survives host tab close"),
            next_edit.snapshot
        );
        let mut permissions = fs::metadata(workspace.path().join("main.rs"))
            .expect("source metadata")
            .permissions();
        permissions.set_readonly(true);
        fs::set_permissions(workspace.path().join("main.rs"), permissions.clone())
            .expect("make source read only");
        assert!(
            dispatcher
                .save_editor_buffer(
                    workspace.path().join("main.rs"),
                    next_edit.snapshot.content.clone(),
                )
                .is_err()
        );
        assert!(
            dispatcher
                .ensure_no_pending_shared_changes("session")
                .is_err()
        );
        permissions.set_readonly(false);
        fs::set_permissions(workspace.path().join("main.rs"), permissions)
            .expect("restore source permissions");
        dispatcher
            .save_editor_buffer(
                workspace.path().join("main.rs"),
                next_edit.snapshot.content.clone(),
            )
            .expect("save collaborator edit");
        dispatcher
            .ensure_no_pending_shared_changes("session")
            .expect("save acknowledged edit");
        let recoveries: Vec<EditorRecoverySummary> = serde_json::from_value(
            host.read()
                .handle_request(AheadRequest::ListEditorRecoveries)
                .expect("list after save"),
        )
        .expect("recovery summaries");
        assert!(recoveries.is_empty());
        assert!(
            dispatcher
                .read_shared_buffer(".ahead/settings.toml")
                .is_err()
        );
        assert!(dispatcher.read_shared_buffer("../outside.rs").is_err());
        assert_eq!(
            fs::read_to_string(workspace.path().join("main.rs")).expect("disk"),
            "another guest edit\n"
        );
    }

    #[test]
    fn incoming_shared_edit_survives_host_process_restart() {
        let workspace = tempfile::tempdir().expect("workspace");
        fs::create_dir(workspace.path().join(".ahead")).expect("private directory");
        let database = workspace.path().join(".ahead/session.db");
        fs::write(workspace.path().join("main.rs"), "disk\n").expect("source");
        {
            let host = Arc::new(parking_lot::RwLock::new(
                crate::ahead::host::AheadSessionHost::new(
                    crate::ahead::store::SessionStore::open(&database)
                        .expect("session store"),
                ),
            ));
            host.read().set_workspace(workspace.path().to_path_buf());
            let mut dispatcher =
                Dispatcher::new(CoreRpcHandler::new(), ProxyRpcHandler::new());
            dispatcher.workspace = Some(workspace.path().to_path_buf());
            dispatcher.ahead_host = Some(host);
            let initial = dispatcher
                .read_shared_buffer("main.rs")
                .expect("read host buffer");
            assert!(
                dispatcher
                    .replace_shared_buffer(
                        "session",
                        "main.rs",
                        initial.revision,
                        "guest unsaved\n".into(),
                    )
                    .expect("accept edit")
                    .applied
            );
        }
        assert_eq!(
            fs::read_to_string(workspace.path().join("main.rs"))
                .expect("unchanged disk"),
            "disk\n"
        );
        let host = crate::ahead::host::AheadSessionHost::new(
            crate::ahead::store::SessionStore::open(&database)
                .expect("reopen session store"),
        );
        host.set_workspace(workspace.path().to_path_buf());
        let recoveries: Vec<EditorRecoverySummary> = serde_json::from_value(
            host.handle_request(AheadRequest::ListEditorRecoveries)
                .expect("reclaim abandoned recovery"),
        )
        .expect("recovery summaries");
        assert_eq!(recoveries.len(), 1);
        let recovery: Option<EditorRecoverySnapshot> = serde_json::from_value(
            host.handle_request(AheadRequest::ReadEditorRecovery {
                buffer_id: recoveries[0].buffer_id.clone(),
            })
            .expect("read recovered edit"),
        )
        .expect("recovery snapshot");
        assert_eq!(
            recovery
                .expect("incoming edit was retained")
                .contents
                .as_deref(),
            Some("guest unsaved\n")
        );
    }

    #[test]
    fn incoming_shared_edit_requires_durable_recovery_before_acceptance() {
        let workspace = tempfile::tempdir().expect("workspace");
        fs::write(workspace.path().join("main.rs"), "disk\n").expect("source");
        let mut dispatcher =
            Dispatcher::new(CoreRpcHandler::new(), ProxyRpcHandler::new());
        dispatcher.workspace = Some(workspace.path().to_path_buf());
        dispatcher.ahead_host = Some(Arc::new(parking_lot::RwLock::new(
            crate::ahead::host::AheadSessionHost::in_memory().expect("session host"),
        )));
        let initial = dispatcher
            .read_shared_buffer("main.rs")
            .expect("read host buffer");
        assert!(
            dispatcher
                .replace_shared_buffer(
                    "session",
                    "main.rs",
                    initial.revision,
                    "guest unsaved\n".into(),
                )
                .is_err()
        );
        assert_eq!(
            dispatcher
                .read_shared_buffer("main.rs")
                .expect("host buffer unchanged"),
            initial
        );
    }

    #[test]
    fn git_metadata_uses_live_buffer_and_head_including_staged_edits() {
        use ahead_rpc::source_control::DiffHunkKind;
        let (directory, repo, path) = git_metadata_fixture();
        let live = "first\nunsaved\nthird\nadded\n";
        let state =
            super::git_file_state(directory.path(), &path, live).expect("metadata");
        let rpc = ProxyRpcHandler::new();
        let mut dispatcher = Dispatcher::new(CoreRpcHandler::new(), rpc.clone());
        dispatcher.workspace = Some(directory.path().to_owned());
        let (sender, receiver) = crossbeam_channel::bounded(1);
        rpc.request_async(
            ProxyRequest::GitFileState {
                path: path.clone(),
                content: live.to_string(),
            },
            move |result| {
                sender.send(result).expect("RPC result");
            },
        );
        let ProxyRpc::Request(id, request) =
            rpc.rx().try_recv().expect("RPC request")
        else {
            panic!("RPC request")
        };
        dispatcher.handle_request(id, request);
        let ProxyResponse::GitFileState { state: from_rpc } = receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("background response")
            .expect("metadata response")
        else {
            panic!("metadata response")
        };
        assert_eq!(from_rpc, state);
        assert_eq!(state.hunks.len(), 2);
        assert_eq!(
            (
                state.hunks[0].start,
                state.hunks[0].len,
                state.hunks[0].kind
            ),
            (2, 1, DiffHunkKind::Modified)
        );
        assert_eq!(state.hunks[0].old_lines, ["second"]);
        assert_eq!(
            (
                state.hunks[1].start,
                state.hunks[1].len,
                state.hunks[1].kind
            ),
            (4, 1, DiffHunkKind::Added)
        );
        assert_eq!(
            state.blame[0]
                .commit
                .as_ref()
                .expect("committed line")
                .author,
            "Ada"
        );
        assert!(
            state
                .blame
                .iter()
                .any(|hunk| hunk.start == 2 && hunk.commit.is_none())
        );
        assert_eq!(
            fs::read_to_string(&path).expect("unchanged disk"),
            "first\nsecond\nthird\n"
        );
        fs::write(&path, live).expect("save fixture");
        let mut index = repo.index().expect("index");
        index
            .add_path(std::path::Path::new("main.ts"))
            .expect("stage source");
        index.write().expect("write index");
        assert_eq!(
            super::git_file_state(directory.path(), &path, live)
                .expect("staged metadata"),
            state
        );

        let deleted =
            super::git_file_state(directory.path(), &path, "second\nthird\n")
                .expect("delete metadata");
        assert_eq!(
            (
                deleted.hunks[0].start,
                deleted.hunks[0].len,
                deleted.hunks[0].kind
            ),
            (1, 0, DiffHunkKind::Deleted)
        );
        assert_eq!(deleted.hunks[0].old_lines, ["first"]);
        assert_eq!(
            deleted.blame[0]
                .commit
                .as_ref()
                .expect("shifted original line")
                .author,
            "Ada"
        );
        let clean =
            super::git_file_state(directory.path(), &path, "first\nsecond\nthird\n")
                .expect("clean snapshot");
        assert!(clean.hunks.is_empty());
        assert!(clean.blame.iter().all(|hunk| hunk.commit.is_some()));
    }

    #[test]
    fn git_metadata_handles_new_empty_ignored_and_non_repository_files() {
        let directory = tempfile::tempdir().expect("non repository");
        let path = directory.path().join("new.py");
        fs::write(&path, "").expect("new file");
        assert_eq!(
            super::git_file_state(directory.path(), &path, "new\n").expect("no Git"),
            Default::default()
        );
        Repository::init(directory.path()).expect("unborn repository");
        let state = super::git_file_state(directory.path(), &path, "new\n")
            .expect("new file metadata");
        assert_eq!(
            state.hunks[0].kind,
            ahead_rpc::source_control::DiffHunkKind::Added
        );
        assert!(state.blame[0].commit.is_none());
        assert_eq!(
            super::git_file_state(directory.path(), &path, "").expect("empty file"),
            Default::default()
        );
        fs::write(directory.path().join(".gitignore"), "new.py\n")
            .expect("ignore rule");
        assert_eq!(
            super::git_file_state(directory.path(), &path, "new\n")
                .expect("ignored file"),
            Default::default()
        );
    }

    #[test]
    fn git_metadata_tracks_linked_worktree_and_subdirectory_repository_paths() {
        let (_directory, repo, _) = git_metadata_fixture();
        let linked = tempfile::tempdir().expect("worktree parent");
        let root = linked.path().join("checkout");
        let worktree = repo
            .worktree("linked", &root, None)
            .expect("linked worktree");
        let linked_repo =
            Repository::open_from_worktree(&worktree).expect("linked repository");
        let subdirectory = root.join("src");
        fs::create_dir(&subdirectory).expect("subdirectory");
        for workspace in [&root, &subdirectory] {
            let notifier = FileWatchNotifier::new(
                Some(workspace.clone()),
                CoreRpcHandler::new(),
                ProxyRpcHandler::new(),
                PluginCatalogRpcHandler::new(CoreRpcHandler::new()),
                None,
            );
            assert!(
                notifier
                    .git_metadata_paths
                    .iter()
                    .any(|path| linked_repo.path().starts_with(path))
            );
            assert!(
                notifier
                    .git_metadata_paths
                    .iter()
                    .any(|path| linked_repo.commondir().starts_with(path))
            );
            assert!(
                notifier
                    .git_metadata_paths
                    .iter()
                    .all(|path| !path.starts_with(workspace))
            );
            let state = super::git_file_state(
                workspace,
                &root.join("main.ts"),
                "first\nunsaved\nthird\n",
            )
            .expect("linked metadata");
            assert_eq!(state.hunks[0].start, 2);
            assert!(
                state
                    .blame
                    .iter()
                    .any(|hunk| hunk.start == 2 && hunk.commit.is_none())
            );
        }
    }

    #[test]
    fn git_metadata_refreshes_when_external_amend_keeps_file_status_unchanged() {
        let (directory, repo, path) = git_metadata_fixture();
        let core = CoreRpcHandler::new();
        let notifier = FileWatchNotifier::new(
            Some(directory.path().to_owned()),
            core.clone(),
            ProxyRpcHandler::new(),
            PluginCatalogRpcHandler::new(CoreRpcHandler::new()),
            None,
        );
        let initial = core
            .rx()
            .recv_timeout(Duration::from_secs(2))
            .expect("initial status");
        let CoreRpc::Notification(initial) = initial else {
            panic!("initial notification")
        };
        let CoreNotification::DiffInfo { diff: before } = *initial else {
            panic!("initial diff")
        };
        let commit = repo.head().expect("HEAD").peel_to_commit().expect("commit");
        let author = git2::Signature::now("Grace", "grace@example.invalid")
            .expect("new author");
        commit
            .amend(
                Some("HEAD"),
                Some(&author),
                Some(&author),
                None,
                Some("Amended source"),
                None,
            )
            .expect("external amend");
        notifier.handle_workspace_fs_event(
            notify::Event::new(notify::EventKind::Modify(
                notify::event::ModifyKind::Data(notify::event::DataChange::Content),
            ))
            .add_path(repo.path().join("HEAD")),
        );
        let after = loop {
            let event = core
                .rx()
                .recv_timeout(Duration::from_secs(2))
                .expect("repository invalidation");
            if let CoreRpc::Notification(event) = event
                && let CoreNotification::DiffInfo { diff } = *event
            {
                break diff;
            }
        };
        assert_eq!(before, after, "the status summary itself did not change");
        let state =
            super::git_file_state(directory.path(), &path, "first\nsecond\nthird\n")
                .expect("amended metadata");
        assert_eq!(
            state.blame[0].commit.as_ref().expect("new blame").author,
            "Grace"
        );
    }

    #[test]
    fn restarting_language_servers_reclassifies_open_buffers() {
        let workspace = tempfile::tempdir().expect("test project");
        let path = workspace.path().join("main.py");
        let saved = "print('hello')\n";
        let unsaved = "print('unsaved')\n";
        fs::write(&path, saved).expect("source");
        let expected_language =
            crate::buffer::language_id_from_path_with_content(&path, Some(unsaved))
                .expect("language ID");
        let mut dispatcher =
            Dispatcher::new(CoreRpcHandler::new(), ProxyRpcHandler::new());
        let mut buffer = Buffer::new(BufferId::next(), path.clone());
        buffer.language_id = "unknown".into();
        dispatcher.buffers.insert(path.clone(), buffer);
        let notifications = dispatcher.catalog_rpc.test_receiver();
        let revision = dispatcher
            .sync_editor_snapshot(path.clone(), unsaved.into())
            .expect("unsaved editor snapshot");

        dispatcher.handle_notification(ProxyNotification::RestartLanguageServers {});

        let documents = notifications
            .try_iter()
            .find_map(|notification| match notification {
                crate::plugin::PluginCatalogRpc::Handler(
                    crate::plugin::PluginCatalogNotification::RestartLanguageServers {
                        documents,
                    },
                ) => Some(documents),
                _ => None,
            })
            .expect("restart notification");
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].language_id, expected_language);
        assert_eq!(documents[0].text, unsaved);
        assert_eq!(documents[0].version, revision as i32);
        assert_eq!(fs::read_to_string(path).expect("saved source"), saved);
    }

    #[test]
    fn session_storage_failure_disables_agent_requests_without_disabling_editor() {
        let workspace = tempfile::tempdir().expect("test project");
        let ahead = workspace.path().join(".ahead");
        fs::create_dir(&ahead).expect("private directory");
        let database = ahead.join("session.db");
        fs::write(&database, b"not a database").expect("invalid database");
        let source = workspace.path().join("main.rs");
        fs::write(&source, "original").expect("source");
        let rpc = ProxyRpcHandler::new();
        let core = CoreRpcHandler::new();
        let mut dispatcher = Dispatcher::new(core.clone(), rpc.clone());
        dispatcher.ahead_host = Some(Arc::new(parking_lot::RwLock::new(
            crate::ahead::host::AheadSessionHost::new(
                crate::ahead::store::SessionStore::in_memory()
                    .expect("previous host"),
            ),
        )));
        dispatcher.handle_notification(ProxyNotification::Initialize {
            workspace: Some(workspace.path().to_path_buf()),
            window_id: 1,
            tab_id: 1,
        });
        assert!(
            dispatcher.ahead_host.is_none(),
            "must not create a non-durable fallback"
        );
        assert!(core.rx().try_iter().any(|message| match message {
            CoreRpc::Notification(notification) => matches!(
                *notification,
                CoreNotification::ShowMessage { message, .. }
                    if message.message.contains("Agent sessions are disabled")
            ),
            _ => false,
        }));
        let (sender, receiver) = crossbeam_channel::bounded(1);
        rpc.request_async(
            ProxyRequest::AheadRequest {
                request: ahead_rpc::ahead::AheadRequest::ListSessions,
            },
            move |result| sender.send(result).expect("agent response"),
        );
        let ProxyRpc::Request(id, request) =
            rpc.rx().try_recv().expect("agent request")
        else {
            panic!("expected agent request");
        };
        dispatcher.handle_request(id, request);
        let error = receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("agent response")
            .expect_err(
                "agent requests must fail without durable workspace storage",
            );
        assert!(error.message.contains("Agent sessions are disabled"));
        let revision = dispatcher
            .sync_editor_snapshot(source.clone(), "edited".into())
            .expect("editor still accepts changes");
        dispatcher
            .save_buffer(&source, revision, false)
            .expect("editor still saves");
        assert_eq!(fs::read_to_string(source).expect("saved source"), "edited");
        assert_eq!(
            fs::read(database).expect("database retained"),
            b"not a database"
        );
    }

    #[test]
    fn slow_agent_prepare_does_not_block_editor_rpc() {
        let rpc = ProxyRpcHandler::new();
        let mut dispatcher = Dispatcher::new(CoreRpcHandler::new(), rpc.clone());
        let host = Arc::new(parking_lot::RwLock::new(
            crate::ahead::host::AheadSessionHost::new(
                crate::ahead::store::SessionStore::in_memory()
                    .expect("session store"),
            ),
        ));
        dispatcher.ahead_host = Some(host.clone());
        let (held_sender, held_receiver) = crossbeam_channel::bounded(1);
        let (release_sender, release_receiver) = crossbeam_channel::bounded::<()>(1);
        let released = Arc::new(AtomicBool::new(false));
        let released_for_thread = released.clone();
        let blocker = std::thread::spawn(move || {
            let guard = host.write();
            held_sender.send(()).expect("host lock acquired");
            match release_receiver.recv_timeout(Duration::from_secs(3)) {
                Ok(()) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {}
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    panic!("agent preparation blocked the proxy thread")
                }
            }
            drop(guard);
            released_for_thread.store(true, Ordering::Release);
        });
        held_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("host lock held");

        let (prepare_sender, prepare_receiver) = crossbeam_channel::bounded(1);
        rpc.request_async(
            ProxyRequest::AheadRequest {
                request: AheadRequest::AgentSessionPrepare {
                    session_id: "disposable-session".into(),
                },
            },
            move |result| prepare_sender.send(result).expect("prepare response"),
        );
        let ProxyRpc::Request(id, request) =
            rpc.rx().try_recv().expect("prepare request")
        else {
            panic!("expected prepare request");
        };
        dispatcher.handle_request(id, request);
        let prepare_is_pending = prepare_receiver.try_recv().is_err();
        let host_is_held = !released.load(Ordering::Acquire);

        let (editor_sender, editor_receiver) = crossbeam_channel::bounded(1);
        rpc.request_async(ProxyRequest::GetOpenFilesContent {}, move |result| {
            editor_sender.send(result).expect("editor response")
        });
        let ProxyRpc::Request(id, request) =
            rpc.rx().try_recv().expect("editor request")
        else {
            panic!("expected editor request");
        };
        dispatcher.handle_request(id, request);
        let editor_response = editor_receiver.recv_timeout(Duration::from_secs(2));
        drop(release_sender);
        blocker.join().expect("release host lock");

        assert!(host_is_held, "agent preparation blocked the proxy thread");
        assert!(
            prepare_is_pending,
            "agent preparation bypassed the host lock"
        );
        assert!(matches!(
            editor_response,
            Ok(Ok(ProxyResponse::GetOpenFilesContentResponse { items })) if items.is_empty()
        ));
        assert!(
            prepare_receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("prepare response after host release")
                .is_err()
        );
    }

    #[test]
    fn closing_editor_buffer_discards_only_its_unsaved_snapshot() {
        let directory = tempfile::tempdir().expect("disposable project");
        let path = directory.path().join("main.ts");
        fs::write(&path, "saved text").expect("source");
        let other = directory.path().join("other.ts");
        fs::write(&other, "other saved text").expect("other source");
        let mut dispatcher =
            Dispatcher::new(CoreRpcHandler::new(), ProxyRpcHandler::new());
        dispatcher
            .sync_editor_snapshot(path.clone(), "unsaved text".into())
            .expect("snapshot");
        dispatcher
            .sync_editor_snapshot(other.clone(), "other unsaved text".into())
            .expect("other snapshot");
        dispatcher.handle_notification(ProxyNotification::CloseEditorBuffer {
            path: path.clone(),
        });
        assert!(!dispatcher.buffers.contains_key(&path));
        assert_eq!(
            dispatcher.buffers[&other].get_document(),
            "other unsaved text"
        );
        assert_eq!(fs::read_to_string(&path).expect("disk"), "saved text");
        dispatcher.close_editor_buffer(&path);
        dispatcher
            .sync_editor_snapshot(path.clone(), "saved text".into())
            .expect("reopen");
        assert_eq!(dispatcher.buffers[&path].get_document(), "saved text");
    }

    #[test]
    fn editor_save_synchronizes_the_requested_snapshot_before_writing() {
        let directory = tempfile::tempdir().expect("test project");
        let path = directory.path().join("main.rs");
        fs::write(&path, "on disk").expect("source");
        let rpc = ProxyRpcHandler::new();
        let mut dispatcher = Dispatcher::new(CoreRpcHandler::new(), rpc.clone());
        dispatcher
            .sync_editor_snapshot(path.clone(), "older unsaved text".into())
            .expect("sync initial buffer");
        let (sender, receiver) = crossbeam_channel::bounded(1);
        rpc.request_async(
            ProxyRequest::SaveEditorBuffer {
                path: path.clone(),
                content: "latest text 🦀\n".into(),
            },
            move |result| sender.send(result).expect("save response"),
        );
        let ProxyRpc::Request(id, request) =
            rpc.rx().try_recv().expect("save request")
        else {
            panic!("expected save request");
        };
        dispatcher.handle_request(id, request);
        assert!(matches!(
            receiver.try_recv().expect("save response"),
            Ok(ProxyResponse::SaveResponse {})
        ));
        assert_eq!(
            fs::read_to_string(&path).expect("saved source"),
            "latest text 🦀\n"
        );
        assert_eq!(dispatcher.buffers[&path].get_document(), "latest text 🦀\n");
        assert!(
            dispatcher
                .save_buffer(&directory.path().join("not-open.rs"), 0, false)
                .is_err()
        );
    }

    #[test]
    fn prediction_cursor_splits_the_live_document_at_utf16_position() {
        let rope = Rope::from("a😀b\nnext line");

        let (prefix, suffix) = prediction_prefix_suffix(
            &rope,
            lsp_types::Position {
                line: 0,
                character: 3,
            },
        );

        assert_eq!(prefix, "a😀");
        assert_eq!(suffix, "b\nnext line");
    }

    #[test]
    fn prediction_context_selects_related_unsaved_workspace_buffers() {
        let temp = tempfile::tempdir().expect("workspace");
        let workspace = temp.path();
        fs::create_dir_all(workspace.join("src")).expect("create source directory");
        fs::create_dir_all(workspace.join("types")).expect("create types directory");
        fs::create_dir_all(workspace.join("unused"))
            .expect("create unrelated directory");
        let active_path = workspace.join("src/main.rs");
        let sibling_path = workspace.join("src/sibling.rs");
        let mentioned_path = workspace.join("types/request.rs");
        let large_path = workspace.join("src/large.rs");
        let private_path = workspace.join("src/.env.local");
        let unrelated_path = workspace.join("unused/notes.rs");
        for path in [
            &active_path,
            &sibling_path,
            &mentioned_path,
            &large_path,
            &private_path,
            &unrelated_path,
        ] {
            fs::write(path, "on disk").expect("create source file");
        }
        assert!(!is_prediction_target_visible(workspace, &private_path));

        let mut buffers = HashMap::new();
        buffers.insert(
            active_path.clone(),
            Buffer::new(BufferId::next(), active_path.clone()),
        );
        let mut sibling = Buffer::new(BufferId::next(), sibling_path.clone());
        sibling.rope = Rope::from("unsaved sibling source");
        buffers.insert(sibling_path, sibling);
        let mut mentioned = Buffer::new(BufferId::next(), mentioned_path.clone());
        mentioned.rope = Rope::from("unsaved request definition");
        buffers.insert(mentioned_path, mentioned);
        let mut large = Buffer::new(BufferId::next(), large_path.clone());
        large.rope = Rope::from("x".repeat(5000));
        buffers.insert(large_path, large);
        buffers.insert(
            private_path.clone(),
            Buffer::new(BufferId::next(), private_path),
        );
        buffers.insert(
            unrelated_path.clone(),
            Buffer::new(BufferId::next(), unrelated_path),
        );

        let context = relevant_open_buffer_contexts(
            &buffers,
            workspace,
            &active_path,
            "mod sibling;\nmod large;\nuse crate::request::Request;",
        );

        assert_eq!(context.len(), 3);
        assert!(context.iter().any(|buffer| {
            buffer.path == "src/sibling.rs"
                && buffer.relevant_excerpt == "unsaved sibling source"
        }));
        assert!(context.iter().any(|buffer| {
            buffer.path == "types/request.rs"
                && buffer.relevant_excerpt == "unsaved request definition"
        }));
        assert!(context.iter().any(|buffer| {
            buffer.path == "src/large.rs"
                && buffer
                    .relevant_excerpt
                    .ends_with("[open-buffer excerpt truncated]")
        }));
        assert!(
            !context
                .iter()
                .any(|buffer| buffer.path == "unused/notes.rs")
        );
        assert!(!context.iter().any(|buffer| buffer.path == "src/.env.local"));
    }

    #[test]
    fn cancel_workspace_files_cancels_only_the_matching_request() {
        let mut dispatcher =
            Dispatcher::new(CoreRpcHandler::new(), ProxyRpcHandler::new());
        let first = Arc::new(AtomicBool::new(false));
        let second = Arc::new(AtomicBool::new(false));
        dispatcher
            .workspace_file_jobs
            .lock()
            .insert(41, first.clone());
        dispatcher
            .workspace_file_jobs
            .lock()
            .insert(42, second.clone());

        ProxyHandler::handle_notification(
            &mut dispatcher,
            ProxyNotification::CancelWorkspaceFiles { request_id: 41 },
        );

        assert!(first.load(Ordering::SeqCst));
        assert!(!second.load(Ordering::SeqCst));
    }

    #[test]
    fn ignore_rule_change_invalidates_proxy_file_index() {
        let workspace = std::env::temp_dir()
            .join(format!("ahead-proxy-index-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&workspace).expect("create workspace");
        let ignore_file = workspace.join(".gitignore");
        let ignored = workspace.join("ignored.rs");
        fs::write(&ignore_file, "ignored.rs\n").expect("write ignore rule");
        fs::write(&ignored, "needle\n").expect("write ignored file");
        let index = Arc::new(WorkspaceFileIndex::new(workspace.clone()));
        assert!(!index.snapshot().contains(&ignored));
        let notifier = FileWatchNotifier::new(
            Some(workspace.clone()),
            CoreRpcHandler::new(),
            ProxyRpcHandler::new(),
            PluginCatalogRpcHandler::new(CoreRpcHandler::new()),
            Some(index.clone()),
        );

        fs::write(&ignore_file, "").expect("clear ignore rule");
        notifier.handle_workspace_fs_event(
            notify::Event::new(notify::EventKind::Modify(
                notify::event::ModifyKind::Data(notify::event::DataChange::Content),
            ))
            .add_path(ignore_file),
        );
        assert!(index.snapshot().contains(&ignored));
        fs::remove_dir_all(workspace).expect("remove workspace");
    }

    #[test]
    fn content_change_notifies_search_without_invalidating_paths() {
        let workspace = std::env::temp_dir()
            .join(format!("ahead-proxy-content-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&workspace).expect("create workspace");
        let path = workspace.join("example.rs");
        fs::write(&path, "before\n").expect("write source");
        let index = Arc::new(WorkspaceFileIndex::new(workspace.clone()));
        assert!(index.snapshot().contains(&path));
        let core_rpc = CoreRpcHandler::new();
        let notifier = FileWatchNotifier::new(
            Some(workspace.clone()),
            core_rpc.clone(),
            ProxyRpcHandler::new(),
            PluginCatalogRpcHandler::new(CoreRpcHandler::new()),
            Some(index.clone()),
        );

        fs::write(&path, "after\n").expect("update source");
        notifier.handle_workspace_fs_event(
            notify::Event::new(notify::EventKind::Modify(
                notify::event::ModifyKind::Data(notify::event::DataChange::Content),
            ))
            .add_path(path),
        );
        assert_eq!(index.generation(), 0);
        assert!(matches!(
            core_rpc.rx().recv_timeout(Duration::from_secs(2)),
            Ok(CoreRpc::Notification(notification))
                if matches!(*notification, CoreNotification::WorkspaceFileChange { generation: 0 })
        ));
        fs::remove_dir_all(workspace).expect("remove workspace");
    }

    #[test]
    fn private_provider_change_notifies_without_reindexing_workspace() {
        let workspace = tempfile::tempdir().expect("test workspace");
        let ahead = workspace.path().join(".ahead");
        fs::create_dir(&ahead).expect("provider settings directory");
        let config = ahead.join("config.toml");
        fs::write(&config, "[ai]\nmodel = 'before'\n")
            .expect("initial provider settings");
        let index =
            Arc::new(WorkspaceFileIndex::new(workspace.path().to_path_buf()));
        assert!(!index.snapshot().contains(&config));
        assert!(is_provider_settings_path(workspace.path(), &config));
        assert!(!is_provider_settings_path(
            workspace.path(),
            &ahead.join("unrelated.toml")
        ));
        let core_rpc = CoreRpcHandler::new();
        let catalog_rpc = PluginCatalogRpcHandler::new(CoreRpcHandler::new());
        let catalog_receiver = catalog_rpc.test_receiver();
        let notifier = FileWatchNotifier::new(
            Some(workspace.path().to_path_buf()),
            core_rpc.clone(),
            ProxyRpcHandler::new(),
            catalog_rpc,
            Some(index.clone()),
        );

        fs::write(&config, "[ai]\nmodel = 'after'\n")
            .expect("updated provider settings");
        notifier.handle_workspace_fs_event(
            notify::Event::new(notify::EventKind::Modify(
                notify::event::ModifyKind::Data(notify::event::DataChange::Content),
            ))
            .add_path(config),
        );
        assert_eq!(index.generation(), 0);
        assert!(matches!(
            core_rpc.rx().recv_timeout(Duration::from_secs(2)),
            Ok(CoreRpc::Notification(notification))
                if matches!(*notification, CoreNotification::WorkspaceFileChange { generation: 0 })
        ));
        assert!(matches!(
            catalog_receiver.recv_timeout(Duration::from_secs(2)),
            Ok(crate::plugin::PluginCatalogRpc::Handler(
                crate::plugin::PluginCatalogNotification::RefreshWorkspaceConfigurations
            ))
        ));
    }

    #[test]
    fn ignored_build_output_does_not_invalidate_search_index() {
        let workspace = std::env::temp_dir()
            .join(format!("ahead-proxy-ignored-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(workspace.join("src")).expect("create source directory");
        fs::create_dir_all(workspace.join("target"))
            .expect("create ignored directory");
        fs::write(workspace.join(".gitignore"), "target/\n")
            .expect("write ignore rule");
        let ignored = workspace.join("target/output.o");
        fs::write(&ignored, "old").expect("write ignored output");
        let index = Arc::new(WorkspaceFileIndex::new(workspace.clone()));
        assert!(!index.snapshot().contains(&ignored));
        let core_rpc = CoreRpcHandler::new();
        let notifier = FileWatchNotifier::new(
            Some(workspace.clone()),
            core_rpc.clone(),
            ProxyRpcHandler::new(),
            PluginCatalogRpcHandler::new(CoreRpcHandler::new()),
            Some(index.clone()),
        );

        let new_ignored = workspace.join("target/new.o");
        fs::write(&new_ignored, "new").expect("write new ignored output");
        notifier.handle_workspace_fs_event(
            notify::Event::new(notify::EventKind::Create(
                notify::event::CreateKind::File,
            ))
            .add_path(new_ignored),
        );
        notifier.handle_workspace_fs_event(
            notify::Event::new(notify::EventKind::Modify(
                notify::event::ModifyKind::Data(notify::event::DataChange::Content),
            ))
            .add_path(ignored),
        );
        assert_eq!(index.generation(), 0);
        assert!(
            core_rpc
                .rx()
                .recv_timeout(Duration::from_millis(700))
                .is_err()
        );

        let visible = workspace.join("src/new.rs");
        fs::write(&visible, "new").expect("write visible source");
        notifier.handle_workspace_fs_event(
            notify::Event::new(notify::EventKind::Create(
                notify::event::CreateKind::File,
            ))
            .add_path(visible),
        );
        assert_eq!(index.generation(), 1);
        assert!(matches!(
            core_rpc.rx().recv_timeout(Duration::from_secs(2)),
            Ok(CoreRpc::Notification(notification))
                if matches!(*notification, CoreNotification::WorkspaceFileChange { generation: 1 })
        ));
        fs::remove_dir_all(workspace).expect("remove workspace");
    }

    #[test]
    fn global_search_uses_open_buffer_content() {
        let workspace = std::env::temp_dir()
            .join(format!("ahead-proxy-search-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&workspace).expect("create workspace");
        let path = workspace.join("open.rs");
        fs::write(&path, "only on disk\n").expect("write disk file");
        let overrides =
            HashMap::from([(path.clone(), "only in buffer\n".to_string())]);
        let generation = AtomicU64::new(1);

        let response = search_in_path(
            ahead_core::search::SearchScope::Workspace(&workspace),
            1,
            &generation,
            std::iter::once(path.clone()),
            &overrides,
            "buffer",
            true,
            false,
            false,
        )
        .expect("search should succeed");
        let ProxyResponse::GlobalSearchResponse { matches } = response else {
            panic!("expected global search results");
        };
        assert_eq!(matches[&path][0].start, 8);

        fs::remove_dir_all(workspace).expect("remove workspace");
    }

    #[test]
    fn global_search_keeps_new_buffers_inside_workspace_and_respects_ignores() {
        let temporary = std::env::temp_dir()
            .join(format!("ahead-proxy-search-paths-{}", uuid::Uuid::new_v4()));
        let workspace = temporary.join("workspace");
        fs::create_dir_all(&workspace).expect("create workspace");
        fs::write(workspace.join(".gitignore"), "ignored.rs\n")
            .expect("write ignore rules");
        fs::write(workspace.join("ignored.rs"), "disk text\n")
            .expect("write ignored file");
        let new_path = workspace.join("new.rs");
        let overrides = HashMap::from([
            (new_path.clone(), "needle in new buffer\n".to_string()),
            (workspace.join("ignored.rs"), "needle ignored\n".to_string()),
            (temporary.join("outside.rs"), "needle outside\n".to_string()),
        ]);
        let paths =
            ahead_core::search::new_open_buffer_paths(&workspace, &overrides)
                .into_iter()
                .chain(ahead_core::search::workspace_paths(&workspace));
        let generation = AtomicU64::new(1);

        let response = search_in_path(
            ahead_core::search::SearchScope::Workspace(&workspace),
            1,
            &generation,
            paths,
            &overrides,
            "needle",
            true,
            false,
            false,
        )
        .expect("search should succeed");
        let ProxyResponse::GlobalSearchResponse { matches } = response else {
            panic!("expected global search results");
        };
        assert_eq!(matches.len(), 1);
        assert!(matches.contains_key(&new_path));
        fs::remove_dir_all(temporary).expect("remove workspace");
    }

    #[test]
    fn initializes_ahead_gitignore_without_overwriting_project_rules() {
        let workspace = std::env::temp_dir()
            .join(format!("ahead-gitignore-test-{}", uuid::Uuid::new_v4()));
        ensure_ahead_gitignore(&workspace).unwrap();
        let path = workspace.join(".ahead/.gitignore");
        assert_eq!(fs::read_to_string(&path).unwrap(), AHEAD_GITIGNORE);

        fs::write(&path, "custom project rules\n").unwrap();
        ensure_ahead_gitignore(&workspace).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "custom project rules\n");
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn agent_changes_use_ahead_author_and_human_committer() {
        let workspace = std::env::temp_dir()
            .join(format!("ahead-git-commit-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(workspace.join("src")).unwrap();
        let repo = Repository::init(&workspace).unwrap();
        {
            let mut config = repo.config().unwrap();
            config.set_str("user.name", "Human Developer").unwrap();
            config.set_str("user.email", "human@example.test").unwrap();
        }
        let path = workspace.join("src/lib.rs");
        fs::write(&path, "pub fn retry() {}\n").unwrap();

        git_commit(
            &workspace,
            "Implement retry",
            vec![FileDiff::Added(path)],
            &["session-1".to_string()],
        )
        .unwrap();

        let repo = Repository::open(&workspace).unwrap();
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(commit.author().name(), Some("ahead"));
        assert_eq!(commit.author().email(), Some("ahead@ahead.local"));
        assert_eq!(commit.committer().name(), Some("Human Developer"));
        assert!(
            commit
                .message()
                .unwrap()
                .contains("Ahead-Session: session-1")
        );
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn rename_commit_stages_destination_and_removes_source() {
        let workspace = std::env::temp_dir()
            .join(format!("ahead-git-rename-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&workspace).unwrap();
        let repo = Repository::init(&workspace).unwrap();
        {
            let mut config = repo.config().unwrap();
            config.set_str("user.name", "Human Developer").unwrap();
            config.set_str("user.email", "human@example.test").unwrap();
        }

        let old = workspace.join("old.md");
        let new = workspace.join("new.md");
        fs::write(&old, "working document\n").unwrap();
        git_commit(
            &workspace,
            "Add working document",
            vec![FileDiff::Added(old.clone())],
            &[],
        )
        .unwrap();
        fs::rename(&old, &new).unwrap();

        git_commit(
            &workspace,
            "Rename working document",
            vec![FileDiff::Renamed(old.clone(), new.clone())],
            &[],
        )
        .unwrap();

        let repo = Repository::open(&workspace).unwrap();
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        let tree = commit.tree().unwrap();
        assert!(tree.get_path(std::path::Path::new("new.md")).is_ok());
        assert!(tree.get_path(std::path::Path::new("old.md")).is_err());
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn commit_rejects_head_that_is_not_a_commit() {
        let workspace = std::env::temp_dir().join(format!(
            "ahead-git-invalid-head-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&workspace).unwrap();
        let repo = Repository::init(&workspace).unwrap();
        {
            let mut config = repo.config().unwrap();
            config.set_str("user.name", "Human Developer").unwrap();
            config.set_str("user.email", "human@example.test").unwrap();
        }
        let path = workspace.join("file.txt");
        fs::write(&path, "before\n").unwrap();
        git_commit(
            &workspace,
            "Initial commit",
            vec![FileDiff::Added(path.clone())],
            &[],
        )
        .unwrap();
        let head = repo.head().unwrap();
        let branch = head.name().unwrap().to_string();
        let tree_id = head.peel_to_commit().unwrap().tree_id();
        repo.reference(&branch, tree_id, true, "test invalid HEAD")
            .unwrap();

        fs::write(&path, "after\n").unwrap();
        let error = git_commit(
            &workspace,
            "Must not replace invalid HEAD",
            vec![FileDiff::Modified(path)],
            &[],
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("HEAD does not point to a commit"),
            "{error}"
        );
        assert_eq!(
            repo.find_reference(&branch).unwrap().target(),
            Some(tree_id)
        );
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn dispatcher_commit_uses_anchor_attribution_and_clears_rows() {
        let workspace = std::env::temp_dir().join(format!(
            "ahead-dispatch-commit-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&workspace).unwrap();
        let repo = Repository::init(&workspace).unwrap();
        {
            let mut config = repo.config().unwrap();
            config.set_str("user.name", "Human Developer").unwrap();
            config.set_str("user.email", "human@example.test").unwrap();
        }

        let path = workspace.join("agent.md");
        fs::write(&path, "AHEAD authored content\n").unwrap();

        let mut dispatcher =
            Dispatcher::new(CoreRpcHandler::new(), ProxyRpcHandler::new());
        dispatcher.handle_notification(ProxyNotification::Initialize {
            workspace: Some(workspace.clone()),
            window_id: 1,
            tab_id: 1,
        });
        let host = dispatcher.ahead_host.as_ref().unwrap().clone();
        let view = host
            .write()
            .start_work(
                Some(ahead_rpc::ahead::WorkKind::ProductChange),
                "Commit route".to_string(),
                "Test attribution".to_string(),
                None,
            )
            .unwrap();
        let session_id = view.session.id;
        host.write()
            .create_anchor(
                &session_id,
                "agent.md".to_string(),
                DisplayRange {
                    start: DisplayPosition { line: 0, col: 0 },
                    end: DisplayPosition { line: 0, col: 23 },
                },
                "AHEAD authored content".to_string(),
                ahead_rpc::ahead::AHEAD_ACTOR_ID,
            )
            .unwrap();
        let second_session_id = host
            .write()
            .start_work(
                Some(ahead_rpc::ahead::WorkKind::ProductChange),
                "Second commit source".to_string(),
                "Verify all session trailers".to_string(),
                None,
            )
            .unwrap()
            .session
            .id;
        for (name, quote, end_col) in [
            ("second.md", "Second agent draft", 18),
            ("third.md", "Third agent draft", 17),
        ] {
            fs::write(workspace.join(name), format!("{quote}\n")).unwrap();
            host.write()
                .create_anchor(
                    &second_session_id,
                    name.to_string(),
                    DisplayRange {
                        start: DisplayPosition { line: 0, col: 0 },
                        end: DisplayPosition {
                            line: 0,
                            col: end_col,
                        },
                    },
                    quote.to_string(),
                    ahead_rpc::ahead::AHEAD_ACTOR_ID,
                )
                .unwrap();
        }

        let request_git = |dispatcher: &mut Dispatcher, request: ProxyRequest| {
            let rpc = dispatcher.proxy_rpc.clone();
            let (sender, receiver) = crossbeam_channel::bounded(1);
            rpc.request_async(request, move |result| {
                sender.send(result).expect("Git response")
            });
            let ProxyRpc::Request(id, request) =
                rpc.rx().try_recv().expect("Git request")
            else {
                panic!("Git request");
            };
            dispatcher.handle_request(id, request);
            receiver.recv().expect("Git response")
        };
        let request_commit =
            |dispatcher: &mut Dispatcher, message: &str, diffs: Vec<FileDiff>| {
                request_git(
                    dispatcher,
                    ProxyRequest::GitCommit {
                        message: message.to_string(),
                        diffs,
                    },
                )
            };
        assert!(matches!(
            request_commit(
                &mut dispatcher,
                "Commit agent work",
                vec![
                    FileDiff::Added(path),
                    FileDiff::Added(workspace.join("second.md")),
                    FileDiff::Added(workspace.join("third.md")),
                ],
            )
            .expect("commit"),
            ProxyResponse::Success {}
        ));

        let repo = Repository::open(&workspace).unwrap();
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(commit.author().name(), Some("ahead"));
        assert_eq!(commit.committer().name(), Some("Human Developer"));
        let actual_trailers = commit
            .message()
            .unwrap()
            .lines()
            .filter(|line| line.starts_with("Ahead-Session: "))
            .collect::<Vec<_>>();
        let mut expected_trailers = vec![
            format!("Ahead-Session: {session_id}"),
            format!("Ahead-Session: {second_session_id}"),
        ];
        expected_trailers.sort();
        assert_eq!(
            actual_trailers,
            expected_trailers
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        assert!(
            host.read()
                .anchors_for_paths(&[
                    "agent.md".to_string(),
                    "second.md".to_string(),
                    "third.md".to_string(),
                ])
                .unwrap()
                .is_empty()
        );

        let human_path = workspace.join("human.md");
        fs::write(&human_path, "Agent draft\n").unwrap();
        host.write()
            .create_anchor(
                &session_id,
                "human.md".to_string(),
                DisplayRange {
                    start: DisplayPosition { line: 0, col: 0 },
                    end: DisplayPosition { line: 0, col: 11 },
                },
                "Agent draft".to_string(),
                ahead_rpc::ahead::AHEAD_ACTOR_ID,
            )
            .unwrap();
        fs::write(&human_path, "Human rewrite\n").unwrap();
        assert!(matches!(
            request_commit(
                &mut dispatcher,
                "Commit human rewrite",
                vec![FileDiff::Added(human_path)],
            )
            .expect("commit"),
            ProxyResponse::Success {}
        ));
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(commit.author().name(), Some("Human Developer"));
        assert_eq!(
            host.read()
                .anchors_for_paths(&["human.md".to_string()])
                .unwrap()
                .len(),
            1,
            "a stale anchor must not be silently cleared"
        );

        let staged_path = workspace.join("staged.md");
        fs::write(&staged_path, "Agent staged content\n").unwrap();
        host.write()
            .create_anchor(
                &session_id,
                "staged.md".to_string(),
                DisplayRange {
                    start: DisplayPosition { line: 0, col: 0 },
                    end: DisplayPosition { line: 0, col: 20 },
                },
                "Agent staged content".to_string(),
                ahead_rpc::ahead::AHEAD_ACTOR_ID,
            )
            .unwrap();
        assert!(matches!(
            request_git(&mut dispatcher, ProxyRequest::GitStageAll {})
                .expect("stage all"),
            ProxyResponse::Success {}
        ));
        assert!(
            repo.index()
                .unwrap()
                .get_path(std::path::Path::new("staged.md"), 0)
                .is_some()
        );
        fs::write(&staged_path, "Human unstaged rewrite\n").unwrap();
        assert!(matches!(
            request_commit(&mut dispatcher, "Commit staged agent work", vec![])
                .expect("staged commit"),
            ProxyResponse::Success {}
        ));
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(commit.author().name(), Some("ahead"));
        assert_eq!(
            host.read()
                .anchors_for_paths(&["staged.md".to_string()])
                .unwrap()
                .len(),
            0
        );
        assert!(
            request_commit(&mut dispatcher, "Nothing staged", vec![])
                .unwrap_err()
                .message
                .contains("Stage changes")
        );

        fs::remove_dir_all(workspace).unwrap();
    }
}
