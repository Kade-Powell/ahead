use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::function_tool::FunctionCallError;
use crate::sandbox_tags::permission_profile_policy_tag;
use crate::sandbox_tags::permission_profile_sandbox_tag;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::unified_exec::ExecCommandArgs;
use crate::tools::router::tool_log_payload;
use crate::tools::tool_dispatch_trace::ToolDispatchTrace;
use crate::util::error_or_panic;
use codex_history::CodexHarnessMetadata;
use codex_history::ResponseItemEnvelope;
use codex_protocol::parse_command::ParsedCommand;
use codex_protocol::protocol::EventMsg;
use codex_shell_command::parse_command::parse_shell_script;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use futures::future::BoxFuture;
use indexmap::IndexMap;
use indexmap::map::Entry;

pub(crate) type ToolTelemetryTags = Vec<(&'static str, String)>;

pub use codex_tools::ToolExecutor;
pub use codex_tools::ToolExposure;

/// Typed runtime contract for locally executed tools.
///
/// Implementers provide shared tool execution plus optional metadata for
/// telemetry, tool search, and argument diffs.
pub(crate) trait CoreToolRuntime: ToolExecutor<ToolInvocation> {
    /// Returns a shared spec when both the spec and search metadata are immutable.
    fn immutable_spec(&self) -> Option<&Arc<ToolSpec>> {
        None
    }

    /// Returns lazily cached Code Mode definitions owned by this runtime.
    fn cached_code_mode_definitions(&self) -> Option<&[codex_code_mode::ToolDefinition]> {
        None
    }

    /// Returns a readiness wait for this exact tool before taking the execution gate.
    fn wait_until_ready<'a>(&'a self, _session: &'a Arc<Session>) -> Option<BoxFuture<'a, ()>> {
        None
    }

    /// Returns the owning server only for MCP-backed tool runtimes.
    fn mcp_server_name(&self) -> Option<&str> {
        None
    }

    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(
            payload,
            ToolPayload::Function { .. } | ToolPayload::ToolSearch { .. }
        )
    }

    fn telemetry_tags(&self, _invocation: &ToolInvocation) -> ToolTelemetryTags {
        Vec::new()
    }

    /// Creates an optional consumer for streamed tool argument diffs.
    fn create_diff_consumer(&self) -> Option<Box<dyn ToolArgumentDiffConsumer>> {
        None
    }
}

/// Consumes streamed argument diffs for a tool call and emits protocol events
/// derived from partial tool input.
pub(crate) trait ToolArgumentDiffConsumer: Send {
    /// Consume the next argument diff for a tool call.
    fn consume_diff(&mut self, turn: &TurnContext, call_id: String, diff: &str)
    -> Option<EventMsg>;

    /// Finish consuming argument diffs before the tool call completes.
    fn finish(&mut self) -> Result<Option<EventMsg>, FunctionCallError> {
        Ok(None)
    }
}

pub(crate) struct AnyToolResult {
    pub(crate) call_id: String,
    pub(crate) payload: ToolPayload,
    pub(crate) result: Box<dyn ToolOutput>,
}

impl AnyToolResult {
    pub(crate) fn into_response(self) -> ResponseItemEnvelope {
        let Self {
            call_id,
            payload,
            result,
            ..
        } = self;
        ResponseItemEnvelope {
            item: result.to_response_item(&call_id, &payload).into(),
            metadata: result
                .fallback_token_limit_override()
                .map(|limit| CodexHarnessMetadata {
                    fallback_token_limit_override: Some(limit),
                    ..Default::default()
                }),
        }
    }

    pub(crate) fn code_mode_result(self) -> serde_json::Value {
        let Self {
            payload, result, ..
        } = self;
        result.code_mode_result(&payload)
    }
}

/// A tool runtime together with its effective exposure for the current step.
pub(crate) struct RegisteredTool {
    pub(crate) runtime: Arc<dyn CoreToolRuntime>,
    pub(crate) exposure: ToolExposure,
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: IndexMap<ToolName, RegisteredTool>,
    first_collision: Option<ToolName>,
}

