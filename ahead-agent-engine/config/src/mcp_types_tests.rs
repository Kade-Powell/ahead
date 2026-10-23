use super::*;
use codex_utils_path_uri::LegacyAppPathString;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use std::path::Path;

#[test]
fn app_tool_approval_restrictions_never_weaken_either_policy() {
    use AppToolApproval::Approve;
    use AppToolApproval::Auto;
    use AppToolApproval::Prompt;
    use AppToolApproval::Writes;

    let modes = [Approve, Auto, Writes, Prompt];
    let expected = [
        [Approve, Auto, Writes, Prompt],
        [Auto, Auto, Prompt, Prompt],
        [Writes, Prompt, Writes, Prompt],
        [Prompt, Prompt, Prompt, Prompt],
    ];

    for (parent_index, parent) in modes.into_iter().enumerate() {
        for (requested_index, requested) in modes.into_iter().enumerate() {
            assert_eq!(
                parent.restrict_to(requested),
                expected[parent_index][requested_index],
                "parent: {parent:?}, requested: {requested:?}",
            );
        }
    }
}

#[test]
fn deserialize_stdio_command_server_config() {
    let cfg: McpServerConfig = toml::from_str(
        r#"
            command = "echo"
        "#,
    )
    .expect("should deserialize command config");

    assert_eq!(
        cfg.transport,
        McpServerTransportConfig::Stdio {
            command: "echo".to_string(),
            args: vec![],
            env: None,
            env_vars: Vec::new(),
            cwd: None,
        }
    );
    assert!(cfg.enabled);
    assert!(!cfg.required);
    assert_eq!(cfg.omit_tools_from, None);
    assert!(cfg.enabled_tools.is_none());
    assert!(cfg.disabled_tools.is_none());
}

#[test]
fn deserialize_stdio_command_server_config_with_args() {
    let cfg: McpServerConfig = toml::from_str(
        r#"
            command = "echo"
            args = ["hello", "world"]
        "#,
    )
    .expect("should deserialize command config");

    assert_eq!(
        cfg.transport,
        McpServerTransportConfig::Stdio {
            command: "echo".to_string(),
            args: vec!["hello".to_string(), "world".to_string()],
            env: None,
            env_vars: Vec::new(),
            cwd: None,
        }
    );
    assert!(cfg.enabled);
}

#[test]
fn deserialize_stdio_server_accepts_absolute_cwd() {
    #[cfg(not(windows))]
    let cwd = r"C:\Users\openai\share";
    #[cfg(windows)]
    let cwd = "/home/openai/share";
    let expected_cwd = LegacyAppPathString::from_path(Path::new(cwd));
    let cfg: McpServerConfig = match toml::from_str(&format!(
        r#"
            command = "echo"
            cwd = {cwd:?}
        "#
    )) {
        Ok(cfg) => cfg,
        Err(error) => panic!("stdio MCP should accept absolute cwd: {error}"),
    };

    assert_eq!(
        cfg.transport,
        McpServerTransportConfig::Stdio {
            command: "echo".to_string(),
            args: vec![],
            env: None,
            env_vars: Vec::new(),
            cwd: Some(expected_cwd),
        }
    );
}

#[test]
fn deserialize_stdio_command_server_config_with_arg_with_args_and_env() {
    let cfg: McpServerConfig = toml::from_str(
        r#"
            command = "echo"
            args = ["hello", "world"]
            env = { "FOO" = "BAR" }
        "#,
    )
    .expect("should deserialize command config");

    assert_eq!(
        cfg.transport,
        McpServerTransportConfig::Stdio {
            command: "echo".to_string(),
            args: vec!["hello".to_string(), "world".to_string()],
            env: Some(HashMap::from([("FOO".to_string(), "BAR".to_string())])),
            env_vars: Vec::new(),
            cwd: None,
        }
    );
    assert!(cfg.enabled);
}

