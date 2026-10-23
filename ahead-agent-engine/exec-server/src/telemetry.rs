use std::time::Duration;

use codex_otel::MetricsClient;
use tracing::warn;

#[derive(Clone, Default)]
pub(crate) struct AgentExecutionTelemetry {
    metrics: Option<MetricsClient>,
}

impl AgentExecutionTelemetry {
    #[cfg(test)]
    pub(crate) fn new(metrics: MetricsClient) -> Self {
        Self {
            metrics: Some(metrics),
        }
    }

    #[cfg(unix)]
    pub(crate) fn shell_snapshot_captured(
        &self,
        duration: Duration,
        result: Result<(), &'static str>,
    ) {
        let Some(metrics) = self.metrics.clone().or_else(codex_otel::global) else {
            return;
        };
        let success = if result.is_ok() { "true" } else { "false" };
        let mut tags = vec![("success", success)];
        if let Err(reason) = result {
            tags.push(("failure_reason", reason));
        }
        if metrics
            .record_duration_seconds_with_description(
                "ahead_agent_shell_snapshot_duration_seconds",
                "Duration of AHEAD agent shell snapshot capture in seconds.",
                duration,
                &tags,
            )
            .is_err()
            || metrics
                .counter_with_description(
                    "ahead_agent_shell_snapshot_total",
                    "Total number of AHEAD agent shell snapshot captures.",
                    1,
                    &tags,
                )
                .is_err()
        {
            warn!("failed to emit AHEAD agent shell snapshot metrics");
        }
    }
}
