use crate::context::world_state::validate_managed_developer_instructions;
use crate::path_utils::normalize_for_native_workdir;
use crate::unified_exec::DEFAULT_MAX_BACKGROUND_TERMINAL_TIMEOUT_MS;
use crate::unified_exec::MIN_EMPTY_YIELD_TIME_MS;
use crate::windows_sandbox::WindowsSandboxLevelExt;
use crate::windows_sandbox::resolve_windows_sandbox_mode;
use crate::windows_sandbox::resolve_windows_sandbox_private_desktop;
use codex_config::ConfigLayerSource;
use codex_config::ConfigLayerStack;
use codex_config::ConfigRequirements;
use codex_config::ConfigRequirementsToml;
use codex_config::ConstrainedWithSource;
use codex_config::FeatureRequirementsToml;
use codex_config::McpServerRequirement;
use codex_config::ResidencyRequirement;
use codex_config::SandboxModeRequirement;
use codex_config::Sourced;
use codex_config::config_toml::ConfigToml;
use codex_config::config_toml::DEFAULT_PROJECT_DOC_MAX_BYTES;
use codex_config::config_toml::RealtimeAudioConfig;
use codex_config::config_toml::RealtimeConfig;
use codex_config::config_toml::validate_model_providers;
use codex_config::loader::load_config_layers_state;
use codex_config::permissions_toml::PermissionProfileToml;
use codex_config::permissions_toml::PermissionsToml;
use codex_config::sandbox_mode_requirement_for_permission_profile;
use codex_config::types::McpServerConfig;
use codex_config::types::McpServerDisabledReason;
use codex_config::types::MemoriesConfig;
use codex_config::types::WindowsSandboxModeToml;
use codex_exec_server::ExecutorFileSystem;
use codex_exec_server::LOCAL_FS;
use codex_exec_server::ReadFileOptions;
use codex_features::CodeModeConfigToml;
use codex_features::CurrentTimeReminderConfigToml;
use codex_features::CurrentTimeReminderDeliveryMode;
use codex_features::CurrentTimeSource;
use codex_features::Feature;
use codex_features::FeatureConfigSource;
use codex_features::FeatureOverrides;
use codex_features::FeatureToml;
use codex_features::Features;
use codex_features::FeaturesToml;
use codex_features::MultiAgentV2ConfigToml;
use codex_features::SleepToolMode;
use codex_features::TokenBudgetConfigToml;
use codex_http_client::HttpClientFactory;
use codex_http_client::OutboundProxyPolicy;
use codex_mcp::DEFAULT_OPTIONAL_MCP_STARTUP_GRACE;
use codex_mcp::McpConfig;
use codex_mcp::McpProtocolMode;
use codex_mcp::McpServerRegistration;
use codex_mcp::ResolvedMcpCatalog;
use codex_model_provider::ProviderCapabilities;
use codex_model_provider_info::LEGACY_OLLAMA_CHAT_PROVIDER_ID;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::OLLAMA_CHAT_PROVIDER_REMOVED_ERROR;
use codex_model_provider_info::built_in_model_providers;
use codex_model_provider_info::merge_configured_model_providers;
use codex_models_manager::ModelsManagerConfig;
use codex_protocol::config_types::AutoCompactTokenLimitScope;
use codex_protocol::config_types::Personality;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::config_types::SERVICE_TIER_DEFAULT_REQUEST_VALUE;
use codex_protocol::config_types::SandboxMode;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::config_types::ShellEnvironmentPolicy;
use codex_protocol::config_types::Verbosity;
use codex_protocol::config_types::WebSearchConfig;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::models::ActivePermissionProfile;
use codex_protocol::models::BaseInstructionsProvenance;
use codex_protocol::models::PermissionProfile;
pub use codex_protocol::models::PermissionProfileSnapshot;
use codex_protocol::models::SandboxEnforcement;
use codex_protocol::openai_models::ModelsResponse;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::permissions::FileSystemPath;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_protocol::permissions::ReadDenyMatcher;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::MultiAgentVersion;
use codex_protocol::protocol::SandboxPolicy;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_absolute_path::AbsolutePathBufGuard;
use codex_utils_home_dir::find_ahead_home;
use codex_utils_path_uri::PathUri;
use rmcp::model::ElicitationCapability;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::io::ErrorKind;
use std::num::NonZeroUsize;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::config::permissions::BUILT_IN_READ_ONLY_PROFILE;
use crate::config::permissions::BUILT_IN_WORKSPACE_PROFILE;
use crate::config::permissions::builtin_permission_profile;
use crate::config::permissions::compile_permission_profile_selection;
use crate::config::permissions::compile_permission_profile_workspace_roots;
use crate::config::permissions::default_builtin_permission_profile_name;
use crate::config::permissions::validate_user_permission_profile_names;
use crate::responses_metadata::validate_extra_metadata;
use toml::Value as TomlValue;

mod managed_features;
mod otel;
mod permission_profile_catalog;
mod permission_profile_selection;
mod permissions;
mod requirements;
mod resolved_permission_profile;
pub use codex_config::ConfigLoadOptions;
pub use codex_config::Constrained;
pub use codex_config::ConstraintError;
pub use codex_config::ConstraintResult;
pub use codex_config::LoaderOverrides;
use codex_sandboxing::compatibility_sandbox_policy_for_permission_profile;
pub use codex_sandboxing::system_bwrap_warning;
pub use managed_features::ManagedFeatures;
pub use permission_profile_catalog::PermissionProfileCatalogEntry;
pub use permission_profile_catalog::permission_profile_catalog;
use permission_profile_catalog::permission_profile_catalog_from_permissions;
use permission_profile_catalog::permission_profile_is_allowed;
use permission_profile_catalog::validate_permission_profile_for_deny_read;
pub use permission_profile_selection::ResolvedPermissionProfileSelection;
pub use permission_profile_selection::resolve_permission_profile_selection;
pub use permissions::compile_permission_profile;
pub(crate) use permissions::is_builtin_permission_profile_name;
pub use permissions::resolve_permission_profile;
pub(crate) use resolved_permission_profile::PermissionProfileState;

/// Maximum number of bytes of the documentation that will be embedded. Larger
/// files are *silently truncated* to this size so we do not take up too much of
/// the context window.
pub(crate) const AGENTS_MD_MAX_BYTES: usize = DEFAULT_PROJECT_DOC_MAX_BYTES; // 32 KiB
pub(crate) const DEFAULT_MULTI_AGENT_V2_MAX_CONCURRENT_THREADS_PER_SESSION: usize = 4;

pub const CONFIG_TOML_FILE: &str = "config.toml";

#[cfg(test)]
pub(crate) async fn test_config() -> Config {
    let codex_home = tempfile::tempdir().expect("create temp dir");
    Config::load_from_base_config_with_overrides(
        ConfigToml {
            model: Some("gpt-5.5".to_string()),
            ..Default::default()
        },
        ConfigOverrides::default(),
        AbsolutePathBuf::from_absolute_path(codex_home.path()).expect("temp dir should resolve"),
    )
    .await
    .expect("load default test config")
}

/// Application configuration loaded from disk and merged with overrides.
#[derive(Debug, Clone, PartialEq)]
pub struct Permissions {
    /// Approval policy for executing commands.
    pub approval_policy: Constrained<AskForApproval>,
    /// Constrained permission profile plus its selected profile identity, if
    /// the profile came from a built-in or named config profile.
    permission_profile_state: PermissionProfileState,
    /// Managed deny-read rules retained independently from user-defined denies.
    managed_deny_read_policy: Option<Arc<FileSystemSandboxPolicy>>,
    /// Thread-scoped runtime workspace roots. Symbolic `:workspace_roots`
    /// entries in the permission profile are materialized against these roots.
    workspace_roots: Vec<AbsolutePathBuf>,
    /// Whether the model may request a login shell for shell-based tools.
    /// Default to `true`
    ///
    /// If `true`, the model may request a login shell (`login = true`), and
    /// omitting `login` defaults to using a login shell.
    /// If `false`, the model can never use a login shell: `login = true`
    /// requests are rejected, and omitting `login` defaults to a non-login
    /// shell.
    pub allow_login_shell: bool,
    /// Policy used to build process environments for shell/unified exec.
    pub shell_environment_policy: ShellEnvironmentPolicy,
    /// Effective Windows sandbox mode derived from `[windows].sandbox` or
    /// legacy feature keys.
    pub windows_sandbox_mode: Option<WindowsSandboxModeToml>,
    /// Whether the final Windows sandboxed child should run on a private desktop.
    pub windows_sandbox_private_desktop: bool,
}

impl Permissions {
    /// Build permissions from the constrained values required for a minimal
    /// in-process configuration.
    pub fn from_approval_and_profile(
        approval_policy: Constrained<AskForApproval>,
        permission_profile: Constrained<PermissionProfile>,
    ) -> ConstraintResult<Self> {
        Ok(Self {
            approval_policy,
            permission_profile_state: PermissionProfileState::from_constrained_legacy(
                permission_profile,
            )?,
            managed_deny_read_policy: None,
            workspace_roots: Vec::new(),
            allow_login_shell: true,
            shell_environment_policy: ShellEnvironmentPolicy::default(),
            windows_sandbox_mode: None,
            windows_sandbox_private_desktop: true,
        })
    }

    pub(crate) fn permission_profile_state(&self) -> &PermissionProfileState {
        &self.permission_profile_state
    }

    pub(crate) fn set_permission_profile_state(
        &mut self,
        permission_profile_state: PermissionProfileState,
    ) {
        self.permission_profile_state = permission_profile_state;
    }

    /// Apply a permission profile snapshot emitted by core session state.
    ///
    /// This is a trusted-state bridge for consumers of `SessionConfigured`.
    /// Config loading and host integrations should resolve named profiles
    /// through config instead of constructing a snapshot directly.
    pub fn set_permission_profile_from_session_snapshot(
        &mut self,
        snapshot: PermissionProfileSnapshot,
    ) -> ConstraintResult<()> {
        self.permission_profile_state
            .set_permission_profile_snapshot(snapshot)
    }

    /// Replace the current permission constraints with a trusted session
    /// snapshot. This is only for clients that must mirror core session state
    /// after their local config constraints reject the snapshot.
    pub fn replace_permission_profile_from_session_snapshot(
        &mut self,
        snapshot: PermissionProfileSnapshot,
    ) -> ConstraintResult<()> {
        let permission_profile = Constrained::allow_only(snapshot.permission_profile().clone());
        self.permission_profile_state =
            PermissionProfileState::from_constrained_snapshot(permission_profile, snapshot)?;
        Ok(())
    }

    /// Borrow the canonical profile before runtime workspace-root
    /// materialization has been applied.
    pub fn permission_profile(&self) -> &PermissionProfile {
        self.permission_profile_state.permission_profile()
    }

    pub fn can_set_permission_profile(
        &self,
        permission_profile: &PermissionProfile,
    ) -> ConstraintResult<()> {
        let permission_profile =
            self.permission_profile_preserving_managed_denied_reads(permission_profile.clone());
        self.permission_profile_state
            .can_set_legacy_permission_profile(&permission_profile)
    }

    pub fn set_workspace_roots(&mut self, workspace_roots: Vec<AbsolutePathBuf>) {
        self.workspace_roots = workspace_roots;
    }

    pub fn workspace_roots(&self) -> &[AbsolutePathBuf] {
        &self.workspace_roots
    }

    /// Workspace roots that came from user-visible configuration or runtime
    /// selection. Internal Codex-only writable roots are intentionally excluded.
    pub fn user_visible_workspace_roots(&self) -> &[AbsolutePathBuf] {
        &self.workspace_roots
    }

    pub fn profile_workspace_roots(&self) -> &[AbsolutePathBuf] {
        self.permission_profile_state.profile_workspace_roots()
    }

    /// Effective runtime permissions after config requirements and runtime
    /// workspace-root materialization have been applied.
    pub fn effective_permission_profile(&self) -> PermissionProfile {
        self.permission_profile()
            .clone()
            .materialize_project_roots_with_workspace_roots(&self.workspace_roots)
    }

    /// Named profile selected by config, if the current profile has one.
    pub fn active_permission_profile(&self) -> Option<ActivePermissionProfile> {
        self.permission_profile_state.active_permission_profile()
    }

    /// Effective filesystem sandbox policy derived from the canonical profile.
    pub fn file_system_sandbox_policy(&self) -> FileSystemSandboxPolicy {
        self.effective_permission_profile()
            .file_system_sandbox_policy()
    }

    /// Effective network sandbox policy derived from the canonical profile.
    pub fn network_sandbox_policy(&self) -> NetworkSandboxPolicy {
        self.permission_profile().network_sandbox_policy()
    }

