use super::super::RequirementsLayerEntry;
use super::RequirementsCompositionError;
use super::compose_requirements_for_hostname;
use super::compose_requirements_with_hostname_resolver;
use crate::ConfigRequirementsToml;
use crate::ConfigRequirementsWithSources;
use crate::RequirementSource;
use crate::Sourced;
use codex_protocol::protocol::AskForApproval;
use codex_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;
use std::cell::Cell;
use std::collections::BTreeMap;
use tempfile::tempdir;

fn layer(id: &str, name: &str, contents: &str) -> RequirementsLayerEntry {
    RequirementsLayerEntry::from_toml(
        RequirementSource::MdmManagedPreferences {
            domain: name.to_string(),
            key: id.to_string(),
        },
        contents,
    )
}

fn compose(
    layers: Vec<RequirementsLayerEntry>,
) -> Result<Option<ConfigRequirementsToml>, RequirementsCompositionError> {
    Ok(
        compose_requirements_for_hostname(layers, /*hostname*/ None)?
            .map(ConfigRequirementsWithSources::into_toml),
    )
}

fn expected_requirements(contents: impl AsRef<str>) -> ConfigRequirementsToml {
    toml::from_str(contents.as_ref()).expect("parse expected requirements TOML")
}

#[test]
fn empty_layers_compose_to_none() {
    let composed = compose(Vec::new()).expect("compose empty layers");
    assert_eq!(composed, None);
}

#[test]
fn retired_runtime_requirements_are_rejected() {
    for key in [
        "allowed_login_methods",
        "allowed_chatgpt_workspaces",
        "cli_auth_credentials_store",
        "chatgpt_base_url",
        "feedback",
        "check_for_update_on_startup",
        "auto_review",
    ] {
        for source in [
            RequirementSource::Unknown,
            RequirementSource::MdmManagedPreferences {
                domain: "com.ahead.test".into(),
                key: "managed".into(),
            },
        ] {
            let error = compose(vec![RequirementsLayerEntry::from_toml(
                source.clone(),
                format!("{key} = []"),
            )])
            .expect_err("retired runtime policy must fail closed");
            assert_eq!(
                error,
                RequirementsCompositionError::Parse {
                    layer_source: source,
                    message: format!("unsupported runtime requirement `{key}`"),
                }
            );
        }
    }
}

#[test]
fn top_level_values_use_toml_priority() {
    let composed = compose(vec![
        layer(
            "req_low",
            "Low",
            r#"
allowed_approval_policies = ["on-request"]
allowed_sandbox_modes = ["workspace-write"]
default_permissions = ":workspace"
additional_developer_instructions = "Lower-priority instructions."

[allowed_permission_profiles]
":read-only" = true
":workspace" = true
"#,
        ),
        layer(
            "req_high",
            "High",
            r#"
allowed_approval_policies = ["never"]
allowed_sandbox_modes = ["read-only"]
default_permissions = ":read-only"
additional_developer_instructions = ""

[allowed_permission_profiles]
":danger-full-access" = false
":workspace" = false
"#,
        ),
    ])
    .expect("compose requirements")
    .expect("requirements present");

    assert_eq!(
        composed,
        expected_requirements(
            r#"
allowed_approval_policies = ["never"]
allowed_sandbox_modes = ["read-only"]
default_permissions = ":read-only"
additional_developer_instructions = ""

[allowed_permission_profiles]
":danger-full-access" = false
":read-only" = true
":workspace" = false
"#
        )
    );
}

#[test]
fn new_thread_model_defaults_use_toml_priority() {
    let composed = compose(vec![
        layer(
            "req_low",
            "Low",
            r#"
[models.new_thread]
model = "low-priority-model"
model_reasoning_effort = "low"
service_tier = "flex"
"#,
        ),
        layer(
            "req_high",
            "High",
            r#"
[models.new_thread]
model = "high-priority-model"
model_reasoning_effort = "high"
service_tier = "fast"
"#,
        ),
    ])
    .expect("compose requirements")
    .expect("requirements present");

    assert_eq!(
        composed,
        expected_requirements(
            r#"
[models.new_thread]
model = "high-priority-model"
model_reasoning_effort = "high"
service_tier = "fast"
"#
        )
    );
}

