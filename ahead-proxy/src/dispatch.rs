use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use ahead_core::{
    directory::Directory,
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
        AheadRequest as AgentHostRequest, PredictionBinding, PredictionRequest,
        SharedBufferEditResult, SharedBufferSnapshot,
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
        BlameCommit, BlameHunk, DiffHunkKind, DiffInfo, FileDiff, GitCommitSummary,
        GitFileState, GitPatch, GitStatusFile, GitWorkspaceStatus,
    },
    style::{ExtensionSyntax, LineStyle, SemanticStyles, SyntaxCapture},
    terminal::TermId,
};
use alacritty_terminal::{event::WindowSize, event_loop::Msg};
use anyhow::{Context, Result, anyhow};
use crossbeam_channel::Sender;
use git2::{
    DiffFormat, DiffOptions, ErrorCode::NotFound, Oid, Repository,
    build::CheckoutBuilder,
};
use indexmap::IndexMap;
use lsp_types::{
    CancelParams, GotoDefinitionResponse, MessageType, NumberOrString, Position,
    Range, ShowMessageParams, TextDocumentItem,
    notification::{Cancel, Notification},
};
use parking_lot::Mutex;
use ropey::Rope;
use sha2::{Digest, Sha256};
use url::Url;

use crate::{
    ahead::prediction::{
        MAX_EXCERPT_TOKENS, OpenBufferContext, PredictionCancellation,
        PredictionContextSnapshot, PredictionEngine,
        prediction_excerpt_for_snapshot,
    },
    buffer::{Buffer, get_mod_time, language_id_from_path_with_content, load_file},
    plugin::{
        PluginCatalogRpcHandler, catalog::PluginCatalog,
        watched_files::LSP_FILE_EVENT_TOKEN,
    },
    terminal::{Terminal, TerminalSender},
    watcher::{FileWatcher, FileWatcherHandle, Notify, WatchToken},
};

const OPEN_FILE_EVENT_TOKEN: WatchToken = WatchToken(1);
const WORKSPACE_EVENT_TOKEN: WatchToken = WatchToken(2);
const USER_SETTINGS_EVENT_TOKEN: WatchToken = WatchToken(3);
const USER_SETTINGS_HOME_EVENT_TOKEN: WatchToken = WatchToken(4);
const INSTRUCTION_EVENT_TOKEN: WatchToken = WatchToken(6);
const INSTRUCTION_ROOT_EVENT_TOKEN: WatchToken = WatchToken(7);
const AHEAD_GITIGNORE: &str = include_str!("../../.ahead/.gitignore");
const MAX_FIM_OPEN_BUFFERS: usize = 4;
const MAX_FIM_OPEN_BUFFER_BYTES: usize = 4096;
const MAX_FIM_CONTEXT_SIDE_BYTES: usize = MAX_EXCERPT_TOKENS * 3;
const MAX_FIM_DEFINITION_REQUESTS: usize = 8;
const MAX_FIM_DEFINITION_CACHE_ENTRIES: usize = 128;
const MAX_CACHED_FIM_SEMANTIC_TOKENS: usize = 16_384;
const FIM_DEFINITION_LINE_RADIUS: usize = 3;
const FIM_DEFINITION_CACHE_TTL: Duration = Duration::from_secs(3);
const FIM_DEFINITION_RESPONSE_TIMEOUT: Duration = Duration::from_millis(100);
const MAX_READY_PREDICTIONS: usize = 32;
const PREDICTION_LIFETIME: Duration = Duration::from_secs(120);
const MAX_SHARED_BUFFER_BYTES: usize = 1024 * 1024;

fn rpc_error_from_agent(error: anyhow::Error) -> RpcError {
    let code = ahead_agent::acp_client::acp_error_code(&error).unwrap_or(0);
    RpcError {
        code,
        message: error.to_string(),
    }
}

fn agent_request_runs_in_background(request: &AgentHostRequest) -> bool {
    matches!(
        request,
        AgentHostRequest::ListExternalAcpAdapters
            | AgentHostRequest::SearchMemory { .. }
            | AgentHostRequest::WriteMemory { .. }
            | AgentHostRequest::AgentTurnStart { .. }
            | AgentHostRequest::AgentTurnRetry { .. }
            | AgentHostRequest::AgentTurnCancel { .. }
            | AgentHostRequest::AgentSessionPrepare { .. }
            | AgentHostRequest::AgentAuthenticate { .. }
            | AgentHostRequest::AgentTerminalAuthLaunch { .. }
            | AgentHostRequest::AgentTerminalAuthComplete { .. }
            | AgentHostRequest::AgentConfigOptionSet { .. }
    )
}

fn is_provider_settings_path(workspace: &Path, path: &Path) -> bool {
    if !matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("settings.toml" | "config.toml" | "config.local.toml")
    ) {
        return false;
    }
    let Some(directory) = path.parent().filter(|directory| {
        directory.file_name().is_some_and(|name| name == ".ahead")
    }) else {
        return false;
    };
    directory.parent().is_some_and(|root| {
        root == workspace || root.canonicalize().is_ok_and(|root| root == workspace)
    })
}

fn user_settings_home_directory() -> Option<PathBuf> {
    Directory::home_dir()?.canonicalize().ok()
}

fn user_settings_directory_in(home: &Path) -> Option<PathBuf> {
    let path = home.join(".ahead");
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return None;
    }
    path.canonicalize().ok()
}

fn user_settings_directory() -> Option<PathBuf> {
    user_settings_directory_in(&user_settings_home_directory()?)
}

fn is_user_ahead_directory(home: &Path, path: &Path) -> bool {
    path.parent() == Some(home)
        && path.file_name().is_some_and(|name| name == ".ahead")
}

