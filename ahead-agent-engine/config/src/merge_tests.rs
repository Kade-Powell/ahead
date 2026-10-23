use super::*;
use crate::config_toml::ConfigToml;
use crate::types::MemoriesToml;
use pretty_assertions::assert_eq;

fn parse_toml(value: &str) -> TomlValue {
    toml::from_str(value).expect("TOML should parse")
}

#[test]
fn merge_toml_values_normalizes_legacy_key_from_base_layer() {
    let mut base = parse_toml(
        r#"
[memories]
no_memories_if_mcp_or_web_search = false
"#,
    );
    let overlay = parse_toml(
        r#"
[memories]
disable_on_external_context = true
"#,
    );

    merge_toml_values(&mut base, &overlay);

    let expected = parse_toml(
        r#"
[memories]
disable_on_external_context = true
"#,
    );
    assert_eq!(base, expected);

    let config: ConfigToml = base.try_into().expect("merged config should deserialize");
    assert_eq!(
        config.memories,
        Some(MemoriesToml {
            disable_on_external_context: Some(true),
            ..Default::default()
        })
    );
}

#[test]
fn merge_toml_values_normalizes_legacy_key_from_overlay_layer() {
    let mut base = parse_toml(
        r#"
[memories]
disable_on_external_context = false
"#,
    );
    let overlay = parse_toml(
        r#"
[memories]
no_memories_if_mcp_or_web_search = true
"#,
    );

    merge_toml_values(&mut base, &overlay);

    let expected = parse_toml(
        r#"
[memories]
disable_on_external_context = true
"#,
    );
    assert_eq!(base, expected);

    let config: ConfigToml = base.try_into().expect("merged config should deserialize");
    assert_eq!(
        config.memories,
        Some(MemoriesToml {
            disable_on_external_context: Some(true),
            ..Default::default()
        })
    );
}

#[test]
fn merge_toml_values_prefers_canonical_key_when_one_layer_has_both_names() {
    let mut base = TomlValue::Table(toml::map::Map::new());
    let overlay = parse_toml(
        r#"
[memories]
disable_on_external_context = true
no_memories_if_mcp_or_web_search = false
"#,
    );

    merge_toml_values(&mut base, &overlay);

    let expected = parse_toml(
        r#"
[memories]
disable_on_external_context = true
"#,
    );
    assert_eq!(base, expected);
}

/// Feature tables added above toggles retain the lower layer's enabled state.
#[test]
fn merge_multi_agent_v2_table_preserves_legacy_boolean_toggle() {
    let mut base = parse_toml("[features]\nmulti_agent_v2 = true\n");
    let overlay = parse_toml("[features.multi_agent_v2]\nmax_concurrent_threads_per_session = 4\n");

    merge_toml_values(&mut base, &overlay);

    assert_eq!(
        base,
        parse_toml(
            "[features.multi_agent_v2]\nenabled = true\nmax_concurrent_threads_per_session = 4\n"
        )
    );
}

/// Feature toggles update enabled state without discarding concurrency limits.
#[test]
fn merge_multi_agent_v2_boolean_preserves_existing_feature_table() {
    let mut base = parse_toml(
        "[features.multi_agent_v2]\nenabled = true\nmax_concurrent_threads_per_session = 4\n",
    );
    let overlay = parse_toml("[features]\nmulti_agent_v2 = false\n");

    merge_toml_values(&mut base, &overlay);

    assert_eq!(
        base,
        parse_toml(
            "[features.multi_agent_v2]\nenabled = false\nmax_concurrent_threads_per_session = 4\n"
        )
    );
}

/// Opaque desktop settings retain ordinary scalar/table replacement semantics.
#[test]
fn merge_multi_agent_v2_compatibility_excludes_opaque_desktop_paths() {
    let cases = [
        (
            "[desktop.features.multi_agent_v2]\nenabled = true\n",
            "[desktop.features]\nmulti_agent_v2 = false\n",
            "[desktop.features]\nmulti_agent_v2 = false\n",
        ),
        (
            "[desktop.features]\nmulti_agent_v2 = true\n",
            "[desktop.features.multi_agent_v2]\ncustom = true\n",
            "[desktop.features.multi_agent_v2]\ncustom = true\n",
        ),
    ];

    for (base, overlay, expected) in cases {
        let mut base = parse_toml(base);
        merge_toml_values(&mut base, &parse_toml(overlay));
        assert_eq!(base, parse_toml(expected));
    }
}

