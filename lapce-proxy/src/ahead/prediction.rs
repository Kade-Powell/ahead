//! AHEAD Edit Prediction Context Assembler & Dispatch
//!
//! Grounded in Section 8.3 of `ahead-editor-mvp.md`.
//! Combines work context (active issue, goal, invariants, phase) with live unsaved buffers,
//! recent edits, and diagnostics without requiring repository-wide agent invocations.

use lapce_rpc::ahead::{
    AssistanceMode, DisplayPosition, PredictionRequest, PredictionResult,
    RepoPath, WorkKind,
};
use anyhow::{bail, Result};

#[derive(Debug, Clone)]
pub struct PredictionWorkContext {
    pub work_kind: WorkKind,
    pub mode: AssistanceMode,
    pub phase_title: String,
    pub primary_issue: Option<String>,
    pub active_invariants: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct OpenBufferContext {
    pub path: RepoPath,
    pub relevant_excerpt: String,
}

pub struct PredictionEngine;

impl PredictionEngine {
    /// Assembles full context prompt for fast edit prediction
    pub fn assemble_context(
        work: &PredictionWorkContext,
        request: &PredictionRequest,
        open_buffers: &[OpenBufferContext],
    ) -> Result<String> {
        if matches!(work.mode, AssistanceMode::Learn) {
            bail!("Predictions are disabled in Learn mode");
        }

        let mut prompt = String::new();
        prompt.push_str(&format!("# AHEAD Prediction Context\n"));
        prompt.push_str(&format!("Mode: {:?}\n", work.mode));
        prompt.push_str(&format!("Work Kind: {}\n", work.work_kind.display_name()));
        prompt.push_str(&format!("Phase: {}\n", work.phase_title));

        if let Some(ref issue) = work.primary_issue {
            prompt.push_str(&format!("Issue: {}\n", issue));
        }

        if !work.active_invariants.is_empty() {
            prompt.push_str("Invariants:\n");
            for inv in &work.active_invariants {
                prompt.push_str(&format!("- {}\n", inv));
            }
        }

        if !open_buffers.is_empty() {
            prompt.push_str("\n# Relevant Open Buffers:\n");
            for buf in open_buffers {
                prompt.push_str(&format!("--- File: {} ---\n{}\n", buf.path, buf.relevant_excerpt));
            }
        }

        prompt.push_str(&format!("\n# Active Buffer: {}\n", request.path));
        prompt.push_str(&format!("Prefix:\n{}\n", request.prefix));
        prompt.push_str("<CURSOR>\n");
        prompt.push_str(&format!("Suffix:\n{}\n", request.suffix));

        Ok(prompt)
    }

    /// Fast mechanical prediction completion mock / probe
    pub fn predict_mechanical(
        work: &PredictionWorkContext,
        request: &PredictionRequest,
        open_buffers: &[OpenBufferContext],
    ) -> Result<PredictionResult> {
        let _context = Self::assemble_context(work, request, open_buffers)?;

        // For testing/probe: return a clean mechanical boilerplate completion
        let trimmed = request.prefix.trim_end();
        let replacement = if trimmed.ends_with("struct") {
            "Config {\n    pub timeout_ms: u64,\n}\n".to_string()
        } else if trimmed.ends_with("fn") {
            "retry_with_backoff() -> Result<()> {\n    Ok(())\n}\n".to_string()
        } else {
            "// AHEAD scaffolded assistance\n".to_string()
        };

        let cursor_offset = replacement.len();
        Ok(PredictionResult {
            request_id: request.request_id.clone(),
            replacement,
            cursor_offset,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assemble_context_includes_invariants_and_buffers() {
        let work = PredictionWorkContext {
            work_kind: WorkKind::ProductChange,
            mode: AssistanceMode::Assist,
            phase_title: "Implement".to_string(),
            primary_issue: Some("#142 Improve retries".to_string()),
            active_invariants: vec!["Preserve idempotency".to_string()],
        };

        let request = PredictionRequest {
            request_id: "req-1".to_string(),
            session_id: "sess-1".to_string(),
            path: "src/retry.rs".to_string(),
            cursor: DisplayPosition { line: 10, col: 4 },
            prefix: "pub fn ".to_string(),
            suffix: "\n}".to_string(),
            work_context: String::new(),
        };

        let open_buffers = vec![OpenBufferContext {
            path: "src/client.rs".to_string(),
            relevant_excerpt: "pub struct Request;".to_string(),
        }];

        let prompt = PredictionEngine::assemble_context(&work, &request, &open_buffers).unwrap();
        assert!(prompt.contains("Preserve idempotency"));
        assert!(prompt.contains("#142 Improve retries"));
        assert!(prompt.contains("src/client.rs"));
        assert!(prompt.contains("pub fn "));

        let res = PredictionEngine::predict_mechanical(&work, &request, &open_buffers).unwrap();
        assert_eq!(res.request_id, "req-1");
        assert!(res.replacement.contains("retry_with_backoff"));
    }

    #[test]
    fn test_prediction_rejected_in_learn_mode() {
        let work = PredictionWorkContext {
            work_kind: WorkKind::Investigation,
            mode: AssistanceMode::Learn,
            phase_title: "Scrutinize".to_string(),
            primary_issue: None,
            active_invariants: vec![],
        };

        let request = PredictionRequest {
            request_id: "req-2".to_string(),
            session_id: "sess-2".to_string(),
            path: "src/lib.rs".to_string(),
            cursor: DisplayPosition { line: 1, col: 0 },
            prefix: "".to_string(),
            suffix: "".to_string(),
            work_context: String::new(),
        };

        let err = PredictionEngine::assemble_context(&work, &request, &[]).unwrap_err();
        assert!(err.to_string().contains("disabled in Learn mode"));
    }
}
