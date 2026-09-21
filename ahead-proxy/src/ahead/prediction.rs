//! AHEAD Edit Prediction Context Assembler & Dispatch
//!
//! Grounded in Section 8.3 of `ahead-editor-mvp.md`.
//! Combines work context (active issue, goal, invariants, phase) with live unsaved buffers,
//! recent edits, and diagnostics without requiring repository-wide agent invocations.
//!
//! FIM behavior follows Zed's `edit_prediction` implementation at the pinned
//! checkout recorded in `docs/development/zed-tracking.md`
//! (`crates/edit_prediction/src/fim.rs`, `ollama.rs`,
//! `open_ai_compatible.rs`, `cursor_excerpt.rs`,
//! `crates/edit_prediction_types/src/edit_prediction_types.rs`):
//! bounded cursor excerpts, per-model prompt formats, stop tokens,
//! completion cleaning, Ollama `/api/generate` routing and
//! prefix-interpolation on typing. AHEAD additions on top: the work-session
//! context header (phase, invariants) rendered as leading code comments,
//! and the explicit no-tools OpenAI-compatible fallback for unknown models.
//! GPL Zed code is referenced for behavior only; everything below is an
//! independent implementation (see decision 0006 clean-room obligation).

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
    ///
    /// Follows Zed's FIM flow (`fim.rs`): bound the excerpt around the
    /// cursor, format the prompt for the model's family, route Ollama to
    /// `/api/generate`, send stop tokens, and clean leaked format tokens
    /// from the completion. Models without a known FIM format fall back to
    /// the assembled session-context prompt.
    pub fn predict(
        work: &PredictionWorkContext,
        request: &PredictionRequest,
        open_buffers: &[OpenBufferContext],
        provider: &PredictionProviderConfig,
    ) -> Result<PredictionResult> {
        if provider.provider.eq_ignore_ascii_case("anthropic") {
            bail!("Anthropic direct does not expose the configured FIM completion route");
        }
        if provider.model.trim().is_empty() {
            bail!("prediction provider model is empty");
        }

        if infer_prompt_format(&provider.model).is_some() {
            return Self::predict_fim(work, request, provider);
        }

        let prompt = Self::assemble_context(work, request, open_buffers)?;
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
        let replacement = extract_completion_text(&payload)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .context("prediction provider returned no completion text")?;

        Ok(PredictionResult {
            request_id: request.request_id.clone(),
            cursor_offset: replacement.len(),
            replacement,
        })
    }

    /// FIM path for models with a known prompt format. The prompt carries
    /// only the bounded excerpt (plus a compact session header); the model
    /// family determines tags, routing and stop tokens.
    fn predict_fim(
        work: &PredictionWorkContext,
        request: &PredictionRequest,
        provider: &PredictionProviderConfig,
    ) -> Result<PredictionResult> {
        let format = infer_prompt_format(&provider.model)
            .context("no FIM prompt format for model")?;
        let full = format!("{}{}", request.prefix, request.suffix);
        let cursor = request.prefix.len().min(full.len());
        let (start, end) = cursor_excerpt_bounds(&full, cursor, MAX_EXCERPT_TOKENS);
        let prefix = full.get(start..cursor).unwrap_or_default();
        let suffix = full.get(cursor..end).unwrap_or_default();

        let mut prompt = String::new();
        if let Some(header) =
            session_fim_header(work, comment_prefix_for_path(&request.path))
        {
            prompt.push_str(&header);
        }
        prompt.push_str(&format_fim_prompt(format, prefix, suffix));
        let stop = fim_stop_tokens();

        let (replacement, request_id) = if provider.provider.eq_ignore_ascii_case("ollama") {
            Self::post_ollama_generate(provider, &prompt, &stop)?
        } else {
            Self::post_completions(provider, &prompt, &stop)?
        };
        let replacement = clean_fim_completion(&replacement);
        if replacement.is_empty() {
            bail!("prediction provider returned no completion text");
        }
        Ok(PredictionResult {
            request_id: request.request_id.clone(),
            cursor_offset: replacement.len(),
            replacement,
        })
    }

    /// Ollama native generate route (`/api/generate`, raw prompt), mirroring
    /// Zed's `ollama.rs`. Returns the completion text and request id.
    fn post_ollama_generate(
        provider: &PredictionProviderConfig,
        prompt: &str,
        stop: &[String],
    ) -> Result<(String, String)> {
        let base = provider.base_url.trim_end_matches('/');
        let base = base.strip_suffix("/v1").unwrap_or(base);
        let endpoint = format!("{base}/api/generate");
        let client = blocking_client(&endpoint)?;
        let body = json!({
            "model": provider.model,
            "prompt": prompt,
            "raw": true,
            "stream": false,
            "options": {
                "num_predict": 128,
                "temperature": 0.2,
                "stop": stop,
            },
        });
        let mut http = client.post(endpoint).json(&body);
        if let Some(api_key) = provider.api_key.as_deref() {
            http = http.bearer_auth(api_key);
        }
        let response = http.send().context("send prediction request")?;
        if !response.status().is_success() {
            bail!("prediction provider returned HTTP {}", response.status());
        }
        let payload = response
            .json::<Value>()
            .context("decode prediction provider response")?;
        let text = extract_completion_text(&payload).unwrap_or_default();
        Ok((text.to_string(), String::new()))
    }

    /// OpenAI-compatible `/completions` route with stop tokens.
    fn post_completions(
        provider: &PredictionProviderConfig,
        prompt: &str,
        stop: &[String],
    ) -> Result<(String, String)> {
        let endpoint = completion_endpoint(&provider.base_url)?;
        let client = blocking_client(&endpoint)?;
        let body = json!({
            "model": provider.model,
            "prompt": prompt,
            "max_tokens": 128,
            "temperature": 0.2,
            "stop": stop,
            "stream": false
        });
        let mut http = client.post(endpoint).json(&body);
        if let Some(api_key) = provider.api_key.as_deref() {
            http = http.bearer_auth(api_key);
        }
        let response = http.send().context("send prediction request")?;
        if !response.status().is_success() {
            bail!("prediction provider returned HTTP {}", response.status());
        }
        let payload = response
            .json::<Value>()
            .context("decode prediction provider response")?;
        let id = payload
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let text = extract_completion_text(&payload).unwrap_or_default();
        Ok((text.to_string(), id))
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

/// Builds a blocking HTTP client that bypasses proxies for loopback
/// endpoints (local providers must not depend on proxy env).
fn blocking_client(endpoint: &str) -> Result<reqwest::blocking::Client> {
    let endpoint_url =
        reqwest::Url::parse(endpoint).context("invalid prediction URL")?;
    let mut builder = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5));
    if endpoint_url.host_str().is_some_and(|host| {
        matches!(host, "localhost" | "127.0.0.1" | "::1")
    }) {
        builder = builder.no_proxy();
    }
    builder.build().context("build prediction provider client")
}