    /// Legacy compatibility projection derived from the canonical profile.
    pub fn legacy_sandbox_policy(&self, cwd: &Path) -> SandboxPolicy {
        let permission_profile = self.effective_permission_profile();
        compatibility_sandbox_policy_for_permission_profile(&permission_profile, cwd)
    }

    /// Check whether a legacy sandbox policy can be applied to this permission
    /// set after projecting it into the canonical permission profile.
    pub fn can_set_legacy_sandbox_policy(
        &self,
        sandbox_policy: &SandboxPolicy,
        cwd: &Path,
    ) -> ConstraintResult<()> {
        let file_system_sandbox_policy =
            FileSystemSandboxPolicy::from_legacy_sandbox_policy_for_cwd(sandbox_policy, cwd);
        let network_sandbox_policy = NetworkSandboxPolicy::from(sandbox_policy);
        let permission_profile = PermissionProfile::from_runtime_permissions_with_enforcement(
            SandboxEnforcement::from_legacy_sandbox_policy(sandbox_policy),
            &file_system_sandbox_policy,
            network_sandbox_policy,
        );
        self.can_set_permission_profile(&permission_profile)
    }

    /// Set permissions from a legacy sandbox policy and keep every permission
    /// projection in sync.
    pub fn set_legacy_sandbox_policy(
        &mut self,
        sandbox_policy: SandboxPolicy,
        cwd: &Path,
    ) -> ConstraintResult<()> {
        self.can_set_legacy_sandbox_policy(&sandbox_policy, cwd)?;
        let file_system_sandbox_policy =
            FileSystemSandboxPolicy::from_legacy_sandbox_policy_for_cwd(&sandbox_policy, cwd);
        let network_sandbox_policy = NetworkSandboxPolicy::from(&sandbox_policy);
        let permission_profile = PermissionProfile::from_runtime_permissions_with_enforcement(
            SandboxEnforcement::from_legacy_sandbox_policy(&sandbox_policy),
            &file_system_sandbox_policy,
            network_sandbox_policy,
        );
        self.workspace_roots = match &sandbox_policy {
            SandboxPolicy::WorkspaceWrite { writable_roots, .. } => {
                let mut workspace_roots = vec![
                    AbsolutePathBuf::from_absolute_path(cwd)
                        .unwrap_or_else(|_| AbsolutePathBuf::resolve_path_against_base(cwd, "/")),
                ];
                for root in writable_roots {
                    if !workspace_roots.iter().any(|existing| existing == root) {
                        workspace_roots.push(root.clone());
                    }
                }
                workspace_roots
            }
            SandboxPolicy::DangerFullAccess
            | SandboxPolicy::ExternalSandbox { .. }
            | SandboxPolicy::ReadOnly { .. } => vec![
                AbsolutePathBuf::from_absolute_path(cwd)
                    .unwrap_or_else(|_| AbsolutePathBuf::resolve_path_against_base(cwd, "/")),
            ],
        };

        self.set_permission_profile(permission_profile)
    }

    /// Set permissions from the canonical profile.
    pub fn set_permission_profile(
        &mut self,
        permission_profile: PermissionProfile,
    ) -> ConstraintResult<()> {
        let permission_profile =
            self.permission_profile_preserving_managed_denied_reads(permission_profile);
        self.permission_profile_state
            .set_legacy_permission_profile(permission_profile)
    }

    fn permission_profile_preserving_managed_denied_reads(
        &self,
        permission_profile: PermissionProfile,
    ) -> PermissionProfile {
        let Some(managed_deny_read_policy) = self.managed_deny_read_policy.as_ref() else {
            return permission_profile;
        };
        if matches!(
            sandbox_mode_requirement_for_permission_profile(&permission_profile),
            SandboxModeRequirement::DangerFullAccess | SandboxModeRequirement::ExternalSandbox
        ) {
            return permission_profile;
        }
        let enforcement = permission_profile.enforcement();
        let (mut file_system_policy, network_policy) = permission_profile.to_runtime_permissions();
        file_system_policy.preserve_deny_read_restrictions_from(managed_deny_read_policy);
        PermissionProfile::from_runtime_permissions_with_enforcement(
            enforcement,
            &file_system_policy,
            network_policy,
        )
    }
}

/// Application configuration loaded from disk and merged with overrides.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Provenance for how this [`Config`] was derived (merged layers + enforced
    /// requirements).
    pub config_layer_stack: ConfigLayerStack,

    /// Warnings collected during config load that should be shown on startup.
    pub startup_warnings: Vec<String>,

    /// Optional override of model selection.
    pub model: Option<String>,

    /// Effective service tier request id preference for new turns.
    /// `default` means the user explicitly selected standard routing.
    pub service_tier: Option<String>,

    /// Size of the context window for the model, in tokens.
    pub model_context_window: Option<i64>,

    /// Token usage threshold triggering auto-compaction of conversation history.
    pub model_auto_compact_token_limit: Option<i64>,

    /// Controls whether `model_auto_compact_token_limit` applies to the full
    /// active context or only tokens after the carried compaction-window prefix.
    pub model_auto_compact_token_limit_scope: AutoCompactTokenLimitScope,

    /// Key into the model_providers map that specifies which provider to use.
    pub model_provider_id: String,

    /// Info needed to make an API request to the model.
    pub model_provider: ModelProviderInfo,

    /// Optionally specify the personality of the model
    pub personality: Option<Personality>,

    /// Effective permission configuration for shell tool execution.
    pub permissions: Permissions,

    /// Whether config explicitly selected named permissions profiles instead
    /// of the legacy `sandbox_mode` syntax.
    pub explicit_permission_profile_mode: bool,

    /// User-defined permission profiles available from effective config.
    pub custom_permission_profiles: Vec<PermissionProfileCatalogEntry>,

    /// enforce_residency means web traffic cannot be routed outside of a
    /// particular geography. HTTP clients should direct their requests
    /// using backend-specific headers or URLs to enforce this.
    pub enforce_residency: Constrained<Option<ResidencyRequirement>>,

    /// When `true`, `AgentReasoning` events emitted by the backend will be
    /// suppressed from the frontend output. This can reduce visual noise when
    /// users are only interested in the final agent responses.
    pub hide_agent_reasoning: bool,

    /// When set to `true`, `AgentReasoningRawContentEvent` events will be shown in the UI/output.
    /// Defaults to `false`.
    pub show_raw_agent_reasoning: bool,

    /// Base instructions override.
    pub base_instructions: Option<String>,

    /// Origin of the configured base instructions when supplied by another session or lockfile.
    pub base_instructions_provenance: Option<BaseInstructionsProvenance>,

    /// Developer instructions override injected as a separate message.
    pub developer_instructions: Option<String>,

    /// Whether to inject the `<permissions instructions>` developer block.
    pub include_permissions_instructions: bool,

    /// Whether to inject the `<collaboration_mode>` developer block.
    pub include_collaboration_mode_instructions: bool,

    /// Whether to inject the `<skills_instructions>` developer block.
    pub include_skill_instructions: bool,

    /// Optional token budget override for the available-skills catalog.
    pub skill_max_context_tokens: Option<NonZeroUsize>,

    /// Whether orchestrator-owned skills are exposed to the model.
    pub orchestrator_skills_enabled: bool,

    /// Whether to inject the `<environment_context>` user block.
    pub include_environment_context: bool,

    /// Compact prompt override.
    pub compact_prompt: Option<String>,

    /// The absolute directory that should be treated as the current working
    /// directory for the session. All relative paths inside the business-logic
    /// layer are resolved against this path.
    pub cwd: AbsolutePathBuf,

    /// Absolute runtime workspace roots for the session. Symbolic
    /// `:workspace_roots` permission entries are materialized against these
    /// roots while profile-defined workspace roots remain encoded directly in
    /// the permission profile.
    pub workspace_roots: Vec<AbsolutePathBuf>,
    /// Whether runtime workspace roots were supplied explicitly by the caller
    /// or legacy config, rather than defaulting to `cwd`.
    pub workspace_roots_explicit: bool,

    /// Definition for MCP servers that Codex can reach out to for tool calls.
    pub mcp_servers: Constrained<HashMap<String, McpServerConfig>>,

    /// When present, only these MCP servers omit the legacy `mcp__` namespace prefix.
    pub non_prefixed_mcp_tool_servers: Option<Vec<String>>,

    /// How long to wait for optional MCP servers while building the initial tool catalog.
    pub mcp_optional_startup_grace: Duration,

    /// Combined provider map (defaults plus user-defined providers).
    pub model_providers: HashMap<String, ModelProviderInfo>,

    /// Maximum total bytes of project instruction content across all selected environments.
    pub project_doc_max_bytes: usize,

    /// Token budget applied when storing tool/function outputs in the context manager.
    pub tool_output_token_limit: Option<usize>,

    /// Memories subsystem settings.
    pub memories: MemoriesConfig,

    /// Runtime state directory (defaults to `~/.ahead` unless the host supplies
    /// a project-specific path or `AHEAD_HOME` is set).
    pub codex_home: AbsolutePathBuf,

    /// Directory where the retained runtime writes log files.
    pub log_dir: PathBuf,

    /// When true, session is not persisted on disk. Default to `false`
    pub ephemeral: bool,

    /// Path to the current Codex executable. This cannot be set in the config
    /// file: it must be set in code via [`ConfigOverrides`].
    pub ahead_self_exe: Option<PathBuf>,

    /// Path to the `codex-linux-sandbox` executable. This must be set if
    /// [`codex_sandboxing::SandboxType::LinuxSeccomp`] is used. Note that this
    /// cannot be set in the config file: it must be set in code via
    /// [`ConfigOverrides`].
    ///
    /// When this program is invoked, arg0 will be set to `codex-linux-sandbox`.
    pub ahead_linux_sandbox_exe: Option<PathBuf>,

    /// Value to use for `reasoning.effort` when making a request using the
    /// Responses API.
    pub model_reasoning_effort: Option<ReasoningEffort>,
    /// Optional Plan-mode-specific reasoning effort override used by the TUI.
    ///
    /// When unset, Plan mode uses the built-in Plan preset default (currently
    /// `medium`). When explicitly set (including `none`), this overrides the
    /// Plan preset. The `none` value means "no reasoning" (not "inherit the
    /// global default").
    pub plan_mode_reasoning_effort: Option<ReasoningEffort>,

    /// Optional value to use for `reasoning.summary` when making a request
    /// using the Responses API. When unset, the model catalog default is used.
    pub model_reasoning_summary: Option<ReasoningSummary>,

    /// Optional full model catalog loaded from `model_catalog_json`.
    /// When set, this replaces the bundled catalog for the current process.
    pub model_catalog: Option<ModelsResponse>,

    /// Optional verbosity control for GPT-5 models (Responses API `text.verbosity`).
    pub model_verbosity: Option<Verbosity>,

    /// Whether AHEAD-owned clients should respect host system proxy settings.
    pub respect_system_proxy: bool,

    /// Bounded, product-owned metadata attached to every Responses API request.
    pub responses_api_metadata: BTreeMap<String, String>,

    /// Machine-local realtime audio device preferences used by realtime voice.
    pub realtime_audio: RealtimeAudioConfig,

    /// Experimental / do not use. Overrides only the realtime conversation
    /// websocket transport base URL (the `Op::RealtimeConversation`
    /// `/v1/realtime`
    /// connection) without changing normal provider HTTP requests.
    pub experimental_realtime_ws_base_url: Option<String>,
    /// Experimental / do not use. Overrides only the WebRTC realtime call
    /// creation base URL.
    pub experimental_realtime_webrtc_call_base_url: Option<String>,
    /// Experimental / do not use. Selects the realtime websocket model/snapshot
    /// used for the `Op::RealtimeConversation` connection.
    pub experimental_realtime_ws_model: Option<String>,
    /// Experimental / do not use. Realtime websocket session selection.
    /// `version` controls v1/v2 and `type` controls conversational/transcription.
    pub realtime: RealtimeConfig,
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
    /// Explicit or feature-derived web search mode.
    pub web_search_mode: Constrained<WebSearchMode>,

    /// Additional parameters for the web search tool when it is enabled.
    pub web_search_config: Option<WebSearchConfig>,

    /// Whether to register the experimental request_user_input tool.
    pub experimental_request_user_input_enabled: bool,

    /// Whether to register the update_plan tool.
    pub update_plan_enabled: bool,

    /// Policy for collecting and validating tool runtimes.
    pub tool_registry: ToolRegistryConfig,

    /// Configuration for the experimental code-mode tool surface.
    pub code_mode: CodeModeConfig,

    /// Maximum poll window for background terminal output (`write_stdin`), in milliseconds.
    /// Default: `300000` (5 minutes).
    pub background_terminal_max_timeout: u64,

    /// Settings for managed child-thread admission.
    pub multi_agent_v2: MultiAgentV2Config,

    /// Context-window token budget configuration, when enabled.
    pub token_budget: Option<TokenBudgetConfig>,
    /// Shared token budget for the root thread and its sub-agents.
    pub rollout_budget: Option<RolloutBudgetConfig>,
    /// Current-time reminder and clock tool configuration, when enabled.
    pub current_time_reminder: Option<CurrentTimeReminderConfig>,
    /// How the sleep tool is selected when its feature gate is enabled.
    pub sleep_tool_mode: SleepToolMode,

    /// Centralized feature flags; source of truth for feature gating.
    pub features: ManagedFeatures,

    /// When `true`, suppress warnings about unstable (under development) features.
    pub suppress_unstable_features_warning: bool,

    /// When `true`, checks for Codex updates on startup and surfaces update prompts.
    /// Set to `false` only if your Codex updates are centrally managed.
    /// Defaults to `true`.
    /// OTEL configuration (exporter type, endpoint, headers, etc.).
    pub otel: codex_config::types::OtelConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ToolRegistryConfig {
    /// Fail the turn when multiple tools share the same effective name.
    pub error_on_tool_collisions: bool,
    /// Include authoritative tool information in per-turn request metadata.
    pub turn_metadata_includes_tool_info: bool,
}

