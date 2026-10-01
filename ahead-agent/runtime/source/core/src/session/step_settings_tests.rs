use super::*;
use crate::config::PermissionProfileState;
use crate::config::test_config;
use crate::session::session::SessionSettingsUpdate;
use crate::session::tests::make_session_configuration_for_tests;
use codex_features::Feature;
use codex_models_manager::manager::StaticModelsManager;
use codex_models_manager::model_info::model_info_from_slug;
use codex_protocol::config_types::ModeKind;
use codex_protocol::config_types::Settings;
use codex_protocol::models::BaseInstructionsProvenance;
use codex_protocol::models::PermissionProfile;
use codex_protocol::openai_models::ModelInstructionsVariables;
use codex_protocol::openai_models::ModelsResponse;
use codex_protocol::openai_models::TruncationPolicyConfig;
use pretty_assertions::assert_eq;
use std::sync::Arc;

pub(crate) fn update_selected_settings_for_test(
    settings: &mut ResolvedStepSettings,
    update: impl FnOnce(&mut StepSettings),
) {
    update(Arc::make_mut(&mut settings.selected));
}

#[tokio::test]
async fn proposed_permission_profile_is_checked_before_step_settings() {
    let mut configuration = make_session_configuration_for_tests().await;
    let permission = Constrained::allow_only(PermissionProfile::read_only());
    let permission_error = permission
        .can_set(&PermissionProfile::Disabled)
        .unwrap_err();
    configuration.permission_profile_state =
        PermissionProfileState::from_constrained_legacy(permission).unwrap();
    let approval = Constrained::allow_only(AskForApproval::OnRequest);
    let approval_error = approval.can_set(&AskForApproval::Never).unwrap_err();
    Arc::make_mut(&mut configuration.step_settings).approval_policy = approval;
    let invalid_profile = SessionSettingsUpdate {
        permission_profile: Some(PermissionProfile::Disabled),
        ..Default::default()
    };
    assert_eq!(
        configuration.apply(&invalid_profile, &[]).err().as_ref(),
        Some(&permission_error)
    );
    for (step_settings, expected) in [(
        StepSettingsUpdate {
            approval_policy: Some(AskForApproval::Never),
            ..Default::default()
        },
        &approval_error,
    )] {
        assert_eq!(
            configuration
                .step_settings
                .apply(
                    &step_settings,
                    &configuration.step_settings_constraints(&[]),
                )
                .err()
                .as_ref(),
            Some(expected),
        );
        assert_eq!(
            configuration
                .apply(
                    &SessionSettingsUpdate {
                        step_settings,
                        ..invalid_profile.clone()
                    },
                    &[]
                )
                .err()
                .as_ref(),
            Some(&permission_error),
        );
    }
}

fn configured_settings() -> StepSettings {
    StepSettings {
        collaboration_mode: CollaborationMode {
            mode: ModeKind::Default,
            settings: Settings {
                model: "model-a".to_string(),
                reasoning_effort: Some(ReasoningEffort::Low),
                developer_instructions: Some("keep these instructions".to_string()),
            },
        },
        reasoning_summary: Some(ReasoningSummary::Concise),
        service_tier: None,
        personality: Some(Personality::Friendly),
        approval_policy: Constrained::allow_any(AskForApproval::OnRequest),
    }
}

fn step_settings_constraints(requirements: &ConfigRequirements) -> StepSettingsConstraints<'_> {
    StepSettingsConstraints { requirements }
}

#[test]
fn sparse_patch_uses_the_settings_version_being_updated() {
    let requirements = ConfigRequirements::default();
    let constraints = step_settings_constraints(&requirements);
    let initial = configured_settings();
    let tier_update = StepSettingsUpdate {
        service_tier: Some(Some("fast".to_string())),
        ..Default::default()
    };
    let latest = initial
        .apply(
            &StepSettingsUpdate {
                model: Some("model-b".to_string()),
                effort: Some(Some(ReasoningEffort::High)),
                ..Default::default()
            },
            &constraints,
        )
        .expect("model update should apply");

    let updated = latest
        .apply(&tier_update, &constraints)
        .expect("a previously prepared sparse update should apply");
    let expected = StepSettings {
        service_tier: Some(ServiceTier::Fast.request_value().to_string()),
        ..latest
    };
    assert_eq!(updated, expected);
}

