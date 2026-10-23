/// Identifies privileged MCP approvals. Changing this key or adding another
/// privileged discriminator requires updating the MCP elicitation router's
/// form-forwarding safeguards.
pub const APPROVAL_KIND_KEY: &str = "codex_approval_kind";
pub const APPROVAL_KIND_MCP_TOOL_CALL: &str = "mcp_tool_call";
pub const APPROVAL_KIND_TOOL_SUGGESTION: &str = "tool_suggestion";
pub const REQUEST_TYPE_KEY: &str = "codex_request_type";
pub const REQUEST_TYPE_APPROVAL_REQUEST: &str = "approval_request";
pub const PERSIST_KEY: &str = "persist";
pub const PERSIST_SESSION: &str = "session";
pub const PERSIST_ALWAYS: &str = "always";
pub const TOOL_NAME_KEY: &str = "tool_name";
pub const TOOL_TITLE_KEY: &str = "tool_title";
pub const TOOL_DESCRIPTION_KEY: &str = "tool_description";
pub const TOOL_PARAMS_KEY: &str = "tool_params";
pub const TOOL_PARAMS_DISPLAY_KEY: &str = "tool_params_display";