const DEFAULT_CODE_MODE_EXEC_YIELD_TIME_MS: u64 = 30_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodeModeConfig {
    pub default_exec_yield_time_ms: u64,
    pub excluded_tool_namespaces: Vec<String>,
    pub direct_only_tool_namespaces: Vec<String>,
    /// Keep code mode fail-closed when the standalone host is unavailable.
    pub disable_in_process_fallback: bool,
}

impl Default for CodeModeConfig {
    fn default() -> Self {
        Self {
            default_exec_yield_time_ms: DEFAULT_CODE_MODE_EXEC_YIELD_TIME_MS,
            excluded_tool_namespaces: Vec::new(),
            direct_only_tool_namespaces: Vec::new(),
            disable_in_process_fallback: false,
        }
    }
}

pub(crate) const DEFAULT_TOKEN_BUDGET_REMINDER_MESSAGE_TEMPLATE: &str = concat!(
    "Your context window is nearly exhausted (only {n_remaining} tokens remaining) and will be automatically reset for you soon. ",
    "Once reset, message items in current context window will be cleared in the new window, but notes and history items will be persistent across windows."
);
const TOKEN_BUDGET_REMINDER_MESSAGE_TEMPLATE_MAX_BYTES: usize = 2000;
const TOKEN_BUDGET_GUIDANCE_MESSAGE_MAX_BYTES: usize = 2000;
const AUTO_COMPACT_FALLBACK_PROMPT_MAX_BYTES: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TokenBudgetConfig {
    pub use_history_notes_extension: bool,
    pub reminder_threshold_tokens: Option<i64>,
    pub reminder_message_template: String,
    pub guidance_message: Option<String>,
    pub auto_compact_fallback_prompt: Option<String>,
    pub auto_compact_fallback_buffer_tokens: Option<i64>,
}

impl TokenBudgetConfig {
    pub(crate) fn validate(&self) -> std::io::Result<()> {
        if self
            .reminder_threshold_tokens
            .is_some_and(|tokens| tokens <= 0)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "features.token_budget.reminder_threshold_tokens must be positive",
            ));
        }

        if self.reminder_message_template.trim().is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "features.token_budget.reminder_message_template must not be empty",
            ));
        }
        if self.reminder_message_template.len() > TOKEN_BUDGET_REMINDER_MESSAGE_TEMPLATE_MAX_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "features.token_budget.reminder_message_template must not exceed {TOKEN_BUDGET_REMINDER_MESSAGE_TEMPLATE_MAX_BYTES} bytes"
                ),
            ));
        }

        if self
            .guidance_message
            .as_ref()
            .is_some_and(|message| message.len() > TOKEN_BUDGET_GUIDANCE_MESSAGE_MAX_BYTES)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "features.token_budget.guidance_message must not exceed {TOKEN_BUDGET_GUIDANCE_MESSAGE_MAX_BYTES} bytes"
                ),
            ));
        }

        if self
            .auto_compact_fallback_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.len() > AUTO_COMPACT_FALLBACK_PROMPT_MAX_BYTES)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "features.token_budget.auto_compact_fallback_prompt must not exceed {AUTO_COMPACT_FALLBACK_PROMPT_MAX_BYTES} bytes"
                ),
            ));
        }
        if self.auto_compact_fallback_prompt.is_some()
            && self.auto_compact_fallback_buffer_tokens.is_none()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "features.token_budget.auto_compact_fallback_buffer_tokens is required when auto_compact_fallback_prompt is set",
            ));
        }
        if self
            .auto_compact_fallback_buffer_tokens
            .is_some_and(|tokens| tokens <= 0)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "features.token_budget.auto_compact_fallback_buffer_tokens must be positive",
            ));
        }

        Ok(())
    }

    pub(crate) fn fallback_buffer_tokens(&self) -> i64 {
        if self.auto_compact_fallback_prompt.is_some() {
            self.auto_compact_fallback_buffer_tokens.unwrap_or(0)
        } else {
            0
        }
    }
}