#[test]
fn model_defaults_use_regular_requirement_precedence() {
    let low = layer(
        "req_low",
        "Low",
        r#"[models.new_thread]
model = "low-priority-model"
model_reasoning_effort = "low""#,
    );
    let high = layer(
        "req_high",
        "High",
        r#"[models.new_thread]
model = "high-priority-model""#,
    );
    let composed = compose_requirements_for_hostname(vec![low, high], /*hostname*/ None)
        .expect("compose requirements")
        .expect("requirements present");

    assert_eq!(
        composed.clone().into_toml(),
        expected_requirements(
            r#"[models.new_thread]
model = "high-priority-model"
model_reasoning_effort = "low""#
        )
    );
}

#[test]
fn relative_paths_resolve_against_their_own_layer_base() {
    let low_dir = tempdir().expect("low-priority requirements directory");
    let high_dir = tempdir().expect("high-priority requirements directory");
    let low_base = AbsolutePathBuf::from_absolute_path(low_dir.path()).expect("absolute low base");
    let high_base =
        AbsolutePathBuf::from_absolute_path(high_dir.path()).expect("absolute high base");

    let composed = compose(vec![
        layer("req_low", "Low", "log_dir = \"low-logs\"").with_base_dir(low_base),
        layer(
            "req_high",
            "High",
            "log_dir = \"high-logs\"\nmodel_catalog_json = \"models.json\"",
        )
        .with_base_dir(high_base),
    ])
    .expect("compose requirements")
    .expect("requirements present");

    assert_eq!(
        composed.log_dir.as_deref(),
        Some(high_dir.path().join("high-logs").as_path())
    );
    assert_eq!(
        composed.model_catalog_json.as_deref(),
        Some(high_dir.path().join("models.json").as_path())
    );
}

#[test]
fn composition_strategy_applies_to_non_cloud_layers() {
    let mdm_source = RequirementSource::MdmManagedPreferences {
        domain: "com.ahead.editor".to_string(),
        key: "requirements_toml_base64".to_string(),
    };
    let system_file = if cfg!(windows) {
        "C:\\requirements.toml"
    } else {
        "/etc/ahead/requirements.toml"
    };
    let system_source = RequirementSource::SystemRequirementsToml {
        file: AbsolutePathBuf::from_absolute_path(system_file).expect("absolute path"),
    };
    let high_path = if cfg!(windows) {
        "C:\\secret"
    } else {
        "/secret"
    };
    let low_path = if cfg!(windows) {
        "C:\\other-secret"
    } else {
        "/other-secret"
    };

    let composed = compose_requirements_for_hostname(
        vec![
            RequirementsLayerEntry::from_toml(
                system_source,
                format!(
                    r#"
allowed_approval_policies = ["on-request"]

[features]
shared = false
system = true

[[rules.prefix_rules]]
pattern = [{{ token = "npm" }}]
decision = "prompt"

[permissions.filesystem]
deny_read = [{low_path:?}]
"#
                ),
            ),
            RequirementsLayerEntry::from_toml(
                mdm_source.clone(),
                format!(
                    r#"
allowed_approval_policies = ["never"]

[features]
shared = true

[[rules.prefix_rules]]
pattern = [{{ token = "git" }}]
decision = "forbidden"

[permissions.filesystem]
deny_read = [{high_path:?}]
"#
                ),
            ),
        ],
        /*hostname*/ None,
    )
    .expect("compose requirements")
    .expect("requirements present");

    assert_eq!(
        composed.clone().into_toml(),
        expected_requirements(format!(
            r#"
allowed_approval_policies = ["never"]

[features]
shared = true
system = true

[[rules.prefix_rules]]
pattern = [{{ token = "git" }}]
decision = "forbidden"

[[rules.prefix_rules]]
pattern = [{{ token = "npm" }}]
decision = "prompt"

[permissions.filesystem]
deny_read = [{high_path:?}, {low_path:?}]
"#
        ))
    );
    assert_eq!(
        composed.allowed_approval_policies,
        Some(Sourced::new(
            vec![AskForApproval::Never],
            mdm_source.clone()
        ))
    );
}

#[test]
fn regular_toml_merge_recurses_into_tables() {
    let composed = compose(vec![
        layer(
            "req_low",
            "Low",
            r#"
[features]
beta = false
shared = false
"#,
        ),
        layer(
            "req_high",
            "High",
            r#"
[features]
alpha = true
shared = true
"#,
        ),
    ])
    .expect("compose requirements")
    .expect("requirements present");

    assert_eq!(
        composed,
        expected_requirements(
            r#"
[features]
alpha = true
beta = false
shared = true
"#
        )
    );
}

