use serde::Deserialize;

use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_tools::ToolName;
use codex_tools::ToolSpec;

use super::DEFAULT_WAIT_YIELD_TIME_MS;
use super::ExecContext;
use super::WAIT_TOOL_NAME;
use super::handle_runtime_response;
use super::telemetry::CodeModeHostTiming;
use super::wait_spec::create_wait_tool;

pub struct CodeModeWaitHandler;

#[derive(Debug, Deserialize)]
struct ExecWaitArgs {
    cell_id: String,
    #[serde(default = "default_wait_yield_time_ms")]
    yield_time_ms: u64,
    #[serde(default)]
    max_tokens: Option<usize>,
    #[serde(default)]
    terminate: bool,
}

fn default_wait_yield_time_ms() -> u64 {
    DEFAULT_WAIT_YIELD_TIME_MS
}

fn parse_arguments<T>(arguments: &str) -> Result<T, FunctionCallError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_str(arguments).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to parse function arguments: {err}"))
    })
}

impl ToolExecutor<ToolInvocation> for CodeModeWaitHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(WAIT_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_wait_tool()
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(self.handle_call(invocation))
    }
}

impl CodeModeWaitHandler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            step_context,
            call_id,
            tool_name,
            payload,
            ..
        } = invocation;
        let turn = step_context.turn.clone();

        let mut timing = CodeModeHostTiming::new(
            session.thread_id.to_string(),
            turn.sub_id.clone(),
            call_id.clone(),
            WAIT_TOOL_NAME,
        );
        match payload {
            ToolPayload::Function { arguments }
                if tool_name.is_default_namespace()
                    && tool_name.name.as_str() == WAIT_TOOL_NAME =>
            {
                let args: ExecWaitArgs = parse_arguments(&arguments)?;
                let exec = ExecContext { session, turn };
                let started_at = std::time::Instant::now();
                timing.cell_id = Some(args.cell_id.clone());
                let cell_id = codex_code_mode::CellId::new(args.cell_id);
                let wait_response = if args.terminate {
                    exec.session
                        .services
                        .code_mode_service
                        .terminate(cell_id)
                        .await
                } else {
                    exec.session
                        .services
                        .code_mode_service
                        .wait(codex_code_mode::WaitRequest {
                            cell_id,
                            yield_time_ms: args.yield_time_ms,
                        })
                        .await
                }
                .map_err(FunctionCallError::RespondToModel)?;
                if let codex_code_mode::WaitOutcome::LiveCell(response) = &wait_response {
                    let runtime_cell_id = match response {
                        codex_code_mode::RuntimeResponse::Yielded { cell_id, .. }
                        | codex_code_mode::RuntimeResponse::Terminated { cell_id, .. }
                        | codex_code_mode::RuntimeResponse::Result { cell_id, .. } => cell_id,
                    };
                    timing.cell_id = Some(runtime_cell_id.to_string());
                    if let Some(executed_tool_calls) =
                        exec.session.services.executed_tool_calls.as_ref()
                    {
                        executed_tool_calls.register_cell(runtime_cell_id, &call_id);
                    }
                    if !matches!(response, codex_code_mode::RuntimeResponse::Yielded { .. }) {
                        exec.session
                            .services
                            .rollout_thread_trace
                            .code_cell_trace_context(
                                exec.turn.sub_id.as_str(),
                                runtime_cell_id.as_str(),
                            )
                            .record_ended(response);
                        exec.session
                            .services
                            .code_mode_service
                            .finish_cell_dispatch(runtime_cell_id);
                    }
                }
                if let Some(code_mode_host_duration) = wait_response.code_mode_host_duration() {
                    timing.record_code_mode_host_duration(code_mode_host_duration);
                }
                exec.session.services.elicitations.wait_until_clear().await;
                let wall_time = wait_response
                    .code_mode_host_duration()
                    .unwrap_or_else(|| started_at.elapsed());
                handle_runtime_response(&exec, wait_response.into(), args.max_tokens, wall_time)
                    .await
                    .map_err(FunctionCallError::RespondToModel)
                    .map(boxed_tool_output)
            }
            _ => Err(FunctionCallError::RespondToModel(format!(
                "{WAIT_TOOL_NAME} expects JSON arguments"
            ))),
        }
    }
}

impl CoreToolRuntime for CodeModeWaitHandler {}