impl Default for TokenBudgetConfig {
    fn default() -> Self {
        Self {
            use_history_notes_extension: false,
            reminder_threshold_tokens: None,
            reminder_message_template: DEFAULT_TOKEN_BUDGET_REMINDER_MESSAGE_TEMPLATE.to_string(),
            guidance_message: None,
            auto_compact_fallback_prompt: None,
            auto_compact_fallback_buffer_tokens: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RolloutBudgetConfig {
    pub limit_tokens: i64,
    pub reminder_at_remaining_tokens: Vec<i64>,
    pub sampling_token_weight: f64,
    pub prefill_token_weight: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CurrentTimeReminderConfig {
    pub reminder_interval_seconds: u64,
    pub clock_source: CurrentTimeSource,
    pub delivery_mode: CurrentTimeReminderDeliveryMode,
    /// Whether to expose the input-interruptible `clock.sleep` tool.
    pub sleep_tool: bool,
}

impl Default for CurrentTimeReminderConfig {
    fn default() -> Self {
        Self {
            reminder_interval_seconds: 1,
            clock_source: CurrentTimeSource::System,
            delivery_mode: CurrentTimeReminderDeliveryMode::AnyInference,
            sleep_tool: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MultiAgentV2Config {
    pub max_concurrent_threads_per_session: usize,
}

impl MultiAgentV2Config {
    fn defaults_for_max_concurrency(max_concurrent_threads_per_session: usize) -> Self {
        Self {
            max_concurrent_threads_per_session,
        }
    }
}

impl Default for MultiAgentV2Config {
    fn default() -> Self {
        Self::defaults_for_max_concurrency(
            DEFAULT_MULTI_AGENT_V2_MAX_CONCURRENT_THREADS_PER_SESSION,
        )
    }
}

#[derive(Clone, Default)]
pub struct ConfigBuilder {
    codex_home: Option<PathBuf>,
    cli_overrides: Option<Vec<(String, TomlValue)>>,
    harness_overrides: Option<ConfigOverrides>,
    loader_overrides: Option<LoaderOverrides>,
    strict_config: bool,
    fallback_cwd: Option<PathBuf>,
}

impl ConfigBuilder {
    pub fn codex_home(mut self, codex_home: PathBuf) -> Self {
        self.codex_home = Some(codex_home);
        self
    }

    pub fn cli_overrides(mut self, cli_overrides: Vec<(String, TomlValue)>) -> Self {
        self.cli_overrides = Some(cli_overrides);
        self
    }

    pub fn harness_overrides(mut self, harness_overrides: ConfigOverrides) -> Self {
        self.harness_overrides = Some(harness_overrides);
        self
    }

    pub fn loader_overrides(mut self, loader_overrides: LoaderOverrides) -> Self {
        self.loader_overrides = Some(loader_overrides);
        self
    }

    pub fn strict_config(mut self, strict_config: bool) -> Self {
        self.strict_config = strict_config;
        self
    }

    pub fn fallback_cwd(mut self, fallback_cwd: Option<PathBuf>) -> Self {
        self.fallback_cwd = fallback_cwd;
        self
    }

    pub async fn build(self) -> std::io::Result<Config> {
        // Keep the large config-loading future off small runtime thread stacks.
        Box::pin(self.build_inner()).await
    }

    async fn build_inner(self) -> std::io::Result<Config> {
        let Self {
            codex_home,
            cli_overrides,
            harness_overrides,
            loader_overrides,
            strict_config,
            fallback_cwd,
        } = self;
        let codex_home = match codex_home {
            Some(codex_home) => AbsolutePathBuf::from_absolute_path(codex_home)?,
            None => find_ahead_home()?,
        };
        let cli_overrides = cli_overrides.unwrap_or_default();
        let mut harness_overrides = harness_overrides.unwrap_or_default();
        let loader_overrides = loader_overrides.unwrap_or_default();
        let cwd_override = harness_overrides.cwd.as_deref().or(fallback_cwd.as_deref());
        let cwd = match cwd_override {
            Some(path) => AbsolutePathBuf::relative_to_current_dir(path)?,
            None => AbsolutePathBuf::current_dir()?,
        };
        harness_overrides.cwd = Some(cwd.to_path_buf());
        let config_layer_stack = load_config_layers_state(
            LOCAL_FS.as_ref(),
            &codex_home,
            Some(cwd),
            &cli_overrides,
            ConfigLoadOptions {
                loader_overrides,
                strict_config,
            },
        )
        .await?;
        let merged_toml = config_layer_stack.effective_config();

        // Note that each layer in ConfigLayerStack should have resolved
        // relative paths to absolute paths based on the parent folder of the
        // respective config file, so we should be safe to deserialize without
        // AbsolutePathBufGuard here.
        let config_toml: ConfigToml = match merged_toml.try_into() {
            Ok(config_toml) => config_toml,
            Err(err) => {
                if let Some(config_error) =
                    codex_config::first_layer_config_error::<ConfigToml>(&config_layer_stack).await
                {
                    return Err(codex_config::io_error_from_config_error(
                        std::io::ErrorKind::InvalidData,
                        config_error,
                        Some(err),
                    ));
                }
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, err));
            }
        };
        Config::load_config_with_layer_stack(
            LOCAL_FS.as_ref(),
            config_toml,
            harness_overrides,
            codex_home,
            config_layer_stack,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) fn isolated_for_tests() -> Self {
        Self::default().loader_overrides(LoaderOverrides::isolated_for_tests())
    }
}

impl Config {
    pub(crate) fn multi_agent_version_override(&self) -> Option<MultiAgentVersion> {
        if self.features.enabled(Feature::MultiAgentV2) {
            Some(MultiAgentVersion::V2)
        } else {
            None
        }
    }

    pub(crate) fn multi_agent_version_from_features(&self) -> MultiAgentVersion {
        self.multi_agent_version_override().unwrap_or_else(|| {
            if self.features.enabled(Feature::Collab) {
                MultiAgentVersion::V1
            } else {
                MultiAgentVersion::Disabled
            }
        })
    }

    pub(crate) fn multi_agent_version_for_model(
        &self,
        model_multi_agent_version: Option<MultiAgentVersion>,
    ) -> MultiAgentVersion {
        self.multi_agent_version_override()
            .or(model_multi_agent_version)
            .unwrap_or_else(|| self.multi_agent_version_from_features())
    }

    pub(crate) fn effective_agent_max_threads(
        &self,
        multi_agent_version: MultiAgentVersion,
    ) -> Option<usize> {
        match multi_agent_version {
            MultiAgentVersion::V2 => Some(
                self.multi_agent_v2
                    .max_concurrent_threads_per_session
                    .saturating_sub(1),
            ),
            MultiAgentVersion::Disabled | MultiAgentVersion::V1 => None,
        }
    }

    pub fn legacy_sandbox_policy(&self) -> SandboxPolicy {
        self.permissions.legacy_sandbox_policy(self.cwd.as_path())
    }

    pub fn set_legacy_sandbox_policy(
        &mut self,
        sandbox_policy: SandboxPolicy,
    ) -> ConstraintResult<()> {
        self.workspace_roots_explicit = matches!(
            &sandbox_policy,
            SandboxPolicy::WorkspaceWrite { writable_roots, .. } if !writable_roots.is_empty()
        );
        self.permissions
            .set_legacy_sandbox_policy(sandbox_policy, self.cwd.as_path())?;
        self.workspace_roots = self.permissions.workspace_roots().to_vec();
        Ok(())
    }

    pub fn effective_workspace_roots(&self) -> Vec<AbsolutePathBuf> {
        let mut workspace_roots = self.workspace_roots.clone();
        workspace_roots.extend(self.permissions.profile_workspace_roots().iter().cloned());
        dedupe_absolute_paths(&mut workspace_roots);
        workspace_roots
    }

    pub fn to_models_manager_config(&self) -> ModelsManagerConfig {
        ModelsManagerConfig {
            model_context_window: self.model_context_window,
            model_auto_compact_token_limit: self.model_auto_compact_token_limit,
            tool_output_token_limit: self.tool_output_token_limit,
            base_instructions: self.base_instructions.clone().filter(|_| {
                !matches!(
                    self.base_instructions_provenance,
                    Some(BaseInstructionsProvenance::Model { .. })
                )
            }),
            personality_enabled: self.features.enabled(Feature::Personality),
            personality: self.personality,
            model_catalog: self.model_catalog.clone(),
        }
    }

    /// Creates the HTTP client factory resolved from the effective feature configuration.
    pub fn http_client_factory(&self) -> HttpClientFactory {
        let outbound_proxy_policy = if self.respect_system_proxy {
            OutboundProxyPolicy::RespectSystemProxy
        } else {
            OutboundProxyPolicy::ReqwestDefault
        };
        HttpClientFactory::new(outbound_proxy_policy)
    }

    pub(crate) fn to_ahead_mcp_config(&self) -> McpConfig {
        let mut catalog = ResolvedMcpCatalog::builder();
        for (name, server) in self.mcp_servers.get() {
            catalog.register(McpServerRegistration::from_config(
                name.clone(),
                server.clone(),
            ));
        }

        McpConfig {
            optional_mcp_startup_grace: self.mcp_optional_startup_grace,
            approval_policy: self.permissions.approval_policy.clone(),
            permission_profile: self.permissions.permission_profile().clone(),
            environment_cwds: HashMap::new(),
            server_permission_profiles: HashMap::new(),
            ahead_linux_sandbox_exe: self.ahead_linux_sandbox_exe.clone(),
            use_legacy_landlock: self.features.use_legacy_landlock(),
            prefix_mcp_tool_names: self.prefix_mcp_tool_names(),
            non_prefixed_mcp_tool_servers: if self
                .features
                .enabled(Feature::NonPrefixedMcpToolNames)
            {
                self.non_prefixed_mcp_tool_servers
                    .clone()
                    .unwrap_or_default()
            } else {
                Vec::new()
            },
            protocol_mode: self.mcp_protocol_mode(),
            client_elicitation_capability: ElicitationCapability::new()
                .with_form(rmcp::model::FormElicitationCapability::new())
                .with_url(rmcp::model::UrlElicitationCapability::new()),
            mcp_server_catalog: catalog.build(),
        }
    }

    pub(crate) fn prefix_mcp_tool_names(&self) -> bool {
        !self.features.enabled(Feature::NonPrefixedMcpToolNames)
            || self.non_prefixed_mcp_tool_servers.is_some()
    }

    pub fn mcp_protocol_mode(&self) -> McpProtocolMode {
        if self.features.enabled(Feature::Mcp20260728) {
            McpProtocolMode::V20260728
        } else {
            McpProtocolMode::Legacy
        }
    }

    pub async fn rebuild_preserving_session_layers(
        &self,
        refreshed_config: &Config,
    ) -> std::io::Result<Self> {
        let mut layers = refreshed_config
            .config_layer_stack
            .all_layers_low_to_high()
            .filter(|layer| !is_session_layer(&layer.name))
            .cloned()
            .collect::<Vec<_>>();
        layers.extend(
            self.config_layer_stack
                .all_layers_low_to_high()
                .filter(|layer| is_session_layer(&layer.name))
                .cloned(),
        );
        layers.sort_by_key(|layer| layer.name.precedence());

        let config_layer_stack = ConfigLayerStack::new(
            layers,
            refreshed_config.config_layer_stack.requirements().clone(),
            refreshed_config
                .config_layer_stack
                .requirements_toml()
                .clone(),
        )?
        .with_user_exec_policy_rules_ignored(
            refreshed_config
                .config_layer_stack
                .ignore_user_exec_policy_rules(),
        );
        let cfg: ConfigToml = config_layer_stack
            .effective_config()
            .try_into()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        Self::load_config_with_layer_stack(
            LOCAL_FS.as_ref(),
            cfg,
            ConfigOverrides {
                cwd: Some(self.cwd.to_path_buf()),
                ..Default::default()
            },
            refreshed_config.codex_home.clone(),
            config_layer_stack,
        )
        .await
    }
}

pub fn deserialize_config_toml_with_base(
    root_value: TomlValue,
    config_base_dir: &Path,
) -> std::io::Result<ConfigToml> {
    // This guard ensures that any relative paths that is deserialized into an
    // [AbsolutePathBuf] is resolved against `config_base_dir`.
    let _guard = AbsolutePathBufGuard::new(config_base_dir);
    root_value
        .try_into()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Validate user-visible feature settings against managed feature requirements.
pub fn validate_feature_requirements_for_config_toml(
    cfg: &ConfigToml,
    feature_requirements: Option<&Sourced<FeatureRequirementsToml>>,
) -> std::io::Result<()> {
    managed_features::validate_explicit_feature_settings_in_config_toml(cfg, feature_requirements)?;
    managed_features::validate_feature_requirements_in_config_toml(cfg, feature_requirements)
}

fn load_catalog_json(path: &AbsolutePathBuf) -> std::io::Result<ModelsResponse> {
    let file_contents = std::fs::read_to_string(path)?;
    let catalog = serde_json::from_str::<ModelsResponse>(&file_contents).map_err(|err| {
        std::io::Error::new(
            ErrorKind::InvalidData,
            format!(
                "failed to parse model_catalog_json path `{}` as JSON: {err}",
                path.display()
            ),
        )
    })?;
    if catalog.models.is_empty() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!(
                "model_catalog_json path `{}` must contain at least one model",
                path.display()
            ),
        ));
    }
    Ok(catalog)
}

fn load_model_catalog(
    model_catalog_json: Option<AbsolutePathBuf>,
) -> std::io::Result<Option<ModelsResponse>> {
    model_catalog_json
        .map(|path| load_catalog_json(&path))
        .transpose()
}

fn filter_mcp_servers_by_requirements(
    mcp_servers: &mut HashMap<String, McpServerConfig>,
    mcp_requirements: Option<&Sourced<BTreeMap<String, McpServerRequirement>>>,
) {
    let Some(allowlist) = mcp_requirements else {
        return;
    };

    let source = allowlist.source.clone();
    for (name, server) in mcp_servers.iter_mut() {
        let allowed = allowlist
            .value
            .get(name)
            .is_some_and(|requirement| server.matches_requirement(requirement));
        if allowed {
            server.disabled_reason = None;
        } else {
            server.enabled = false;
            server.disabled_reason = Some(McpServerDisabledReason::Requirements {
                source: source.clone(),
            });
        }
    }
}

fn constrain_mcp_servers(
    mcp_servers: HashMap<String, McpServerConfig>,
    mcp_requirements: Option<&Sourced<BTreeMap<String, McpServerRequirement>>>,
) -> ConstraintResult<Constrained<HashMap<String, McpServerConfig>>> {
    if mcp_requirements.is_none() {
        return Ok(Constrained::allow_any(mcp_servers));
    }

    let mcp_requirements = mcp_requirements.cloned();
    Constrained::normalized(mcp_servers, move |mut servers| {
        filter_mcp_servers_by_requirements(&mut servers, mcp_requirements.as_ref());
        servers
    })
}

fn apply_requirement_constrained_value<T>(
    field_name: &'static str,
    configured_value: T,
    constrained_value: &mut ConstrainedWithSource<T>,
    startup_warnings: &mut Vec<String>,
) -> std::io::Result<bool>
where
    T: Clone + std::fmt::Debug + Send + Sync,
{
    if let Err(err) = constrained_value.set(configured_value) {
        let fallback_value = constrained_value.get().clone();
        tracing::warn!(
            error = %err,
            ?fallback_value,
            requirement_source = ?constrained_value.source,
            "configured value is disallowed by requirements; falling back to required value for {field_name}"
        );
        let message = format!(
            "Configured value for `{field_name}` is disallowed by requirements; falling back to required value {fallback_value:?}. Details: {err}"
        );
        startup_warnings.push(message);

        constrained_value.set(fallback_value).map_err(|fallback_err| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "configured value for `{field_name}` is disallowed by requirements ({err}); fallback to a requirement-compliant value also failed ({fallback_err})"
                ),
            )
        })?;
        return Ok(true);
    }

    Ok(false)
}

fn is_session_layer(source: &ConfigLayerSource) -> bool {
    matches!(source, ConfigLayerSource::SessionFlags)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PermissionConfigSyntax {
    Legacy,
    Profiles,
}

#[derive(Debug, Deserialize, Default)]
struct PermissionSelectionToml {
    default_permissions: Option<String>,
    sandbox_mode: Option<SandboxMode>,
}

// Resolve the named-profile catalog and selected profile id together. Runtime
// profile constraints are applied later after this selection compiles into a
// concrete `PermissionProfile`.
#[derive(Debug)]
struct EffectivePermissionSelection<'a> {
    profiles: Option<PermissionsToml>,
    selected_profile_id: Option<&'a str>,
    persisted_profile_id_was_provided: bool,
    requirements_force_profile_selection: bool,
}

impl EffectivePermissionSelection<'_> {
    fn has_profiles(&self) -> bool {
        self.profiles
            .as_ref()
            .is_some_and(|profiles| !profiles.is_empty())
    }

    fn profiles_are_active(
        &self,
        default_permissions_override: Option<&str>,
        permission_config_syntax: Option<PermissionConfigSyntax>,
    ) -> bool {
        self.persisted_profile_id_was_provided
            || self.requirements_force_profile_selection
            || default_permissions_override.is_some()
            || matches!(
                permission_config_syntax,
                Some(PermissionConfigSyntax::Profiles)
            )
            || permission_config_syntax.is_none()
    }
}

fn resolve_permission_config_syntax(
    config_layer_stack: &ConfigLayerStack,
    cfg: &ConfigToml,
    sandbox_mode_override: Option<SandboxMode>,
) -> Option<PermissionConfigSyntax> {
    if sandbox_mode_override.is_some() {
        return Some(PermissionConfigSyntax::Legacy);
    }

    let session_flags_select_profiles = config_layer_stack
        .layers_high_to_low()
        .find(|layer| matches!(layer.name, ConfigLayerSource::SessionFlags))
        .and_then(|layer| {
            layer
                .config
                .clone()
                .try_into::<PermissionSelectionToml>()
                .ok()
        })
        .is_some_and(|selection| selection.default_permissions.is_some());
    if session_flags_select_profiles {
        return Some(PermissionConfigSyntax::Profiles);
    }

    let mut selection = None;
    for layer in config_layer_stack.layers_low_to_high() {
        let Ok(layer_selection) = layer.config.clone().try_into::<PermissionSelectionToml>() else {
            continue;
        };

        if layer_selection.sandbox_mode.is_some() {
            selection = Some(PermissionConfigSyntax::Legacy);
        }
        if layer_selection.default_permissions.is_some() {
            selection = Some(PermissionConfigSyntax::Profiles);
        }
    }

    selection.or_else(|| {
        if cfg.default_permissions.is_some() {
            Some(PermissionConfigSyntax::Profiles)
        } else if cfg.sandbox_mode.is_some() {
            Some(PermissionConfigSyntax::Legacy)
        } else {
            None
        }
    })
}

fn apply_managed_filesystem_constraints(
    file_system_sandbox_policy: &mut FileSystemSandboxPolicy,
    filesystem_constraints: &codex_config::FilesystemConstraints,
) {
    for deny_read in &filesystem_constraints.deny_read {
        let deny_entry = if deny_read.contains_glob() {
            codex_protocol::permissions::FileSystemSandboxEntry {
                path: codex_protocol::permissions::FileSystemPath::GlobPattern {
                    pattern: deny_read.as_str().to_string(),
                },
                access: codex_protocol::permissions::FileSystemAccessMode::Deny,
                missing_path_behavior: None,
            }
        } else {
            let Ok(path) = AbsolutePathBuf::try_from(deny_read.as_str()) else {
                continue;
            };
            codex_protocol::permissions::FileSystemSandboxEntry {
                path: path.into(),
                access: codex_protocol::permissions::FileSystemAccessMode::Deny,
                missing_path_behavior: None,
            }
        };
        if !file_system_sandbox_policy
            .entries
            .iter()
            .any(|existing| existing == &deny_entry)
        {
            file_system_sandbox_policy.entries.push(deny_entry);
        }
    }
}

