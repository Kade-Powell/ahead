//! RMCP client lifecycle for MCP server connections.
//!
//! This module owns startup of individual RMCP clients: building the transport,
//! initializing the server, listing raw tools, applying per-server tool filters,
//! and exposing per-server tools while a client is still connecting.
//! Higher-level aggregation and resource/tool APIs live in
//! [`crate::connection_manager`].

#[path = "rmcp_client/status.rs"]
mod status;

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use crate::elicitation::ElicitationRequestManager;
use crate::pagination::collect_paginated_with_limit;
use crate::runtime::McpRuntimeContext;
use crate::runtime::emit_duration;
use crate::server::EffectiveMcpServer;
use crate::tool_catalog_cache::McpToolCatalogCacheContext;
use crate::tool_catalog_cache::McpToolCatalogFetchTicket;
use crate::tools::ToolInfo;
use anyhow::Result;
use anyhow::anyhow;
use async_channel::Sender;
use codex_async_utils::CancelErr;
use codex_async_utils::OrCancelExt;
use codex_config::McpServerConfig;
use codex_config::McpServerTransportConfig;
use codex_protocol::mcp::ClientMcpExtensions;
use codex_protocol::protocol::Event;
use codex_rmcp_client::ExecutorStdioServerLauncher;
use codex_rmcp_client::LocalStdioServerLauncher;
use codex_rmcp_client::McpProtocolMode;
use codex_rmcp_client::RmcpClient;
use codex_rmcp_client::StdioServerLauncher;
use futures::future::BoxFuture;
use futures::future::FutureExt;
use futures::future::Shared;
use rmcp::model::ClientCapabilities;
use rmcp::model::ElicitationCapability;
use rmcp::model::Implementation;
use rmcp::model::InitializeRequestParams;
use rmcp::model::ProtocolVersion;
use rmcp::model::ServerPeerInfo;
use rmcp::model::Tool as RmcpTool;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;
use tracing::instrument;
use tracing::warn;

/// MCP server capability indicating that Codex should include [`SandboxState`]
/// in tool-call request `_meta` under this key.
pub const MCP_SANDBOX_STATE_META_CAPABILITY: &str = "codex/sandbox-state-meta";
/// Experimental MCP server capability for development and testing only; production servers should
/// not use it. Its `cacheable: false` property disables sharing tool definitions across connections.
const MCP_TOOL_CATALOG_CACHE_CAPABILITY: &str = "codex/tool-catalog-cache";
const MCP_TOOL_CATALOG_CACHEABLE_PROPERTY: &str = "cacheable";
pub(crate) const MCP_TOOLS_LIST_DURATION_METRIC: &str = "codex.mcp.tools.list.duration_ms";
pub(crate) const MCP_TOOLS_FETCH_UNCACHED_DURATION_METRIC: &str =
    "codex.mcp.tools.fetch_uncached.duration_ms";
pub(crate) const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(300);

const UNTRUSTED_CONNECTOR_META_KEYS: &[&str] = &[
    "connector_id",
    "connector_name",
    "connector_display_name",
    "connector_description",
    "connectorDescription",
];

#[derive(Clone)]
pub(crate) struct ManagedClient {
    pub(crate) client: Arc<RmcpClient>,
    pub(crate) tools: Vec<ToolInfo>,
    pub(crate) tool_timeout: Option<Duration>,
    pub(crate) server_supports_sandbox_state_meta_capability: bool,
}

pub(crate) type ManagedClientFuture =
    Shared<BoxFuture<'static, Result<ManagedClient, StartupOutcomeError>>>;

#[derive(Clone)]
struct ManagedClientStartup {
    server_name: String,
    server: EffectiveMcpServer,
    tx_event: Option<Sender<Event>>,
    elicitation_requests: ElicitationRequestManager,
    tool_catalog_cache_context: Option<McpToolCatalogCacheContext>,
    runtime_context: McpRuntimeContext,
    resolved_environment: std::result::Result<Arc<codex_exec_server::Environment>, String>,
    client_elicitation_capability: ElicitationCapability,
    client_mcp_extensions: ClientMcpExtensions,
    protocol_mode: McpProtocolMode,
    catalog_item_limit: usize,
    cancel_token: CancellationToken,
    startup_complete: Arc<AtomicBool>,
}