/// Reads completion text from either an OpenAI-style `choices` payload or
/// an Ollama `/api/generate` payload (`response` field).
fn extract_completion_text(payload: &Value) -> Option<&str> {
    payload
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| {
            choice.get("text").and_then(Value::as_str).or_else(|| {
                choice
                    .get("message")
                    .and_then(|message| message.get("content"))
                    .and_then(Value::as_str)
            })
        })
        .or_else(|| payload.get("response").and_then(Value::as_str))
}

/// FIM prompt families, mirroring Zed's `EditPredictionPromptFormat`
/// restricted to models servable over plain FIM endpoints (Zed's
/// cloud-only Zeta/Sweep formats are intentionally unsupported here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptFormat {
    CodeLlama,
    StarCoder,
    DeepseekCoder,
    Qwen,
    CodeGemma,
    Codestral,
    Glm,
    Generic,
}

/// Infers the FIM prompt format from a model name, mirroring Zed's
/// `fim::infer_prompt_format`. Returns `None` for models without a known
/// FIM format (including Zed-cloud-only families); callers fall back to
/// the assembled session-context prompt.
pub fn infer_prompt_format(model: &str) -> Option<PromptFormat> {
    let base = model.split(':').next().unwrap_or(model);
    Some(match base {
        "codellama" | "code-llama" => PromptFormat::CodeLlama,
        "starcoder" | "starcoder2" | "starcoderbase" => PromptFormat::StarCoder,
        "deepseek-coder" | "deepseek-coder-v2" => PromptFormat::DeepseekCoder,
        "qwen2.5-coder" | "qwen-coder" | "qwen" => PromptFormat::Qwen,
        "codegemma" => PromptFormat::CodeGemma,
        "codestral" | "mistral" => PromptFormat::Codestral,
        "glm" | "glm-4" | "glm-4.5" => PromptFormat::Glm,
        _ => {
            return None;
        }
    })
}