#[test]
fn merged_table_source_is_composite_in_priority_order() {
    let high_source = RequirementSource::MdmManagedPreferences {
        domain: "com.ahead.test.high".to_string(),
        key: "req_high".to_string(),
    };
    let low_source = RequirementSource::MdmManagedPreferences {
        domain: "com.ahead.test.low".to_string(),
        key: "req_low".to_string(),
    };
    let composed = compose_requirements_for_hostname(
        vec![
            RequirementsLayerEntry::from_toml(
                low_source.clone(),
                r#"
[features]
beta = true
"#,
            ),
            RequirementsLayerEntry::from_toml(
                high_source.clone(),
                r#"
[features]
alpha = true
"#,
            ),
        ],
        /*hostname*/ None,
    )
    .expect("compose requirements")
    .expect("requirements present");

    assert_eq!(
        composed.feature_requirements.expect("features"),
        Sourced::new(
            crate::FeatureRequirementsToml {
                entries: BTreeMap::from([("alpha".to_string(), true), ("beta".to_string(), true),]),
            },
            RequirementSource::composite([high_source, low_source]),
        )
    );
}

#[test]
fn mcp_requirements_use_regular_toml_merge() {
    let composed = compose(vec![
        layer(
            "req_low",
            "Low",
            r#"
[mcp_servers.shared.identity]
command = "low-mcp"

[mcp_servers.low.identity]
command = "low-mcp"
"#,
        ),
        layer(
            "req_high",
            "High",
            r#"
[mcp_servers.shared.identity]
command = "high-mcp"
"#,
        ),
    ])
    .expect("compose requirements")
    .expect("requirements present");

    assert_eq!(
        composed,
        expected_requirements(
            r#"
[mcp_servers.low.identity]
command = "low-mcp"

[mcp_servers.shared.identity]
command = "high-mcp"
"#
        )
    );
}

#[test]
fn windows_requirements_use_regular_toml_merge() {
    let composed = compose(vec![
        layer(
            "req_low",
            "Low",
            r#"
[windows]
allowed_sandbox_implementations = ["unelevated"]
"#,
        ),
        layer(
            "req_high",
            "High",
            r#"
[windows]
allowed_sandbox_implementations = ["elevated"]
"#,
        ),
    ])
    .expect("compose requirements")
    .expect("requirements present");

    assert_eq!(
        composed,
        expected_requirements(
            r#"
[windows]
allowed_sandbox_implementations = ["elevated"]
"#
        )
    );
}

#[test]
fn remote_sandbox_config_is_applied_per_layer() {
    let composed = compose_requirements_for_hostname(
        vec![
            layer(
                "req_low",
                "Low",
                r#"
allowed_sandbox_modes = ["read-only"]
"#,
            ),
            layer(
                "req_high",
                "High",
                r#"
[[remote_sandbox_config]]
hostname_patterns = ["build-*.example.com"]
allowed_sandbox_modes = ["workspace-write"]
"#,
            ),
        ],
        Some("BUILD-01.EXAMPLE.COM."),
    )
    .expect("compose requirements")
    .expect("requirements present")
    .into_toml();

    assert_eq!(
        composed,
        expected_requirements(
            r#"
allowed_sandbox_modes = ["workspace-write"]
"#
        )
    );
}

#[test]
fn unmatched_remote_sandbox_config_does_not_shadow_lower_layers() {
    let composed = compose_requirements_for_hostname(
        vec![
            layer(
                "req_low",
                "Low",
                r#"
allowed_sandbox_modes = ["read-only"]
"#,
            ),
            layer(
                "req_high",
                "High",
                r#"
[[remote_sandbox_config]]
hostname_patterns = ["mac-*.example.com"]
allowed_sandbox_modes = ["workspace-write"]
"#,
            ),
        ],
        Some("linux-01.example.com"),
    )
    .expect("compose requirements")
    .expect("requirements present")
    .into_toml();

    assert_eq!(
        composed,
        expected_requirements(
            r#"
allowed_sandbox_modes = ["read-only"]
"#
        )
    );
}

