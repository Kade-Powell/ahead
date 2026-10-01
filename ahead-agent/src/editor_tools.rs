//! Agent-facing tools backed by the AHEAD editor.
//!
//! Keep these schemas and their argument validation independent of any agent
//! runtime so every adapter can expose the same editor capability.

use std::path::Path;

use ahead_rpc::ahead::AgentPresentationAction;
use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::runtime_support::path_is_allowed;

/// Tool name for reading unsaved content from an open editor buffer.
pub const AHEAD_READ_EDITOR_BUFFER_TOOL: &str = "read_editor_buffer";
/// Tool name for opening and highlighting an exact code quote.
pub const AHEAD_PRESENT_CODE_TOOL: &str = "present_code";
/// Tool name for moving the agent pointer while keeping the active highlight.
pub const AHEAD_MOVE_CODE_POINTER_TOOL: &str = "move_code_pointer";
/// Tool name for clearing a presentation cue.
pub const AHEAD_CLEAR_PRESENTATION_TOOL: &str = "clear_presentation";
/// Tool name for speaking an explanation aloud.
pub const AHEAD_SPEAK_TEXT_TOOL: &str = "speak_text";
/// Tool name for stopping speech without cancelling the agent turn.
pub const AHEAD_STOP_SPEAKING_TOOL: &str = "stop_speaking";

/// Returns the shared model-facing definitions for AHEAD editor tools.
pub fn presentation_tool_definitions() -> Vec<(String, String, Value)> {
    vec![
        (
            AHEAD_READ_EDITOR_BUFFER_TOOL.to_string(),
            "Read the current unsaved contents of one open editor buffer. Use this to verify an exact quote before calling present_code. Private files and files outside the worktree are not available.".to_string(),
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Worktree-relative path of an open file, such as src/main.rs." }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        (
            AHEAD_PRESENT_CODE_TOOL.to_string(),
            "Open a worktree-relative file and point at an exact quote in the live editor buffer. Show a short label and explanatory note beside the highlight without moving the human caret. The editor rejects ambiguous or stale quotes.".to_string(),
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Worktree-relative path, such as src/main.rs." },
                    "quote": { "type": "string", "description": "Exact code text to highlight. Include line breaks when the passage spans lines." },
                    "label": { "type": "string", "description": "Short pointer label." },
                    "note": { "type": "string", "description": "A concise inline explanation shown beside the highlighted code." }
                },
                "required": ["path", "quote", "label", "note"],
                "additionalProperties": false
            }),
        ),
        (
            AHEAD_MOVE_CODE_POINTER_TOOL.to_string(),
            "Move the agent pointer to an exact, unique quote in the file for an active presentation. This leaves the highlighted range and inline note in place and returns after the new pointer is displayed; call read_editor_buffer first to verify the quote.".to_string(),
            json!({
                "type": "object",
                "properties": {
                    "cue_id": { "type": "string", "description": "Active cue id returned by present_code." },
                    "quote": { "type": "string", "description": "Exact code text in the same file to point at. Include line breaks when needed." }
                },
                "required": ["cue_id", "quote"],
                "additionalProperties": false
            }),
        ),
        (
            AHEAD_CLEAR_PRESENTATION_TOOL.to_string(),
            "Remove the current code pointer and inline note. Omit cue_id to clear the active pointer, or supply the cue_id returned by present_code to clear only that cue.".to_string(),
            json!({
                "type": "object",
                "properties": { "cue_id": { "type": "string" } },
                "additionalProperties": false
            }),
        ),
        (
            AHEAD_SPEAK_TEXT_TOOL.to_string(),
            "Speak a short explanation aloud using the operating system voice. When speaking about code, call present_code first and pass its cue_id so speech stays tied to the highlighted passage. User keyboard input stops speech without cancelling the agent turn.".to_string(),
            json!({
                "type": "object",
                "properties": {
                    "cue_id": { "type": "string", "description": "Optional active presentation cue id." },
                    "text": { "type": "string", "description": "The short explanation to speak aloud." }
                },
                "required": ["text"],
                "additionalProperties": false
            }),
        ),
        (
            AHEAD_STOP_SPEAKING_TOOL.to_string(),
            "Stop the current spoken explanation without cancelling the agent turn.".to_string(),
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
    ]
}