/// CLI overrides preserve the agent toggle and concurrency option in either ordering.
#[test]
fn multi_agent_v2_cli_overrides_preserve_boolean_and_concurrency_configuration() {
    let instructions = (
        "features.multi_agent_v2.max_concurrent_threads_per_session".to_string(),
        TomlValue::Integer(4),
    );
    let enabled = (
        "features.multi_agent_v2".to_string(),
        TomlValue::Boolean(true),
    );
    let feature_table = (
        "features.multi_agent_v2".to_string(),
        parse_toml("max_concurrent_threads_per_session = 4\n"),
    );
    let expected = parse_toml(
        "[features.multi_agent_v2]\nenabled = true\nmax_concurrent_threads_per_session = 4\n",
    );

    for overrides in [
        vec![enabled.clone(), instructions.clone()],
        vec![instructions, enabled.clone()],
        vec![enabled.clone(), feature_table.clone()],
        vec![feature_table, enabled],
    ] {
        assert_eq!(crate::build_cli_overrides_layer(&overrides), expected);
    }
}

#[test]
fn sleep_tool_overrides_preserve_disabled_state_and_mode() {
    let disabled = ("features.sleep_tool".to_string(), TomlValue::Boolean(false));
    let mode = (
        "features.sleep_tool.mode".to_string(),
        TomlValue::String("always_on".to_string()),
    );
    let expected = parse_toml("[features.sleep_tool]\nenabled = false\nmode = \"always_on\"\n");
    for overrides in [vec![disabled.clone(), mode.clone()], vec![mode, disabled]] {
        assert_eq!(crate::build_cli_overrides_layer(&overrides), expected);
    }

    let boolean_layer = parse_toml("[features]\nsleep_tool = false\n");
    let table_layer = parse_toml("[features.sleep_tool]\nmode = \"always_on\"\n");
    for (mut base, overlay) in [
        (boolean_layer.clone(), table_layer.clone()),
        (table_layer, boolean_layer),
    ] {
        merge_toml_values(&mut base, &overlay);
        assert_eq!(base, expected);
    }
}

/// Repeated opaque desktop overrides continue to replace their previous value.
#[test]
fn multi_agent_v2_cli_compatibility_excludes_opaque_desktop_paths() {
    let path = "desktop.features.multi_agent_v2".to_string();
    let enabled = (path.clone(), TomlValue::Boolean(true));
    let feature_table = (path, parse_toml("custom = true\n"));

    assert_eq!(
        crate::build_cli_overrides_layer(&[enabled.clone(), feature_table.clone()]),
        parse_toml("[desktop.features.multi_agent_v2]\ncustom = true\n")
    );
    assert_eq!(
        crate::build_cli_overrides_layer(&[feature_table, enabled]),
        parse_toml("[desktop.features]\nmulti_agent_v2 = true\n")
    );
}

#[test]
fn merge_toml_values_normalizes_permission_network_domains_before_overlaying() {
    let mut base = parse_toml(
        r#"
[permissions.dev.network.domains]
"example.com" = "deny"
"#,
    );
    let overlay = parse_toml(
        r#"
[permissions.dev.network.domains]
"EXAMPLE.COM" = "allow"
"#,
    );

    merge_toml_values(&mut base, &overlay);

    let expected = parse_toml(
        r#"
[permissions.dev.network.domains]
"example.com" = "allow"
"#,
    );
    assert_eq!(base, expected);
}