fn is_user_lsp_settings_path(settings_directory: &Path, path: &Path) -> bool {
    path.parent() == Some(settings_directory)
        && path.file_name().is_some_and(|name| name == "settings.toml")
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
    user_settings_watched: bool,
    file_index: Option<Arc<WorkspaceFileIndex>>,
    file_index_warmup: Option<Arc<AtomicBool>>,
    workspace_file_jobs: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
    shown_prediction_errors: Arc<Mutex<HashSet<String>>>,
    prediction_definition_cache: Arc<
        Mutex<HashMap<PredictionDefinitionCacheKey, CachedPredictionDefinitions>>,
    >,
    prediction_semantic_tokens: Arc<Mutex<PredictionSemanticTokenCache>>,
    inline_prediction_jobs: Arc<Mutex<HashMap<RequestId, InlinePredictionJob>>>,
    ready_predictions: Arc<Mutex<HashMap<PathBuf, ReadyPrediction>>>,
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

struct InlinePredictionJob {
    path: PathBuf,
    dependencies: HashSet<PathBuf>,
    cancellation: PredictionCancellation,
}

#[derive(Default)]
struct PredictionSemanticTokenCache {
    buffers: HashMap<PathBuf, PredictionSemanticTokenEntry>,
}

struct PredictionSemanticTokenEntry {
    buffer_id: BufferId,
    revision: u64,
    len: usize,
    styles: Option<SemanticStyles>,
    syntax_captures: Option<Vec<SyntaxCapture>>,
}

impl PredictionSemanticTokenCache {
    fn track_buffer(&mut self, path: &Path, buffer: &Buffer) {
        let entry = self.buffers.entry(path.to_path_buf()).or_insert_with(|| {
            PredictionSemanticTokenEntry {
                buffer_id: buffer.id.clone(),
                revision: buffer.rev,
                len: buffer.rope.len(),
                styles: None,
                syntax_captures: None,
            }
        });
        if entry.buffer_id != buffer.id
            || entry.revision != buffer.rev
            || entry.len != buffer.rope.len()
        {
            *entry = PredictionSemanticTokenEntry {
                buffer_id: buffer.id.clone(),
                revision: buffer.rev,
                len: buffer.rope.len(),
                styles: None,
                syntax_captures: None,
            };
        }
    }

    fn remove_buffer(&mut self, path: &Path) {
        self.buffers.remove(path);
    }

    fn store(&mut self, buffer_id: BufferId, styles: &SemanticStyles) {
        let Some(entry) = self.buffers.get_mut(&styles.path) else {
            return;
        };
        if entry.buffer_id == buffer_id
            && entry.revision == styles.rev
            && entry.len == styles.len
        {
            let mut identifier_styles = styles.clone();
            identifier_styles.styles.retain(|style| {
                style
                    .style
                    .fg_color
                    .as_deref()
                    .is_some_and(is_prediction_identifier_semantic_token_type)
            });
            if identifier_styles.styles.len() <= MAX_CACHED_FIM_SEMANTIC_TOKENS {
                entry.styles = Some(identifier_styles);
            } else {
                entry.styles = None;
            }
        }
    }

    fn styles_for(&self, path: &Path, buffer: &Buffer) -> Option<SemanticStyles> {
        let entry = self.buffers.get(path)?;
        if entry.buffer_id == buffer.id
            && entry.revision == buffer.rev
            && entry.len == buffer.rope.len()
        {
            entry.styles.clone()
        } else {
            None
        }
    }

    fn store_syntax_captures(
        &mut self,
        buffer_id: &BufferId,
        path: &Path,
        revision: u64,
        len: usize,
        captures: Vec<SyntaxCapture>,
    ) {
        let Some(entry) = self.buffers.get_mut(path) else {
            return;
        };
        if &entry.buffer_id != buffer_id
            || entry.revision != revision
            || entry.len != len
        {
            return;
        }

        let captures = captures
            .into_iter()
            .filter(|capture| {
                capture.start < capture.end
                    && capture.end <= len
                    && is_prediction_identifier_syntax_scope(&capture.scope)
            })
            .collect::<Vec<_>>();
        entry.syntax_captures =
            (captures.len() <= MAX_CACHED_FIM_SEMANTIC_TOKENS).then_some(captures);
    }

    fn syntax_captures_for(
        &self,
        path: &Path,
        buffer: &Buffer,
    ) -> Option<Vec<SyntaxCapture>> {
        let entry = self.buffers.get(path)?;
        if entry.buffer_id == buffer.id
            && entry.revision == buffer.rev
            && entry.len == buffer.rope.len()
        {
            entry.syntax_captures.clone()
        } else {
            None
        }
    }
}

#[derive(Clone, Hash, Eq, PartialEq)]
struct PredictionDefinitionCacheKey {
    path: PathBuf,
    buffer_id: BufferId,
    revision: u64,
    identifier: String,
    line: u32,
    character: u32,
}

struct CachedPredictionDefinition {
    path: PathBuf,
    line: u32,
    buffer_id: BufferId,
    revision: u64,
}

struct CachedPredictionDefinitions {
    created: Instant,
    locations: Vec<CachedPredictionDefinition>,
}

struct ReadyPrediction {
    binding: PredictionBinding,
    created: Instant,
    snapshot: PredictionContextSnapshot,
    request: PredictionRequest,
    open_buffers: Vec<OpenBufferContext>,
    dependencies: BTreeMap<PathBuf, (BufferId, u64)>,
    target_id: BufferId,
    target: Rope,
    insertion_offset: usize,
    replacement: String,
}

impl ReadyPrediction {
    fn new(
        mut snapshot: PredictionContextSnapshot,
        mut request: PredictionRequest,
        open_buffers: Vec<OpenBufferContext>,
        buffers: &HashMap<PathBuf, Buffer>,
        target: &Buffer,
        replacement: String,
    ) -> Result<Self> {
        let dependencies = open_buffers
            .iter()
            .map(|context| {
                let path = snapshot.workspace.join(&context.path);
                let buffer =
                    buffers.get(&path).context("prediction dependency closed")?;
                Ok((path, (buffer.id, buffer.rev)))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let insertion_offset = prediction_cursor_byte_offset(
            &target.rope,
            Position::new(request.cursor.line, request.cursor.col),
        )
        .context("prediction cursor is outside the target buffer")?;
        let (prefix, suffix) =
            prediction_excerpt_for_snapshot(&snapshot, &open_buffers)?;
        request.prefix = prefix.clone();
        request.suffix = suffix.clone();
        snapshot.request.prefix = prefix;
        snapshot.request.suffix = suffix;
        let bytes = serde_json::to_vec(&(
            1,
            &snapshot,
            &open_buffers,
            &dependencies,
            target.id,
            target.rev,
        ))?;
        let binding = PredictionBinding {
            version: 1,
            request_id: request.request_id.clone(),
            context_sha256: format!("{:x}", Sha256::digest(bytes)),
            target_revision: target.rev,
        };
        Ok(Self {
            binding,
            created: Instant::now(),
            snapshot,
            request,
            open_buffers,
            dependencies,
            target_id: target.id,
            target: target.rope.clone(),
            insertion_offset,
            replacement,
        })
    }

    fn publish(
        self,
        path: PathBuf,
        cache: &Mutex<HashMap<PathBuf, Self>>,
    ) -> PredictionBinding {
        let binding = self.binding.clone();
        let mut cached = cache.lock();
        cached.retain(|_, ready| ready.created.elapsed() <= PREDICTION_LIFETIME);
        if cached.len() >= MAX_READY_PREDICTIONS
            && !cached.contains_key(&path)
            && let Some(oldest) = cached
                .iter()
                .min_by_key(|(_, ready)| ready.created)
                .map(|(path, _)| path.clone())
        {
            cached.remove(&oldest);
        }
        cached.insert(path, self);
        binding
    }
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
                self.cancel_inline_predictions(None);
                self.ready_predictions.lock().clear();
                if let Some(cancelled) = self.file_index_warmup.take() {
                    cancelled.store(true, Ordering::SeqCst);
                }
                for cancelled in self.workspace_file_jobs.lock().values() {
                    cancelled.store(true, Ordering::SeqCst);
                }
                self.workspace_file_jobs.lock().clear();
                self.shown_prediction_errors.lock().clear();
                self.workspace = workspace;
                self.file_index = self.workspace.as_ref().map(|workspace| {
                    Arc::new(WorkspaceFileIndex::new(workspace.clone()))
                });
                let mut notifier = FileWatchNotifier::new(
                    self.workspace.clone(),
                    self.core_rpc.clone(),
                    self.proxy_rpc.clone(),
                    self.catalog_rpc.clone(),
                    self.file_index.clone(),
                );
                let git_metadata_paths = notifier.git_metadata_paths.clone();
                let user_settings_home = user_settings_home_directory();
                let user_settings_directory = user_settings_home
                    .as_deref()
                    .and_then(user_settings_directory_in);
                notifier.user_settings_home = user_settings_home.clone();
                notifier.user_settings_watcher = Some(self.file_watcher.handle());
                notifier.watch_instruction_ancestors(&mut self.file_watcher);
                *notifier.user_settings_directory.lock() =
                    user_settings_directory.clone();
                if !self.user_settings_watched
                    && let Some(home) = user_settings_home.as_deref()
                {
                    let watched_home = home.to_owned();
                    self.file_watcher.watch_filtered(
                        home,
                        false,
                        USER_SETTINGS_HOME_EVENT_TOKEN,
                        move |path| is_user_ahead_directory(&watched_home, path),
                    );
                    if let Some(settings_directory) =
                        user_settings_directory.as_deref()
                    {
                        let watched_directory = settings_directory.to_owned();
                        self.file_watcher.watch_filtered(
                            settings_directory,
                            false,
                            USER_SETTINGS_EVENT_TOKEN,
                            move |path| {
                                is_user_lsp_settings_path(&watched_directory, path)
                            },
                        );
                    }
                    self.user_settings_watched = true;
                }
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
                self.file_watcher.notify(notifier);
                self.file_index_warmup = self.file_index.as_ref().map(|index| {
                    let cancelled = Arc::new(AtomicBool::new(false));
                    let warmup_cancelled = cancelled.clone();
                    let index = index.clone();
                    thread::spawn(move || {
                        let _ = index.snapshot_with_generation_while(move || {
                            !warmup_cancelled.load(Ordering::SeqCst)
                        });
                    });
                    cancelled
                });

                let plugin_rpc = self.catalog_rpc.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                let workspace = self.workspace.clone();
                let file_watcher = self.file_watcher.handle();
                let (stopped, catalog_stopped) = crossbeam_channel::bounded(1);
                self.catalog_stopped = Some(catalog_stopped);
                thread::spawn(move || {
                    let mut plugin =
                        PluginCatalog::new(workspace, plugin_rpc.clone())
                            .with_proxy_rpc(proxy_rpc)
                            .with_file_watcher(file_watcher);
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
            CancelCompletion { request_id } => {
                self.catalog_rpc.cancel_completion(request_id);
            }
            SignatureHelp {
                request_id,
                path,
                position,
                context,
            } => {
                self.catalog_rpc
                    .signature_help(request_id, &path, position, context);
            }
            CancelInlineCompletions { request_id } => {
                if let Some(job) =
                    self.inline_prediction_jobs.lock().remove(&request_id)
                {
                    job.cancellation.cancel();
                }
            }
            Shutdown {} => {
                self.cancel_inline_predictions(None);
                self.ready_predictions.lock().clear();
                self.shared_session_server = None;
                *self.shared_session_client.lock() = None;
                let deadline = std::time::Instant::now() + Duration::from_secs(8);
                if let Some(cancelled) = self.file_index_warmup.take() {
                    cancelled.store(true, Ordering::SeqCst);
                }
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
            RepairLanguageServer { server_id } => {
                if let Err(error) = self.catalog_rpc.catalog_notification(
                    crate::plugin::PluginCatalogNotification::RepairBuiltinNpmServer {
                        server_id,
                    },
                ) {
                    tracing::error!(?error, "repairing cached language server");
                }
            }
            RestartLanguageServers { extension_id } => {
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
                if let Err(error) = self.catalog_rpc.catalog_notification(
                    crate::plugin::PluginCatalogNotification::RestartLanguageServers {
                        documents,
                        extension_id,
                    },
                ) {
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
                self.prediction_semantic_tokens
                    .lock()
                    .track_buffer(&path, buffer);
                self.catalog_rpc.did_change_text_document(
                    &path,
                    rev,
                    delta,
                    old_text,
                    buffer.rope.clone(),
                );
                self.cancel_inline_predictions(Some(&path));
            }
            EditorEdit { path, delta } => {
                if self.shared_remote_pending.contains_key(&path) {
                    tracing::debug!(
                        ?path,
                        "editor edit waits for shared change acknowledgement"
                    );
                    return;
                }
                if let Err(error) = self.sync_editor_delta(path.clone(), delta) {
                    tracing::warn!(
                        ?error,
                        path = %path.display(),
                        "applying incremental editor edit"
                    );
                }
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
            UninstallLanguageExtension { extension_id } => {
                let proxy_rpc = self.proxy_rpc.clone();
                let failed_request_id = id.clone();
                if let Err(error) = self.catalog_rpc.uninstall_language_extension(
                    extension_id,
                    Box::new(move |result: Result<(), RpcError>| {
                        proxy_rpc.handle_response(
                            id,
                            result.map(|()| ProxyResponse::Success {}),
                        );
                    }),
                ) {
                    self.proxy_rpc.handle_response(
                        failed_request_id,
                        Err(RpcError {
                            code: 0,
                            message: error.to_string(),
                        }),
                    );
                }
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
            GitStatus {} => {
                let workspace = self.workspace.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                thread::spawn(move || {
                    let result = workspace
                        .ok_or_else(|| anyhow!("no workspace set"))
                        .and_then(|workspace| git_workspace_status(&workspace))
                        .map(|status| ProxyResponse::GitStatus { status })
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                    proxy_rpc.handle_response(id, result);
                });
            }
            GitCheckout { reference } => {
                let workspace = self.workspace.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                thread::spawn(move || {
                    let result = workspace
                        .ok_or_else(|| anyhow!("no workspace set"))
                        .and_then(|workspace| git_checkout(&workspace, &reference))
                        .map(|_| ProxyResponse::Success {})
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                    proxy_rpc.handle_response(id, result);
                });
            }
            GitCreateBranch { reference } => {
                let workspace = self.workspace.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                thread::spawn(move || {
                    let result = workspace
                        .ok_or_else(|| anyhow!("no workspace set"))
                        .and_then(|workspace| {
                            git_create_branch(&workspace, &reference)
                        })
                        .map(|_| ProxyResponse::Success {})
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                    proxy_rpc.handle_response(id, result);
                });
            }
            GitDeleteBranch { reference } => {
                let workspace = self.workspace.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                thread::spawn(move || {
                    let result = workspace
                        .ok_or_else(|| anyhow!("no workspace set"))
                        .and_then(|workspace| {
                            git_delete_branch(&workspace, &reference)
                        })
                        .map(|_| ProxyResponse::Success {})
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                    proxy_rpc.handle_response(id, result);
                });
            }
            GitPush { remote } => {
                let workspace = self.workspace.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                let core_rpc = self.core_rpc.clone();
                thread::spawn(move || {
                    let result = workspace
                        .ok_or_else(|| anyhow!("no workspace set"))
                        .and_then(|workspace| {
                            git_push(&workspace, remote.as_deref(), core_rpc)
                        })
                        .map(|_| ProxyResponse::Success {})
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                    proxy_rpc.handle_response(id, result);
                });
            }
            GitHistory { limit } => {
                let workspace = self.workspace.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                thread::spawn(move || {
                    let result = workspace
                        .ok_or_else(|| anyhow!("no workspace set"))
                        .and_then(|workspace| git_history(&workspace, limit))
                        .map(|commits| ProxyResponse::GitHistory { commits })
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                    proxy_rpc.handle_response(id, result);
                });
            }
            GitCommitPatch { id: commit_id } => {
                let workspace = self.workspace.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                thread::spawn(move || {
                    let result = workspace
                        .ok_or_else(|| anyhow!("no workspace set"))
                        .and_then(|workspace| {
                            git_commit_patch(&workspace, &commit_id)
                        })
                        .map(|patch| ProxyResponse::GitCommitPatch { patch })
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                    proxy_rpc.handle_response(id, result);
                });
            }
            GitWorktreeFilePatch { path } => {
                let workspace = self.workspace.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                thread::spawn(move || {
                    let result = workspace
                        .ok_or_else(|| anyhow!("no workspace set"))
                        .and_then(|workspace| {
                            git_worktree_file_patch(&workspace, &path)
                        })
                        .map(|patch| ProxyResponse::GitWorktreeFilePatch { patch })
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                    proxy_rpc.handle_response(id, result);
                });
            }
            GitWorktreePatch {} => {
                let workspace = self.workspace.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                thread::spawn(move || {
                    let result = workspace
                        .ok_or_else(|| anyhow!("no workspace set"))
                        .and_then(|workspace| git_worktree_patch(&workspace))
                        .map(|patch| ProxyResponse::GitWorktreePatch { patch })
                        .map_err(|error| RpcError {
                            code: 0,
                            message: error.to_string(),
                        });
                    proxy_rpc.handle_response(id, result);
                });
            }
            GitStagePaths { paths, staged } => {
                let workspace = self.workspace.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                thread::spawn(move || {
                    let result = workspace
                        .ok_or_else(|| anyhow!("no workspace set"))
                        .and_then(|workspace| {
                            git_stage_paths(&workspace, &paths, staged)
                        })
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
                let background = agent_request_runs_in_background(&request);
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
                            .map_err(rpc_error_from_agent)
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
                self.prediction_semantic_tokens
                    .lock()
                    .track_buffer(&path, &buffer);
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
                let workspace = self.workspace.clone();
                let host = self.ahead_host.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                thread::spawn(move || {
                    let result = workspace
                        .ok_or_else(|| anyhow!("no workspace set"))
                        .and_then(|workspace| {
                            let host = host.as_ref().map(|host| host.read());
                            git_commit_attributed(
                                &workspace,
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
                    proxy_rpc.handle_response(id, result);
                });
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
                            match index.snapshot_with_generation_while(move || {
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
                            index.snapshot_with_generation_while(move || {
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
                let Some(buffer) = self.buffers.get(&path) else {
                    self.respond_rpc(
                        id,
                        Err(RpcError {
                            code: 0,
                            message: format!(
                                "inlay hints requested for unopened buffer: {}",
                                path.display()
                            ),
                        }),
                    );
                    return;
                };
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
                session_id,
                model_context,
            } => {
                let proxy_rpc = self.proxy_rpc.clone();
                let ahead_host = self.ahead_host.clone();
                let core_rpc = self.core_rpc.clone();
                let shown_prediction_errors = self.shown_prediction_errors.clone();
                let prediction_definition_cache =
                    self.prediction_definition_cache.clone();
                let prediction_semantic_tokens =
                    self.prediction_semantic_tokens.clone();
                let buffer_snapshot = self.buffers.clone();
                let workspace = self.workspace.clone();
                let path_buf = path.clone();
                let cancellation = PredictionCancellation::default();
                let mut dependencies = HashSet::from([path.clone()]);
                if let Some(workspace) = workspace.as_deref()
                    && let Some(buffer) = buffer_snapshot.get(&path)
                {
                    dependencies.extend(
                        relevant_open_buffer_contexts(
                            &buffer_snapshot,
                            workspace,
                            &path,
                            &buffer.rope,
                            &[],
                        )
                        .into_iter()
                        .map(|buffer| workspace.join(buffer.path)),
                    );
                }
                let prediction_jobs = self.inline_prediction_jobs.clone();
                let ready_predictions = self.ready_predictions.clone();
                let catalog_rpc = self.catalog_rpc.clone();
                {
                    let mut jobs = prediction_jobs.lock();
                    jobs.retain(|_, job| {
                        if job.path == path {
                            job.cancellation.cancel();
                            false
                        } else {
                            true
                        }
                    });
                    jobs.insert(
                        id,
                        InlinePredictionJob {
                            path: path.clone(),
                            dependencies,
                            cancellation: cancellation.clone(),
                        },
                    );
                }
                self.catalog_rpc.get_inline_completions(
                    &path,
                    position,
                    trigger_kind,
                    move |_, result| {
                        let failed_proxy = proxy_rpc.clone();
                        let failed_jobs = prediction_jobs.clone();
                        let has_items = result.as_ref().is_ok_and(|completions| {
                            match completions {
                                lsp_types::InlineCompletionResponse::Array(items) => {
                                    !items.is_empty()
                                }
                                lsp_types::InlineCompletionResponse::List(list) => {
                                    !list.items.is_empty()
                                }
                            }
                        });
                        let can_resolve_related_buffers = !has_items
                            && !cancellation.is_cancelled()
                            && ahead_host.is_some()
                            && workspace.as_deref().is_some_and(|workspace| {
                                is_prediction_target_visible(workspace, &path_buf)
                            })
                            && buffer_snapshot.contains_key(&path_buf);
                        let definition_responses = if can_resolve_related_buffers {
                            buffer_snapshot.get(&path_buf).and_then(|buffer| {
                                let (semantic_styles, syntax_captures) = {
                                    let cache = prediction_semantic_tokens.lock();
                                    (
                                        cache.styles_for(&path_buf, buffer),
                                        cache.syntax_captures_for(&path_buf, buffer),
                                    )
                                };
                                let positions =
                                    prediction_identifier_positions_with_semantic_styles(
                                        &buffer.rope,
                                        position,
                                        semantic_styles.as_ref(),
                                        syntax_captures.as_deref(),
                                    );
                                if positions.is_empty() {
                                    return None;
                                }

                                let mut cached = Vec::new();
                                let mut pending = Vec::new();
                                for (index, (identifier, definition_position)) in
                                    positions.iter().enumerate()
                                {
                                    let key = PredictionDefinitionCacheKey {
                                        path: path_buf.clone(),
                                        buffer_id: buffer.id,
                                        revision: buffer.rev,
                                        identifier: identifier.clone(),
                                        line: definition_position.line,
                                        character: definition_position.character,
                                    };
                                    if let Some(locations) = cached_prediction_definitions(
                                        &prediction_definition_cache,
                                        &key,
                                        workspace.as_deref(),
                                        &buffer_snapshot,
                                    ) {
                                        cached.push((index, locations));
                                    } else {
                                        pending.push((index, *definition_position, key));
                                    }
                                }

                                let (sender, receiver) =
                                    crossbeam_channel::bounded(pending.len().max(1));
                                for (index, definition_position, key) in &pending {
                                    let sender = sender.clone();
                                    let key = key.clone();
                                    let index = *index;
                                    let definition_position = *definition_position;
                                    catalog_rpc.get_definition(
                                        &path_buf,
                                        definition_position,
                                        move |_, result| {
                                            let locations = result
                                                .ok()
                                                .map(prediction_definition_locations);
                                            if sender.send((index, key, locations)).is_err() {
                                                tracing::debug!(
                                                    "discarding edit-prediction definition after request ended"
                                                );
                                            }
                                        },
                                    );
                                }
                                drop(sender);
                                Some((receiver, pending.len(), cached))
                            })
                        } else {
                            None
                        };
                        let response_deadline =
                            Instant::now() + FIM_DEFINITION_RESPONSE_TIMEOUT;
                        let run_prediction = move || {
                            let mut completions_opt = result.ok();
                            let mut context_paths = Vec::new();
                            let mut ready_prediction = None;
                            let definition_locations: Vec<(PathBuf, u32)> = definition_responses
                                .filter(|_| !cancellation.is_cancelled())
                                .map(|(receiver, expected, mut responses)| {
                                    let mut pending_responses = 0;
                                    while pending_responses < expected
                                        && !cancellation.is_cancelled()
                                    {
                                        let remaining =
                                            response_deadline
                                                .saturating_duration_since(Instant::now());
                                        if remaining.is_zero() {
                                            break;
                                        }
                                        match receiver.recv_timeout(remaining) {
                                            Ok((index, key, locations)) => {
                                                if let Some(locations) = locations {
                                                    cache_prediction_definitions(
                                                        &prediction_definition_cache,
                                                        key,
                                                        &locations,
                                                        workspace.as_deref(),
                                                        &buffer_snapshot,
                                                    );
                                                    responses.push((index, locations));
                                                } else {
                                                    responses.push((index, Vec::new()));
                                                }
                                                pending_responses += 1;
                                            }
                                            Err(_) => break,
                                        }
                                    }
                                    responses.sort_by_key(|(index, _)| *index);
                                    responses
                                        .into_iter()
                                        .flat_map(|(_, locations)| locations)
                                        .collect()
                                })
                                .unwrap_or_default();
                            if !has_items && !cancellation.is_cancelled() {
                                if let Some(host_lock) = ahead_host {
                                    if let Some(workspace) = workspace.as_deref()
                                        && is_prediction_target_visible(
                                            workspace,
                                            &path_buf,
                                        )
                                        && let Some(buffer) =
                                            buffer_snapshot.get(&path_buf)
                                    {
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
                                                &buffer.rope,
                                                &definition_locations,
                                            );
                                        if let Some(job) =
                                            prediction_jobs.lock().get_mut(&id)
                                        {
                                            job.dependencies.extend(
                                                open_buffers.iter().map(|buffer| {
                                                    workspace.join(&buffer.path)
                                                }),
                                            );
                                        }
                                        context_paths = open_buffers.iter()
                                            .map(|buffer| PathBuf::from(&buffer.path))
                                            .collect();
                                        let request =
                                            ahead_rpc::ahead::PredictionRequest {
                                                request_id: uuid::Uuid::new_v4()
                                                    .to_string(),
                                                session_id,
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
                                        let prediction = (|| -> Result<_> {
                                            let snapshot = host_lock.read()
                                                .prepare_prediction_with_model_context(
                                                    request.clone(),
                                                    &open_buffers,
                                                    model_context.as_ref(),
                                                )?;
                                            let prediction = PredictionEngine::predict_cancellable_with_model_context(
                                                &snapshot.work, &snapshot.request, &open_buffers,
                                                &snapshot.provider, snapshot.model_context.as_ref(),
                                                &cancellation,
                                            )?;
                                            let current = host_lock.read()
                                                .prepare_prediction_with_model_context(
                                                    request.clone(),
                                                    &open_buffers,
                                                    model_context.as_ref(),
                                                )?;
                                            if current != snapshot {
                                                cancellation.cancel();
                                                anyhow::bail!("prediction context changed during inference");
                                            }
                                            let ready = ReadyPrediction::new(snapshot, request,
                                                open_buffers, &buffer_snapshot, buffer,
                                                prediction.replacement.clone())?;
                                            Ok((prediction, ready))
                                        })();
                                        match prediction {
                                            Ok((prediction, ready)) => {
                                                shown_prediction_errors.lock().clear();
                                                if !prediction.replacement.is_empty() {
                                                    ready_prediction = Some(ready);
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
                                            Err(_) if cancellation.is_cancelled() => {}
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

                            if cancellation.is_cancelled() {
                                completions_opt = None;
                                ready_prediction = None;
                            }
                            prediction_jobs.lock().remove(&id);
                            let prediction = ready_prediction
                                .map(|ready| ready.publish(path_buf, &ready_predictions));

                            let final_result = Ok(ProxyResponse::GetInlineCompletions {
                                context_paths,
                                prediction,
                                completions:
                                    completions_opt.unwrap_or_else(|| {
                                        lsp_types::InlineCompletionResponse::Array(
                                            Vec::new(),
                                        )
                                    }),
                            });
                            proxy_rpc.handle_response(id, final_result);
                        };
                        let spawned = thread::Builder::new()
                            .name("ahead-inline-prediction".into())
                            .spawn(run_prediction);
                        if let Err(error) = spawned {
                            failed_jobs.lock().remove(&id);
                            failed_proxy.handle_response(id, Err(RpcError {
                                code: 0,
                                message: format!("could not start edit prediction: {error}"),
                            }));
                        }
                    },
                );
            }
            ValidateInlinePrediction {
                path,
                position,
                content_sha256,
                replacement,
                binding,
            } => {
                let result = self
                    .validate_inline_prediction(
                        &path,
                        position,
                        &content_sha256,
                        &replacement,
                        &binding,
                    )
                    .map(|valid| ProxyResponse::ValidateInlinePrediction { valid })
                    .map_err(rpc_error_from_agent);
                self.respond_rpc(id, result);
            }
            GetSemanticTokens { path } => {
                let Some(buffer) = self.buffers.get(&path) else {
                    self.respond_rpc(
                        id,
                        Err(RpcError {
                            code: 0,
                            message: format!(
                                "semantic tokens requested for unopened buffer: {}",
                                path.display()
                            ),
                        }),
                    );
                    return;
                };
                let text = buffer.rope.clone();
                let rev = buffer.rev;
                let len = buffer.rope.len();
                let buffer_id = buffer.id.clone();
                let local_path = path.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                let catalog_rpc = self.catalog_rpc.clone();
                let semantic_token_cache = self.prediction_semantic_tokens.clone();

                let handle_tokens =
                    move |result: Result<Vec<LineStyle>, RpcError>| match result {
                        Ok(styles) => {
                            let semantic_styles = SemanticStyles {
                                rev,
                                path: local_path.clone(),
                                styles,
                                len,
                            };
                            semantic_token_cache
                                .lock()
                                .store(buffer_id, &semantic_styles);
                            proxy_rpc.handle_response(
                                id,
                                Ok(ProxyResponse::GetSemanticTokens {
                                    styles: semantic_styles,
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
            GetExtensionSyntax { path } => {
                let Some(buffer) = self.buffers.get(&path) else {
                    self.respond_rpc(
                        id,
                        Err(RpcError {
                            code: 0,
                            message: format!(
                                "extension syntax requested for unopened buffer: {}",
                                path.display()
                            ),
                        }),
                    );
                    return;
                };
                let source = buffer.rope.to_string();
                let revision = buffer.rev;
                let source_length = source.len();
                let buffer_id = buffer.id.clone();
                let local_path = path.clone();
                let proxy_rpc = self.proxy_rpc.clone();
                let syntax_cache = self.prediction_semantic_tokens.clone();
                thread::spawn(move || {
                    let result = Directory::plugins_directory()
                        .map(|extensions_root| {
                            ahead_extension_host::extension_syntax_for_path(
                                &extensions_root,
                                &local_path,
                                &source,
                            )
                        })
                        .transpose()
                        .map(|syntax| syntax.flatten());
                    match result {
                        Ok(syntax) => {
                            let (language_name, captures, comments) = match syntax {
                                Some(syntax) => (
                                    Some(syntax.language_name),
                                    syntax
                                        .captures
                                        .into_iter()
                                        .map(|capture| SyntaxCapture {
                                            start: capture.start,
                                            end: capture.end,
                                            scope: capture.scope,
                                        })
                                        .collect(),
                                    Some(ahead_rpc::style::LanguageComments {
                                        line_comment: syntax.comments.line_comment,
                                        block_comment: syntax.comments.block_comment,
                                    }),
                                ),
                                None => (None, Vec::new(), None),
                            };
                            syntax_cache.lock().store_syntax_captures(
                                &buffer_id,
                                &local_path,
                                revision,
                                source_length,
                                captures.clone(),
                            );
                            proxy_rpc.handle_response(
                                id,
                                Ok(ProxyResponse::GetExtensionSyntax {
                                    syntax: ExtensionSyntax {
                                        rev: revision,
                                        path: local_path,
                                        len: source_length,
                                        language_name,
                                        captures,
                                        comments,
                                    },
                                }),
                            );
                        }
                        Err(error) => {
                            tracing::warn!(
                                path = %local_path.display(),
                                ?error,
                                "loading installed extension syntax"
                            );
                            proxy_rpc.handle_response(
                                id,
                                Err(RpcError {
                                    code: 0,
                                    message: format!(
                                        "loading extension syntax: {error:#}"
                                    ),
                                }),
                            );
                        }
                    }
                });
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
                    move |result| {
                        let result = result.map(|actions| {
                            ProxyResponse::GetCodeActionsResponse { actions }
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
            GetDocumentOnTypeFormatting {
                path,
                position,
                trigger_character,
            } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc.get_document_on_type_formatting(
                    &path,
                    position,
                    trigger_character,
                    move |_, result| {
                        let result = result.map(|edits| {
                            ProxyResponse::GetDocumentOnTypeFormatting { edits }
                        });
                        proxy_rpc.handle_response(id, result);
                    },
                );
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
                self.prediction_semantic_tokens
                    .lock()
                    .track_buffer(&path, &buffer);
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
                                    self.prediction_semantic_tokens
                                        .lock()
                                        .remove_buffer(&path);
                                    let new_path = to.join(suffix);
                                    buffer.path = new_path;

                                    self.prediction_semantic_tokens
                                        .lock()
                                        .track_buffer(&buffer.path, &buffer);
                                    self.buffers.insert(buffer.path.clone(), buffer);
                                }
                            }
                        } else if is_file {
                            // If the renamed file is open in a buffer, update it to use the new
                            // path.
                            let buffer = self.buffers.remove(&from);

                            if let Some(mut buffer) = buffer {
                                self.prediction_semantic_tokens
                                    .lock()
                                    .remove_buffer(&from);
                                buffer.path.clone_from(&to);
                                self.prediction_semantic_tokens
                                    .lock()
                                    .track_buffer(&to, &buffer);
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
            ExecuteLspCommand {
                plugin_id,
                command,
                arguments,
            } => {
                let proxy_rpc = self.proxy_rpc.clone();
                self.catalog_rpc.execute_command(
                    plugin_id,
                    command,
                    arguments,
                    move |result| {
                        let result = result
                            .map(|_| ProxyResponse::ExecuteLspCommandResponse {});
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

fn prediction_cursor_byte_offset(rope: &Rope, position: Position) -> Option<usize> {
    let line_index = position.line as usize;
    if line_index >= rope.len_lines(ropey::LineType::LF_CR) {
        return None;
    }
    let line_start = rope.line_to_byte_idx(line_index, ropey::LineType::LF_CR);
    let line = rope.line(line_index, ropey::LineType::LF_CR);

    let target_utf16_column = position.character as usize;
    let mut utf16_column = 0;
    let mut byte_column = 0;
    for character in line.chars() {
        if matches!(character, '\r' | '\n') || utf16_column >= target_utf16_column {
            break;
        }
        byte_column += character.len_utf8();
        utf16_column += character.len_utf16();
    }
    Some(line_start + byte_column)
}

fn rope_byte_offset_floor(rope: &Rope, byte_offset: usize) -> usize {
    let mut byte_offset = byte_offset.min(rope.len());
    while byte_offset > 0 && !rope.is_char_boundary(byte_offset) {
        byte_offset -= 1;
    }
    byte_offset
}

fn prediction_prefix_suffix(rope: &Rope, position: Position) -> (String, String) {
    let Some(cursor_offset) = prediction_cursor_byte_offset(rope, position) else {
        return (String::new(), String::new());
    };
    let prefix_start = rope_byte_offset_floor(
        rope,
        cursor_offset.saturating_sub(MAX_FIM_CONTEXT_SIDE_BYTES),
    );
    let suffix_end = rope_byte_offset_floor(
        rope,
        cursor_offset
            .saturating_add(MAX_FIM_CONTEXT_SIDE_BYTES)
            .min(rope.len()),
    );
    (
        rope.slice(prefix_start..cursor_offset).to_string(),
        rope.slice(cursor_offset..suffix_end).to_string(),
    )
}

// shortcut: use current LSP semantic tokens first; fall back to lexical scanning
// until editor parse captures can be reused here.
fn prediction_identifier_positions(
    rope: &Rope,
    cursor: Position,
) -> Vec<(String, Position)> {
    let line_count = rope.len_lines(ropey::LineType::LF_CR);
    if line_count == 0 {
        return Vec::new();
    }

    let cursor_line = (cursor.line as usize).min(line_count - 1);
    let cursor_line_start =
        rope.line_to_byte_idx(cursor_line, ropey::LineType::LF_CR);
    let cursor_text = rope.line(cursor_line, ropey::LineType::LF_CR).to_string();
    let cursor_content = cursor_text.trim_end_matches(['\r', '\n']);
    let cursor_byte = cursor_line_start
        + offset_utf16_to_utf8_str(cursor_content, cursor.character as usize);
    let first_line = cursor_line.saturating_sub(FIM_DEFINITION_LINE_RADIUS);
    let last_line = cursor_line
        .saturating_add(FIM_DEFINITION_LINE_RADIUS)
        .min(line_count - 1);

    let mut identifiers = Vec::new();
    for line in first_line..=last_line {
        let line_start = rope.line_to_byte_idx(line, ropey::LineType::LF_CR);
        let text = rope.line(line, ropey::LineType::LF_CR).to_string();
        let content = text.trim_end_matches(['\r', '\n']);
        let mut token_start = None;

        for (byte_index, character) in content
            .char_indices()
            .chain(std::iter::once((content.len(), ' ')))
        {
            if token_start.is_none() {
                if character == '_' || character.is_alphabetic() {
                    token_start = Some(byte_index);
                }
                continue;
            }

            if character == '_' || character.is_alphanumeric() {
                continue;
            }

            let Some(start) = token_start.take() else {
                continue;
            };
            let name = &content[start..byte_index];
            let byte_start = line_start + start;
            let byte_end = line_start + byte_index;
            let distance = if cursor_byte < byte_start {
                byte_start - cursor_byte
            } else if cursor_byte > byte_end {
                cursor_byte - byte_end
            } else {
                0
            };
            let Ok(utf16_column) =
                u32::try_from(content[..start].encode_utf16().count())
            else {
                continue;
            };
            identifiers.push((
                distance,
                name.to_string(),
                Position::new(line as u32, utf16_column),
            ));
        }
    }

    identifiers.sort_by(
        |(left_distance, left_name, left_position),
         (right_distance, right_name, right_position)| {
            left_distance
                .cmp(right_distance)
                .then_with(|| left_position.line.cmp(&right_position.line))
                .then_with(|| left_position.character.cmp(&right_position.character))
                .then_with(|| left_name.cmp(right_name))
        },
    );

    let mut seen = HashSet::new();
    identifiers
        .into_iter()
        .filter_map(|(_, name, position)| {
            seen.insert(name.clone()).then_some((name, position))
        })
        .take(MAX_FIM_DEFINITION_REQUESTS)
        .collect()
}

fn prediction_identifier_positions_with_semantic_styles(
    rope: &Rope,
    cursor: Position,
    semantic_styles: Option<&SemanticStyles>,
    syntax_captures: Option<&[SyntaxCapture]>,
) -> Vec<(String, Position)> {
    let semantic_positions = semantic_styles.and_then(|semantic_styles| {
        prediction_semantic_identifier_positions(rope, cursor, semantic_styles)
    });
    if let Some(positions) = semantic_positions
        .as_ref()
        .filter(|items| !items.is_empty())
    {
        return positions.clone();
    }
    if let Some(captures) = syntax_captures {
        let positions =
            prediction_syntax_identifier_positions(rope, cursor, captures);
        if !positions.is_empty() {
            return positions;
        }
    }
    if let Some(positions) = semantic_positions {
        return positions;
    }

    prediction_identifier_positions(rope, cursor)
}

fn prediction_syntax_identifier_positions(
    rope: &Rope,
    cursor: Position,
    captures: &[SyntaxCapture],
) -> Vec<(String, Position)> {
    let text = rope.to_string();
    let line_count = rope.len_lines(ropey::LineType::LF_CR);
    if line_count == 0 {
        return Vec::new();
    }
    let cursor_line = (cursor.line as usize).min(line_count - 1);
    let cursor_line_start =
        rope.line_to_byte_idx(cursor_line, ropey::LineType::LF_CR);
    let cursor_text = rope.line(cursor_line, ropey::LineType::LF_CR).to_string();
    let cursor_content = cursor_text.trim_end_matches(['\r', '\n']);
    let cursor_byte = cursor_line_start
        + offset_utf16_to_utf8_str(cursor_content, cursor.character as usize);
    let first_line = cursor_line.saturating_sub(FIM_DEFINITION_LINE_RADIUS);
    let last_line = cursor_line
        .saturating_add(FIM_DEFINITION_LINE_RADIUS)
        .min(line_count - 1);

    let mut identifiers = Vec::new();
    for capture in captures {
        if capture.start >= capture.end
            || capture.end > text.len()
            || !is_prediction_identifier_syntax_scope(&capture.scope)
        {
            continue;
        }
        let Some(name) = text.get(capture.start..capture.end) else {
            continue;
        };
        if !is_prediction_identifier(name) {
            continue;
        }
        let line = rope.byte_to_line_idx(capture.start, ropey::LineType::LF_CR);
        if line < first_line || line > last_line {
            continue;
        }
        let line_start = rope.line_to_byte_idx(line, ropey::LineType::LF_CR);
        let line_text = rope.line(line, ropey::LineType::LF_CR).to_string();
        let line_content = line_text.trim_end_matches(['\r', '\n']);
        let Some(column_prefix) = line_content.get(..capture.start - line_start)
        else {
            continue;
        };
        let Ok(line) = u32::try_from(line) else {
            continue;
        };
        let Ok(character) = u32::try_from(column_prefix.encode_utf16().count())
        else {
            continue;
        };
        let distance = if cursor_byte < capture.start {
            capture.start - cursor_byte
        } else if cursor_byte > capture.end {
            cursor_byte - capture.end
        } else {
            0
        };
        identifiers.push((
            distance,
            name.to_owned(),
            Position::new(line, character),
        ));
    }

    identifiers.sort_by(
        |(left_distance, left_name, left_position),
         (right_distance, right_name, right_position)| {
            left_distance
                .cmp(right_distance)
                .then_with(|| left_position.line.cmp(&right_position.line))
                .then_with(|| left_position.character.cmp(&right_position.character))
                .then_with(|| left_name.cmp(right_name))
        },
    );
    let mut seen = HashSet::new();
    identifiers
        .into_iter()
        .filter_map(|(_, name, position)| {
            seen.insert(name.clone()).then_some((name, position))
        })
        .take(MAX_FIM_DEFINITION_REQUESTS)
        .collect()
}

fn is_prediction_identifier_syntax_scope(scope: &str) -> bool {
    matches!(
        scope
            .trim_start_matches('@')
            .split('.')
            .next()
            .unwrap_or_default(),
        "attribute"
            | "class"
            | "constant"
            | "constructor"
            | "enum"
            | "enumMember"
            | "event"
            | "field"
            | "function"
            | "identifier"
            | "interface"
            | "label"
            | "macro"
            | "method"
            | "module"
            | "namespace"
            | "parameter"
            | "property"
            | "struct"
            | "tag"
            | "type"
            | "typeAlias"
            | "typeParameter"
            | "variable"
    )
}

fn prediction_semantic_identifier_positions(
    rope: &Rope,
    cursor: Position,
    semantic_styles: &SemanticStyles,
) -> Option<Vec<(String, Position)>> {
    let text = rope.to_string();
    if semantic_styles.len != text.len()
        || !semantic_styles.styles.iter().any(|style| {
            style
                .style
                .fg_color
                .as_deref()
                .is_some_and(is_prediction_identifier_semantic_token_type)
        })
    {
        return None;
    }

    let line_count = rope.len_lines(ropey::LineType::LF_CR);
    if line_count == 0 {
        return Some(Vec::new());
    }
    let cursor_line = (cursor.line as usize).min(line_count - 1);
    let cursor_line_start =
        rope.line_to_byte_idx(cursor_line, ropey::LineType::LF_CR);
    let cursor_text = rope.line(cursor_line, ropey::LineType::LF_CR).to_string();
    let cursor_content = cursor_text.trim_end_matches(['\r', '\n']);
    let cursor_byte = cursor_line_start
        + offset_utf16_to_utf8_str(cursor_content, cursor.character as usize);
    let first_line = cursor_line.saturating_sub(FIM_DEFINITION_LINE_RADIUS);
    let last_line = cursor_line
        .saturating_add(FIM_DEFINITION_LINE_RADIUS)
        .min(line_count - 1);

    let mut identifiers = Vec::new();
    for style in &semantic_styles.styles {
        if style.start >= style.end || style.end > text.len() {
            continue;
        }
        let Some(token_type) = style.style.fg_color.as_deref() else {
            continue;
        };
        if !is_prediction_identifier_semantic_token_type(token_type) {
            continue;
        }
        let Some(name) = text.get(style.start..style.end) else {
            continue;
        };
        if !is_prediction_identifier(name) {
            continue;
        }
        let line = rope.byte_to_line_idx(style.start, ropey::LineType::LF_CR);
        if line < first_line || line > last_line {
            continue;
        }
        let line_start = rope.line_to_byte_idx(line, ropey::LineType::LF_CR);
        let line_text = rope.line(line, ropey::LineType::LF_CR).to_string();
        let line_content = line_text.trim_end_matches(['\r', '\n']);
        let Some(column_prefix) = line_content.get(..style.start - line_start)
        else {
            continue;
        };
        let Ok(line) = u32::try_from(line) else {
            continue;
        };
        let Ok(character) = u32::try_from(column_prefix.encode_utf16().count())
        else {
            continue;
        };
        let distance = if cursor_byte < style.start {
            style.start - cursor_byte
        } else if cursor_byte > style.end {
            cursor_byte - style.end
        } else {
            0
        };
        identifiers.push((
            distance,
            name.to_owned(),
            Position::new(line, character),
        ));
    }

    identifiers.sort_by(
        |(left_distance, left_name, left_position),
         (right_distance, right_name, right_position)| {
            left_distance
                .cmp(right_distance)
                .then_with(|| left_position.line.cmp(&right_position.line))
                .then_with(|| left_position.character.cmp(&right_position.character))
                .then_with(|| left_name.cmp(right_name))
        },
    );

    let mut seen = HashSet::new();
    Some(
        identifiers
            .into_iter()
            .filter_map(|(_, name, position)| {
                seen.insert(name.clone()).then_some((name, position))
            })
            .take(MAX_FIM_DEFINITION_REQUESTS)
            .collect(),
    )
}

fn is_prediction_identifier_semantic_token_type(token_type: &str) -> bool {
    matches!(
        token_type,
        "class"
            | "decorator"
            | "enum"
            | "enumMember"
            | "event"
            | "function"
            | "interface"
            | "macro"
            | "method"
            | "module"
            | "namespace"
            | "parameter"
            | "property"
            | "struct"
            | "type"
            | "typeAlias"
            | "typeParameter"
            | "variable"
    )
}

fn is_prediction_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters.next().is_some_and(|character| {
        character == '_' || character == '$' || character.is_alphabetic()
    }) && characters.all(|character| {
        character == '_' || character == '$' || character.is_alphanumeric()
    })
}

fn cached_prediction_definitions(
    cache: &Mutex<
        HashMap<PredictionDefinitionCacheKey, CachedPredictionDefinitions>,
    >,
    key: &PredictionDefinitionCacheKey,
    workspace: Option<&Path>,
    buffers: &HashMap<PathBuf, Buffer>,
) -> Option<Vec<(PathBuf, u32)>> {
    let mut cache = cache.lock();
    let workspace_root =
        workspace.and_then(|workspace| workspace.canonicalize().ok())?;
    let is_stale = {
        let entry = cache.get(key)?;
        entry.created.elapsed() > FIM_DEFINITION_CACHE_TTL
            || entry.locations.iter().any(|location| {
                open_buffer_for_resolved_path(
                    buffers,
                    &workspace_root,
                    workspace,
                    &location.path,
                )
                .is_none_or(|buffer| {
                    buffer.id != location.buffer_id
                        || buffer.rev != location.revision
                })
            })
    };
    if is_stale {
        cache.remove(key);
        return None;
    }

    let entry = cache.get(key)?;
    Some(
        entry
            .locations
            .iter()
            .map(|location| (location.path.clone(), location.line))
            .collect(),
    )
}

fn open_buffer_for_resolved_path<'a>(
    buffers: &'a HashMap<PathBuf, Buffer>,
    workspace_root: &Path,
    workspace: Option<&Path>,
    resolved_path: &Path,
) -> Option<&'a Buffer> {
    buffers.values().find(|buffer| {
        let relative = buffer.path.strip_prefix(workspace_root).ok().or_else(|| {
            workspace.and_then(|workspace| buffer.path.strip_prefix(workspace).ok())
        });
        relative
            .and_then(|relative| resolve_open_buffer_path(workspace_root, relative))
            .is_some_and(|open_path| open_path == resolved_path)
    })
}

fn cache_prediction_definitions(
    cache: &Mutex<
        HashMap<PredictionDefinitionCacheKey, CachedPredictionDefinitions>,
    >,
    key: PredictionDefinitionCacheKey,
    locations: &[(PathBuf, u32)],
    workspace: Option<&Path>,
    buffers: &HashMap<PathBuf, Buffer>,
) {
    let Some(workspace_root) =
        workspace.and_then(|workspace| workspace.canonicalize().ok())
    else {
        return;
    };

    let mut cached_locations = Vec::new();
    let mut cached_paths = HashSet::new();
    for (path, line) in locations {
        let relative = path.strip_prefix(&workspace_root).ok().or_else(|| {
            workspace.and_then(|workspace| path.strip_prefix(workspace).ok())
        });
        let Some(relative) = relative else {
            continue;
        };
        let Some(resolved_path) =
            resolve_open_buffer_path(&workspace_root, relative)
        else {
            continue;
        };
        if !ahead_core::search::is_agent_visible_path(
            &workspace_root,
            &resolved_path,
        ) {
            continue;
        }
        let Some(buffer) = open_buffer_for_resolved_path(
            buffers,
            &workspace_root,
            workspace,
            &resolved_path,
        ) else {
            continue;
        };
        if cached_paths.insert(resolved_path.clone()) {
            cached_locations.push(CachedPredictionDefinition {
                path: resolved_path,
                line: *line,
                buffer_id: buffer.id,
                revision: buffer.rev,
            });
        }
    }

    if !locations.is_empty() && cached_locations.is_empty() {
        return;
    }

    let mut cache = cache.lock();
    cache.retain(|_, entry| entry.created.elapsed() <= FIM_DEFINITION_CACHE_TTL);
    if cache.len() >= MAX_FIM_DEFINITION_CACHE_ENTRIES {
        let oldest_key = cache
            .iter()
            .min_by_key(|(_, entry)| entry.created)
            .map(|(key, _)| key.clone());
        if let Some(oldest_key) = oldest_key {
            cache.remove(&oldest_key);
        }
    }
    cache.insert(
        key,
        CachedPredictionDefinitions {
            created: Instant::now(),
            locations: cached_locations,
        },
    );
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

fn is_prediction_import_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("use ")
        || line.starts_with("pub use ")
        || line.starts_with("pub(crate) use ")
        || line.starts_with("mod ")
        || line.starts_with("import ")
        || line.starts_with("from ")
        || line.starts_with("export ")
        || line.starts_with("#include")
        || line.contains("import(")
        || line.contains("require(")
        || line.contains("include!(")
}

fn contains_prediction_identifier(source: &str, identifier: &str) -> bool {
    !identifier.is_empty()
        && source.match_indices(identifier).any(|(start, matched)| {
            let end = start + matched.len();
            let before = source[..start].chars().next_back();
            let after = source[end..].chars().next();
            before.is_none_or(|character| {
                !character.is_alphanumeric() && character != '_'
            }) && after.is_none_or(|character| {
                !character.is_alphanumeric() && character != '_'
            })
        })
}

fn prediction_definition_locations(
    response: GotoDefinitionResponse,
) -> Vec<(PathBuf, u32)> {
    fn push_location(locations: &mut Vec<(PathBuf, u32)>, uri: &Url, line: u32) {
        if let Ok(path) = uri.to_file_path() {
            locations.push((path, line));
        }
    }

    let mut locations = Vec::new();
    match response {
        GotoDefinitionResponse::Scalar(location) => {
            push_location(&mut locations, &location.uri, location.range.start.line);
        }
        GotoDefinitionResponse::Array(definitions) => {
            for location in definitions {
                push_location(
                    &mut locations,
                    &location.uri,
                    location.range.start.line,
                );
            }
        }
        GotoDefinitionResponse::Link(links) => {
            for link in links {
                push_location(
                    &mut locations,
                    &link.target_uri,
                    link.target_selection_range.start.line,
                );
            }
        }
    }
    locations
}

fn prediction_buffer_excerpt(
    buffer: &Buffer,
    definition_line: Option<u32>,
) -> String {
    let rope = &buffer.rope;
    let contents_len = rope.len();
    if contents_len <= MAX_FIM_OPEN_BUFFER_BYTES {
        return rope.to_string();
    }

    let target_byte = definition_line.and_then(|line| {
        let line_count = buffer.rope.len_lines(ropey::LineType::LF_CR);
        (line_count > 0).then(|| {
            buffer.rope.line_to_byte_idx(
                (line as usize).min(line_count - 1),
                ropey::LineType::LF_CR,
            )
        })
    });
    let mut start = target_byte
        .map(|offset| offset.saturating_sub(MAX_FIM_OPEN_BUFFER_BYTES / 2))
        .unwrap_or(0);
    start = rope_byte_offset_floor(rope, start);
    let mut end = start
        .saturating_add(MAX_FIM_OPEN_BUFFER_BYTES)
        .min(contents_len);
    end = rope_byte_offset_floor(rope, end);

    let mut excerpt = String::new();
    if start > 0 {
        excerpt.push_str("[open-buffer excerpt truncated]\n");
    }
    if let Some(line) = definition_line {
        excerpt.push_str(&format!(
            "[definition near line {}]\n",
            line.saturating_add(1)
        ));
    }
    excerpt.push_str(&rope.slice(start..end).to_string());
    if end < contents_len {
        excerpt.push_str("\n[open-buffer excerpt truncated]");
    }
    excerpt
}

fn relevant_open_buffer_contexts(
    buffers: &HashMap<PathBuf, Buffer>,
    workspace: &Path,
    active_path: &Path,
    active_rope: &Rope,
    definition_locations: &[(PathBuf, u32)],
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
    let definition_rows = definition_locations
        .iter()
        .filter_map(|(definition_path, line)| {
            let relative = definition_path
                .strip_prefix(&workspace_root)
                .or_else(|_| definition_path.strip_prefix(workspace))
                .ok()?;
            let path = resolve_open_buffer_path(&workspace_root, relative)?;
            Some((path, *line))
        })
        .fold(HashMap::new(), |mut rows, (path, line)| {
            rows.entry(path).or_insert(line);
            rows
        });
    let candidates = buffers
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
            let definition_line = definition_rows.get(&path).copied();
            let sibling = relative.parent() == active_relative.parent();
            Some((relative, buffer, definition_line, stem, sibling))
        })
        .collect::<Vec<_>>();
    let candidate_stems = candidates
        .iter()
        .filter(|(_, _, definition_line, stem, _)| {
            definition_line.is_none() && stem.chars().count() >= 3
        })
        .map(|(_, _, _, stem, _)| stem.clone())
        .collect::<HashSet<_>>();
    let mut mentioned_stems = HashSet::new();
    let mut imported_stems = HashSet::new();
    if !candidate_stems.is_empty() {
        for line in active_rope.lines(ropey::LineType::LF_CR) {
            let line = line.to_string();
            let is_import = is_prediction_import_line(&line);
            let line = line.to_lowercase();
            for stem in &candidate_stems {
                if !mentioned_stems.contains(stem)
                    && contains_prediction_identifier(&line, stem)
                {
                    mentioned_stems.insert(stem.clone());
                }
                if is_import
                    && !imported_stems.contains(stem)
                    && contains_prediction_identifier(&line, stem)
                {
                    imported_stems.insert(stem.clone());
                }
            }
        }
    }
    let mut candidates = candidates
        .into_iter()
        .filter_map(|(relative, buffer, definition_line, stem, sibling)| {
            let imported = imported_stems.contains(&stem);
            let mentioned = mentioned_stems.contains(&stem);
            let relevance = if definition_line.is_some() {
                4
            } else if imported {
                3
            } else if mentioned {
                2
            } else if sibling {
                1
            } else {
                0
            };
            (relevance > 0).then_some((relevance, relative, buffer, definition_line))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(
        |(left_relevance, left_path, _, _), (right_relevance, right_path, _, _)| {
            right_relevance
                .cmp(left_relevance)
                .then_with(|| left_path.cmp(right_path))
        },
    );

    candidates
        .into_iter()
        .take(MAX_FIM_OPEN_BUFFERS)
        .map(|(_, path, buffer, definition_line)| {
            let excerpt = prediction_buffer_excerpt(buffer, definition_line);
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
            .filter(|(_, (pending_session, _))| pending_session == session_id)
            .map(|(path, _)| path.display().to_string())
            .collect();
        anyhow::ensure!(
            paths.is_empty(),
            "Open and save collaborator edits before ending this share: {}",
            paths.join(", ")
        );
        Ok(())
    }

    fn cancel_inline_predictions(&self, path: Option<&Path>) {
        self.inline_prediction_jobs.lock().retain(|_, job| {
            if path.is_none_or(|path| job.dependencies.contains(path)) {
                job.cancellation.cancel();
                false
            } else {
                true
            }
        });
    }

    fn validate_inline_prediction(
        &self,
        path: &Path,
        position: Position,
        content_sha256: &str,
        replacement: &str,
        binding: &PredictionBinding,
    ) -> Result<bool> {
        let ready = {
            let mut ready = self.ready_predictions.lock();
            if !ready
                .get(path)
                .is_some_and(|ready| &ready.binding == binding)
            {
                return Ok(false);
            }
            ready.remove(path).context("prediction disappeared")?
        };
        let Some(target) = self.buffers.get(path) else {
            return Ok(false);
        };
        if ready.created.elapsed() > PREDICTION_LIFETIME
            || target.id != ready.target_id
            || target.rev < binding.target_revision
            || target.read_only
            || !is_prediction_target_visible(&ready.snapshot.workspace, path)
            || ready.dependencies.iter().any(|(path, version)| {
                self.buffers
                    .get(path)
                    .is_none_or(|buffer| (buffer.id, buffer.rev) != *version)
            })
        {
            return Ok(false);
        }
        let text = target.rope.to_string();
        if format!("{:x}", Sha256::digest(text.as_bytes())) != content_sha256 {
            return Ok(false);
        }
        let original = ready.target.to_string();
        let Some((prefix, suffix)) = original
            .get(..ready.insertion_offset)
            .zip(original.get(ready.insertion_offset..))
        else {
            return Ok(false);
        };
        let Some(typed) = text
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix(suffix))
        else {
            return Ok(false);
        };
        if typed.len() >= ready.replacement.len()
            || !ready.replacement.starts_with(typed)
            || ready.replacement.strip_prefix(typed) != Some(replacement)
            || target.offset_to_position(ready.insertion_offset + typed.len())
                != position
        {
            return Ok(false);
        }
        let host = self
            .ahead_host
            .as_ref()
            .context("prediction host disconnected")?;
        let model_context = ready.snapshot.model_context.clone();
        Ok(host.read().prepare_prediction_with_model_context(
            ready.request,
            &ready.open_buffers,
            model_context.as_ref(),
        )? == ready.snapshot)
    }

    fn close_editor_buffer(&mut self, path: &Path) {
        if self.shared_remote_pending.contains_key(path) {
            return;
        }
        self.cancel_inline_predictions(Some(path));
        self.ready_predictions.lock().remove(path);
        self.prediction_semantic_tokens.lock().remove_buffer(path);
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
        if self
            .buffers
            .get(&path)
            .is_none_or(|buffer| buffer.get_document() != content)
        {
            self.cancel_inline_predictions(Some(&path));
        }
        if let Some(buffer) = self.buffers.get_mut(&path) {
            if buffer.get_document() == content {
                self.prediction_semantic_tokens
                    .lock()
                    .track_buffer(&path, buffer);
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
            self.prediction_semantic_tokens
                .lock()
                .track_buffer(&path, buffer);
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
            self.prediction_semantic_tokens
                .lock()
                .track_buffer(&path, &buffer);
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

    fn sync_editor_delta(
        &mut self,
        path: PathBuf,
        delta: AheadDelta,
    ) -> Result<u64> {
        let rev = self
            .buffers
            .get(&path)
            .context("incremental edit for unopened editor buffer")?
            .rev
            .checked_add(1)
            .context("buffer revision overflow")?;
        let buffer = self
            .buffers
            .get_mut(&path)
            .context("incremental edit for unopened editor buffer")?;
        let old_text = buffer.rope.clone();
        let old_byte_len = old_text.chunks().map(str::len).sum::<usize>();
        anyhow::ensure!(
            delta.base_len() == old_byte_len,
            "editor delta base length does not match the open buffer"
        );
        buffer
            .update(&delta, rev)
            .context("editor delta revision mismatch")?;
        self.prediction_semantic_tokens
            .lock()
            .track_buffer(&path, buffer);
        self.catalog_rpc.did_change_text_document(
            &path,
            rev,
            delta,
            old_text,
            buffer.rope.clone(),
        );
        self.cancel_inline_predictions(Some(&path));
        Ok(rev)
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
            user_settings_watched: false,
            file_index: None,
            file_index_warmup: None,
            workspace_file_jobs: Arc::new(Mutex::new(HashMap::new())),
            shown_prediction_errors: Arc::new(Mutex::new(HashSet::new())),
            prediction_definition_cache: Arc::new(Mutex::new(HashMap::new())),
            prediction_semantic_tokens: Arc::new(Mutex::new(
                PredictionSemanticTokenCache::default(),
            )),
            inline_prediction_jobs: Arc::new(Mutex::new(HashMap::new())),
            ready_predictions: Arc::new(Mutex::new(HashMap::new())),
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
    user_settings_directory: Mutex<Option<PathBuf>>,
    user_settings_home: Option<PathBuf>,
    user_settings_watcher: Option<FileWatcherHandle>,
    file_index: Option<Arc<WorkspaceFileIndex>>,
    workspace_fs_change_handler: Arc<
        Mutex<
            Option<
                Sender<(bool, bool, bool, Vec<PathBuf>, Vec<(PathBuf, PathBuf)>)>,
            >,
        >,
    >,
    git_metadata_paths: Vec<PathBuf>,
    instruction_watch_paths: Mutex<HashSet<PathBuf>>,
    instruction_root_watch_paths: Mutex<HashSet<PathBuf>>,
}

fn instruction_watch_directories(
    workspace: &Path,
) -> (HashSet<PathBuf>, HashSet<PathBuf>) {
    let root = ahead_agent::project_instruction_root(workspace);
    let instruction_directories = workspace
        .ancestors()
        .skip(1)
        .take_while(|directory| directory.starts_with(&root))
        .map(Path::to_path_buf)
        .collect();
    // Keep the shallow ancestor chain watched so adding or removing a `.git`
    // marker can change the instruction boundary without restarting the session.
    let root_marker_directories = workspace
        .ancestors()
        .filter(|directory| directory.parent().is_some())
        .map(Path::to_path_buf)
        .collect();
    (instruction_directories, root_marker_directories)
}

fn reconcile_instruction_watches(
    watcher: &FileWatcherHandle,
    current_directories: &Mutex<HashSet<PathBuf>>,
    desired_directories: HashSet<PathBuf>,
    token: WatchToken,
    marker_name: &'static str,
) {
    let (removed, added) = {
        let mut current = current_directories.lock();
        let removed = current
            .difference(&desired_directories)
            .cloned()
            .collect::<Vec<_>>();
        let added = desired_directories
            .difference(&current)
            .cloned()
            .collect::<Vec<_>>();
        *current = desired_directories;
        (removed, added)
    };
    for directory in removed {
        watcher.unwatch(&directory, token);
    }
    for directory in added {
        let marker = directory.join(marker_name);
        watcher.watch_filtered(&directory, false, token, move |path| path == marker);
    }
}

fn workspace_file_rename(event: &notify::Event) -> Option<(PathBuf, PathBuf)> {
    if matches!(
        &event.kind,
        notify::EventKind::Modify(notify::event::ModifyKind::Name(
            notify::event::RenameMode::Both
        ))
    ) && event.paths.len() == 2
    {
        Some((event.paths[0].clone(), event.paths[1].clone()))
    } else {
        None
    }
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
        let workspace = workspace.map(|path| match path.canonicalize() {
            Ok(canonical) => canonical,
            Err(error) => {
                tracing::warn!(?error, path = %path.display(), "normalizing workspace watcher root");
                path
            }
        });
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
            user_settings_directory: Mutex::new(user_settings_directory()),
            user_settings_home: None,
            user_settings_watcher: None,
            file_index,
            core_rpc,
            proxy_rpc,
            catalog_rpc,
            workspace_fs_change_handler: Arc::new(Mutex::new(None)),
            git_metadata_paths,
            instruction_watch_paths: Mutex::default(),
            instruction_root_watch_paths: Mutex::default(),
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
                WORKSPACE_EVENT_TOKEN => {
                    if event.paths.iter().any(|path| {
                        matches!(
                            path.file_name().and_then(|name| name.to_str()),
                            Some("pyproject.toml" | "ruff.toml" | ".ruff.toml")
                        )
                    }) {
                        self.core_rpc.log(
                            ahead_rpc::core::LogLevel::Trace,
                            format!(
                                "workspace watcher delivered {:?} for {:?}",
                                event.kind, event.paths
                            ),
                            Some("ahead_proxy::dispatch::lsp_file_watch".into()),
                        );
                    }
                    self.handle_workspace_fs_event(event);
                }
                LSP_FILE_EVENT_TOKEN => self.notify_lsp_file_event(event),
                USER_SETTINGS_EVENT_TOKEN => {
                    self.handle_user_settings_fs_event(event)
                }
                USER_SETTINGS_HOME_EVENT_TOKEN => {
                    self.handle_user_settings_home_fs_event(event)
                }
                INSTRUCTION_EVENT_TOKEN => {
                    self.handle_instruction_fs_event(&event);
                }
                INSTRUCTION_ROOT_EVENT_TOKEN => {
                    self.handle_instruction_root_fs_event(&event);
                }
                _ => {}
            }
        }
    }

    fn watch_instruction_ancestors(&self, watcher: &mut FileWatcher) {
        let Some(workspace) = self.workspace.as_deref() else {
            return;
        };
        let (instruction_directories, root_marker_directories) =
            instruction_watch_directories(workspace);
        for directory in &instruction_directories {
            // Watch the directory, not the file: instructions may be absent at
            // startup or replaced atomically by an editor.
            let instruction = directory.join("AGENTS.md");
            watcher.watch_filtered(
                directory,
                false,
                INSTRUCTION_EVENT_TOKEN,
                move |path| path == instruction,
            );
        }
        for directory in &root_marker_directories {
            let root_marker = directory.join(".git");
            watcher.watch_filtered(
                directory,
                false,
                INSTRUCTION_ROOT_EVENT_TOKEN,
                move |path| path == root_marker,
            );
        }
        *self.instruction_watch_paths.lock() = instruction_directories;
        *self.instruction_root_watch_paths.lock() = root_marker_directories;
    }

    fn handle_instruction_root_fs_event(&self, event: &notify::Event) {
        if !matches!(
            event.kind,
            notify::EventKind::Create(_)
                | notify::EventKind::Remove(_)
                | notify::EventKind::Modify(_)
        ) {
            return;
        }
        let root_marker_watch_paths = self.instruction_root_watch_paths.lock();
        let is_root_marker_event = event.paths.iter().any(|path| {
            if path.file_name().is_none_or(|name| name != ".git") {
                return false;
            }
            let Some(directory) = path.parent() else {
                return false;
            };
            let directory = directory
                .canonicalize()
                .unwrap_or_else(|_| directory.to_path_buf());
            root_marker_watch_paths.contains(&directory)
        });
        drop(root_marker_watch_paths);
        if is_root_marker_event {
            self.refresh_instruction_ancestor_watches();
            self.invalidate_prediction_configuration();
        }
    }

    fn refresh_instruction_ancestor_watches(&self) {
        let Some(workspace) = self.workspace.as_deref() else {
            return;
        };
        let Some(watcher) = self.user_settings_watcher.as_ref() else {
            return;
        };
        let (instruction_directories, root_marker_directories) =
            instruction_watch_directories(workspace);
        reconcile_instruction_watches(
            watcher,
            &self.instruction_watch_paths,
            instruction_directories,
            INSTRUCTION_EVENT_TOKEN,
            "AGENTS.md",
        );
        reconcile_instruction_watches(
            watcher,
            &self.instruction_root_watch_paths,
            root_marker_directories,
            INSTRUCTION_ROOT_EVENT_TOKEN,
            ".git",
        );
    }

    fn handle_instruction_fs_event(&self, event: &notify::Event) {
        if !matches!(
            event.kind,
            notify::EventKind::Create(_)
                | notify::EventKind::Remove(_)
                | notify::EventKind::Modify(_)
        ) {
            return;
        }
        let Some(workspace) = self.workspace.as_deref() else {
            return;
        };
        let root = ahead_agent::project_instruction_root(workspace);
        if event.paths.iter().any(|path| {
            if path.file_name().is_none_or(|name| name != "AGENTS.md") {
                return false;
            }
            let Some(directory) = path.parent() else {
                return false;
            };
            let directory = directory
                .canonicalize()
                .unwrap_or_else(|_| directory.to_path_buf());
            directory.starts_with(workspace)
                || (workspace.starts_with(&directory)
                    && directory.starts_with(&root))
        }) {
            self.invalidate_prediction_configuration();
        }
    }

    fn invalidate_prediction_configuration(&self) {
        self.core_rpc.ahead_notification(
            ahead_rpc::ahead::AheadNotification::PredictionContextChanged {
                session_id: None,
            },
        );
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

    fn notify_lsp_file_event(&self, event: notify::Event) {
        self.core_rpc.log(
            ahead_rpc::core::LogLevel::Trace,
            format!(
                "LSP file watcher delivered {:?} for {:?}",
                event.kind, event.paths
            ),
            Some("ahead_proxy::dispatch::lsp_file_watch".into()),
        );
        if let Err(error) = self.catalog_rpc.catalog_notification(
            crate::plugin::PluginCatalogNotification::WatchedFilesChanged(event),
        ) {
            tracing::debug!(?error, "forwarding language-server file event");
        }
    }

    fn handle_workspace_fs_event(&self, event: notify::Event) {
        self.handle_instruction_fs_event(&event);
        let renames = workspace_file_rename(&event)
            .into_iter()
            .collect::<Vec<_>>();
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
        if settings_relevant {
            self.invalidate_prediction_configuration();
        }
        let notify_relevant =
            search_relevant || settings_relevant || !renames.is_empty();
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

        self.queue_filesystem_changes(
            notify_relevant,
            git_relevant,
            settings_relevant,
            event.paths,
            renames,
        );
    }

    fn handle_user_settings_fs_event(&self, event: notify::Event) {
        let is_settings_change = matches!(
            event.kind,
            notify::EventKind::Create(_)
                | notify::EventKind::Remove(_)
                | notify::EventKind::Modify(_)
        );
        let settings_directory = self.user_settings_directory.lock().clone();
        if !is_settings_change
            || !settings_directory.is_some_and(|directory| {
                event
                    .paths
                    .iter()
                    .any(|path| is_user_lsp_settings_path(&directory, path))
            })
        {
            return;
        }

        self.invalidate_prediction_configuration();
        self.queue_filesystem_changes(true, false, true, event.paths, Vec::new());
    }

    fn handle_user_settings_home_fs_event(&self, event: notify::Event) {
        let is_directory_change = matches!(
            event.kind,
            notify::EventKind::Create(_)
                | notify::EventKind::Remove(_)
                | notify::EventKind::Modify(notify::event::ModifyKind::Name(_))
        );
        let Some(home) = self.user_settings_home.as_deref() else {
            return;
        };
        if !is_directory_change
            || !event
                .paths
                .iter()
                .any(|path| is_user_ahead_directory(home, path))
        {
            return;
        }

        let next_directory = user_settings_directory_in(home);
        let mut current_directory = self.user_settings_directory.lock();
        if let Some(watcher) = &self.user_settings_watcher {
            if let Some(previous_directory) = current_directory.as_deref() {
                watcher.unwatch(previous_directory, USER_SETTINGS_EVENT_TOKEN);
            }
            if let Some(next_directory) = next_directory.as_deref() {
                let watched_directory = next_directory.to_owned();
                watcher.watch_filtered(
                    next_directory,
                    false,
                    USER_SETTINGS_EVENT_TOKEN,
                    move |path| is_user_lsp_settings_path(&watched_directory, path),
                );
            }
        }
        *current_directory = next_directory;
        drop(current_directory);
        self.invalidate_prediction_configuration();
        self.queue_filesystem_changes(true, false, true, event.paths, Vec::new());
    }

    fn queue_filesystem_changes(
        &self,
        notify_relevant: bool,
        git_relevant: bool,
        settings_relevant: bool,
        paths: Vec<PathBuf>,
        renames: Vec<(PathBuf, PathBuf)>,
    ) {
        let mut handler = self.workspace_fs_change_handler.lock();
        if let Some(sender) = handler.as_mut() {
            if let Err(err) = sender.send((
                notify_relevant,
                git_relevant,
                settings_relevant,
                paths,
                renames,
            )) {
                tracing::error!("{:?}", err);
            }
            return;
        }
        let (sender, receiver) = crossbeam_channel::unbounded();
        if let Err(err) = sender.send((
            notify_relevant,
            git_relevant,
            settings_relevant,
            paths,
            renames,
        )) {
            tracing::error!("{:?}", err);
        }

        let local_handler = self.workspace_fs_change_handler.clone();
        let core_rpc = self.core_rpc.clone();
        let catalog_rpc = self.catalog_rpc.clone();
        let file_index = self.file_index.clone();
        let workspace = self.workspace.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(500));

            {
                local_handler.lock().take();
            }

            let (notify_relevant, git_relevant, settings_relevant, paths, renames) =
                receiver.into_iter().fold(
                    (false, false, false, Vec::new(), Vec::new()),
                    |(notify, git, settings, mut paths, mut renames),
                     (
                        next_notify,
                        next_git,
                        next_settings,
                        next_paths,
                        next_renames,
                    )| {
                        paths.extend(next_paths);
                        renames.extend(next_renames);
                        (
                            notify || next_notify,
                            git || next_git,
                            settings || next_settings,
                            paths,
                            renames,
                        )
                    },
                );
            if notify_relevant {
                let generation = file_index
                    .as_ref()
                    .map(|index| index.generation())
                    .unwrap_or_default();
                let mut paths = paths;
                paths.sort();
                paths.dedup();
                let mut renames = renames;
                renames.dedup();
                for (old_path, new_path) in renames {
                    core_rpc.workspace_file_renamed(generation, old_path, new_path);
                }
                core_rpc.workspace_file_change(generation, paths);
            }
            if git_relevant && let Some(workspace) = workspace {
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
    let status = git_workspace_status(workspace_path)?;
    // Stage the non-conflicted paths explicitly. An `:(exclude)` pathspec for
    // an unmerged path makes `git add` fail with "did not match any files", so
    // the exclusion has to happen here instead.
    let paths = status
        .files
        .iter()
        .filter(|file| !git_status_is_conflicted(&file.state))
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return Ok(());
    }
    let mut command = std::process::Command::new("git");
    command
        .args(["--literal-pathspecs", "add", "-A", "--"])
        .args(&paths)
        .current_dir(workspace_path);
    let output = command.output()?;
    anyhow::ensure!(
        output.status.success(),
        "Could not stage non-conflicted changes: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

fn git_repository_status(repository_root: &Path) -> Result<(bool, bool)> {
    let output = std::process::Command::new("git")
        .args([
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--no-renames",
            "-z",
        ])
        .current_dir(repository_root)
        .output()
        .context("Could not start Git status")?;
    anyhow::ensure!(
        output.status.success(),
        "Git status failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let records = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty());
    let mut repository_dirty = false;
    let mut repository_has_conflicts = false;
    for record in records {
        anyhow::ensure!(
            record.len() >= 4 && record[2] == b' ',
            "Git returned malformed porcelain status"
        );
        repository_dirty = true;
        repository_has_conflicts |=
            git_status_bytes_are_conflicted(record[0], record[1]);
    }
    Ok((repository_dirty, repository_has_conflicts))
}

fn git_repository_dirty(repository_root: &Path) -> Result<bool> {
    Ok(git_repository_status(repository_root)?.0)
}

fn git_status_bytes_are_conflicted(index_status: u8, worktree_status: u8) -> bool {
    index_status == b'U'
        || worktree_status == b'U'
        || matches!((index_status, worktree_status), (b'A', b'A') | (b'D', b'D'))
}

fn git_status_is_conflicted(status: &str) -> bool {
    let mut status_bytes = status.bytes();
    let Some(index_status) = status_bytes.next() else {
        return false;
    };
    let Some(worktree_status) = status_bytes.next() else {
        return false;
    };
    git_status_bytes_are_conflicted(index_status, worktree_status)
}

fn git_workspace_status(workspace_path: &Path) -> Result<GitWorkspaceStatus> {
    let selected_root = workspace_path
        .canonicalize()
        .context("Could not resolve workspace folder")?;
    let repository = Repository::discover(&selected_root)?;
    let repository_root = repository
        .workdir()
        .context("Git repository has no working tree")?
        .canonicalize()
        .context("Could not resolve Git working tree")?;
    let (repository_dirty, repository_has_conflicts) =
        git_repository_status(&repository_root)?;
    let workspace_relative = selected_root
        .strip_prefix(&repository_root)
        .context("Workspace folder is outside the Git working tree")?;
    let status_prefix = workspace_relative
        .components()
        .map(|component| match component {
            std::path::Component::Normal(component) => component
                .to_str()
                .context("Workspace path is not valid UTF-8"),
            _ => anyhow::bail!("Workspace path is outside the Git working tree"),
        })
        .collect::<Result<Vec<_>>>()?
        .join("/");

    let branch_output = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(&repository_root)
        .output()
        .context("Could not start git branch")?;
    anyhow::ensure!(
        branch_output.status.success(),
        "Could not read Git branch: {}",
        String::from_utf8_lossy(&branch_output.stderr).trim()
    );
    let branch = String::from_utf8_lossy(&branch_output.stdout)
        .trim()
        .to_string();
    let branch_has_upstream = if branch.is_empty() {
        false
    } else {
        match repository.find_branch(&branch, git2::BranchType::Local) {
            Ok(branch) => match branch.upstream() {
                Ok(_) => true,
                Err(error) if error.code() == NotFound => false,
                Err(error) => return Err(error.into()),
            },
            Err(error) if error.code() == NotFound => false,
            Err(error) => return Err(error.into()),
        }
    };
    let remotes = repository
        .remotes()?
        .iter()
        .flatten()
        .map(str::to_owned)
        .collect();
    let mut branches = Vec::new();
    for branch in repository.branches(Some(git2::BranchType::Local))? {
        let (branch, _) = branch?;
        if let Some(name) = branch.name()? {
            branches.push(name.to_string());
        }
    }
    branches.sort();

    // Follow Zed's porcelain-v1 status query: NUL-delimited paths avoid Git's
    // quoted-path format, and no-renames keeps each file row unambiguous.
    let mut status_command = std::process::Command::new("git");
    status_command
        .args([
            "--literal-pathspecs",
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--no-renames",
            "-z",
            "--",
        ])
        .current_dir(&repository_root);
    if !status_prefix.is_empty() {
        status_command.arg(&status_prefix);
    }
    let output = status_command
        .output()
        .context("Could not start git status")?;
    anyhow::ensure!(
        output.status.success(),
        "Git status failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );

    let mut files = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| {
            anyhow::ensure!(
                record.len() >= 4 && record[2] == b' ',
                "Git returned malformed porcelain status"
            );
            let status = String::from_utf8_lossy(&record[..2]).trim().to_owned();
            let staged = !git_status_bytes_are_conflicted(record[0], record[1])
                && record[0] != b' '
                && record[0] != b'?';
            let path = String::from_utf8_lossy(&record[3..]);
            let path = if status_prefix.is_empty() {
                path.into_owned()
            } else {
                path.strip_prefix(&format!("{status_prefix}/"))
                    .context("Git returned a path outside the selected workspace")?
                    .to_owned()
            };
            Ok(GitStatusFile {
                path,
                state: status,
                staged,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(GitWorkspaceStatus {
        branch: if branch.is_empty() {
            "HEAD".to_string()
        } else {
            branch
        },
        branches,
        branch_has_upstream,
        remotes,
        repository_dirty,
        repository_has_conflicts,
        files,
    })
}

fn git_history(workspace_path: &Path, limit: u32) -> Result<Vec<GitCommitSummary>> {
    anyhow::ensure!(
        (1..=100).contains(&limit),
        "Git history limit must be between 1 and 100"
    );
    let repository = Repository::discover(workspace_path)?;
    let head = match repository.head() {
        Ok(head) => head,
        Err(error)
            if matches!(error.code(), git2::ErrorCode::UnbornBranch | NotFound) =>
        {
            return Ok(Vec::new());
        }
        Err(error) => return Err(error.into()),
    };
    let head = head
        .target()
        .context("Git HEAD does not point to a commit")?;
    let mut revisions = repository.revwalk()?;
    revisions.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;
    revisions.push(head)?;
    revisions
        .take(limit as usize)
        .map(|revision| {
            let commit = repository.find_commit(revision?)?;
            let author = commit.author();
            Ok(GitCommitSummary {
                id: commit.id().to_string(),
                author: author.name().unwrap_or("Unknown author").to_string(),
                timestamp: author.when().seconds(),
                subject: commit
                    .summary()
                    .unwrap_or("(no commit message)")
                    .to_string(),
            })
        })
        .collect()
}

const MAX_GIT_PATCH_BYTES: usize = 2 * 1024 * 1024;

fn bounded_git_patch(id: String, diff: &git2::Diff<'_>) -> Result<GitPatch> {
    let mut patch = String::new();
    let mut truncated = false;
    let print_result = diff.print(DiffFormat::Patch, |_, _, line| {
        // libgit2 hands back the line body without its diff prefix, and the
        // patch viewer (and `git apply`) rely on `+`/`-`/space to classify
        // lines, so restore it here.
        let origin = line.origin();
        let prefixed = matches!(origin, ' ' | '+' | '-');
        let content = String::from_utf8_lossy(line.content());
        let needed = content.len() + usize::from(prefixed);
        let remaining = MAX_GIT_PATCH_BYTES.saturating_sub(patch.len());
        if needed > remaining {
            let mut boundary = remaining.saturating_sub(usize::from(prefixed));
            while boundary > 0 && !content.is_char_boundary(boundary) {
                boundary -= 1;
            }
            if prefixed && remaining > 0 {
                patch.push(origin);
            }
            patch.push_str(&content[..boundary]);
            truncated = true;
            return false;
        }
        if prefixed {
            patch.push(origin);
        }
        patch.push_str(&content);
        true
    });
    if !truncated {
        print_result?;
    }
    Ok(GitPatch {
        id,
        patch,
        truncated,
    })
}

fn git_commit_patch(workspace_path: &Path, commit_id: &str) -> Result<GitPatch> {
    let commit_oid = Oid::from_str(commit_id).context("Invalid commit id")?;
    let repository = Repository::discover(workspace_path)?;
    let commit = repository
        .find_commit(commit_oid)
        .context("Commit was not found in this repository")?;
    let parent_tree = if commit.parent_count() == 0 {
        None
    } else {
        Some(commit.parent(0)?.tree()?)
    };
    let commit_tree = commit.tree()?;
    let mut options = DiffOptions::new();
    let diff = repository.diff_tree_to_tree(
        parent_tree.as_ref(),
        Some(&commit_tree),
        Some(&mut options),
    )?;
    bounded_git_patch(commit_oid.to_string(), &diff)
}

fn git_worktree_file_patch(
    workspace_path: &Path,
    relative_path: &Path,
) -> Result<GitPatch> {
    anyhow::ensure!(
        relative_path.is_relative()
            && !relative_path.as_os_str().is_empty()
            && relative_path.components().all(|component| matches!(
                component,
                std::path::Component::Normal(_)
            )),
        "Git diff path must be a non-empty workspace-relative path"
    );
    let selected_root = workspace_path
        .canonicalize()
        .context("Could not resolve workspace folder")?;
    let repository = Repository::discover(&selected_root)?;
    let repository_root = repository
        .workdir()
        .context("Git repository has no working directory")?
        .canonicalize()
        .context("Could not resolve Git working tree")?;
    let workspace_relative = selected_root
        .strip_prefix(&repository_root)
        .context("Workspace folder is outside the Git working tree")?;
    let repository_relative_path = workspace_relative.join(relative_path);
    let head_tree = match repository.head() {
        Ok(head) => Some(head.peel_to_commit()?.tree()?),
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
    let mut options = DiffOptions::new();
    options
        .pathspec(&repository_relative_path)
        .include_untracked(true)
        .recurse_untracked_dirs(false);
    let diff = repository
        .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut options))?;
    bounded_git_patch(relative_path.to_string_lossy().into_owned(), &diff)
}

fn git_worktree_patch(workspace_path: &Path) -> Result<GitPatch> {
    let selected_root = workspace_path
        .canonicalize()
        .context("Could not resolve workspace folder")?;
    let repository = Repository::discover(&selected_root)?;
    let repository_root = repository
        .workdir()
        .context("Git repository has no working directory")?
        .canonicalize()
        .context("Could not resolve Git working tree")?;
    let workspace_relative = selected_root
        .strip_prefix(&repository_root)
        .context("Workspace folder is outside the Git working tree")?;
    let head_tree = match repository.head() {
        Ok(head) => Some(head.peel_to_commit()?.tree()?),
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
    let mut options = DiffOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true)
        .max_size(MAX_GIT_PATCH_BYTES as i64);
    if !workspace_relative.as_os_str().is_empty() {
        options
            .pathspec(workspace_relative)
            .disable_pathspec_match(true);
    }
    let diff = repository
        .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut options))?;
    bounded_git_patch("working-tree".into(), &diff)
}

fn git_stage_paths(
    workspace_path: &Path,
    paths: &[String],
    staged: bool,
) -> Result<()> {
    anyhow::ensure!(!paths.is_empty(), "Select at least one changed file");
    for path in paths {
        anyhow::ensure!(
            !path.is_empty()
                && !path.as_bytes().contains(&0)
                && Path::new(path).is_relative()
                && Path::new(path).components().all(|component| matches!(
                    component,
                    std::path::Component::Normal(_)
                )),
            "Git path must stay within the workspace"
        );
    }
    let changed_paths = git_workspace_status(workspace_path)?
        .files
        .into_iter()
        .map(|file| file.path)
        .collect::<HashSet<_>>();
    anyhow::ensure!(
        paths.iter().all(|path| changed_paths.contains(path)),
        "Git status changed; refresh before staging"
    );

    let repository = Repository::discover(workspace_path)?;
    let unborn = match repository.head() {
        Ok(_) => false,
        Err(error) if error.code() == git2::ErrorCode::UnbornBranch => true,
        Err(error) => return Err(error.into()),
    };
    let mut command = std::process::Command::new("git");
    command
        .arg("--literal-pathspecs")
        .current_dir(workspace_path);
    if staged {
        command.args(["add", "-A", "--"]);
    } else if unborn {
        command.args(["update-index", "--force-remove", "--"]);
    } else {
        command.args(["reset", "--quiet", "--"]);
    }
    command.args(paths);
    let output = command.output().context("Could not update Git index")?;
    anyhow::ensure!(
        output.status.success(),
        "Could not {} files: {}",
        if staged { "stage" } else { "unstage" },
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
    let repository_root = repo
        .workdir()
        .context("Git repository has no working tree")?;
    anyhow::ensure!(
        !git_repository_status(repository_root)?.1,
        "Resolve and stage all conflicted files before committing"
    );
    let mut index = repo.index()?;
    stage_diffs(&mut index, workspace_path, diffs)?;
    index.write()?;
    match repo.signature() {
        Ok(_) => {}
        Err(error) if error.code() == NotFound => {
            return Err(anyhow!(
                "No user.name and/or user.email configured for this git repository."
            ));
        }
        Err(error) => {
            return Err(anyhow!(
                "Error while creating commit's signature: {}",
                error.message()
            ));
        }
    }
    match repo.head() {
        Ok(head) => {
            head.peel_to_commit()
                .context("HEAD does not point to a commit")?;
        }
        Err(error)
            if matches!(error.code(), NotFound | git2::ErrorCode::UnbornBranch) => {}
        Err(error) => return Err(error.into()),
    }

    // Let Git run the user's hooks and signing configuration. `--author`
    // changes only authorship; the configured human remains the committer.
    let commit_message = if agent_session_ids.is_empty() {
        message.to_string()
    } else {
        format!(
            "{message}\n\nAhead-Session: {}",
            agent_session_ids.join("\nAhead-Session: ")
        )
    };
    let mut command = std::process::Command::new("git");
    command
        .current_dir(workspace_path)
        .args(["commit", "--cleanup=strip", "-m"])
        .arg(commit_message);
    if !agent_session_ids.is_empty() {
        command.args(["--author", "ahead <ahead@ahead.local>"]);
    }
    let output = command.output().context("Could not start git commit")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let details = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    anyhow::ensure!(output.status.success(), "Git commit failed: {details}");
    Ok(())
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
        let cleanup = (|| -> Result<()> {
            let repo = Repository::discover(workspace_path)?;
            let (paths, contents) = committed_tree_changes(&repo)?;
            host.clear_committed_anchors(&paths, &contents)?;
            Ok(())
        })();
        if let Err(error) = cleanup {
            tracing::warn!(
                "failed to verify committed tree for anchor cleanup: {error}"
            );
        }
    }
    Ok(())
}

fn committed_tree_changes(
    repo: &Repository,
) -> Result<(Vec<String>, HashMap<String, String>)> {
    let commit = repo
        .head()
        .context("committed HEAD is unavailable")?
        .peel_to_commit()
        .context("committed HEAD does not point to a commit")?;
    let tree = commit.tree()?;
    let parent_tree = if commit.parent_count() > 0 {
        Some(commit.parent(0)?.tree()?)
    } else {
        None
    };
    let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)?;
    let mut paths = diff
        .deltas()
        .flat_map(|delta| [delta.old_file().path(), delta.new_file().path()])
        .flatten()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();

    let mut contents = HashMap::new();
    for path in &paths {
        let entry = match tree.get_path(Path::new(path)) {
            Ok(entry) => entry,
            Err(error) if error.code() == NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if entry.kind() != Some(git2::ObjectType::Blob) {
            continue;
        }
        let blob = repo.find_blob(entry.id())?;
        if let Ok(content) = std::str::from_utf8(blob.content()) {
            contents.insert(path.clone(), content.to_string());
        }
    }
    Ok((paths, contents))
}

fn git_checkout(workspace_path: &Path, reference: &str) -> Result<()> {
    let repo = Repository::discover(workspace_path)?;
    let repository_root = repo
        .workdir()
        .context("Git repository has no working tree")?;
    anyhow::ensure!(
        !git_repository_dirty(repository_root)?,
        "Commit, stash, or discard all repository changes before switching branches"
    );
    let branch = repo
        .find_branch(reference, git2::BranchType::Local)
        .with_context(|| format!("Local branch '{reference}' was not found"))?;
    let object = branch.get().peel(git2::ObjectType::Commit)?;
    let reference_name = branch
        .get()
        .name()
        .context("Git branch name is not valid UTF-8")?;
    let mut checkout = CheckoutBuilder::new();
    checkout.safe();
    repo.checkout_tree(&object, Some(&mut checkout))?;
    repo.set_head(reference_name)?;
    Ok(())
}

fn git_push(
    workspace_path: &Path,
    selected_remote: Option<&str>,
    core_rpc: CoreRpcHandler,
) -> Result<()> {
    let repository = Repository::discover(workspace_path)?;
    let repository_root = repository
        .workdir()
        .context("Git repository has no working tree")?;
    let head = repository
        .head()
        .context("Create a commit before publishing this branch")?;
    let branch = head
        .shorthand()
        .context("Publish requires a named local branch")?;
    let local_ref = head
        .name()
        .context("Current branch name is not valid UTF-8")?;
    anyhow::ensure!(
        local_ref.starts_with("refs/heads/"),
        "Publish requires a named local branch"
    );
    let branch_has_upstream =
        match repository.find_branch(branch, git2::BranchType::Local) {
            Ok(branch) => match branch.upstream() {
                Ok(_) => true,
                Err(error) if error.code() == NotFound => false,
                Err(error) => return Err(error.into()),
            },
            Err(error) if error.code() == NotFound => false,
            Err(error) => return Err(error.into()),
        };

    let config = repository.config()?;
    let configured_git_askpass = git_optional_config(&config, "core.askPass")?
        .is_some_and(|command| !command.trim().is_empty());
    let branch_prefix = format!("branch.{branch}.");
    let remote_names = repository
        .remotes()?
        .iter()
        .flatten()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let remote = if let Some(remote) = selected_remote {
        anyhow::ensure!(
            remote_names.iter().any(|name| name == remote),
            "Selected Git remote '{remote}' no longer exists"
        );
        remote.to_string()
    } else {
        let push_remote =
            git_optional_config(&config, &format!("{branch_prefix}pushRemote"))?
                .or(git_optional_config(&config, "remote.pushDefault")?)
                .or(git_optional_config(
                    &config,
                    &format!("{branch_prefix}remote"),
                )?);
        push_remote
            .or_else(|| remote_names.iter().find(|name| *name == "origin").cloned())
            .or_else(|| (remote_names.len() == 1).then(|| remote_names[0].clone()))
            .ok_or_else(|| {
                anyhow!(
                    "No push remote is configured. Add an 'origin' remote or set remote.pushDefault."
                )
            })?
    };
    anyhow::ensure!(
        repository.find_remote(&remote).is_ok(),
        "Selected Git remote '{remote}' was not found"
    );

    let remote_ref = git_optional_config(&config, &format!("{branch_prefix}merge"))?
        .unwrap_or_else(|| local_ref.to_string());
    anyhow::ensure!(
        remote_ref.starts_with("refs/heads/"),
        "Configured upstream '{remote_ref}' is not a remote branch"
    );

    let refspec = format!("{local_ref}:{remote_ref}");
    let mut command = std::process::Command::new("git");
    command.arg("push");
    if !branch_has_upstream {
        command.arg("--set-upstream");
    }
    command
        .arg(&remote)
        .arg(&refspec)
        .env("GIT_TERMINAL_PROMPT", "0")
        .current_dir(repository_root);
    let output = crate::git_askpass::run_git_command(
        &mut command,
        core_rpc,
        configured_git_askpass,
    )?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let details = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    anyhow::ensure!(
        output.status.success(),
        "Git push failed: {}",
        if details.is_empty() {
            format!("process exited with {}", output.status)
        } else {
            details
        }
    );
    Ok(())
}

fn git_optional_config(config: &git2::Config, key: &str) -> Result<Option<String>> {
    match config.get_string(key) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.code() == NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn git_create_branch(workspace_path: &Path, reference: &str) -> Result<()> {
    anyhow::ensure!(
        !reference.trim().is_empty() && reference == reference.trim(),
        "Enter a valid local branch name"
    );
    let reference_name = format!("refs/heads/{reference}");
    anyhow::ensure!(
        git2::Reference::is_valid_name(&reference_name),
        "Invalid local branch name '{reference}'"
    );
    let repo = Repository::discover(workspace_path)?;
    let repository_root = repo
        .workdir()
        .context("Git repository has no working tree")?;
    anyhow::ensure!(
        !git_repository_dirty(repository_root)?,
        "Commit, stash, or discard all repository changes before creating a branch"
    );
    match repo.find_branch(reference, git2::BranchType::Local) {
        Ok(_) => anyhow::bail!("Local branch '{reference}' already exists"),
        Err(error) if error.code() == NotFound => {}
        Err(error) => return Err(error.into()),
    }

    match repo.head() {
        Ok(head) => {
            let commit = head.peel_to_commit()?;
            repo.branch(reference, &commit, false)?;
        }
        Err(error)
            if matches!(error.code(), git2::ErrorCode::UnbornBranch | NotFound) => {}
        Err(error) => return Err(error.into()),
    }
    repo.set_head(&reference_name)?;
    Ok(())
}

fn git_delete_branch(workspace_path: &Path, reference: &str) -> Result<()> {
    anyhow::ensure!(
        !reference.trim().is_empty() && reference == reference.trim(),
        "Select a valid local branch"
    );
    anyhow::ensure!(
        git2::Reference::is_valid_name(&format!("refs/heads/{reference}")),
        "Invalid local branch name '{reference}'"
    );
    let repo = Repository::discover(workspace_path)?;
    let repository_root = repo
        .workdir()
        .context("Git repository has no working tree")?;
    anyhow::ensure!(
        !git_repository_dirty(repository_root)?,
        "Commit, stash, or discard all repository changes before deleting a branch"
    );
    let head = repo
        .head()
        .context("Cannot delete a branch before the repository has a commit")?;
    anyhow::ensure!(
        head.shorthand() != Some(reference),
        "Switch to another branch before deleting the current branch"
    );
    let head_commit = head.peel_to_commit()?;
    let mut branch = repo
        .find_branch(reference, git2::BranchType::Local)
        .with_context(|| format!("Local branch '{reference}' was not found"))?;
    let branch_commit = branch
        .get()
        .peel_to_commit()
        .with_context(|| format!("Local branch '{reference}' has no commit"))?;
    anyhow::ensure!(
        head_commit.id() == branch_commit.id()
            || repo.graph_descendant_of(head_commit.id(), branch_commit.id())?,
        "Branch '{reference}' has commits not merged into the current branch"
    );
    branch.delete()?;
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
        Err(error) if error.code() == git2::ErrorCode::UnbornBranch => repo
            .find_reference("HEAD")
            .ok()
            .and_then(|head| head.symbolic_target().map(str::to_owned))
            .and_then(|target| target.strip_prefix("refs/heads/").map(str::to_owned))
            .unwrap_or_else(|| "(No branch)".to_owned()),
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
        AHEAD_GITIGNORE, Buffer, CachedPredictionDefinitions, Dispatcher,
        FileWatchNotifier, GotoDefinitionResponse, INSTRUCTION_EVENT_TOKEN,
        MAX_FIM_OPEN_BUFFERS, PredictionDefinitionCacheKey,
        PredictionSemanticTokenCache, USER_SETTINGS_HOME_EVENT_TOKEN,
        WORKSPACE_EVENT_TOKEN, agent_request_runs_in_background,
        cache_prediction_definitions, cached_prediction_definitions,
        ensure_ahead_gitignore, git_checkout, git_commit, git_commit_patch,
        git_create_branch, git_delete_branch, git_diff_new, git_history, git_push,
        git_stage_all, git_stage_paths, git_status_bytes_are_conflicted,
        git_workspace_status, git_worktree_file_patch, git_worktree_patch,
        is_prediction_target_visible, is_provider_settings_path,
        is_user_ahead_directory, is_user_lsp_settings_path,
        prediction_definition_locations, prediction_identifier_positions,
        prediction_identifier_positions_with_semantic_styles,
        prediction_prefix_suffix, relevant_open_buffer_contexts, search_in_path,
        workspace_file_rename,
    };
    use crate::plugin::{PluginCatalogRpc, PluginCatalogRpcHandler};
    use crate::watcher::FileWatcher;
    use ahead_core::search::WorkspaceFileIndex;
    use ahead_rpc::{
        ahead::{AheadRequest, DisplayPosition, DisplayRange},
        buffer::BufferId,
        core::{CoreNotification, CoreRpc, CoreRpcHandler},
        delta::{AheadDelta, DeltaOp},
        file::{EditorRecoverySnapshot, EditorRecoverySummary},
        proxy::{
            ProxyHandler, ProxyNotification, ProxyRequest, ProxyResponse, ProxyRpc,
            ProxyRpcHandler,
        },
        source_control::{FileDiff, GitStatusFile},
    };
    use git2::Repository;
    use ropey::Rope;
    use std::{
        collections::HashMap,
        fs,
        path::{Path, PathBuf},
        sync::Arc,
        sync::atomic::{AtomicBool, AtomicU64, Ordering},
        time::{Duration, Instant},
    };
    use url::Url;

    fn wait_for_prediction_context(receiver: &crossbeam_channel::Receiver<CoreRpc>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let message = receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("instruction changes should invalidate prediction context");
            if matches!(message, CoreRpc::Notification(notification)
            if matches!(*notification, CoreNotification::AheadNotification {
                notification: ahead_rpc::ahead::AheadNotification::PredictionContextChanged { session_id: None }
            })) {
                return;
            }
        }
    }

    #[test]
    fn workspace_rename_preserves_from_to_path_order() {
        let old_path = PathBuf::from("src/old.rs");
        let new_path = PathBuf::from("src/new.rs");
        let event = notify::Event::new(notify::EventKind::Modify(
            notify::event::ModifyKind::Name(notify::event::RenameMode::Both),
        ))
        .add_path(old_path.clone())
        .add_path(new_path.clone());

        assert_eq!(workspace_file_rename(&event), Some((old_path, new_path)));
    }

    #[test]
    fn external_acp_catalog_refresh_does_not_block_proxy_dispatch() {
        assert!(agent_request_runs_in_background(
            &AheadRequest::ListExternalAcpAdapters
        ));
        assert!(!agent_request_runs_in_background(
            &AheadRequest::ListSessions
        ));
    }

    fn ready_prediction_fixture() -> (
        tempfile::TempDir,
        Dispatcher,
        std::path::PathBuf,
        ahead_rpc::ahead::PredictionBinding,
    ) {
        use super::{OpenBufferContext, PredictionRequest, ReadyPrediction};
        let directory =
            tempfile::tempdir().expect("disposable prediction workspace");
        let workspace = directory.path().canonicalize().expect("workspace");
        fs::create_dir(workspace.join(".ahead")).expect("private settings");
        fs::write(workspace.join("AGENTS.md"), "Preserve retries.")
            .expect("instructions");
        fs::write(workspace.join(".ahead/settings.toml"), "[ai]\nactive_connection='binding-test'\n[[ai.connections]]\nname='binding-test'\nprovider_id='binding-test'\nprovider='openai-compatible'\nbase_url='http://127.0.0.1:1/v1'\nmodel='generic-model'\napi_key='SYNTHETIC-KEY-NOT-FOR-THE-WIRE'\n").expect("provider route");
        let host = Arc::new(parking_lot::RwLock::new(
            crate::ahead::host::AheadSessionHost::in_memory().expect("host"),
        ));
        host.read().set_workspace(workspace.clone());
        let view = host
            .read()
            .start_work(
                Some(ahead_rpc::ahead::WorkKind::ProductChange),
                "Retries".into(),
                "Preserve idempotency".into(),
                None,
            )
            .expect("session");
        let mut dispatcher =
            Dispatcher::new(CoreRpcHandler::new(), ProxyRpcHandler::new());
        dispatcher.workspace = Some(workspace.clone());
        dispatcher.ahead_host = Some(host.clone());
        let path = workspace.join("main.rs");
        dispatcher
            .sync_editor_snapshot(path.clone(), "fn ".into())
            .expect("active buffer");
        let helper = workspace.join("helper.rs");
        dispatcher
            .sync_editor_snapshot(helper, "fn helper() {}".into())
            .expect("related buffer");
        let request = PredictionRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            session_id: Some(view.session.id),
            path: path.to_string_lossy().into_owned(),
            cursor: DisplayPosition { line: 0, col: 3 },
            prefix: "fn ".into(),
            suffix: String::new(),
            work_context: "ahead-editor-flow".into(),
        };
        let open_buffers = vec![OpenBufferContext {
            path: "helper.rs".into(),
            relevant_excerpt: "fn helper() {}".into(),
        }];
        let snapshot = host
            .read()
            .prepare_prediction(request.clone(), &open_buffers)
            .expect("snapshot");
        let serialized =
            serde_json::to_string(&snapshot).expect("fingerprint input");
        assert!(!serialized.contains("SYNTHETIC-KEY-NOT-FOR-THE-WIRE"));
        assert!(!serialized.contains("api_key"));
        let ready = ReadyPrediction::new(
            snapshot,
            request,
            open_buffers,
            &dispatcher.buffers,
            &dispatcher.buffers[&path],
            "main() {}".into(),
        )
        .expect("ready prediction");
        let binding = ready.publish(path.clone(), &dispatcher.ready_predictions);
        (directory, dispatcher, path, binding)
    }

    #[test]
    fn prediction_acceptance_rechecks_the_host_and_target_without_notifications() {
        use sha2::{Digest, Sha256};
        for change in [
            "none",
            "typed-prefix",
            "unrelated",
            "target-edit",
            "dependency-edit",
            "reopen",
            "instructions",
            "provider",
            "credential",
            "session",
            "position",
            "content-hash",
            "replacement",
            "version",
            "fingerprint",
            "expired",
        ] {
            let (_directory, mut dispatcher, path, mut binding) =
                ready_prediction_fixture();
            let workspace = path.parent().expect("workspace");
            let mut position = lsp_types::Position::new(0, 3);
            match change {
                "none" | "content-hash" | "replacement" => {}
                "typed-prefix" => {
                    dispatcher
                        .sync_editor_snapshot(path.clone(), "fn ma".into())
                        .expect("matching typing");
                    position.character = 5;
                }
                "unrelated" => {
                    dispatcher
                        .sync_editor_snapshot(
                            workspace.join("other.md"),
                            "changed".into(),
                        )
                        .expect("unrelated change");
                }
                "target-edit" => {
                    dispatcher
                        .sync_editor_snapshot(path.clone(), "fn bad".into())
                        .expect("divergence");
                }
                "dependency-edit" => {
                    dispatcher
                        .sync_editor_snapshot(
                            workspace.join("helper.rs"),
                            "changed".into(),
                        )
                        .expect("consumed change");
                }
                "reopen" => {
                    dispatcher.close_editor_buffer(&workspace.join("helper.rs"));
                    dispatcher
                        .sync_editor_snapshot(
                            workspace.join("helper.rs"),
                            "fn helper() {}".into(),
                        )
                        .expect("new buffer identity");
                }
                "instructions" => {
                    fs::write(workspace.join("AGENTS.md"), "Do not retry.")
                        .expect("changed instructions")
                }
                "provider" | "credential" => {
                    let settings = workspace.join(".ahead/settings.toml");
                    let text = fs::read_to_string(&settings).expect("settings");
                    fs::write(
                        settings,
                        if change == "provider" {
                            text.replace("generic-model", "new-model")
                        } else {
                            text.replace(
                                "SYNTHETIC-KEY-NOT-FOR-THE-WIRE",
                                "REPLACED-SYNTHETIC-KEY",
                            )
                        },
                    )
                    .expect("route change");
                }
                "session" => {
                    let session = dispatcher.ready_predictions.lock()[&path]
                        .request
                        .session_id
                        .clone()
                        .expect("session");
                    dispatcher
                        .ahead_host
                        .as_ref()
                        .expect("host")
                        .read()
                        .archive_session(&session)
                        .expect("changed lifecycle");
                }
                "position" => position.character = 0,
                "version" => binding.version += 1,
                "fingerprint" => binding.context_sha256 = "wrong".into(),
                "expired" => {
                    dispatcher
                        .ready_predictions
                        .lock()
                        .get_mut(&path)
                        .expect("cached binding")
                        .created = Instant::now()
                        .checked_sub(
                            super::PREDICTION_LIFETIME + Duration::from_secs(1),
                        )
                        .expect("expired instant")
                }
                _ => unreachable!(),
            }
            let digest = if change == "content-hash" {
                "wrong".into()
            } else {
                format!(
                    "{:x}",
                    Sha256::digest(
                        dispatcher.buffers[&path].rope.to_string().as_bytes()
                    )
                )
            };
            let replacement = if change == "typed-prefix" {
                "in() {}"
            } else if change == "replacement" {
                "forged_completion()"
            } else {
                "main() {}"
            };
            let outcome = dispatcher.validate_inline_prediction(
                &path,
                position,
                &digest,
                replacement,
                &binding,
            );
            if change == "session" {
                assert!(
                    outcome
                        .expect_err("archived context")
                        .to_string()
                        .contains("Archived")
                );
                continue;
            }
            let valid = outcome.expect("authoritative validation");
            assert_eq!(
                valid,
                matches!(change, "none" | "typed-prefix" | "unrelated"),
                "{change}"
            );
            if valid {
                assert!(
                    !dispatcher
                        .validate_inline_prediction(
                            &path,
                            position,
                            &digest,
                            replacement,
                            &binding
                        )
                        .expect("one-shot binding")
                );
            }
        }
    }

    #[test]
    fn prediction_ready_cache_is_bounded_and_does_not_retire_a_newer_binding() {
        let (_directory, dispatcher, path, old) = ready_prediction_fixture();
        let mut ready = dispatcher
            .ready_predictions
            .lock()
            .remove(&path)
            .expect("ready prediction");
        ready.binding.request_id = "newer".into();
        let newer = ready.publish(path.clone(), &dispatcher.ready_predictions);
        assert!(
            !dispatcher
                .validate_inline_prediction(
                    &path,
                    lsp_types::Position::new(0, 3),
                    "unused",
                    "main() {}",
                    &old
                )
                .expect("old token")
        );
        assert_eq!(dispatcher.ready_predictions.lock()[&path].binding, newer);
        for index in 0..=super::MAX_READY_PREDICTIONS {
            let (_directory, fixture, fixture_path, _) = ready_prediction_fixture();
            let ready = fixture
                .ready_predictions
                .lock()
                .remove(&fixture_path)
                .expect("ready entry");
            ready.publish(
                path.with_file_name(format!("{index}.rs")),
                &dispatcher.ready_predictions,
            );
        }
        assert_eq!(
            dispatcher.ready_predictions.lock().len(),
            super::MAX_READY_PREDICTIONS
        );
    }

    #[test]
    fn prediction_binding_reuses_bounded_excerpts_for_large_unicode_targets() {
        use super::ReadyPrediction;
        use sha2::{Digest, Sha256};
        let (_directory, mut dispatcher, path, _) = ready_prediction_fixture();
        let previous = dispatcher
            .ready_predictions
            .lock()
            .remove(&path)
            .expect("previous binding");
        let text = format!("{}fn ", "// café 日本語\n".repeat(8000));
        dispatcher
            .sync_editor_snapshot(path.clone(), text.clone())
            .expect("large unsaved target");
        let target = &dispatcher.buffers[&path];
        let position = target.offset_to_position(text.len());
        let (prefix, suffix) = prediction_prefix_suffix(
            &target.rope,
            lsp_types::Position::new(position.line, position.character),
        );
        let mut request = previous.request;
        request.prefix = prefix;
        request.suffix = suffix;
        request.cursor = DisplayPosition {
            line: position.line,
            col: position.character,
        };
        let snapshot = dispatcher
            .ahead_host
            .as_ref()
            .expect("host")
            .read()
            .prepare_prediction(request.clone(), &previous.open_buffers)
            .expect("large context");
        let ready = ReadyPrediction::new(
            snapshot,
            request,
            previous.open_buffers,
            &dispatcher.buffers,
            target,
            "main() {}".into(),
        )
        .expect("bounded binding");
        assert!(ready.request.prefix.len() < text.len());
        assert_eq!(ready.insertion_offset, text.len());
        let binding = ready.publish(path.clone(), &dispatcher.ready_predictions);
        assert!(
            dispatcher
                .validate_inline_prediction(
                    &path,
                    position,
                    &format!("{:x}", Sha256::digest(text.as_bytes())),
                    "main() {}",
                    &binding
                )
                .expect("Unicode target acceptance")
        );
    }

    #[test]
    fn prediction_provider_reply_carries_a_binding_through_acceptance_rpc() {
        use sha2::{Digest, Sha256};
        use std::io::{BufRead, Read, Write};
        let (_directory, mut dispatcher, path, _) = ready_prediction_fixture();
        let listener =
            std::net::TcpListener::bind(("127.0.0.1", 0)).expect("local provider");
        listener.set_nonblocking(true).expect("bounded listener");
        let address = listener.local_addr().expect("address");
        let settings = path
            .parent()
            .expect("workspace")
            .join(".ahead/settings.toml");
        fs::write(
            &settings,
            fs::read_to_string(&settings)
                .expect("settings")
                .replace("127.0.0.1:1", &address.to_string()),
        )
        .expect("local route");
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("provider connection: {error}"),
                }
            };
            stream
                .set_nonblocking(false)
                .expect("blocking provider reads");
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .expect("bounded reads");
            let mut reader = std::io::BufReader::new(stream);
            let mut length = 0;
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).expect("header") > 0);
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().expect("length");
                }
            }
            let mut bytes = vec![0; length];
            reader.read_exact(&mut bytes).expect("outbound prompt");
            let body: serde_json::Value =
                serde_json::from_slice(&bytes).expect("provider request");
            assert!(
                body["prompt"]
                    .as_str()
                    .expect("prompt")
                    .contains("Preserve retries.")
            );
            let response = r#"{"choices":[{"text":"main() {}"}]}"#;
            write!(reader.get_mut(), "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).expect("completion response");
        });
        let rpc = dispatcher.proxy_rpc.clone();
        let catalog = dispatcher.catalog_rpc.test_receiver();
        catalog.try_iter().for_each(drop);
        let session_id = dispatcher.ready_predictions.lock()[&path]
            .request
            .session_id
            .clone();
        let (sender, receiver) = crossbeam_channel::bounded(1);
        rpc.get_inline_completions(
            path.clone(),
            lsp_types::Position::new(0, 3),
            lsp_types::InlineCompletionTriggerKind::Automatic,
            session_id,
            None,
            move |result| sender.send(result).expect("prediction outcome"),
        );
        let ProxyRpc::Request(id, request) = rpc.rx().try_recv().expect("request")
        else {
            panic!("request")
        };
        dispatcher.handle_request(id, request);
        let PluginCatalogRpc::ServerRequest { f, .. } = catalog
            .recv_timeout(Duration::from_secs(2))
            .expect("LSP inline request")
        else {
            panic!("LSP request")
        };
        f(ahead_rpc::plugin::PluginId(0), Ok(serde_json::json!([])));
        let response = receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("prediction response")
            .expect("prediction success");
        let ProxyResponse::GetInlineCompletions {
            prediction: Some(binding),
            ..
        } = response
        else {
            panic!("host must bind FIM output")
        };
        assert_eq!(binding.version, 1);
        assert_eq!(binding.context_sha256.len(), 64);
        let request = ProxyRequest::ValidateInlinePrediction {
            path: path.clone(),
            position: lsp_types::Position::new(0, 3),
            content_sha256: format!("{:x}", Sha256::digest(b"fn ")),
            replacement: "main() {}".into(),
            binding,
        };
        let request: ProxyRequest = serde_json::from_str(
            &serde_json::to_string(&request).expect("encode validation"),
        )
        .expect("decode validation");
        let (sender, receiver) = crossbeam_channel::bounded(1);
        rpc.request_async(request, move |result| {
            sender.send(result).expect("validation outcome")
        });
        let ProxyRpc::Request(id, request) =
            rpc.rx().try_recv().expect("validation")
        else {
            panic!("validation request")
        };
        dispatcher.handle_request(id, request);
        assert!(matches!(
            receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("validation response")
                .expect("validation success"),
            ProxyResponse::ValidateInlinePrediction { valid: true }
        ));
        assert_eq!(
            dispatcher.buffers[&path].rope.to_string(),
            "fn ",
            "validation never applies an editor change"
        );
        server.join().expect("local provider");
    }

    #[test]
    fn inline_prediction_cancellation_tracks_only_consumed_buffers() {
        use super::InlinePredictionJob;
        use crate::ahead::prediction::PredictionCancellation;
        let directory = tempfile::tempdir().expect("disposable FIM workspace");
        let active = directory.path().join("main.rs");
        let related = directory.path().join("helper.rs");
        let unrelated = directory.path().join("other/readme.md");
        let mut dispatcher =
            Dispatcher::new(CoreRpcHandler::new(), ProxyRpcHandler::new());
        dispatcher
            .sync_editor_snapshot(active.clone(), "fn main() {}".into())
            .expect("active buffer");
        dispatcher
            .sync_editor_snapshot(related.clone(), "fn helper() {}".into())
            .expect("related buffer");
        for action in ["related edit", "close", "shutdown"] {
            let cancellation = PredictionCancellation::default();
            dispatcher.inline_prediction_jobs.lock().insert(
                1,
                InlinePredictionJob {
                    path: active.clone(),
                    dependencies: [active.clone(), related.clone()]
                        .into_iter()
                        .collect(),
                    cancellation: cancellation.clone(),
                },
            );
            dispatcher
                .sync_editor_snapshot(unrelated.clone(), action.into())
                .expect("unrelated edit");
            assert!(
                !cancellation.is_cancelled(),
                "unrelated buffer must not cancel FIM"
            );
            dispatcher
                .sync_editor_snapshot(active.clone(), "fn main() {}".into())
                .expect("unchanged snapshot");
            assert!(
                !cancellation.is_cancelled(),
                "unchanged snapshot must not cancel FIM"
            );
            match action {
                "related edit" => {
                    dispatcher
                        .sync_editor_snapshot(
                            related.clone(),
                            "fn changed() {}".into(),
                        )
                        .expect("changed dependency");
                }
                "close" => dispatcher.close_editor_buffer(&related),
                "shutdown" => {
                    dispatcher.handle_notification(ProxyNotification::Shutdown {})
                }
                _ => unreachable!(),
            }
            assert!(cancellation.is_cancelled(), "{action}");
            assert!(dispatcher.inline_prediction_jobs.lock().is_empty());
        }
    }

    #[test]
    fn inline_prediction_releases_the_host_and_aborts_after_an_editor_change() {
        use std::io::{BufRead, Read};
        for explicit_cancel in [false, true] {
            let directory = tempfile::tempdir().expect("disposable FIM project");
            let workspace = directory.path().canonicalize().expect("workspace");
            fs::create_dir(workspace.join(".ahead"))
                .expect("private settings directory");
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
                .expect("local prediction provider");
            listener.set_nonblocking(true).expect("bounded accept");
            let address = listener.local_addr().expect("provider address");
            let connection = format!("fim-cancel-test-{}", uuid::Uuid::new_v4());
            fs::write(workspace.join(".ahead/settings.toml"), format!(
            "[ai]\nactive_connection = '{connection}'\n[[ai.connections]]\nname = '{connection}'\nprovider_id = '{connection}'\nprovider = 'openai-compatible'\nbase_url = 'http://{address}/v1'\nmodel = 'generic-model'\n",
        )).expect("isolated prediction endpoint");
            let host = Arc::new(parking_lot::RwLock::new(
                crate::ahead::host::AheadSessionHost::in_memory()
                    .expect("session host"),
            ));
            host.read().set_workspace(workspace.clone());
            let rpc = ProxyRpcHandler::new();
            let mut dispatcher = Dispatcher::new(CoreRpcHandler::new(), rpc.clone());
            dispatcher.workspace = Some(workspace.clone());
            dispatcher.ahead_host = Some(host.clone());
            let active = workspace.join("main.rs");
            dispatcher
                .sync_editor_snapshot(active.clone(), "fn ".into())
                .expect("active buffer");
            let catalog = dispatcher.catalog_rpc.test_receiver();
            drop(catalog.try_iter().collect::<Vec<_>>());
            let (sender, receiver) = crossbeam_channel::bounded(1);
            rpc.get_inline_completions(
                active.clone(),
                lsp_types::Position::new(0, 3),
                lsp_types::InlineCompletionTriggerKind::Automatic,
                None,
                None,
                move |result| sender.send(result).expect("inline response"),
            );
            let ProxyRpc::Request(id, request) =
                rpc.rx().try_recv().expect("inline request")
            else {
                panic!("expected inline request");
            };
            dispatcher.handle_request(id, request);
            let PluginCatalogRpc::ServerRequest { f, .. } = catalog
                .recv_timeout(Duration::from_secs(2))
                .expect("inline LSP request")
            else {
                panic!("expected inline LSP request");
            };
            f(ahead_rpc::plugin::PluginId(0), Ok(serde_json::json!([])));
            let deadline = Instant::now() + Duration::from_secs(2);
            let (stream, _) = loop {
                match listener.accept() {
                    Ok(connected) => break connected,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("FIM did not connect promptly: {error}"),
                }
            };
            stream
                .set_nonblocking(false)
                .expect("blocking provider reads");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("bounded socket reads");
            let mut provider = std::io::BufReader::new(stream);
            let mut content_length = 0;
            loop {
                let mut line = String::new();
                assert!(
                    provider.read_line(&mut line).expect("FIM request header") > 0
                );
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    content_length = value.trim().parse().expect("body length");
                }
            }
            provider
                .read_exact(&mut vec![0; content_length])
                .expect("FIM request body");
            assert!(
                host.try_write().is_some(),
                "inference must not hold the host lock"
            );
            assert!(receiver.try_recv().is_err(), "provider has not answered");
            if explicit_cancel {
                dispatcher.handle_notification(
                    ProxyNotification::CancelInlineCompletions { request_id: id },
                );
            } else {
                dispatcher
                    .sync_editor_snapshot(active, "fn main".into())
                    .expect("typing invalidates prediction");
            }
            let result = receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("cancelled inline response")
                .expect("empty completion response");
            assert!(matches!(result, ProxyResponse::GetInlineCompletions {
            completions: lsp_types::InlineCompletionResponse::Array(items), ..
        } if items.is_empty()));
            assert_eq!(provider.read(&mut [0]).expect("provider socket closure"), 0);
            assert!(dispatcher.inline_prediction_jobs.lock().is_empty());
            drop(catalog.try_iter().collect::<Vec<_>>());

            let (sender, receiver) = crossbeam_channel::bounded(1);
            let active = workspace.join("main.rs");
            rpc.get_inline_completions(
                active.clone(),
                lsp_types::Position::new(0, 7),
                lsp_types::InlineCompletionTriggerKind::Automatic,
                None,
                None,
                move |result| sender.send(result).expect("late inline response"),
            );
            let ProxyRpc::Request(id, request) =
                rpc.rx().try_recv().expect("second inline request")
            else {
                panic!("expected second inline request");
            };
            dispatcher.handle_request(id, request);
            let PluginCatalogRpc::ServerRequest { f, .. } = catalog
                .recv_timeout(Duration::from_secs(2))
                .expect("second inline LSP request")
            else {
                panic!("expected second inline LSP request");
            };
            dispatcher.close_editor_buffer(&active);
            f(ahead_rpc::plugin::PluginId(0), Ok(serde_json::json!([])));
            let result = receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("closed-buffer response")
                .expect("empty completion response");
            assert!(matches!(result, ProxyResponse::GetInlineCompletions {
            completions: lsp_types::InlineCompletionResponse::Array(items), ..
        } if items.is_empty()));
            assert!(
                listener.accept().is_err_and(
                    |error| error.kind() == std::io::ErrorKind::WouldBlock
                ),
                "a late LSP reply must not launch FIM for a closed buffer"
            );
        }
    }

    fn git_metadata_fixture() -> (tempfile::TempDir, Repository, std::path::PathBuf)
    {
        let directory = tempfile::tempdir().expect("Git fixture");
        let mut init_options = git2::RepositoryInitOptions::new();
        // The machines running these tests do not agree on a default branch
        // name, so pin it rather than inheriting `init.defaultBranch`.
        init_options.initial_head("main");
        let repo = Repository::init_opts(directory.path(), &init_options)
            .expect("init fixture");
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
    fn git_history_returns_newest_commits_and_handles_unborn_repositories() {
        let (directory, repository, _) = git_metadata_fixture();
        let first = repository
            .head()
            .expect("HEAD")
            .peel_to_commit()
            .expect("initial commit");
        let signature = git2::Signature::now("Grace", "grace@example.invalid")
            .expect("signature");
        let tree = first.tree().expect("initial tree");
        let second_id = repository
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                "Second source",
                &tree,
                &[&first],
            )
            .expect("second commit");

        let latest = git_history(directory.path(), 1).expect("latest commit");
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].id, second_id.to_string());
        assert_eq!(latest[0].author, "Grace");
        assert_eq!(latest[0].subject, "Second source");

        let all = git_history(directory.path(), 100).expect("history");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, second_id.to_string());
        assert_eq!(all[1].subject, "Initial source");

        let unborn = tempfile::tempdir().expect("unborn repository directory");
        Repository::init(unborn.path()).expect("initialize unborn repository");
        assert!(
            git_history(unborn.path(), 100)
                .expect("empty history")
                .is_empty()
        );
    }

    #[test]
    fn git_commit_patch_returns_root_and_parent_diffs() {
        let (directory, repository, tracked_path) = git_metadata_fixture();
        let first = repository
            .head()
            .expect("HEAD")
            .peel_to_commit()
            .expect("initial commit");

        let root_patch = git_commit_patch(directory.path(), &first.id().to_string())
            .expect("root commit patch");
        assert!(
            root_patch.patch.contains("diff --git a/main.ts b/main.ts"),
            "{:?}",
            root_patch.patch
        );
        assert!(
            root_patch.patch.contains("+first"),
            "{:?}",
            root_patch.patch
        );
        assert!(!root_patch.truncated);

        fs::write(&tracked_path, "first\nupdated\nthird\n").expect("updated source");
        let mut index = repository.index().expect("index");
        index
            .add_path(std::path::Path::new("main.ts"))
            .expect("stage updated source");
        index.write().expect("write index");
        let tree_id = index.write_tree().expect("updated tree");
        let signature = git2::Signature::now("Grace", "grace@example.invalid")
            .expect("signature");
        let second_id = repository
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                "Update source",
                &repository.find_tree(tree_id).expect("updated tree"),
                &[&first],
            )
            .expect("second commit");

        let change_patch =
            git_commit_patch(directory.path(), &second_id.to_string())
                .expect("change commit patch");
        assert!(change_patch.patch.contains("-second"));
        assert!(change_patch.patch.contains("+updated"));
        assert!(!change_patch.truncated);
    }

    #[test]
    fn git_worktree_file_patch_reads_deleted_content_from_head() {
        let (directory, repository, _) = git_metadata_fixture();
        let nested_directory = directory.path().join("nested");
        fs::create_dir_all(&nested_directory).expect("nested workspace");
        let deleted_path = nested_directory.join("deleted.ts");
        fs::write(&deleted_path, "export const beforeDelete = 42;\n")
            .expect("source to delete");

        let mut index = repository.index().expect("index");
        index
            .add_path(Path::new("nested/deleted.ts"))
            .expect("stage source");
        index.write().expect("write index");
        let tree_id = index.write_tree().expect("write tree");
        let parent = repository
            .head()
            .expect("HEAD")
            .peel_to_commit()
            .expect("initial commit");
        let signature = git2::Signature::now("Grace", "grace@example.invalid")
            .expect("signature");
        repository
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                "Add nested source",
                &repository.find_tree(tree_id).expect("tree"),
                &[&parent],
            )
            .expect("commit source");
        fs::remove_file(&deleted_path).expect("delete source");

        let patch =
            git_worktree_file_patch(&nested_directory, Path::new("deleted.ts"))
                .expect("deleted file patch");
        assert_eq!(patch.id, "deleted.ts");
        assert!(
            patch
                .patch
                .contains("diff --git a/nested/deleted.ts b/nested/deleted.ts")
        );
        assert!(patch.patch.contains("-export const beforeDelete = 42;"));
        assert!(!patch.truncated);
    }

    #[test]
    fn git_worktree_file_patch_rejects_paths_outside_workspace() {
        let (directory, _, _) = git_metadata_fixture();
        let error =
            git_worktree_file_patch(directory.path(), Path::new("../secret.txt"))
                .expect_err("parent traversal must be rejected");
        assert!(error.to_string().contains("workspace-relative"));
    }

    #[test]
    fn git_worktree_patch_includes_all_changes_and_scopes_nested_workspaces() {
        let (directory, repository, _) = git_metadata_fixture();
        let nested_workspace = directory.path().join("nested");
        fs::create_dir_all(&nested_workspace).expect("nested workspace");
        let tracked_path = nested_workspace.join("tracked.ts");
        let deleted_path = nested_workspace.join("deleted.ts");
        let sibling_path = directory.path().join("sibling.ts");
        fs::write(&tracked_path, "baseline\n").expect("tracked source");
        fs::write(&deleted_path, "remove me\n").expect("deleted source");
        fs::write(&sibling_path, "sibling baseline\n").expect("sibling source");
        let mut index = repository.index().expect("index");
        index
            .add_path(Path::new("nested/tracked.ts"))
            .expect("stage nested source");
        index
            .add_path(Path::new("nested/deleted.ts"))
            .expect("stage deleted source");
        index
            .add_path(Path::new("sibling.ts"))
            .expect("stage sibling source");
        index.write().expect("write index");
        let tree_id = index.write_tree().expect("tree");
        let parent = repository
            .head()
            .expect("HEAD")
            .peel_to_commit()
            .expect("initial commit");
        let signature = git2::Signature::now("Grace", "grace@example.invalid")
            .expect("signature");
        repository
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                "Add nested files",
                &repository.find_tree(tree_id).expect("tree"),
                &[&parent],
            )
            .expect("commit fixture files");

        fs::write(&tracked_path, "staged version\n").expect("staged edit");
        let mut index = repository.index().expect("index");
        index
            .add_path(Path::new("nested/tracked.ts"))
            .expect("stage edit");
        index.write().expect("write staged edit");
        fs::write(&tracked_path, "unstaged version\n").expect("unstaged edit");
        fs::remove_file(&deleted_path).expect("delete nested file");
        let untracked_path = nested_workspace.join("new directory/untracked.ts");
        fs::create_dir_all(untracked_path.parent().expect("untracked parent"))
            .expect("create untracked directory");
        fs::write(&untracked_path, "new file\n").expect("untracked source");
        fs::write(&sibling_path, "sibling edit\n").expect("outside edit");

        let patch = git_worktree_patch(&nested_workspace)
            .expect("nested workspace aggregate diff");

        assert_eq!(patch.id, "working-tree");
        assert!(patch.patch.contains("nested/tracked.ts"));
        assert!(patch.patch.contains("+unstaged version"));
        assert!(patch.patch.contains("nested/deleted.ts"));
        assert!(patch.patch.contains("-remove me"));
        assert!(patch.patch.contains("nested/new directory/untracked.ts"));
        assert!(patch.patch.contains("+new file"));
        assert!(!patch.patch.contains("sibling.ts"));
        assert!(!patch.truncated);
    }

    #[test]
    fn workspace_git_status_preserves_paths_and_distinguishes_index_from_worktree() {
        let (directory, _, tracked_path) = git_metadata_fixture();
        fs::write(&tracked_path, "staged edit\n").expect("staged source");
        let stage = std::process::Command::new("git")
            .args(["add", "--", "main.ts"])
            .current_dir(directory.path())
            .status()
            .expect("stage source");
        assert!(stage.success());
        fs::write(&tracked_path, "unstaged edit\n").expect("unstaged source");
        let untracked_path = directory.path().join("nested directory/new file.ts");
        fs::create_dir_all(untracked_path.parent().expect("parent"))
            .expect("nested directory");
        fs::write(&untracked_path, "new source\n").expect("untracked source");

        let status = git_workspace_status(directory.path()).expect("Git status");
        assert_ne!(status.branch, "HEAD");
        assert!(status.branches.contains(&status.branch));
        assert_eq!(
            status.files,
            vec![
                GitStatusFile {
                    path: "main.ts".into(),
                    state: "MM".into(),
                    staged: true,
                },
                GitStatusFile {
                    path: "nested directory/new file.ts".into(),
                    state: "??".into(),
                    staged: false,
                },
            ]
        );
    }

    #[test]
    fn stage_all_leaves_conflicts_unmerged_and_commit_refuses_them() {
        let (directory, repository, tracked_path) = git_metadata_fixture();
        let original_branch = repository
            .head()
            .expect("HEAD")
            .shorthand()
            .expect("branch name")
            .to_string();
        {
            let mut config = repository.config().expect("config");
            config
                .set_str("user.name", "Human Developer")
                .expect("name");
            config
                .set_str("user.email", "human@example.test")
                .expect("email");
        }

        let base = repository
            .head()
            .expect("HEAD")
            .peel_to_commit()
            .expect("base commit");
        repository
            .branch("side", &base, false)
            .expect("side branch");
        fs::write(&tracked_path, "ours\n").expect("our edit");
        let output = std::process::Command::new("git")
            .args(["add", "--", "main.ts"])
            .current_dir(directory.path())
            .status()
            .expect("stage our edit");
        assert!(output.success());
        let output = std::process::Command::new("git")
            .args(["commit", "-m", "ours"])
            .current_dir(directory.path())
            .status()
            .expect("commit our edit");
        assert!(output.success());

        let output = std::process::Command::new("git")
            .args(["checkout", "side"])
            .current_dir(directory.path())
            .status()
            .expect("checkout side");
        assert!(output.success());
        fs::write(&tracked_path, "theirs\n").expect("side edit");
        let output = std::process::Command::new("git")
            .args(["add", "--", "main.ts"])
            .current_dir(directory.path())
            .status()
            .expect("stage side edit");
        assert!(output.success());
        let output = std::process::Command::new("git")
            .args(["commit", "-m", "theirs"])
            .current_dir(directory.path())
            .status()
            .expect("commit side edit");
        assert!(output.success());
        let output = std::process::Command::new("git")
            .args(["checkout", &original_branch])
            .current_dir(directory.path())
            .status()
            .expect("return to original branch");
        assert!(output.success());
        let output = std::process::Command::new("git")
            .args(["merge", "side"])
            .current_dir(directory.path())
            .output()
            .expect("create merge conflict");
        assert!(!output.status.success(), "merge must stop for resolution");

        let conflict_status =
            git_workspace_status(directory.path()).expect("conflict status");
        assert!(conflict_status.repository_has_conflicts);
        assert_eq!(
            conflict_status.files[0].state, "UU",
            "{:?}",
            conflict_status.files
        );
        assert!(!conflict_status.files[0].staged);

        fs::write(directory.path().join("other.ts"), "safe to stage\n")
            .expect("additional change");
        git_stage_all(directory.path()).expect("stage non-conflicted changes");
        let status = git_workspace_status(directory.path()).expect("updated status");
        let conflict = status
            .files
            .iter()
            .find(|file| file.path == "main.ts")
            .expect("conflicted path remains");
        assert_eq!(conflict.state, "UU");
        assert!(!conflict.staged);
        assert!(
            status
                .files
                .iter()
                .any(|file| file.path == "other.ts" && file.staged)
        );

        let nested_workspace = directory.path().join("project");
        fs::create_dir_all(&nested_workspace).expect("nested workspace");
        let nested_status =
            git_workspace_status(&nested_workspace).expect("nested Git status");
        assert!(nested_status.repository_has_conflicts);
        assert!(nested_status.files.is_empty());

        let error = git_commit(&nested_workspace, "Blocked", Vec::new(), &[])
            .expect_err("unresolved repository conflicts block commits");
        assert!(
            error
                .to_string()
                .contains("Resolve and stage all conflicted files")
        );
        assert!(
            git_workspace_status(directory.path())
                .expect("conflict remains after blocked commit")
                .repository_has_conflicts
        );
    }

    #[test]
    fn git_status_marks_all_porcelain_unmerged_states_as_conflicts() {
        for (index_status, worktree_status) in [
            (b'D', b'D'),
            (b'A', b'U'),
            (b'U', b'D'),
            (b'U', b'A'),
            (b'D', b'U'),
            (b'A', b'A'),
            (b'U', b'U'),
        ] {
            assert!(git_status_bytes_are_conflicted(
                index_status,
                worktree_status
            ));
        }
        assert!(!git_status_bytes_are_conflicted(b'M', b' '));
        assert!(!git_status_bytes_are_conflicted(b'?', b'?'));
    }

    #[test]
    fn git_checkout_switches_local_branches_only_with_a_clean_worktree() {
        let (directory, repository, tracked_path) = git_metadata_fixture();
        let original_branch = repository
            .head()
            .expect("HEAD")
            .shorthand()
            .expect("branch name")
            .to_string();
        let head = repository
            .head()
            .expect("HEAD")
            .peel_to_commit()
            .expect("initial commit");
        repository
            .branch("feature", &head, false)
            .expect("create feature branch");

        git_checkout(directory.path(), "feature").expect("switch branch");
        let checked_out_repository =
            Repository::open(directory.path()).expect("reopen repo");
        assert_eq!(
            checked_out_repository
                .head()
                .expect("feature HEAD")
                .shorthand(),
            Some("feature")
        );

        let nested_workspace = directory.path().join("project");
        fs::create_dir_all(&nested_workspace).expect("nested workspace");
        fs::write(&tracked_path, "uncommitted sibling change\n")
            .expect("dirty sibling source");
        let error = git_checkout(&nested_workspace, &original_branch)
            .expect_err("dirty worktrees must not be switched");
        assert!(error.to_string().contains("all repository changes"));
        let unchanged_repository =
            Repository::open(directory.path()).expect("reopen repo");
        assert_eq!(
            unchanged_repository
                .head()
                .expect("unchanged HEAD")
                .shorthand(),
            Some("feature")
        );
    }

    #[test]
    fn git_create_branch_selects_local_and_unborn_branches() {
        let (directory, _, _) = git_metadata_fixture();
        git_create_branch(directory.path(), "feature/topic")
            .expect("create and switch to local branch");
        let status = git_workspace_status(directory.path()).expect("branch status");
        assert_eq!(status.branch, "feature/topic");
        assert!(status.branches.contains(&"feature/topic".to_string()));

        let unborn = tempfile::tempdir().expect("unborn repository");
        Repository::init(unborn.path()).expect("init unborn repository");
        git_create_branch(unborn.path(), "main").expect("name unborn branch");
        let status = git_workspace_status(unborn.path()).expect("unborn status");
        assert_eq!(status.branch, "main");
        assert!(status.branches.is_empty());

        let diff = git_diff_new(unborn.path()).expect("unborn Git diff");
        assert_eq!(diff.head, "main");
    }

    #[test]
    fn git_push_publishes_selected_remote_and_preserves_existing_upstream() {
        let (directory, repository, _) = git_metadata_fixture();
        let remote_directory = tempfile::tempdir().expect("bare remote");
        let hook_directory = directory.path().join("empty-hooks");
        fs::create_dir(&hook_directory).expect("empty hook directory");
        let remote_repository = Repository::init_bare(remote_directory.path())
            .expect("initialize remote");
        remote_repository
            .config()
            .expect("bare remote config")
            .set_str(
                "core.hooksPath",
                hook_directory.to_str().expect("hook path is UTF-8"),
            )
            .expect("disable inherited hooks on the test remote");
        let branch_name = repository
            .head()
            .expect("current branch")
            .shorthand()
            .expect("branch name")
            .to_string();
        let remote_name = format!("ahead-push-test-{}", std::process::id());
        repository
            .remote(
                &remote_name,
                remote_directory
                    .path()
                    .to_str()
                    .expect("remote path is UTF-8"),
            )
            .expect("configure local test remote");
        let mut config = repository.config().expect("repository config");
        config
            .set_str(&format!("branch.{branch_name}.pushRemote"), &remote_name)
            .expect("select local test remote");
        config
            .set_str(
                &format!("branch.{branch_name}.merge"),
                &format!("refs/heads/{branch_name}"),
            )
            .expect("select local test branch");
        config
            .set_str(
                "core.hooksPath",
                hook_directory.to_str().expect("hook path is UTF-8"),
            )
            .expect("disable inherited Git hooks for this fixture");
        drop(config);

        git_push(directory.path(), Some(&remote_name), CoreRpcHandler::new())
            .expect("publish current branch");

        let second_remote_directory =
            tempfile::tempdir().expect("second bare remote");
        let second_remote_repository =
            Repository::init_bare(second_remote_directory.path())
                .expect("initialize second remote");
        second_remote_repository
            .config()
            .expect("second bare remote config")
            .set_str(
                "core.hooksPath",
                hook_directory.to_str().expect("hook path is UTF-8"),
            )
            .expect("disable inherited hooks on the second test remote");
        let second_remote_name =
            format!("ahead-push-test-secondary-{}", std::process::id());
        repository
            .remote(
                &second_remote_name,
                second_remote_directory
                    .path()
                    .to_str()
                    .expect("second remote path is UTF-8"),
            )
            .expect("configure second local test remote");
        git_push(
            directory.path(),
            Some(&second_remote_name),
            CoreRpcHandler::new(),
        )
        .expect("push to selected second remote");

        let repository =
            Repository::open(directory.path()).expect("reopen repository");
        let branch = repository
            .find_branch(&branch_name, git2::BranchType::Local)
            .expect("published local branch");
        let expected_upstream = format!("refs/remotes/{remote_name}/{branch_name}");
        let upstream = branch.upstream().expect("configured upstream");
        assert_eq!(upstream.get().name(), Some(expected_upstream.as_str()));
        let status = git_workspace_status(directory.path()).expect("updated status");
        assert!(status.branch_has_upstream);
        assert!(status.remotes.contains(&remote_name));
    }

    #[test]
    fn git_create_branch_rejects_invalid_duplicate_and_dirty_requests() {
        let (directory, repository, tracked_path) = git_metadata_fixture();
        assert!(git_create_branch(directory.path(), "bad..name").is_err());
        git_create_branch(directory.path(), "feature")
            .expect("create feature branch");
        let duplicate = git_create_branch(directory.path(), "feature")
            .expect_err("duplicate branch");
        assert!(duplicate.to_string().contains("already exists"));

        fs::write(&tracked_path, "uncommitted\n").expect("dirty source");
        let dirty = git_create_branch(directory.path(), "other")
            .expect_err("dirty worktree");
        assert!(dirty.to_string().contains("repository changes"));
        assert!(
            repository
                .find_branch("other", git2::BranchType::Local)
                .is_err()
        );
    }

    #[test]
    fn git_delete_branch_requires_clean_merged_non_current_branch() {
        let (directory, repository, tracked_path) = git_metadata_fixture();
        git_create_branch(directory.path(), "feature")
            .expect("create branch to delete");
        git_checkout(directory.path(), "main").expect("switch to main");

        git_delete_branch(directory.path(), "feature")
            .expect("delete merged branch");
        assert!(
            repository
                .find_branch("feature", git2::BranchType::Local)
                .is_err()
        );

        let current = git_delete_branch(directory.path(), "main")
            .expect_err("current branch cannot be deleted");
        assert!(current.to_string().contains("current branch"));

        git_create_branch(directory.path(), "unmerged").expect("create branch");
        let signature =
            git2::Signature::now("Ada", "ada@example.invalid").expect("signature");
        let head = repository
            .head()
            .expect("branch HEAD")
            .peel_to_commit()
            .expect("branch commit");
        let tree = head.tree().expect("tree");
        repository
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                "Unmerged work",
                &tree,
                &[&head],
            )
            .expect("commit on unmerged branch");
        git_checkout(directory.path(), "main").expect("return to main");
        let unmerged = git_delete_branch(directory.path(), "unmerged")
            .expect_err("unmerged branch must be protected");
        assert!(unmerged.to_string().contains("not merged"));

        fs::write(&tracked_path, "dirty\n").expect("dirty source");
        let dirty = git_delete_branch(directory.path(), "unmerged")
            .expect_err("dirty worktree must be protected");
        assert!(dirty.to_string().contains("repository changes"));
        assert!(
            repository
                .find_branch("unmerged", git2::BranchType::Local)
                .is_ok()
        );
    }

    #[test]
    fn workspace_git_status_scopes_nested_workspace_and_stage_paths() {
        let (directory, repository, tracked_path) = git_metadata_fixture();
        drop(repository);
        fs::write(&tracked_path, "sibling edit\n").expect("sibling edit");
        let nested_workspace = directory.path().join("project space");
        fs::create_dir_all(&nested_workspace).expect("nested workspace");
        fs::write(nested_workspace.join("new.ts"), "nested source\n")
            .expect("nested source");

        let status = git_workspace_status(&nested_workspace).expect("scoped status");
        assert!(status.repository_dirty);
        assert_eq!(
            status.files,
            vec![GitStatusFile {
                path: "new.ts".into(),
                state: "??".into(),
                staged: false,
            }]
        );

        git_stage_paths(&nested_workspace, &["new.ts".into()], true)
            .expect("stage nested path");
        assert!(
            Repository::open(directory.path())
                .expect("reopen repository")
                .index()
                .expect("index")
                .get_path(std::path::Path::new("project space/new.ts"), 0)
                .is_some()
        );
        assert_eq!(
            git_workspace_status(&nested_workspace)
                .expect("refreshed scoped status")
                .files,
            vec![GitStatusFile {
                path: "new.ts".into(),
                state: "A".into(),
                staged: true,
            }]
        );
    }

    #[test]
    fn workspace_git_status_returns_errors_instead_of_an_empty_snapshot() {
        let workspace = tempfile::tempdir().expect("workspace");
        let error =
            git_workspace_status(workspace.path()).expect_err("not a Git repo");
        assert!(!error.to_string().is_empty());
    }

    #[test]
    fn stage_all_from_nested_workspace_does_not_stage_sibling_changes() {
        let (directory, repository, tracked_path) = git_metadata_fixture();
        drop(repository);
        fs::write(&tracked_path, "sibling edit\n").expect("sibling edit");
        let nested_workspace = directory.path().join("project");
        fs::create_dir_all(&nested_workspace).expect("nested workspace");
        fs::write(nested_workspace.join("new.ts"), "nested source\n")
            .expect("nested source");

        git_stage_all(&nested_workspace).expect("stage nested workspace");
        let status = git_workspace_status(directory.path()).expect("root status");
        assert_eq!(
            status.files,
            vec![
                GitStatusFile {
                    path: "main.ts".into(),
                    state: "M".into(),
                    staged: false,
                },
                GitStatusFile {
                    path: "project/new.ts".into(),
                    state: "A".into(),
                    staged: true,
                },
            ]
        );
    }

    #[test]
    fn stage_and_unstage_paths_handles_literal_names_and_keeps_worktree_content() {
        let (directory, _, _) = git_metadata_fixture();
        let relative_path = "nested directory/:(glob)*.ts";
        let file_path = directory.path().join(relative_path);
        fs::create_dir_all(file_path.parent().expect("parent"))
            .expect("nested directory");
        fs::write(&file_path, "new source\n").expect("source");

        git_stage_paths(directory.path(), &[relative_path.to_string()], true)
            .expect("stage selected path");
        assert!(
            Repository::open(directory.path())
                .expect("reopen repository")
                .index()
                .expect("index")
                .get_path(std::path::Path::new(relative_path), 0)
                .is_some()
        );

        git_stage_paths(directory.path(), &[relative_path.to_string()], false)
            .expect("unstage selected path");
        assert!(
            Repository::open(directory.path())
                .expect("reopen repository")
                .index()
                .expect("index")
                .get_path(std::path::Path::new(relative_path), 0)
                .is_none()
        );
        assert_eq!(
            fs::read_to_string(file_path).expect("worktree file"),
            "new source\n"
        );
        assert!(
            git_stage_paths(directory.path(), &["../outside".into()], true).is_err()
        );
    }

    #[test]
    fn unstage_paths_on_an_unborn_branch_only_removes_the_index_entry() {
        let directory = tempfile::tempdir().expect("workspace");
        Repository::init(directory.path()).expect("Git repository");
        let relative_path = "first file.rs";
        let file_path = directory.path().join(relative_path);
        fs::write(&file_path, "first commit content\n").expect("source");

        git_stage_paths(directory.path(), &[relative_path.to_string()], true)
            .expect("stage initial file");
        git_stage_paths(directory.path(), &[relative_path.to_string()], false)
            .expect("unstage initial file");
        assert!(
            Repository::open(directory.path())
                .expect("reopen repository")
                .index()
                .expect("index")
                .get_path(std::path::Path::new(relative_path), 0)
                .is_none()
        );
        assert_eq!(
            fs::read_to_string(file_path).expect("worktree file"),
            "first commit content\n"
        );
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
        let source_directory =
            repo.workdir().expect("repository workdir").join("src");
        fs::create_dir(&source_directory).expect("source directory");
        fs::write(source_directory.join("main.ts"), "first\nsecond\nthird\n")
            .expect("nested source");
        let mut index = repo.index().expect("index");
        index
            .add_path(std::path::Path::new("src/main.ts"))
            .expect("add nested source");
        index.write().expect("write index");
        let tree_id = index.write_tree().expect("write tree");
        let tree = repo.find_tree(tree_id).expect("nested tree");
        let parent = repo.head().expect("HEAD").peel_to_commit().expect("commit");
        let signature = git2::Signature::now("Grace", "grace@example.invalid")
            .expect("signature");
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            "Add nested source",
            &tree,
            &[&parent],
        )
        .expect("commit nested source");
        let linked = tempfile::tempdir().expect("worktree parent");
        let root = linked.path().join("checkout");
        let worktree = repo
            .worktree("linked", &root, None)
            .expect("linked worktree");
        let linked_repo =
            Repository::open_from_worktree(&worktree).expect("linked repository");
        let subdirectory = root.join("src");
        let path = subdirectory.join("main.ts");
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
            let state =
                super::git_file_state(workspace, &path, "first\nunsaved\nthird\n")
                    .expect("linked metadata");
            assert_eq!(state.hunks[0].start, 2);
            assert_eq!(
                state.blame[0]
                    .commit
                    .as_ref()
                    .expect("committed nested line")
                    .author,
                "Grace"
            );
            assert!(
                state
                    .blame
                    .iter()
                    .any(|hunk| hunk.start == 2 && hunk.commit.is_none())
            );
        }
    }

    #[test]
    fn git_metadata_refreshes_after_external_amend_and_checkout() {
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
        let original_id = commit.id();
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

        let original = repo.find_commit(original_id).expect("original commit");
        repo.branch("baseline", &original, false)
            .expect("baseline branch");
        repo.set_head("refs/heads/baseline").expect("switch branch");
        repo.checkout_head(None).expect("checkout baseline");
        notifier.handle_workspace_fs_event(
            notify::Event::new(notify::EventKind::Modify(
                notify::event::ModifyKind::Data(notify::event::DataChange::Content),
            ))
            .add_path(repo.path().join("HEAD")),
        );
        let after_checkout = loop {
            let event = core
                .rx()
                .recv_timeout(Duration::from_secs(2))
                .expect("checkout invalidation");
            if let CoreRpc::Notification(event) = event
                && let CoreNotification::DiffInfo { diff } = *event
            {
                break diff;
            }
        };
        assert_ne!(before.head, "baseline");
        assert_eq!(after_checkout.head, "baseline");
        assert!(after_checkout.diffs.is_empty());
        let state =
            super::git_file_state(directory.path(), &path, "first\nsecond\nthird\n")
                .expect("checked-out metadata");
        assert_eq!(
            state.blame[0]
                .commit
                .as_ref()
                .expect("checked-out blame")
                .author,
            "Ada"
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

        dispatcher.handle_notification(ProxyNotification::RestartLanguageServers {
            extension_id: None,
        });

        let documents = notifications
            .try_iter()
            .find_map(|notification| match notification {
                crate::plugin::PluginCatalogRpc::Handler(
                    crate::plugin::PluginCatalogNotification::RestartLanguageServers {
                        documents,
                        extension_id,
                    },
                ) => Some((documents, extension_id)),
                _ => None,
            })
            .expect("restart notification");
        let (documents, extension_id) = documents;
        assert_eq!(extension_id, None);
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
    fn lsp_requests_for_closed_buffers_return_errors_instead_of_panicking() {
        let rpc = ProxyRpcHandler::new();
        let mut dispatcher = Dispatcher::new(CoreRpcHandler::new(), rpc.clone());
        let path = std::path::PathBuf::from("/workspace/src/main.rs");

        for (request, expected_message) in [
            (
                ProxyRequest::GetInlayHints { path: path.clone() },
                "inlay hints requested for unopened buffer",
            ),
            (
                ProxyRequest::GetSemanticTokens { path: path.clone() },
                "semantic tokens requested for unopened buffer",
            ),
        ] {
            let (sender, receiver) = crossbeam_channel::bounded(1);
            rpc.request_async(request, move |result| {
                sender.send(result).expect("RPC response receiver");
            });
            let ProxyRpc::Request(id, request) =
                rpc.rx().try_recv().expect("LSP request")
            else {
                panic!("expected LSP request");
            };

            dispatcher.handle_request(id, request);

            let response = receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("RPC response");
            let Err(error) = response else {
                panic!("closed-buffer LSP request unexpectedly succeeded");
            };
            assert!(error.message.contains(expected_message));
        }
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
    fn editor_snapshot_replaces_unicode_buffer_completely() {
        let directory = tempfile::tempdir().expect("disposable project");
        let path = directory.path().join("main.rs");
        fs::write(&path, "// 🦀 old snapshot\n").expect("source");
        let mut dispatcher =
            Dispatcher::new(CoreRpcHandler::new(), ProxyRpcHandler::new());
        dispatcher
            .sync_editor_snapshot(path.clone(), "// 🦀 old snapshot\n".into())
            .expect("initial snapshot");

        let updated = "// 🦀 updated snapshot with a longer body\nfn main() {}\n";
        dispatcher
            .sync_editor_snapshot(path.clone(), updated.into())
            .expect("updated snapshot");

        assert_eq!(dispatcher.buffers[&path].get_document(), updated);
    }

    #[test]
    fn incremental_editor_edit_updates_the_buffer_and_lsp_notification() {
        let directory = tempfile::tempdir().expect("disposable project");
        let path = directory.path().join("main.rs");
        let previous = "fn main() { let crab = \"🦀\"; let value = 1; }\n";
        let current = "fn main() { let crab = \"🦀\"; let value = 2; }\n";
        fs::write(&path, previous).expect("source");
        let mut dispatcher =
            Dispatcher::new(CoreRpcHandler::new(), ProxyRpcHandler::new());
        let notifications = dispatcher.catalog_rpc.test_receiver();
        dispatcher
            .sync_editor_snapshot(path.clone(), previous.into())
            .expect("open editor buffer");
        drop(notifications.try_iter().collect::<Vec<_>>());

        let offset = previous.find('1').expect("changed character");
        let delta = AheadDelta::new(
            previous.len(),
            vec![
                DeltaOp::Retain(offset),
                DeltaOp::Delete(1),
                DeltaOp::Insert("2".into()),
                DeltaOp::Retain(previous.len() - offset - 1),
            ],
        );
        dispatcher.handle_notification(ProxyNotification::EditorEdit {
            path: path.clone(),
            delta,
        });

        assert_eq!(dispatcher.buffers[&path].get_document(), current);
        assert_eq!(dispatcher.buffers[&path].rev, 2);
        let change = notifications
            .try_iter()
            .find_map(|notification| match notification {
                PluginCatalogRpc::DidChangeTextDocument {
                    document,
                    delta,
                    text,
                    new_text,
                    ..
                } => Some((document, delta, text, new_text)),
                _ => None,
            })
            .expect("incremental LSP change");
        assert_eq!(change.0.version, 2);
        assert_eq!(change.1.apply(&Rope::from(previous)).to_string(), current);
        assert_eq!(change.2.to_string(), previous);
        assert_eq!(change.3.to_string(), current);
        assert_eq!(fs::read_to_string(path).expect("saved source"), previous);
    }

    #[test]
    fn incremental_editor_edit_rejects_a_stale_base_without_mutating_text() {
        let directory = tempfile::tempdir().expect("disposable project");
        let path = directory.path().join("main.rs");
        let previous = "fn main() {}\n";
        fs::write(&path, previous).expect("source");
        let mut dispatcher =
            Dispatcher::new(CoreRpcHandler::new(), ProxyRpcHandler::new());
        dispatcher
            .sync_editor_snapshot(path.clone(), previous.into())
            .expect("open editor buffer");

        dispatcher.handle_notification(ProxyNotification::EditorEdit {
            path: path.clone(),
            delta: AheadDelta::new(1, vec![DeltaOp::Delete(1)]),
        });

        assert_eq!(dispatcher.buffers[&path].get_document(), previous);
        assert_eq!(dispatcher.buffers[&path].rev, 1);
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

        let active_rope =
            Rope::from("mod sibling;\nmod large;\nuse crate::request::Request;");
        let context = relevant_open_buffer_contexts(
            &buffers,
            workspace,
            &active_path,
            &active_rope,
            &[],
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
    fn prediction_context_prefers_imports_and_rejects_substring_collisions() {
        let temp = tempfile::tempdir().expect("workspace");
        let workspace = temp.path();
        fs::create_dir_all(workspace.join("src")).expect("create source directory");
        fs::create_dir_all(workspace.join("models"))
            .expect("create models directory");
        fs::create_dir_all(workspace.join("docs")).expect("create docs directory");
        let active_path = workspace.join("src/main.ts");
        let imported_path = workspace.join("models/request.ts");
        let substring_path = workspace.join("models/quest.ts");
        let prose_path = workspace.join("models/log.ts");
        let generic_paths = [
            workspace.join("docs/alpha.ts"),
            workspace.join("docs/beta.ts"),
            workspace.join("docs/gamma.ts"),
            workspace.join("docs/delta.ts"),
        ];
        let paths = std::iter::once(active_path.clone())
            .chain([imported_path, substring_path, prose_path])
            .chain(generic_paths)
            .collect::<Vec<_>>();
        for path in &paths {
            fs::write(path, "on disk").expect("create source file");
        }

        let buffers = paths
            .into_iter()
            .map(|path| (path.clone(), Buffer::new(BufferId::next(), path)))
            .collect::<HashMap<_, _>>();
        let active_rope = Rope::from(
            "import { Request } from '../models/request';\n\
             const names = 'alpha beta gamma delta catalog';",
        );
        let context = relevant_open_buffer_contexts(
            &buffers,
            workspace,
            &active_path,
            &active_rope,
            &[],
        );

        assert_eq!(context.len(), MAX_FIM_OPEN_BUFFERS);
        assert!(
            context
                .iter()
                .any(|buffer| buffer.path == "models/request.ts")
        );
        assert!(
            !context
                .iter()
                .any(|buffer| buffer.path == "models/quest.ts")
        );
        assert!(!context.iter().any(|buffer| buffer.path == "models/log.ts"));
    }

    #[test]
    fn prediction_context_uses_open_lsp_definitions_and_excerpt_location() {
        let temp = tempfile::tempdir().expect("workspace");
        let workspace = temp.path();
        fs::create_dir_all(workspace.join("src")).expect("create source directory");
        fs::create_dir_all(workspace.join("types")).expect("create types directory");
        let active_path = workspace.join("src/main.rs");
        let definition_path = workspace.join("types/shared.rs");
        fs::write(&active_path, "use_shared_type();").expect("create active file");
        fs::write(&definition_path, "on disk definition")
            .expect("create target file");

        let definition_text = (0..120)
            .map(|line| {
                if line == 70 {
                    "pub struct SharedType;".to_string()
                } else {
                    format!("// context line {line} {}", "x".repeat(72))
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut active = Buffer::new(BufferId::next(), active_path.clone());
        let active_rope = Rope::from("use_shared_type();");
        active.rope = active_rope.clone();
        let mut definition = Buffer::new(BufferId::next(), definition_path.clone());
        definition.rope = Rope::from(definition_text);
        let buffers = HashMap::from([
            (active_path.clone(), active),
            (definition_path.clone(), definition),
        ]);

        let context = relevant_open_buffer_contexts(
            &buffers,
            workspace,
            &active_path,
            &active_rope,
            &[(definition_path, 70)],
        );

        assert_eq!(context.len(), 1);
        assert_eq!(context[0].path, "types/shared.rs");
        assert!(
            context[0]
                .relevant_excerpt
                .contains("definition near line 71")
        );
        assert!(
            context[0]
                .relevant_excerpt
                .contains("pub struct SharedType;")
        );
    }

    #[test]
    fn prediction_identifier_positions_rank_nearby_symbols_and_use_utf16_columns() {
        let source = "fn 𐐀x(value: Thing) {\n    call(value, other);\n}\n";
        let positions = prediction_identifier_positions(
            &Rope::from(source),
            lsp_types::Position::new(1, 11),
        );

        assert_eq!(
            positions.first().map(|(name, _)| name.as_str()),
            Some("value")
        );
        assert_eq!(
            positions.iter().filter(|(name, _)| name == "value").count(),
            1
        );
        assert_eq!(
            positions
                .iter()
                .find(|(name, _)| name == "𐐀x")
                .map(|(_, position)| *position),
            Some(lsp_types::Position::new(0, 3))
        );
    }

    #[test]
    fn prediction_context_prefers_current_semantic_tokens_over_comments_and_strings()
    {
        use ahead_rpc::style::{LineStyle, SemanticStyles, Style};

        let source =
            "let value = target;\n// hiddenComment\nlet text = \"hiddenString\";\n";
        let semantic_style = |name: &str, token_type: &str| {
            let start = source.find(name).expect("semantic token text");
            LineStyle {
                start,
                end: start + name.len(),
                style: Style {
                    fg_color: Some(token_type.into()),
                },
            }
        };
        let styles = SemanticStyles {
            rev: 3,
            path: PathBuf::from("main.rs"),
            len: source.len(),
            styles: vec![
                semantic_style("value", "variable"),
                semantic_style("target", "parameter"),
                semantic_style("hiddenComment", "comment"),
                semantic_style("hiddenString", "string"),
            ],
        };

        let positions = prediction_identifier_positions_with_semantic_styles(
            &Rope::from(source),
            lsp_types::Position::new(0, 15),
            Some(&styles),
            None,
        );

        assert_eq!(
            positions
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec!["target", "value"]
        );
    }

    #[test]
    fn prediction_context_uses_extension_identifier_captures_before_lexical_fallback()
     {
        use ahead_rpc::style::SyntaxCapture;

        let source =
            "let actualName = 1;\n// hiddenComment\nlet text = \"hiddenString\";\n";
        let capture = |name: &str, scope: &str| {
            let start = source.find(name).expect("capture text");
            SyntaxCapture {
                start,
                end: start + name.len(),
                scope: scope.into(),
            }
        };
        let captures = vec![
            capture("actualName", "variable"),
            capture("hiddenComment", "comment"),
            capture("hiddenString", "string"),
        ];

        let positions = prediction_identifier_positions_with_semantic_styles(
            &Rope::from(source),
            lsp_types::Position::new(1, 3),
            None,
            Some(&captures),
        );

        assert_eq!(
            positions,
            vec![("actualName".into(), lsp_types::Position::new(0, 4))]
        );
    }

    #[test]
    fn prediction_semantic_token_cache_rejects_stale_buffer_revisions() {
        use ahead_rpc::style::{SemanticStyles, SyntaxCapture};

        let path = PathBuf::from("main.rs");
        let buffer_id = BufferId::next();
        let mut cache = PredictionSemanticTokenCache::default();
        let mut buffer = Buffer::new(buffer_id.clone(), path.clone());
        buffer.rope = Rope::from("let value = \"🦀\";");
        buffer.rev = 4;
        cache.track_buffer(&path, &buffer);

        let styles = SemanticStyles {
            rev: 4,
            path: path.clone(),
            len: buffer.rope.len(),
            styles: Vec::new(),
        };
        cache.store(buffer_id.clone(), &styles);
        assert!(cache.styles_for(&path, &buffer).is_some());
        let captures = vec![SyntaxCapture {
            start: 4,
            end: 9,
            scope: "variable".into(),
        }];
        cache.store_syntax_captures(
            &buffer_id,
            &path,
            4,
            buffer.rope.len(),
            captures.clone(),
        );
        let cached_captures = cache
            .syntax_captures_for(&path, &buffer)
            .expect("current syntax captures");
        assert_eq!(cached_captures.len(), 1);
        assert_eq!(cached_captures[0].scope, "variable");

        buffer.rev += 1;
        cache.track_buffer(&path, &buffer);
        assert!(cache.styles_for(&path, &buffer).is_none());
        assert!(cache.syntax_captures_for(&path, &buffer).is_none());
        cache.store(buffer_id, &styles);
        cache.store_syntax_captures(
            &buffer.id,
            &path,
            4,
            buffer.rope.len(),
            vec![SyntaxCapture {
                start: 4,
                end: 9,
                scope: "variable".into(),
            }],
        );
        assert!(cache.styles_for(&path, &buffer).is_none());
        assert!(cache.syntax_captures_for(&path, &buffer).is_none());
    }

    #[test]
    fn prediction_definition_cache_invalidates_when_open_target_changes() {
        let temp = tempfile::tempdir().expect("workspace");
        let workspace = temp.path();
        fs::create_dir_all(workspace.join("src")).expect("create source directory");
        fs::create_dir_all(workspace.join("types")).expect("create types directory");
        let active_path = workspace.join("src/main.rs");
        let definition_path = workspace.join("types/shared.rs");
        let private_path = workspace.join(".env");
        fs::write(&active_path, "use_shared_type();").expect("create active file");
        fs::write(&definition_path, "struct SharedType;")
            .expect("create definition file");
        fs::write(&private_path, "SECRET=value").expect("create private file");

        let buffers = HashMap::from([
            (
                active_path.clone(),
                Buffer::new(BufferId::next(), active_path.clone()),
            ),
            (
                definition_path.clone(),
                Buffer::new(BufferId::next(), definition_path.clone()),
            ),
            (
                private_path.clone(),
                Buffer::new(BufferId::next(), private_path.clone()),
            ),
        ]);
        let active = buffers.get(&active_path).expect("active buffer");
        let resolved_definition_path = workspace
            .canonicalize()
            .expect("canonical workspace")
            .join("types/shared.rs");
        let key = PredictionDefinitionCacheKey {
            path: active_path,
            buffer_id: active.id,
            revision: active.rev,
            identifier: "SharedType".into(),
            line: 0,
            character: 4,
        };
        let cache = parking_lot::Mutex::new(HashMap::<
            PredictionDefinitionCacheKey,
            CachedPredictionDefinitions,
        >::new());

        cache_prediction_definitions(
            &cache,
            key.clone(),
            &[(definition_path.clone(), 0), (private_path, 0)],
            Some(workspace),
            &buffers,
        );
        assert_eq!(
            cached_prediction_definitions(&cache, &key, Some(workspace), &buffers),
            Some(vec![(resolved_definition_path, 0)])
        );

        let mut changed_buffers = buffers.clone();
        changed_buffers
            .get_mut(&definition_path)
            .expect("definition buffer")
            .rev += 1;
        assert!(
            cached_prediction_definitions(
                &cache,
                &key,
                Some(workspace),
                &changed_buffers
            )
            .is_none()
        );
    }

    #[test]
    fn prediction_definition_locations_handle_all_lsp_response_shapes() {
        let temp = tempfile::tempdir().expect("workspace");
        let scalar_path = temp.path().join("scalar.rs");
        let array_path = temp.path().join("array.rs");
        let link_path = temp.path().join("link.rs");
        let uri = |path: &PathBuf| Url::from_file_path(path).expect("file URL");
        let position = lsp_types::Position::new(9, 2);
        let range = lsp_types::Range::new(position, position);

        let scalar = GotoDefinitionResponse::Scalar(lsp_types::Location {
            uri: uri(&scalar_path),
            range,
        });
        let array = GotoDefinitionResponse::Array(vec![lsp_types::Location {
            uri: uri(&array_path),
            range,
        }]);
        let link = GotoDefinitionResponse::Link(vec![lsp_types::LocationLink {
            origin_selection_range: None,
            target_uri: uri(&link_path),
            target_range: range,
            target_selection_range: range,
        }]);

        assert_eq!(
            prediction_definition_locations(scalar),
            vec![(scalar_path, 9)]
        );
        assert_eq!(
            prediction_definition_locations(array),
            vec![(array_path, 9)]
        );
        assert_eq!(prediction_definition_locations(link), vec![(link_path, 9)]);
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
    fn shutdown_cancels_workspace_file_index_warmup() {
        let workspace = tempfile::tempdir().expect("disposable workspace");
        fs::write(workspace.path().join("main.rs"), "fn main() {}")
            .expect("write source file");
        let mut dispatcher =
            Dispatcher::new(CoreRpcHandler::new(), ProxyRpcHandler::new());
        ProxyHandler::handle_notification(
            &mut dispatcher,
            ProxyNotification::Initialize {
                workspace: Some(workspace.path().to_path_buf()),
                window_id: 1,
                tab_id: 1,
            },
        );
        let cancelled = dispatcher
            .file_index_warmup
            .as_ref()
            .expect("workspace initialization starts index warmup")
            .clone();

        ProxyHandler::handle_notification(
            &mut dispatcher,
            ProxyNotification::Shutdown {},
        );

        assert!(cancelled.load(Ordering::SeqCst));
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
        let changed_path = path.clone();
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
                if matches!(&*notification, CoreNotification::WorkspaceFileChange { generation: 0, paths }
                    if paths == &vec![changed_path.clone()])
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
            .add_path(config.clone()),
        );
        assert_eq!(index.generation(), 0);
        assert!(
            matches!(
                core_rpc.rx().try_recv(),
                Ok(CoreRpc::Notification(notification))
                    if matches!(*notification, CoreNotification::AheadNotification {
                        notification: ahead_rpc::ahead::AheadNotification::PredictionContextChanged { session_id: None }
                    })
            ),
            "provider invalidation must not wait for the filesystem debounce"
        );
        assert!(matches!(
            core_rpc.rx().recv_timeout(Duration::from_secs(2)),
            Ok(CoreRpc::Notification(notification))
                if matches!(*notification, CoreNotification::WorkspaceFileChange { generation: 0, .. })
        ));
        assert!(matches!(
            catalog_receiver.recv_timeout(Duration::from_secs(2)),
            Ok(crate::plugin::PluginCatalogRpc::Handler(
                crate::plugin::PluginCatalogNotification::RefreshWorkspaceConfigurations
            ))
        ));
    }

    #[test]
    fn instruction_changes_are_immediate_and_scoped_to_the_repository() {
        use notify::{
            Event, EventKind,
            event::{CreateKind, DataChange, ModifyKind, RemoveKind, RenameMode},
        };
        let directory = tempfile::tempdir().expect("instruction fixture");
        let repository = directory.path().join("repo");
        let workspace = repository.join("packages/app");
        fs::create_dir_all(workspace.join("src")).expect("nested workspace");
        fs::create_dir(repository.join(".git")).expect("repository boundary");
        fs::write(workspace.join(".gitignore"), "AGENTS.md\n")
            .expect("search exclusions");
        fs::write(
            workspace.join("src/AGENTS.md"),
            "ignored source instructions",
        )
        .expect("ignored instructions");
        let index = Arc::new(WorkspaceFileIndex::new(workspace.clone()));
        assert!(!index.snapshot().contains(&workspace.join("src/AGENTS.md")));
        let core = CoreRpcHandler::new();
        let catalog = PluginCatalogRpcHandler::new(CoreRpcHandler::new());
        let catalog_events = catalog.test_receiver();
        let notifier = FileWatchNotifier::new(
            Some(workspace.clone()),
            core.clone(),
            ProxyRpcHandler::new(),
            catalog,
            Some(index),
        );
        for path in [
            directory.path().join("AGENTS.md"),
            repository.join("sibling/AGENTS.md"),
            repository.join("CLAUDE.md"),
            repository.join("AGENT.md"),
        ] {
            notifier.handle_fs_events(vec![(
                INSTRUCTION_EVENT_TOKEN,
                Event::new(EventKind::Modify(ModifyKind::Any)).add_path(path),
            )]);
            assert!(
                core.rx().try_recv().is_err(),
                "unrelated instructions must not notify"
            );
        }
        for (path, token) in [
            (repository.join("AGENTS.md"), INSTRUCTION_EVENT_TOKEN),
            (
                repository.join("packages/AGENTS.md"),
                INSTRUCTION_EVENT_TOKEN,
            ),
            (workspace.join("AGENTS.md"), WORKSPACE_EVENT_TOKEN),
            (workspace.join("src/AGENTS.md"), WORKSPACE_EVENT_TOKEN),
        ] {
            for kind in [
                EventKind::Create(CreateKind::File),
                EventKind::Modify(ModifyKind::Data(DataChange::Content)),
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                EventKind::Remove(RemoveKind::File),
            ] {
                let mut event = Event::new(kind).add_path(path.clone());
                if matches!(
                    kind,
                    EventKind::Modify(ModifyKind::Name(RenameMode::Both))
                ) {
                    event = event.add_path(path.with_file_name("saved.tmp"));
                }
                notifier.handle_fs_events(vec![(token, event)]);
                assert!(
                    core.rx().try_iter().any(|message| matches!(message, CoreRpc::Notification(notification)
                    if matches!(*notification, CoreNotification::AheadNotification {
                        notification: ahead_rpc::ahead::AheadNotification::PredictionContextChanged { session_id: None }
                    }))),
                    "{path:?} {kind:?} must invalidate before debounce"
                );
            }
        }
        assert!(
            catalog_events.try_recv().is_err(),
            "instruction changes must not restart language servers"
        );
    }

    #[test]
    fn ancestor_instruction_watch_delivers_native_create_replace_and_remove() {
        for operation in ["create", "replace", "remove"] {
            let directory = tempfile::tempdir().expect("native instruction fixture");
            let repository = directory.path().join("repo");
            let workspace = repository.join("packages/app");
            fs::create_dir_all(&workspace).expect("nested workspace");
            if operation == "remove" {
                fs::write(repository.join(".git"), "gitdir: elsewhere\n")
                    .expect("worktree marker");
            } else {
                fs::create_dir(repository.join(".git")).expect("repository marker");
            }
            let instruction = if operation == "replace" {
                repository.join("packages/AGENTS.md")
            } else {
                repository.join("AGENTS.md")
            };
            if operation != "create" {
                fs::write(&instruction, "before").expect("initial instructions");
            }
            let core = CoreRpcHandler::new();
            let notifier = FileWatchNotifier::new(
                Some(workspace),
                core.clone(),
                ProxyRpcHandler::new(),
                PluginCatalogRpcHandler::new(CoreRpcHandler::new()),
                None,
            );
            let mut watcher = FileWatcher::new();
            notifier.watch_instruction_ancestors(&mut watcher);
            watcher.notify(notifier);
            match operation {
                "create" => fs::write(&instruction, "new instructions")
                    .expect("create instructions"),
                "replace" => {
                    let replacement = instruction.with_file_name("instructions.tmp");
                    fs::write(&replacement, "replacement instructions")
                        .expect("replacement contents");
                    fs::rename(replacement, &instruction)
                        .expect("atomic instruction replacement");
                }
                "remove" => {
                    fs::remove_file(&instruction).expect("remove instructions")
                }
                _ => unreachable!(),
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let message = core
                    .rx()
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .unwrap_or_else(|error| {
                        panic!("{operation} instruction event missing: {error}")
                    });
                if matches!(message, CoreRpc::Notification(notification)
                if matches!(*notification, CoreNotification::AheadNotification {
                    notification: ahead_rpc::ahead::AheadNotification::PredictionContextChanged { session_id: None }
                })) {
                    break;
                }
            }
        }
    }

    #[test]
    fn instruction_watch_boundary_tracks_git_root_changes() {
        let directory = tempfile::tempdir().expect("instruction root fixture");
        let repository = directory.path().join("repo");
        let workspace = repository.join("packages/app");
        let package = repository.join("packages");
        fs::create_dir_all(&workspace).expect("nested workspace");
        fs::create_dir(repository.join(".git")).expect("outer repository marker");
        let core = CoreRpcHandler::new();
        let mut notifier = FileWatchNotifier::new(
            Some(workspace),
            core.clone(),
            ProxyRpcHandler::new(),
            PluginCatalogRpcHandler::new(CoreRpcHandler::new()),
            None,
        );
        let mut watcher = FileWatcher::new();
        notifier.user_settings_watcher = Some(watcher.handle());
        notifier.watch_instruction_ancestors(&mut watcher);
        let receiver = core.rx();
        watcher.notify(notifier);

        let nested_marker = package.join(".git");
        fs::create_dir(&nested_marker).expect("nested repository marker");
        wait_for_prediction_context(&receiver);
        assert_eq!(
            ahead_agent::project_instruction_root(&package.join("app")),
            package
        );
        fs::write(package.join("AGENTS.md"), "nested instructions")
            .expect("write newly active ancestor instructions");
        wait_for_prediction_context(&receiver);

        fs::remove_dir(&nested_marker).expect("remove nested repository marker");
        wait_for_prediction_context(&receiver);
        fs::write(repository.join("AGENTS.md"), "outer instructions")
            .expect("write restored outer ancestor instructions");
        wait_for_prediction_context(&receiver);
    }

    #[test]
    #[cfg(unix)]
    fn workspace_settings_watch_preserves_refresh_through_a_workspace_alias() {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::tempdir().expect("fixture");
        let real = temporary.path().join("workspace");
        fs::create_dir_all(real.join(".ahead")).expect("settings directory");
        let alias = temporary.path().join("alias");
        symlink(&real, &alias).expect("workspace alias");
        let config = real.join(".ahead/config.toml");
        fs::write(&config, "[languages]\n").expect("initial settings");
        let canonical_root = real.canonicalize().expect("canonical root");
        let aliased_config = alias.join(".ahead/config.toml");
        assert!(is_provider_settings_path(&canonical_root, &aliased_config));
        fs::remove_file(&config).expect("remove settings");
        assert!(is_provider_settings_path(&canonical_root, &aliased_config));
        assert!(!is_provider_settings_path(
            &canonical_root,
            &temporary.path().join(".ahead/config.toml")
        ));
        fs::write(&config, "[languages]\n").expect("restore settings");
        let catalog = PluginCatalogRpcHandler::new(CoreRpcHandler::new());
        let receiver = catalog.test_receiver();
        let notifier = FileWatchNotifier::new(
            Some(alias.clone()),
            CoreRpcHandler::new(),
            ProxyRpcHandler::new(),
            catalog,
            Some(Arc::new(WorkspaceFileIndex::new(alias.clone()))),
        );
        assert_eq!(notifier.workspace, Some(canonical_root));
        let mut watcher = FileWatcher::new();
        watcher.watch(&alias, true, WORKSPACE_EVENT_TOKEN);
        watcher.notify(notifier);
        fs::write(&config, "[languages.Python]\nlanguage_servers = []\n")
            .expect("change settings");
        assert!(matches!(receiver.recv_timeout(Duration::from_secs(5)), Ok(PluginCatalogRpc::Handler(crate::plugin::PluginCatalogNotification::RefreshWorkspaceConfigurations))));
    }

    #[test]
    fn workspace_settings_change_reaches_catalog_from_native_file_watcher() {
        let temporary = tempfile::tempdir().expect("test workspace");
        let workspace = temporary
            .path()
            .canonicalize()
            .expect("canonical workspace");
        let ahead = workspace.join(".ahead");
        fs::create_dir(&ahead).expect("settings directory");
        let config = ahead.join("config.toml");
        fs::write(&config, "[lsp]\n").expect("initial settings");

        let core_rpc = CoreRpcHandler::new();
        let catalog_rpc = PluginCatalogRpcHandler::new(CoreRpcHandler::new());
        let catalog_receiver = catalog_rpc.test_receiver();
        let notifier = FileWatchNotifier::new(
            Some(workspace.clone()),
            core_rpc,
            ProxyRpcHandler::new(),
            catalog_rpc,
            Some(Arc::new(WorkspaceFileIndex::new(workspace.clone()))),
        );
        let mut file_watcher = FileWatcher::new();
        file_watcher.watch(&workspace, true, WORKSPACE_EVENT_TOKEN);
        file_watcher.notify(notifier);

        fs::write(
            &config,
            "[lsp.rust-analyzer.settings.check]\ncommand = 'clippy'\n",
        )
        .expect("update settings");

        assert!(matches!(
            catalog_receiver.recv_timeout(Duration::from_secs(5)),
            Ok(PluginCatalogRpc::Handler(
                crate::plugin::PluginCatalogNotification::RefreshWorkspaceConfigurations
            ))
        ));
    }

    #[test]
    fn user_lsp_settings_change_refreshes_extensions_without_workspace_change() {
        let user_home = tempfile::tempdir().expect("user home");
        let settings_directory = user_home.path().join(".ahead");
        fs::create_dir(&settings_directory).expect("user settings directory");
        let settings = settings_directory.join("settings.toml");
        fs::write(&settings, "[lsp]").expect("initial user settings");
        assert!(is_user_lsp_settings_path(&settings_directory, &settings));
        assert!(!is_user_lsp_settings_path(
            &settings_directory,
            &settings_directory.join("config.toml")
        ));

        let core_rpc = CoreRpcHandler::new();
        let core_receiver = core_rpc.rx();
        let catalog_rpc = PluginCatalogRpcHandler::new(CoreRpcHandler::new());
        let catalog_receiver = catalog_rpc.test_receiver();
        let notifier = FileWatchNotifier::new(
            None,
            core_rpc.clone(),
            ProxyRpcHandler::new(),
            catalog_rpc,
            None,
        );
        *notifier.user_settings_directory.lock() = Some(settings_directory);

        fs::write(
            &settings,
            "[lsp.rust-analyzer.settings]\ncheck = 'clippy'\n",
        )
        .expect("update user settings");
        notifier.handle_user_settings_fs_event(
            notify::Event::new(notify::EventKind::Modify(
                notify::event::ModifyKind::Data(notify::event::DataChange::Content),
            ))
            .add_path(settings.clone()),
        );

        assert!(matches!(
            catalog_receiver.recv_timeout(Duration::from_secs(2)),
            Ok(PluginCatalogRpc::Handler(
                crate::plugin::PluginCatalogNotification::RefreshWorkspaceConfigurations
            ))
        ));
        assert!(matches!(
            core_receiver.recv_timeout(Duration::from_secs(2)),
            Ok(CoreRpc::Notification(notification)) if matches!(
                &*notification,
                CoreNotification::AheadNotification {
                    notification: ahead_rpc::ahead::AheadNotification::PredictionContextChanged { session_id: None }
                }
            )
        ));
        assert!(matches!(
            core_receiver.recv_timeout(Duration::from_secs(2)),
            Ok(CoreRpc::Notification(notification)) if matches!(
                &*notification,
                CoreNotification::WorkspaceFileChange { paths, .. }
                    if paths == &vec![settings]
            )
        ));
    }

    #[test]
    fn user_lsp_settings_directory_created_after_startup_installs_settings_watch() {
        let user_home = tempfile::tempdir().expect("user home");
        let home = user_home.path().canonicalize().expect("canonical home");
        let settings_directory = user_home.path().join(".ahead");
        fs::create_dir(&settings_directory).expect("create user settings directory");
        let settings_directory = settings_directory
            .canonicalize()
            .expect("canonical user settings directory");
        let core_rpc = CoreRpcHandler::new();
        let catalog_rpc = PluginCatalogRpcHandler::new(CoreRpcHandler::new());
        let catalog_receiver = catalog_rpc.test_receiver();
        let mut file_watcher = FileWatcher::new();
        let watched_home = home.clone();
        file_watcher.watch_filtered(
            &home,
            false,
            USER_SETTINGS_HOME_EVENT_TOKEN,
            move |path| is_user_ahead_directory(&watched_home, path),
        );

        let mut notifier = FileWatchNotifier::new(
            None,
            core_rpc,
            ProxyRpcHandler::new(),
            catalog_rpc,
            None,
        );
        *notifier.user_settings_directory.lock() = None;
        notifier.user_settings_home = Some(home);
        notifier.user_settings_watcher = Some(file_watcher.handle());
        notifier.handle_user_settings_home_fs_event(
            notify::Event::new(notify::EventKind::Create(
                notify::event::CreateKind::Folder,
            ))
            .add_path(settings_directory.clone()),
        );
        assert!(matches!(
            catalog_receiver.recv_timeout(Duration::from_secs(5)),
            Ok(PluginCatalogRpc::Handler(
                crate::plugin::PluginCatalogNotification::RefreshWorkspaceConfigurations
            ))
        ));
        assert_eq!(
            notifier.user_settings_directory.lock().as_deref(),
            Some(settings_directory.as_path())
        );

        let settings = settings_directory.join("settings.toml");
        fs::write(&settings, "[lsp]\n").expect("create user settings");
        notifier.handle_user_settings_fs_event(
            notify::Event::new(notify::EventKind::Create(
                notify::event::CreateKind::File,
            ))
            .add_path(settings),
        );
        assert!(matches!(
            catalog_receiver.recv_timeout(Duration::from_secs(5)),
            Ok(PluginCatalogRpc::Handler(
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
                if matches!(*notification, CoreNotification::WorkspaceFileChange { generation: 1, .. })
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

    #[cfg(unix)]
    #[test]
    fn commit_hook_failure_keeps_staged_changes() {
        use std::os::unix::fs::PermissionsExt;

        let workspace = tempfile::tempdir().expect("Git fixture");
        let repo = Repository::init(workspace.path()).expect("init fixture");
        {
            let mut config = repo.config().expect("config");
            config
                .set_str("user.name", "Human Developer")
                .expect("name");
            config
                .set_str("user.email", "human@example.test")
                .expect("email");
        }
        let hook_directory = workspace.path().join("hooks");
        fs::create_dir_all(&hook_directory).expect("create hook directory");
        let hook = hook_directory.join("pre-commit");
        fs::write(&hook, "#!/bin/sh\necho rejected-by-hook >&2\nexit 1\n")
            .expect("write hook");
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))
            .expect("make hook executable");
        repo.config()
            .expect("config")
            .set_str(
                "core.hooksPath",
                hook_directory.to_str().expect("hook path"),
            )
            .expect("configure hooks");

        let path = workspace.path().join("main.rs");
        fs::write(&path, "pub fn answer() -> u8 { 42 }\n").expect("source");
        let error = git_commit(
            workspace.path(),
            "Rejected commit",
            vec![FileDiff::Added(path)],
            &[],
        )
        .expect_err("the pre-commit hook must run");
        assert!(error.to_string().contains("rejected-by-hook"), "{error}");
        let index = repo.index().expect("index");
        let entry = index
            .get_path(std::path::Path::new("main.rs"), 0)
            .expect("staged entry remains");
        let blob = repo.find_blob(entry.id).expect("staged blob");
        assert_eq!(blob.content(), b"pub fn answer() -> u8 { 42 }\n");
        assert!(repo.head().is_err(), "failed commit must not advance HEAD");
    }

    #[cfg(unix)]
    #[test]
    fn signing_failure_keeps_staged_changes() {
        let workspace = tempfile::tempdir().expect("Git fixture");
        let repo = Repository::init(workspace.path()).expect("init fixture");
        {
            let mut config = repo.config().expect("config");
            config
                .set_str("user.name", "Human Developer")
                .expect("name");
            config
                .set_str("user.email", "human@example.test")
                .expect("email");
            config
                .set_str("user.signingkey", "not-a-real-key")
                .expect("signing key");
            config
                .set_str("gpg.format", "openpgp")
                .expect("signing format");
            config
                .set_str("gpg.program", "/usr/bin/false")
                .expect("signing program");
            config
                .set_bool("commit.gpgsign", true)
                .expect("enable signing");
        }

        let path = workspace.path().join("main.rs");
        fs::write(&path, "pub fn answer() -> u8 { 42 }\n").expect("source");
        let error = git_commit(
            workspace.path(),
            "Unsigned commit must fail",
            vec![FileDiff::Added(path)],
            &[],
        )
        .expect_err("the configured signing failure must stop commit");
        assert!(error.to_string().contains("Git commit failed"), "{error}");
        let index = repo.index().expect("index");
        let entry = index
            .get_path(std::path::Path::new("main.rs"), 0)
            .expect("staged entry remains");
        let blob = repo.find_blob(entry.id).expect("staged blob");
        assert_eq!(blob.content(), b"pub fn answer() -> u8 { 42 }\n");
        assert!(repo.head().is_err(), "failed commit must not advance HEAD");
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
            receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("Git response")
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
        let (locked_sender, locked_receiver) = crossbeam_channel::bounded(1);
        let (release_sender, release_receiver) = crossbeam_channel::bounded(1);
        let held_host = host.clone();
        let holder = std::thread::spawn(move || {
            let _guard = held_host.write();
            locked_sender.send(()).expect("host lock acquired");
            release_receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("commit dispatch should release host lock");
        });
        locked_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("host lock acquired");
        let rpc = dispatcher.proxy_rpc.clone();
        let (sender, receiver) = crossbeam_channel::bounded(1);
        rpc.request_async(
            ProxyRequest::GitCommit {
                message: "Commit agent work".to_string(),
                diffs: vec![
                    FileDiff::Added(path),
                    FileDiff::Added(workspace.join("second.md")),
                    FileDiff::Added(workspace.join("third.md")),
                ],
            },
            move |result| sender.send(result).expect("Git response"),
        );
        let ProxyRpc::Request(id, request) =
            rpc.rx().try_recv().expect("Git request")
        else {
            panic!("Git request");
        };
        let started = Instant::now();
        dispatcher.handle_request(id, request);
        let dispatch_time = started.elapsed();
        release_sender.send(()).expect("release host lock");
        holder.join().expect("host lock holder");
        assert!(
            dispatch_time < Duration::from_secs(1),
            "Git commit blocked proxy dispatch for {dispatch_time:?}"
        );
        assert!(matches!(
            receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("Git response")
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

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let hooked_path = workspace.join("hooked.md");
            fs::write(&hooked_path, "Agent content\n").unwrap();
            host.write()
                .create_anchor(
                    &session_id,
                    "hooked.md".to_string(),
                    DisplayRange {
                        start: DisplayPosition { line: 0, col: 0 },
                        end: DisplayPosition { line: 0, col: 13 },
                    },
                    "Agent content".to_string(),
                    ahead_rpc::ahead::AHEAD_ACTOR_ID,
                )
                .unwrap();
            let hooks = workspace.join("git-hooks");
            fs::create_dir_all(&hooks).unwrap();
            let hook = hooks.join("pre-commit");
            fs::write(
                &hook,
                "#!/bin/sh\nprintf 'Hook changed content\\n' > hooked.md\ngit add hooked.md\n",
            )
            .unwrap();
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
            repo.config()
                .unwrap()
                .set_str("core.hooksPath", hooks.to_str().unwrap())
                .unwrap();

            assert!(matches!(
                request_commit(
                    &mut dispatcher,
                    "Commit with a hook rewrite",
                    vec![FileDiff::Added(hooked_path)],
                )
                .expect("hooked commit"),
                ProxyResponse::Success {}
            ));
            let commit = repo.head().unwrap().peel_to_commit().unwrap();
            assert_eq!(commit.author().name(), Some("ahead"));
            let committed_tree = commit.tree().unwrap();
            let committed_entry = committed_tree
                .get_path(std::path::Path::new("hooked.md"))
                .unwrap();
            let committed_blob = repo.find_blob(committed_entry.id()).unwrap();
            assert_eq!(committed_blob.content(), b"Hook changed content\n");
            assert_eq!(
                host.read()
                    .anchors_for_paths(&["hooked.md".to_string()])
                    .unwrap()
                    .len(),
                1,
                "keep attribution when the hook changes the anchored content"
            );
        }

        fs::remove_dir_all(workspace).unwrap();
    }
}