/// Optional overrides for user configuration (e.g., from CLI flags).
#[derive(Default, Debug, Clone)]
pub struct ConfigOverrides {
    pub model: Option<String>,
    pub cwd: Option<PathBuf>,
    pub approval_policy: Option<AskForApproval>,
    pub sandbox_mode: Option<SandboxMode>,
    pub permission_profile: Option<PermissionProfile>,
    pub default_permissions: Option<String>,
    /// Permission profile ID recovered from persisted thread state. Explicit
    /// permission overrides take precedence, and stale profile IDs fall back
    /// to the configured default.
    pub persisted_permission_profile_id: Option<String>,
    pub model_provider: Option<String>,
    pub service_tier: Option<Option<String>>,
    pub ahead_self_exe: Option<PathBuf>,
    pub ahead_linux_sandbox_exe: Option<PathBuf>,
    pub base_instructions: Option<String>,
    pub developer_instructions: Option<String>,
    pub personality: Option<Personality>,
    pub compact_prompt: Option<String>,
    pub show_raw_agent_reasoning: Option<bool>,
    pub tools_web_search_request: Option<bool>,
    pub ephemeral: Option<bool>,
    /// Additional directories that should be treated as writable roots for this session.
    pub additional_writable_roots: Vec<PathBuf>,
    /// Explicit absolute runtime workspace roots for this session. When set,
    /// this is the full runtime root list rather than an additive override.
    pub workspace_roots: Option<Vec<AbsolutePathBuf>>,
}

fn dedupe_absolute_paths(paths: &mut Vec<AbsolutePathBuf>) {
    let mut seen = HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
}

/// Resolve the web search mode from explicit config and feature flags.
fn resolve_web_search_mode(config_toml: &ConfigToml, features: &Features) -> Option<WebSearchMode> {
    if let Some(mode) = config_toml.web_search {
        return Some(mode);
    }
    if features.enabled(Feature::WebSearchCached) {
        return Some(WebSearchMode::Cached);
    }
    if features.enabled(Feature::WebSearchRequest) {
        return Some(WebSearchMode::Live);
    }
    None
}

fn resolve_web_search_config(config_toml: &ConfigToml) -> Option<WebSearchConfig> {
    config_toml
        .tools
        .as_ref()
        .and_then(|tools| tools.web_search.as_ref())
        .cloned()
        .map(Into::into)
}

fn resolve_experimental_request_user_input_enabled(config_toml: &ConfigToml) -> bool {
    config_toml
        .tools
        .as_ref()
        .and_then(|tools| tools.experimental_request_user_input.as_ref())
        .is_none_or(|config| config.enabled)
}

fn resolve_update_plan_enabled(config_toml: &ConfigToml) -> bool {
    config_toml
        .tools
        .as_ref()
        .and_then(|tools| tools.update_plan.as_ref())
        .is_some_and(|config| config.enabled)
}

fn resolve_orchestrator_feature_enabled(
    feature: Option<&codex_config::config_toml::OrchestratorFeatureToml>,
) -> bool {
    feature.and_then(|feature| feature.enabled).unwrap_or(true)
}

fn resolve_code_mode_config(config_toml: &ConfigToml) -> CodeModeConfig {
    let base = code_mode_toml_config(config_toml.features.as_ref());
    let host = config_toml
        .features
        .as_ref()
        .and_then(|features| features.code_mode_host.as_ref())
        .and_then(|feature| match feature {
            FeatureToml::Enabled(_) => None,
            FeatureToml::Config(config) => Some(config),
        });

    CodeModeConfig {
        default_exec_yield_time_ms: base
            .and_then(|config| config.default_exec_yield_time_ms)
            .unwrap_or(DEFAULT_CODE_MODE_EXEC_YIELD_TIME_MS),
        excluded_tool_namespaces: base
            .and_then(|config| config.excluded_tool_namespaces.as_ref())
            .cloned()
            .unwrap_or_default(),
        direct_only_tool_namespaces: base
            .and_then(|config| config.direct_only_tool_namespaces.as_ref())
            .cloned()
            .unwrap_or_default(),
        disable_in_process_fallback: host
            .and_then(|config| config.disable_in_process_fallback)
            .unwrap_or_default(),
    }
}

fn resolve_multi_agent_v2_config(config_toml: &ConfigToml) -> MultiAgentV2Config {
    let base = multi_agent_v2_toml_config(config_toml.features.as_ref());
    let max_concurrent_threads_per_session = base
        .and_then(|config| config.max_concurrent_threads_per_session)
        .unwrap_or(DEFAULT_MULTI_AGENT_V2_MAX_CONCURRENT_THREADS_PER_SESSION);
    MultiAgentV2Config::defaults_for_max_concurrency(max_concurrent_threads_per_session)
}

fn resolve_token_budget_config(
    config_toml: &ConfigToml,
    features: &ManagedFeatures,
) -> std::io::Result<Option<TokenBudgetConfig>> {
    if !features.enabled(Feature::TokenBudget) {
        return Ok(None);
    }

    let token_budget_config = token_budget_toml_config(config_toml.features.as_ref());
    let use_history_notes_extension = token_budget_config
        .and_then(|config| config.use_history_notes_extension)
        .unwrap_or_default();
    let reminder_threshold_tokens =
        token_budget_config.and_then(|config| config.reminder_threshold_tokens);
    let reminder_message_template = token_budget_config
        .and_then(|config| config.reminder_message_template.clone())
        .unwrap_or_else(|| DEFAULT_TOKEN_BUDGET_REMINDER_MESSAGE_TEMPLATE.to_string());
    let guidance_message = token_budget_config
        .and_then(|config| config.guidance_message.clone())
        .filter(|message| !message.trim().is_empty());
    let auto_compact_fallback_prompt = token_budget_config
        .and_then(|config| config.auto_compact_fallback_prompt.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let auto_compact_fallback_buffer_tokens =
        token_budget_config.and_then(|config| config.auto_compact_fallback_buffer_tokens);

    let token_budget = TokenBudgetConfig {
        use_history_notes_extension,
        reminder_threshold_tokens,
        reminder_message_template,
        guidance_message,
        auto_compact_fallback_prompt,
        auto_compact_fallback_buffer_tokens,
    };
    token_budget.validate()?;
    Ok(Some(token_budget))
}

fn resolve_rollout_budget_config(
    config_toml: &ConfigToml,
    features: &ManagedFeatures,
) -> std::io::Result<Option<RolloutBudgetConfig>> {
    if !features.enabled(Feature::RolloutBudget) {
        return Ok(None);
    }
    let missing_limit_error = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "features.rollout_budget.limit_tokens is required when rollout_budget is enabled",
        )
    };
    let Some(FeatureToml::Config(config)) = config_toml
        .features
        .as_ref()
        .and_then(|features| features.rollout_budget.as_ref())
    else {
        return Err(missing_limit_error());
    };
    let Some(limit_tokens) = config.limit_tokens else {
        return Err(missing_limit_error());
    };
    if limit_tokens <= 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "features.rollout_budget.limit_tokens must be positive",
        ));
    }
    let reminder_at_remaining_tokens =
        config
            .reminder_at_remaining_tokens
            .clone()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "features.rollout_budget.reminder_at_remaining_tokens is required when rollout_budget is enabled",
                )
            })?;
    if reminder_at_remaining_tokens
        .iter()
        .any(|&tokens| tokens <= 0 || tokens >= limit_tokens)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "features.rollout_budget.reminder_at_remaining_tokens must contain only positive values below limit_tokens",
        ));
    }
    let sampling_token_weight = config.sampling_token_weight.unwrap_or(1.0);
    let prefill_token_weight = config.prefill_token_weight.unwrap_or(1.0);
    for (field, weight) in [
        ("sampling_token_weight", sampling_token_weight),
        ("prefill_token_weight", prefill_token_weight),
    ] {
        if !weight.is_finite() || weight < 0.0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("features.rollout_budget.{field} must be finite and non-negative"),
            ));
        }
    }
    Ok(Some(RolloutBudgetConfig {
        limit_tokens,
        reminder_at_remaining_tokens,
        sampling_token_weight,
        prefill_token_weight,
    }))
}

fn resolve_current_time_reminder_config(
    config_toml: &ConfigToml,
    features: &ManagedFeatures,
) -> std::io::Result<Option<CurrentTimeReminderConfig>> {
    if !features.enabled(Feature::CurrentTimeReminder) {
        return Ok(None);
    }

    let base = current_time_reminder_toml_config(config_toml.features.as_ref());
    let default = CurrentTimeReminderConfig::default();
    let reminder_interval_seconds = base
        .and_then(|config| config.reminder_interval_seconds)
        .unwrap_or(default.reminder_interval_seconds);

    Ok(Some(CurrentTimeReminderConfig {
        reminder_interval_seconds,
        clock_source: base
            .and_then(|config| config.clock_source)
            .unwrap_or(default.clock_source),
        delivery_mode: base
            .and_then(|config| config.delivery_mode)
            .unwrap_or(default.delivery_mode),
        sleep_tool: base
            .and_then(|config| config.sleep_tool)
            .unwrap_or(default.sleep_tool),
    }))
}

fn code_mode_toml_config(features: Option<&FeaturesToml>) -> Option<&CodeModeConfigToml> {
    match features?.code_mode.as_ref()? {
        FeatureToml::Enabled(_) => None,
        FeatureToml::Config(config) => Some(config),
    }
}

fn multi_agent_v2_toml_config(features: Option<&FeaturesToml>) -> Option<&MultiAgentV2ConfigToml> {
    match features?.multi_agent_v2.as_ref()? {
        FeatureToml::Enabled(_) => None,
        FeatureToml::Config(config) => Some(config),
    }
}

fn token_budget_toml_config(features: Option<&FeaturesToml>) -> Option<&TokenBudgetConfigToml> {
    match features?.token_budget.as_ref()? {
        FeatureToml::Enabled(_) => None,
        FeatureToml::Config(config) => Some(config),
    }
}

fn current_time_reminder_toml_config(
    features: Option<&FeaturesToml>,
) -> Option<&CurrentTimeReminderConfigToml> {
    match features?.current_time_reminder.as_ref()? {
        FeatureToml::Enabled(_) => None,
        FeatureToml::Config(config) => Some(config),
    }
}

pub(crate) fn resolve_web_search_mode_for_turn(
    web_search_mode: &Constrained<WebSearchMode>,
    permission_profile: &PermissionProfile,
    provider_capabilities: ProviderCapabilities,
) -> WebSearchMode {
    let preferred = web_search_mode.value();
    let is_allowed = |mode: WebSearchMode| {
        let provider_supports_mode = match mode {
            WebSearchMode::Live | WebSearchMode::Indexed => {
                provider_capabilities.external_web_access
            }
            WebSearchMode::Cached | WebSearchMode::Disabled => true,
        };

        provider_supports_mode && web_search_mode.can_set(&mode).is_ok()
    };

    if matches!(permission_profile, PermissionProfile::Disabled)
        && !matches!(preferred, WebSearchMode::Disabled | WebSearchMode::Indexed)
    {
        for mode in [
            WebSearchMode::Live,
            WebSearchMode::Cached,
            WebSearchMode::Disabled,
        ] {
            if is_allowed(mode) {
                return mode;
            }
        }
    } else {
        if is_allowed(preferred) {
            return preferred;
        }
        for mode in [
            WebSearchMode::Cached,
            WebSearchMode::Live,
            WebSearchMode::Disabled,
        ] {
            if is_allowed(mode) {
                return mode;
            }
        }
    }

    WebSearchMode::Disabled
}

impl Config {
    #[cfg(test)]
    async fn load_from_base_config_with_overrides(
        cfg: ConfigToml,
        overrides: ConfigOverrides,
        codex_home: AbsolutePathBuf,
    ) -> std::io::Result<Self> {
        // Note this ignores requirements.toml enforcement for tests.
        let config_layer_stack = ConfigLayerStack::default();
        Self::load_config_with_layer_stack(
            LOCAL_FS.as_ref(),
            cfg,
            overrides,
            codex_home,
            config_layer_stack,
        )
        .await
    }

