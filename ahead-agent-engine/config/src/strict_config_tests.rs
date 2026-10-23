use super::*;
use crate::config_toml::ConfigToml;
use crate::diagnostics::TextPosition;
use crate::diagnostics::TextRange;
use pretty_assertions::assert_eq;
use std::path::PathBuf;

#[test]
fn ignored_toml_field_errors_accept_non_file_source_names() {
    let source_name = "com.ahead.editor:requirements_toml_base64";
    let contents = r#"
model = "gpt-5"
unknown_key = true"#;

    let value = toml::from_str::<TomlValue>(contents).expect("valid TOML");
    let error = config_error_from_ignored_toml_value_fields_for_source_name::<ConfigToml>(
        source_name,
        contents,
        value,
    )
    .expect("unknown field error");

    assert_eq!(
        error,
        ConfigError::new(
            PathBuf::from(source_name),
            TextRange {
                start: TextPosition { line: 3, column: 1 },
                end: TextPosition {
                    line: 3,
                    column: 11,
                },
            },
            "unknown configuration field `unknown_key`",
        )
    );
}

#[test]
fn type_errors_take_precedence_over_ignored_fields() {
    let path = Path::new("/tmp/config.toml");
    let contents = r#"
model_context_window = "wide"
unknown_key = true"#;

    let error =
        config_error_from_ignored_toml_fields::<ConfigToml>(path, contents).expect("type error");

    assert_eq!(
        error,
        ConfigError::new(
            path.to_path_buf(),
            TextRange {
                start: TextPosition {
                    line: 2,
                    column: 24,
                },
                end: TextPosition {
                    line: 2,
                    column: 29,
                },
            },
            "invalid type: string \"wide\", expected i64",
        )
    );
}

#[test]
fn strict_config_rejects_unknown_feature_key() {
    let path = Path::new("/tmp/config.toml");
    let contents = r#"
[features]
foo = true"#;

    let error = config_error_from_ignored_toml_fields::<ConfigToml>(path, contents)
        .expect("unknown feature error");

    assert_eq!(
        error,
        ConfigError::new(
            path.to_path_buf(),
            TextRange {
                start: TextPosition { line: 3, column: 1 },
                end: TextPosition { line: 3, column: 3 },
            },
            "unknown configuration field `features.foo`",
        )
    );
}

#[test]
fn strict_config_rejects_retired_runtime_settings() {
    for key in [
        "forced_login_method",
        "forced_chatgpt_workspace_id",
        "cli_auth_credentials_store",
        "chatgpt_base_url",
        "feedback",
        "check_for_update_on_startup",
        "disable_paste_burst",
    ] {
        let contents = format!("{key} = \"chatgpt\"");
        let error = config_error_from_ignored_toml_fields::<ConfigToml>(
            Path::new("/tmp/config.toml"),
            &contents,
        )
        .expect("retired runtime setting must be rejected");
        assert_eq!(
            error.message,
            format!("unknown configuration field `{key}`")
        );
    }
}

#[test]
fn strict_config_rejects_retired_tui_settings() {
    let error = config_error_from_ignored_toml_fields::<ConfigToml>(
        Path::new("/tmp/config.toml"),
        "[tui]\ntheme = \"dark\"",
    )
    .expect("retired TUI setting must be rejected");
    assert_eq!(error.message, "unknown configuration field `tui`");
}

#[test]
fn strict_config_rejects_removed_runtime_feature_keys() {
    let path = Path::new("/tmp/config.toml");

    for key in [
        "guardian_approval",
        "guardian_reuse_parent_compaction",
        "guardian_enhanced_node_repl_transcripts",
        "guardian_node_repl_transcript_images",
        "guardian_ext",
        "use_agent_identity",
        "undo",
        "js_repl",
        "code_mode_buffered_exec",
        "js_repl_tools_only",
        "terminal_resize_reflow",
        "search_tool",
        "codex_git_commit",
        "apply_patch_freeform",
        "use_linux_sandbox_bwrap",
        "request_rule",
        "remote_models",
        "multi_agent_mode",
        "enable_fanout",
        "tool_search",
        "tool_search_always_defer_mcp_tools",
        "unavailable_dummy_tools",
        "external_migration",
        "resize_all_images",
        "unified_exec_zsh_fork",
        "item_ids",
        "skill_env_var_dependency_prompt",
        "steer",
        "send_async_message",
        "collaboration_modes",
        "remote_control",
        "image_detail_original",
        "tui_app_server",
        "workspace_owner_usage_nudge",
        "responses_websockets",
        "responses_websockets_v2",
        "plugins",
        "recommended_plugins",
        "remote_plugin",
        "plugin_sharing",
        "plugin_hooks",
    ] {
        let contents = format!("[features]\n{key} = true\n");
        let error = config_error_from_ignored_toml_fields::<ConfigToml>(path, &contents)
            .expect("removed runtime feature key should be rejected");
        assert_eq!(
            error.message,
            format!("unknown configuration field `features.{key}`")
        );
    }

    for (contents, field) in [
        ("profile = \"work\"", "profile"),
        ("[profiles.work]\nmodel = \"gpt-test\"", "profiles"),
    ] {
        let error = config_error_from_ignored_toml_fields::<ConfigToml>(path, contents)
            .expect("removed Codex config profile must be rejected in strict mode");
        assert_eq!(
            error.message,
            format!("unknown configuration field `{field}`")
        );
    }
}

#[test]
fn strict_config_rejects_retired_feature_aliases() {
    let path = Path::new("/tmp/config.toml");
    for key in [
        "enable_experimental_windows_sandbox",
        "experimental_use_unified_exec_tool",
        "request_permissions",
        "web_search",
        "imagegenext",
        "collab",
        "memory_tool",
        "telepathy",
        "codex_hooks",
    ] {
        let contents = format!("[features]\n{key} = true\n");
        let error = config_error_from_ignored_toml_fields::<ConfigToml>(path, &contents)
            .expect("retired alias should be rejected");
        assert_eq!(
            error.message,
            format!("unknown configuration field `features.{key}`")
        );
    }

    let contents = "experimental_use_unified_exec_tool = true\n";
    assert!(
        config_error_from_ignored_toml_fields::<ConfigToml>(path, contents).is_some(),
        "{contents}"
    );
}

#[test]
fn strict_config_accepts_tool_registry_config() {
    let path = Path::new("/tmp/config.toml");

    for contents in [
        "[features.tool_registry]\nerror_on_tool_collisions = true\n",
        "[features.tool_registry]\nturn_metadata_includes_tool_info = true\n",
    ] {
        assert_eq!(
            config_error_from_ignored_toml_fields::<ConfigToml>(path, contents),
            None
        );
    }

    assert!(
        config_error_from_ignored_toml_fields::<ConfigToml>(
            path,
            "[features.tool_registry]\nunknown = true\n",
        )
        .is_some()
    );
}

#[test]
fn strict_config_rejects_retired_codex_product_sections() {
    let path = Path::new("/tmp/config.toml");
    for (contents, expected) in [
        ("[desktop]\nappearanceTheme = \"dark\"\n", "desktop"),
        ("[goals]\nmax_goal_token_budget = 1000\n", "goals"),
        (
            "[ghost_snapshot]\ndisable_warnings = true\n",
            "ghost_snapshot",
        ),
        ("oss_provider = \"ollama\"\n", "oss_provider"),
    ] {
        let error = config_error_from_ignored_toml_fields::<ConfigToml>(path, contents)
            .expect("retired Codex product config field should be rejected");
        assert_eq!(
            error.message,
            format!("unknown configuration field `{expected}`")
        );
    }
}
