use std::collections::BTreeMap;
use std::collections::HashMap;
use std::time::Duration;

use codex_config::AppToolApproval;
use codex_config::DEFAULT_MCP_SERVER_ENVIRONMENT_ID;
use codex_config::McpServerConfig;
use codex_config::McpServerToolConfig;
use codex_config::McpServerTransportConfig;
use codex_protocol::mcp_policy::EnvironmentMcpPolicy;
use pretty_assertions::assert_eq;

use super::McpEnvironmentAuthority;
use super::McpServerConflict;
use super::McpServerConflictAction;
use super::McpServerRegistration;
use super::McpServerSource;
use super::ResolvedMcpCatalog;
use super::ResolvedMcpServer;

fn server(url: &str) -> McpServerConfig {
    McpServerConfig {
        transport: McpServerTransportConfig::StreamableHttp {
            url: url.to_string(),
            bearer_token_env_var: None,
            http_headers: None,
            env_http_headers: None,
            http_headers_helper: None,
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
        scopes: None,
        oauth: None,
        oauth_resource: None,
        tools: HashMap::from([(
            "read".to_string(),
            McpServerToolConfig {
                approval_mode: Some(AppToolApproval::Approve),
                ..Default::default()
            },
        )]),
    }
}

fn compatibility_source(id: &str) -> McpServerSource {
    McpServerSource::Compatibility { id: id.to_string() }
}

fn extension_source(id: &str) -> McpServerSource {
    McpServerSource::Extension { id: id.to_string() }
}

fn register(source: McpServerSource) -> McpServerConflictAction {
    McpServerConflictAction::Register(source)
}

fn remove(source: McpServerSource) -> McpServerConflictAction {
    McpServerConflictAction::Remove(source)
}

#[test]
fn source_precedence_preserves_the_winning_registration() {
    let first_extension = server("https://extension.example/mcp");
    let second_extension = server("https://second-extension.example/mcp");
    let mut config_server = server("https://config.example/mcp");
    config_server.enabled = false;
    let mut builder = ResolvedMcpCatalog::builder();
    builder.register(McpServerRegistration::from_extension(
        "docs".to_string(),
        "hosted",
        /*contribution_order*/ 0,
        first_extension,
    ));
    builder.register(McpServerRegistration::from_extension(
        "docs".to_string(),
        "second-hosted",
        /*contribution_order*/ 1,
        second_extension.clone(),
    ));
    builder.register(McpServerRegistration::from_compatibility(
        "docs".to_string(),
        "legacy",
        server("https://compatibility.example/mcp"),
    ));
    builder.register(McpServerRegistration::from_config(
        "docs".to_string(),
        config_server,
    ));

    let catalog = builder.build();
    let resolved = catalog.server("docs").expect("resolved server");

    assert_eq!(
        resolved.source(),
        &McpServerSource::Extension {
            id: "second-hosted".to_string(),
        }
    );
    assert_eq!(resolved.config(), &second_extension);
    assert_eq!(
        catalog.conflicts(),
        &[McpServerConflict {
            name: "docs".to_string(),
            outcome: register(extension_source("second-hosted")),
            contenders: vec![
                register(extension_source("hosted")),
                register(extension_source("second-hosted")),
            ],
        }]
    );
}

#[test]
fn disabled_veto_only_disables_the_winning_registration() {
    let extension = server("https://extension.example/mcp");
    let mut expected = extension.clone();
    expected.enabled = false;
    let mut builder = ResolvedMcpCatalog::builder();
    builder.register(McpServerRegistration::from_extension(
        "docs".to_string(),
        "hosted",
        /*contribution_order*/ 0,
        extension,
    ));
    builder.disable("docs".to_string());

    let actual = builder
        .build()
        .server("docs")
        .expect("resolved server")
        .config()
        .clone();

    assert_eq!(actual, expected);
}

#[test]
fn disabled_winner_remains_a_veto_when_the_catalog_is_extended() {
    let mut disabled = server("https://config.example/mcp");
    disabled.enabled = false;
    let mut expected = server("https://extension.example/mcp");
    expected.enabled = false;
    let mut builder = ResolvedMcpCatalog::builder();
    builder.register(McpServerRegistration::from_config(
        "docs".to_string(),
        disabled,
    ));
    let mut builder = builder.build().to_builder();
    builder.register(McpServerRegistration::from_extension(
        "docs".to_string(),
        "hosted",
        /*contribution_order*/ 0,
        server("https://extension.example/mcp"),
    ));

    let resolved = builder.build();

    assert_eq!(
        resolved.server("docs"),
        Some(&super::ResolvedMcpServer {
            source: extension_source("hosted"),
            config: expected,
        })
    );
}

#[test]
fn equal_precedence_uses_insertion_order_not_source_identity() {
    let mut builder = ResolvedMcpCatalog::builder();
    builder.register(McpServerRegistration::from_compatibility(
        "docs".to_string(),
        "z-first",
        server("https://first.example/mcp"),
    ));
    builder.register(McpServerRegistration::from_compatibility(
        "docs".to_string(),
        "a-second",
        server("https://second.example/mcp"),
    ));

    let catalog = builder.build();

    assert_eq!(
        catalog.server("docs"),
        Some(&super::ResolvedMcpServer {
            source: compatibility_source("a-second"),
            config: server("https://second.example/mcp"),
        })
    );
    let mut builder = catalog.to_builder();
    builder.remove_compatibility("docs".to_string(), "remove-last");

    let catalog = builder.build();

    assert_eq!(catalog.server("docs"), None);
    assert_eq!(
        catalog.conflicts(),
        &[McpServerConflict {
            name: "docs".to_string(),
            outcome: remove(compatibility_source("remove-last")),
            contenders: vec![
                register(compatibility_source("z-first")),
                register(compatibility_source("a-second")),
                remove(compatibility_source("remove-last")),
            ],
        }]
    );
}

#[test]
fn environment_policy_restricts_extension_servers() {
    let policy = EnvironmentMcpPolicy {
        servers: Some(BTreeMap::new()),
    };
    let mut builder = ResolvedMcpCatalog::builder();
    builder.register(McpServerRegistration::from_extension(
        "docs".to_string(),
        "extension",
        /*contribution_order*/ 0,
        server("https://docs.example/mcp"),
    ));
    let catalog =
        builder.build_with_environment_authority(|_| McpEnvironmentAuthority::Restricted(&policy));
    assert!(
        !catalog
            .server("docs")
            .expect("registered server")
            .config()
            .enabled
    );
}
