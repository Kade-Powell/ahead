use super::session::Session;
use super::turn_context::TurnContext;
use crate::config::Config;
use codex_protocol::config_types::AutoCompactTokenLimitScope;
use codex_protocol::openai_models::ModelInfo;

#[derive(Debug)]
pub(crate) struct ContextWindowTokenStatus {
    // Full active context usage, independent of the configured auto-compact scope.
    pub(crate) active_context_tokens: i64,
    // Usage counted against `model_auto_compact_token_limit` for the current scope.
    pub(crate) auto_compact_scope_tokens: i64,
    pub(crate) auto_compact_scope_limit: Option<i64>,
    buffered_auto_compact_limit: Option<i64>,
    pub(crate) full_context_window_limit: Option<i64>,
    pub(crate) base_window_tokens_remaining: Option<i64>,
    pub(crate) auto_compact_window_prefill_tokens: Option<i64>,
    pub(crate) full_context_window_limit_reached: bool,
    pub(crate) token_limit_reached: bool,
}

impl ContextWindowTokenStatus {
    pub(crate) fn token_limit_reached_with_pending_tokens(&self, pending_tokens: i64) -> bool {
        let pending_tokens = pending_tokens.max(0);
        self.buffered_auto_compact_limit.is_some_and(|limit| {
            self.auto_compact_scope_tokens
                .saturating_add(pending_tokens)
                >= limit
        }) || self
            .full_context_window_limit
            .is_some_and(|limit| self.active_context_tokens.saturating_add(pending_tokens) >= limit)
    }
}

fn tokens_remaining(limit: Option<i64>, used: i64) -> Option<i64> {
    limit.map(|limit| limit.saturating_sub(used).max(0))
}

pub(crate) async fn context_window_token_status(
    sess: &Session,
    turn_context: &TurnContext,
) -> ContextWindowTokenStatus {
    context_window_token_status_with_config(
        sess,
        turn_context.config.as_ref(),
        turn_context.model_info().as_ref(),
    )
    .await
}

async fn context_window_token_status_with_config(
    sess: &Session,
    config: &Config,
    model_info: &ModelInfo,
) -> ContextWindowTokenStatus {
    let active_context_tokens = sess.get_total_token_usage().await;
    let configured_auto_compact_token_limit =
        auto_compact_token_limit(config.model_auto_compact_token_limit, model_info);

    // Count either the full active context or only the tokens added after the initial prefix.
    let (auto_compact_scope_tokens, auto_compact_scope_limit, auto_compact_window_prefill_tokens) =
        match config.model_auto_compact_token_limit_scope {
            AutoCompactTokenLimitScope::Total => (
                active_context_tokens,
                configured_auto_compact_token_limit,
                None,
            ),
            AutoCompactTokenLimitScope::BodyAfterPrefix => {
                let window = sess.auto_compact_window_snapshot().await;
                let baseline = window.prefill_input_tokens.unwrap_or(active_context_tokens);

                (
                    active_context_tokens.saturating_sub(baseline),
                    configured_auto_compact_token_limit,
                    window.prefill_input_tokens,
                )
            }
        };

    // The model's full context window is a hard cap, independent of the auto-compaction scope.
    let full_context_window_limit = model_info.resolved_context_window().map(|context_window| {
        context_window.saturating_mul(model_info.effective_context_window_percent) / 100
    });

    // Report remaining tokens against the base (unbuffered) window, capped by the full context.
    let base_window_tokens_remaining = [
        tokens_remaining(auto_compact_scope_limit, auto_compact_scope_tokens),
        tokens_remaining(full_context_window_limit, active_context_tokens),
    ]
    .into_iter()
    .flatten()
    .min();

    // Only reserve the fallback buffer when there is a fallback prompt to use it.
    let auto_compact_fallback_buffer_tokens = config
        .token_budget
        .as_ref()
        .map_or(0, crate::config::TokenBudgetConfig::fallback_buffer_tokens);
    let buffered_auto_compact_limit = auto_compact_scope_limit
        .map(|limit| limit.saturating_add(auto_compact_fallback_buffer_tokens));

    let full_context_window_limit_reached =
        full_context_window_limit.is_some_and(|limit| active_context_tokens >= limit);
    let mut status = ContextWindowTokenStatus {
        active_context_tokens,
        auto_compact_scope_tokens,
        auto_compact_scope_limit,
        buffered_auto_compact_limit,
        full_context_window_limit,
        base_window_tokens_remaining,
        auto_compact_window_prefill_tokens,
        full_context_window_limit_reached,
        token_limit_reached: false,
    };
    status.token_limit_reached = status.token_limit_reached_with_pending_tokens(0);
    status
}

pub(super) fn auto_compact_token_limit(
    configured_limit: Option<i64>,
    model_info: &ModelInfo,
) -> Option<i64> {
    configured_limit.or_else(|| model_info.auto_compact_token_limit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_models_manager::model_info::model_info_from_slug;

    #[test]
    fn configured_auto_compact_limit_overrides_model_default() {
        let mut model_info = model_info_from_slug("model-with-default-limit");
        model_info.auto_compact_token_limit = Some(10_000);

        assert_eq!(
            auto_compact_token_limit(Some(1_000), &model_info),
            Some(1_000),
        );

        assert_eq!(auto_compact_token_limit(None, &model_info), Some(10_000),);
    }
}
