use std::time::Duration;

pub(super) struct CodeModeHostTiming {
    thread_id: String,
    turn_id: String,
    call_id: String,
    pub(super) cell_id: Option<String>,
    tool_name: &'static str,
}

impl CodeModeHostTiming {
    pub(super) fn new(
        thread_id: String,
        turn_id: String,
        call_id: String,
        tool_name: &'static str,
    ) -> Self {
        Self {
            thread_id,
            turn_id,
            call_id,
            cell_id: None,
            tool_name,
        }
    }

    pub(super) fn record_code_mode_host_duration(&self, duration: Duration) {
        let Ok(code_mode_host_duration_ns) = u64::try_from(duration.as_nanos()) else {
            return;
        };
        // Bridge joins this record to the outer tool-completion event. Emit it
        // before the handler returns so consumers never need another timeout.
        tracing::info!(
            target: "codex_code_mode::timing",
            {
                event.name = "codex.code_mode.host_timing",
                conversation_id = %self.thread_id,
                turn_id = %self.turn_id,
                call_id = %self.call_id,
                cell_id = self.cell_id.as_deref(),
                tool_name = self.tool_name,
                code_mode_host_duration_ns,
            },
            "code-mode host operation completed"
        );
    }
}
