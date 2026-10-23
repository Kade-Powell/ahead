use codex_protocol::openai_models::ApplyPatchToolType;
use codex_tools::AdditionalProperties;
use codex_tools::FreeformTool;
use codex_tools::FreeformToolFormat;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use std::collections::BTreeMap;

const APPLY_PATCH_LARK_GRAMMAR: &str = include_str!("../../../assets/tools/apply_patch.lark");

/// Returns a custom tool that can be used to edit files. Well-suited for GPT-5 models
/// https://platform.openai.com/docs/guides/function-calling#custom-tools
pub fn create_apply_patch_freeform_tool() -> ToolSpec {
    ToolSpec::Freeform(FreeformTool {
        name: "apply_patch".to_string(),
        description: "The `apply_patch` tool can be used to edit files. This is a FREEFORM tool, so do not wrap the patch in JSON.".to_string(),
        defer_loading: None,
        format: FreeformToolFormat {
            r#type: "grammar".to_string(),
            syntax: "lark".to_string(),
            definition: APPLY_PATCH_LARK_GRAMMAR.to_string(),
        },
    })
}

pub fn create_apply_patch_function_tool() -> ToolSpec {
    ToolSpec::Function(ResponsesApiTool {
        name: "apply_patch".to_string(),
        description: "Apply a patch to files in the current workspace. Provide the complete patch text in the `patch` field.".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            BTreeMap::from([(
                "patch".to_string(),
                JsonSchema::string(Some("Complete apply_patch text, including Begin Patch and End Patch markers.".to_string())),
            )]),
            Some(vec!["patch".to_string()]),
            Some(AdditionalProperties::Boolean(false)),
        ),
        output_schema: None,
    })
}

pub fn create_apply_patch_tool(tool_type: &ApplyPatchToolType) -> ToolSpec {
    match tool_type {
        ApplyPatchToolType::Freeform => create_apply_patch_freeform_tool(),
        ApplyPatchToolType::Function => create_apply_patch_function_tool(),
    }
}

#[cfg(test)]
#[path = "apply_patch_spec_tests.rs"]
mod tests;
