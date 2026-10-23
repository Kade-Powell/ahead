use anyhow::Result;
use async_channel::Sender;
use codex_config::McpServerConfig;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::McpStartupUpdateEvent;

use crate::rmcp_client::DEFAULT_STARTUP_TIMEOUT;
use crate::rmcp_client::StartupOutcomeError;

pub(super) async fn emit_update(
    submit_id: &str,
    tx_event: &Sender<Event>,
    update: McpStartupUpdateEvent,
) -> Result<(), async_channel::SendError<Event>> {
    tx_event
        .send(Event {
            id: submit_id.to_string(),
            msg: EventMsg::McpStartupUpdate(update),
        })
        .await
}

pub(super) fn mcp_init_error_display(
    server_name: &str,
    config: Option<&McpServerConfig>,
    error: &StartupOutcomeError,
) -> String {
    let server_key = if !server_name.is_empty()
        && server_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        server_name.to_string()
    } else {
        serde_json::Value::String(server_name.to_string()).to_string()
    };
    if matches!(
        error,
        StartupOutcomeError::Failed { error }
            if error.contains("request timed out")
                || error.contains("timed out handshaking with MCP server")
                || error.contains("MCP client startup timed out")
    ) {
        let startup_timeout_secs = config
            .and_then(|config| config.startup_timeout_sec)
            .unwrap_or(DEFAULT_STARTUP_TIMEOUT)
            .as_secs();
        format!(
            "MCP client for `{server_name}` timed out after {startup_timeout_secs} seconds. Add or adjust `startup_timeout_sec` in `.ahead/config.toml` under `[mcp.servers.{server_key}]`."
        )
    } else {
        format!("MCP client for `{server_name}` failed to start: {error:#}")
    }
}
