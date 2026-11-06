//! MCP server configuration types.

use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroUsize;
use std::time::Duration;

use codex_protocol::config_types::ToolExposureSurface;
use codex_utils_path_uri::LegacyAppPathString;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::de::Error as SerdeError;

use crate::RequirementSource;

/// Effective MCP environment id when config omits `environment_id`.
pub const DEFAULT_MCP_SERVER_ENVIRONMENT_ID: &str = "local";

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AppToolApproval {
    #[default]
    Auto,
    Prompt,
    Writes,
    Approve,
}

impl AppToolApproval {
    /// Requires approval whenever either policy could require it.
    ///
    /// `Auto` and `Writes` are incomparable: each can require approval for a
    /// tool the other would approve. Their conservative intersection is `Prompt`.
    pub fn restrict_to(self, requested: Self) -> Self {
        match (self, requested) {
            (Self::Prompt, _) | (_, Self::Prompt) => Self::Prompt,
            (Self::Approve, mode) | (mode, Self::Approve) => mode,
            (Self::Auto, Self::Auto) => Self::Auto,
            (Self::Writes, Self::Writes) => Self::Writes,
            (Self::Auto, Self::Writes) | (Self::Writes, Self::Auto) => Self::Prompt,
        }
    }
}

/// Human-readable reason a configured MCP server was disabled after requirements
/// were applied.
///
/// `Display` is intentionally implemented for CLI/TUI status output; avoid
/// relying on `Debug` because enum variant syntax is not part of the user-facing
/// message contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerDisabledReason {
    /// The server is disabled, but there is no more specific user-facing reason.
    Unknown,
    /// The server was disabled by config requirements from the given source.
    Requirements { source: RequirementSource },
}

impl fmt::Display for McpServerDisabledReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            McpServerDisabledReason::Unknown => write!(f, "unknown"),
            McpServerDisabledReason::Requirements { source } => {
                write!(f, "requirements ({source})")
            }
        }
    }
}

/// Per-tool settings for a single MCP server tool.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct McpServerToolConfig {
    /// Approval mode for this tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_mode: Option<AppToolApproval>,

    /// Token budget for this tool's output, before the standard 20% serialization allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_token_limit: Option<NonZeroUsize>,
}

impl McpServerToolConfig {
    /// Applies the stricter explicit output budget without changing approval policy.
    pub fn restrict_output_token_limit(&mut self, limit: Option<NonZeroUsize>) {
        self.output_token_limit = self.output_token_limit.into_iter().chain(limit).min();
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(transparent)]
pub struct McpServerEnvVar(String);

impl McpServerEnvVar {
    pub fn name(&self) -> &str {
        &self.0
    }
}

impl From<String> for McpServerEnvVar {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for McpServerEnvVar {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl AsRef<str> for McpServerEnvVar {
    fn as_ref(&self) -> &str {
        self.name()
    }
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct McpServerConfig {
    #[serde(flatten)]
    pub transport: McpServerTransportConfig,

    /// Local runtime environment used to start this MCP server.
    #[serde(skip_serializing)]
    pub environment_id: String,

    /// When `false`, AHEAD skips initializing this MCP server.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// When `true`, session startup fails if this MCP server cannot initialize.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,

    /// When `true`, every tool from this server is advertised as safe for parallel tool calls.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub supports_parallel_tool_calls: bool,

    /// Model-facing surfaces from which this server's tools must be omitted.
    /// `None` leaves lower-priority configuration unchanged; an empty list clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omit_tools_from: Option<Vec<ToolExposureSurface>>,

    /// Reason this server was disabled after applying requirements.
    #[serde(skip)]
    pub disabled_reason: Option<McpServerDisabledReason>,

    /// Startup timeout in seconds for initializing MCP server & initially listing tools.
    #[serde(
        default,
        with = "option_duration_secs",
        skip_serializing_if = "Option::is_none"
    )]
    pub startup_timeout_sec: Option<Duration>,

    /// Default timeout for MCP tool calls initiated via this server.
    #[serde(default, with = "option_duration_secs")]
    pub tool_timeout_sec: Option<Duration>,

    /// Approval mode for tools in this server unless a tool override exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_tools_approval_mode: Option<AppToolApproval>,

    /// Explicit allow-list of tools exposed from this server. When set, only these tools will be registered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_tools: Option<Vec<String>>,

    /// Explicit deny-list of tools. These tools will be removed after applying `enabled_tools`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_tools: Option<Vec<String>>,

