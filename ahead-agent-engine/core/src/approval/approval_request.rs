use std::path::Path;

use codex_protocol::models::AdditionalPermissionProfile;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::LegacyAppPathString;
use codex_utils_path_uri::PathUri;
use serde::Serialize;
use serde_json::Value;

use super::AHEAD_MAX_APPROVAL_ACTION_STRING_TOKENS;
use super::prompt::truncate_text_to_token_limit;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ApprovalRequest {
    ExecCommand {
        id: String,
        command: Vec<String>,
        cwd: AbsolutePathBuf,
        sandbox_permissions: crate::sandboxing::SandboxPermissions,
        additional_permissions: Option<AdditionalPermissionProfile>,
        justification: Option<String>,
        tty: bool,
    },
    WriteStdin {
        id: String,
        approval_id: String,
        environment_id: String,
        process_id: i32,
        input: String,
        cwd: PathUri,
        tty: bool,
        sandbox_permissions: crate::sandboxing::SandboxPermissions,
        additional_permissions: Option<AdditionalPermissionProfile>,
    },
    ApplyPatch {
        id: String,
        cwd: AbsolutePathBuf,
        files: Vec<AbsolutePathBuf>,
        patch: String,
    },
    McpToolCall {
        id: String,
        server: String,
        tool_name: String,
        arguments: Option<Value>,
        tool_title: Option<String>,
        tool_description: Option<String>,
        annotations: Option<McpToolAnnotations>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct McpToolAnnotations {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) destructive_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) open_world_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) read_only_hint: Option<bool>,
}

#[derive(Serialize)]
struct CommandApprovalAction<'a> {
    tool: &'a str,
    command: &'a [String],
    cwd: &'a Path,
    sandbox_permissions: crate::sandboxing::SandboxPermissions,
    #[serde(skip_serializing_if = "Option::is_none")]
    additional_permissions: Option<&'a AdditionalPermissionProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    justification: Option<&'a String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tty: Option<bool>,
}

#[derive(Serialize)]
struct WriteStdinApprovalAction<'a> {
    tool: &'static str,
    environment_id: &'a str,
    session_id: i32,
    chars: &'a str,
    cwd: LegacyAppPathString,
    sandbox_permissions: crate::sandboxing::SandboxPermissions,
    #[serde(skip_serializing_if = "Option::is_none")]
    additional_permissions: Option<&'a AdditionalPermissionProfile>,
    tty: bool,
}

#[derive(Serialize)]
struct McpToolCallApprovalAction<'a> {
    tool: &'static str,
    server: &'a str,
    tool_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<&'a Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_title: Option<&'a String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_description: Option<&'a String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    annotations: Option<&'a McpToolAnnotations>,
}

fn serialize_approval_action(value: impl Serialize) -> serde_json::Result<Value> {
    serde_json::to_value(value)
}

fn serialize_command_approval_action(
    tool: &'static str,
    command: &[String],
    cwd: &Path,
    sandbox_permissions: crate::sandboxing::SandboxPermissions,
    additional_permissions: Option<&AdditionalPermissionProfile>,
    justification: Option<&String>,
    tty: Option<bool>,
) -> serde_json::Result<Value> {
    serialize_approval_action(CommandApprovalAction {
        tool,
        command,
        cwd,
        sandbox_permissions,
        additional_permissions,
        justification,
        tty,
    })
}

fn truncate_approval_action_value(value: Value) -> (Value, bool) {
    match value {
        Value::String(text) => {
            let (text, truncated) =
                truncate_text_to_token_limit(&text, AHEAD_MAX_APPROVAL_ACTION_STRING_TOKENS);
            (Value::String(text), truncated)
        }
        Value::Array(values) => {
            let mut truncated = false;
            let values = values
                .into_iter()
                .map(|value| {
                    let (value, value_truncated) = truncate_approval_action_value(value);
                    truncated |= value_truncated;
                    value
                })
                .collect::<Vec<_>>();
            (Value::Array(values), truncated)
        }
        Value::Object(values) => {
            let mut entries = values.into_iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            let mut truncated = false;
            let values = entries
                .into_iter()
                .map(|(key, value)| {
                    let (value, value_truncated) = truncate_approval_action_value(value);
                    truncated |= value_truncated;
                    (key, value)
                })
                .collect();
            (Value::Object(values), truncated)
        }
        other => (other, false),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FormattedApprovalAction {
    pub(crate) text: String,
    pub(crate) truncated: bool,
}

pub(crate) fn approval_request_to_json(action: &ApprovalRequest) -> serde_json::Result<Value> {
    match action {
        ApprovalRequest::ExecCommand {
            id: _,
            command,
            cwd,
            sandbox_permissions,
            additional_permissions,
            justification,
            tty,
        } => serialize_command_approval_action(
            "exec_command",
            command,
            cwd,
            *sandbox_permissions,
            additional_permissions.as_ref(),
            justification.as_ref(),
            Some(*tty),
        ),
        ApprovalRequest::WriteStdin {
            environment_id,
            process_id,
            input,
            cwd,
            tty,
            sandbox_permissions,
            additional_permissions,
            ..
        } => serialize_approval_action(WriteStdinApprovalAction {
            tool: "write_stdin",
            environment_id,
            session_id: *process_id,
            chars: input,
            cwd: cwd.clone().into(),
            sandbox_permissions: *sandbox_permissions,
            additional_permissions: additional_permissions.as_ref(),
            tty: *tty,
        }),
        ApprovalRequest::ApplyPatch {
            id: _,
            cwd,
            files,
            patch,
        } => Ok(serde_json::json!({
            "tool": "apply_patch",
            "cwd": cwd,
            "files": files,
            "patch": patch,
        })),
        ApprovalRequest::McpToolCall {
            id: _,
            server,
            tool_name,
            arguments,
            tool_title,
            tool_description,
            annotations,
        } => serialize_approval_action(McpToolCallApprovalAction {
            tool: "mcp_tool_call",
            server,
            tool_name,
            arguments: arguments.as_ref(),
            tool_title: tool_title.as_ref(),
            tool_description: tool_description.as_ref(),
            annotations: annotations.as_ref(),
        }),
    }
}

pub(crate) fn format_approval_action_pretty(
    action: &ApprovalRequest,
) -> serde_json::Result<FormattedApprovalAction> {
    let value = approval_request_to_json(action)?;
    let (value, truncated) = truncate_approval_action_value(value);
    Ok(FormattedApprovalAction {
        text: serde_json::to_string_pretty(&value)?,
        truncated,
    })
}