/// Formats a fill-in-the-middle prompt for the model family, mirroring
/// Zed's `format_fim_prompt`.
pub fn format_fim_prompt(
    format: PromptFormat,
    prefix: &str,
    suffix: &str,
) -> String {
    match format {
        PromptFormat::CodeLlama => {
            format!("<PRE> {prefix} <SUF>{suffix} <MID>")
        }
        PromptFormat::StarCoder | PromptFormat::Generic => {
            format!("<fim_prefix>{prefix}<fim_suffix>{suffix}<fim_middle>")
        }
        PromptFormat::DeepseekCoder => {
            format!("<｜fim▁begin｜>{prefix}<｜fim▁hole｜>{suffix}<｜fim▁end｜>")
        }
        PromptFormat::Qwen | PromptFormat::CodeGemma => {
            format!("<|fim_prefix|>{prefix}<|fim_suffix|>{suffix}<|fim_middle|>")
        }
        PromptFormat::Codestral => {
            format!("[SUFFIX]{suffix}[PREFIX]{prefix}")
        }
        PromptFormat::Glm => {
            format!("<|code_prefix|>{prefix}<|code_suffix|>{suffix}<|code_middle|>")
        }
    }
}

/// Stop tokens covering every supported FIM family, mirroring Zed's
/// `get_fim_stop_tokens`.
pub fn fim_stop_tokens() -> Vec<String> {
    [
        "<|endoftext|>",
        "<|file_separator|>",
        "<|fim_pad|>",
        "<|fim_prefix|>",
        "<|fim_middle|>",
        "<|fim_suffix|>",
        "<fim_prefix>",
        "<fim_middle>",
        "<fim_suffix>",
        "<PRE>",
        "<SUF>",
        "<MID>",
        "[PREFIX]",
        "[SUFFIX]",
    ]
    .iter()
    .map(ToString::to_string)
    .collect()
}

/// Truncates a completion at the first leaked format token, mirroring
/// Zed's `clean_fim_completion`.
pub fn clean_fim_completion(response: &str) -> String {
    let mut result = response.to_string();
    for token in fim_stop_tokens() {
        if let Some(pos) = result.find(&token) {
            result.truncate(pos);
        }
    }
    result
}

/// Token-count guess (bytes/3, erring low), mirroring Zed's
/// `cursor_excerpt::guess_token_count`.
pub fn guess_token_count(bytes: usize) -> usize {
    bytes / 3
}

/// Total excerpt budget in tokens, mirroring Zed's
/// `CURSOR_EXCERPT_TOKEN_BUDGET`.
pub const MAX_EXCERPT_TOKENS: usize = 8192;

/// Computes a linewise excerpt window around the cursor that fits the token
/// budget, mirroring Zed's `compute_cursor_excerpt` (symmetric expansion,
/// down first then up). Returns byte offsets into `text`; inputs are
/// clamped to valid boundaries so arbitrary offsets cannot panic.
pub fn cursor_excerpt_bounds(
    text: &str,
    cursor: usize,
    budget_tokens: usize,
) -> (usize, usize) {
    let cursor = cursor.min(text.len());
    let line_starts = line_start_offsets(text);
    if line_starts.is_empty() {
        return (0, 0);
    }
    let cursor_row = line_starts
        .iter()
        .rposition(|&start| start <= cursor)
        .unwrap_or(0);
    let mut budget = budget_tokens.saturating_sub(line_token_count(text, cursor_row));
    let mut start_row = cursor_row;
    let mut end_row = cursor_row;
    loop {
        let can_up = start_row > 0;
        let can_down = end_row + 1 < line_starts.len();
        if budget == 0 || (!can_up && !can_down) {
            break;
        }
        if can_down {
            let cost = line_token_count(text, end_row + 1);
            if cost <= budget {
                end_row += 1;
                budget = budget.saturating_sub(cost);
            } else {
                break;
            }
        }
        if can_up && budget > 0 {
            let cost = line_token_count(text, start_row - 1);
            if cost <= budget {
                start_row -= 1;
                budget = budget.saturating_sub(cost);
            } else {
                break;
            }
        }
    }
    let start = line_starts[start_row];
    let end = line_starts
        .get(end_row + 1)
        .copied()
        .unwrap_or_else(|| text.len());
    (start, end)
}