impl ManagedClientStartup {
    fn start(&self) -> ManagedClientFuture {
        let Self {
            server_name,
            server,
            tx_event,
            elicitation_requests,
            tool_catalog_cache_context,
            runtime_context,
            resolved_environment,
            client_elicitation_capability,
            client_mcp_extensions,
            protocol_mode,
            catalog_item_limit,
            cancel_token,
            startup_complete,
        } = self.clone();
        let startup_timeout = server
            .config()
            .startup_timeout_sec
            .unwrap_or(DEFAULT_STARTUP_TIMEOUT);
        let cancel_token_for_fut = cancel_token;
        async move {
            let tool_catalog_fetch_ticket = tool_catalog_cache_context
                .as_ref()
                .map(McpToolCatalogCacheContext::begin_fetch);
            let outcome = match async {
                if let Err(error) = validate_mcp_server_name(&server_name) {
                    return Err(error.into());
                }

                let client = match tokio::time::timeout(
                    startup_timeout,
                    make_rmcp_client(
                        &server_name,
                        server.clone(),
                        runtime_context,
                        resolved_environment,
                        protocol_mode,
                    ),
                )
                .await
                {
                    Ok(result) => Arc::new(result?),
                    Err(_) => {
                        return Err(StartupOutcomeError::from(anyhow!(
                            "MCP client startup timed out after {startup_timeout:?}"
                        )));
                    }
                };
                start_server_task(
                    server_name,
                    client,
                    StartServerTaskParams {
                        startup_timeout: Some(startup_timeout),
                        tx_event,
                        elicitation_requests,
                        tool_catalog_cache_context,
                        tool_catalog_fetch_ticket,
                        client_elicitation_capability,
                        client_mcp_extensions,
                        catalog_item_limit,
                    },
                )
                .await
            }
            .or_cancel(&cancel_token_for_fut)
            .await
            {
                Ok(result) => result,
                Err(CancelErr::Cancelled) => Err(StartupOutcomeError::Cancelled),
            };
            startup_complete.store(true, Ordering::Release);
            outcome
        }
        .in_current_span()
        .boxed()
        .shared()
    }
}

#[derive(Clone)]
pub(crate) struct AsyncManagedClient {
    pub(crate) client: ManagedClientFuture,
    pub(crate) tool_catalog_cache_context: Option<McpToolCatalogCacheContext>,
    pub(crate) startup_complete: Arc<AtomicBool>,
    pub(crate) cancel_token: CancellationToken,
}