    pub(crate) async fn load_config_with_layer_stack(
        fs: &dyn ExecutorFileSystem,
        mut cfg: ConfigToml,
        overrides: ConfigOverrides,
        codex_home: AbsolutePathBuf,
        config_layer_stack: ConfigLayerStack,
    ) -> std::io::Result<Self> {
        // Keep the large config-construction future off small test thread stacks.
        Box::pin(async move {
        if cfg.experimental_thread_store.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "`experimental_thread_store` is no longer supported; managed sessions use Turso",
            ));
        }
        if cfg.experimental_thread_store_endpoint.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "`experimental_thread_store_endpoint` is no longer supported; remove it from config.toml",
            ));
        }

        validate_model_providers(&cfg.model_providers)
            .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))?;
        if let Some(responses_api_metadata) = cfg.responses_api_metadata.as_ref() {
            validate_extra_metadata(responses_api_metadata.iter()).map_err(|message| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, message)
            })?;
        }
        validate_managed_developer_instructions(
            config_layer_stack
                .requirements()
                .additional_developer_instructions
                .as_ref(),
        )?;
        let orchestrator = cfg.orchestrator.as_ref();
        let orchestrator_skills_enabled =
            resolve_orchestrator_feature_enabled(orchestrator.and_then(|value| value.skills.as_ref()));
        let mut startup_warnings = Vec::new();
        requirements::apply_to_config(
            &mut cfg,
            config_layer_stack.requirements(),
            &mut startup_warnings,
        );
        // Destructure every field to ensure ConfigRequirements additions are
        // either applied above or handled while constructing the final Config.
        let ConfigRequirements {
            log_dir: _,
            model_catalog_json: _,
            allow_login_shell: _,
            approval_policy: mut constrained_approval_policy,
            permission_profile: mut constrained_permission_profile,
            windows_sandbox_mode: mut constrained_windows_sandbox_mode,
            windows_sandbox_private_desktop: _,
            web_search_mode: mut constrained_web_search_mode,
            allow_appshots: _,
            allow_remote_control: _,
            computer_use: _,
            feature_requirements,
            mcp_servers,
            exec_policy: _,
            enforce_residency,
            filesystem: filesystem_requirements,
            additional_developer_instructions: _,
        } = config_layer_stack.requirements().clone();

        // Destructure ConfigOverrides fully to ensure all overrides are applied.
        let ConfigOverrides {
            model,
            cwd,
            approval_policy: approval_policy_override,
            sandbox_mode,
            permission_profile,
            default_permissions: default_permissions_override,
            persisted_permission_profile_id,
            model_provider,
            service_tier: service_tier_override,
            ahead_self_exe,
            ahead_linux_sandbox_exe,
            base_instructions,
            developer_instructions,
            personality,
            compact_prompt,
            show_raw_agent_reasoning,
            tools_web_search_request: override_tools_web_search_request,
            ephemeral,
            additional_writable_roots,
            workspace_roots: workspace_roots_override,
        } = overrides;

        if sandbox_mode.is_some() && permission_profile.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "`sandbox_mode` and `permission_profile` overrides cannot both be set",
            ));
        }
        if sandbox_mode.is_some() && default_permissions_override.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "`sandbox_mode` and `default_permissions` overrides cannot both be set",
            ));
        }
        if permission_profile.is_some() && default_permissions_override.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "`permission_profile` and `default_permissions` overrides cannot both be set",
            ));
        }
        let feature_overrides = FeatureOverrides {
            web_search_request: override_tools_web_search_request,
        };

        let configured_features = Features::from_sources(
            FeatureConfigSource {
                features: cfg.features.as_ref(),
            },
            FeatureConfigSource {
                ..Default::default()
            },
            feature_overrides,
        );
        let features = ManagedFeatures::from_configured_with_warnings(
            configured_features,
            feature_requirements,
            &mut startup_warnings,
        )?;
        let non_prefixed_mcp_tool_servers = if features.enabled(Feature::NonPrefixedMcpToolNames) {
            cfg.features
                .as_ref()
                .and_then(|features| features.non_prefixed_mcp_tool_names.as_ref())
                .and_then(|feature| match feature {
                    FeatureToml::Enabled(_) => None,
                    FeatureToml::Config(config) => config.server_names.clone(),
                })
        } else {
            None
        };
        let respect_system_proxy = features.enabled(Feature::RespectSystemProxy);
        let configured_windows_sandbox_mode = resolve_windows_sandbox_mode(&cfg);
        // Keep the configured mode separate so a requirement-constrained mode
        // does not look like it was explicitly selected in config.
        let selected_windows_sandbox_mode = configured_windows_sandbox_mode.or_else(|| {
            match WindowsSandboxLevel::from_features(&features) {
                WindowsSandboxLevel::Elevated => Some(WindowsSandboxModeToml::Elevated),
                WindowsSandboxLevel::RestrictedToken => Some(WindowsSandboxModeToml::Unelevated),
                WindowsSandboxLevel::Disabled => None,
            }
        });
        apply_requirement_constrained_value(
            "windows.sandbox",
            selected_windows_sandbox_mode,
            &mut constrained_windows_sandbox_mode,
            &mut startup_warnings,
        )?;
        let effective_windows_sandbox_mode = *constrained_windows_sandbox_mode.get();
        let windows_sandbox_mode = if constrained_windows_sandbox_mode.source.is_some() {
            effective_windows_sandbox_mode
        } else {
            configured_windows_sandbox_mode
        };
        let windows_sandbox_private_desktop = resolve_windows_sandbox_private_desktop(&cfg);
        let resolved_cwd = AbsolutePathBuf::try_from(normalize_for_native_workdir({
            use std::env;

            match cwd {
                None => {
                    tracing::info!("cwd not set, using current dir");
                    env::current_dir()?
                }
                Some(p) if p.is_absolute() => p,
                Some(p) => {
                    // Resolve relative path against the current working directory.
                    tracing::info!("cwd is relative, resolving against current dir");
                    let mut current = env::current_dir()?;
                    current.push(p);
                    current
                }
            }
        }))?;
        let requested_additional_writable_roots: Vec<AbsolutePathBuf> = additional_writable_roots
            .into_iter()
            .map(|path| AbsolutePathBuf::resolve_path_against_base(path, resolved_cwd.as_path()))
            .collect();
        let permission_config_syntax = resolve_permission_config_syntax(
            &config_layer_stack,
            &cfg,
            sandbox_mode,
        );
        let requirements_toml = config_layer_stack.requirements_toml();
        let windows_sandbox_level = match effective_windows_sandbox_mode {
            Some(WindowsSandboxModeToml::Elevated) => WindowsSandboxLevel::Elevated,
            Some(WindowsSandboxModeToml::Unelevated) => WindowsSandboxLevel::RestrictedToken,
            None => WindowsSandboxLevel::Disabled,
        };
        let persisted_permission_profile_id = if sandbox_mode.is_some()
            || permission_profile.is_some()
            || default_permissions_override.is_some()
        {
            None
        } else {
            persisted_permission_profile_id.as_deref()
        };
        let effective_permission_selection = resolve_effective_permission_selection(
            cfg.permissions.as_ref(),
            default_permissions_override.as_deref(),
            persisted_permission_profile_id,
            cfg.default_permissions.as_deref(),
            requirements_toml,
            &mut startup_warnings,
        )?;
        if effective_permission_selection.has_profiles()
            && !matches!(
                permission_config_syntax,
                Some(PermissionConfigSyntax::Legacy)
            )
            && effective_permission_selection.selected_profile_id.is_none()
            && !effective_permission_selection.requirements_force_profile_selection
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "config defines `[permissions]` profiles but does not set `default_permissions`",
            ));
        }

        let memories_config: MemoriesConfig = cfg.memories.clone().unwrap_or_default().into();

        let profiles_are_active = effective_permission_selection.profiles_are_active(
            default_permissions_override.as_deref(),
            permission_config_syntax,
        );
        let explicit_permission_profile_mode = effective_permission_selection
            .persisted_profile_id_was_provided
            || default_permissions_override.is_some()
            || matches!(
                permission_config_syntax,
                Some(PermissionConfigSyntax::Profiles)
            );
        let custom_permission_profiles = permission_profile_catalog_from_permissions(
            &config_layer_stack,
            effective_permission_selection.profiles.as_ref(),
        )?
        .into_iter()
        .filter(|profile| !is_builtin_permission_profile_name(&profile.id))
        .collect();
        let using_implicit_builtin_profile = !effective_permission_selection
            .persisted_profile_id_was_provided
            && permission_config_syntax.is_none()
            && effective_permission_selection.selected_profile_id.is_none();
        let should_seed_legacy_workspace_roots = effective_permission_selection
            .selected_profile_id
            .is_none()
            && matches!(
                permission_config_syntax,
                None | Some(PermissionConfigSyntax::Legacy)
            );
        let legacy_workspace_roots_explicit = should_seed_legacy_workspace_roots
            && cfg
                .sandbox_workspace_write
                .as_ref()
                .is_some_and(|sandbox_workspace_write| {
                    !sandbox_workspace_write.writable_roots.is_empty()
                });
        let workspace_roots_explicit = workspace_roots_override.is_some()
            || !requested_additional_writable_roots.is_empty()
            || legacy_workspace_roots_explicit;
        let mut workspace_roots = match workspace_roots_override {
            Some(workspace_roots) => workspace_roots,
            None => {
                let mut workspace_roots = vec![resolved_cwd.clone()];
                workspace_roots.extend(requested_additional_writable_roots.clone());
                if should_seed_legacy_workspace_roots
                    && let Some(sandbox_workspace_write) = cfg.sandbox_workspace_write.as_ref()
                {
                    workspace_roots.extend(sandbox_workspace_write.writable_roots.clone());
                }
                workspace_roots
            }
        };
        dedupe_absolute_paths(&mut workspace_roots);
        let (
            permission_profile,
            file_system_sandbox_policy,
            mut active_permission_profile,
            mut profile_workspace_roots,
        ) = if let Some(permission_profile) = permission_profile {
            let (file_system_sandbox_policy, _network_sandbox_policy) =
                permission_profile.to_runtime_permissions();
            (
                permission_profile,
                file_system_sandbox_policy,
                None,
                Vec::new(),
            )
        } else if profiles_are_active {
            let default_permissions = effective_permission_selection
                .selected_profile_id
                .unwrap_or_else(|| {
                    default_builtin_permission_profile_name()
                });
            let builtin_workspace_write_settings = if using_implicit_builtin_profile {
                cfg.sandbox_workspace_write.as_ref()
            } else {
                None
            };
            let (mut file_system_sandbox_policy, network_sandbox_policy) =
                compile_permission_profile_selection(
                    effective_permission_selection.profiles.as_ref(),
                    default_permissions,
                    builtin_workspace_write_settings,
                    &mut startup_warnings,
                )?;
            let mut configured_workspace_roots = compile_permission_profile_workspace_roots(
                effective_permission_selection.profiles.as_ref(),
                default_permissions,
                resolved_cwd.as_path(),
            )?;
            if using_implicit_builtin_profile
                && default_permissions == BUILT_IN_WORKSPACE_PROFILE
                && let Some(sandbox_workspace_write) = cfg.sandbox_workspace_write.as_ref()
            {
                configured_workspace_roots.extend(sandbox_workspace_write.writable_roots.clone());
            }
            dedupe_absolute_paths(&mut configured_workspace_roots);
            file_system_sandbox_policy = file_system_sandbox_policy
                .with_materialized_project_roots_for_workspace_roots(&configured_workspace_roots);
            let permission_profile = if let Some(permission_profile) =
                builtin_permission_profile(default_permissions, builtin_workspace_write_settings)
            {
                permission_profile
            } else {
                PermissionProfile::from_runtime_permissions(
                    &file_system_sandbox_policy,
                    network_sandbox_policy,
                )
            };
            let active_permission_profile = if using_implicit_builtin_profile
                && default_permissions == BUILT_IN_WORKSPACE_PROFILE
                && cfg.sandbox_workspace_write.is_some()
            {
                // The implicit built-in profile preserves legacy
                // `[sandbox_workspace_write]` customizations, but explicitly
                // selecting `:workspace` intentionally ignores those legacy
                // settings. Do not advertise a re-selectable active profile
                // when doing so would lose roots, network, or tmp settings.
                None
            } else {
                let selected_profile_extends = cfg
                    .permissions
                    .as_ref()
                    .and_then(|permissions| permissions.entries.get(default_permissions))
                    .and_then(|profile| profile.extends.clone());
                Some(ActivePermissionProfile {
                    id: default_permissions.to_string(),
                    extends: selected_profile_extends,
                })
            };
            (
                permission_profile,
                file_system_sandbox_policy,
                active_permission_profile,
                configured_workspace_roots,
            )
        } else {
            // No named `[permissions]` profile is active, but permissions
            // should still flow through the canonical profile representation.
            // Derive the old `sandbox_mode` defaults as a profile first, then
            // keep a legacy-compatible projection only for the remaining code
            // paths that still speak `SandboxPolicy`.
            let mut permission_profile = cfg
                .derive_permission_profile(
                    sandbox_mode,
                    windows_sandbox_level,
                    Some(&constrained_permission_profile),
                )
                .await;
            // The legacy-derived profiles above are expected to be
            // representable as `SandboxPolicy`. This guard keeps the old safe
            // fallback behavior if future changes make this branch derive a
            // profile with split-only filesystem semantics, such as root write
            // with carveouts or writes that are not expressible as
            // workspace-write roots.
            if let Err(err) = permission_profile.to_legacy_sandbox_policy(resolved_cwd.as_path()) {
                tracing::warn!(
                    error = %err,
                    "derived permission profile cannot be represented as a legacy sandbox policy; falling back to read-only"
                );
                permission_profile = PermissionProfile::read_only();
            }
            let (file_system_sandbox_policy, _network_sandbox_policy) =
                permission_profile.to_runtime_permissions();
            (
                permission_profile,
                file_system_sandbox_policy,
                None,
                Vec::new(),
            )
        };
        let approval_policy_was_explicit =
            approval_policy_override.is_some() || cfg.approval_policy.is_some();
        let mut approval_policy = approval_policy_override
            .or(cfg.approval_policy)
            .unwrap_or_default();
        if !approval_policy_was_explicit
            && let Err(err) = constrained_approval_policy.can_set(&approval_policy)
        {
            tracing::warn!(
                error = %err,
                "default approval policy is disallowed by requirements; falling back to required default"
            );
            approval_policy = constrained_approval_policy.value();
        }
        let web_search_mode =
            resolve_web_search_mode(&cfg, &features).unwrap_or(WebSearchMode::Cached);
        let web_search_config = resolve_web_search_config(&cfg);
        let experimental_request_user_input_enabled =
            resolve_experimental_request_user_input_enabled(&cfg);
        let update_plan_enabled = resolve_update_plan_enabled(&cfg);
        let tool_registry = ToolRegistryConfig {
            error_on_tool_collisions: cfg
                .features
                .as_ref()
                .and_then(|features| features.tool_registry.as_ref())
                .and_then(|config| config.error_on_tool_collisions)
                .unwrap_or_default(),
            turn_metadata_includes_tool_info: cfg
                .features
                .as_ref()
                .and_then(|features| features.tool_registry.as_ref())
                .and_then(|config| config.turn_metadata_includes_tool_info)
                .unwrap_or_default(),
        };
        let code_mode = resolve_code_mode_config(&cfg);
        let multi_agent_v2 = resolve_multi_agent_v2_config(&cfg);
        let token_budget = resolve_token_budget_config(&cfg, &features)?;
        let rollout_budget = resolve_rollout_budget_config(&cfg, &features)?;
        let current_time_reminder = resolve_current_time_reminder_config(&cfg, &features)?;
        let sleep_tool_mode = cfg
            .features
            .as_ref()
            .and_then(|features| features.sleep_tool.as_ref())
            .and_then(|feature| match feature {
                FeatureToml::Enabled(_) => None,
                FeatureToml::Config(config) => config.mode,
            })
            .unwrap_or_default();

        let openai_base_url = cfg
            .openai_base_url
            .clone()
            .filter(|value| !value.is_empty());

        let model_providers =
            merge_configured_model_providers(built_in_model_providers(openai_base_url), cfg.model_providers)
                .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidData, message))?;

        let model_provider_id = model_provider
            .or(cfg.model_provider)
            .unwrap_or_else(|| "openai".to_string());
        let model_provider = model_providers
            .get(&model_provider_id)
            .ok_or_else(|| {
                let message = if model_provider_id == LEGACY_OLLAMA_CHAT_PROVIDER_ID {
                    OLLAMA_CHAT_PROVIDER_REMOVED_ERROR.to_string()
                } else {
                    format!("Model provider `{model_provider_id}` not found")
                };
                std::io::Error::new(std::io::ErrorKind::NotFound, message)
            })?
            .clone();

        let shell_environment_policy = cfg.shell_environment_policy.into();
        let allow_login_shell = cfg.allow_login_shell.unwrap_or(true);


        if multi_agent_v2.max_concurrent_threads_per_session == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "features.multi_agent_v2.max_concurrent_threads_per_session must be at least 1",
            ));
        }
        let background_terminal_max_timeout = cfg
            .background_terminal_max_timeout
            .unwrap_or(DEFAULT_MAX_BACKGROUND_TERMINAL_TIMEOUT_MS)
            .max(MIN_EMPTY_YIELD_TIME_MS);

        let model = model.or(cfg.model);
        let service_tier = match service_tier_override {
            Some(Some(service_tier)) => Some(service_tier),
            Some(None) => Some(SERVICE_TIER_DEFAULT_REQUEST_VALUE.to_string()),
            None => cfg.service_tier,
        };
        let service_tier = service_tier.and_then(|service_tier| {
            match ServiceTier::from_request_value(&service_tier) {
                Some(ServiceTier::Fast) => features
                    .enabled(Feature::FastMode)
                    .then(|| ServiceTier::Fast.request_value().to_string()),
                Some(ServiceTier::Flex) => Some(ServiceTier::Flex.request_value().to_string()),
                None => Some(service_tier),
            }
        });

        let compact_prompt = compact_prompt.or(cfg.compact_prompt).and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        });

        // Load base instructions override from a file if specified. If the
        // path is relative, resolve it against the effective cwd so the
        // behaviour matches other path-like config values.
        let model_instructions_path = cfg.model_instructions_file.as_ref();
        let file_base_instructions = Self::try_read_non_empty_file(
            fs,
            model_instructions_path,
            "model instructions file",
        )
        .await?;
        let base_instructions = base_instructions
            .or(file_base_instructions)
            .or(cfg.instructions.clone());
        let base_instructions_provenance = base_instructions
            .as_ref()
            .map(|_| BaseInstructionsProvenance::Custom);
        let developer_instructions = developer_instructions.or(cfg.developer_instructions);
        let include_permissions_instructions = cfg.include_permissions_instructions.unwrap_or(true);
        let include_collaboration_mode_instructions =
            cfg.include_collaboration_mode_instructions.unwrap_or(true);
        let include_skill_instructions = cfg
            .skills
            .as_ref()
            .and_then(|skills| skills.include_instructions)
            .unwrap_or(true);
        let skill_max_context_tokens = cfg
            .skills
            .as_ref()
            .and_then(|skills| skills.max_context_tokens);
        let include_environment_context = cfg.include_environment_context.unwrap_or(true);
        let personality = personality
            .or(cfg.personality)
            .or_else(|| {
                features
                    .enabled(Feature::Personality)
                    .then_some(Personality::Pragmatic)
            });

        let experimental_compact_prompt_path = cfg.experimental_compact_prompt_file.as_ref();
        let file_compact_prompt = Self::try_read_non_empty_file(
            fs,
            experimental_compact_prompt_path,
            "experimental compact prompt file",
        )
        .await?;
        let compact_prompt = compact_prompt.or(file_compact_prompt);
        let model_catalog = load_model_catalog(cfg.model_catalog_json.clone())?;

        let log_dir = cfg
            .log_dir
            .as_ref()
            .map(AbsolutePathBuf::to_path_buf)
            .unwrap_or_else(|| codex_home.join("log").to_path_buf());
        let original_permission_profile = permission_profile.clone();
        apply_requirement_constrained_value(
            "approval_policy",
            approval_policy,
            &mut constrained_approval_policy,
            &mut startup_warnings,
        )?;
        if let Some(Sourced {
            value: filesystem_requirements,
            source: filesystem_requirements_source,
        }) = filesystem_requirements.as_ref()
            && !filesystem_requirements.deny_read.is_empty()
        {
            let requirement_source = filesystem_requirements_source.clone();
            constrained_permission_profile
                .value
                .add_validator(move |permission_profile| {
                    validate_permission_profile_for_deny_read(
                        permission_profile,
                        &requirement_source,
                    )
                })
                .map_err(std::io::Error::from)?;
        }
        let permission_profile_was_constrained = apply_requirement_constrained_value(
            "permission_profile",
            permission_profile,
            &mut constrained_permission_profile,
            &mut startup_warnings,
        )?;
        if permission_profile_was_constrained
            && sandbox_mode_requirement_for_permission_profile(&original_permission_profile)
                == SandboxModeRequirement::DangerFullAccess
            && constrained_permission_profile.get() == &PermissionProfile::read_only()
            && constrained_approval_policy.value() == AskForApproval::Never
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "`approval_policy = \"never\"` cannot be used because requirements do not allow `sandbox_mode = \"danger-full-access\"`; Codex would fall back to read-only permissions with approvals disabled. Choose an `approval_policy` based on what you need, such as `on-request`, or choose an allowed sandbox mode.",
            ));
        }
        if permission_profile_was_constrained {
            // The selected profile no longer describes the effective
            // permissions after requirements forced a fallback.
            active_permission_profile = None;
            profile_workspace_roots.clear();
        }
        apply_requirement_constrained_value(
            "web_search_mode",
            web_search_mode,
            &mut constrained_web_search_mode,
            &mut startup_warnings,
        )?;

        let mcp_servers = constrain_mcp_servers(cfg.mcp_servers.clone(), mcp_servers.as_ref())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{e}")))?;

        let effective_permission_profile = constrained_permission_profile.value.get().clone();
        let (mut effective_file_system_sandbox_policy, effective_network_sandbox_policy) =
            effective_permission_profile.to_runtime_permissions();
        if effective_permission_profile != original_permission_profile {
            effective_file_system_sandbox_policy
                .preserve_deny_read_restrictions_from(&file_system_sandbox_policy);
        }
        let managed_deny_read_policy = filesystem_requirements
            .as_ref()
            .filter(|Sourced { value, .. }| !value.deny_read.is_empty())
            .map(|Sourced { value, .. }| {
                let mut policy = FileSystemSandboxPolicy::restricted(Vec::new());
                apply_managed_filesystem_constraints(&mut policy, value);
                Arc::new(policy)
            });
        if let Some(managed_deny_read_policy) = managed_deny_read_policy.as_ref() {
            effective_file_system_sandbox_policy
                .preserve_deny_read_restrictions_from(managed_deny_read_policy);
        }
        let effective_permission_profile = PermissionProfile::from_runtime_permissions_with_enforcement(
            effective_permission_profile.enforcement(),
            &effective_file_system_sandbox_policy,
            effective_network_sandbox_policy,
        );
        constrained_permission_profile
            .value
            .set(effective_permission_profile)
            .map_err(std::io::Error::from)?;
        if let Some(Sourced {
            source: requirement_source,
            ..
        }) = filesystem_requirements.as_ref()
            && let Some(managed_file_system_policy) = managed_deny_read_policy.as_ref()
        {
            let _initial_matcher =
                ReadDenyMatcher::try_new(managed_file_system_policy, resolved_cwd.as_path())
                    .map_err(std::io::Error::other)?;
            let managed_file_system_policy = Arc::clone(managed_file_system_policy);
            let permission_cwd = resolved_cwd.clone();
            let requirement_source = requirement_source.clone();
            constrained_permission_profile
                .value
                .add_validator(move |permission_profile| {
                    let managed_deny_matcher =
                        ReadDenyMatcher::new(&managed_file_system_policy, permission_cwd.as_path());
                    let file_system_policy = permission_profile.file_system_sandbox_policy();
                    let missing_required_deny = managed_file_system_policy
                        .entries
                        .iter()
                        .any(|entry| !file_system_policy.entries.contains(entry));
                    let violating_root = file_system_policy
                        .entries
                        .iter()
                        .filter(|entry| entry.access.can_read())
                        .find_map(|entry| {
                            let FileSystemPath::Path { path } = &entry.path else {
                                return None;
                            };
                            let path = path.to_abs_path().ok()?;
                            managed_deny_matcher
                                .as_ref()
                                .is_some_and(|matcher| matcher.is_read_denied(path.as_path()))
                                .then_some(path)
                        });
                    if missing_required_deny || violating_root.is_some() {
                        return Err(ConstraintError::InvalidValue {
                            field_name: "permissions.filesystem",
                            candidate: violating_root
                                .map_or_else(|| "missing managed deny".to_string(), |path| {
                                    path.to_string_lossy().into_owned()
                                }),
                            allowed: "all managed deny_read restrictions".to_string(),
                            requirement_source: requirement_source.clone(),
                        });
                    }

                    Ok(())
                })
                .map_err(std::io::Error::from)?;
        }
        let permission_profile_state = PermissionProfileState::from_constrained_active_profile(
            constrained_permission_profile.value,
            active_permission_profile,
            profile_workspace_roots,
        )
        .map_err(std::io::Error::from)?;
        let otel = otel::resolve_config(cfg.otel.unwrap_or_default(), &mut startup_warnings);
        let config = Self {
            model,
            service_tier,
            model_context_window: cfg.model_context_window,
            model_auto_compact_token_limit: cfg.model_auto_compact_token_limit,
            model_auto_compact_token_limit_scope: cfg
                .model_auto_compact_token_limit_scope
                .unwrap_or_default(),
            model_provider_id,
            model_provider,
            cwd: resolved_cwd,
            workspace_roots: workspace_roots.clone(),
            workspace_roots_explicit,
            startup_warnings,
            permissions: Permissions {
                approval_policy: constrained_approval_policy.value,
                permission_profile_state,
                managed_deny_read_policy,
                workspace_roots,
                allow_login_shell,
                shell_environment_policy,
                windows_sandbox_mode,
                windows_sandbox_private_desktop,
            },
            explicit_permission_profile_mode,
            custom_permission_profiles,
            enforce_residency: enforce_residency.value,
            base_instructions,
            base_instructions_provenance,
            personality,
            developer_instructions,
            compact_prompt,
            include_permissions_instructions,
            include_collaboration_mode_instructions,
            include_skill_instructions,
            skill_max_context_tokens,
            orchestrator_skills_enabled,
            include_environment_context,
            mcp_servers,
            non_prefixed_mcp_tool_servers,
            mcp_optional_startup_grace: cfg
                .mcp_optional_startup_grace_ms
                .map(Duration::from_millis)
                .unwrap_or(DEFAULT_OPTIONAL_MCP_STARTUP_GRACE),
            model_providers,
            project_doc_max_bytes: cfg.project_doc_max_bytes.unwrap_or(AGENTS_MD_MAX_BYTES),
            tool_output_token_limit: cfg.tool_output_token_limit,
            memories: memories_config,
            codex_home,
            log_dir,
            config_layer_stack,
            ephemeral: ephemeral.unwrap_or_default(),
            ahead_self_exe,
            ahead_linux_sandbox_exe,

            hide_agent_reasoning: cfg.hide_agent_reasoning.unwrap_or(false),
            show_raw_agent_reasoning: cfg
                .show_raw_agent_reasoning
                .or(show_raw_agent_reasoning)
                .unwrap_or(false),
            model_reasoning_effort: cfg.model_reasoning_effort,
            plan_mode_reasoning_effort: cfg.plan_mode_reasoning_effort,
            model_reasoning_summary: cfg.model_reasoning_summary,
            model_catalog,
            model_verbosity: cfg.model_verbosity,
            respect_system_proxy,
            responses_api_metadata: cfg.responses_api_metadata.unwrap_or_default(),
            realtime_audio: cfg
                .audio
                .map_or_else(RealtimeAudioConfig::default, |audio| RealtimeAudioConfig {
                    microphone: audio.microphone,
                    speaker: audio.speaker,
                }),
            experimental_realtime_ws_base_url: cfg.experimental_realtime_ws_base_url,
            experimental_realtime_webrtc_call_base_url: cfg
                .experimental_realtime_webrtc_call_base_url,
            experimental_realtime_ws_model: cfg.experimental_realtime_ws_model,
            realtime: cfg
                .realtime
                .map_or_else(RealtimeConfig::default, |realtime| {
                    let defaults = RealtimeConfig::default();
                    RealtimeConfig {
                        version: realtime.version.unwrap_or(defaults.version),
                        session_type: realtime.session_type.unwrap_or(defaults.session_type),
                        transport: realtime.transport.unwrap_or(defaults.transport),
                        voice: realtime.voice,
                    }
                }),
            experimental_realtime_ws_backend_prompt: cfg.experimental_realtime_ws_backend_prompt,
            experimental_realtime_ws_startup_context: cfg.experimental_realtime_ws_startup_context,
            experimental_realtime_start_instructions: cfg.experimental_realtime_start_instructions,
            web_search_mode: constrained_web_search_mode.value,
            web_search_config,
            experimental_request_user_input_enabled,
            update_plan_enabled,
            tool_registry,
            code_mode,
            background_terminal_max_timeout,
            multi_agent_v2,
            token_budget,
            rollout_budget,
            current_time_reminder,
            sleep_tool_mode,
            features,
            suppress_unstable_features_warning: cfg
                .suppress_unstable_features_warning
                .unwrap_or(false),
            otel,
        };
        Ok(config)
        })
        .await
    }

    /// If `path` is `Some`, attempts to read the file at the given path and
    /// returns its contents as a trimmed `String`. If the file is empty, or
    /// is `Some` but cannot be read, returns an `Err`.
    async fn try_read_non_empty_file(
        fs: &dyn ExecutorFileSystem,
        path: Option<&AbsolutePathBuf>,
        context: &str,
    ) -> std::io::Result<Option<String>> {
        let Some(path) = path else {
            return Ok(None);
        };

        let path_uri = PathUri::from_abs_path(path);
        let contents = fs
            .read_file_text(&path_uri, ReadFileOptions::default(), /*sandbox*/ None)
            .await
            .map_err(|e| {
                std::io::Error::new(
                    e.kind(),
                    format!("failed to read {context} {}: {e}", path.display()),
                )
            })?;

        let s = contents.trim().to_string();
        if s.is_empty() {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{context} is empty: {}", path.display()),
            ))
        } else {
            Ok(Some(s))
        }
    }

    pub fn set_windows_sandbox_enabled(&mut self, value: bool) {
        self.permissions.windows_sandbox_mode = if value {
            Some(WindowsSandboxModeToml::Unelevated)
        } else if matches!(
            self.permissions.windows_sandbox_mode,
            Some(WindowsSandboxModeToml::Unelevated)
        ) {
            None
        } else {
            self.permissions.windows_sandbox_mode
        };
    }

    pub fn set_windows_elevated_sandbox_enabled(&mut self, value: bool) {
        self.permissions.windows_sandbox_mode = if value {
            Some(WindowsSandboxModeToml::Elevated)
        } else if matches!(
            self.permissions.windows_sandbox_mode,
            Some(WindowsSandboxModeToml::Elevated)
        ) {
            None
        } else {
            self.permissions.windows_sandbox_mode
        };
    }

    /// Resolves a named permission profile from effective config and managed requirements.
    pub fn resolve_permission_profile(
        &self,
        profile_name: &str,
    ) -> std::io::Result<PermissionProfileToml> {
        let cfg: ConfigToml = self
            .config_layer_stack
            .effective_config()
            .try_into()
            .map_err(|err| {
                std::io::Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "failed to read effective config for selected permission profile: {err}"
                    ),
                )
            })?;
        let permissions = merge_managed_permission_profiles(
            cfg.permissions.as_ref(),
            self.config_layer_stack.requirements_toml(),
        )?
        .unwrap_or_default();

        permissions::resolve_permission_profile(&permissions, profile_name)
    }

    pub fn bundled_skills_enabled(&self) -> bool {
        codex_config::bundled_skills_enabled_from_stack(&self.config_layer_stack)
    }

    /// Returns whether effective requirements allow selecting a concrete profile.
    pub fn is_permission_profile_allowed(
        &self,
        profile_id: &str,
        permission_profile: &PermissionProfile,
    ) -> bool {
        permission_profile_is_allowed(&self.config_layer_stack, profile_id, permission_profile)
    }
}