#[test]
fn shell_environment_policy_legacy_array_overlay_replaces_legacy_array() {
    let mut base = parse_toml(
        r#"
[shell_environment_policy]
exclude = ["LOW_*", "SHARED_*"]
"#,
    );
    let overlay = parse_toml(
        r#"
[shell_environment_policy]
exclude = ["HIGH_*"]
"#,
    );

    merge_toml_values(&mut base, &overlay);

    assert_eq!(base, overlay);
}

#[test]
fn shell_environment_policy_filters_overlay_merges_by_key_case_insensitively() {
    let mut base = parse_toml(
        r#"
[shell_environment_policy.filters]
"FLIP_*" = "exclude"
"KEEP_*" = "include"
"#,
    );
    let overlay = parse_toml(
        r#"
[shell_environment_policy.filters]
"ADD_*" = "exclude"
"flip_*" = "include"
"#,
    );

    merge_toml_values(&mut base, &overlay);

    assert_eq!(
        base,
        parse_toml(
            r#"
[shell_environment_policy.filters]
"add_*" = "exclude"
"flip_*" = "include"
"keep_*" = "include"
"#,
        )
    );
}

#[test]
fn shell_environment_policy_filters_overlay_merges_unicode_keys_case_insensitively() {
    let mut base = parse_toml(
        r#"
[shell_environment_policy.filters]
"СЕКРЕТ_*" = "exclude"
"#,
    );
    let overlay = parse_toml(
        r#"
[shell_environment_policy.filters]
"секрет_*" = "include"
"#,
    );

    merge_toml_values(&mut base, &overlay);

    assert_eq!(base, overlay);
}

#[test]
fn shell_environment_policy_filters_replace_lower_legacy_filter_fields() {
    let mut base = parse_toml(
        r#"
[shell_environment_policy]
inherit = "core"
exclude = ["FLIP_TO_INCLUDE", "KEEP_EXCLUDED"]
include_only = ["FLIP_TO_EXCLUDE", "KEEP_INCLUDED"]
"#,
    );
    let overlay = parse_toml(
        r#"
[shell_environment_policy.filters]
"ADD_INCLUDED" = "include"
"FLIP_TO_EXCLUDE" = "exclude"
"FLIP_TO_INCLUDE" = "include"
"#,
    );

    merge_toml_values(&mut base, &overlay);

    assert_eq!(
        base,
        parse_toml(
            r#"
[shell_environment_policy]
inherit = "core"

[shell_environment_policy.filters]
"ADD_INCLUDED" = "include"
"FLIP_TO_EXCLUDE" = "exclude"
"FLIP_TO_INCLUDE" = "include"
"#,
        )
    );
}

#[test]
fn shell_environment_policy_legacy_arrays_replace_lower_filters() {
    let mut base = parse_toml(
        r#"
[shell_environment_policy]
inherit = "core"

[shell_environment_policy.filters]
"FLIP_TO_EXCLUDE" = "include"
"LOW_EXCLUDED" = "exclude"
"KEEP_INCLUDED" = "include"
"#,
    );
    let overlay = parse_toml(
        r#"
[shell_environment_policy]
exclude = ["FLIP_TO_EXCLUDE", "HIGH_EXCLUDED"]
"#,
    );

    merge_toml_values(&mut base, &overlay);

    assert_eq!(
        base,
        parse_toml(
            r#"
[shell_environment_policy]
inherit = "core"
exclude = ["FLIP_TO_EXCLUDE", "HIGH_EXCLUDED"]
"#,
        )
    );
}

#[test]
fn empty_shell_environment_filter_representations_replace_the_other_form() {
    let cases = [
        (
            r#"[shell_environment_policy]
exclude = ["AWS_*"]
include_only = ["PATH"]
"#,
            r#"[shell_environment_policy.filters]
"#,
        ),
        (
            r#"[shell_environment_policy.filters]
"AWS_*" = "include"
"#,
            r#"[shell_environment_policy]
exclude = []
"#,
        ),
    ];

    for (base, overlay) in cases {
        let mut base = parse_toml(base);
        let overlay = parse_toml(overlay);

        merge_toml_values(&mut base, &overlay);

        assert_eq!(base, overlay);
    }
}
