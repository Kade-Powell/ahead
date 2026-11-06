//! Schema-heavy configuration TOML types used by Codex.

use std::collections::BTreeMap;
use std::collections::HashMap;

use crate::permissions_toml::PermissionsToml;
use crate::types::McpServerConfig;
use crate::types::MemoriesToml;
use crate::types::OtelConfigToml;
use crate::types::SandboxWorkspaceWrite;
use crate::types::ShellEnvironmentPolicyToml;
use crate::types::SkillsConfig;
use crate::types::WindowsToml;
use codex_features::FeaturesToml;
use codex_model_provider_info::AMAZON_BEDROCK_PROVIDER_ID;
use codex_model_provider_info::AMAZON_BEDROCK_RUNTIME_PROVIDER_ID;
use codex_model_provider_info::LMSTUDIO_OSS_PROVIDER_ID;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::OLLAMA_OSS_PROVIDER_ID;
use codex_model_provider_info::OPENAI_PROVIDER_ID;
use codex_protocol::config_types::AutoCompactTokenLimitScope;
use codex_protocol::config_types::Personality;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::config_types::SandboxMode;
use codex_protocol::config_types::Verbosity;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::config_types::WebSearchToolConfig;
use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::models::PermissionProfile;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_protocol::protocol::AskForApproval;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde_json::Value as JsonValue;

const RESERVED_MODEL_PROVIDER_IDS: [&str; 5] = [
    AMAZON_BEDROCK_PROVIDER_ID,
    AMAZON_BEDROCK_RUNTIME_PROVIDER_ID,
    OPENAI_PROVIDER_ID,
    OLLAMA_OSS_PROVIDER_ID,
    LMSTUDIO_OSS_PROVIDER_ID,
];

pub const DEFAULT_PROJECT_DOC_MAX_BYTES: usize = 32 * 1024;

const fn default_project_doc_max_bytes() -> Option<usize> {
    Some(DEFAULT_PROJECT_DOC_MAX_BYTES)
}

const fn default_hide_agent_reasoning() -> Option<bool> {
    Some(false)
}

const fn default_true() -> bool {
    true
}

/// Orchestrator-owned feature settings.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct OrchestratorToml {
    pub skills: Option<OrchestratorFeatureToml>,
}

/// Settings for a feature owned by the orchestrator.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct OrchestratorFeatureToml {
    pub enabled: Option<bool>,
}

