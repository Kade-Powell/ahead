use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use codex_config::AppToolApproval;
use codex_mcp::configured_mcp_servers;
use codex_protocol::items::McpToolCallError;
use codex_protocol::items::McpToolCallItem;
use codex_protocol::items::McpToolCallStatus;
use codex_protocol::items::TurnItem;
use codex_protocol::mcp::CallToolResult;
use codex_protocol::models::function_call_output_content_items_to_text;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::protocol::TruncationPolicy;
use codex_tools::ToolName;
use codex_utils_output_truncation::truncate_text;
use rmcp::model::ListResourceTemplatesResult;
use rmcp::model::ListResourcesResult;
use rmcp::model::PaginatedRequestParams;
use rmcp::model::ReadResourceResult;
use rmcp::model::Resource;
use rmcp::model::ResourceTemplate;
use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::approval::ApprovalReviewContext;
use crate::function_tool::FunctionCallError;
use crate::session::session::Session;
use crate::session::step_context::StepContext;
use crate::session::turn_context::TurnContext;
use crate::tools::ApprovalContext;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolOutput;
use crate::tools::context::boxed_tool_output;
use crate::tools::hook_names::HookToolName;
use crate::tools::sandboxing::ApprovalAction;
use crate::tools::sandboxing::ToolError;
use codex_protocol::protocol::McpInvocation;

mod list_mcp_resource_templates;
mod list_mcp_resources;
mod read_mcp_resource;

pub use list_mcp_resource_templates::ListMcpResourceTemplatesHandler;
pub use list_mcp_resources::ListMcpResourcesHandler;
pub use read_mcp_resource::ReadMcpResourceHandler;

#[derive(Debug, Deserialize, Default, PartialEq, Eq)]
struct ListResourceArgs {
    #[serde(default)]
    server: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
}

impl ListResourceArgs {
    fn normalized(self) -> Self {
        Self {
            server: normalize_optional_string(self.server),
            cursor: normalize_optional_string(self.cursor),
        }
    }

