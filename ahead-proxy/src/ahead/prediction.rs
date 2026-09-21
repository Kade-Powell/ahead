//! AHEAD Edit Prediction Context Assembler & Dispatch
//!
//! Grounded in Section 8.3 of `ahead-editor-mvp.md`.
//! Combines work context (active issue, goal, invariants, phase) with live unsaved buffers,
//! recent edits, and diagnostics without requiring repository-wide agent invocations.

use ahead_rpc::ahead::{
    AssistanceMode, PredictionRequest, PredictionResult, RepoPath, WorkKind,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::Path;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredictionProviderConfig {
    pub provider: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
}

impl Default for PredictionProviderConfig {
    fn default() -> Self {
        Self {
            provider: "ollama".to_string(),
            base_url: "http://localhost:11434/v1".to_string(),
            api_key: None,
            model: "qwen2.5-coder:7b".to_string(),
        }
    }
}

impl PredictionProviderConfig {
    /// Loads non-secret provider settings with project-over-user precedence.
    /// The API key is intentionally read from the user file only.
    pub fn from_workspace(workspace: Option<&Path>) -> Self {
        let mut configs = Vec::new();
        if let Ok(home) = std::env::var("HOME") {
            let path = Path::new(&home).join(".ahead").join("settings.toml");
            if let Some(value) = read_toml(&path) {
                configs.push((path, value));
            }
        }
        if let Some(workspace) = workspace {
            for path in [
                workspace.join(".ahead/config.toml"),
                workspace.join(".ahead/config.local.toml"),
            ] {
                if let Some(value) = read_toml(&path) {
                    configs.push((path, value));
                }
            }
        }

        let defaults = Self::default();
        let value = |key: &str| {
            configs
                .iter()
                .rev()
                .find_map(|(_, config)| {
                    config
                        .get("ai")
                        .and_then(|ai| ai.get(key))
                        .and_then(Value::as_str)
                        .filter(|value| !value.trim().is_empty())
                        .map(str::to_string)
                })
        };
        let api_key = configs
            .first()
            .and_then(|(_, config)| config.get("ai"))
            .and_then(|ai| ai.get("api_key"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string);

        Self {
            provider: value("provider").unwrap_or(defaults.provider),
            base_url: value("base_url").unwrap_or(defaults.base_url),
            api_key,
            model: value("model").unwrap_or(defaults.model),
        }
    }
}

fn read_toml(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path)
        .ok()?
        .parse::<toml::Value>()
        .ok()
        .map(|value| serde_json::to_value(value).ok())
        .flatten()
}

fn completion_endpoint(base_url: &str) -> Result<String> {
    let url = reqwest::Url::parse(base_url.trim_end_matches('/'))
        .context("invalid prediction provider base URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("prediction provider URL must use http or https");
    }
    if url.path().ends_with("/completions") {
        Ok(url.to_string())
    } else {
        Ok(format!("{}/completions", url.to_string().trim_end_matches('/')))
    }
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
        prompt.push_str("# AHEAD Prediction Context\n");
        prompt.push_str(&format!("Mode: {:?}\n", work.mode));
        prompt.push_str(&format!("Work Kind: {}\n", work.work_kind.display_name()));
        prompt.push_str(&format!("Phase: {}\n", work.phase_title));

        if let Some(ref issue) = work.primary_issue {
            prompt.push_str(&format!("Issue: {issue}\n"));
        }

        if !work.active_invariants.is_empty() {
            prompt.push_str("Invariants:\n");
            for inv in &work.active_invariants {
                prompt.push_str(&format!("- {inv}\n"));
            }
        }

        if !request.work_context.trim().is_empty() {
            prompt.push_str("\n# Session Context:\n");
            prompt.push_str(request.work_context.trim());
            prompt.push('\n');
        }

        if !open_buffers.is_empty() {
            prompt.push_str("\n# Relevant Open Buffers:\n");
            for buf in open_buffers {
                prompt.push_str(&format!(
                    "--- File: {} ---\n{}\n",
                    buf.path, buf.relevant_excerpt
                ));
            }
        }

        prompt.push_str(&format!("\n# Active Buffer: {}\n", request.path));
        prompt.push_str(&format!("Prefix:\n{}\n", request.prefix));
        prompt.push_str("<CURSOR>\n");
        prompt.push_str(&format!("Suffix:\n{}\n", request.suffix));

        Ok(prompt)
    }

    /// Dispatches a no-tools FIM request to the explicitly configured
    /// OpenAI-compatible completion endpoint. Provider errors are surfaced to
    /// the caller; there is no fabricated or cross-provider fallback.
    pub fn predict(
        work: &PredictionWorkContext,
        request: &PredictionRequest,
        open_buffers: &[OpenBufferContext],
        provider: &PredictionProviderConfig,
    ) -> Result<PredictionResult> {
        let prompt = Self::assemble_context(work, request, open_buffers)?;
        if provider.provider.eq_ignore_ascii_case("anthropic") {
            bail!("Anthropic direct does not expose the configured FIM completion route");
        }
        if provider.model.trim().is_empty() {
            bail!("prediction provider model is empty");
        }
        let endpoint = completion_endpoint(&provider.base_url)?;
        let endpoint_url = reqwest::Url::parse(&endpoint)
            .context("invalid prediction completion URL")?;
        let mut client_builder = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(5));
        if endpoint_url.host_str().is_some_and(|host| {
            matches!(host, "localhost" | "127.0.0.1" | "::1")
        }) {
            client_builder = client_builder.no_proxy();
        }
        let client = client_builder
            .build()
            .context("build prediction provider client")?;
        let body = json!({
            "model": provider.model,
            "prompt": prompt,
            "suffix": request.suffix,
            "max_tokens": 128,
            "temperature": 0.2,
            "stream": false
        });
        let mut http = client.post(endpoint).json(&body);
        if let Some(api_key) = provider.api_key.as_deref() {
            http = http.bearer_auth(api_key);
        }
        let response = http.send().context("send prediction request")?;
        let status = response.status();
        if !status.is_success() {
            bail!("prediction provider returned HTTP {status}");
        }
        let payload = response
            .json::<Value>()
            .context("decode prediction provider response")?;
        let replacement = payload
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| {
                choice
                    .get("text")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        choice
                            .get("message")
                            .and_then(|message| message.get("content"))
                            .and_then(Value::as_str)
                    })
            })
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .context("prediction provider returned no completion text")?;

        Ok(PredictionResult {
            request_id: request.request_id.clone(),
            cursor_offset: replacement.len(),
            replacement,
        })
    }

    /// Test-only deterministic context probe. Production dispatch must use
    /// [`Self::predict`] so the editor never displays fabricated code.
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
    use ahead_rpc::ahead::DisplayPosition;

    #[test]
    fn completion_endpoint_preserves_explicit_route() {
        assert_eq!(
            completion_endpoint("http://localhost:11434/v1").unwrap(),
            "http://localhost:11434/v1/completions"
        );
        assert_eq!(
            completion_endpoint("https://example.test/completions").unwrap(),
            "https://example.test/completions"
        );
        assert!(completion_endpoint("file:///tmp/provider").is_err());
    }

    #[test]
    fn prediction_sends_context_to_a_local_completion_provider() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            let mut body_start;
            let mut content_length;
            loop {
                let read = stream.read(&mut chunk).unwrap();
                assert!(read > 0, "provider closed before sending a request");
                request.extend_from_slice(&chunk[..read]);
                if let Some(header_end) = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                {
                    body_start = header_end + 4;
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.trim()
                                .eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())?
                        })
                        .unwrap_or(0);
                    if request.len() >= body_start + content_length {
                        break;
                    }
                }
            }
            let body = String::from_utf8(request[body_start..body_start + content_length].to_vec())
                .unwrap();
            let response_body = r#"{"choices":[{"text":"retry_with_backoff()"}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            )
            .unwrap();
            body
        });

        let work = PredictionWorkContext {
            work_kind: WorkKind::ProductChange,
            mode: AssistanceMode::Assist,
            phase_title: "Implement".to_string(),
            primary_issue: None,
            active_invariants: vec!["Preserve idempotency".to_string()],
        };
        let request = PredictionRequest {
            request_id: "provider-1".to_string(),
            session_id: "session-1".to_string(),
            path: "src/retry.rs".to_string(),
            cursor: DisplayPosition { line: 4, col: 7 },
            prefix: "pub fn retry".to_string(),
            suffix: " {\n}".to_string(),
            work_context: "Active step: implement the retry policy".to_string(),
        };
        let provider = PredictionProviderConfig {
            provider: "openai-compatible".to_string(),
            base_url: format!("http://{address}/v1"),
            api_key: None,
            model: "test-fim".to_string(),
        };

        let result = PredictionEngine::predict(&work, &request, &[], &provider).unwrap();
        let body = server.join().unwrap();
        assert_eq!(result.replacement, "retry_with_backoff()");
        assert!(body.contains("Preserve idempotency"));
        assert!(body.contains("Active step: implement the retry policy"));
        assert!(body.contains("pub fn retry"));
        assert!(body.contains("\"suffix\":\" {\\n}\""));
    }

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
            work_context:
                "Decision: preserve idempotency\nActive step: add eligibility check"
                    .to_string(),
        };

        let open_buffers = vec![OpenBufferContext {
            path: "src/client.rs".to_string(),
            relevant_excerpt: "pub struct Request;".to_string(),
        }];

        let prompt =
            PredictionEngine::assemble_context(&work, &request, &open_buffers)
                .unwrap();
        assert!(prompt.contains("Preserve idempotency"));
        assert!(prompt.contains("#142 Improve retries"));
        assert!(prompt.contains("src/client.rs"));
        assert!(prompt.contains("pub fn "));
        assert!(prompt.contains("preserve idempotency"));
        assert!(prompt.contains("add eligibility check"));

        let res =
            PredictionEngine::predict_mechanical(&work, &request, &open_buffers)
                .unwrap();
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
            prefix: String::new(),
            suffix: String::new(),
            work_context: String::new(),
        };

        let err =
            PredictionEngine::assemble_context(&work, &request, &[]).unwrap_err();
        assert!(err.to_string().contains("disabled in Learn mode"));
    }
}
