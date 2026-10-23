use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use codex_config::Constrained;
use codex_config::McpServerConfig;
use codex_config::types::AppToolApproval;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::AskForApproval;
use codex_utils_path_uri::PathUri;
use rmcp::model::ElicitationCapability;

use crate::McpProtocolMode;
use crate::ResolvedMcpCatalog;
use crate::server::EffectiveMcpServer;

const MCP_TOOL_NAME_PREFIX: &str = "mcp";
const MCP_TOOL_NAME_DELIMITER: &str = "__";

pub fn qualified_mcp_tool_name_prefix(server_name: &str) -> String {
    sanitize_responses_api_tool_name(&format!(
        "{MCP_TOOL_NAME_PREFIX}{MCP_TOOL_NAME_DELIMITER}{server_name}{MCP_TOOL_NAME_DELIMITER}"
    ))
}

/// Returns true when MCP permission prompts should resolve as approved instead
/// of being shown to the user.
pub fn mcp_permission_prompt_is_auto_approved(
    approval_policy: AskForApproval,
    permission_profile: &PermissionProfile,
    context: McpPermissionPromptAutoApproveContext,
) -> bool {
    if context.tool_approval_mode == Some(AppToolApproval::Approve) {
        return true;
    }

    if approval_policy != AskForApproval::Never {
        return false;
    }

    match permission_profile {
        PermissionProfile::Disabled | PermissionProfile::External { .. } => true,
        PermissionProfile::Managed { file_system, .. } => {
            file_system.to_sandbox_policy().has_full_disk_write_access()
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct McpPermissionPromptAutoApproveContext {
    pub tool_approval_mode: Option<AppToolApproval>,
}

/// MCP runtime settings derived from `codex_core::config::Config`.
///
/// Each published runtime and prepared call owns one immutable copy of these
/// settings, so its connection, approval policy, and sandbox authority cannot
/// change independently.
#[derive(Debug, Clone)]
pub struct McpConfig {
    /// How long a tool catalog capture waits for optional MCP servers to initialize.
    ///
    /// A zero duration disables the shared grace and waits for each server's
    /// configured startup timeout instead.
    pub optional_mcp_startup_grace: Duration,
    /// Approval policy used for MCP tool calls and MCP elicitation requests.
    pub approval_policy: Constrained<AskForApproval>,
    /// Permission profile captured with the connections and approval policy.
    pub permission_profile: PermissionProfile,
    /// Working directories for the exact environment handles used by this runtime.
    pub environment_cwds: HashMap<String, PathUri>,
    /// Explicit server permissions; unresolved or unavailable servers have no entry.
    pub server_permission_profiles: HashMap<String, PermissionProfile>,
    /// Optional path to `codex-linux-sandbox` for sandboxed MCP tool execution.
    pub ahead_linux_sandbox_exe: Option<PathBuf>,
    /// Whether to use legacy Landlock behavior in the MCP sandbox state.
    // TODO(anp): Reconcile this runtime-wide copy with TurnEnvironment::sandbox_context
    // for the environment that owns each MCP server.
    pub use_legacy_landlock: bool,
    /// Whether model-visible MCP tool namespaces should keep the legacy
    /// `mcp__` prefix.
    pub prefix_mcp_tool_names: bool,
    /// MCP servers whose model-visible tool namespaces omit the `mcp__` prefix.
    pub non_prefixed_mcp_tool_servers: Vec<String>,
    /// Protocol compatibility policy captured when this MCP configuration is created.
    pub protocol_mode: McpProtocolMode,
    /// Client-side elicitation capabilities advertised during MCP initialization.
    pub client_elicitation_capability: ElicitationCapability,
    /// Resolved MCP registrations keyed by logical server name.
    pub mcp_server_catalog: ResolvedMcpCatalog,
}

/// Default amount of time a tool catalog capture waits for optional MCP servers.
pub const DEFAULT_OPTIONAL_MCP_STARTUP_GRACE: Duration = Duration::from_secs(1);

impl McpConfig {
    /// Resolves enabled runtime servers against the exact attachment permissions being published.
    pub fn set_server_permission_profiles(
        &mut self,
        servers: &HashMap<String, EffectiveMcpServer>,
        environment_profiles: impl IntoIterator<Item = (String, PermissionProfile)>,
    ) {
        let environment_profiles = environment_profiles.into_iter().collect::<HashMap<_, _>>();
        self.server_permission_profiles = servers
            .iter()
            .filter(|(_, server)| server.enabled())
            .filter_map(|(server_name, _)| {
                let server = self.mcp_server_catalog.server(server_name)?;
                let permission_profile = if let Some(permission_profile) =
                    environment_profiles.get(&server.config().environment_id)
                {
                    permission_profile
                } else if server.config().is_local_environment() {
                    &self.permission_profile
                } else {
                    return None;
                };
                Some((server_name.clone(), permission_profile.clone()))
            })
            .collect();
    }

    /// Returns this server's published permission authority.
    pub fn permission_profile_for_server(&self, server_name: &str) -> Option<&PermissionProfile> {
        self.server_permission_profiles.get(server_name)
    }
}

pub fn configured_mcp_servers(config: &McpConfig) -> HashMap<String, McpServerConfig> {
    config.mcp_server_catalog.configured_servers()
}

pub fn effective_mcp_servers(config: &McpConfig) -> HashMap<String, EffectiveMcpServer> {
    configured_mcp_servers(config)
        .into_iter()
        .map(|(name, server)| (name, EffectiveMcpServer::configured(server)))
        .collect()
}

/// The Responses API requires tool names to match `^[a-zA-Z0-9_-]+$`.
/// MCP server/tool names are user-controlled, so sanitize the fully-qualified
/// name we expose to the model by replacing any disallowed character with `_`.
pub(crate) fn sanitize_responses_api_tool_name(name: &str) -> String {
    let mut sanitized = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            sanitized.push(c);
        } else {
            sanitized.push('_');
        }
    }

    if sanitized.is_empty() {
        "_".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
pub(crate) mod tests;