#[test]
fn hostname_resolver_is_not_called_without_remote_sandbox_config() {
    let calls = Cell::<usize>::default();
    let composed = compose_requirements_with_hostname_resolver(
        vec![layer(
            "req",
            "No remote selector",
            r#"
allowed_sandbox_modes = ["read-only"]
"#,
        )],
        || {
            calls.set(calls.get() + 1);
            Some("build-01.example.com".to_string())
        },
    )
    .expect("compose requirements")
    .expect("requirements present")
    .into_toml();

    assert_eq!(calls.get(), 0);
    assert_eq!(
        composed,
        expected_requirements(
            r#"
allowed_sandbox_modes = ["read-only"]
"#
        )
    );
}

#[test]
fn hostname_resolver_is_called_once_for_multiple_remote_sandbox_layers() {
    let calls = Cell::<usize>::default();
    let composed = compose_requirements_with_hostname_resolver(
        vec![
            layer(
                "req_low",
                "Low",
                r#"
[[remote_sandbox_config]]
hostname_patterns = ["build-*.example.com"]
allowed_sandbox_modes = ["read-only"]
"#,
            ),
            layer(
                "req_high",
                "High",
                r#"
[[remote_sandbox_config]]
hostname_patterns = ["build-*.example.com"]
allowed_sandbox_modes = ["workspace-write"]
"#,
            ),
        ],
        || {
            calls.set(calls.get() + 1);
            Some("build-01.example.com".to_string())
        },
    )
    .expect("compose requirements")
    .expect("requirements present")
    .into_toml();

    assert_eq!(calls.get(), 1);
    assert_eq!(
        composed,
        expected_requirements(
            r#"
allowed_sandbox_modes = ["workspace-write"]
"#
        )
    );
}

#[test]
fn rules_are_appended_in_priority_order() {
    let composed = compose(vec![
        layer(
            "req_low",
            "Low",
            r#"
[[rules.prefix_rules]]
pattern = [{ token = "npm" }]
decision = "prompt"
"#,
        ),
        layer(
            "req_high",
            "High",
            r#"
[[rules.prefix_rules]]
pattern = [{ token = "git" }]
decision = "forbidden"
"#,
        ),
    ])
    .expect("compose requirements")
    .expect("requirements present");

    assert_eq!(
        composed,
        expected_requirements(
            r#"
[[rules.prefix_rules]]
pattern = [{ token = "git" }]
decision = "forbidden"

[[rules.prefix_rules]]
pattern = [{ token = "npm" }]
decision = "prompt"
"#
        )
    );
}

#[test]
fn permissions_deny_read_unions_while_profiles_use_regular_toml_merge() {
    let high_path = if cfg!(windows) {
        "C:\\secret"
    } else {
        "/secret"
    };
    let low_path = if cfg!(windows) {
        "C:\\other-secret"
    } else {
        "/other-secret"
    };
    let composed = compose(vec![
        layer(
            "req_low",
            "Low",
            &format!(
                r#"
[permissions.filesystem]
deny_read = [{high_path:?}, {low_path:?}]

[permissions.managed-standard]
description = "Low profile"
extends = ":workspace"
"#
            ),
        ),
        layer(
            "req_high",
            "High",
            &format!(
                r#"
[permissions.filesystem]
deny_read = [{high_path:?}]

[permissions.managed-standard]
description = "High profile"
"#
            ),
        ),
    ])
    .expect("compose requirements")
    .expect("requirements present");

    assert_eq!(
        composed,
        expected_requirements(format!(
            r#"
[permissions.filesystem]
deny_read = [{high_path:?}, {low_path:?}]

[permissions.managed-standard]
description = "High profile"
extends = ":workspace"
"#
        ))
    );
}

#[test]
fn deny_read_only_layers_do_not_leave_empty_permissions_tables() {
    let path = if cfg!(windows) {
        "C:\\secret"
    } else {
        "/secret"
    };
    let composed = compose(vec![layer(
        "req_high",
        "High",
        &format!(
            r#"
[permissions.filesystem]
deny_read = [{path:?}]
"#
        ),
    )])
    .expect("compose requirements")
    .expect("requirements present");

    assert_eq!(
        composed,
        expected_requirements(format!(
            r#"
[permissions.filesystem]
deny_read = [{path:?}]
"#
        ))
    );
}

#[test]
fn parse_error_names_layer() {
    let err = compose(vec![layer(
        "req_bad",
        "Bad layer",
        "allowed_approval_policies = [1]",
    )])
    .expect_err("invalid layer should fail");

    assert!(err.to_string().contains("MDM Bad layer:req_bad"));
    assert!(err.to_string().contains("allowed_approval_policies"));
}