    /// Per-tool settings keyed by tool name.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub tools: HashMap<String, McpServerToolConfig>,
}

impl McpServerConfig {
    pub fn is_local_environment(&self) -> bool {
        self.environment_id == DEFAULT_MCP_SERVER_ENVIRONMENT_ID
    }
}

/// Raw MCP config shape used for deserialization and supported-field JSON
/// Schema generation.
///
/// Fields that are accepted only to produce targeted validation errors should
/// be skipped in the generated schema.
///
/// Keep `TryFrom<RawMcpServerConfig> for McpServerConfig` exhaustively
/// destructuring this struct so new TOML fields cannot be added here without
/// updating the validation/mapping logic that produces [`McpServerConfig`].
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RawMcpServerConfig {
    // stdio
    pub command: Option<String>,
    #[serde(default)]
    pub args: Option<Vec<String>>,
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    #[serde(default)]
    pub env_vars: Option<Vec<McpServerEnvVar>>,
    #[serde(default)]
    pub cwd: Option<LegacyAppPathString>,

    // shared
    #[serde(default)]
    pub startup_timeout_sec: Option<f64>,
    #[serde(default)]
    pub startup_timeout_ms: Option<u64>,
    #[serde(default, with = "option_duration_secs")]
    pub tool_timeout_sec: Option<Duration>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub required: Option<bool>,
    #[serde(default)]
    pub supports_parallel_tool_calls: Option<bool>,
    #[serde(default)]
    pub omit_tools_from: Option<Vec<ToolExposureSurface>>,
    #[serde(default)]
    pub default_tools_approval_mode: Option<AppToolApproval>,
    #[serde(default)]
    pub enabled_tools: Option<Vec<String>>,
    #[serde(default)]
    pub disabled_tools: Option<Vec<String>>,
    #[serde(default)]
    pub tools: Option<HashMap<String, McpServerToolConfig>>,
}

impl TryFrom<RawMcpServerConfig> for McpServerConfig {
    type Error = String;

    fn try_from(raw: RawMcpServerConfig) -> Result<Self, Self::Error> {
        let RawMcpServerConfig {
            command,
            args,
            env,
            env_vars,
            cwd,
            startup_timeout_sec,
            startup_timeout_ms,
            tool_timeout_sec,
            enabled,
            required,
            supports_parallel_tool_calls,
            omit_tools_from,
            default_tools_approval_mode,
            enabled_tools,
            disabled_tools,
            tools,
        } = raw;

        let startup_timeout_sec = match (startup_timeout_sec, startup_timeout_ms) {
            (Some(sec), _) => {
                Some(Duration::try_from_secs_f64(sec).map_err(|err| err.to_string())?)
            }
            (None, Some(ms)) => Some(Duration::from_millis(ms)),
            (None, None) => None,
        };

        let command = command.ok_or_else(|| "stdio MCP server requires command".to_string())?;
        let transport = McpServerTransportConfig::Stdio {
            command,
            args: args.unwrap_or_default(),
            env,
            env_vars: env_vars.unwrap_or_default(),
            cwd,
        };

        Ok(Self {
            transport,
            environment_id: DEFAULT_MCP_SERVER_ENVIRONMENT_ID.to_string(),
            startup_timeout_sec,
            tool_timeout_sec,
            enabled: enabled.unwrap_or_else(default_enabled),
            required: required.unwrap_or_default(),
            supports_parallel_tool_calls: supports_parallel_tool_calls.unwrap_or_default(),
            omit_tools_from,
            disabled_reason: None,
            default_tools_approval_mode,
            enabled_tools,
            disabled_tools,
            tools: tools.unwrap_or_default(),
        })
    }
}

impl<'de> Deserialize<'de> for McpServerConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        RawMcpServerConfig::deserialize(deserializer)?
            .try_into()
            .map_err(SerdeError::custom)
    }
}

const fn default_enabled() -> bool {
    true
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(untagged, deny_unknown_fields, rename_all = "snake_case")]
pub enum McpServerTransportConfig {
    /// https://modelcontextprotocol.io/specification/2025-06-18/basic/transports#stdio
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        env: Option<HashMap<String, String>>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        env_vars: Vec<McpServerEnvVar>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<LegacyAppPathString>,
    },
}

mod option_duration_secs {
    use serde::Deserialize;
    use serde::Deserializer;
    use serde::Serializer;
    use std::time::Duration;

    pub fn serialize<S>(value: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(duration) => serializer.serialize_some(&duration.as_secs_f64()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = Option::<f64>::deserialize(deserializer)?;
        secs.map(|secs| Duration::try_from_secs_f64(secs).map_err(serde::de::Error::custom))
            .transpose()
    }
}

#[cfg(test)]
#[path = "mcp_types_tests.rs"]
mod tests;
