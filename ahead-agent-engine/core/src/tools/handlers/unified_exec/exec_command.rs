use std::sync::Arc;
use std::time::Duration;

use crate::exec::DEFAULT_EXEC_COMMAND_TIMEOUT_MS;
use crate::exec_policy::prompt_is_rejected_by_policy;
use crate::function_tool::FunctionCallError;
use crate::maybe_emit_implicit_skill_invocation;
use crate::tools::context::ExecCommandToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::apply_granted_turn_permissions;
use crate::tools::handlers::apply_patch::intercept_apply_patch;
use crate::tools::handlers::implicit_granted_permissions;
use crate::tools::handlers::normalize_and_validate_additional_permissions;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::parse_arguments_with_base_path;
use crate::tools::handlers::resolve_sandbox_permissions;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use crate::unified_exec::ExecCommandRequest;
use crate::unified_exec::UnifiedExecContext;
use crate::unified_exec::UnifiedExecError;
use crate::unified_exec::UnifiedExecProcessManager;
use crate::unified_exec::generate_chunk_id;
use codex_features::Feature;
use codex_otel::SessionTelemetry;
use codex_otel::TOOL_CALL_UNIFIED_EXEC_METRIC;
use codex_tools::JsonSchema;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use codex_utils_output_truncation::approx_token_count;

use super::super::shell_spec::CommandToolOptions;
use super::super::shell_spec::create_exec_command_tool_with_options;
use super::ExecCommandArgs;
use super::ExecCommandLocationArgs;
use super::get_command;

#[derive(Clone, Copy)]
pub(crate) struct ExecCommandHandlerOptions {
    pub(crate) allow_login_shell: bool,
    pub(crate) exec_permission_approvals_enabled: bool,
    pub(crate) include_shell_parameter: bool,
    pub(crate) include_windows_shell_guidance: bool,
}

#[derive(Clone, Copy)]
enum ExecCommandLifetime {
    Interactive,
    OneShot,
}

pub struct ExecCommandHandler {
    options: ExecCommandHandlerOptions,
    lifetime: ExecCommandLifetime,
}

impl Default for ExecCommandHandler {
    fn default() -> Self {
        Self {
            lifetime: ExecCommandLifetime::Interactive,
            options: ExecCommandHandlerOptions {
                allow_login_shell: false,
                exec_permission_approvals_enabled: false,
                include_shell_parameter: true,
                include_windows_shell_guidance: cfg!(windows),
            },
        }
    }
}

impl ExecCommandHandler {
    pub(crate) fn new(options: ExecCommandHandlerOptions) -> Self {
        Self {
            options,
            lifetime: ExecCommandLifetime::Interactive,
        }
    }

    pub(crate) fn one_shot(options: ExecCommandHandlerOptions) -> Self {
        Self {
            options,
            lifetime: ExecCommandLifetime::OneShot,
        }
    }
}

impl ToolExecutor<ToolInvocation> for ExecCommandHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("exec_command")
    }

    fn spec(&self) -> ToolSpec {
        let spec = create_exec_command_tool_with_options(
            CommandToolOptions {
                allow_login_shell: self.options.allow_login_shell,
                exec_permission_approvals_enabled: self.options.exec_permission_approvals_enabled,
            },
            self.options.include_shell_parameter,
            self.options.include_windows_shell_guidance,
        );
        match self.lifetime {
            ExecCommandLifetime::Interactive => spec,
            ExecCommandLifetime::OneShot => one_shot_exec_command_spec(spec),
        }
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(self.handle_call(invocation))
    }
}