/// Configuration shape consumed by AHEAD's managed agent runtime.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct ConfigToml {
    /// Optional override of model selection.
    pub model: Option<String>,
    /// Provider to use from the model_providers map.
    pub model_provider: Option<String>,

    /// Size of the context window for the model, in tokens.
    pub model_context_window: Option<i64>,

    /// Token usage threshold triggering auto-compaction of conversation history.
    pub model_auto_compact_token_limit: Option<i64>,

    /// Controls whether the auto-compaction limit applies to the full context or
    /// only to tokens after the carried prefix in the current compaction window.
    pub model_auto_compact_token_limit_scope: Option<AutoCompactTokenLimitScope>,

    /// Default approval policy for executing commands.
    pub approval_policy: Option<AskForApproval>,

    #[serde(default)]
    pub shell_environment_policy: ShellEnvironmentPolicyToml,

    /// Whether the model may request a login shell for shell-based tools.
    /// Default to `true`
    ///
    /// If `true`, the model may request a login shell (`login = true`), and
    /// omitting `login` defaults to using a login shell.
    /// If `false`, the model can never use a login shell: `login = true`
    /// requests are rejected, and omitting `login` defaults to a non-login
    /// shell.
    pub allow_login_shell: Option<bool>,

    /// Sandbox mode to use.
    pub sandbox_mode: Option<SandboxMode>,

    /// Sandbox configuration to apply if `sandbox` is `WorkspaceWrite`.
    pub sandbox_workspace_write: Option<SandboxWorkspaceWrite>,

    /// Default permissions profile to apply. Names starting with `:` refer to
    /// built-in profiles; other names are resolved from the `[permissions]`
    /// table.
    pub default_permissions: Option<String>,

    /// Named permissions profiles.
    #[serde(default)]
    pub permissions: Option<PermissionsToml>,

    /// System instructions.
    pub instructions: Option<String>,

    /// Developer instructions inserted as a `developer` role message.
    #[serde(default)]
    pub developer_instructions: Option<String>,

    /// Whether to inject the `<permissions instructions>` developer block.
    pub include_permissions_instructions: Option<bool>,

    /// Whether to inject the `<collaboration_mode>` developer block.
    pub include_collaboration_mode_instructions: Option<bool>,

    /// Whether to inject the `<environment_context>` user block.
    pub include_environment_context: Option<bool>,

    /// Optional path to a file containing model instructions that will override
    /// the built-in instructions for the selected model. Users are STRONGLY
    /// DISCOURAGED from using this field, as deviating from the instructions
    /// sanctioned by Codex will likely degrade model performance.
    pub model_instructions_file: Option<AbsolutePathBuf>,

    /// Compact prompt used for history compaction.
    pub compact_prompt: Option<String>,

    /// Definition for MCP servers that Codex can reach out to for tool calls.
    #[serde(default)]
    // Uses the raw MCP input shape (custom deserialization) rather than `McpServerConfig`.
    pub mcp_servers: HashMap<String, McpServerConfig>,

    /// Milliseconds to wait for optional MCP servers while building the initial tool catalog.
    ///
    /// Defaults to 1000. Set to 0 to disable the shared grace and wait for each
    /// server's configured `startup_timeout_sec` instead.
    pub mcp_optional_startup_grace_ms: Option<u64>,

    /// User-defined provider entries that extend the built-in list. Built-in
    /// IDs cannot be overridden.
    #[serde(default, deserialize_with = "deserialize_model_providers")]
    pub model_providers: HashMap<String, ModelProviderInfo>,

    /// Maximum total bytes of project instruction content across all selected environments.
    #[serde(default = "default_project_doc_max_bytes")]
    pub project_doc_max_bytes: Option<usize>,

    /// Token budget applied when storing tool/function outputs in the context manager.
    pub tool_output_token_limit: Option<usize>,

    /// Maximum poll window for background terminal output (`write_stdin`), in milliseconds.
    /// Default: `300000` (5 minutes).
    pub background_terminal_max_timeout: Option<u64>,

    /// Directory where Codex writes log files. Setting this value explicitly
    /// also enables the TUI text log in this directory.
    /// Defaults to `$AHEAD_RUNTIME_HOME/log`.
    pub log_dir: Option<AbsolutePathBuf>,

    /// When set to `true`, `AgentReasoning` events will be hidden from the
    /// UI/output. Defaults to `false`.
    #[serde(default = "default_hide_agent_reasoning")]
    pub hide_agent_reasoning: Option<bool>,

    /// When set to `true`, `AgentReasoningRawContentEvent` events will be shown in the UI/output.
    /// Defaults to `false`.
    pub show_raw_agent_reasoning: Option<bool>,

    pub model_reasoning_effort: Option<ReasoningEffort>,
    pub plan_mode_reasoning_effort: Option<ReasoningEffort>,
    pub model_reasoning_summary: Option<ReasoningSummary>,
    /// Optional verbosity control for GPT-5 models (Responses API `text.verbosity`).
    pub model_verbosity: Option<Verbosity>,

    /// Optional path to a JSON model catalog (applied on startup only).
    /// Per-thread `config` overrides are accepted but do not reapply this (no-ops).
    pub model_catalog_json: Option<AbsolutePathBuf>,

    /// Optionally specify a personality for the model
    pub personality: Option<Personality>,

    /// Optional explicit service tier request id for new turns (for example
    /// `default`, `priority`, or `flex`; legacy `fast` also works).
    pub service_tier: Option<String>,

    /// Bounded, product-owned metadata attached to every Responses API request.
    pub responses_api_metadata: Option<BTreeMap<String, String>>,

    /// Orchestrator-owned feature settings.
    pub orchestrator: Option<OrchestratorToml>,

    /// Base URL override for the built-in `openai` model provider.
    pub openai_base_url: Option<String>,

    /// Machine-local realtime audio device preferences used by realtime voice.
    #[serde(default)]
    pub audio: Option<RealtimeAudioToml>,

    /// Experimental / do not use. Overrides only the realtime conversation
    /// websocket transport base URL (the `Op::RealtimeConversation`
    /// `/v1/realtime`
    /// connection) without changing normal provider HTTP requests.
    pub experimental_realtime_ws_base_url: Option<String>,
    /// Experimental / do not use. Overrides only the WebRTC realtime call
    /// creation base URL. This is separate from `experimental_realtime_ws_base_url`
    /// because WebRTC call creation is HTTP, while sideband control is websocket.
    pub experimental_realtime_webrtc_call_base_url: Option<String>,
    /// Experimental / do not use. Selects the realtime websocket model/snapshot
    /// used for the `Op::RealtimeConversation` connection.
    pub experimental_realtime_ws_model: Option<String>,
    /// Experimental / do not use. Realtime websocket session selection.
    /// `version` controls v1/v2 and `type` controls conversational/transcription.
    #[serde(default)]
    pub realtime: Option<RealtimeToml>,
    /// Experimental / do not use. Overrides only the realtime conversation
    /// websocket transport instructions (the `Op::RealtimeConversation`
    /// `/ws` session.update instructions) without changing normal prompts.
    pub experimental_realtime_ws_backend_prompt: Option<String>,
    /// Experimental / do not use. Replaces the synthesized realtime startup
    /// context appended to websocket session instructions. An empty string
    /// disables startup context injection entirely.
    pub experimental_realtime_ws_startup_context: Option<String>,
    /// Experimental / do not use. Replaces the built-in realtime start
    /// instructions inserted into developer messages when realtime becomes
    /// active.
    pub experimental_realtime_start_instructions: Option<String>,

    /// Removed. Retained only to fail fast rather than silently ignore a
    /// configured remote thread-store endpoint.
    pub experimental_thread_store_endpoint: Option<String>,

    /// Removed. Managed sessions always use the application-injected Turso store.
    pub experimental_thread_store: Option<JsonValue>,

    /// Controls the web search tool mode: disabled, cached, indexed, or live.
    pub web_search: Option<WebSearchMode>,

    /// Nested tools section for feature toggles
    pub tools: Option<ToolsToml>,

    /// Memories subsystem settings.
    pub memories: Option<MemoriesToml>,

    /// User-level skill config entries keyed by SKILL.md path.
    pub skills: Option<SkillsConfig>,

    /// Centralized feature flags (new). Prefer this over individual toggles.
    #[serde(default)]
    // Injects known feature keys into the schema and forbids unknown keys.
    pub features: Option<FeaturesToml>,

    /// Suppress warnings about unstable (under development) features.
    pub suppress_unstable_features_warning: Option<bool>,

    /// Markers used to find the root for standard `AGENTS.md` and skill discovery.
    /// Defaults to [".git"] when unset.
    #[serde(default)]
    pub project_root_markers: Option<Vec<String>>,

    /// OTEL configuration.
    pub otel: Option<OtelConfigToml>,

    /// Windows-specific configuration.
    #[serde(default)]
    pub windows: Option<WindowsToml>,

    pub experimental_compact_prompt_file: Option<AbsolutePathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RealtimeAudioConfig {
    pub microphone: Option<String>,
    pub speaker: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeWsMode {
    #[default]
    Conversational,
    Transcription,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeTransport {
    #[default]
    #[serde(rename = "webrtc")]
    WebRtc,
    Websocket,
}

pub use codex_protocol::protocol::RealtimeConversationVersion as RealtimeWsVersion;
pub use codex_protocol::protocol::RealtimeVoice;

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct RealtimeConfig {
    pub version: RealtimeWsVersion,
    #[serde(rename = "type")]
    pub session_type: RealtimeWsMode,
    pub transport: RealtimeTransport,
    pub voice: Option<RealtimeVoice>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct RealtimeToml {
    pub version: Option<RealtimeWsVersion>,
    #[serde(rename = "type")]
    pub session_type: Option<RealtimeWsMode>,
    pub transport: Option<RealtimeTransport>,
    pub voice: Option<RealtimeVoice>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct RealtimeAudioToml {
    pub microphone: Option<String>,
    pub speaker: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct ToolsToml {
    #[serde(
        default,
        deserialize_with = "deserialize_optional_web_search_tool_config"
    )]
    pub web_search: Option<WebSearchToolConfig>,
    pub experimental_request_user_input: Option<ExperimentalRequestUserInput>,
    pub update_plan: Option<UpdatePlanToolConfig>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ExperimentalRequestUserInput {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct UpdatePlanToolConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WebSearchToolConfigInput {
    Enabled(bool),
    Config(WebSearchToolConfig),
}

fn deserialize_optional_web_search_tool_config<'de, D>(
    deserializer: D,
) -> Result<Option<WebSearchToolConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<WebSearchToolConfigInput>::deserialize(deserializer)?;

    Ok(match value {
        None => None,
        Some(WebSearchToolConfigInput::Enabled(enabled)) => {
            let _ = enabled;
            None
        }
        Some(WebSearchToolConfigInput::Config(config)) => Some(config),
    })
}

impl ConfigToml {
    /// Derive the effective permission profile from legacy sandbox config.
    ///
    /// Call this only after ruling out `default_permissions`: named
    /// `[permissions]` profiles must be compiled through the permissions
    /// profile pipeline, not reconstructed from `sandbox_mode`.
    pub async fn derive_permission_profile(
        &self,
        sandbox_mode_override: Option<SandboxMode>,
        windows_sandbox_level: WindowsSandboxLevel,
        permission_profile_constraint: Option<&crate::Constrained<PermissionProfile>>,
    ) -> PermissionProfile {
        let configured_sandbox_mode = sandbox_mode_override.or(self.sandbox_mode);
        let resolved_sandbox_mode = configured_sandbox_mode.unwrap_or_default();
        let effective_sandbox_mode = if cfg!(target_os = "windows")
            // If the experimental Windows sandbox is enabled, do not force a downgrade.
            && windows_sandbox_level == WindowsSandboxLevel::Disabled
            && matches!(resolved_sandbox_mode, SandboxMode::WorkspaceWrite)
        {
            SandboxMode::ReadOnly
        } else {
            resolved_sandbox_mode
        };

        let permission_profile = match effective_sandbox_mode {
            SandboxMode::ReadOnly => PermissionProfile::read_only(),
            SandboxMode::WorkspaceWrite => match self.sandbox_workspace_write.as_ref() {
                Some(SandboxWorkspaceWrite {
                    writable_roots,
                    network_access,
                    exclude_tmpdir_env_var,
                    exclude_slash_tmp,
                }) => {
                    let network_policy = if *network_access {
                        NetworkSandboxPolicy::Enabled
                    } else {
                        NetworkSandboxPolicy::Restricted
                    };
                    PermissionProfile::workspace_write_with(
                        writable_roots,
                        network_policy,
                        *exclude_tmpdir_env_var,
                        *exclude_slash_tmp,
                    )
                }
                None => PermissionProfile::workspace_write(),
            },
            SandboxMode::DangerFullAccess => PermissionProfile::Disabled,
        };
        if configured_sandbox_mode.is_none()
            && let Some(constraint) = permission_profile_constraint
            && let Err(err) = constraint.can_set(&permission_profile)
        {
            tracing::warn!(
                error = %err,
                "default sandbox policy is disallowed by requirements; falling back to required default"
            );
            PermissionProfile::read_only()
        } else {
            permission_profile
        }
    }
}

pub fn validate_reserved_model_provider_ids(
    model_providers: &HashMap<String, ModelProviderInfo>,
) -> Result<(), String> {
    let mut conflicts = model_providers
        .keys()
        .filter(|key| {
            !matches!(
                key.as_str(),
                AMAZON_BEDROCK_PROVIDER_ID | AMAZON_BEDROCK_RUNTIME_PROVIDER_ID
            ) && RESERVED_MODEL_PROVIDER_IDS.contains(&key.as_str())
        })
        .map(|key| format!("`{key}`"))
        .collect::<Vec<_>>();
    conflicts.sort_unstable();
    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "model_providers contains reserved built-in provider IDs: {}. \
Built-in providers cannot be overridden. Rename your custom provider (for example, `openai-custom`).",
            conflicts.join(", ")
        ))
    }
}