impl ToolRegistry {
    #[cfg(test)]
    pub(crate) fn from_tools(tools: impl IntoIterator<Item = Arc<dyn CoreToolRuntime>>) -> Self {
        let mut registry = Self::default();

        for runtime in tools {
            registry.register_trusted(runtime);
        }

        registry
    }

    pub(crate) fn add<T>(&mut self, handler: T)
    where
        T: CoreToolRuntime + 'static,
    {
        self.register_trusted(Arc::new(handler));
    }

    pub(crate) fn add_with_exposure<T>(&mut self, handler: T, exposure: ToolExposure)
    where
        T: CoreToolRuntime + 'static,
    {
        self.register_trusted_with_exposure(Arc::new(handler), exposure);
    }

    pub(crate) fn register_trusted(&mut self, runtime: Arc<dyn CoreToolRuntime>) {
        let exposure = runtime.exposure();
        self.register_trusted_with_exposure(runtime, exposure);
    }

    pub(crate) fn register_trusted_with_exposure(
        &mut self,
        runtime: Arc<dyn CoreToolRuntime>,
        exposure: ToolExposure,
    ) {
        let tool_name = runtime.tool_name().with_default_namespace();
        match self.tools.entry(tool_name) {
            Entry::Vacant(entry) => {
                entry.insert(RegisteredTool { runtime, exposure });
            }
            Entry::Occupied(entry) => {
                let tool_name = entry.key();
                error_or_panic(format!("tool {tool_name} already registered"));
            }
        }
    }

    pub(crate) fn prepend_trusted(&mut self, runtime: Arc<dyn CoreToolRuntime>) {
        let tool_name = runtime.tool_name().with_default_namespace();
        if self.tools.contains_key(&tool_name) {
            error_or_panic(format!("tool {tool_name} already registered"));
            return;
        }

        let exposure = runtime.exposure();
        self.tools
            .shift_insert(0, tool_name, RegisteredTool { runtime, exposure });
    }

    pub(crate) fn register_external(&mut self, runtime: Arc<dyn CoreToolRuntime>) -> bool {
        let exposure = runtime.exposure();
        self.register_external_with_exposure(runtime, exposure)
    }

    pub(crate) fn register_external_with_exposure(
        &mut self,
        runtime: Arc<dyn CoreToolRuntime>,
        exposure: ToolExposure,
    ) -> bool {
        let tool_name = runtime.tool_name().with_default_namespace();
        if tool_name.is_default_namespace()
            && matches!(tool_name.name.as_str(), "exec_command" | "shell_command")
        {
            tracing::warn!(tool_name = %tool_name, "skipping external tool with reserved name");
            if self.tools.contains_key(&tool_name) {
                self.record_collision(tool_name);
            }
            return false;
        }

        match self.tools.entry(tool_name) {
            Entry::Vacant(entry) => {
                entry.insert(RegisteredTool { runtime, exposure });
                true
            }
            Entry::Occupied(entry) => {
                tracing::warn!(
                    tool_name = %entry.key(),
                    "skipping duplicate external tool that is already registered"
                );
                self.first_collision
                    .get_or_insert_with(|| entry.key().clone());
                false
            }
        }
    }

    pub(crate) fn record_collision(&mut self, tool_name: ToolName) {
        self.first_collision.get_or_insert(tool_name);
    }

    pub(crate) fn first_collision(&self) -> Option<&ToolName> {
        self.first_collision.as_ref()
    }

    pub(crate) fn remove(&mut self, tool_name: &ToolName) -> Option<Arc<dyn CoreToolRuntime>> {
        self.tools
            .shift_remove(&tool_name.clone().with_default_namespace())
            .map(|tool| tool.runtime)
    }

    pub(crate) fn entries(&self) -> impl Iterator<Item = &RegisteredTool> {
        self.tools.values()
    }

    pub(crate) fn entries_mut(&mut self) -> impl Iterator<Item = &mut RegisteredTool> {
        self.tools.values_mut()
    }

