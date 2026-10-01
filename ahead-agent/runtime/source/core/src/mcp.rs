use std::collections::HashMap;

use crate::config::Config;
use crate::environment_selection::ThreadEnvironments;
use codex_config::DEFAULT_MCP_SERVER_ENVIRONMENT_ID;
use codex_config::McpServerConfig;
use codex_mcp::EffectiveMcpServer;
use codex_mcp::McpConfig;
use codex_mcp::McpEnvironmentAuthority;
use codex_mcp::McpToolCatalogCache;
use codex_mcp::configured_mcp_servers;
use codex_mcp::effective_mcp_servers;
use codex_protocol::protocol::EnvironmentConfigState;
use codex_protocol::protocol::TurnEnvironmentSelection;

/// MCP configuration and capability availability derived from the same inputs.
#[derive(Clone)]
pub(crate) struct McpRuntimeProjection {
    pub(crate) config: McpConfig,
}

pub(crate) enum McpEnvironmentScope<'a> {
    /// Controller-level operations without an associated thread.
    HostOnly,
    /// Initial thread selections before the live environment store exists.
    Initial(&'a [TurnEnvironmentSelection]),
    /// Current attachment state for an existing thread.
    Live(&'a ThreadEnvironments),
}

#[derive(Clone)]
pub struct McpManager {
    tool_catalog_cache: McpToolCatalogCache,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            tool_catalog_cache: McpToolCatalogCache::default(),
        }
    }

    pub fn tool_catalog_cache(&self) -> McpToolCatalogCache {
        self.tool_catalog_cache.clone()
    }

    /// Returns the MCP config for threadless operations.
    pub async fn runtime_config(&self, config: &Config) -> McpConfig {
        self.runtime_config_for_step(config, McpEnvironmentScope::HostOnly)
            .await
            .config
    }

    #[tracing::instrument(name = "mcp.runtime_config.project_for_step", skip_all)]
    pub(crate) async fn runtime_config_for_step(
        &self,
        config: &Config,
        environment_scope: McpEnvironmentScope<'_>,
    ) -> McpRuntimeProjection {
        let mut mcp_config = config.to_ahead_mcp_config();
        let catalog = mcp_config.mcp_server_catalog.to_builder();
        let selections = match environment_scope {
            McpEnvironmentScope::HostOnly => None,
            McpEnvironmentScope::Initial(selections) => Some(selections.to_vec()),
            McpEnvironmentScope::Live(environments) => Some(environments.selections()),
        };
        let catalog = catalog.build_with_environment_authority(|environment_id| {
            let Some(selections) = selections.as_ref() else {
                return McpEnvironmentAuthority::Unrestricted;
            };
            let Some(selection) = selections
                .iter()
                .find(|selection| selection.environment_id == environment_id)
            else {
                return if environment_id == DEFAULT_MCP_SERVER_ENVIRONMENT_ID {
                    McpEnvironmentAuthority::Unrestricted
                } else {
                    McpEnvironmentAuthority::Unavailable
                };
            };

            match &selection.config {
                EnvironmentConfigState::FromThread => McpEnvironmentAuthority::Unrestricted,
                EnvironmentConfigState::Pending | EnvironmentConfigState::Failed(_) => {
                    McpEnvironmentAuthority::Unavailable
                }
                EnvironmentConfigState::Ready(config) => config
                    .mcp_policy
                    .as_ref()
                    .map_or(McpEnvironmentAuthority::Unrestricted, |policy| {
                        McpEnvironmentAuthority::Restricted(policy)
                    }),
            }
        });
        for conflict in catalog.conflicts() {
            tracing::warn!(
                server = conflict.name,
                outcome = ?conflict.outcome,
                contenders = ?conflict.contenders,
                "conflicting MCP server actions; using resolved catalog outcome"
            );
        }
        mcp_config.mcp_server_catalog = catalog;
        McpRuntimeProjection { config: mcp_config }
    }

    /// Returns servers declared by the effective configuration.
    pub async fn configured_servers(&self, config: &Config) -> HashMap<String, McpServerConfig> {
        let mcp_config = config.to_ahead_mcp_config();
        configured_mcp_servers(&mcp_config)
    }

    /// Returns servers declared by the runtime configuration.
    pub async fn runtime_servers(&self, config: &Config) -> HashMap<String, McpServerConfig> {
        let mcp_config = self.runtime_config(config).await;
        configured_mcp_servers(&mcp_config)
    }

    /// Returns the runtime view of configured servers.
    pub async fn effective_servers(&self, config: &Config) -> HashMap<String, EffectiveMcpServer> {
        let mcp_config = self.runtime_config(config).await;
        effective_mcp_servers(&mcp_config)
    }
}
