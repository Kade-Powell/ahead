use super::*;
use pretty_assertions::assert_eq;

#[test]
fn create_apply_patch_freeform_tool_matches_expected_spec() {
    assert_eq!(
        create_apply_patch_freeform_tool(),
        ToolSpec::Freeform(FreeformTool {
            name: "apply_patch".to_string(),
            description:
                "The `apply_patch` tool can be used to edit files. This is a FREEFORM tool, so do not wrap the patch in JSON."
                    .to_string(),
            defer_loading: None,
            format: FreeformToolFormat {
                r#type: "grammar".to_string(),
                syntax: "lark".to_string(),
                definition: APPLY_PATCH_LARK_GRAMMAR.to_string(),
            },
        })
    );
}

#[test]
fn create_apply_patch_function_tool_uses_a_patch_argument_schema() {
    let spec = create_apply_patch_function_tool();
    let value = serde_json::to_value(spec).expect("function tool should serialize");

    assert_eq!(value["type"], "function");
    assert_eq!(value["name"], "apply_patch");
    assert_eq!(value["strict"], false);
    assert_eq!(value["parameters"]["type"], "object");
    assert_eq!(value["parameters"]["required"][0], "patch");
    assert_eq!(value["parameters"]["properties"]["patch"]["type"], "string");
    assert_eq!(value["parameters"]["additionalProperties"], false);
}
