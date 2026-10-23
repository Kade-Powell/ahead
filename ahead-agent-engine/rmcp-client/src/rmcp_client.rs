use std::collections::HashMap;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::future::Future;
use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use anyhow::anyhow;
use codex_config::types::McpServerEnvVar;
use futures::FutureExt;
use futures::future::BoxFuture;
use rmcp::model::CallToolRequestParams;
use rmcp::model::CallToolResult;
use rmcp::model::ClientNotification;
use rmcp::model::ClientRequest;
use rmcp::model::ClientResult;
use rmcp::model::CustomNotification;
use rmcp::model::CustomRequest;
use rmcp::model::ElicitRequestParams;
use rmcp::model::ElicitResult;
use rmcp::model::ElicitationAction;
use rmcp::model::Extensions;
use rmcp::model::InitializeRequestParams;
use rmcp::model::InputRequest;
use rmcp::model::InputResponses;
use rmcp::model::ListResourceTemplatesResult;
use rmcp::model::ListResourcesResult;
use rmcp::model::ListToolsResult;
use rmcp::model::PaginatedRequestParams;
use rmcp::model::ProtocolVersion;
use rmcp::model::ReadResourceRequestParams;
use rmcp::model::ReadResourceResult;
use rmcp::model::RequestId;
use rmcp::model::RequestMetaObject;
use rmcp::model::RequestParamsMeta;
use rmcp::model::ServerPeerInfo;
use rmcp::model::ServerRequest;
use rmcp::model::ServerResult;
use rmcp::service::ClientCacheConfig;
use rmcp::service::ClientServiceExt;
use rmcp::service::PeerRequestOptions;
use rmcp::service::RequestContext;
use rmcp::service::RoleClient;
use rmcp::service::RunningService;
use rmcp::service::Service;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::sync::watch;
use tokio::time;
use tokio_util::sync::CancellationToken;
use tracing::instrument;
use tracing::warn;

use crate::elicitation_client_service::ElicitationClientService;
use crate::in_process_transport::InProcessTransportFactory;
use crate::protocol_mode::McpProtocolMode;
use crate::stdio_server_launcher::StdioServerCommand;
use crate::stdio_server_launcher::StdioServerLauncher;
use crate::stdio_server_launcher::StdioServerProcessHandle;
use crate::stdio_server_launcher::StdioServerTransport;

enum PendingTransport {
    InProcess {
        transport: tokio::io::DuplexStream,
    },
    Stdio {
        transport: Box<StdioServerTransport>,
    },
}

enum ClientState {
    Connecting {
        transport: Option<PendingTransport>,
    },
    Ready {
        service: Arc<RunningService<RoleClient, ElicitationClientService>>,
    },
    Closed,
}

#[derive(Clone)]
pub(crate) struct ElicitationPauseState {
    active_count: Arc<AtomicUsize>,
    paused: watch::Sender<bool>,
}

impl ElicitationPauseState {
    fn new() -> Self {
        let (paused, _rx) = watch::channel(false);
        Self {
            active_count: Arc::new(AtomicUsize::new(0)),
            paused,
        }
    }

    pub(crate) fn enter(&self) -> ElicitationPauseGuard {
        if self.active_count.fetch_add(1, Ordering::AcqRel) == 0 {
            self.paused.send_replace(true);
        }
        ElicitationPauseGuard {
            pause_state: self.clone(),
        }
    }

    fn subscribe(&self) -> watch::Receiver<bool> {
        self.paused.subscribe()
    }
}

pub(crate) struct ElicitationPauseGuard {
    pause_state: ElicitationPauseState,
}

impl Drop for ElicitationPauseGuard {
    fn drop(&mut self) {
        if self.pause_state.active_count.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.pause_state.paused.send_replace(false);
        }
    }
}

