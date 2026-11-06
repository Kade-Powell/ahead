use crate::Feature;
use crate::FeatureConfigSource;
use crate::FeatureOverrides;
use crate::FeatureToml;
use crate::Features;
use crate::FeaturesToml;
use crate::Stage;
use crate::feature_for_key;
use crate::unstable_features_warning_event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WarningEvent;
use std::collections::BTreeMap;
use toml::Table;
use toml::Value as TomlValue;

#[test]
fn retired_feature_keys_are_not_known() {
    let defaults = Features::with_defaults();
    let mut features = defaults.clone();
    let keys = [
        "transcript_v2",
        "guardian_reuse_parent_compaction",
        "guardian_enhanced_node_repl_transcripts",
        "guardian_node_repl_transcript_images",
        "guardian_ext",
        "guardian_approval",
    ];
    let entries = keys.iter().map(|key| ((*key).to_string(), true)).collect();

    for key in keys {
        assert!(!crate::is_known_feature_key(key));
    }
    features.apply_map(&entries);

    assert_eq!(features, defaults);
}

#[test]
fn sleep_tool_config_rejects_unknown_mode() {
    assert!(toml::from_str::<FeaturesToml>("[sleep_tool]\nmode = 'off'").is_err());
}

#[test]
fn under_development_features_are_disabled_by_default() {
    for spec in crate::FEATURES {
        if matches!(spec.stage, Stage::UnderDevelopment) {
            assert_eq!(
                spec.default_enabled, false,
                "feature `{}` is under development and must be disabled by default",
                spec.key
            );
        }
    }
}

#[test]
fn tool_registry_config_is_not_a_feature_toggle() {
    let features: FeaturesToml = toml::from_str(
        "[tool_registry]\nerror_on_tool_collisions = true\nturn_metadata_includes_tool_info = true\n",
    )
    .expect("tool registry settings should deserialize");

    assert_eq!(
        features.tool_registry,
        Some(crate::ToolRegistryConfigToml {
            error_on_tool_collisions: Some(true),
            turn_metadata_includes_tool_info: Some(true),
        })
    );
    assert!(features.entries().is_empty());
    assert!(crate::is_known_feature_key("tool_registry"));
    assert_eq!(feature_for_key("tool_registry"), None);
}

#[test]
fn cwd_relative_turn_diffs_is_an_opt_in_map_feature() {
    let mut features = Features::with_defaults();
    assert!(!features.enabled(Feature::CwdRelativeTurnDiffs));

    features.apply_map(&BTreeMap::from([(
        "cwd_relative_turn_diffs".to_string(),
        true,
    )]));

    assert!(features.enabled(Feature::CwdRelativeTurnDiffs));

    features.apply_map(&BTreeMap::from([(
        "cwd_relative_turn_diffs".to_string(),
        false,
    )]));

    assert!(!features.enabled(Feature::CwdRelativeTurnDiffs));
}

#[test]
fn default_enabled_features_are_stable() {
    for spec in crate::FEATURES {
        if spec.default_enabled {
            assert!(
                matches!(spec.stage, Stage::Stable),
                "feature `{}` is enabled by default but is not stable ({:?})",
                spec.key,
                spec.stage
            );
        }
    }
}

#[test]
fn code_mode_only_requires_code_mode() {
    let mut features = Features::with_defaults();
    features.enable(Feature::CodeModeOnly);
    features.normalize_dependencies();

    assert_eq!(features.enabled(Feature::CodeModeOnly), true);
    assert_eq!(features.enabled(Feature::CodeMode), true);
}

#[test]
fn code_mode_host_feature_config_preserves_boolean_toggle() {
    let features: FeaturesToml =
        toml::from_str("code_mode_host = false").expect("features table should deserialize");

    assert_eq!(features.code_mode_host, Some(FeatureToml::Enabled(false)));
    assert_eq!(
        features.entries(),
        BTreeMap::from([("code_mode_host".to_string(), false)])
    );
}

#[test]
fn code_mode_host_feature_config_deserializes_fallback_setting() {
    let features: FeaturesToml = toml::from_str(
        r#"
[code_mode_host]
enabled = true
disable_in_process_fallback = true
"#,
    )
    .expect("features table should deserialize");

    assert_eq!(
        features.code_mode_host,
        Some(FeatureToml::Config(crate::CodeModeHostConfigToml {
            enabled: Some(true),
            disable_in_process_fallback: Some(true),
        }))
    );
    assert_eq!(
        features.entries(),
        BTreeMap::from([("code_mode_host".to_string(), true)])
    );
}

#[test]
fn retired_feature_aliases_are_not_known() {
    for key in [
        "enable_experimental_windows_sandbox",
        "experimental_windows_sandbox",
        "elevated_windows_sandbox",
        "experimental_use_unified_exec_tool",
        "request_permissions",
        "web_search",
        "imagegenext",
        "collab",
        "memory_tool",
        "telepathy",
        "codex_hooks",
        "image_generation",
        "standalone_web_search",
    ] {
        assert_eq!(feature_for_key(key), None, "{key}");
        assert!(!crate::is_known_feature_key(key), "{key}");
    }
}

#[test]
fn retired_codex_product_features_are_not_known() {
    for key in [
        "artifact",
        "browser_use",
        "browser_use_external",
        "browser_use_full_cdp_access",
        "computer_use",
        "goals",
        "in_app_browser",
        "in_app_chat",
        "in_app_dictation",
        "in_app_local_automation",
        "in_app_updates",
        "mentions_v2",
        "memories",
        "prevent_idle_sleep",
        "realtime_conversation",
        "skill_search",
        "terminal_visualization_instructions",
        "workspace_dependencies",
    ] {
        assert_eq!(feature_for_key(key), None, "{key}");
        assert!(!crate::is_known_feature_key(key), "{key}");
    }
}

