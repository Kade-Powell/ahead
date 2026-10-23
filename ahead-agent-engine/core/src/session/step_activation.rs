//! Restricted updates to a running turn's immutable settings snapshots.

use super::session::Session;
use super::session::SessionConfiguration;
use super::step_settings::ResolvedStepSettings;
use super::step_settings::StepSettingsConstraints;
use super::step_settings::StepSettingsUpdate;
use super::turn_context::TurnContext;
use crate::config::ConstraintResult;
use codex_features::Feature;
use codex_protocol::protocol::TurnSettingsUpdate;
use codex_protocol::protocol::TurnSettingsUpdateOutcome;
use std::sync::Arc;

/// Approvals still read the admitted `TurnContext`, so a live model update may
/// not change its authorization policy.
fn check_turn_approval_safety(
    turn_context: &TurnContext,
    current: &ResolvedStepSettings,
    destination: &ResolvedStepSettings,
) -> Result<(), String> {
    if destination.constrained_approval_policy() != current.constrained_approval_policy()
        || destination.approval_policy() != turn_context.approval_policy()
    {
        return Err("the destination changes the admitted approval policy".to_string());
    }
    Ok(())
}

impl Session {
    /// Publishes settings to the named, originally captured live task, regardless
    /// of task kind. Publication does not propagate to child sessions or require
    /// the task to sample; consumers using initial settings remain unchanged.
    ///
    /// Callers must serialize updates through completion, including model
    /// resolution, so each sparse patch sees the preceding publication.
    #[expect(
        clippy::await_holding_invalid_type,
        reason = "the final managed-policy check and active settings publication must remain atomic"
    )]
    pub(super) async fn apply_turn_settings(
        &self,
        turn_id: &str,
        update: TurnSettingsUpdate,
    ) -> TurnSettingsUpdateOutcome {
        if !self.features.enabled(Feature::StepModelSwitching) {
            return TurnSettingsUpdateOutcome::Rejected {
                reason: "turn settings updates require the step_model_switching feature"
                    .to_string(),
            };
        }

        // Capture the exact live task and its settings, then release the
        // lock. A task that starts during preparation is never a new target.
        let target = {
            let active = self.active_turn.lock().await;
            active.as_ref().and_then(|active| {
                active.task.as_ref().and_then(|task| {
                    (task.turn_context.sub_id == turn_id && !task.cancellation_token.is_cancelled())
                        .then(|| {
                            (
                                Arc::clone(&task.turn_context),
                                Arc::clone(&task.done),
                                task.turn_context.current_settings.load_full(),
                            )
                        })
                })
            })
        };
        let Some((turn_context, task_done, current)) = target else {
            return TurnSettingsUpdateOutcome::TargetUnavailable;
        };
        let TurnSettingsUpdate {
            model,
            effort,
            summary,
            service_tier,
        } = update;
        let update = StepSettingsUpdate {
            model,
            effort,
            reasoning_summary: summary,
            service_tier,
            ..Default::default()
        };
        // Apply the sparse patch to the captured active base using the shared
        // settings rules. The task can progress, finish, or be cancelled while
        // preparation awaits; no publication locks are held here.
        let prepared = self
            .prepare_step_settings_activation(&turn_context, &current, &update)
            .await;
        let active = self.active_turn.lock().await;
        let Some(task) = active.as_ref().and_then(|active| active.task.as_ref()) else {
            return TurnSettingsUpdateOutcome::TargetUnavailable;
        };
        // A later task may reuse the same context and turn ID. `done` is
        // allocated per task, so matching only the ID/context is insufficient.
        // A mismatch abandons the update without retrying or retargeting.
        if !Arc::ptr_eq(&task.done, &task_done)
            || !Arc::ptr_eq(&task.turn_context, &turn_context)
            || !Arc::ptr_eq(&task.turn_context.current_settings.load_full(), &current)
            || task.cancellation_token.is_cancelled()
        {
            return TurnSettingsUpdateOutcome::TargetUnavailable;
        }
        let destination = match prepared {
            Ok(destination) => destination,
            Err(reason) => return TurnSettingsUpdateOutcome::Rejected { reason },
        };
        // Managed requirements can change during resolution. Keep the live
        // authorization and safety checks atomic with publication under state
        // and active_turn; no asynchronous preparation runs under these locks.
        let state = self.state.lock().await;
        if let Err(reason) = self
            .validate_active_step_settings(
                &turn_context,
                &destination,
                &state.session_configuration,
            )
            .map_err(|error| error.to_string())
            .and_then(|()| check_turn_approval_safety(&turn_context, &current, &destination))
        {
            return TurnSettingsUpdateOutcome::Rejected { reason };
        }
        // Publish the immutable snapshot. Frozen initial settings, existing step
        // captures, and future thread settings are not changed.
        task.turn_context
            .current_settings
            .store(Arc::new(destination));
        TurnSettingsUpdateOutcome::Applied
    }

    async fn prepare_step_settings_activation(
        &self,
        _turn_context: &TurnContext,
        current: &ResolvedStepSettings,
        update: &StepSettingsUpdate,
    ) -> Result<ResolvedStepSettings, String> {
        let (requirements, overrides) = {
            let state = self.state.lock().await;
            let configuration = &state.session_configuration;
            let stack = &configuration.original_config_do_not_use.config_layer_stack;
            (
                stack.requirements().clone(),
                configuration.model_info_overrides.clone(),
            )
        };
        let constraints = StepSettingsConstraints {
            requirements: &requirements,
        };
        current
            .apply_update(
                update,
                &constraints,
                self.services.models_manager.as_ref(),
                &overrides,
                self.features.enabled(Feature::Personality),
                self.features.enabled(Feature::FastMode),
            )
            .await
            .map_err(|error| error.to_string())
    }

    /// Rechecks ordinary managed authorization after asynchronous resolution.
    /// Unlike the temporary legacy-turn check, these requirements also apply
    /// once all execution consumers read their captured `StepContext`.
    fn validate_active_step_settings(
        &self,
        _turn_context: &TurnContext,
        settings: &ResolvedStepSettings,
        configuration: &SessionConfiguration,
    ) -> ConstraintResult<()> {
        let requirements = configuration
            .original_config_do_not_use
            .config_layer_stack
            .requirements();
        settings.revalidate(&StepSettingsConstraints { requirements })
    }
}

#[cfg(test)]
#[path = "step_activation_tests.rs"]
mod tests;
