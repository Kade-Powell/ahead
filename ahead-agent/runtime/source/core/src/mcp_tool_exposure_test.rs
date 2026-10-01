use std::collections::HashMap;
use std::sync::Arc;

use codex_mcp::ToolInfo;
use codex_tools::ToolExposure;
use codex_tools::ToolName;
use pretty_assertions::assert_eq;
use rmcp::model::JsonObject;
use rmcp::model::MetaObject;
use rmcp::model::Tool;

use super::*;

fn make_mcp_tool(
    server_name: &str,
    tool_name: &str,
    callable_namespace: &str,
    callable_name: &str,
    connector_id: Option<&str>,
    connector_name: Option<&str>,
) -> ToolInfo {
    ToolInfo {
        server_name: server_name.to_string(),
        supports_parallel_tool_calls: false,
        server_origin: None,
        callable_name: callable_name.to_string(),
        callable_namespace: callable_namespace.to_string(),
        namespace_description: None,
        tool: Tool::new(
            tool_name.to_string(),
            format!("Test tool: {tool_name}"),
            Arc::new(JsonObject::default()),
        ),
        openai_file_input_optional_fields: Default::default(),
        connector_id: connector_id.map(str::to_string),
        connector_name: connector_name.map(str::to_string),
    }
}

fn numbered_mcp_tools(count: usize) -> Vec<ToolInfo> {
    (0..count)
        .map(|index| {
            let tool_name = format!("tool_{index}");
            make_mcp_tool(
                "rmcp",
                &tool_name,
                "mcp__rmcp",
                &tool_name,
                /*connector_id*/ None,
                /*connector_name*/ None,
            )
        })
        .collect()
}

fn expected_runtimes(
    tools: &[ToolInfo],
    exposure: ToolExposure,
) -> HashMap<ToolName, ToolExposure> {
    tools
        .iter()
        .map(|tool| (tool.canonical_tool_name(), exposure))
        .collect()
}

fn runtimes_by_name(
    tools: &[ToolInfo],
    search_tool_enabled: bool,
) -> HashMap<ToolName, ToolExposure> {
    let mut handlers = HashMap::new();
    let mut registry = ToolRegistry::default();
    append_mcp_tools(tools, search_tool_enabled, &mut handlers, &mut registry);
    registry
        .entries()
        .map(|tool| (tool.runtime.tool_name(), tool.exposure))
        .collect()
}

fn with_visibility(mut tool: ToolInfo, visibility: &[&str]) -> ToolInfo {
    tool.tool.meta = Some(MetaObject(
        serde_json::json!({ "ui": { "visibility": visibility } })
            .as_object()
            .expect("metadata object")
            .clone(),
    ));
    tool
}

#[test]
fn directly_exposes_effective_tool_sets_when_search_is_unavailable() {
    let mcp_tools = numbered_mcp_tools(/*count*/ 2);

    let runtimes = runtimes_by_name(&mcp_tools, /*search_tool_enabled*/ false);

    assert_eq!(
        runtimes,
        expected_runtimes(&mcp_tools, ToolExposure::Direct)
    );
}

#[test]
fn excludes_tools_hidden_from_model_exposure() {
    let visible_tool = make_mcp_tool(
        "rmcp",
        "visible_tool",
        "mcp__rmcp",
        "visible_tool",
        /*connector_id*/ None,
        /*connector_name*/ None,
    );
    let hidden_tool = with_visibility(
        make_mcp_tool(
            "rmcp",
            "hidden_tool",
            "mcp__rmcp",
            "hidden_tool",
            /*connector_id*/ None,
            /*connector_name*/ None,
        ),
        &["app"],
    );
    let empty_visibility_tool = with_visibility(
        make_mcp_tool(
            "rmcp",
            "empty_visibility_tool",
            "mcp__rmcp",
            "empty_visibility_tool",
            /*connector_id*/ None,
            /*connector_name*/ None,
        ),
        &[],
    );
    let visible_calendar_tool = with_visibility(
        make_mcp_tool(
            "calendar",
            "calendar_read",
            "mcp__calendar",
            "read",
            Some("calendar"),
            Some("Calendar"),
        ),
        &["app", "model"],
    );
    let hidden_calendar_tool = with_visibility(
        make_mcp_tool(
            "calendar",
            "calendar_open",
            "mcp__calendar",
            "open",
            Some("calendar"),
            Some("Calendar"),
        ),
        &["app"],
    );
    let mcp_tools = vec![
        visible_tool.clone(),
        hidden_tool,
        empty_visibility_tool,
        visible_calendar_tool.clone(),
        hidden_calendar_tool,
    ];
    let runtimes = runtimes_by_name(&mcp_tools, /*search_tool_enabled*/ false);

    assert_eq!(
        runtimes,
        expected_runtimes(&[visible_tool, visible_calendar_tool], ToolExposure::Direct)
    );
}

#[test]
fn defers_effective_tool_sets_when_search_is_available() {
    let mcp_tools = numbered_mcp_tools(/*count*/ 2);

    let runtimes = runtimes_by_name(&mcp_tools, /*search_tool_enabled*/ true);

    assert_eq!(
        runtimes,
        expected_runtimes(&mcp_tools, ToolExposure::Deferred)
    );
}