#[test]
fn from_sources_applies_base_profile_and_overrides() {
    let base_features = FeaturesToml::default();

    let mut profile_entries = BTreeMap::new();
    profile_entries.insert("code_mode_only".to_string(), true);
    let profile_features = FeaturesToml {
        entries: profile_entries,
        ..Default::default()
    };

    let features = Features::from_sources(
        FeatureConfigSource {
            features: Some(&base_features),
            ..Default::default()
        },
        FeatureConfigSource {
            features: Some(&profile_features),
            ..Default::default()
        },
        FeatureOverrides {
            web_search_request: Some(false),
        },
    );

    assert_eq!(features.enabled(Feature::CodeModeOnly), true);
    assert_eq!(features.enabled(Feature::CodeMode), true);
    assert_eq!(features.enabled(Feature::WebSearchRequest), false);
}

#[test]
fn multi_agent_v2_feature_config_deserializes_boolean_toggle() {
    let features: FeaturesToml = toml::from_str(
        r#"
multi_agent_v2 = true
"#,
    )
    .expect("features table should deserialize");

    assert_eq!(
        features.entries(),
        BTreeMap::from([("multi_agent_v2".to_string(), true)])
    );
    assert_eq!(features.multi_agent_v2, Some(FeatureToml::Enabled(true)));
}

#[test]
fn multi_agent_v2_feature_config_deserializes_table() {
    let features: FeaturesToml = toml::from_str(
        r#"
[multi_agent_v2]
enabled = true
max_concurrent_threads_per_session = 4
"#,
    )
    .expect("features table should deserialize");

    assert_eq!(
        features.entries(),
        BTreeMap::from([("multi_agent_v2".to_string(), true)])
    );
    assert_eq!(
        features.multi_agent_v2,
        Some(crate::FeatureToml::Config(crate::MultiAgentV2ConfigToml {
            enabled: Some(true),
            max_concurrent_threads_per_session: Some(4),
        }))
    );
}

#[test]
fn non_prefixed_mcp_tool_names_feature_config_deserializes_boolean_toggle() {
    let features: FeaturesToml = toml::from_str("non_prefixed_mcp_tool_names = true")
        .expect("features table should deserialize");

    assert_eq!(
        features.entries(),
        BTreeMap::from([("non_prefixed_mcp_tool_names".to_string(), true)])
    );
    assert_eq!(
        features.non_prefixed_mcp_tool_names,
        Some(FeatureToml::Enabled(true))
    );
}

#[test]
fn non_prefixed_mcp_tool_names_feature_config_deserializes_table() {
    let features: FeaturesToml = toml::from_str(
        r#"
[non_prefixed_mcp_tool_names]
enabled = true
server_names = ["history", "notes"]
"#,
    )
    .expect("features table should deserialize");

    assert_eq!(
        features.entries(),
        BTreeMap::from([("non_prefixed_mcp_tool_names".to_string(), true)])
    );
    assert_eq!(
        features.non_prefixed_mcp_tool_names,
        Some(FeatureToml::Config(
            crate::NonPrefixedMcpToolNamesConfigToml {
                enabled: Some(true),
                server_names: Some(vec!["history".to_string(), "notes".to_string()]),
            }
        ))
    );
}

#[test]
fn unstable_warning_event_only_mentions_enabled_under_development_features() {
    let mut configured_features = Table::new();
    configured_features.insert(
        "apply_patch_streaming_events".to_string(),
        TomlValue::Boolean(true),
    );
    configured_features.insert("personality".to_string(), TomlValue::Boolean(true));
    configured_features.insert("unknown".to_string(), TomlValue::Boolean(true));

    let mut features = Features::with_defaults();
    features.enable(Feature::ApplyPatchStreamingEvents);

    let warning = unstable_features_warning_event(
        Some(&configured_features),
        /*suppress_unstable_features_warning*/ false,
        &features,
        "/tmp/config.toml",
    )
    .expect("warning event");

    let EventMsg::Warning(WarningEvent { message }) = warning.msg else {
        panic!("expected warning event");
    };
    assert!(message.contains("apply_patch_streaming_events"));
    assert!(!message.contains("personality"));
    assert!(message.contains("/tmp/config.toml"));
}

#[test]
fn unstable_warning_event_ignores_enabled_structured_stable_feature() {
    let configured_features: Table = toml::from_str(
        r#"
multi_agent_v2 = { enabled = true, max_concurrent_threads_per_session = 4 }
code_mode = true
"#,
    )
    .expect("features table should deserialize");

    let mut features = Features::with_defaults();
    features.enable(Feature::MultiAgentV2);
    features.enable(Feature::CodeMode);

    let warning = unstable_features_warning_event(
        Some(&configured_features),
        /*suppress_unstable_features_warning*/ false,
        &features,
        "/tmp/config.toml",
    )
    .expect("warning event");

    let EventMsg::Warning(WarningEvent { message }) = warning.msg else {
        panic!("expected warning event");
    };
    assert_eq!(
        "Under-development features enabled: code_mode. Under-development features are incomplete and may behave unpredictably. To suppress this warning, set `suppress_unstable_features_warning = true` in /tmp/config.toml.".to_string(),
        message
    );
}
