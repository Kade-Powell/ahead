use crate::types::RawMcpServerConfig;
use codex_features::FEATURES;
use codex_protocol::protocol::GranularApprovalConfig;
use schemars::JsonSchema;
use schemars::r#gen::SchemaGenerator;
use schemars::schema::InstanceType;
use schemars::schema::ObjectValidation;
use schemars::schema::Schema;
use schemars::schema::SchemaObject;

/// Determines the conditions under which the user is consulted to approve
/// running the command proposed by Codex.
#[allow(dead_code)]
#[derive(JsonSchema)]
#[schemars(rename = "AskForApproval")]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ConfigAskForApproval {
    /// The model decides when to ask the user for approval.
    OnRequest,

    /// Fine-grained controls for individual approval flows.
    ///
    /// When a field is `true`, commands in that category are allowed. When it
    /// is `false`, those requests are automatically rejected instead of shown
    /// to the user.
    Granular(GranularApprovalConfig),

    /// Never ask the user to approve commands. Failures are immediately returned
    /// to the model, and never escalated to the user for approval.
    Never,
}

/// Schema for the `[features]` map with known + legacy keys only.
pub fn features_schema(schema_gen: &mut SchemaGenerator) -> Schema {
    let mut object = SchemaObject {
        instance_type: Some(InstanceType::Object.into()),
        ..Default::default()
    };

    let mut validation = ObjectValidation::default();
    for feature in FEATURES {
        if feature.id == codex_features::Feature::Artifact {
            continue;
        }
        if feature.id == codex_features::Feature::CodeMode {
            validation.properties.insert(
                feature.key.to_string(),
                schema_gen.subschema_for::<codex_features::FeatureToml<
                    codex_features::CodeModeConfigToml,
                >>(),
            );
            continue;
        }
        if feature.id == codex_features::Feature::CodeModeHost {
            validation.properties.insert(
                feature.key.to_string(),
                schema_gen.subschema_for::<codex_features::FeatureToml<
                    codex_features::CodeModeHostConfigToml,
                >>(),
            );
            continue;
        }
        if feature.id == codex_features::Feature::NonPrefixedMcpToolNames {
            validation.properties.insert(
                feature.key.to_string(),
                schema_gen.subschema_for::<codex_features::FeatureToml<
                    codex_features::NonPrefixedMcpToolNamesConfigToml,
                >>(),
            );
            continue;
        }
        if feature.id == codex_features::Feature::MultiAgentV2 {
            validation.properties.insert(
                feature.key.to_string(),
                schema_gen.subschema_for::<codex_features::FeatureToml<
                    codex_features::MultiAgentV2ConfigToml,
                >>(),
            );
            continue;
        }
        if feature.id == codex_features::Feature::TokenBudget {
            validation.properties.insert(
                feature.key.to_string(),
                schema_gen.subschema_for::<codex_features::FeatureToml<
                    codex_features::TokenBudgetConfigToml,
                >>(),
            );
            continue;
        }
        if feature.id == codex_features::Feature::RolloutBudget {
            validation.properties.insert(
                feature.key.to_string(),
                schema_gen.subschema_for::<codex_features::FeatureToml<
                    codex_features::RolloutBudgetConfigToml,
                >>(),
            );
            continue;
        }
        if feature.id == codex_features::Feature::CurrentTimeReminder {
            validation.properties.insert(
                feature.key.to_string(),
                schema_gen.subschema_for::<codex_features::FeatureToml<
                    codex_features::CurrentTimeReminderConfigToml,
                >>(),
            );
            continue;
        }
        if feature.id == codex_features::Feature::SleepTool {
            validation.properties.insert(
                feature.key.to_string(),
                schema_gen.subschema_for::<codex_features::FeatureToml<
                    codex_features::SleepToolConfigToml,
                >>(),
            );
            continue;
        }
        if feature.id == codex_features::Feature::NetworkProxy {
            validation.properties.insert(
                feature.key.to_string(),
                schema_gen.subschema_for::<codex_features::FeatureToml<
                    codex_features::NetworkProxyConfigToml,
                >>(),
            );
            continue;
        }
        validation
            .properties
            .insert(feature.key.to_string(), schema_gen.subschema_for::<bool>());
    }
    validation.properties.insert(
        "tool_registry".to_string(),
        schema_gen.subschema_for::<codex_features::ToolRegistryConfigToml>(),
    );
    validation.additional_properties = Some(Box::new(Schema::Bool(false)));
    object.object = Some(Box::new(validation));

    Schema::Object(object)
}

/// Schema for the `[mcp_servers]` map using the raw input shape.
pub fn mcp_servers_schema(schema_gen: &mut SchemaGenerator) -> Schema {
    let mut object = SchemaObject {
        instance_type: Some(InstanceType::Object.into()),
        ..Default::default()
    };

    let validation = ObjectValidation {
        additional_properties: Some(Box::new(schema_gen.subschema_for::<RawMcpServerConfig>())),
        ..Default::default()
    };
    object.object = Some(Box::new(validation));

    Schema::Object(object)
}