    pub(crate) fn deferred_tool_namespaces(&self) -> BTreeMap<String, String> {
        let mut namespaces = BTreeMap::<String, String>::new();
        for (name, tool) in &self.tools {
            if !tool.exposure.is_deferred() || name.is_default_namespace() {
                continue;
            }
            let Some(namespace) = &name.namespace else {
                continue;
            };
            let existing_description = namespaces.entry(namespace.clone()).or_default();
            if !existing_description.trim().is_empty() {
                continue;
            }
            let owned_spec;
            let spec = if let Some(spec) = tool.runtime.immutable_spec() {
                spec.as_ref()
            } else {
                owned_spec = tool.runtime.spec();
                &owned_spec
            };
            let description = match spec {
                ToolSpec::Namespace(namespace) => namespace.description.as_str(),
                ToolSpec::Function(_)
                | ToolSpec::Freeform(_)
                | ToolSpec::ToolSearch { .. }
                | ToolSpec::WebSearch { .. } => "",
            };
            if !description.trim().is_empty() {
                *existing_description = description.to_string();
            }
        }
        namespaces
    }

    #[cfg(test)]
    pub(crate) fn empty_for_test() -> Self {
        Self::from_tools(std::iter::empty())
    }

    #[cfg(test)]
    pub(crate) fn with_handler_for_test<T>(handler: Arc<T>) -> Self
    where
        T: CoreToolRuntime + 'static,
    {
        Self::from_tools([handler as Arc<dyn CoreToolRuntime>])
    }

    pub(crate) fn tool(&self, name: &ToolName) -> Option<Arc<dyn CoreToolRuntime>> {
        self.tools
            .get(&name.clone().with_default_namespace())
            .map(|tool| Arc::clone(&tool.runtime))
    }