#[test]
fn deserialize_stdio_command_server_config_with_env_vars() {
    let cfg: McpServerConfig = toml::from_str(
        r#"
            command = "echo"
            env_vars = ["FOO", "BAR"]
        "#,
    )
    .expect("should deserialize command config with env_vars");

    assert_eq!(
        cfg.transport,
        McpServerTransportConfig::Stdio {
            command: "echo".to_string(),
            args: vec![],
            env: None,
            env_vars: vec!["FOO".into(), "BAR".into()],
            cwd: None,
        }
    );
}

#[test]
fn deserialize_stdio_command_server_config_rejects_sourced_env_vars() {
    let err = toml::from_str::<McpServerConfig>(
        r#"
            command = "echo"
            env_vars = [{ name = "TOKEN", source = "remote" }]
        "#,
    )
    .expect_err("MCP environment references must be plain local variable names");
    assert!(!err.to_string().is_empty());
}

#[test]
fn deserialize_stdio_command_server_config_with_cwd() {
    let cfg: McpServerConfig = toml::from_str(
        r#"
            command = "echo"
            cwd = "/tmp"
        "#,
    )
    .expect("should deserialize command config with cwd");

    assert_eq!(
        cfg.transport,
        McpServerTransportConfig::Stdio {
            command: "echo".to_string(),
            args: vec![],
            env: None,
            env_vars: Vec::new(),
            cwd: Some(LegacyAppPathString::from_path(Path::new("/tmp"))),
        }
    );
}

#[test]
fn deserialize_disabled_server_config() {
    let cfg: McpServerConfig = toml::from_str(
        r#"
            command = "echo"
            enabled = false
        "#,
    )
    .expect("should deserialize disabled server config");

    assert!(!cfg.enabled);
    assert!(!cfg.required);
}

#[test]
fn deserialize_required_server_config() {
    let cfg: McpServerConfig = toml::from_str(
        r#"
            command = "echo"
            required = true
        "#,
    )
    .expect("should deserialize required server config");

    assert!(cfg.required);
}

#[test]
fn rejects_unsupported_remote_mcp_and_oauth_settings() {
    for contents in [
        "url = \"https://example.com/mcp\"",
        "command = \"server\"\noauth_resource = \"https://api.example.com\"",
        "command = \"server\"\n[oauth]\nclient_id = \"client\"",
    ] {
        toml::from_str::<McpServerConfig>(contents)
            .expect_err("unsupported interactive OAuth settings should be rejected");
    }
}

#[test]
fn deserialize_server_config_with_tool_filters() {
    let cfg: McpServerConfig = toml::from_str(
        r#"
            command = "echo"
            enabled_tools = ["allowed"]
            disabled_tools = ["blocked"]
        "#,
    )
    .expect("should deserialize tool filters");

    assert_eq!(cfg.enabled_tools, Some(vec!["allowed".to_string()]));
    assert_eq!(cfg.disabled_tools, Some(vec!["blocked".to_string()]));
}

#[test]
fn deserialize_server_config_with_parallel_tool_calls() {
    let cfg: McpServerConfig = toml::from_str(
        r#"
            command = "echo"
            supports_parallel_tool_calls = true
        "#,
    )
    .expect("should deserialize supports_parallel_tool_calls");

    assert!(cfg.supports_parallel_tool_calls);
}