#[test]
fn collaboration_replacement_wins_and_effort_clear_remains_sparse() {
    let requirements = ConfigRequirements::default();
    let constraints = step_settings_constraints(&requirements);
    let initial = configured_settings();
    let replacement = CollaborationMode {
        mode: ModeKind::Plan,
        settings: Settings {
            model: "model-b".to_string(),
            reasoning_effort: Some(ReasoningEffort::Medium),
            developer_instructions: None,
        },
    };
    let replaced = initial
        .apply(
            &StepSettingsUpdate {
                model: Some("ignored-model".to_string()),
                effort: Some(Some(ReasoningEffort::High)),
                collaboration_mode: Some(replacement.clone()),
                ..Default::default()
            },
            &constraints,
        )
        .expect("collaboration mode should replace model and effort edits");
    assert_eq!(
        replaced,
        StepSettings {
            collaboration_mode: replacement.clone(),
            ..initial
        }
    );

    let cleared = replaced
        .apply(
            &StepSettingsUpdate {
                effort: Some(None),
                ..Default::default()
            },
            &constraints,
        )
        .expect("effort clear should apply");
    assert_eq!(
        cleared,
        StepSettings {
            collaboration_mode: replacement.with_updates(
                /*model*/ None,
                Some(None),
                /*developer_instructions*/ None,
            ),
            ..replaced
        }
    );
}

#[tokio::test]
async fn model_resolution_preserves_custom_instruction_provenance() {
    model_resolution_preserves_startup_overrides_and_instruction_provenance(
        BaseInstructionsProvenance::Custom,
    )
    .await;
}

#[tokio::test]
async fn model_resolution_preserves_model_instruction_provenance() {
    model_resolution_preserves_startup_overrides_and_instruction_provenance(
        BaseInstructionsProvenance::Model {
            model: "model-a".to_string(),
        },
    )
    .await;
}

async fn model_resolution_preserves_startup_overrides_and_instruction_provenance(
    provenance: BaseInstructionsProvenance,
) {
    let configured_instructions = "explicit {{ personality }}";
    let mut model = model_info_from_slug("model-b");
    model.context_window = Some(90_000);
    model.max_context_window = Some(100_000);
    model.auto_compact_token_limit = Some(80_000);
    model.truncation_policy = TruncationPolicyConfig::tokens(/*limit*/ 1_000);
    let messages = model
        .model_messages
        .as_mut()
        .expect("test model should have instruction metadata");
    messages.instructions_template =
        Some("Catalog B.\n# Personality\n{{ personality }}\n# Rules\nKeep the rules.".to_string());
    messages.instructions_variables = Some(ModelInstructionsVariables {
        personality_default: Some("default".to_string()),
        personality_friendly: Some("friendly".to_string()),
        personality_pragmatic: Some("pragmatic".to_string()),
    });
    let catalog = ModelsResponse {
        models: vec![model],
    };
    let models_manager = StaticModelsManager::new(/*auth_manager*/ None, catalog.clone());

    let mut config = test_config().await;
    config.model = Some("model-a".to_string());
    config.model_catalog = Some(catalog);
    config.model_context_window = Some(160_000);
    config.model_auto_compact_token_limit = Some(70_000);
    config.tool_output_token_limit = Some(777);
    config.base_instructions = Some(configured_instructions.to_string());
    let explicit_instructions = matches!(&provenance, BaseInstructionsProvenance::Custom);
    config.base_instructions_provenance = Some(provenance);

    // Capture the same filtered explicit overrides that session startup owns.
    let overrides = ModelInfoOverrides::from(config.to_models_manager_config());

    for (personality, personality_enabled, catalog_instructions) in [
        (
            Personality::Friendly,
            true,
            "Catalog B.\n# Personality\nfriendly\n# Rules\nKeep the rules.",
        ),
        (
            Personality::None,
            true,
            "Catalog B.\n# Rules\nKeep the rules.",
        ),
        (
            Personality::Friendly,
            false,
            "Catalog B.\n# Personality\ndefault\n# Rules\nKeep the rules.",
        ),
    ] {
        config.personality = Some(personality);
        config
            .features
            .set_enabled(Feature::Personality, personality_enabled)
            .expect("test config should allow personality changes");
        let mut settings = configured_settings();
        settings.collaboration_mode = settings.collaboration_mode.with_updates(
            Some("model-b".to_string()),
            /*effort*/ None,
            /*developer_instructions*/ None,
        );
        settings.personality = config.personality;

        let legacy = models_manager
            .get_model_info("model-b", &config.to_models_manager_config())
            .await;
        let resolved = settings
            .resolve_model_info(
                &models_manager,
                &overrides,
                config.features.enabled(Feature::Personality),
            )
            .await;
        assert_eq!(resolved, legacy);
        assert_eq!(
            resolved.get_model_instructions(settings.personality),
            if explicit_instructions {
                configured_instructions
            } else {
                catalog_instructions
            },
        );
    }
}
