use super::*;
use crate::McpServerRegistration;
use codex_config::Constrained;
use codex_config::McpServerTransportConfig;
use codex_config::types::AppToolApproval;
use codex_config::types::AuthKeyringBackendKind;
use codex_protocol::models::ManagedFileSystemPermissions;
use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::GranularApprovalConfig;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) fn test_mcp_config() -> McpConfig {
    McpConfig {
        mcp_oauth_credentials_store_mode: OAuthCredentialsStoreMode::default(),
        auth_keyring_backend_kind: AuthKeyringBackendKind::default(),
        mcp_oauth_callback_port: None,
        mcp_oauth_callback_url: None,
        optional_mcp_startup_grace: DEFAULT_OPTIONAL_MCP_STARTUP_GRACE,
        approval_policy: Constrained::allow_any(AskForApproval::OnRequest),
        permission_profile: PermissionProfile::default(),
        environment_cwds: HashMap::new(),
        server_permission_profiles: HashMap::new(),
        codex_linux_sandbox_exe: None,
        use_legacy_landlock: false,
        prefix_mcp_tool_names: true,
        non_prefixed_mcp_tool_servers: Vec::new(),
        protocol_mode: McpProtocolMode::Legacy,
        client_elicitation_capability: ElicitationCapability::default(),
        mcp_server_catalog: ResolvedMcpCatalog::default(),
    }
}

pub(crate) fn test_elicitation_config(
    server_name: &str,
    approval_policy: AskForApproval,
    permission_profile: PermissionProfile,
) -> Arc<McpConfig> {
    let mut config = test_mcp_config();
    config.approval_policy = Constrained::allow_any(approval_policy);
    config.permission_profile = permission_profile.clone();
    config
        .server_permission_profiles
        .insert(server_name.to_string(), permission_profile);
    Arc::new(config)
}

pub(crate) fn test_http_server_config(url: &str) -> McpServerConfig {
    serde_json::from_value(serde_json::json!({"url": url})).expect("HTTP MCP server config")
}

#[test]
fn qualified_mcp_tool_name_prefix_sanitizes_server_names_without_lowercasing() {
    assert_eq!(
        qualified_mcp_tool_name_prefix("Some-Server"),
        "mcp__Some_Server__".to_string()
    );
}

#[test]
fn mcp_server_permissions_handle_unattached_and_threadless_servers() {
    let mut config = test_mcp_config();
    config.permission_profile = PermissionProfile::Disabled;
    let mut missing_server = test_http_server_config("https://example.com/mcp");
    missing_server.environment_id = "missing".to_string();
    let mut selected_server = missing_server.clone();
    selected_server.environment_id = "unattached".to_string();
    let mut catalog = ResolvedMcpCatalog::builder();
    catalog.register(McpServerRegistration::from_config(
        "missing".to_string(),
        missing_server,
    ));
    catalog.register(McpServerRegistration::from_config(
        "selected".to_string(),
        selected_server,
    ));
    config.mcp_server_catalog = catalog.build();
    let servers = effective_mcp_servers(&config);
    assert_eq!(config.permission_profile_for_server("selected"), None);
    config.set_server_permission_profiles(&servers, std::iter::empty());

    assert_eq!(
        config.permission_profile_for_server("selected"),
        Some(&PermissionProfile::Disabled)
    );
    assert_eq!(config.permission_profile_for_server("missing"), None);

    let config = config.for_threadless_operations(&servers);
    assert_eq!(
        config.permission_profile_for_server("selected"),
        Some(&PermissionProfile::default())
    );
}

