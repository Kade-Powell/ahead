use super::*;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

fn test_user_config_path(temp_dir: &TempDir, file_name: &str) -> AbsolutePathBuf {
    AbsolutePathBuf::from_absolute_path(temp_dir.path().join(file_name))
        .expect("test user config path should be absolute")
}

#[test]
fn origins_use_canonical_key_aliases() {
    let layer = ConfigLayerEntry::new(
        ConfigLayerSource::SessionFlags,
        toml::from_str(
            r#"
[memories]
no_memories_if_mcp_or_web_search = true
"#,
        )
        .expect("config TOML should parse"),
    );
    let metadata = layer.metadata();
    let stack = ConfigLayerStack::new(
        vec![layer],
        ConfigRequirements::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("single layer stack should be valid");

    let origins = stack.origins();

    assert_eq!(
        origins.get("memories.disable_on_external_context"),
        Some(&metadata)
    );
    assert!(
        !origins.contains_key("memories.no_memories_if_mcp_or_web_search"),
        "legacy key should be canonicalized before origin recording"
    );
}

/// Feature toggles and their concurrency setting keep independent origins.
#[test]
fn origins_attribute_multi_agent_v2_values_to_their_respective_layers() {
    let temp_dir = TempDir::new().expect("tempdir");
    let user_layer = ConfigLayerEntry::new(
        ConfigLayerSource::User {
            file: test_user_config_path(&temp_dir, "config.toml"),
        },
        toml::from_str(
            "[features.multi_agent_v2]\nenabled = true\nmax_concurrent_threads_per_session = 4\n",
        )
        .expect("user config"),
    );
    let user_metadata = user_layer.metadata();
    let session_layer = ConfigLayerEntry::new(
        ConfigLayerSource::SessionFlags,
        toml::from_str("[features]\nmulti_agent_v2 = false\n").expect("session config"),
    );
    let session_metadata = session_layer.metadata();
    let stack = ConfigLayerStack::new(
        vec![user_layer, session_layer],
        ConfigRequirements::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("layer stack should be valid");

    let origins = stack.origins();

    assert_eq!(
        origins.get("features.multi_agent_v2.enabled"),
        Some(&session_metadata)
    );
    assert_eq!(
        origins.get("features.multi_agent_v2.max_concurrent_threads_per_session"),
        Some(&user_metadata)
    );
}

#[test]
fn enabled_layers_validate_shell_environment_policy() {
    let layer = ConfigLayerEntry::new(
        ConfigLayerSource::SessionFlags,
        toml::from_str(
            r#"
[shell_environment_policy]
exclude = ["LEGACY_*"]

[shell_environment_policy.filters]
"CANONICAL_*" = "include"
"#,
        )
        .expect("session config"),
    );

    let error = ConfigLayerStack::new(
        vec![layer],
        ConfigRequirements::default(),
        ConfigRequirementsToml::default(),
    )
    .expect_err("enabled layers should be validated");

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(
        error
            .to_string()
            .contains("cannot mix `filters` with legacy `exclude` or `include_only`")
    );
}

#[test]
fn disabled_layers_do_not_validate_shell_environment_policy() {
    let temp_dir = TempDir::new().expect("tempdir");
    let layer = ConfigLayerEntry::new_disabled(
        ConfigLayerSource::User {
            file: test_user_config_path(&temp_dir, "config.toml"),
        },
        toml::from_str(
            r#"
[shell_environment_policy]
exclude = ["LEGACY_*"]

[shell_environment_policy.filters]
"CANONICAL_*" = "include"
"#,
        )
        .expect("project config"),
        "config is disabled",
    );

    ConfigLayerStack::new(
        vec![layer],
        ConfigRequirements::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("disabled layers should not be validated");
}

#[test]
fn enabled_layers_only_validate_representation_sensitive_shell_policy_fields() {
    let cases = [
        r#"shell_environment_policy = 17"#,
        r#"
[shell_environment_policy]
inherit = "invalid"
set = ["invalid"]
"#,
    ];

    for contents in cases {
        let layer = ConfigLayerEntry::new(
            ConfigLayerSource::SessionFlags,
            toml::from_str(contents).expect("session config"),
        );

        ConfigLayerStack::new(
            vec![layer],
            ConfigRequirements::default(),
            ConfigRequirementsToml::default(),
        )
        .expect("unrelated shell policy fields should retain normal overlay semantics");
    }
}

#[test]
fn config_layer_stack_rejects_malformed_user_shell_policy_filter_fields() {
    let temp_dir = TempDir::new().expect("tempdir");
    let config_file = test_user_config_path(&temp_dir, "config.toml");
    let cases = [
        r#"
[shell_environment_policy]
exclude = ["SECRET_*", 17]
"#,
        r#"
[shell_environment_policy.filters]
"SECRET_*" = "keep"
"#,
        r#"
[shell_environment_policy]
exclude = ["SECRET_*"]

[shell_environment_policy.filters]
"PATH" = "include"
"#,
        r#"
[shell_environment_policy.filters]
"SECRET_*" = "exclude"
"secret_*" = "include"
"#,
    ];

    for contents in cases {
        let user_layer = ConfigLayerEntry::new(
            ConfigLayerSource::User {
                file: config_file.clone(),
            },
            toml::from_str(contents).expect("user config"),
        );
        let error = ConfigLayerStack::new(
            vec![user_layer],
            ConfigRequirements::default(),
            ConfigRequirementsToml::default(),
        )
        .expect_err("malformed shell policy filter fields should be rejected");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
}

#[test]
fn layer_iterators_preserve_precedence_and_disabled_layers() {
    let temp_dir = TempDir::new().expect("tempdir");
    let user_source = ConfigLayerSource::User {
        file: test_user_config_path(&temp_dir, "config.toml"),
    };
    let disabled_packaged_source = ConfigLayerSource::PackagedDefaults {
        file: AbsolutePathBuf::from_absolute_path(temp_dir.path().join("defaults.toml"))
            .expect("packaged defaults path should be absolute"),
    };
    let session_source = ConfigLayerSource::SessionFlags;
    let empty_config = TomlValue::Table(toml::map::Map::new());
    let stack = ConfigLayerStack::new(
        vec![
            ConfigLayerEntry::new_disabled(
                disabled_packaged_source.clone(),
                empty_config.clone(),
                "config is disabled",
            ),
            ConfigLayerEntry::new(user_source.clone(), empty_config.clone()),
            ConfigLayerEntry::new(session_source.clone(), empty_config),
        ],
        ConfigRequirements::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("layer stack should be valid");

    assert_eq!(
        stack
            .layers_low_to_high()
            .map(|layer| &layer.name)
            .collect::<Vec<_>>(),
        vec![&user_source, &session_source]
    );
    assert_eq!(
        stack
            .layers_high_to_low()
            .map(|layer| &layer.name)
            .collect::<Vec<_>>(),
        vec![&session_source, &user_source]
    );
    assert_eq!(
        stack
            .all_layers_low_to_high()
            .map(|layer| &layer.name)
            .collect::<Vec<_>>(),
        vec![&disabled_packaged_source, &user_source, &session_source]
    );
    assert_eq!(
        stack
            .all_layers_high_to_low()
            .map(|layer| &layer.name)
            .collect::<Vec<_>>(),
        vec![&session_source, &user_source, &disabled_packaged_source]
    );
}