    fn target(
        &self,
    ) -> Result<Option<(String, Option<PaginatedRequestParams>)>, FunctionCallError> {
        match &self.server {
            Some(server) => {
                let params = self
                    .cursor
                    .clone()
                    .map(|cursor| PaginatedRequestParams::default().with_cursor(Some(cursor)));
                Ok(Some((server.clone(), params)))
            }
            None if self.cursor.is_some() => Err(FunctionCallError::RespondToModel(
                "cursor can only be used when a server is specified".to_string(),
            )),
            None => Ok(None),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ReadResourceArgs {
    server: String,
    uri: String,
}

#[derive(Debug, Serialize)]
struct ResourceWithServer<T> {
    server: String,
    #[serde(flatten)]
    resource: T,
}

impl<T> ResourceWithServer<T> {
    fn new(server: String, resource: T) -> Self {
        Self { server, resource }
    }

    fn from_server(server: &str, resources: Vec<T>) -> Vec<Self> {
        resources
            .into_iter()
            .map(|resource| Self::new(server.to_string(), resource))
            .collect()
    }

    fn from_all_servers(resources_by_server: HashMap<String, Vec<T>>) -> Vec<Self> {
        let mut entries: Vec<_> = resources_by_server.into_iter().collect();
        entries.sort_by(|(left, _), (right, _)| left.cmp(right));
        entries
            .into_iter()
            .flat_map(|(server, resources)| Self::from_server(&server, resources))
            .collect()
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListResourcesPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    server: Option<String>,
    resources: Vec<ResourceWithServer<Resource>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

impl ListResourcesPayload {
    fn from_single_server(server: String, result: ListResourcesResult) -> Self {
        Self {
            resources: ResourceWithServer::from_server(&server, result.resources),
            server: Some(server),
            next_cursor: result.next_cursor,
        }
    }

    fn from_all_servers(resources_by_server: HashMap<String, Vec<Resource>>) -> Self {
        Self {
            server: None,
            resources: ResourceWithServer::from_all_servers(resources_by_server),
            next_cursor: None,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListResourceTemplatesPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    server: Option<String>,
    resource_templates: Vec<ResourceWithServer<ResourceTemplate>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

impl ListResourceTemplatesPayload {
    fn from_single_server(server: String, result: ListResourceTemplatesResult) -> Self {
        Self {
            resource_templates: ResourceWithServer::from_server(&server, result.resource_templates),
            server: Some(server),
            next_cursor: result.next_cursor,
        }
    }

    fn from_all_servers(templates_by_server: HashMap<String, Vec<ResourceTemplate>>) -> Self {
        Self {
            server: None,
            resource_templates: ResourceWithServer::from_all_servers(templates_by_server),
            next_cursor: None,
        }
    }
}

#[derive(Debug, Serialize)]
struct ReadResourcePayload {
    server: String,
    uri: String,
    #[serde(flatten)]
    result: ReadResourceResult,
}

fn call_tool_result_from_content(content: &str, success: Option<bool>) -> CallToolResult {
    CallToolResult {
        content: vec![serde_json::json!({"type": "text", "text": content})],
        structured_content: None,
        is_error: success.map(|value| !value),
        meta: None,
    }
}

fn resource_servers(
    step_context: &StepContext,
    requested_server: Option<&str>,
) -> Result<Vec<String>, FunctionCallError> {
    let configured = configured_mcp_servers(step_context.mcp.config());
    if let Some(server) = requested_server {
        if configured.get(server).is_some_and(|config| config.enabled) {
            return Ok(vec![server.to_string()]);
        }
        return Err(FunctionCallError::RespondToModel(format!(
            "MCP server `{server}` is not configured for this turn"
        )));
    }
    let mut servers = configured
        .into_iter()
        .filter_map(|(name, config)| config.enabled.then_some(name))
        .collect::<Vec<_>>();
    servers.sort();
    Ok(servers)
}

async fn approve_resource_operation(
    session: &Arc<Session>,
    step_context: &Arc<StepContext>,
    call_id: &str,
    invocation: &McpInvocation,
    server: &str,
) -> Result<(), FunctionCallError> {
    let config = step_context
        .mcp
        .config()
        .mcp_server_catalog
        .server(server)
        .ok_or_else(|| {
            FunctionCallError::RespondToModel(format!(
                "MCP server `{server}` is not configured for this turn"
            ))
        })?;
    let strict_auto_review = session
        .active_turn_context_and_strict_auto_review()
        .await
        .is_some_and(|(_, _, strict)| strict);
    if !strict_auto_review
        && config.config().default_tools_approval_mode == Some(AppToolApproval::Approve)
    {
        return Ok(());
    }
    let approval_id = format!("{call_id}:{server}");
    let action = ApprovalAction::McpToolCall {
        id: approval_id.clone(),
        server: server.to_string(),
        tool_name: invocation.tool.clone(),
        arguments: invocation.arguments.clone(),
        tool_title: None,
        tool_description: None,
        annotations: None,
        hook_tool_name: HookToolName::new(format!("mcp__{server}__{}", invocation.tool)),
        approval_policy: step_context.mcp.config().approval_policy.value(),
        approval_mode: AppToolApproval::Prompt,
        allow_session_remember: false,
    };
    let approval_context = ApprovalContext {
        review_context: ApprovalReviewContext::from(step_context),
        call_id: approval_id,
        tool_name: ToolName::plain(invocation.tool.clone()),
        strict_auto_review,
        approval_reason: None,
        retry_reason: None,
        network_approval_context: None,
    };
    match session.request_approval(action, approval_context).await {
        Ok(
            ReviewDecision::Approved
            | ReviewDecision::ApprovedForSession
            | ReviewDecision::ApprovedMcpPolicyAmendment
            | ReviewDecision::ApprovedExecpolicyAmendment { .. }
            | ReviewDecision::NetworkPolicyAmendment { .. },
        ) => Ok(()),
        Ok(ReviewDecision::Denied { rejection }) => {
            Err(FunctionCallError::RespondToModel(rejection))
        }
        Ok(ReviewDecision::TimedOut) => Err(FunctionCallError::RespondToModel(
            crate::tools::APPROVAL_TIMEOUT_MESSAGE.to_string(),
        )),
        Ok(ReviewDecision::Abort) => Err(FunctionCallError::RespondToModel(
            "user cancelled MCP resource operation".to_string(),
        )),
        Err(ToolError::Rejected(rejection)) => Err(FunctionCallError::RespondToModel(rejection)),
        Err(ToolError::Codex(_)) => Err(FunctionCallError::RespondToModel(
            "user cancelled MCP resource operation".to_string(),
        )),
    }
}

async fn emit_tool_call_begin(
    session: &Arc<Session>,
    turn: &TurnContext,
    call_id: &str,
    invocation: McpInvocation,
) {
    let McpInvocation {
        server,
        tool,
        arguments,
    } = invocation;
    let item = TurnItem::McpToolCall(McpToolCallItem {
        id: call_id.to_string(),
        server,
        tool,
        arguments: arguments.unwrap_or(Value::Null),
        mcp_app_resource_uri: None,
        read_only_hint: None,
        status: McpToolCallStatus::InProgress,
        result: None,
        error: None,
        duration: None,
    });
    session.emit_turn_item_started(turn, &item).await;
}

async fn emit_tool_call_end(
    session: &Arc<Session>,
    turn: &TurnContext,
    call_id: &str,
    invocation: McpInvocation,
    duration: Duration,
    result: Result<CallToolResult, String>,
) {
    let (status, result, error) = match result {
        Ok(result) if result.is_error.unwrap_or(false) => {
            (McpToolCallStatus::Failed, Some(result), None)
        }
        Ok(result) => (McpToolCallStatus::Completed, Some(result), None),
        Err(message) => (
            McpToolCallStatus::Failed,
            None,
            Some(McpToolCallError { message }),
        ),
    };
    let McpInvocation {
        server,
        tool,
        arguments,
    } = invocation;
    let item = TurnItem::McpToolCall(McpToolCallItem {
        id: call_id.to_string(),
        server,
        tool,
        arguments: arguments.unwrap_or(Value::Null),
        mcp_app_resource_uri: None,
        read_only_hint: None,
        status,
        result,
        error,
        duration: Some(duration),
    });
    session.emit_turn_item_completed(turn, item).await;
}

async fn run_resource_operation<T>(
    session: &Arc<Session>,
    step_context: &Arc<StepContext>,
    call_id: &str,
    invocation: McpInvocation,
    servers: &[String],
    operation: impl Future<Output = Result<T, FunctionCallError>>,
) -> Result<Box<dyn ToolOutput>, FunctionCallError>
where
    T: Serialize,
{
    let turn = step_context.turn.as_ref();
    emit_tool_call_begin(session, turn, call_id, invocation.clone()).await;
    let start = Instant::now();
    let result = async {
        for server in servers {
            approve_resource_operation(session, step_context, call_id, &invocation, server).await?;
        }
        operation.await.and_then(|payload| {
            serialize_function_output(payload, turn.model_info().truncation_policy.into())
        })
    }
    .await;

    match result {
        Ok(output) => {
            let content =
                function_call_output_content_items_to_text(&output.body).unwrap_or_default();
            emit_tool_call_end(
                session,
                turn,
                call_id,
                invocation,
                start.elapsed(),
                Ok(call_tool_result_from_content(&content, output.success)),
            )
            .await;
            Ok(boxed_tool_output(output))
        }
        Err(error) => {
            emit_tool_call_end(
                session,
                turn,
                call_id,
                invocation,
                start.elapsed(),
                Err(error.to_string()),
            )
            .await;
            Err(error)
        }
    }
}

fn normalize_optional_string(input: Option<String>) -> Option<String> {
    input.and_then(|value| {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

fn normalize_required_string(field: &str, value: String) -> Result<String, FunctionCallError> {
    match normalize_optional_string(Some(value)) {
        Some(normalized) => Ok(normalized),
        None => Err(FunctionCallError::RespondToModel(format!(
            "{field} must be provided"
        ))),
    }
}

fn serialize_function_output<T>(
    payload: T,
    truncation_policy: TruncationPolicy,
) -> Result<FunctionToolOutput, FunctionCallError>
where
    T: Serialize,
{
    let content = serde_json::to_string(&payload).map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "failed to serialize MCP resource response: {err}"
        ))
    })?;
    // Match regular MCP tool outputs by bounding the copy persisted to the
    // rollout and injected into model context.
    let content = truncate_text(&content, truncation_policy * 1.2);

    Ok(FunctionToolOutput::from_text(content, Some(true)))
}

fn parse_arguments(raw_args: &str) -> Result<Option<Value>, FunctionCallError> {
    if raw_args.trim().is_empty() {
        Ok(None)
    } else {
        let value: Value = serde_json::from_str(raw_args).map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to parse function arguments: {err}"))
        })?;
        if value.is_null() {
            Ok(None)
        } else {
            Ok(Some(value))
        }
    }
}

fn parse_args<T>(arguments: Option<Value>) -> Result<T, FunctionCallError>
where
    T: DeserializeOwned,
{
    match arguments {
        Some(value) => serde_json::from_value(value).map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to parse function arguments: {err}"))
        }),
        None => Err(FunctionCallError::RespondToModel(
            "failed to parse function arguments: expected value".to_string(),
        )),
    }
}

fn parse_args_with_default<T>(arguments: Option<Value>) -> Result<T, FunctionCallError>
where
    T: DeserializeOwned + Default,
{
    match arguments {
        Some(value) => parse_args(Some(value)),
        None => Ok(T::default()),
    }
}

#[cfg(test)]
#[path = "mcp_resource_tests.rs"]
mod tests;