fn line_start_offsets(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' && index + 1 < text.len() {
            starts.push(index + 1);
        }
    }
    starts
}

fn line_token_count(text: &str, row: usize) -> usize {
    let starts = line_start_offsets(text);
    let Some(&start) = starts.get(row) else {
        return 1;
    };
    let end = starts.get(row + 1).copied().unwrap_or_else(|| text.len());
    guess_token_count(end.saturating_sub(start)).max(1)
}

/// Comment prefix for rendering the session header inside FIM prompts.
/// Returns `None` for languages without a clear line-comment style, in
/// which case the header is omitted rather than corrupting the hole.
fn comment_prefix_for_path(path: &str) -> Option<&'static str> {
    let extension = Path::new(path).extension()?.to_str()?;
    match extension {
        "rs" | "js" | "jsx" | "ts" | "tsx" | "go" | "java" | "c" | "h"
        | "cpp" | "hpp" | "swift" | "kt" | "kts" | "scala" | "cs" | "php" => {
            Some("//")
        }
        "py" | "sh" | "bash" | "rb" | "toml" | "yaml" | "yml" | "r" | "pl" => {
            Some("#")
        }
        "lua" | "sql" | "hs" => Some("--"),
        _ => None,
    }
}

/// Renders the AHEAD work-session context as compact leading comment lines
/// for FIM prompts: phase plus capped invariants. This is the AHEAD
/// addition on top of Zed's pure prefix/suffix hole.
fn session_fim_header(
    work: &PredictionWorkContext,
    comment: Option<&str>,
) -> Option<String> {
    let comment = comment?;
    let mut header = format!("{comment} AHEAD {:?} · {}\n", work.mode, work.phase_title);
    let invariants: Vec<&str> = work
        .active_invariants
        .iter()
        .take(3)
        .map(String::as_str)
        .collect();
    if !invariants.is_empty() {
        header.push_str(&format!("{comment} Invariants: {}\n", invariants.join("; ")));
    }
    Some(header)
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

    #[test]
    fn infer_prompt_format_matches_known_families() {
        assert_eq!(
            infer_prompt_format("qwen2.5-coder:7b"),
            Some(PromptFormat::Qwen)
        );
        assert_eq!(
            infer_prompt_format("codellama:7b"),
            Some(PromptFormat::CodeLlama)
        );
        assert_eq!(
            infer_prompt_format("deepseek-coder-v2:16b"),
            Some(PromptFormat::DeepseekCoder)
        );
        assert_eq!(
            infer_prompt_format("starcoder2:3b"),
            Some(PromptFormat::StarCoder)
        );
        assert_eq!(
            infer_prompt_format("codestral:latest"),
            Some(PromptFormat::Codestral)
        );
        assert_eq!(
            infer_prompt_format("glm-4:9b"),
            Some(PromptFormat::Glm)
        );
        assert_eq!(infer_prompt_format("test-fim"), None);
        assert_eq!(infer_prompt_format("llama3:70b"), None);
    }

    #[test]
    fn format_fim_prompt_wraps_hole_per_family() {
        assert_eq!(
            format_fim_prompt(PromptFormat::Qwen, "a", "b"),
            "<|fim_prefix|>a<|fim_suffix|>b<|fim_middle|>"
        );
        assert_eq!(
            format_fim_prompt(PromptFormat::CodeLlama, "a", "b"),
            "<PRE> a <SUF>b <MID>"
        );
        assert!(format_fim_prompt(PromptFormat::Codestral, "a", "b")
            .starts_with("[SUFFIX]"));
    }

    #[test]
    fn clean_fim_completion_truncates_leaked_tokens() {
        assert_eq!(
            clean_fim_completion("ok()<|fim_suffix|>trailing"),
            "ok()"
        );
        assert_eq!(clean_fim_completion("ok()"), "ok()");
    }

    #[test]
    fn cursor_excerpt_bounds_cover_small_buffers_fully() {
        let text = "fn a() {}\nfn b() {}\n";
        assert_eq!(cursor_excerpt_bounds(text, 5, MAX_EXCERPT_TOKENS), (0, text.len()));
    }

    #[test]
    fn cursor_excerpt_bounds_window_around_cursor() {
        let text = (0..400).map(|i| format!("line {i:03}\n")).collect::<String>();
        let cursor = text.find("line 200").unwrap();
        let (start, end) = cursor_excerpt_bounds(&text, cursor, 60);
        assert!(start <= cursor && cursor <= end);
        assert!(end - start < text.len());
        assert!(text.is_char_boundary(start) && text.is_char_boundary(end));
        assert!(start == 0 || text[..start].ends_with('\n'));
    }

    #[test]
    fn session_fim_header_uses_file_comment_style() {
        let work = PredictionWorkContext {
            work_kind: WorkKind::ProductChange,
            mode: AssistanceMode::Assist,
            phase_title: "Implement".to_string(),
            primary_issue: None,
            active_invariants: vec!["Preserve idempotency".to_string()],
        };
        let header =
            session_fim_header(&work, comment_prefix_for_path("src/retry.rs")).unwrap();
        assert!(header.starts_with("// AHEAD"));
        assert!(header.contains("Preserve idempotency"));
        assert!(session_fim_header(&work, comment_prefix_for_path("notes.xyz")).is_none());
    }

    /// Minimal HTTP stub: reads one request with Content-Length framing and
    /// answers with a fixed JSON body, returning the raw request body.
    fn spawn_stub(
        response_body: &'static str,
    ) -> (
        std::net::SocketAddr,
        std::thread::JoinHandle<String>,
    ) {
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
            let body = String::from_utf8(
                request[body_start..body_start + content_length].to_vec(),
            )
            .unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            )
            .unwrap();
            body
        });
        (address, server)
    }

    #[test]
    fn prediction_fim_path_formats_prompt_and_stop_tokens() {
        let work = PredictionWorkContext {
            work_kind: WorkKind::ProductChange,
            mode: AssistanceMode::Assist,
            phase_title: "Implement".to_string(),
            primary_issue: None,
            active_invariants: vec!["Preserve idempotency".to_string()],
        };
        let request = PredictionRequest {
            request_id: "fim-1".to_string(),
            session_id: "session-1".to_string(),
            path: "src/retry.rs".to_string(),
            cursor: DisplayPosition { line: 4, col: 7 },
            prefix: "pub fn retry".to_string(),
            suffix: " {\n}".to_string(),
            work_context: String::new(),
        };
        let (address, server) =
            spawn_stub(r#"{"choices":[{"text":"retry_with_backoff()"}]}"#);
        let provider = PredictionProviderConfig {
            provider: "openai-compatible".to_string(),
            base_url: format!("http://{address}/v1"),
            api_key: None,
            model: "qwen2.5-coder:7b".to_string(),
        };

        let result =
            PredictionEngine::predict(&work, &request, &[], &provider).unwrap();
        let body = server.join().unwrap();
        assert_eq!(result.replacement, "retry_with_backoff()");
        assert!(body.contains("<|fim_prefix|>pub fn retry"));
        assert!(body.contains("\"stop\""));
        assert!(body.contains("Invariants: Preserve idempotency"));
    }

    #[test]
    fn prediction_ollama_uses_generate_route() {
        let work = PredictionWorkContext {
            work_kind: WorkKind::ProductChange,
            mode: AssistanceMode::Assist,
            phase_title: "Implement".to_string(),
            primary_issue: None,
            active_invariants: vec![],
        };
        let request = PredictionRequest {
            request_id: "fim-2".to_string(),
            session_id: "session-1".to_string(),
            path: "src/retry.rs".to_string(),
            cursor: DisplayPosition { line: 1, col: 0 },
            prefix: "pub fn ".to_string(),
            suffix: String::new(),
            work_context: String::new(),
        };
        let (address, server) = spawn_stub(r#"{"response":"ok()"}"#);
        let provider = PredictionProviderConfig {
            provider: "ollama".to_string(),
            base_url: format!("http://{address}/v1"),
            api_key: None,
            model: "qwen2.5-coder:7b".to_string(),
        };

        let result =
            PredictionEngine::predict(&work, &request, &[], &provider).unwrap();
        let body = server.join().unwrap();
        assert_eq!(result.replacement, "ok()");
        assert!(body.contains("\"raw\":true"));
        assert!(body.contains("\"num_predict\""));
    }
}
