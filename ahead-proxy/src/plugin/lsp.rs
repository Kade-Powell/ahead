#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::{
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::{self, Child, Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use ahead_core::meta;
use ahead_rpc::{
    RpcError,
    delta::AheadDelta,
    plugin::{PluginId, ServerId},
    style::LineStyle,
};
use anyhow::{Result, anyhow};
use jsonrpc_lite::{Id, Params};
use lsp_types::{
    notification::{Initialized, Notification},
    request::{Initialize, Request},
    *,
};
use parking_lot::Mutex;
use ropey::Rope;
use serde_json::Value;

use super::{
    client_capabilities,
    psp::{
        PluginHandlerNotification, PluginHostHandler, PluginServerHandler,
        PluginServerRpcHandler, ResponseSender, RpcCallback,
        SERVER_SHUTDOWN_TIMEOUT, handle_plugin_server_message,
    },
};
use crate::{buffer::Buffer, plugin::PluginCatalogRpcHandler};

const HEADER_CONTENT_LENGTH: &str = "content-length";
const HEADER_CONTENT_TYPE: &str = "content-type";

pub enum LspRpc {
    Request {
        id: u64,
        method: String,
        params: Params,
    },
    Notification {
        method: String,
        params: Params,
    },
    Response {
        id: u64,
        result: Value,
    },
    Error {
        id: u64,
        error: RpcError,
    },
}

pub struct LspClient {
    server_rpc: PluginServerRpcHandler,
    process: Arc<Mutex<Child>>,
    workspace: Option<PathBuf>,
    host: PluginHostHandler,
    options: Option<Value>,
}

impl PluginServerHandler for LspClient {
    fn method_registered(&mut self, method: &str) -> bool {
        self.host.method_registered(method)
    }

    fn document_supported(
        &mut self,
        lanaguage_id: Option<&str>,
        path: Option<&Path>,
    ) -> bool {
        self.host.document_supported(lanaguage_id, path)
    }

    fn handle_handler_notification(
        &mut self,
        notification: PluginHandlerNotification,
    ) {
        use PluginHandlerNotification::*;
        match notification {
            Initialize => {
                self.initialize();
            }
            InitializeResult(result) => {
                self.host.initialized(result);
            }
            Shutdown => {
                self.shutdown();
            }
        }
    }

    fn handle_host_request(
        &mut self,
        id: Id,
        method: String,
        params: Params,
        resp: ResponseSender,
    ) {
        self.host.handle_request(id, method, params, resp);
    }

    fn handle_host_notification(
        &mut self,
        method: String,
        params: Params,
        from: String,
    ) {
        if let Err(err) = self.host.handle_notification(method, params, from) {
            tracing::error!("{:?}", err);
        }
    }

    fn handle_did_save_text_document(
        &self,
        language_id: String,
        path: PathBuf,
        text_document: TextDocumentIdentifier,
        text: Rope,
    ) {
        self.host.handle_did_save_text_document(
            language_id,
            path,
            text_document,
            text,
        );
    }

    fn handle_did_change_text_document(
        &mut self,
        language_id: String,
        document: lsp_types::VersionedTextDocumentIdentifier,
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
        self.host.handle_did_change_text_document(
            language_id,
            document,
            delta,
            text,
            new_text,
            change,
        );
    }

    fn format_semantic_tokens(
        &self,
        tokens: SemanticTokens,
        text: Rope,
        f: Box<dyn RpcCallback<Vec<LineStyle>, RpcError>>,
    ) {
        self.host.format_semantic_tokens(tokens, text, f);
    }
}

impl LspClient {
    #[allow(clippy::too_many_arguments)]
    fn new(
        plugin_rpc: PluginCatalogRpcHandler,
        document_selector: DocumentSelector,
        workspace: Option<PathBuf>,
        server_id: ServerId,
        server_display_name: String,
        plugin_id: Option<PluginId>,
        pwd: Option<PathBuf>,
        server_uri: Url,
        args: Vec<String>,
        env: Vec<(String, String)>,
        options: Option<Value>,
        workspace_configuration: Option<Value>,
    ) -> Result<Self> {
        let server = match server_uri.scheme() {
            "file" => {
                let path = server_uri.to_file_path().map_err(|_| anyhow!(""))?;
                #[cfg(unix)]
                if let Err(err) = std::process::Command::new("chmod")
                    .arg("+x")
                    .arg(&path)
                    .output()
                {
                    tracing::error!("{:?}", err);
                }
                path.to_str().ok_or_else(|| anyhow!(""))?.to_string()
            }
            "urn" => server_uri.path().to_string(),
            _ => return Err(anyhow!("uri not supported")),
        };

        let mut process = Self::process(workspace.as_ref(), &server, &args, &env)?;
        let stdin = process.stdin.take().unwrap();
        let stdout = process.stdout.take().unwrap();
        let stderr = process.stderr.take().unwrap();

        let mut writer = Box::new(BufWriter::new(stdin));
        let (io_tx, io_rx) = crossbeam_channel::unbounded();
        let server_rpc =
            PluginServerRpcHandler::new(server_id.clone(), plugin_id, io_tx.clone());
        let writer_rpc = server_rpc.clone();
        thread::spawn(move || {
            for msg in io_rx {
                let Some(msg) = msg else {
                    break;
                };
                if let Ok(msg) = serde_json::to_string(&msg) {
                    tracing::debug!("write to lsp: {}", msg);
                    let msg =
                        format!("Content-Length: {}\r\n\r\n{}", msg.len(), msg);
                    if let Err(err) = writer
                        .write_all(msg.as_bytes())
                        .and_then(|()| writer.flush())
                    {
                        tracing::error!("{:?}", err);
                        writer_rpc.shutdown();
                        break;
                    }
                }
            }
        });

        let local_server_rpc = server_rpc.clone();
        let core_rpc = plugin_rpc.core_rpc.clone();
        let server_id_closure = server_id.clone();
        let name = server_display_name.clone();
        thread::spawn(move || {
            let mut reader = Box::new(BufReader::new(stdout));
            loop {
                match read_message(&mut reader) {
                    Ok(message_str) => {
                        if !message_str.contains("$/progress") {
                            tracing::debug!("read from lsp: {}", message_str);
                        }
                        if let Some(resp) = handle_plugin_server_message(
                            &local_server_rpc,
                            &message_str,
                            &name,
                        ) {
                            if let Err(err) = io_tx.send(Some(resp)) {
                                tracing::error!("{:?}", err);
                            }
                        }
                    }
                    Err(error) => {
                        local_server_rpc.shutdown();
                        core_rpc.log(
                            ahead_rpc::core::LogLevel::Error,
                            format!("lsp server {server} stopped: {error}"),
                            Some(format!(
                                "ahead_proxy::plugin::lsp::{}::{}::stopped",
                                server_id_closure.author, server_id_closure.name
                            )),
                        );
                        return;
                    }
                };
            }
        });

        let core_rpc = plugin_rpc.core_rpc.clone();
        let server_id_closure = server_id.clone();
        thread::spawn(move || {
            let mut reader = Box::new(BufReader::new(stderr));
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(n) => {
                        if n == 0 {
                            return;
                        }
                        core_rpc.log(
                            ahead_rpc::core::LogLevel::Trace,
                            line.trim_end().to_string(),
                            Some(format!(
                                "ahead_proxy::plugin::lsp::{}::{}::stderr",
                                server_id_closure.author, server_id_closure.name
                            )),
                        );
                    }
                    Err(_) => {
                        return;
                    }
                }
            }
        });

        let process = Arc::new(Mutex::new(process));
        let monitored_process = process.clone();
        let monitored_rpc = server_rpc.clone();
        let catalog_rpc = plugin_rpc.clone();
        thread::spawn(move || {
            loop {
                monitored_rpc.expire_requests(Instant::now());
                let exit = monitored_process.lock().try_wait();
                let message = match exit {
                    Ok(Some(status)) => format!(
                        "Language server exited ({status}). Restart language servers to reconnect."
                    ),
                    Ok(None) => {
                        thread::sleep(std::time::Duration::from_millis(100));
                        continue;
                    }
                    Err(error) => {
                        format!("Could not inspect language server process: {error}")
                    }
                };
                monitored_rpc.shutdown();
                if let Err(error) = catalog_rpc.catalog_notification(
                    super::PluginCatalogNotification::LanguageServerStopped {
                        plugin_id: monitored_rpc.plugin_id,
                        message,
                    },
                ) {
                    tracing::error!(?error, "reporting language server exit");
                }
                break;
            }
        });

        let host = PluginHostHandler::new(
            workspace.clone(),
            pwd,
            server_id,
            server_display_name,
            document_selector,
            plugin_rpc.core_rpc.clone(),
            server_rpc.clone(),
            plugin_rpc.clone(),
            workspace_configuration,
        );

        Ok(Self {
            server_rpc,
            process,
            workspace,
            host,
            options,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start(
        plugin_rpc: PluginCatalogRpcHandler,
        document_selector: DocumentSelector,
        workspace: Option<PathBuf>,
        server_id: ServerId,
        server_display_name: String,
        plugin_id: Option<PluginId>,
        pwd: Option<PathBuf>,
        server_uri: Url,
        args: Vec<String>,
        env: Vec<(String, String)>,
        options: Option<Value>,
        workspace_configuration: Option<Value>,
    ) -> Result<(PluginId, PluginServerRpcHandler)> {
        let mut lsp = Self::new(
            plugin_rpc,
            document_selector,
            workspace,
            server_id,
            server_display_name,
            plugin_id,
            pwd,
            server_uri,
            args,
            env,
            options,
            workspace_configuration,
        )?;
        let plugin_id = lsp.server_rpc.plugin_id;

        let rpc = lsp.server_rpc.clone();
        let handler = lsp.server_rpc.clone();
        thread::spawn(move || {
            rpc.mainloop(&mut lsp);
        });
        Ok((plugin_id, handler))
    }

    fn initialize(&mut self) {
        let root_uri = self
            .workspace
            .clone()
            .map(|p| Url::from_directory_path(p).unwrap());
        tracing::debug!("initialization_options {:?}", self.options);
        #[allow(deprecated)]
        let params = InitializeParams {
            process_id: Some(process::id()),
            root_uri: root_uri.clone(),
            initialization_options: self.options.clone(),
            capabilities: client_capabilities(),
            trace: Some(TraceValue::Verbose),
            workspace_folders: root_uri.map(|uri| {
                vec![WorkspaceFolder {
                    name: uri.as_str().to_string(),
                    uri,
                }]
            }),
            client_info: Some(ClientInfo {
                name: meta::NAME.to_owned(),
                version: Some(meta::VERSION.to_owned()),
            }),
            locale: None,
            root_path: None,
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        let result = self
            .server_rpc
            .server_request(Initialize::METHOD, params, None, None, false)
            .map_err(|error| error.message)
            .and_then(|value| {
                serde_json::from_value::<InitializeResult>(value)
                    .map_err(|error| format!("Invalid initialize response: {error}"))
            });
        match result {
            Ok(result) => {
                self.server_rpc.server_notification(
                    Initialized::METHOD,
                    InitializedParams {},
                    None,
                    None,
                    false,
                );
                self.host.initialized(result);
            }
            Err(error) => {
                self.host.initialization_failed(error);
                self.server_rpc.shutdown();
            }
        }
    }

    fn shutdown(&mut self) {
        let deadline = Instant::now() + SERVER_SHUTDOWN_TIMEOUT;
        if matches!(self.process.lock().try_wait(), Ok(None)) {
            if let Err(error) = self.server_rpc.shutdown_protocol(deadline) {
                tracing::warn!(?error, "language server graceful shutdown failed");
            }
            while Instant::now() < deadline
                && matches!(self.process.lock().try_wait(), Ok(None))
            {
                thread::sleep(Duration::from_millis(10));
            }
        }
        self.server_rpc.close_io();
        let mut process = self.process.lock();
        if !matches!(process.try_wait(), Ok(Some(_))) {
            if let Err(err) = process.kill() {
                tracing::error!("{:?}", err);
            }
        }
        if let Err(err) = process.wait() {
            tracing::error!("{:?}", err);
        }
    }

    fn process(
        workspace: Option<&PathBuf>,
        server: &str,
        args: &[String],
        env: &[(String, String)],
    ) -> Result<Child> {
        let mut process = Command::new(server);
        if let Some(workspace) = workspace {
            process.current_dir(workspace);
        }

        process.args(args);
        process.envs(env.iter().map(|(key, value)| (key, value)));

        #[cfg(target_os = "windows")]
        let process = process.creation_flags(0x08000000);
        let child = process
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        Ok(child)
    }
}

pub struct DocumentFilter {
    /// The document must have this language id, if it exists
    pub language_id: Option<String>,
    /// The document's path must match this glob, if it exists
    pub pattern: Option<globset::GlobMatcher>,
    // TODO: URI Scheme from lsp-types document filter
}
impl DocumentFilter {
    /// Constructs a document filter from the LSP version
    /// This ignores any fields that are badly constructed
    pub(crate) fn from_lsp_filter_loose(
        filter: &lsp_types::DocumentFilter,
    ) -> DocumentFilter {
        DocumentFilter {
            language_id: filter.language.clone(),
            // TODO: clean this up
            pattern: filter
                .pattern
                .as_deref()
                .map(globset::Glob::new)
                .and_then(Result::ok)
                .map(|x| globset::Glob::compile_matcher(&x)),
        }
    }
}

pub enum LspHeader {
    ContentType,
    ContentLength(usize),
}

fn parse_header(s: &str) -> Result<LspHeader> {
    let split: Vec<String> =
        s.splitn(2, ": ").map(|s| s.trim().to_lowercase()).collect();
    if split.len() != 2 {
        return Err(anyhow!("Malformed"));
    };
    match split[0].as_ref() {
        HEADER_CONTENT_TYPE => Ok(LspHeader::ContentType),
        HEADER_CONTENT_LENGTH => {
            Ok(LspHeader::ContentLength(split[1].parse::<usize>()?))
        }
        _ => Err(anyhow!("Unknown parse error occurred")),
    }
}

pub fn read_message<T: BufRead>(reader: &mut T) -> Result<String> {
    let mut buffer = String::new();
    let mut content_length: Option<usize> = None;

    loop {
        buffer.clear();
        let _ = reader.read_line(&mut buffer)?;
        // eprin
        match &buffer {
            s if s.trim().is_empty() => break,
            s => {
                match parse_header(s)? {
                    LspHeader::ContentLength(len) => content_length = Some(len),
                    LspHeader::ContentType => (),
                };
            }
        };
    }

    let content_length = content_length
        .ok_or_else(|| anyhow!("missing content-length header: {}", buffer))?;

    let mut body_buffer = vec![0; content_length];
    reader.read_exact(&mut body_buffer)?;

    let body = String::from_utf8(body_buffer)?;
    Ok(body)
}

pub fn get_change_for_sync_kind(
    sync_kind: TextDocumentSyncKind,
    buffer: &Buffer,
    content_change: &TextDocumentContentChangeEvent,
) -> Option<Vec<TextDocumentContentChangeEvent>> {
    match sync_kind {
        TextDocumentSyncKind::NONE => None,
        TextDocumentSyncKind::FULL => {
            let text_document_content_change_event =
                TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: buffer.get_document(),
                };
            Some(vec![text_document_content_change_event])
        }
        TextDocumentSyncKind::INCREMENTAL => Some(vec![content_change.clone()]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::psp::{PluginServerRpc, ResponseHandler};

    #[cfg(unix)]
    #[test]
    fn shutdown_exits_cooperative_processes_and_kills_unresponsive_ones() {
        use crate::plugin::{PluginCatalogNotification, PluginCatalogRpc};
        let script = r#"
shutdown_seen=no
while IFS= read -r header; do
    length=$(printf '%s' "$header" | tr -cd '0-9')
    IFS= read -r separator
    body=$(dd bs=1 count="$length" 2>/dev/null)
    id=$(printf '%s' "$body" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
    case "$body" in
        *'"method":"initialize"'*)
            response='{"jsonrpc":"2.0","id":0,"result":{"capabilities":{}}}'
            printf 'Content-Length: %s\r\n\r\n%s' "${#response}" "$response"
            ;;
        *'"method":"shutdown"'*)
            [ "$1" = ignore ] && continue
            shutdown_seen=yes
            response=$(printf '{"jsonrpc":"2.0","id":%s,"result":null}' "$id")
            printf 'Content-Length: %s\r\n\r\n%s' "${#response}" "$response"
            ;;
        *'"method":"exit"'*)
            [ "$1" = ignore ] && continue
            [ "$shutdown_seen" = yes ] || exit 7
            exit 0
            ;;
    esac
done
[ "$1" = ignore ] && exec sleep 30
exit 8
"#;
        for mode in ["cooperative", "ignore"] {
            let catalog =
                PluginCatalogRpcHandler::new(ahead_rpc::core::CoreRpcHandler::new());
            let notifications =
                catalog.plugin_rx.lock().take().expect("catalog receiver");
            let mut client = LspClient::new(
                catalog,
                Vec::new(),
                None,
                ServerId {
                    author: "ahead".into(),
                    name: "lsp-shutdown-test".into(),
                },
                "shutdown-test".into(),
                None,
                None,
                Url::parse("urn:sh").expect("shell URI"),
                vec!["-c".into(), script.into(), "test-lsp".into(), mode.into()],
                Vec::new(),
                None,
                None,
            )
            .expect("owned fake server");
            let process = client.process.clone();
            let rpc = client.server_rpc.clone();
            let handler = rpc.clone();
            let mainloop = thread::spawn(move || handler.mainloop(&mut client));
            let ready = matches!(notifications.recv_timeout(Duration::from_secs(5)), Ok(PluginCatalogRpc::Handler(PluginCatalogNotification::LanguageServerStatus { params, .. })) if params.is_ok());
            let started = Instant::now();
            rpc.shutdown();
            let stopped = rpc.wait_for_shutdown();
            if !stopped {
                let mut process = process.lock();
                process.kill().expect("clean up unresponsive test child");
                process.wait().expect("reap test child");
            }
            assert!(ready, "fake server initialized ({mode})");
            assert!(stopped, "shutdown must be bounded ({mode})");
            mainloop.join().expect("handler exited");
            let status = process
                .lock()
                .try_wait()
                .expect("child status")
                .expect("child exited");
            assert_eq!(status.success(), mode == "cooperative", "{status}");
            if mode == "ignore" {
                assert!(started.elapsed() >= SERVER_SHUTDOWN_TIMEOUT);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn process_exit_is_detected_even_when_a_descendant_keeps_stdout_open() {
        use crate::plugin::{PluginCatalogNotification, PluginCatalogRpc};
        let catalog_rpc =
            PluginCatalogRpcHandler::new(ahead_rpc::core::CoreRpcHandler::new());
        let notifications = catalog_rpc
            .plugin_rx
            .lock()
            .take()
            .expect("catalog receiver");
        let (plugin_id, rpc) = LspClient::start(
            catalog_rpc,
            Vec::new(),
            None,
            ServerId {
                author: "ahead".into(),
                name: "lsp-crash-test".into(),
            },
            "crash-test".into(),
            None,
            None,
            Url::parse("urn:sh").expect("shell URI"),
            vec!["-c".into(), "sleep 3 & exit 7".into()],
            Vec::new(),
            None,
            None,
        )
        .expect("owned child");
        let (sender, receiver) = crossbeam_channel::bounded(1);
        rpc.server_request_async(
            "test/pending",
            Value::Null,
            None,
            None,
            true,
            move |reply| sender.send(reply).expect("pending request reply"),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let notification = notifications
                .recv_deadline(deadline)
                .expect("process exit before inherited stdout closes");
            if let PluginCatalogRpc::Handler(
                PluginCatalogNotification::LanguageServerStopped {
                    plugin_id: stopped,
                    message,
                },
            ) = notification
            {
                assert_eq!(stopped, plugin_id);
                assert!(message.contains("exited"), "{message}");
                break;
            }
        }
        assert!(
            receiver
                .recv_deadline(deadline)
                .expect("failed pending request")
                .is_err()
        );
        assert!(rpc.wait_for_shutdown());
    }

    #[test]
    fn completion_resolve_is_a_noop_without_the_optional_capability() {
        let core_rpc = ahead_rpc::core::CoreRpcHandler::new();
        let catalog_rpc = PluginCatalogRpcHandler::new(core_rpc.clone());
        let server_id = ServerId {
            author: "ahead".into(),
            name: "test".into(),
        };
        let (sender, receiver) = crossbeam_channel::unbounded();
        let server_rpc =
            PluginServerRpcHandler::new(server_id.clone(), None, sender);
        let host = PluginHostHandler::new(
            None,
            None,
            server_id,
            "Test LSP".into(),
            Vec::new(),
            core_rpc,
            server_rpc.clone(),
            catalog_rpc,
            None,
        );
        let (reply_sender, reply_receiver) = crossbeam_channel::bounded(1);
        let item = serde_json::json!({"label": "calculate", "additionalTextEdits": [], "data": {"opaque": 1}});
        server_rpc.handle_rpc(PluginServerRpc::ServerRequest {
            id: Id::Num(42),
            method: lsp_types::request::ResolveCompletionItem::METHOD.into(),
            params: Params::from(item.clone()),
            language_id: None,
            path: None,
            rh: ResponseHandler::Chan(reply_sender),
        });
        server_rpc.handle_rpc(PluginServerRpc::Shutdown);
        let mut process =
            Command::new(std::env::current_exe().expect("test executable"))
                .arg("--list")
                .stdout(Stdio::null())
                .spawn()
                .expect("test child");
        assert!(process.wait().expect("reap test child").success());
        let mut client = LspClient {
            server_rpc: server_rpc.clone(),
            process: Arc::new(Mutex::new(process)),
            workspace: None,
            host,
            options: None,
        };
        let responder = server_rpc.clone();
        let initialize = std::thread::spawn(move || {
            let request = receiver
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("initialize request")
                .expect("initialize frame");
            assert_eq!(request.get_method(), Some(Initialize::METHOD));
            responder.handle_server_response(
                request.get_id().expect("initialize id").clone(),
                Ok(serde_json::json!({"capabilities": {}})),
            );
            let initialized = receiver
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("initialized notification")
                .expect("initialized frame");
            assert_eq!(initialized.get_method(), Some(Initialized::METHOD));
            receiver
        });
        server_rpc.mainloop(&mut client);
        assert_eq!(
            reply_receiver
                .try_recv()
                .expect("reply")
                .expect("no resolve needed"),
            item
        );
        assert!(
            initialize
                .join()
                .expect("server initialization")
                .try_recv()
                .is_err(),
            "must not send an unsupported request"
        );
    }
}