async fn active_time_timeout<T, Fut>(
    duration: Duration,
    mut pause_state: watch::Receiver<bool>,
    operation: Fut,
) -> std::result::Result<T, ()>
where
    Fut: Future<Output = T>,
{
    let mut remaining = duration;
    tokio::pin!(operation);

    loop {
        if *pause_state.borrow_and_update() {
            tokio::select! {
                result = &mut operation => return Ok(result),
                changed = pause_state.changed() => {
                    if changed.is_err() {
                        return time::timeout(remaining, operation).await.map_err(|_| ());
                    }
                    let _paused = *pause_state.borrow_and_update();
                }
            }
            continue;
        }

        let active_start = Instant::now();
        tokio::select! {
            result = &mut operation => return Ok(result),
            _ = time::sleep(remaining) => {
                return Err(());
            }
            changed = pause_state.changed() => {
                if changed.is_err() {
                    return time::timeout(remaining, operation).await.map_err(|_| ());
                }
                if *pause_state.borrow_and_update() {
                    remaining = remaining.saturating_sub(active_start.elapsed());
                    if remaining.is_zero() {
                        return Err(());
                    }
                }
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ClientOperationError {
    #[error(transparent)]
    Service(#[from] rmcp::service::ServiceError),
    #[error("timed out awaiting {label} after {duration:.0?}")]
    Timeout { label: String, duration: Duration },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Elicitation {
    Mcp(ElicitRequestParams),
}

impl Elicitation {
    pub fn meta(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        match self {
            Self::Mcp(request) => request.meta().map(|meta| &meta.0.0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElicitationResponse {
    pub action: ElicitationAction,
    pub content: Option<serde_json::Value>,
    #[serde(rename = "_meta")]
    pub meta: Option<serde_json::Value>,
}

impl From<ElicitResult> for ElicitationResponse {
    fn from(value: ElicitResult) -> Self {
        Self {
            action: value.action,
            content: value.content,
            meta: value.meta.map(|meta| Value::Object(meta.0)),
        }
    }
}

impl From<ElicitationResponse> for ElicitResult {
    fn from(value: ElicitationResponse) -> Self {
        let mut result = Self::new(value.action);
        result.content = value.content;
        result.meta = value.meta.and_then(|meta| match meta {
            Value::Object(meta) => Some(rmcp::model::MetaObject::from(meta)),
            _ => None,
        });
        result
    }
}

/// Interface for sending elicitation requests to the UI and awaiting a response.
pub type SendElicitation = Box<
    dyn Fn(RequestId, Elicitation) -> BoxFuture<'static, Result<ElicitationResponse>> + Send + Sync,
>;

/// MCP client implemented on top of the official `rmcp` SDK.
/// https://github.com/modelcontextprotocol/rust-sdk
pub struct RmcpClient {
    state: Mutex<ClientState>,
    stdio_process: Option<StdioServerProcessHandle>,
    protocol_mode: McpProtocolMode,
    elicitation_pause_state: ElicitationPauseState,
}

impl RmcpClient {
    /// Returns the protocol compatibility policy captured when this client was created.
    pub fn protocol_mode(&self) -> McpProtocolMode {
        self.protocol_mode
    }

    pub async fn new_in_process_client(
        factory: Arc<dyn InProcessTransportFactory>,
    ) -> io::Result<Self> {
        let transport = PendingTransport::InProcess {
            transport: factory.open().await.map_err(io::Error::other)?,
        };

        Ok(Self {
            state: Mutex::new(ClientState::Connecting {
                transport: Some(transport),
            }),
            stdio_process: None,
            protocol_mode: McpProtocolMode::Legacy,
            elicitation_pause_state: ElicitationPauseState::new(),
        })
    }

    pub async fn new_stdio_client(
        program: OsString,
        args: Vec<OsString>,
        env: Option<HashMap<OsString, OsString>>,
        env_vars: &[McpServerEnvVar],
        cwd: Option<String>,
        launcher: Arc<dyn StdioServerLauncher>,
    ) -> io::Result<Self> {
        Self::new_stdio_client_with_protocol_mode(
            program,
            args,
            env,
            env_vars,
            cwd,
            launcher,
            McpProtocolMode::Legacy,
        )
        .await
    }

    /// Constructs a stdio client with an explicitly selected compatibility policy.
    #[allow(clippy::too_many_arguments)]
    pub async fn new_stdio_client_with_protocol_mode(
        program: OsString,
        args: Vec<OsString>,
        mut env: Option<HashMap<OsString, OsString>>,
        env_vars: &[McpServerEnvVar],
        cwd: Option<String>,
        launcher: Arc<dyn StdioServerLauncher>,
        protocol_mode: McpProtocolMode,
    ) -> io::Result<Self> {
        let requested_stdio_version = match protocol_mode {
            McpProtocolMode::Legacy => None,
            McpProtocolMode::V20260728 => env
                .as_mut()
                .and_then(|env| env.remove(OsStr::new("CODEX_MCP_PROTOCOL_VERSION"))),
        };
        let protocol_mode = protocol_mode.stdio_mode(requested_stdio_version.as_deref())?;
        let command =
            StdioServerCommand::new(program, args, env, env_vars.to_vec(), cwd, protocol_mode);
        let transport = launcher.launch(command).await.map_err(io::Error::other)?;
        let transport = PendingTransport::Stdio {
            transport: Box::new(transport),
        };
        let stdio_process = match &transport {
            PendingTransport::Stdio { transport } => Some(transport.process_handle()),
            PendingTransport::InProcess { .. } => None,
        };

        Ok(Self {
            state: Mutex::new(ClientState::Connecting {
                transport: Some(transport),
            }),
            stdio_process,
            protocol_mode,
            elicitation_pause_state: ElicitationPauseState::new(),
        })
    }

    /// Perform the initialization handshake with the MCP server.
    /// https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle#initialization
    #[instrument(level = "trace", skip_all)]
    pub async fn initialize(
        &self,
        params: InitializeRequestParams,
        timeout: Option<Duration>,
        send_elicitation: SendElicitation,
    ) -> Result<ServerPeerInfo> {
        let client_service = ElicitationClientService::new(
            params.clone(),
            send_elicitation,
            self.elicitation_pause_state.clone(),
        );
        let pending_transport = {
            let mut guard = self.state.lock().await;
            match &mut *guard {
                ClientState::Connecting { transport } => match transport.take() {
                    Some(transport) => transport,
                    None => return Err(anyhow!("client already initializing")),
                },
                ClientState::Ready { .. } => return Err(anyhow!("client already initialized")),
                ClientState::Closed => return Err(anyhow!("MCP client is shut down")),
            }
        };

        let service = self
            .connect_pending_transport(pending_transport, client_service, timeout)
            .await?;

        let initialize_result_rmcp = service
            .peer()
            .peer_info()
            .ok_or_else(|| anyhow!("handshake succeeded but server info was missing"))?;
        let initialize_result = initialize_result_rmcp.as_ref().clone();

        {
            let mut guard = self.state.lock().await;
            if matches!(*guard, ClientState::Closed) {
                return Err(anyhow!("MCP client is shut down"));
            }
            *guard = ClientState::Ready { service };
        }

        Ok(initialize_result)
    }

    pub async fn list_tools(
        &self,
        params: Option<PaginatedRequestParams>,
        timeout: Option<Duration>,
    ) -> Result<ListToolsResult> {
        self.run_service_operation("tools/list", timeout, move |service| {
            let params = params.clone();
            async move { service.list_tools(params).await }.boxed()
        })
        .await
    }

    pub async fn list_resources(
        &self,
        params: Option<PaginatedRequestParams>,
        timeout: Option<Duration>,
    ) -> Result<ListResourcesResult> {
        self.run_service_operation("resources/list", timeout, move |service| {
            let params = params.clone();
            async move { service.list_resources(params).await }.boxed()
        })
        .await
    }

    pub async fn list_resource_templates(
        &self,
        params: Option<PaginatedRequestParams>,
        timeout: Option<Duration>,
    ) -> Result<ListResourceTemplatesResult> {
        self.run_service_operation("resources/templates/list", timeout, move |service| {
            let params = params.clone();
            async move { service.list_resource_templates(params).await }.boxed()
        })
        .await
    }

    pub async fn read_resource(
        &self,
        params: ReadResourceRequestParams,
        timeout: Option<Duration>,
    ) -> Result<ReadResourceResult> {
        let requested_modern = self.protocol_mode == McpProtocolMode::V20260728;
        self.run_service_operation("resources/read", timeout, move |service| {
            let params = params.clone();
            async move {
                let modern_session = requested_modern
                    && service
                        .peer()
                        .peer_info()
                        .is_some_and(|info| info.protocol_version == ProtocolVersion::V_2026_07_28);
                if modern_session {
                    service.read_resource(params).await
                } else {
                    service.peer().read_resource(params).await
                }
            }
            .boxed()
        })
        .await
    }

    pub async fn call_tool(
        &self,
        name: String,
        arguments: Option<serde_json::Value>,
        meta: Option<serde_json::Value>,
        timeout: Option<Duration>,
    ) -> Result<CallToolResult> {
        self.call_tool_cancellable(name, arguments, meta, timeout, CancellationToken::new())
            .await
    }

    pub async fn call_tool_cancellable(
        &self,
        name: String,
        arguments: Option<serde_json::Value>,
        meta: Option<serde_json::Value>,
        timeout: Option<Duration>,
        cancellation_token: CancellationToken,
    ) -> Result<CallToolResult> {
        let arguments = match arguments {
            Some(Value::Object(map)) => Some(map),
            Some(other) => {
                return Err(anyhow!(
                    "MCP tool arguments must be a JSON object, got {other}"
                ));
            }
            None => None,
        };
        let meta = match meta {
            Some(Value::Object(map)) => Some(RequestMetaObject::from(map)),
            Some(other) => {
                return Err(anyhow!(
                    "MCP tool request _meta must be a JSON object, got {other}"
                ));
            }
            None => None,
        };
        let mut params = CallToolRequestParams::new(name);
        params.arguments = arguments;
        let service = self.service().await?;
        let requested_modern = self.protocol_mode == McpProtocolMode::V20260728;
        let operation_cancellation = cancellation_token.child_token();
        let task_cancellation = operation_cancellation.clone();
        let mut operation = tokio::spawn(async move {
            call_tool_with_cancellation(service, params, meta, requested_modern, task_cancellation)
                .await
        });

        let result = match timeout {
            Some(duration) => {
                active_time_timeout(
                    duration,
                    self.elicitation_pause_state.subscribe(),
                    &mut operation,
                )
                .await
            }
            None => Ok((&mut operation).await),
        };

        match result {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(error))) => Err(error.into()),
            Ok(Err(error)) => Err(anyhow!("MCP tools/call task failed: {error}")),
            Err(()) => {
                let Some(duration) = timeout else {
                    return Err(anyhow!("MCP tools/call timed out without a deadline"));
                };
                operation_cancellation.cancel();
                match time::timeout(Duration::from_millis(250), &mut operation).await {
                    Ok(Ok(Ok(_))) => {}
                    Ok(Ok(Err(rmcp::service::ServiceError::Cancelled { .. }))) => {}
                    Ok(Ok(Err(error))) => {
                        warn!("MCP tools/call cleanup after timeout failed: {error}");
                    }
                    Ok(Err(error)) => {
                        warn!("MCP tools/call task failed during timeout cleanup: {error}");
                    }
                    Err(_) => {
                        operation.abort();
                        match operation.await {
                            Ok(_) => {}
                            Err(error) if error.is_cancelled() => {}
                            Err(error) => {
                                warn!("MCP tools/call task failed to abort after timeout: {error}");
                            }
                        }
                    }
                }
                Err(ClientOperationError::Timeout {
                    label: "tools/call".to_string(),
                    duration,
                }
                .into())
            }
        }
    }

    pub async fn send_custom_notification(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<()> {
        self.run_service_operation(
            "notifications/custom",
            /*timeout*/ None,
            move |service| {
                let params = params.clone();
                async move {
                    service
                        .send_notification(ClientNotification::CustomNotification(
                            CustomNotification {
                                method: method.to_string(),
                                params,
                                extensions: Extensions::new(),
                            },
                        ))
                        .await
                }
                .boxed()
            },
        )
        .await
    }

    pub async fn send_custom_request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<ServerResult> {
        self.send_custom_request_with_timeout(method, params, /*timeout*/ None)
            .await
    }

    pub async fn send_custom_request_with_timeout(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
        timeout: Option<Duration>,
    ) -> Result<ServerResult> {
        self.run_service_operation("requests/custom", timeout, move |service| {
            let params = params.clone();
            async move {
                service
                    .send_request(ClientRequest::CustomRequest(CustomRequest::new(
                        method, params,
                    )))
                    .await
            }
            .boxed()
        })
        .await
    }

    async fn service(&self) -> Result<Arc<RunningService<RoleClient, ElicitationClientService>>> {
        let guard = self.state.lock().await;
        match &*guard {
            ClientState::Ready { service, .. } => Ok(Arc::clone(service)),
            ClientState::Connecting { .. } => Err(anyhow!("MCP client not initialized")),
            ClientState::Closed => Err(anyhow!("MCP client is shut down")),
        }
    }

    /// Returns whether an initialized transport or its underlying service has stopped.
    pub async fn is_closed(&self) -> bool {
        let state = self.state.lock().await;
        match &*state {
            ClientState::Ready { service, .. } => {
                service.is_closed() || service.peer().is_transport_closed()
            }
            ClientState::Connecting { .. } => false,
            ClientState::Closed => true,
        }
    }

    /// Stop the MCP transport and any stdio server process owned by this client.
    pub async fn shutdown(&self) {
        let previous_state = {
            let mut guard = self.state.lock().await;
            std::mem::replace(&mut *guard, ClientState::Closed)
        };

        if let Some(process) = &self.stdio_process
            && let Err(error) = process.terminate().await
        {
            warn!("failed to terminate MCP stdio server process: {error}");
        }

        drop(previous_state);
    }

    async fn connect_pending_transport(
        &self,
        pending_transport: PendingTransport,
        client_service: ElicitationClientService,
        timeout: Option<Duration>,
    ) -> Result<Arc<RunningService<RoleClient, ElicitationClientService>>> {
        let lifecycle = self.protocol_mode.client_lifecycle();
        let transport = match pending_transport {
            PendingTransport::InProcess { transport } => client_service
                .serve_with_lifecycle(transport, lifecycle)
                .boxed(),
            PendingTransport::Stdio { transport } => client_service
                .serve_with_lifecycle(*transport, lifecycle)
                .boxed(),
        };

        let service_result = match timeout {
            Some(duration) => match time::timeout(duration, transport).await {
                Ok(result) => result.map_err(anyhow::Error::from),
                Err(_elapsed) => Err(anyhow!(
                    "timed out handshaking with MCP server after {duration:?}"
                )),
            },
            None => transport.await.map_err(anyhow::Error::from),
        };
        let service = match service_result {
            Ok(service) => service,
            Err(error) => return Err(error),
        };

        // Preserve Codex's existing snapshot and request-freshness behavior. rmcp 3
        // enables response caching and stale-on-error fallback by default.
        service
            .peer()
            .set_response_cache_config(ClientCacheConfig::disabled())
            .await;

        Ok(Arc::new(service))
    }

    async fn run_service_operation<T, F, Fut>(
        &self,
        label: &str,
        timeout: Option<Duration>,
        operation: F,
    ) -> Result<T>
    where
        F: Fn(Arc<RunningService<RoleClient, ElicitationClientService>>) -> Fut,
        Fut: std::future::Future<Output = std::result::Result<T, rmcp::service::ServiceError>>,
    {
        let service = self.service().await?;
        Self::run_service_operation_once(
            service,
            label,
            timeout,
            self.elicitation_pause_state.clone(),
            &operation,
        )
        .await
        .map_err(Into::into)
    }

    async fn run_service_operation_once<T, F, Fut>(
        service: Arc<RunningService<RoleClient, ElicitationClientService>>,
        label: &str,
        timeout: Option<Duration>,
        pause_state: ElicitationPauseState,
        operation: &F,
    ) -> std::result::Result<T, ClientOperationError>
    where
        F: Fn(Arc<RunningService<RoleClient, ElicitationClientService>>) -> Fut,
        Fut: std::future::Future<Output = std::result::Result<T, rmcp::service::ServiceError>>,
    {
        match timeout {
            Some(duration) => {
                active_time_timeout(duration, pause_state.subscribe(), operation(service))
                    .await
                    .map_err(|_| ClientOperationError::Timeout {
                        label: label.to_string(),
                        duration,
                    })?
                    .map_err(ClientOperationError::from)
            }
            None => operation(service).await.map_err(ClientOperationError::from),
        }
    }
}

async fn call_tool_with_cancellation(
    service: Arc<RunningService<RoleClient, ElicitationClientService>>,
    mut params: CallToolRequestParams,
    meta: Option<RequestMetaObject>,
    requested_modern: bool,
    cancellation_token: CancellationToken,
) -> std::result::Result<CallToolResult, rmcp::service::ServiceError> {
    let mut state_only_rounds = 0usize;
    for _round in 0..rmcp::model::DEFAULT_MRTR_MAX_ROUNDS {
        if cancellation_token.is_cancelled() {
            return Err(mcp_tool_call_cancelled());
        }

        let modern_session = requested_modern
            && service
                .peer()
                .peer_info()
                .is_some_and(|info| info.protocol_version == ProtocolVersion::V_2026_07_28);
        let mut request_params = params.clone();
        let mut options = PeerRequestOptions::no_options();
        if modern_session {
            request_params.meta = meta.clone();
        } else {
            options.meta = meta.clone();
        }
        let mut request = service
            .peer()
            .send_cancellable_request(
                ClientRequest::CallToolRequest(rmcp::model::CallToolRequest::new(request_params)),
                options,
            )
            .await?;
        let response = tokio::select! {
            biased;
            response = &mut request.rx => Some(response),
            _ = cancellation_token.cancelled() => None,
        };
        let response = match response {
            Some(Ok(response)) => response?,
            Some(Err(_)) => return Err(rmcp::service::ServiceError::TransportClosed),
            None => {
                let reason = "MCP tool call cancelled".to_string();
                request.cancel(Some(reason.clone())).await?;
                return Err(rmcp::service::ServiceError::Cancelled {
                    reason: Some(reason),
                });
            }
        };

        match response {
            ServerResult::CallToolResult(result) => return Ok(result),
            ServerResult::InputRequiredResult(result) => {
                let requests = result.input_requests.unwrap_or_default();
                let had_input_requests = !requests.is_empty();
                if !had_input_requests && result.request_state.is_none() {
                    return Err(rmcp::service::ServiceError::UnexpectedResponse);
                }
                let responses =
                    fulfill_mcp_input_requests(&service, requests, cancellation_token.clone())
                        .await?;
                if had_input_requests {
                    state_only_rounds = 0;
                } else {
                    let delay = Duration::from_millis(
                        (50u64.saturating_mul(1_u64 << state_only_rounds.min(3))).min(250),
                    );
                    tokio::select! {
                        biased;
                        _ = cancellation_token.cancelled() => return Err(mcp_tool_call_cancelled()),
                        _ = time::sleep(delay) => {}
                    }
                    state_only_rounds += 1;
                }
                params.input_responses = (!responses.is_empty()).then_some(responses);
                params.request_state = result.request_state;
            }
            _ => return Err(rmcp::service::ServiceError::UnexpectedResponse),
        }
    }

    Err(rmcp::service::ServiceError::InputRequiredRoundsExceeded {
        max_rounds: rmcp::model::DEFAULT_MRTR_MAX_ROUNDS,
    })
}

async fn fulfill_mcp_input_requests(
    service: &Arc<RunningService<RoleClient, ElicitationClientService>>,
    requests: rmcp::model::InputRequests,
    cancellation_token: CancellationToken,
) -> std::result::Result<InputResponses, rmcp::service::ServiceError> {
    let responses = futures::future::try_join_all(requests.into_iter().map(|(key, request)| {
        let service = Arc::clone(service);
        let cancellation_token = cancellation_token.clone();
        async move {
            let response = match request {
                InputRequest::CreateMessage(request) => {
                    let mut request = ServerRequest::CreateMessageRequest(request);
                    let context = mcp_input_request_context(
                        &service,
                        &key,
                        &mut request,
                        cancellation_token.clone(),
                    );
                    let result = tokio::select! {
                        biased;
                        _ = cancellation_token.cancelled() => return Err(mcp_tool_call_cancelled()),
                        result = service.service().handle_request(request, context) => {
                            result.map_err(rmcp::service::ServiceError::McpError)?
                        }
                    };
                    match result {
                        ClientResult::CreateMessageResult(result) => {
                            serde_json::to_value(result).map_err(|error| {
                                rmcp::service::ServiceError::McpError(
                                    rmcp::ErrorData::internal_error(error.to_string(), None),
                                )
                            })?
                        }
                        _ => return Err(rmcp::service::ServiceError::UnexpectedResponse),
                    }
                }
                InputRequest::Elicitation(request) => {
                    let mut request = ServerRequest::ElicitRequest(request);
                    let context = mcp_input_request_context(
                        &service,
                        &key,
                        &mut request,
                        cancellation_token.clone(),
                    );
                    let result = tokio::select! {
                        biased;
                        _ = cancellation_token.cancelled() => return Err(mcp_tool_call_cancelled()),
                        result = service.service().handle_request(request, context) => {
                            result.map_err(rmcp::service::ServiceError::McpError)?
                        }
                    };
                    match result {
                        ClientResult::ElicitResult(result) => serde_json::to_value(result)
                            .map_err(|error| {
                                rmcp::service::ServiceError::McpError(
                                    rmcp::ErrorData::internal_error(error.to_string(), None),
                                )
                            })?,
                        _ => return Err(rmcp::service::ServiceError::UnexpectedResponse),
                    }
                }
                InputRequest::ListRoots(request) => {
                    let mut request = ServerRequest::ListRootsRequest(request);
                    let context = mcp_input_request_context(
                        &service,
                        &key,
                        &mut request,
                        cancellation_token.clone(),
                    );
                    let result = tokio::select! {
                        biased;
                        _ = cancellation_token.cancelled() => return Err(mcp_tool_call_cancelled()),
                        result = service.service().handle_request(request, context) => {
                            result.map_err(rmcp::service::ServiceError::McpError)?
                        }
                    };
                    match result {
                        ClientResult::ListRootsResult(result) => serde_json::to_value(result)
                            .map_err(|error| {
                                rmcp::service::ServiceError::McpError(
                                    rmcp::ErrorData::internal_error(error.to_string(), None),
                                )
                            })?,
                        _ => return Err(rmcp::service::ServiceError::UnexpectedResponse),
                    }
                }
                _ => return Err(rmcp::service::ServiceError::UnexpectedResponse),
            };
            Ok((key, response))
        }
    }))
    .await?;
    Ok(responses.into_iter().collect())
}

fn mcp_input_request_context<T>(
    service: &RunningService<RoleClient, ElicitationClientService>,
    key: &str,
    request: &mut T,
    cancellation_token: CancellationToken,
) -> RequestContext<RoleClient>
where
    T: rmcp::model::GetMeta<Metadata = RequestMetaObject> + rmcp::model::GetExtensions,
{
    let mut meta = RequestMetaObject::default();
    let mut extensions = Extensions::default();
    std::mem::swap(&mut meta, request.get_meta_mut());
    std::mem::swap(&mut extensions, request.extensions_mut());
    let mut context =
        RequestContext::new(RequestId::String(Arc::from(key)), service.peer().clone());
    context.ct = cancellation_token;
    context.meta = meta;
    context.extensions = extensions;
    context
}

fn mcp_tool_call_cancelled() -> rmcp::service::ServiceError {
    rmcp::service::ServiceError::Cancelled {
        reason: Some("MCP tool call cancelled".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pretty_assertions::assert_eq;
    use tokio::time;

    use super::*;

    #[test]
    fn client_operation_timeout_rounds_duration() {
        let error = ClientOperationError::Timeout {
            label: "tools/list".to_string(),
            duration: Duration::from_nanos(29_999_999_875),
        };

        assert_eq!(error.to_string(), "timed out awaiting tools/list after 30s");
    }

    #[tokio::test]
    async fn active_time_timeout_pauses_while_elicitation_is_pending() {
        let pause_state = ElicitationPauseState::new();
        let pause = pause_state.enter();
        tokio::spawn(async move {
            time::sleep(Duration::from_millis(75)).await;
            drop(pause);
        });

        let result =
            active_time_timeout(Duration::from_millis(50), pause_state.subscribe(), async {
                time::sleep(Duration::from_millis(90)).await;
                "done"
            })
            .await;

        assert_eq!(Ok("done"), result);
    }
}