impl ExecCommandHandler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            step_context,
            cancellation_token,
            tracker,
            call_id,
            payload,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "exec_command handler received unsupported payload".to_string(),
                ));
            }
        };

        let manager: &UnifiedExecProcessManager = &session.services.unified_exec_manager;
        let context = UnifiedExecContext::new(
            session.clone(),
            step_context.clone(),
            cancellation_token,
            call_id.clone(),
        );
        let environment_args: ExecCommandLocationArgs = parse_arguments(&arguments)?;
        let Some(turn_environment) = resolve_tool_environment(&step_context.environments) else {
            return Err(FunctionCallError::RespondToModel(
                "unified exec is unavailable in this session".to_string(),
            ));
        };
        let native_environment_cwd = turn_environment.cwd().clone();
        let cwd = environment_args
            .workdir
            .as_deref()
            .filter(|workdir| !workdir.is_empty())
            .map_or_else(
                || Ok(native_environment_cwd.clone()),
                |workdir| native_environment_cwd.join(workdir),
            )
            .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?;
        let environment = Arc::clone(&turn_environment.environment);
        let fs = environment.get_filesystem();

        let native_cwd = cwd
            .to_abs_path()
            .map_err(|error| FunctionCallError::RespondToModel(error.to_string()))?;
        let args: ExecCommandArgs = parse_arguments_with_base_path(&arguments, &native_cwd)?;
        let sandbox_permissions =
            resolve_sandbox_permissions(args.sandbox_permissions, args.justification.as_deref())?;
        let command_text = args.cmd.clone();
        maybe_emit_implicit_skill_invocation(
            context.step_context.turn.as_ref(),
            &command_text,
            &cwd,
            Some(&native_cwd),
            &turn_environment.selection.environment_id,
        )
        .await;
        let shell = turn_environment
            .shell
            .clone()
            .map(Arc::new)
            .unwrap_or_else(|| session.user_shell());
        let process_id = manager.allocate_process_id().await;
        let resolved_command =
            get_command(&args, shell, turn_environment.config().allow_login_shell)
                .map_err(FunctionCallError::RespondToModel)?;
        let command = resolved_command.command;
        let shell_type = resolved_command.shell_type;
        let command_for_display = codex_shell_command::parse_command::shlex_join(&command);

        let ExecCommandArgs {
            mut tty,
            yield_time_ms,
            timeout_ms,
            max_output_tokens,
            sandbox_permissions: _,
            additional_permissions,
            justification,
            prefix_rule,
            ..
        } = args;
        let completion_timeout = match self.lifetime {
            ExecCommandLifetime::Interactive => None,
            ExecCommandLifetime::OneShot => {
                tty = false;
                Some(Duration::from_millis(
                    timeout_ms.unwrap_or(DEFAULT_EXEC_COMMAND_TIMEOUT_MS),
                ))
            }
        };

        let exec_permission_approvals_enabled =
            session.features().enabled(Feature::ExecPermissionApprovals);
        let requested_additional_permissions = additional_permissions.clone();
        let permission_cwd = &native_cwd;
        let effective_additional_permissions = apply_granted_turn_permissions(
            context.session.as_ref(),
            &turn_environment.selection.environment_id,
            permission_cwd.as_path(),
            sandbox_permissions,
            additional_permissions,
        )
        .await;
        let additional_permissions_allowed = exec_permission_approvals_enabled
            || (session.features().enabled(Feature::RequestPermissionsTool)
                && effective_additional_permissions.permissions_preapproved);

        // Sticky turn permissions have already been approved, so they should
        // continue through the normal exec approval flow for the command.
        let approval_policy = context.step_context.settings.approval_policy();
        if effective_additional_permissions
            .sandbox_permissions
            .requests_sandbox_override()
            && !effective_additional_permissions.permissions_preapproved
            && prompt_is_rejected_by_policy(approval_policy, /*prompt_is_rule*/ false).is_some()
        {
            manager.release_process_id(process_id).await;
            return Err(FunctionCallError::RespondToModel(format!(
                "approval policy is {approval_policy:?}; reject command — you cannot ask for escalated permissions if the approval policy is {approval_policy:?}"
            )));
        }

        let normalized_additional_permissions = match implicit_granted_permissions(
            sandbox_permissions,
            requested_additional_permissions.as_ref(),
            &effective_additional_permissions,
        )
        .map_or_else(
            || {
                normalize_and_validate_additional_permissions(
                    additional_permissions_allowed,
                    approval_policy,
                    effective_additional_permissions.sandbox_permissions,
                    effective_additional_permissions.additional_permissions,
                    effective_additional_permissions.permissions_preapproved,
                    permission_cwd,
                )
            },
            |permissions| Ok(Some(permissions)),
        ) {
            Ok(normalized) => normalized,
            Err(err) => {
                manager.release_process_id(process_id).await;
                return Err(FunctionCallError::RespondToModel(err));
            }
        };

        let intercepted_patch = intercept_apply_patch(
            &command,
            &cwd,
            fs.as_ref(),
            turn_environment.clone(),
            context.session.clone(),
            Arc::clone(&context.step_context),
            Some(&tracker),
            &context.call_id,
            "exec_command",
        )
        .await;
        // Keep the reservation when interception returns `Ok(None)`: the normal command below
        // still needs this process ID.
        if intercepted_patch.is_err() {
            manager.release_process_id(process_id).await;
        }
        if let Some(output) = intercepted_patch? {
            manager.release_process_id(process_id).await;
            return Ok(boxed_tool_output(ExecCommandToolOutput {
                chunk_id: String::new(),
                wall_time: std::time::Duration::ZERO,
                raw_output: output.into_text().into_bytes(),
                truncation_policy: turn.model_info().truncation_policy.into(),
                max_output_tokens,
                process_id: None,
                exit_code: None,
                original_token_count: None,
                output_omitted_bytes: None,
            }));
        }

        emit_unified_exec_tty_metric(&turn.session_telemetry, tty);
        let request = ExecCommandRequest {
            command,
            shell_type,
            command_text: command_text.clone(),
            process_id,
            yield_time_ms,
            max_output_tokens,
            cwd,
            sandbox_cwd: native_environment_cwd,
            turn_environment: turn_environment.clone(),
            tty,
            sandbox_permissions: effective_additional_permissions.sandbox_permissions,
            additional_permissions: normalized_additional_permissions,
            additional_permissions_preapproved: effective_additional_permissions
                .permissions_preapproved,
            justification,
            prefix_rule,
        };
        let result = match completion_timeout {
            Some(timeout) => {
                UnifiedExecProcessManager::exec_command_to_completion(request, &context, timeout)
                    .await
            }
            None => manager.exec_command(request, &context).await,
        };
        match result {
            Ok(response) => Ok(boxed_tool_output(response)),
            Err(UnifiedExecError::SandboxDenied {
                output,
                original_token_count,
                output_omitted_bytes,
                ..
            }) => {
                let output_text = output.aggregated_output.text;
                let original_token_count =
                    original_token_count.unwrap_or_else(|| approx_token_count(&output_text));
                Ok(boxed_tool_output(ExecCommandToolOutput {
                    chunk_id: generate_chunk_id(),
                    wall_time: output.duration,
                    raw_output: output_text.into_bytes(),
                    truncation_policy: turn.model_info().truncation_policy.into(),
                    max_output_tokens,
                    // Sandbox denial is terminal, so there is no live
                    // process for write_stdin to resume.
                    process_id: None,
                    exit_code: Some(output.exit_code),
                    original_token_count: Some(original_token_count),
                    output_omitted_bytes,
                }))
            }
            Err(err) => Err(FunctionCallError::RespondToModel(format!(
                "exec_command failed for `{command_for_display}`: {err:?}"
            ))),
        }
    }
}

