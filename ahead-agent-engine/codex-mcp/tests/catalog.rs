use std::collections::BTreeMap;
use std::collections::HashMap;
use std::time::Duration;

use codex_config::AppToolApproval;
use codex_config::DEFAULT_MCP_SERVER_ENVIRONMENT_ID;
use codex_config::McpServerConfig;
use codex_config::McpServerToolConfig;
use codex_config::McpServerTransportConfig;
use codex_mcp::McpEnvironmentAuthority;
use codex_mcp::McpServerRegistration;
use codex_mcp::ResolvedMcpCatalog;
use codex_protocol::mcp_policy::EnvironmentMcpPolicy;

fn server() -> McpServerConfig {
    McpServerConfig {
        transport: McpServerTransportConfig::Stdio {
            command: "docs-server".to_string(),
            args: Vec::new(),
            env: None,
            env_vars: Vec::new(),
            cwd: None,
        },
        environment_id: DEFAULT_MCP_SERVER_ENVIRONMENT_ID.to_string(),
        enabled: true,
        required: true,
        supports_parallel_tool_calls: true,
        omit_tools_from: None,
        disabled_reason: None,
        startup_timeout_sec: Some(Duration::from_secs(7)),
        tool_timeout_sec: Some(Duration::from_secs(11)),
        default_tools_approval_mode: Some(AppToolApproval::Prompt),
        enabled_tools: Some(vec!["read".to_string()]),
        disabled_tools: Some(vec!["write".to_string()]),
        tools: HashMap::from([(
            "read".to_string(),
            McpServerToolConfig {
                approval_mode: Some(AppToolApproval::Approve),
                ..Default::default()
            },
        )]),
    }
}

#[test]
fn restricted_environment_policy_disables_unlisted_configured_servers() {
    let policy = EnvironmentMcpPolicy {
        servers: Some(BTreeMap::new()),
    };
    let mut builder = ResolvedMcpCatalog::builder();
    builder.register(McpServerRegistration::from_config(
        "docs".to_string(),
        server(),
    ));

    let catalog =
        builder.build_with_environment_authority(|_| McpEnvironmentAuthority::Restricted(&policy));

    assert!(
        !catalog
            .server("docs")
            .expect("configured server remains visible")
            .config()
            .enabled
    );
}

#[test]
fn unavailable_environment_policy_fails_closed() {
    let mut builder = ResolvedMcpCatalog::builder();
    builder.register(McpServerRegistration::from_config(
        "docs".to_string(),
        server(),
    ));

    let catalog =
        builder.build_with_environment_authority(|_| McpEnvironmentAuthority::Unavailable);

    assert!(
        !catalog
            .server("docs")
            .expect("configured server remains visible")
            .config()
            .enabled
    );
}

#[test]
fn catalog_rebuild_preserves_configured_servers() {
    let mut builder = ResolvedMcpCatalog::builder();
    builder.register(McpServerRegistration::from_config(
        "docs".to_string(),
        server(),
    ));
    let catalog = builder.build();

    assert_eq!(
        catalog.to_builder().build().configured_servers(),
        catalog.configured_servers(),
    );
}