#[test]
fn serialize_round_trips_server_config_with_omitted_tool_exposure_surfaces() {
    for omitted_surfaces in [
        vec![],
        vec![ToolExposureSurface::CodeMode],
        vec![ToolExposureSurface::Deferred],
        vec![ToolExposureSurface::Direct],
        vec![ToolExposureSurface::CodeMode, ToolExposureSurface::Deferred],
        vec![ToolExposureSurface::CodeMode, ToolExposureSurface::Direct],
        vec![ToolExposureSurface::Deferred, ToolExposureSurface::Direct],
        vec![
            ToolExposureSurface::CodeMode,
            ToolExposureSurface::Deferred,
            ToolExposureSurface::Direct,
        ],
    ] {
        let serialized_surfaces = omitted_surfaces
            .iter()
            .map(|surface| format!("\"{surface}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let config = format!("command = \"echo\"\nomit_tools_from = [{serialized_surfaces}]\n");
        let cfg: McpServerConfig =
            toml::from_str(&config).expect("should deserialize omitted MCP exposure surfaces");
        assert_eq!(cfg.omit_tools_from, Some(omitted_surfaces.clone()));

        let serialized = toml::to_string(&cfg).expect("should serialize MCP config");
        assert!(serialized.contains(&format!("omit_tools_from = [{serialized_surfaces}]")));

        let round_tripped: McpServerConfig =
            toml::from_str(&serialized).expect("should deserialize serialized MCP config");
        assert_eq!(round_tripped, cfg);
    }
}

#[test]
fn deserialize_server_config_with_default_tool_approval_mode() {
    let cfg: McpServerConfig = toml::from_str(
        r#"
            command = "echo"
            default_tools_approval_mode = "approve"

            [tools.search]
            approval_mode = "prompt"
            output_token_limit = 30000
        "#,
    )
    .expect("should deserialize default tool approval mode");

    assert_eq!(
        cfg.default_tools_approval_mode,
        Some(AppToolApproval::Approve)
    );
    assert_eq!(
        cfg.tools.get("search"),
        Some(&McpServerToolConfig {
            approval_mode: Some(AppToolApproval::Prompt),
            output_token_limit: std::num::NonZeroUsize::new(30_000),
        })
    );

    let serialized = toml::to_string(&cfg).expect("should serialize MCP config");
    assert!(serialized.contains("default_tools_approval_mode = \"approve\""));
    assert!(serialized.contains("output_token_limit = 30000"));

    let round_tripped: McpServerConfig =
        toml::from_str(&serialized).expect("should deserialize serialized MCP config");
    assert_eq!(round_tripped, cfg);
}

#[test]
fn deserialize_rejects_nonpositive_mcp_tool_output_limits() {
    for output_token_limit in [0, -1] {
        let config = format!(
            "command = \"echo\"\n[tools.search]\noutput_token_limit = {output_token_limit}\n"
        );
        let error = toml::from_str::<McpServerConfig>(&config)
            .expect_err("MCP tool output limit must be positive");
        assert!(error.to_string().contains("output_token_limit"));
    }
}

#[test]
fn serialize_round_trips_server_config_with_parallel_tool_calls() {
    let cfg: McpServerConfig = toml::from_str(
        r#"
            command = "echo"
            supports_parallel_tool_calls = true
            tool_timeout_sec = 2.0
        "#,
    )
    .expect("should deserialize supports_parallel_tool_calls");

    let serialized = toml::to_string(&cfg).expect("should serialize MCP config");
    assert!(serialized.contains("supports_parallel_tool_calls = true"));

    let round_tripped: McpServerConfig =
        toml::from_str(&serialized).expect("should deserialize serialized MCP config");
    assert_eq!(round_tripped, cfg);
}

#[test]
fn deserialize_rejects_unknown_server_fields() {
    toml::from_str::<McpServerConfig>(
        r#"
            command = "echo"
            ignored_test_field = "ignored"
        "#,
    )
    .expect_err("unsupported MCP options should fail closed");
}

#[test]
fn deserialize_rejects_command_and_url() {
    toml::from_str::<McpServerConfig>(
        r#"
            command = "echo"
            url = "https://example.com"
        "#,
    )
    .expect_err("should reject command+url");
}

#[test]
fn deserialize_rejects_removed_http_transport_options() {
    for field in [
        "url = \"https://example.com/mcp\"",
        "bearer_token_env_var = \"MCP_TOKEN\"",
        "http_headers = { \"X-Foo\" = \"bar\" }",
        "env_http_headers = { \"X-Token\" = \"TOKEN_ENV\" }",
        "http_headers_helper = \"auth-cli headers\"",
    ] {
        let config = format!("command = \"echo\"\n{field}\n");
        toml::from_str::<McpServerConfig>(&config)
            .expect_err("removed remote MCP transport fields must be rejected");
    }
}
