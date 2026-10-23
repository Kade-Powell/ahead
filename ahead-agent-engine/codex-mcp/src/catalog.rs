use std::collections::BTreeMap;
use std::collections::HashMap;

use codex_config::McpServerConfig;
use codex_config::McpServerDisabledReason;
use codex_config::RequirementSource;
use codex_protocol::mcp_policy::EnvironmentMcpPolicy;

/// One named MCP server declaration from effective AHEAD configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct McpServerRegistration {
    name: String,
    config: McpServerConfig,
}

impl McpServerRegistration {
    pub fn from_config(name: String, config: McpServerConfig) -> Self {
        Self { name, config }
    }
}

/// The authority available for MCP servers running in one environment.
#[derive(Clone, Copy, Debug)]
pub enum McpEnvironmentAuthority<'a> {
    /// The selected environment adds no restrictions to the controller policy.
    Unrestricted,
    /// The owner supplied the final restrictions for this environment.
    Restricted(&'a EnvironmentMcpPolicy),
    /// The attachment is pending or failed, so its owner policy is not available.
    Unavailable,
}

/// Mutable inputs used to produce an immutable resolved catalog.
#[derive(Clone, Debug, Default)]
pub struct McpCatalogBuilder {
    registrations: Vec<McpServerRegistration>,
}

impl McpCatalogBuilder {
    pub fn register(&mut self, registration: McpServerRegistration) {
        self.registrations.push(registration);
    }

    /// Applies environment authority before resolving immutable server registrations.
    pub fn build_with_environment_authority<'a>(
        mut self,
        mut authority_for_environment: impl FnMut(&str) -> McpEnvironmentAuthority<'a>,
    ) -> ResolvedMcpCatalog {
        for registration in &mut self.registrations {
            if !registration.config.enabled {
                continue;
            }

            let allowed = match authority_for_environment(&registration.config.environment_id) {
                McpEnvironmentAuthority::Unrestricted => true,
                McpEnvironmentAuthority::Unavailable => false,
                McpEnvironmentAuthority::Restricted(policy) => {
                    policy.servers.as_ref().is_none_or(|requirements| {
                        requirements
                            .get(&registration.name)
                            .is_some_and(|requirement| {
                                registration.config.matches_requirement(requirement)
                            })
                    })
                }
            };

            if !allowed {
                registration.config.enabled = false;
                registration.config.disabled_reason = Some(McpServerDisabledReason::Requirements {
                    source: RequirementSource::Unknown,
                });
            }
        }
        self.build()
    }

    pub fn build(self) -> ResolvedMcpCatalog {
        let servers = self
            .registrations
            .into_iter()
            .map(|registration| {
                (
                    registration.name,
                    ResolvedMcpServer {
                        config: registration.config,
                    },
                )
            })
            .collect();
        ResolvedMcpCatalog { servers }
    }
}

/// A single configured MCP server.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedMcpServer {
    config: McpServerConfig,
}

impl ResolvedMcpServer {
    pub fn config(&self) -> &McpServerConfig {
        &self.config
    }
}

/// Immutable MCP server catalog after environment policy is applied.
#[derive(Clone, Debug, Default)]
pub struct ResolvedMcpCatalog {
    servers: BTreeMap<String, ResolvedMcpServer>,
}

impl ResolvedMcpCatalog {
    pub fn builder() -> McpCatalogBuilder {
        McpCatalogBuilder::default()
    }

    pub fn to_builder(&self) -> McpCatalogBuilder {
        McpCatalogBuilder {
            registrations: self
                .servers
                .iter()
                .map(|(name, server)| McpServerRegistration {
                    name: name.clone(),
                    config: server.config.clone(),
                })
                .collect(),
        }
    }

    pub fn server(&self, name: &str) -> Option<&ResolvedMcpServer> {
        self.servers.get(name)
    }

    pub fn configured_servers(&self) -> HashMap<String, McpServerConfig> {
        self.servers
            .iter()
            .map(|(name, server)| (name.clone(), server.config.clone()))
            .collect()
    }
}