#[test]
fn mcp_prompt_auto_approval_honors_unrestricted_managed_profiles() {
    assert!(mcp_permission_prompt_is_auto_approved(
        AskForApproval::Never,
        &PermissionProfile::Managed {
            file_system: ManagedFileSystemPermissions::Unrestricted,
            network: NetworkSandboxPolicy::Enabled,
        },
        McpPermissionPromptAutoApproveContext::default(),
    ));
    assert!(mcp_permission_prompt_is_auto_approved(
        AskForApproval::Never,
        &PermissionProfile::Managed {
            file_system: ManagedFileSystemPermissions::Unrestricted,
            network: NetworkSandboxPolicy::Restricted,
        },
        McpPermissionPromptAutoApproveContext::default(),
    ));
    assert!(!mcp_permission_prompt_is_auto_approved(
        AskForApproval::Never,
        &PermissionProfile::read_only(),
        McpPermissionPromptAutoApproveContext::default(),
    ));
    assert!(!mcp_permission_prompt_is_auto_approved(
        AskForApproval::OnRequest,
        &PermissionProfile::Managed {
            file_system: ManagedFileSystemPermissions::Unrestricted,
            network: NetworkSandboxPolicy::Enabled,
        },
        McpPermissionPromptAutoApproveContext::default(),
    ));
}

#[test]
fn mcp_prompt_auto_approval_honors_approved_tools_in_all_permission_modes() {
    for approval_policy in [
        AskForApproval::UnlessTrusted,
        AskForApproval::OnRequest,
        AskForApproval::Granular(GranularApprovalConfig {
            sandbox_approval: true,
            rules: true,
            skill_approval: true,
            request_permissions: true,
            mcp_elicitations: true,
        }),
        AskForApproval::Never,
    ] {
        assert!(mcp_permission_prompt_is_auto_approved(
            approval_policy,
            &PermissionProfile::read_only(),
            McpPermissionPromptAutoApproveContext {
                tool_approval_mode: Some(AppToolApproval::Approve),
            },
        ));
    }

    assert!(!mcp_permission_prompt_is_auto_approved(
        AskForApproval::OnRequest,
        &PermissionProfile::read_only(),
        McpPermissionPromptAutoApproveContext {
            tool_approval_mode: Some(AppToolApproval::Auto),
        },
    ));
}

#[test]
fn mcp_prompt_auto_approval_rejects_auto_mode_in_default_permission_mode() {
    assert!(!mcp_permission_prompt_is_auto_approved(
        AskForApproval::OnRequest,
        &PermissionProfile::read_only(),
        McpPermissionPromptAutoApproveContext {
            tool_approval_mode: Some(AppToolApproval::Auto),
        },
    ));
}

#[test]
fn effective_mcp_servers_preserve_runtime_servers() {
    let mut config = test_mcp_config();

    let mut catalog = ResolvedMcpCatalog::builder();
    catalog.register(McpServerRegistration::from_config(
        "sample".to_string(),
        McpServerConfig {
            transport: McpServerTransportConfig::StreamableHttp {
                url: "https://user.example/mcp".to_string(),
                bearer_token_env_var: None,
                http_headers: None,
                env_http_headers: None,
                http_headers_helper: None,
            },
            environment_id: codex_config::DEFAULT_MCP_SERVER_ENVIRONMENT_ID.to_string(),
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
            scopes: None,
            oauth: None,
            oauth_resource: None,
            tools: HashMap::new(),
        },
    ));
    catalog.register(McpServerRegistration::from_config(
        "docs".to_string(),
        McpServerConfig {
            transport: McpServerTransportConfig::StreamableHttp {
                url: "https://docs.example/mcp".to_string(),
                bearer_token_env_var: None,
                http_headers: None,
                env_http_headers: None,
                http_headers_helper: None,
            },
            environment_id: codex_config::DEFAULT_MCP_SERVER_ENVIRONMENT_ID.to_string(),
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
            scopes: None,
            oauth: None,
            oauth_resource: None,
            tools: HashMap::new(),
        },
    ));
    config.mcp_server_catalog = catalog.build();

    let effective = effective_mcp_servers(&config);

    let sample = effective.get("sample").expect("user server should exist");
    let docs = effective
        .get("docs")
        .expect("configured server should exist");

    let sample = sample.config();
    let docs = docs.config();

    match &sample.transport {
        McpServerTransportConfig::StreamableHttp { url, .. } => {
            assert_eq!(url, "https://user.example/mcp");
        }
        other => panic!("expected streamable http transport, got {other:?}"),
    }
    match &docs.transport {
        McpServerTransportConfig::StreamableHttp { url, .. } => {
            assert_eq!(url, "https://docs.example/mcp");
        }
        other => panic!("expected streamable http transport, got {other:?}"),
    }
}