fn one_shot_exec_command_spec(spec: ToolSpec) -> ToolSpec {
    let ToolSpec::Function(mut spec) = spec else {
        unreachable!("exec_command has a function schema");
    };
    spec.description = spec.description.replacen(
        "Runs a command in a PTY, returning output or a session ID for ongoing interaction.",
        "Runs a command to completion and returns its output. The process is terminated on timeout or cancellation and cannot be resumed.",
        1,
    );
    let properties = spec.parameters.properties.get_or_insert_default();
    properties.remove("tty");
    properties.remove("yield_time_ms");
    properties.insert(
        "timeout_ms".to_string(),
        JsonSchema::number(Some(
            "Maximum command runtime. Defaults to 10000 ms.".to_string(),
        )),
    );
    if let Some(output_properties) = spec
        .output_schema
        .as_mut()
        .and_then(|schema| schema.get_mut("properties"))
        .and_then(serde_json::Value::as_object_mut)
    {
        output_properties.remove("session_id");
    }
    ToolSpec::Function(spec)
}

impl CoreToolRuntime for ExecCommandHandler {
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }
}

fn emit_unified_exec_tty_metric(session_telemetry: &SessionTelemetry, tty: bool) {
    session_telemetry.counter(
        TOOL_CALL_UNIFIED_EXEC_METRIC,
        /*inc*/ 1,
        &[("tty", if tty { "true" } else { "false" })],
    );
}