    #[cfg(test)]
    pub(crate) fn tool_names_for_test(&self) -> Vec<ToolName> {
        let mut names = self.tools.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    #[cfg(test)]
    pub(crate) fn tool_exposure(&self, name: &ToolName) -> Option<ToolExposure> {
        self.tools
            .get(&name.clone().with_default_namespace())
            .map(|tool| tool.exposure)
    }

    pub(crate) fn create_diff_consumer(
        &self,
        name: &ToolName,
    ) -> Option<Box<dyn ToolArgumentDiffConsumer>> {
        self.tool(name)?.create_diff_consumer()
    }

    pub(crate) fn supports_parallel_tool_calls(&self, name: &ToolName) -> Option<bool> {
        let tool = self.tools.get(&name.clone().with_default_namespace())?;
        Some(tool.exposure != ToolExposure::Hidden && tool.runtime.supports_parallel_tool_calls())
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "tool dispatch must keep active-turn accounting atomic"
    )]
    pub(crate) async fn dispatch_any_with_terminal_outcome(
        &self,
        invocation: ToolInvocation,
        terminal_outcome_reached: Option<Arc<AtomicBool>>,
    ) -> Result<AnyToolResult, FunctionCallError> {
        let tool_name = invocation.tool_name.clone();
        let call_id_owned = invocation.call_id.clone();
        let otel = invocation.step_context.turn.session_telemetry.clone();
        let permission_profile = invocation.step_context.turn.permission_profile();
        // TODO(anp): Reconcile these tags with TurnEnvironment::sandbox_context
        // instead of reporting the thread-wide backend for environment-scoped tools.
        let base_tool_result_tags = [
            (
                "sandbox",
                permission_profile_sandbox_tag(
                    &permission_profile,
                    invocation.step_context.turn.windows_sandbox_level,
                ),
            ),
            (
                "sandbox_policy",
                permission_profile_policy_tag(
                    &permission_profile,
                    #[allow(deprecated)]
                    invocation.step_context.turn.cwd.as_path(),
                ),
            ),
        ];

        {
            let mut active = invocation.session.active_turn.lock().await;
            if let Some(active_turn) = active.as_mut() {
                let mut turn_state = active_turn.turn_state.lock().await;
                turn_state.tool_calls = turn_state.tool_calls.saturating_add(1);
            }
        }

        let dispatch_trace = ToolDispatchTrace::start(&invocation);
        let tool = match self.tool(&tool_name) {
            Some(tool) => tool,
            None => {
                let message = unsupported_tool_call_message(&invocation.payload, &tool_name);
                let log_payload = tool_log_payload(&invocation.payload, &invocation.source);
                otel.tool_result_with_tags(
                    &tool_name,
                    &call_id_owned,
                    log_payload.as_ref(),
                    Duration::ZERO,
                    /*success*/ false,
                    &message,
                    &base_tool_result_tags,
                    /*extra_trace_fields*/ &[],
                );
                let err = FunctionCallError::RespondToModel(message);
                dispatch_trace.record_failed(&err);
                return Err(err);
            }
        };
        let telemetry_tags = tool.telemetry_tags(&invocation);
        let mut tool_result_tags =
            Vec::with_capacity(base_tool_result_tags.len() + telemetry_tags.len() + 1);
        let mut extra_trace_fields = Vec::new();
        tool_result_tags.extend_from_slice(&base_tool_result_tags);
        for (key, value) in &telemetry_tags {
            if matches!(*key, "mcp_server" | "mcp_server_origin") {
                extra_trace_fields.push((*key, value.as_str()));
            } else {
                tool_result_tags.push((*key, value.as_str()));
            }
        }
        if !tool.matches_kind(&invocation.payload) {
            let message = format!("tool {tool_name} invoked with incompatible payload");
            let log_payload = tool_log_payload(&invocation.payload, &invocation.source);
            otel.tool_result_with_tags(
                &tool_name,
                &call_id_owned,
                log_payload.as_ref(),
                Duration::ZERO,
                /*success*/ false,
                &message,
                &tool_result_tags,
                &extra_trace_fields,
            );
            let err = FunctionCallError::Fatal(message);
            dispatch_trace.record_failed(&err);
            return Err(err);
        }

        if let Some(command) = shell_script_for_invocation(&invocation) {
            let parsed = parse_shell_script(&command);
            let mut categories = parsed.iter().map(|command| match command {
                ParsedCommand::Read { .. } => "read",
                ParsedCommand::ListFiles { .. } => "list_files",
                ParsedCommand::Search { .. } => "search",
                ParsedCommand::Unknown { .. } => "unknown",
            });
            let category = match categories.next() {
                Some(first) if categories.all(|category| category == first) => first,
                Some(_) => "mixed",
                None => "unknown",
            };
            tool_result_tags.push(("command_category", category));
        }

        let log_payload = tool_log_payload(&invocation.payload, &invocation.source);

        let result = otel
            .log_tool_result_with_tags(
                &tool_name,
                &call_id_owned,
                log_payload.as_ref(),
                &tool_result_tags,
                &extra_trace_fields,
                || handle_any_tool(tool.as_ref(), invocation.clone()),
                |result| {
                    (
                        result.result.log_output(),
                        result.result.success_for_logging(),
                    )
                },
            )
            .await;
        mark_tool_outcome_reached(terminal_outcome_reached.as_deref());

        match result {
            Ok(result) => {
                dispatch_trace.record_completed(
                    &invocation,
                    &result.call_id,
                    &result.payload,
                    result.result.as_ref(),
                );
                Ok(result)
            }
            Err(err) => {
                dispatch_trace.record_failed(&err);
                Err(err)
            }
        }
    }
}

fn shell_script_for_invocation(invocation: &ToolInvocation) -> Option<String> {
    let ToolPayload::Function { arguments } = &invocation.payload else {
        return None;
    };
    if !invocation.tool_name.is_default_namespace() || invocation.tool_name.name != "exec_command" {
        return None;
    }
    serde_json::from_str::<ExecCommandArgs>(arguments)
        .ok()
        .map(|params| params.cmd)
}

fn mark_tool_outcome_reached(terminal_outcome_reached: Option<&AtomicBool>) {
    if let Some(reached) = terminal_outcome_reached {
        reached.store(true, Ordering::Release);
    }
}

async fn handle_any_tool(
    tool: &dyn CoreToolRuntime,
    invocation: ToolInvocation,
) -> Result<AnyToolResult, FunctionCallError> {
    let call_id = invocation.call_id.clone();
    let payload = invocation.payload.clone();
    let output = tool.handle(invocation.clone()).await?;
    Ok(AnyToolResult {
        call_id,
        payload,
        result: output,
    })
}

fn unsupported_tool_call_message(payload: &ToolPayload, tool_name: &ToolName) -> String {
    match payload {
        ToolPayload::Custom { .. } => format!("unsupported custom tool call: {tool_name}"),
        _ => format!("unsupported call: {tool_name}"),
    }
}
#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
