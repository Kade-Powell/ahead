use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use crate::runtime::McpRuntimeContext;
use codex_config::AppToolApproval;
use codex_config::McpServerConfig;
use codex_config::McpServerTransportConfig;
use codex_exec_server::Environment;
use codex_protocol::mcp::ClientMcpExtensions;
use rmcp::model::ElicitationCapability;

/// MCP server after runtime additions have been applied.
#[derive(Debug, Clone)]
pub struct EffectiveMcpServer {
    config: McpServerConfig,
}

impl EffectiveMcpServer {
    pub fn configured(config: McpServerConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &McpServerConfig {
        &self.config
    }

    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    pub fn required(&self) -> bool {
        self.config.required
    }
}

/// Inputs that determine the identity of a live MCP connection.
///
/// Tool policy and presentation metadata intentionally do not appear here:
/// those belong to a publication and can change without reconnecting.
#[derive(Clone)]
pub(crate) struct McpServerConnectionIdentity {
    transport: McpServerTransportConfig,
    environment_id: String,
    resolved_environment: Result<Arc<Environment>, String>,
    local_stdio_fallback_cwd: Option<PathBuf>,
    referenced_environment_variables: Vec<(String, Option<OsString>)>,
    client_elicitation_capability: ElicitationCapability,
    client_mcp_extensions: ClientMcpExtensions,
}

impl McpServerConnectionIdentity {
    pub(crate) fn new(
        server: &EffectiveMcpServer,
        resolved_environment: &Result<Arc<Environment>, String>,
        runtime_context: &McpRuntimeContext,
        client_elicitation_capability: ElicitationCapability,
        client_mcp_extensions: ClientMcpExtensions,
    ) -> Self {
        let config = server.config();
        let local_stdio_fallback_cwd = (config.is_local_environment()
            && matches!(
                config.transport,
                McpServerTransportConfig::Stdio { cwd: None, .. }
            ))
        .then(|| runtime_context.local_process_cwd());
        let referenced_environment_variables = referenced_environment_variables(config);

        Self {
            transport: config.transport.clone(),
            environment_id: config.environment_id.clone(),
            resolved_environment: resolved_environment.clone(),
            local_stdio_fallback_cwd,
            referenced_environment_variables,
            client_elicitation_capability,
            client_mcp_extensions,
        }
    }

    pub(crate) fn has_same_connection_config(&self, other: &Self) -> bool {
        self.transport == other.transport
            && self.environment_id == other.environment_id
            && same_resolved_environment(&self.resolved_environment, &other.resolved_environment)
            && self.local_stdio_fallback_cwd == other.local_stdio_fallback_cwd
            && self.referenced_environment_variables == other.referenced_environment_variables
            && self.client_elicitation_capability == other.client_elicitation_capability
            && self.client_mcp_extensions == other.client_mcp_extensions
    }
}

impl PartialEq for McpServerConnectionIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.has_same_connection_config(other)
    }
}

fn same_resolved_environment(
    left: &Result<Arc<Environment>, String>,
    right: &Result<Arc<Environment>, String>,
) -> bool {
    match (left, right) {
        (Ok(left), Ok(right)) => Arc::ptr_eq(left, right),
        (Err(left), Err(right)) => left == right,
        (Ok(_), Err(_)) | (Err(_), Ok(_)) => false,
    }
}

fn referenced_environment_variables(config: &McpServerConfig) -> Vec<(String, Option<OsString>)> {
    let McpServerTransportConfig::Stdio { env_vars, .. } = &config.transport;
    let mut names = env_vars
        .iter()
        .map(|env_var| env_var.name().to_string())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            let value = std::env::var_os(&name);
            (name, value)
        })
        .collect()
}

/// Transport origin retained for metrics and diagnostics after server launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum McpServerOrigin {
    Stdio,
}

impl McpServerOrigin {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Stdio => "stdio",
        }
    }
}

/// Semantic metadata that must survive after the server is launched.
#[derive(Debug, Clone)]
pub(crate) struct McpServerMetadata {
    pub environment_id: String,
    pub origin: Option<McpServerOrigin>,
    pub supports_parallel_tool_calls: bool,
    pub default_tools_approval_mode: Option<AppToolApproval>,
    pub tool_approval_modes: HashMap<String, AppToolApproval>,
}

impl McpServerMetadata {
    pub fn tool_approval_mode(&self, tool_name: &str) -> AppToolApproval {
        self.tool_approval_modes
            .get(tool_name)
            .copied()
            .or(self.default_tools_approval_mode)
            .unwrap_or_default()
    }
}

impl From<&EffectiveMcpServer> for McpServerMetadata {
    fn from(server: &EffectiveMcpServer) -> Self {
        let config = server.config();
        Self {
            environment_id: config.environment_id.clone(),
            origin: Some(McpServerOrigin::Stdio),
            supports_parallel_tool_calls: config.supports_parallel_tool_calls,
            default_tools_approval_mode: config.default_tools_approval_mode,
            tool_approval_modes: config
                .tools
                .iter()
                .filter_map(|(name, config)| {
                    config
                        .approval_mode
                        .map(|approval_mode| (name.clone(), approval_mode))
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn referenced_environment_variables_come_from_the_stdio_declaration() {
        let config = McpServerConfig {
            transport: McpServerTransportConfig::Stdio {
                command: "mcp-server".to_string(),
                args: Vec::new(),
                env: None,
                env_vars: vec!["PATH".into(), "NODE_REPL_AUTH_TOKEN".into()],
                cwd: None,
            },
            environment_id: "remote".to_string(),
            enabled: true,
            required: false,
            supports_parallel_tool_calls: false,
            omit_tools_from: None,
            disabled_reason: None,
            startup_timeout_sec: None,
            tool_timeout_sec: None,
            default_tools_approval_mode: None,
            enabled_tools: None,
            disabled_tools: None,
            tools: HashMap::new(),
        };

        assert_eq!(
            referenced_environment_variables(&config),
            vec![
                (
                    "NODE_REPL_AUTH_TOKEN".to_string(),
                    std::env::var_os("NODE_REPL_AUTH_TOKEN"),
                ),
                ("PATH".to_string(), std::env::var_os("PATH")),
            ]
        );
    }
}