fn merge_managed_permission_profiles(
    configured_permissions: Option<&PermissionsToml>,
    requirements_toml: &ConfigRequirementsToml,
) -> std::io::Result<Option<PermissionsToml>> {
    let managed_profiles = requirements_toml
        .permissions
        .as_ref()
        .map(|permissions| &permissions.profiles)
        .filter(|profiles| !profiles.is_empty());
    let Some(managed_profiles) = managed_profiles else {
        return Ok(configured_permissions.cloned());
    };

    let mut merged_permissions = configured_permissions.cloned().unwrap_or_default();
    for (profile_id, managed_profile) in managed_profiles {
        if merged_permissions.entries.contains_key(profile_id) {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "requirements.toml permissions profile `{profile_id}` conflicts with a config-defined profile of the same name"
                ),
            ));
        }
        merged_permissions
            .entries
            .insert(profile_id.clone(), managed_profile.clone());
    }

    Ok(Some(merged_permissions))
}

fn resolve_effective_permission_selection<'a>(
    configured_profiles: Option<&PermissionsToml>,
    default_permissions_override: Option<&'a str>,
    persisted_profile_id: Option<&'a str>,
    configured_default_profile_id: Option<&'a str>,
    requirements_toml: &'a ConfigRequirementsToml,
    startup_warnings: &mut Vec<String>,
) -> std::io::Result<EffectivePermissionSelection<'a>> {
    let profiles = merge_managed_permission_profiles(configured_profiles, requirements_toml)?;
    validate_user_permission_profile_names(profiles.as_ref())?;
    validate_required_permission_profile_catalog(requirements_toml, profiles.as_ref())?;
    let valid_persisted_profile_id = persisted_profile_id.filter(|profile_id| {
        is_builtin_permission_profile_name(profile_id)
            || profiles.as_ref().is_some_and(|profiles| {
                compile_permission_profile_selection(
                    Some(profiles),
                    profile_id,
                    /*workspace_write*/ None,
                    &mut Vec::new(),
                )
                .is_ok()
            })
    });
    let selected_profile_id = resolve_default_permissions(
        default_permissions_override.or(valid_persisted_profile_id),
        configured_default_profile_id,
        requirements_toml,
        startup_warnings,
    )?;

    Ok(EffectivePermissionSelection {
        profiles,
        selected_profile_id,
        persisted_profile_id_was_provided: default_permissions_override.is_none()
            && valid_persisted_profile_id.is_some(),
        requirements_force_profile_selection: requirements_toml
            .allowed_permission_profiles
            .is_some(),
    })
}

