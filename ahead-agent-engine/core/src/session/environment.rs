use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::protocol::EnvironmentConfig;
use codex_protocol::protocol::EnvironmentConfigState;
use codex_protocol::protocol::TurnEnvironmentSelection;

use crate::config::ConstraintError;
use crate::config::ConstraintResult;
use crate::session::session::Session;
use crate::session::session::SessionConfiguration;
use crate::session::session::SessionSettingsUpdate;
use codex_exec_server::LOCAL_ENVIRONMENT_ID;

pub(super) fn validate_environment_selections(
    selections: &[TurnEnvironmentSelection],
) -> ConstraintResult<()> {
    if selections.len() > 1 {
        return Err(ConstraintError::InvalidValue {
            field_name: "environments",
            candidate: format!("{} environment selections", selections.len()),
            allowed: "at most one local environment".to_string(),
            requirement_source: codex_config::RequirementSource::Unknown,
        });
    }

    for selection in selections {
        if selection.environment_id != LOCAL_ENVIRONMENT_ID {
            return Err(ConstraintError::InvalidValue {
                field_name: "environments",
                candidate: selection.environment_id.clone(),
                allowed: format!("{LOCAL_ENVIRONMENT_ID} environment"),
                requirement_source: codex_config::RequirementSource::Unknown,
            });
        }
        match &selection.config {
            EnvironmentConfigState::FromThread
            | EnvironmentConfigState::Pending
            | EnvironmentConfigState::Failed(_) => {}
            EnvironmentConfigState::Ready(config) => {
                validate_environment_config(config).map_err(|error| {
                    ConstraintError::InvalidValue {
                        field_name: "environments",
                        candidate: "environment configuration".to_string(),
                        allowed: format!("valid environment configuration ({error})"),
                        requirement_source: codex_config::RequirementSource::Unknown,
                    }
                })?;
            }
        }
    }
    Ok(())
}

fn validate_environment_config(config: &EnvironmentConfig) -> CodexResult<()> {
    if config
        .exec_policy
        .as_ref()
        .is_some_and(|policy| !policy.as_ref().get_allowed_prefixes().is_empty())
    {
        return Err(CodexErr::InvalidRequest(
            "environment command policy cannot contain allow rules".to_string(),
        ));
    }

    Ok(())
}

impl Session {
    pub(super) fn apply_session_settings(
        &self,
        current: &SessionConfiguration,
        updates: &SessionSettingsUpdate,
    ) -> ConstraintResult<SessionConfiguration> {
        let current_environments = self.services.turn_environments.selections();
        if let Some(environments) = &updates.environments
            && let Some(environment) = environments.environments.iter().find(|environment| {
                environment.config == EnvironmentConfigState::FromThread
                    && current_environments.iter().any(|current| {
                        current.environment_id == environment.environment_id
                            && current.config != EnvironmentConfigState::FromThread
                    })
            })
        {
            return Err(ConstraintError::InvalidValue {
                field_name: "environments",
                candidate: environment.environment_id.clone(),
                allowed: "owner-provided environment configuration".to_string(),
                requirement_source: codex_config::RequirementSource::Unknown,
            });
        }

        current.apply(updates, &current_environments)
    }

    pub(crate) async fn environment_ready(
        &self,
        selection: &TurnEnvironmentSelection,
        config: EnvironmentConfig,
    ) -> CodexResult<()> {
        validate_environment_config(&config)?;
        self.update_environment_configuration(selection, EnvironmentConfigState::Ready(config))
            .await
    }

    pub(crate) async fn environment_failed(
        &self,
        selection: &TurnEnvironmentSelection,
        error: String,
    ) -> CodexResult<()> {
        self.update_environment_configuration(selection, EnvironmentConfigState::Failed(error))
            .await
    }

    async fn update_environment_configuration(
        &self,
        selection: &TurnEnvironmentSelection,
        config: EnvironmentConfigState,
    ) -> CodexResult<()> {
        // Serialize owner callbacks with ordinary thread settings updates.
        let state = self.state.lock().await;
        let mut environments = self.services.turn_environments.selections();
        let Some(environment) = environments.iter_mut().find(|environment| {
            environment.environment_id == selection.environment_id
                && environment.cwd == selection.cwd
                && environment.workspace_roots == selection.workspace_roots
        }) else {
            return Err(CodexErr::InvalidRequest(format!(
                "environment `{}` is not selected on this thread with the requested workspace",
                selection.environment_id
            )));
        };

        environment.config = config;
        if matches!(environment.config, EnvironmentConfigState::Ready(_)) {
            state
                .session_configuration
                .validate(&environments)
                .map_err(|error| CodexErr::InvalidRequest(error.to_string()))?;
        }

        // Invalidate MCP before installed configuration can wake a waiting turn.
        self.mark_mcp_runtime_dirty();
        self.services.turn_environments.update_selections(
            &environments,
            &state.session_configuration.inferred_environment_config(),
        );
        Ok(())
    }
}
