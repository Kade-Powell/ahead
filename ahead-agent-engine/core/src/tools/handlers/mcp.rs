use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Instant;

use crate::function_tool::FunctionCallError;
use crate::mcp_tool_call::handle_mcp_tool_call;
use crate::original_image_detail::can_request_original_image_detail;
use crate::session::session::Session;
use crate::tools::context::McpToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::flat_tool_name;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use crate::tools::registry::ToolTelemetryTags;
use codex_mcp::ToolInfo;
use codex_tools::ResponsesApiNamespace;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ToolName;
use codex_tools::ToolSearchInfo;
use codex_tools::ToolSearchSourceInfo;
use codex_tools::ToolSpec;
use codex_tools::mcp_tool_to_responses_api_tool;
use codex_utils_output_truncation::TruncationPolicy;
use codex_utils_string::take_bytes_at_char_boundary;
use futures::future::BoxFuture;
const MAX_MCP_NAMESPACE_DESCRIPTION_BYTES: usize = 512 * 1024;

pub struct McpHandler {
    tool_info: ToolInfo,
    spec: Arc<ToolSpec>,
    code_mode_tool_definitions: OnceLock<Vec<codex_code_mode::ToolDefinition>>,
}

impl McpHandler {
    pub fn new(tool_info: ToolInfo) -> Result<Self, serde_json::Error> {
        let spec = Arc::new(create_tool_spec(&tool_info)?);
        Ok(Self {
            tool_info,
            spec,
            code_mode_tool_definitions: OnceLock::new(),
        })
    }
}

impl ToolExecutor<ToolInvocation> for McpHandler {
    fn tool_name(&self) -> ToolName {
        self.tool_info.canonical_tool_name()
    }

    fn spec(&self) -> ToolSpec {
        self.spec.as_ref().clone()
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        // Correctly implemented MCP servers should tolerate parallel calls to
        // tools that advertise themselves as read-only.
        self.tool_info.supports_parallel_tool_calls
            || self
                .tool_info
                .tool
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.read_only_hint)
                .unwrap_or(false)
    }

    fn search_info(&self) -> Option<ToolSearchInfo> {
        let source_name = self.tool_info.server_name.trim();
        let source_info = (!source_name.is_empty()).then(|| ToolSearchSourceInfo {
            name: source_name.to_string(),
            description: self
                .tool_info
                .namespace_description
                .as_deref()
                .map(str::trim)
                .filter(|description| !description.is_empty())
                .map(str::to_string),
        });

        ToolSearchInfo::from_spec(
            build_mcp_search_text(&self.tool_info),
            self.spec(),
            source_info,
        )
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(self.handle_call(invocation))
    }
}

impl McpHandler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let prepared_mcp_call = invocation
            .session
            .prepare_mcp_call(
                &self.tool_info.server_name,
                self.tool_info.tool.name.as_ref(),
            )
            .await;
        let originating_item_id = invocation.originating_item_id().await;
        let ToolInvocation {
            session,
            step_context,
            call_id,
            tool_name,
            payload,
            cancellation_token,
            ..
        } = invocation;
        let turn = Arc::clone(&step_context.turn);

        let payload = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "mcp handler received unsupported payload".to_string(),
                ));
            }
        };

        // Capture presentation policy from the same config snapshot used for execution.
        let truncation_policy = prepared_mcp_call
            .as_ref()
            .and_then(codex_mcp::PreparedMcpCall::output_token_limit)
            .map(TruncationPolicy::Tokens)
            .unwrap_or(turn.model_info().truncation_policy.into());
        let started = Instant::now();
        let result = handle_mcp_tool_call(
            Arc::clone(&session),
            &step_context,
            call_id.clone(),
            originating_item_id,
            &self.tool_info,
            prepared_mcp_call,
            cancellation_token,
            tool_name,
            payload,
        )
        .await;

        Ok(boxed_tool_output(McpToolOutput {
            result: result.result,
            wall_time: started.elapsed(),
            original_image_detail_supported: can_request_original_image_detail(turn.model_info()),
            truncation_policy,
        }))
    }
}

impl CoreToolRuntime for McpHandler {
    fn immutable_spec(&self) -> Option<&Arc<ToolSpec>> {
        Some(&self.spec)
    }

    fn cached_code_mode_definitions(&self) -> Option<&[codex_code_mode::ToolDefinition]> {
        Some(
            self.code_mode_tool_definitions
                .get_or_init(|| {
                    let mut definitions = codex_tools::collect_code_mode_tool_definitions(
                        std::iter::once(self.spec.as_ref()),
                    );
                    for definition in &mut definitions {
                        definition.input_schema = None;
                        definition.output_schema = None;
                    }
                    definitions
                })
                .as_slice(),
        )
    }

    fn wait_until_ready<'a>(&'a self, session: &'a Arc<Session>) -> Option<BoxFuture<'a, ()>> {
        Some(Box::pin(async move {
            session
                .wait_for_mcp_server(&self.tool_info.server_name)
                .await;
        }))
    }