fn resolve_default_permissions<'a>(
    default_permissions_override: Option<&'a str>,
    configured_default_permissions: Option<&'a str>,
    requirements_toml: &'a ConfigRequirementsToml,
    startup_warnings: &mut Vec<String>,
) -> std::io::Result<Option<&'a str>> {
    let selected_permissions = default_permissions_override.or(configured_default_permissions);
    let Some(allowed_permission_profiles) = requirements_toml.allowed_permission_profiles.as_ref()
    else {
        return Ok(selected_permissions);
    };
    let Some(fallback_permissions) = requirements_toml
        .default_permissions
        .as_deref()
        .or_else(|| implicit_default_permissions(allowed_permission_profiles))
    else {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "requirements.toml default_permissions must be set unless allowed_permission_profiles allows both `:workspace` and `:read-only`",
        ));
    };

    match selected_permissions {
        None => Ok(Some(fallback_permissions)),
        Some(selected_permissions)
            if is_permission_allowed(allowed_permission_profiles, selected_permissions) =>
        {
            Ok(Some(selected_permissions))
        }
        Some(selected_permissions) => {
            startup_warnings.push(format!(
                "Configured value for `permission_profile` is disallowed by requirements; falling back from `{selected_permissions}` to required value `{fallback_permissions}`."
            ));
            Ok(Some(fallback_permissions))
        }
    }
}

fn validate_required_permission_profile_catalog(
    requirements_toml: &ConfigRequirementsToml,
    available_permissions: Option<&PermissionsToml>,
) -> std::io::Result<()> {
    let is_known_profile = |profile_id: &str| {
        is_builtin_permission_profile_name(profile_id)
            || available_permissions
                .as_ref()
                .is_some_and(|permissions| permissions.entries.contains_key(profile_id))
    };

    let Some(allowed_permission_profiles) = requirements_toml.allowed_permission_profiles.as_ref()
    else {
        if requirements_toml.default_permissions.is_some() {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "requirements.toml default_permissions requires allowed_permission_profiles",
            ));
        }
        return Ok(());
    };
    for profile_id in allowed_permission_profiles.keys() {
        if !is_known_profile(profile_id) {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "requirements.toml allowed_permission_profiles refers to undefined profile `{profile_id}`"
                ),
            ));
        }
    }

    let Some(default_permissions) = requirements_toml
        .default_permissions
        .as_deref()
        .or_else(|| implicit_default_permissions(allowed_permission_profiles))
    else {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "requirements.toml default_permissions must be set unless allowed_permission_profiles allows both `:workspace` and `:read-only`",
        ));
    };
    if !is_permission_allowed(allowed_permission_profiles, default_permissions) {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "requirements.toml default_permissions `{default_permissions}` must be allowed by allowed_permission_profiles"
            ),
        ));
    }

    Ok(())
}

fn implicit_default_permissions(
    allowed_permission_profiles: &BTreeMap<String, bool>,
) -> Option<&'static str> {
    (is_permission_allowed(allowed_permission_profiles, BUILT_IN_WORKSPACE_PROFILE)
        && is_permission_allowed(allowed_permission_profiles, BUILT_IN_READ_ONLY_PROFILE))
    .then_some(BUILT_IN_WORKSPACE_PROFILE)
}

fn is_permission_allowed(
    allowed_permission_profiles: &BTreeMap<String, bool>,
    profile_id: &str,
) -> bool {
    allowed_permission_profiles
        .get(profile_id)
        .copied()
        .unwrap_or(false)
}

/// Returns the path to the folder where Codex logs are stored. Does not verify
/// that the directory exists.
pub fn log_dir(cfg: &Config) -> std::io::Result<PathBuf> {
    Ok(cfg.log_dir.clone())
}