impl AsyncManagedClient {
    // Keep this constructor flat so the startup inputs remain readable at the
    // single call site instead of introducing a one-off params wrapper.
    #[instrument(level = "trace", skip_all, fields(server_name = %server_name))]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        server_name: String,
        server: EffectiveMcpServer,
        cancel_token: CancellationToken,
        tx_event: Option<Sender<Event>>,
        elicitation_requests: ElicitationRequestManager,
        tool_catalog_cache_context: Option<McpToolCatalogCacheContext>,
        runtime_context: McpRuntimeContext,
        resolved_environment: std::result::Result<Arc<codex_exec_server::Environment>, String>,
        client_elicitation_capability: ElicitationCapability,
        client_mcp_extensions: ClientMcpExtensions,
        protocol_mode: McpProtocolMode,
        catalog_item_limit: usize,
    ) -> Self {
        let startup_complete = Arc::new(AtomicBool::new(false));
        let startup = Arc::new(ManagedClientStartup {
            server_name,
            server,
            tx_event,
            elicitation_requests,
            tool_catalog_cache_context: tool_catalog_cache_context.clone(),
            runtime_context,
            resolved_environment,
            client_elicitation_capability,
            client_mcp_extensions,
            protocol_mode,
            catalog_item_limit,
            cancel_token: cancel_token.clone(),
            startup_complete: Arc::clone(&startup_complete),
        });
        let client = startup.start();
        Self {
            client,
            tool_catalog_cache_context,
            startup_complete,
            cancel_token,
        }
    }

    pub(crate) async fn client(&self) -> Result<ManagedClient, StartupOutcomeError> {
        self.client.clone().await
    }

    /// Returns the current ready client, including its tool catalog and metadata,
    /// without waiting for startup or initiating a reconnection.
    pub(crate) fn ready_client(&self) -> Option<ManagedClient> {
        self.client
            .peek()
            .and_then(|result| result.as_ref().ok())
            .cloned()
    }

    pub(crate) fn ready_transport(&self) -> Option<Arc<RmcpClient>> {
        self.ready_client().map(|client| client.client)
    }

    pub(crate) async fn shutdown(&self) {
        self.cancel_token.cancel();
        match self.client().await {
            Ok(client) => client.client.shutdown().await,
            Err(StartupOutcomeError::Cancelled) => {}
            Err(error) => {
                warn!("failed to initialize MCP client during shutdown: {error:#}");
            }
        }
    }

    pub(crate) fn has_cached_tools(&self) -> bool {
        self.tool_catalog_cache_context
            .as_ref()
            .is_some_and(McpToolCatalogCacheContext::has_tools)
    }

    pub(crate) fn cached_tools(&self) -> Option<Vec<ToolInfo>> {
        self.cached_tools_or(/*fallback*/ None)
    }

    pub(crate) fn cached_tools_or(&self, fallback: Option<Vec<ToolInfo>>) -> Option<Vec<ToolInfo>> {
        self.tool_catalog_cache_context
            .as_ref()
            .and_then(|cache| cache.current_tools_or(fallback))
    }

    pub(crate) async fn listed_tools(&self) -> Option<Vec<ToolInfo>> {
        if !self.startup_complete.load(Ordering::Acquire)
            && let Some(startup_tools) = self.cached_tools()
        {
            Some(startup_tools)
        } else {
            match self.client().await {
                Ok(client) => Some(client.tools),
                Err(_) => None,
            }
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub(crate) enum StartupOutcomeError {
    #[error("MCP startup cancelled")]
    Cancelled,
    // We can't store the original error here because anyhow::Error doesn't implement
    // `Clone`.
    #[error("MCP startup failed: {error}")]
    Failed { error: String },
}

impl From<anyhow::Error> for StartupOutcomeError {
    fn from(error: anyhow::Error) -> Self {
        Self::Failed {
            error: error.to_string(),
        }
    }
}

#[instrument(level = "trace", skip_all, fields(server_name = %server_name))]
pub(crate) async fn list_tools_for_client_uncached(
    server_name: &str,
    client: &Arc<RmcpClient>,
    timeout: Option<Duration>,
    catalog_item_limit: usize,
    server_instructions: Option<&str>,
) -> Result<Vec<ToolInfo>> {
    let fetch_start = Instant::now();
    let protocol_mode = client.protocol_mode();
    let tools = collect_paginated_with_limit("tools/list", timeout, catalog_item_limit, |params| {
        let client = Arc::clone(client);
        async move {
            let response = client.list_tools(params, timeout).await?;
            let next_cursor = match protocol_mode {
                McpProtocolMode::Legacy => None,
                McpProtocolMode::V20260728 => response.next_cursor,
            };
            Ok((response.tools, next_cursor))
        }
    })
    .await?
    .into_iter()
    .map(|tool| tool_info_from_listed_tool(server_name, server_instructions, tool))
    .collect();
    emit_duration(
        MCP_TOOLS_FETCH_UNCACHED_DURATION_METRIC,
        fetch_start.elapsed(),
        &[],
    );
    Ok(tools)
}

/// Keep ordinary MCP tool names and strip untrusted connector metadata.
fn tool_info_from_listed_tool(
    server_name: &str,
    server_instructions: Option<&str>,
    tool: RmcpTool,
) -> ToolInfo {
    let mut tool_def = tool;
    strip_untrusted_connector_meta(&mut tool_def);
    ToolInfo {
        server_name: server_name.to_owned(),
        supports_parallel_tool_calls: false,
        server_origin: None,
        callable_name: tool_def.name.to_string(),
        callable_namespace: server_name.to_string(),
        namespace_description: server_instructions.map(str::to_string),
        tool: tool_def,
    }
}

fn strip_untrusted_connector_meta(tool: &mut RmcpTool) {
    if let Some(meta) = tool.meta.as_mut() {
        meta.retain(|key, _| !is_untrusted_connector_meta_key(key));
    }
}

fn is_untrusted_connector_meta_key(key: &str) -> bool {
    UNTRUSTED_CONNECTOR_META_KEYS.contains(&key)
}

fn validate_mcp_server_name(server_name: &str) -> Result<()> {
    let re = regex_lite::Regex::new(r"^[a-zA-Z0-9_:@/.-]+$")?;
    if !re.is_match(server_name) {
        return Err(anyhow!(
            "Invalid MCP server name '{server_name}': must match pattern {pattern}",
            pattern = re.as_str()
        ));
    }
    Ok(())
}

#[instrument(level = "trace", skip_all, fields(server_name = %server_name))]
async fn start_server_task(
    server_name: String,
    client: Arc<RmcpClient>,
    params: StartServerTaskParams,
) -> Result<ManagedClient, StartupOutcomeError> {
    let StartServerTaskParams {
        startup_timeout,
        tx_event,
        elicitation_requests,
        tool_catalog_cache_context,
        tool_catalog_fetch_ticket,
        client_elicitation_capability,
        client_mcp_extensions,
        catalog_item_limit,
    } = params;
    let params =
        mcp_initialize_request_params(client_elicitation_capability, client_mcp_extensions);
    let send_elicitation = elicitation_requests.make_sender(server_name.clone(), tx_event);

    let started_at = Instant::now();
    let initialize_result = client
        .initialize(params, startup_timeout, send_elicitation)
        .await;
    record_protocol_discovery_metrics(client.protocol_mode(), started_at, &initialize_result);
    let initialize_result = initialize_result.map_err(StartupOutcomeError::from)?;

    let server_disables_tool_catalog_cache = initialize_result
        .capabilities
        .experimental
        .as_ref()
        .and_then(|experimental| experimental.get(MCP_TOOL_CATALOG_CACHE_CAPABILITY))
        .and_then(|capability| capability.get(MCP_TOOL_CATALOG_CACHEABLE_PROPERTY))
        .and_then(serde_json::Value::as_bool)
        == Some(false);
    if server_disables_tool_catalog_cache
        && let Some(cache_context) = tool_catalog_cache_context.as_ref()
    {
        cache_context.disable();
    }
    let server_supports_sandbox_state_meta_capability = initialize_result
        .capabilities
        .experimental
        .as_ref()
        .and_then(|exp| exp.get(MCP_SANDBOX_STATE_META_CAPABILITY))
        .is_some();
    let list_start = Instant::now();
    let client_tools = list_tools_for_client_uncached(
        &server_name,
        &client,
        startup_timeout,
        catalog_item_limit,
        initialize_result.instructions.as_deref(),
    )
    .await
    .map_err(StartupOutcomeError::from)?;
    if let (Some(cache_context), Some(fetch_ticket)) = (
        tool_catalog_cache_context.as_ref(),
        tool_catalog_fetch_ticket,
    ) {
        cache_context.publish_if_newest(fetch_ticket, &client_tools);
    }
    if tool_catalog_cache_context.is_some() {
        emit_duration(
            MCP_TOOLS_LIST_DURATION_METRIC,
            list_start.elapsed(),
            &[("cache", "miss")],
        );
    }
    let managed = ManagedClient {
        client: Arc::clone(&client),
        tools: client_tools,
        tool_timeout: None,
        server_supports_sandbox_state_meta_capability,
    };

    Ok(managed)
}

fn record_protocol_discovery_metrics(
    mode: McpProtocolMode,
    started_at: Instant,
    result: &Result<ServerPeerInfo>,
) {
    let Some(metrics) = codex_otel::global() else {
        return;
    };

    let mode = match mode {
        McpProtocolMode::Legacy => "legacy",
        McpProtocolMode::V20260728 => "auto",
    };
    let outcome = match result {
        Ok(result) if result.protocol_version == ProtocolVersion::V_2026_07_28 => "modern",
        Ok(_) => "legacy",
        Err(_) => "failure",
    };
    let tags = [("mode", mode), ("outcome", outcome)];
    let _ = metrics.counter("codex.mcp.protocol_discovery", /*inc*/ 1, &tags);
    let _ = metrics.record_duration(
        "codex.mcp.protocol_discovery.duration_ms",
        started_at.elapsed(),
        &tags,
    );
}

fn mcp_initialize_request_params(
    client_elicitation_capability: ElicitationCapability,
    client_mcp_extensions: ClientMcpExtensions,
) -> InitializeRequestParams {
    let mut capabilities = ClientCapabilities::default();
    capabilities.elicitation = Some(client_elicitation_capability);
    let extensions = client_mcp_extensions
        .iter()
        .filter_map(|(id, settings)| {
            settings
                .as_object()
                .cloned()
                .map(|settings| (id.to_string(), settings))
        })
        .collect::<BTreeMap<_, _>>();
    if !extensions.is_empty() {
        capabilities.extensions = Some(extensions);
    }
    InitializeRequestParams::new(
        capabilities,
        Implementation::new("AHEAD", env!("CARGO_PKG_VERSION")).with_title("AHEAD"),
    )
    .with_protocol_version(ProtocolVersion::V_2025_06_18)
}

struct StartServerTaskParams {
    startup_timeout: Option<Duration>, // TODO: cancel_token should handle this.
    tx_event: Option<Sender<Event>>,
    elicitation_requests: ElicitationRequestManager,
    tool_catalog_cache_context: Option<McpToolCatalogCacheContext>,
    tool_catalog_fetch_ticket: Option<McpToolCatalogFetchTicket>,
    client_elicitation_capability: ElicitationCapability,
    client_mcp_extensions: ClientMcpExtensions,
    catalog_item_limit: usize,
}

#[allow(clippy::too_many_arguments)]
#[instrument(level = "trace", skip_all, fields(server_name = %server_name))]
async fn make_rmcp_client(
    server_name: &str,
    server: EffectiveMcpServer,
    runtime_context: McpRuntimeContext,
    resolved_environment: std::result::Result<Arc<codex_exec_server::Environment>, String>,
    protocol_mode: McpProtocolMode,
) -> Result<RmcpClient, StartupOutcomeError> {
    let config = server.config().clone();
    let resolved_environment =
        resolved_environment.map_err(|err| StartupOutcomeError::from(anyhow!(err)))?;
    let is_local_environment = config.is_local_environment();
    let McpServerConfig { transport, .. } = config;
    let McpServerTransportConfig::Stdio {
        command,
        args,
        env,
        env_vars,
        cwd,
    } = transport;
    let command_os: OsString = command.into();
    let args_os: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let env_os = env.map(|env| {
        env.into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect::<HashMap<_, _>>()
    });
    let launcher = if is_local_environment {
        // TODO(starr): Unify local stdio MCP launch with
        // `ExecutorStdioServerLauncher` once the executor-backed path
        // preserves `LocalStdioServerLauncher` semantics.
        Arc::new(LocalStdioServerLauncher::new(
            runtime_context.local_process_cwd(),
        )) as Arc<dyn StdioServerLauncher>
    } else {
        Arc::new(ExecutorStdioServerLauncher::new(
            resolved_environment.get_exec_backend(),
        )) as Arc<dyn StdioServerLauncher>
    };

    let cwd = cwd.map(codex_utils_path_uri::LegacyAppPathString::into_string);
    RmcpClient::new_stdio_client_with_protocol_mode(
        command_os,
        args_os,
        env_os,
        &env_vars,
        cwd,
        launcher,
        protocol_mode,
    )
    .await
    .map_err(|err| StartupOutcomeError::from(anyhow!(err)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::mcp::MCP_APP_UI_EXTENSION_ID;
    use pretty_assertions::assert_eq;
    use rmcp::model::JsonObject;
    use rmcp::model::MetaObject;
    #[test]
    fn mcp_initialize_advertises_client_extensions() {
        let unsupported = mcp_initialize_request_params(
            ElicitationCapability::default(),
            ClientMcpExtensions::default(),
        );
        assert_eq!(unsupported.capabilities.extensions, None);

        let app_ui = serde_json::json!({
            "mimeTypes": ["text/html;profile=mcp-app"],
            "futureField": {"preserved": true},
        });
        let supported = mcp_initialize_request_params(
            ElicitationCapability::default(),
            ClientMcpExtensions::new([
                ("example/form".to_string(), serde_json::json!({})),
                (MCP_APP_UI_EXTENSION_ID.to_string(), app_ui.clone()),
            ]),
        );
        assert_eq!(
            supported.capabilities.extensions,
            Some(BTreeMap::from([
                ("example/form".to_string(), JsonObject::new()),
                (
                    MCP_APP_UI_EXTENSION_ID.to_string(),
                    app_ui.as_object().cloned().expect("app UI settings"),
                ),
            ]))
        );
    }

    fn tool_with_connector_meta() -> RmcpTool {
        RmcpTool::new(
            "capture_file_upload",
            "test tool",
            Arc::new(JsonObject::default()),
        )
        .with_meta(MetaObject(
            serde_json::json!({
                "connector_id": "connector_gmail",
                "connector_name": "Gmail",
                "connector_display_name": "Gmail",
                "connector_description": "Mail connector",
                "connectorDescription": "Mail connector",
                "connectorFutureField": "future connector metadata",
                "CONNECTOR_UPPERCASE": "uppercase connector metadata",
                "openai/fileParams": ["file"],
                "custom": "kept"
            })
            .as_object()
            .expect("object")
            .clone(),
        ))
    }

    #[test]
    fn custom_mcp_connector_metadata_is_stripped() {
        let mut tool = tool_with_connector_meta();

        strip_untrusted_connector_meta(&mut tool);

        let meta = tool.meta.as_ref().expect("meta");
        for key in [
            "connector_id",
            "connector_name",
            "connector_display_name",
            "connector_description",
            "connectorDescription",
        ] {
            assert!(!meta.0.contains_key(key), "{key} should be stripped");
        }
        assert!(meta.0.contains_key("connectorFutureField"));
        assert!(meta.0.contains_key("CONNECTOR_UPPERCASE"));
        assert!(meta.0.contains_key("openai/fileParams"));
        assert_eq!(
            meta.0.get("custom").and_then(|value| value.as_str()),
            Some("kept")
        );
    }
}