    fn mcp_server_name(&self) -> Option<&str> {
        Some(&self.tool_info.server_name)
    }

    fn telemetry_tags(&self, _invocation: &ToolInvocation) -> ToolTelemetryTags {
        let mut tags = vec![("mcp_server", self.tool_info.server_name.clone())];
        if let Some(origin) = &self.tool_info.server_origin {
            tags.push(("mcp_server_origin", origin.clone()));
        }
        tags
    }
}

fn create_tool_spec(tool_info: &ToolInfo) -> Result<ToolSpec, serde_json::Error> {
    let tool_name = tool_info.canonical_tool_name();
    let tool = mcp_tool_to_responses_api_tool(&tool_name, &tool_info.tool)?;
    let description = tool_info
        .namespace_description
        .as_deref()
        .map(str::trim)
        .filter(|description| !description.is_empty())
        .map(str::to_string)
        .unwrap_or_default();

    Ok(ToolSpec::Namespace(ResponsesApiNamespace {
        name: tool_info.callable_namespace.clone(),
        description: take_bytes_at_char_boundary(&description, MAX_MCP_NAMESPACE_DESCRIPTION_BYTES)
            .to_string(),
        tools: vec![ResponsesApiNamespaceTool::Function(tool)],
    }))
}

fn build_mcp_search_text(info: &ToolInfo) -> String {
    let tool_name = info.canonical_tool_name();
    let mut schema_properties = info
        .tool
        .input_schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .map(|map| map.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    schema_properties.sort();
    let mut parts = vec![
        flat_tool_name(&tool_name).into_owned(),
        info.callable_name.clone(),
        info.tool.name.to_string(),
        info.server_name.clone(),
    ];
    if let Some(title) = info.tool.title.as_deref().map(str::trim)
        && !title.is_empty()
    {
        parts.push(title.to_string());
    }
    if let Some(description) = info.tool.description.as_deref().map(str::trim)
        && !description.is_empty()
    {
        parts.push(description.to_string());
    }
    if let Some(namespace_description) = info.namespace_description.as_deref().map(str::trim)
        && !namespace_description.is_empty()
    {
        parts.push(namespace_description.to_string());
    }
    parts.extend(schema_properties);
    parts.join(" ")
}

#[cfg(test)]
#[path = "mcp_search_tests.rs"]
mod search_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_code_mode_definitions_are_cached_lazily() {
        let handler = McpHandler::new(tool_info("filesystem", "mcp__filesystem", "read_file"))
            .expect("MCP tool spec should build");

        assert!(handler.code_mode_tool_definitions.get().is_none());
        assert!(Arc::ptr_eq(
            handler
                .immutable_spec()
                .expect("MCP spec should be immutable"),
            &handler.spec,
        ));

        let first = handler
            .cached_code_mode_definitions()
            .expect("MCP definitions should be cached");
        assert_eq!(first.len(), 1);
        assert!(first[0].input_schema.is_none());
        assert!(first[0].output_schema.is_none());

        let second = handler
            .cached_code_mode_definitions()
            .expect("MCP definitions should be cached");
        assert!(std::ptr::eq(first, second));
    }

    #[test]
    fn mcp_read_only_hint_supports_parallel_calls_without_server_opt_in() {
        let mut read_only_info = tool_info("foo", "mcp__foo__", "read");
        read_only_info.tool.annotations = Some(rmcp::model::ToolAnnotations::new().read_only(true));

        assert!(
            McpHandler::new(read_only_info)
                .expect("MCP tool spec should build")
                .supports_parallel_tool_calls()
        );
    }

    #[test]
    fn mcp_parallel_calls_require_read_only_hint_or_server_opt_in() {
        let missing_hint_info = tool_info("foo", "mcp__foo__", "unannotated");
        assert!(
            !McpHandler::new(missing_hint_info)
                .expect("MCP tool spec should build")
                .supports_parallel_tool_calls()
        );

        let mut writable_info = tool_info("foo", "mcp__foo__", "write");
        writable_info.tool.annotations = Some(rmcp::model::ToolAnnotations::new().read_only(false));
        assert!(
            !McpHandler::new(writable_info)
                .expect("MCP tool spec should build")
                .supports_parallel_tool_calls()
        );

        let mut server_opt_in_info = tool_info("foo", "mcp__foo__", "server_opt_in");
        server_opt_in_info.supports_parallel_tool_calls = true;
        assert!(
            McpHandler::new(server_opt_in_info)
                .expect("MCP tool spec should build")
                .supports_parallel_tool_calls()
        );
    }

    fn tool_info(server_name: &str, callable_namespace: &str, tool_name: &str) -> ToolInfo {
        ToolInfo {
            server_name: server_name.to_string(),
            supports_parallel_tool_calls: false,
            server_origin: None,
            callable_name: tool_name.to_string(),
            callable_namespace: callable_namespace.to_string(),
            namespace_description: None,
            tool: rmcp::model::Tool::new_with_raw(
                tool_name.to_string(),
                None,
                Arc::new(rmcp::model::object(serde_json::json!({
                    "type": "object",
                }))),
            ),
        }
    }
}