pub fn validate_model_providers(
    model_providers: &HashMap<String, ModelProviderInfo>,
) -> Result<(), String> {
    validate_reserved_model_provider_ids(model_providers)?;
    for (key, provider) in model_providers {
        if !matches!(
            key.as_str(),
            AMAZON_BEDROCK_PROVIDER_ID | AMAZON_BEDROCK_RUNTIME_PROVIDER_ID
        ) {
            if provider.aws.is_some() {
                return Err(format!(
                    "model_providers.{key}: provider aws is only supported for \
`{AMAZON_BEDROCK_PROVIDER_ID}` or `{AMAZON_BEDROCK_RUNTIME_PROVIDER_ID}`"
                ));
            }
            if provider.name.trim().is_empty() {
                return Err(format!(
                    "model_providers.{key}: provider name must not be empty"
                ));
            }
        }
        provider
            .validate()
            .map_err(|message| format!("model_providers.{key}: {message}"))?;
    }
    Ok(())
}

fn deserialize_model_providers<'de, D>(
    deserializer: D,
) -> Result<HashMap<String, ModelProviderInfo>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let model_providers = HashMap::<String, ModelProviderInfo>::deserialize(deserializer)?;
    validate_model_providers(&model_providers).map_err(serde::de::Error::custom)?;
    Ok(model_providers)
}

#[cfg(test)]
#[path = "bedrock_runtime_tests.rs"]
mod bedrock_runtime_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amazon_bedrock_auth_command_must_not_be_empty() {
        let err = toml::from_str::<ConfigToml>(
            r#"
[model_providers.amazon-bedrock.auth]
command = "   "
"#,
        )
        .expect_err("empty Amazon Bedrock auth command should be rejected");

        assert!(
            err.to_string().contains(
                "model_providers.amazon-bedrock: provider auth.command must not be empty"
            )
        );
    }
}