/// Validates an editor tool call and converts it to an action for the editor.
///
/// `allowed_paths` should contain the calling session's scope when it has one.
/// The active workspace and private-file checks always apply.
pub fn parse_presentation_action(
    tool: &str,
    arguments: Value,
    workspace: &Path,
    allowed_paths: Option<&[String]>,
) -> Result<AgentPresentationAction> {
    if tool == AHEAD_MOVE_CODE_POINTER_TOOL {
        let cue_id = arguments
            .get("cue_id")
            .and_then(Value::as_str)
            .filter(|cue_id| !cue_id.trim().is_empty())
            .context("AHEAD move_code_pointer requires an active cue_id")?;
        let quote = arguments
            .get("quote")
            .and_then(Value::as_str)
            .filter(|quote| !quote.is_empty())
            .context("AHEAD move_code_pointer requires an exact code quote")?;
        anyhow::ensure!(
            cue_id.len() <= 128,
            "AHEAD presentation cue id is too long"
        );
        anyhow::ensure!(quote.len() <= 4_096, "AHEAD pointer quote is too long");
        return Ok(AgentPresentationAction::MovePointer {
            cue_id: cue_id.to_string(),
            quote: quote.to_string(),
        });
    }
    if tool == AHEAD_CLEAR_PRESENTATION_TOOL {
        let cue_id = arguments
            .get("cue_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        return Ok(AgentPresentationAction::Clear { cue_id });
    }
    if tool == AHEAD_STOP_SPEAKING_TOOL {
        return Ok(AgentPresentationAction::StopSpeaking);
    }
    if tool == AHEAD_SPEAK_TEXT_TOOL {
        let text = arguments
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .context("AHEAD speak_text requires text to speak")?;
        anyhow::ensure!(text.len() <= 4_000, "AHEAD speech text is too long");
        return Ok(AgentPresentationAction::Speak {
            cue_id: arguments
                .get("cue_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            text: text.to_string(),
        });
    }
    anyhow::ensure!(
        tool == AHEAD_PRESENT_CODE_TOOL,
        "Unknown AHEAD presentation tool `{tool}`"
    );

    let path = arguments
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .context("AHEAD present_code requires a worktree path")?;
    let quote = arguments
        .get("quote")
        .and_then(Value::as_str)
        .filter(|quote| !quote.is_empty())
        .context("AHEAD present_code requires the exact code quote to show")?;
    let label = arguments
        .get("label")
        .and_then(Value::as_str)
        .filter(|label| !label.trim().is_empty())
        .context("AHEAD present_code requires a short label")?;
    let note = arguments
        .get("note")
        .and_then(Value::as_str)
        .filter(|note| !note.trim().is_empty())
        .context("AHEAD present_code requires an explanatory note")?;
    anyhow::ensure!(path.len() <= 512, "AHEAD presentation path is too long");
    anyhow::ensure!(quote.len() <= 4_096, "AHEAD presentation quote is too long");
    anyhow::ensure!(label.len() <= 120, "AHEAD presentation label is too long");
    anyhow::ensure!(note.len() <= 2_000, "AHEAD presentation note is too long");

    let model_path = Path::new(path);
    let relative = if model_path.is_absolute() {
        model_path
            .strip_prefix(workspace)
            .context("AHEAD presentation path is outside the active worktree")?
    } else {
        model_path
    };
    anyhow::ensure!(
        relative
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_))),
        "AHEAD presentation path must stay inside the worktree"
    );
    anyhow::ensure!(
        !ahead_core::search::is_private_file(relative),
        "AHEAD cannot present a private file"
    );
    anyhow::ensure!(
        ahead_core::search::resolve_open_buffer_path(workspace, relative).is_some(),
        "AHEAD presentation path is not a safe file in the worktree"
    );
    if let Some(allowed_paths) = allowed_paths {
        anyhow::ensure!(
            path_is_allowed(path, allowed_paths, workspace),
            "AHEAD presentation path is outside this session's file scope"
        );
    }
    let relative_path = relative
        .to_str()
        .context("AHEAD presentation path is not valid UTF-8")?;

    Ok(AgentPresentationAction::Present {
        cue_id: uuid::Uuid::new_v4().to_string(),
        path: relative_path.to_string(),
        quote: quote.to_string(),
        label: label.to_string(),
        note: note.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_code_respects_the_calling_agents_file_scope() {
        let workspace = tempfile::tempdir().expect("create workspace");
        std::fs::create_dir_all(workspace.path().join("src"))
            .expect("create allowed directory");
        std::fs::create_dir_all(workspace.path().join("other"))
            .expect("create out-of-scope directory");
        std::fs::write(workspace.path().join("src/main.rs"), "fn main() {}")
            .expect("create allowed source");
        std::fs::write(workspace.path().join("other/main.rs"), "fn main() {}")
            .expect("create out-of-scope source");

        let allowed_paths = ["src".to_string()];
        let result = parse_presentation_action(
            AHEAD_PRESENT_CODE_TOOL,
            json!({
                "path": "other/main.rs",
                "quote": "fn main()",
                "label": "Entry point",
                "note": "This is outside the calling agent's scope.",
            }),
            workspace.path(),
            Some(&allowed_paths),
        );

        assert!(result.is_err());
    }
}
