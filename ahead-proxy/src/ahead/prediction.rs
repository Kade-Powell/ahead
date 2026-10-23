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
    AssistanceMode, PredictionModelContext, PredictionRequest, PredictionResult,
    RepoPath, SessionView, WorkKind,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

pub(crate) const MAX_PREDICTION_CONTEXT_BYTES: usize = 64 * 1024;
const MAX_PREDICTION_PROMPT_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub(crate) struct PredictionSessionVersion {
    pub format: u32,
    pub revision: u64,
}

#[derive(Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct PredictionContextSnapshot {
    pub work: PredictionWorkContext,
    pub request: PredictionRequest,
    pub provider: PredictionProviderConfig,
    pub model_context: Option<PredictionModelContext>,
    pub session: Option<SessionView>,
    pub session_context_version: Option<PredictionSessionVersion>,
    pub workspace: std::path::PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PredictionWorkContext {
    pub work_kind: WorkKind,
    pub mode: AssistanceMode,
    pub phase_title: String,
    pub primary_issue: Option<String>,
    pub active_invariants: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct PredictionCancellation(tokio::sync::watch::Sender<bool>);

impl Default for PredictionCancellation {
    fn default() -> Self {
        Self(tokio::sync::watch::channel(false).0)
    }
}

impl PredictionCancellation {
    pub(crate) fn cancel(&self) {
        self.0.send_replace(true);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }

    async fn cancelled(&self) {
        let mut receiver = self.0.subscribe();
        if receiver.wait_for(|cancelled| *cancelled).await.is_err() {
            tracing::debug!("prediction cancellation channel closed");
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OpenBufferContext {
    pub path: RepoPath,
    pub relevant_excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PredictionProviderConfig {
    pub provider_id: Option<String>,
    pub provider: String,
    pub base_url: String,
    #[serde(skip_serializing)]
    pub api_key: Option<String>,
    pub model: String,
}

impl Default for PredictionProviderConfig {
    fn default() -> Self {
        Self {
            provider_id: None,
            provider: "openai-compatible".to_string(),
            base_url: "http://localhost:1234/v1".to_string(),
            api_key: None,
            model: "openai-compatible-model".to_string(),
        }
    }
}

impl PredictionProviderConfig {
    /// Loads the selected session endpoint when it is configured, otherwise
    /// the active endpoint from the AHEAD provider catalog.
    ///
    /// Each `[[ai.connections]]` entry is kept as a separate endpoint. Later
    /// config layers override matching provider IDs. A key from an earlier
    /// private layer is retained only when the endpoint stays identical.
    pub fn from_workspace(workspace: Option<&Path>) -> Result<Self> {
        Self::from_workspace_for_model(workspace, None)
    }

    pub(crate) fn from_workspace_for_model(
        workspace: Option<&Path>,
        model_context: Option<&PredictionModelContext>,
    ) -> Result<Self> {
        let user_home = ahead_core::directory::Directory::home_dir();
        if model_context.is_none() {
            return Self::from_workspace_with_user_home(
                workspace,
                user_home.as_deref(),
            );
        }
        Self::from_workspace_with_user_home_and_model(
            workspace,
            user_home.as_deref(),
            model_context,
        )
    }

    fn from_workspace_with_user_home(
        workspace: Option<&Path>,
        user_home: Option<&Path>,
    ) -> Result<Self> {
        Self::from_workspace_with_user_home_and_model(workspace, user_home, None)
    }

    fn from_workspace_with_user_home_and_model(
        workspace: Option<&Path>,
        user_home: Option<&Path>,
        model_context: Option<&PredictionModelContext>,
    ) -> Result<Self> {
        let mut configs = Vec::new();
        if let Some(home) = user_home {
            match std::fs::symlink_metadata(home) {
                Ok(_) => {
                    if let Some(value) = read_toml(home, "settings.toml")
                        .context("user AHEAD provider settings")?
                    {
                        configs.push((value, true));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).context("failed to inspect AHEAD user home");
                }
            }
        }
        if let Some(workspace) = workspace {
            for filename in ["settings.toml", "config.toml", "config.local.toml"] {
                if let Some(value) = read_toml(workspace, filename)
                    .context("workspace AHEAD provider settings")?
                {
                    let private = filename != "config.toml";
                    configs.push((value, private));
                }
            }
        }

        let mut providers =
            BTreeMap::<String, (String, bool, PredictionProviderConfig)>::new();
        let mut provider_order = Vec::new();
        let mut active_connection = None;
        for (config, private) in configs {
            let Some(ai) = config.get("ai") else {
                continue;
            };
            if let Some(active) = ai
                .get("active_connection")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|active| !active.is_empty())
            {
                active_connection = Some(active.to_string());
            }
            let fallback_provider = ai
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or("openai-compatible");
            if let Some(connections) =
                ai.get("connections").and_then(Value::as_array)
            {
                for connection in connections {
                    if let Some((id, name, provider)) =
                        prediction_provider(connection, fallback_provider)
                    {
                        if !provider_order.contains(&id) {
                            provider_order.push(id.clone());
                        }
                        merge_prediction_provider(
                            &mut providers,
                            id,
                            name,
                            private,
                            provider,
                        );
                    }
                }
            } else if let Some((id, name, provider)) =
                prediction_provider(ai, fallback_provider)
            {
                merge_prediction_provider(
                    &mut providers,
                    id,
                    name,
                    private,
                    provider,
                );
            }
        }

        let defaults = Self::default();
        let active_provider_id = active_connection.as_ref().and_then(|active| {
            let id = providers.iter().find_map(|(id, (name, _, _))| {
                (id == active || name == active).then_some(id)
            })?;
            Some(id.as_str())
        });
        let selected_provider_id = model_context
            .filter(|context| !context.model_id.trim().is_empty())
            .map(|context| context.provider_id.as_str())
            .filter(|provider_id| providers.contains_key(*provider_id));
        let provider_id =
            selected_provider_id.or(active_provider_id).or_else(|| {
                provider_order
                    .iter()
                    .find(|id| providers.contains_key(*id))
                    .map(String::as_str)
            });
        let Some((provider_id, (_, _, provider))) =
            provider_id.and_then(|id| providers.get_key_value(id))
        else {
            return Ok(defaults);
        };
        let mut provider = provider.clone();
        provider.provider_id = Some(provider_id.clone());
        if let Some(model_context) = model_context.filter(|context| {
            context.provider_id.as_str() == provider_id.as_str()
                && !context.model_id.trim().is_empty()
        }) {
            provider.model = model_context.model_id.clone();
        }
        Ok(provider)
    }
}

fn provider_id_for(name: &str, fallback: &str) -> String {
    let mut id = String::from("ahead-");
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            id.push(character.to_ascii_lowercase());
        } else if !id.ends_with('-') {
            id.push('-');
        }
    }
    while id.ends_with('-') {
        id.pop();
    }
    if id == "ahead" {
        format!("ahead-{}", fallback.trim_matches('-'))
    } else {
        id
    }
}

fn prediction_provider(
    value: &Value,
    fallback_provider: &str,
) -> Option<(String, String, PredictionProviderConfig)> {
    let table = value.as_object()?;
    let name = table
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Configured model")
        .trim();
    let base_url = table.get("base_url").and_then(Value::as_str)?.trim();
    let model = table
        .get("model")
        .and_then(Value::as_str)
        .or_else(|| {
            table
                .get("models")
                .and_then(Value::as_array)
                .and_then(|models| models.first())
                .and_then(Value::as_str)
        })
        .map(str::trim)?;
    if name.is_empty() || base_url.is_empty() || model.is_empty() {
        return None;
    }
    let id = table
        .get("provider_id")
        .or_else(|| table.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| provider_id_for(name, fallback_provider));
    let provider = table
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or(fallback_provider)
        .trim()
        .to_string();
    let api_key = table
        .get("api_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|api_key| !api_key.is_empty())
        .map(str::to_string);
    Some((
        id.clone(),
        name.to_string(),
        PredictionProviderConfig {
            provider_id: Some(id.clone()),
            provider,
            base_url: base_url.to_string(),
            api_key,
            model: model.to_string(),
        },
    ))
}

fn merge_prediction_provider(
    providers: &mut BTreeMap<String, (String, bool, PredictionProviderConfig)>,
    id: String,
    name: String,
    private: bool,
    mut provider: PredictionProviderConfig,
) {
    if let Some((_, previous_private, previous)) = providers.get(&id) {
        if provider.api_key.is_none()
            && (*previous_private || private)
            && provider.base_url == previous.base_url
        {
            provider.api_key = previous.api_key.clone();
        }
    }
    providers.insert(id, (name, private, provider));
}

fn read_toml(root: &Path, filename: &str) -> Result<Option<Value>> {
    let Some(text) = ahead_core::config::read_ahead_config(root, filename)
        .with_context(|| {
            format!("AHEAD provider layer `{filename}` could not be read")
        })?
    else {
        return Ok(None);
    };
    let table: toml::Table = text.parse().map_err(|_| {
        anyhow::anyhow!("AHEAD provider layer `{filename}` contains invalid TOML")
    })?;
    Ok(Some(serde_json::to_value(table)?))
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
        Ok(format!(
            "{}/completions",
            url.to_string().trim_end_matches('/')
        ))
    }
}

pub struct PredictionEngine;

impl PredictionEngine {
    pub(crate) fn validate_context_budget(
        work: &PredictionWorkContext,
        request: &PredictionRequest,
        open_buffers: &[OpenBufferContext],
    ) -> Result<()> {
        let mut fields = std::iter::once(work.phase_title.as_str())
            .chain(work.primary_issue.as_deref())
            .chain(work.active_invariants.iter().map(String::as_str))
            .chain([request.path.as_str(), request.work_context.as_str()])
            .chain(open_buffers.iter().flat_map(|buffer| {
                [buffer.path.as_str(), buffer.relevant_excerpt.as_str()]
            }));
        let bytes =
            fields.try_fold(0usize, |bytes, field| bytes.checked_add(field.len()));
        anyhow::ensure!(
            bytes.is_some_and(|bytes| bytes <= MAX_PREDICTION_CONTEXT_BYTES),
            "Prediction session, instructions and related buffers exceed the 64 KiB context budget; no required context was dropped"
        );
        Ok(())
    }

    pub(crate) fn model_fim_context_fits(
        work: &PredictionWorkContext,
        request: &PredictionRequest,
        open_buffers: &[OpenBufferContext],
        provider: &PredictionProviderConfig,
        model_context: Option<&PredictionModelContext>,
    ) -> Result<bool> {
        let Some(format) = infer_prompt_format(&provider.model) else {
            return Ok(true);
        };
        let header = session_fim_header(
            work,
            comment_prefix_for_path(&request.path),
            &request.work_context,
            open_buffers,
        );
        let budget = model_aware_excerpt_budget_for_header(
            provider,
            model_context,
            &header,
            format,
        )?;
        Ok(match budget {
            Some(budget) => budget > 0,
            None => true,
        })
    }

    fn validate_prompt_budget(prompt: &str) -> Result<()> {
        anyhow::ensure!(
            prompt.len() <= MAX_PREDICTION_PROMPT_BYTES,
            "Prediction prompt exceeds the 128 KiB budget; no required context was dropped"
        );
        Ok(())
    }

    /// Assembles full context prompt for fast edit prediction
    pub fn assemble_context(
        work: &PredictionWorkContext,
        request: &PredictionRequest,
        open_buffers: &[OpenBufferContext],
    ) -> Result<String> {
        if matches!(work.mode, AssistanceMode::Learn) {
            bail!("Predictions are disabled in Learn mode");
        }

        Self::validate_context_budget(work, request, open_buffers)?;

        let mut prompt = String::new();
        prompt.push_str("# AHEAD Prediction Context\n");
        let task_intent = match work.mode {
            AssistanceMode::Learn => "Teaching",
            AssistanceMode::Assist => "Assistance",
        };
        prompt.push_str(&format!("Task intent: {task_intent}\n"));
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

        let (prefix, suffix) = prediction_excerpt(request);
        prompt.push_str(&format!("\n# Active Buffer: {}\n", request.path));
        prompt.push_str(&format!("Prefix:\n{prefix}\n"));
        prompt.push_str("<CURSOR>\n");
        prompt.push_str(&format!("Suffix:\n{suffix}\n"));

        Self::validate_prompt_budget(&prompt)?;
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
        Self::predict_cancellable(
            work,
            request,
            open_buffers,
            provider,
            &PredictionCancellation::default(),
        )
    }

    pub(crate) fn predict_cancellable(
        work: &PredictionWorkContext,
        request: &PredictionRequest,
        open_buffers: &[OpenBufferContext],
        provider: &PredictionProviderConfig,
        cancellation: &PredictionCancellation,
    ) -> Result<PredictionResult> {
        Self::predict_cancellable_with_model_context(
            work,
            request,
            open_buffers,
            provider,
            None,
            cancellation,
        )
    }

    pub(crate) fn predict_cancellable_with_model_context(
        work: &PredictionWorkContext,
        request: &PredictionRequest,
        open_buffers: &[OpenBufferContext],
        provider: &PredictionProviderConfig,
        model_context: Option<&PredictionModelContext>,
        cancellation: &PredictionCancellation,
    ) -> Result<PredictionResult> {
        if cancellation.is_cancelled() {
            bail!("prediction cancelled");
        }
        if matches!(work.mode, AssistanceMode::Learn) {
            bail!("Predictions are disabled in Learn mode");
        }
        if provider.provider.eq_ignore_ascii_case("anthropic") {
            bail!(
                "Anthropic direct does not expose the configured FIM completion route"
            );
        }
        if provider.model.trim().is_empty() {
            bail!("prediction provider model is empty");
        }

        Self::validate_context_budget(work, request, open_buffers)?;

        if infer_prompt_format(&provider.model).is_some() {
            return Self::predict_fim(
                work,
                request,
                open_buffers,
                provider,
                model_context,
                cancellation,
            );
        }

        let prompt = Self::assemble_context(work, request, open_buffers)?;
        let (_, suffix) = prediction_excerpt(request);
        let endpoint = completion_endpoint(&provider.base_url)?;
        let body = json!({
            "model": provider.model,
            "prompt": prompt,
            "suffix": suffix,
            "max_tokens": 128,
            "temperature": 0.2,
            "stream": false
        });
        let payload =
            post_prediction_json(provider, &endpoint, &body, cancellation)?;
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
        open_buffers: &[OpenBufferContext],
        provider: &PredictionProviderConfig,
        model_context: Option<&PredictionModelContext>,
        cancellation: &PredictionCancellation,
    ) -> Result<PredictionResult> {
        let format = infer_prompt_format(&provider.model)
            .context("no FIM prompt format for model")?;
        let header = session_fim_header(
            work,
            comment_prefix_for_path(&request.path),
            &request.work_context,
            open_buffers,
        );
        let excerpt_budget =
            model_aware_excerpt_budget(provider, model_context, &header, format)?;
        let (prefix, suffix) =
            prediction_excerpt_with_budget(request, excerpt_budget);

        let mut prompt = header;
        prompt.push_str(&format_fim_prompt(format, &prefix, &suffix));
        Self::validate_prompt_budget(&prompt)?;
        let stop = fim_stop_tokens();

        let (replacement, _request_id) =
            if provider.provider.eq_ignore_ascii_case("ollama") {
                Self::post_ollama_generate(provider, &prompt, &stop, cancellation)?
            } else {
                Self::post_completions(provider, &prompt, &stop, cancellation)?
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
        cancellation: &PredictionCancellation,
    ) -> Result<(String, String)> {
        let base = provider.base_url.trim_end_matches('/');
        let base = base.strip_suffix("/v1").unwrap_or(base);
        let endpoint = format!("{base}/api/generate");
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
        let payload =
            post_prediction_json(provider, &endpoint, &body, cancellation)?;
        let text = extract_completion_text(&payload).unwrap_or_default();
        Ok((text.to_string(), String::new()))
    }

    /// OpenAI-compatible `/completions` route with stop tokens.
    fn post_completions(
        provider: &PredictionProviderConfig,
        prompt: &str,
        stop: &[String],
        cancellation: &PredictionCancellation,
    ) -> Result<(String, String)> {
        let endpoint = completion_endpoint(&provider.base_url)?;
        let body = json!({
            "model": provider.model,
            "prompt": prompt,
            "max_tokens": 128,
            "temperature": 0.2,
            "stop": stop,
            "stream": false
        });
        let payload =
            post_prediction_json(provider, &endpoint, &body, cancellation)?;
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

fn post_prediction_json(
    provider: &PredictionProviderConfig,
    endpoint: &str,
    body: &Value,
    cancellation: &PredictionCancellation,
) -> Result<Value> {
    let endpoint_url =
        reqwest::Url::parse(endpoint).context("invalid prediction URL")?;
    let mut builder =
        reqwest::Client::builder().timeout(std::time::Duration::from_secs(5));
    if endpoint_url
        .host_str()
        .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"))
    {
        builder = builder.no_proxy();
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build prediction executor")?;
    runtime.block_on(async {
        let client = builder.build().context("build prediction provider client")?;
        let mut http = client.post(endpoint).json(body);
        if let Some(api_key) = provider.api_key.as_deref() {
            http = http.bearer_auth(api_key);
        }
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => bail!("prediction cancelled"),
            payload = async {
                let response = http.send().await.context("send prediction request")?;
                if !response.status().is_success() {
                    bail!("prediction provider returned HTTP {}", response.status());
                }
                response.json::<Value>().await.context("decode prediction provider response")
            } => payload,
        }
    })
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
    bytes / BYTES_PER_TOKEN_GUESS
}

const BYTES_PER_TOKEN_GUESS: usize = 3;

/// Total excerpt budget in tokens, mirroring Zed's
/// `CURSOR_EXCERPT_TOKEN_BUDGET`.
pub const MAX_EXCERPT_TOKENS: usize = 8192;

pub(crate) fn prediction_excerpt(request: &PredictionRequest) -> (String, String) {
    prediction_excerpt_with_budget(request, MAX_EXCERPT_TOKENS)
}

fn prediction_excerpt_with_budget(
    request: &PredictionRequest,
    budget_tokens: usize,
) -> (String, String) {
    let full = format!("{}{}", request.prefix, request.suffix);
    let cursor = request.prefix.len().min(full.len());
    let budget_bytes = budget_tokens
        .min(MAX_EXCERPT_TOKENS)
        .saturating_mul(BYTES_PER_TOKEN_GUESS);
    let (start, end) = cursor_excerpt_bounds(&full, cursor, budget_tokens);
    let Some(before) = full.get(start..cursor) else {
        return (String::new(), String::new());
    };
    let Some(after) = full.get(cursor..end) else {
        return (String::new(), String::new());
    };

    let mut before_bytes = budget_bytes / 2;
    let mut after_bytes = budget_bytes.saturating_sub(before_bytes);
    before_bytes = before_bytes.min(before.len());
    after_bytes = after_bytes.min(after.len());
    let remaining = budget_bytes
        .saturating_sub(before_bytes)
        .saturating_sub(after_bytes);
    before_bytes = before.len().min(before_bytes.saturating_add(remaining));
    let remaining = budget_bytes
        .saturating_sub(before_bytes)
        .saturating_sub(after_bytes);
    after_bytes = after.len().min(after_bytes.saturating_add(remaining));

    let mut before_start = before.len().saturating_sub(before_bytes);
    while !before.is_char_boundary(before_start) {
        before_start += 1;
    }
    let mut after_end = after_bytes.min(after.len());
    while !after.is_char_boundary(after_end) {
        after_end -= 1;
    }
    (
        before[before_start..].to_string(),
        after[..after_end].to_string(),
    )
}

fn model_aware_excerpt_budget(
    provider: &PredictionProviderConfig,
    model_context: Option<&PredictionModelContext>,
    header: &str,
    format: PromptFormat,
) -> Result<usize> {
    let Some(excerpt_budget) = model_aware_excerpt_budget_for_header(
        provider,
        model_context,
        header,
        format,
    )?
    else {
        return Ok(MAX_EXCERPT_TOKENS);
    };
    anyhow::ensure!(
        excerpt_budget > 0,
        "Required FIM session context exceeds the safe input budget for the selected model; no constraints were dropped"
    );
    Ok(excerpt_budget.min(MAX_EXCERPT_TOKENS))
}

fn model_aware_excerpt_budget_for_header(
    provider: &PredictionProviderConfig,
    model_context: Option<&PredictionModelContext>,
    header: &str,
    format: PromptFormat,
) -> Result<Option<usize>> {
    let matching_context = model_context.filter(|context| {
        provider.provider_id.as_deref() == Some(context.provider_id.as_str())
            && provider.model.as_str() == context.model_id.as_str()
    });
    let Some(context_window) =
        matching_context.and_then(|context| context.context_window)
    else {
        return Ok(None);
    };
    anyhow::ensure!(context_window > 0, "model context window must be positive");

    let tags = format_fim_prompt(format, "", "");
    let fixed_prompt_tokens = guess_token_count(header.len() + tags.len());
    let safe_prompt_budget = (context_window as usize).saturating_mul(4) / 5;
    let reserved_completion_tokens = 128;
    Ok(Some(safe_prompt_budget.saturating_sub(
        fixed_prompt_tokens + reserved_completion_tokens,
    )))
}

pub(crate) fn prediction_excerpt_for_snapshot(
    snapshot: &PredictionContextSnapshot,
    open_buffers: &[OpenBufferContext],
) -> Result<(String, String)> {
    let Some(format) = infer_prompt_format(&snapshot.provider.model) else {
        return Ok(prediction_excerpt(&snapshot.request));
    };
    let header = session_fim_header(
        &snapshot.work,
        comment_prefix_for_path(&snapshot.request.path),
        &snapshot.request.work_context,
        open_buffers,
    );
    let budget = model_aware_excerpt_budget(
        &snapshot.provider,
        snapshot.model_context.as_ref(),
        &header,
        format,
    )?;
    Ok(prediction_excerpt_with_budget(&snapshot.request, budget))
}

/// Computes a linewise excerpt window around the cursor that fits the token
/// budget, mirroring Zed's `compute_cursor_excerpt` (symmetric expansion,
/// down first then up). Returns byte offsets into `text`; inputs are
/// clamped to valid boundaries so arbitrary offsets cannot panic.
pub fn cursor_excerpt_bounds(
    text: &str,
    cursor: usize,
    budget_tokens: usize,
) -> (usize, usize) {
    let mut cursor = cursor.min(text.len());
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let line_starts = line_start_offsets(text);
    if line_starts.is_empty() {
        return (0, 0);
    }
    let cursor_row = line_starts
        .iter()
        .rposition(|&start| start <= cursor)
        .unwrap_or(0);
    let mut budget = budget_tokens.saturating_sub(line_token_count(
        &line_starts,
        text.len(),
        cursor_row,
    ));
    let mut start_row = cursor_row;
    let mut end_row = cursor_row;
    loop {
        let can_up = start_row > 0;
        let can_down = end_row + 1 < line_starts.len();
        if budget == 0 || (!can_up && !can_down) {
            break;
        }
        if can_down {
            let cost = line_token_count(&line_starts, text.len(), end_row + 1);
            if cost <= budget {
                end_row += 1;
                budget = budget.saturating_sub(cost);
            } else {
                break;
            }
        }
        if can_up && budget > 0 {
            let cost = line_token_count(&line_starts, text.len(), start_row - 1);
            if cost <= budget {
                start_row -= 1;
                budget = budget.saturating_sub(cost);
            } else {
                break;
            }
        }
    }
    let start = line_starts[start_row];
    let end = line_starts.get(end_row + 1).copied().unwrap_or(text.len());
    let max_bytes = budget_tokens.saturating_mul(BYTES_PER_TOKEN_GUESS);
    if end - start <= max_bytes {
        return (start, end);
    }

    // A single minified line can exceed the entire linewise budget.
    let mut clipped_start = cursor.saturating_sub(max_bytes / 2).max(start);
    let mut clipped_end = clipped_start.saturating_add(max_bytes).min(end);
    clipped_start = clipped_end.saturating_sub(max_bytes).max(start);
    while !text.is_char_boundary(clipped_start) {
        clipped_start += 1;
    }
    while !text.is_char_boundary(clipped_end) {
        clipped_end -= 1;
    }
    (clipped_start, clipped_end)
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

fn line_token_count(starts: &[usize], text_len: usize, row: usize) -> usize {
    let Some(&start) = starts.get(row) else {
        return 1;
    };
    let end = starts.get(row + 1).copied().unwrap_or(text_len);
    guess_token_count(end.saturating_sub(start)).max(1)
}

/// Comment prefix for rendering the session header inside FIM prompts.
/// Returns `None` for languages without a clear line-comment style, in
/// which case the header is omitted rather than corrupting the hole.
fn comment_prefix_for_path(path: &str) -> Option<&'static str> {
    let extension = Path::new(path).extension()?.to_str()?;
    match extension {
        "rs" | "js" | "jsx" | "ts" | "tsx" | "go" | "java" | "c" | "h" | "cpp"
        | "hpp" | "swift" | "kt" | "kts" | "scala" | "cs" | "php" => Some("//"),
        "py" | "sh" | "bash" | "rb" | "toml" | "yaml" | "yml" | "r" | "pl" => {
            Some("#")
        }
        "lua" | "sql" | "hs" => Some("--"),
        _ => None,
    }
}

/// Renders the AHEAD work-session context as compact leading comment lines
/// for FIM prompts: phase and all supplied invariants. This is the AHEAD
/// addition on top of Zed's pure prefix/suffix hole.
fn session_fim_header(
    work: &PredictionWorkContext,
    comment: Option<&str>,
    request_context: &str,
    open_buffers: &[OpenBufferContext],
) -> String {
    let mut context = format!(
        "AHEAD {:?} · {} · {}\n",
        work.mode,
        work.work_kind.display_name(),
        work.phase_title
    );
    if let Some(issue) = &work.primary_issue {
        context.push_str(&format!("Issue: {issue}\n"));
    }
    let invariants: Vec<&str> =
        work.active_invariants.iter().map(String::as_str).collect();
    if !invariants.is_empty() {
        context.push_str(&format!("Invariants: {}\n", invariants.join("; ")));
    }
    if !request_context.trim().is_empty() {
        context.push_str("Session and applicable instructions:\n");
        context.push_str(request_context.trim());
        context.push('\n');
    }
    for buffer in open_buffers {
        context.push_str(&format!(
            "Relevant open buffer: {}\n{}\n",
            buffer.path, buffer.relevant_excerpt
        ));
    }

    if let Some(comment) = comment {
        let mut rendered = String::new();
        for line in context.lines() {
            rendered.push_str(comment);
            rendered.push(' ');
            rendered.push_str(line);
            rendered.push('\n');
        }
        rendered
    } else {
        format!("[AHEAD FIM context]\n{context}[/AHEAD FIM context]\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ahead_rpc::ahead::DisplayPosition;

    #[test]
    fn prediction_budget_preserves_constraints_and_rejects_oversized_prompts()
    -> Result<()> {
        let work = PredictionWorkContext {
            work_kind: WorkKind::ProductChange,
            mode: AssistanceMode::Assist,
            phase_title: "Implementation".into(),
            primary_issue: None,
            active_invariants: (0..8)
                .map(|index| format!("Keep invariant {index}"))
                .collect(),
        };
        let mut request = PredictionRequest {
            request_id: "budget".into(),
            session_id: None,
            path: "main.py".into(),
            cursor: DisplayPosition { line: 0, col: 0 },
            prefix: String::new(),
            suffix: String::new(),
            work_context: "Decision: preserve the public contract".into(),
        };
        let prompt = PredictionEngine::assemble_context(&work, &request, &[])?;
        let header =
            session_fim_header(&work, Some("#"), &request.work_context, &[]);
        for invariant in &work.active_invariants {
            assert!(prompt.contains(invariant));
            assert!(header.contains(invariant));
        }
        let buffers = [OpenBufferContext {
            path: "related.py".into(),
            relevant_excerpt: "日本語".repeat(MAX_PREDICTION_CONTEXT_BYTES / 8),
        }];
        assert!(
            PredictionEngine::assemble_context(&work, &request, &buffers)
                .unwrap_err()
                .to_string()
                .contains("64 KiB context budget")
        );

        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let mut provider = PredictionProviderConfig {
            provider_id: None,
            provider: "openai-compatible".into(),
            base_url: format!("http://{}/v1", listener.local_addr()?),
            api_key: None,
            model: "generic-completion".into(),
        };
        request.work_context = "x".repeat(MAX_PREDICTION_CONTEXT_BYTES + 1);
        for model in ["generic-completion", "qwen2.5-coder:7b"] {
            provider.model = model.into();
            let error = PredictionEngine::predict(&work, &request, &[], &provider)
                .unwrap_err();
            assert!(error.to_string().contains("64 KiB context budget"));
        }
        request.work_context =
            format!("Required decision\n{}", "\n".repeat(60 * 1024));
        request.work_context.push_str("Required decision");
        let error =
            PredictionEngine::predict(&work, &request, &[], &provider).unwrap_err();
        assert!(error.to_string().contains("128 KiB budget"), "{error}");
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        Ok(())
    }

    #[test]
    fn selected_model_context_bounds_the_excerpt_without_dropping_session_context()
    -> Result<()> {
        let work = PredictionWorkContext {
            work_kind: WorkKind::ProductChange,
            mode: AssistanceMode::Assist,
            phase_title: "Implement".into(),
            primary_issue: None,
            active_invariants: vec!["Preserve the public contract".into()],
        };
        let request = PredictionRequest {
            request_id: "model-budget".into(),
            session_id: Some("session-1".into()),
            path: "src/main.rs".into(),
            cursor: DisplayPosition { line: 0, col: 0 },
            prefix: format!("{}CURSOR", "a".repeat(10_000)),
            suffix: format!("CURSOR{}", "b".repeat(10_000)),
            work_context: "Active plan step: keep the behavior stable".into(),
        };
        let provider = PredictionProviderConfig {
            provider_id: Some("hosted".into()),
            provider: "openai-compatible".into(),
            base_url: "http://localhost".into(),
            api_key: None,
            model: "qwen2.5-coder:7b".into(),
        };
        let model_context = PredictionModelContext {
            provider_id: "hosted".into(),
            model_id: "qwen2.5-coder:7b".into(),
            context_window: Some(512),
        };
        let header =
            session_fim_header(&work, Some("//"), &request.work_context, &[]);
        let budget = model_aware_excerpt_budget(
            &provider,
            Some(&model_context),
            &header,
            PromptFormat::Qwen,
        )?;
        let (prefix, suffix) = prediction_excerpt_with_budget(&request, budget);
        assert!(budget < MAX_EXCERPT_TOKENS);
        assert!(prefix.ends_with("CURSOR"));
        assert!(suffix.starts_with("CURSOR"));
        assert!(prefix.len() + suffix.len() <= budget * BYTES_PER_TOKEN_GUESS);
        assert!(header.contains("Preserve the public contract"));
        assert!(header.contains("Active plan step: keep the behavior stable"));
        let large_session = PredictionRequest {
            work_context: "Prior plan step and progress. ".repeat(500),
            ..request.clone()
        };
        assert!(!PredictionEngine::model_fim_context_fits(
            &work,
            &large_session,
            &[],
            &provider,
            Some(&model_context),
        )?);
        let compact_session = PredictionRequest {
            work_context: "Goal: preserve the public contract. Plan: 12/20 complete; active: update retry path."
                .to_string(),
            ..request.clone()
        };
        assert!(PredictionEngine::model_fim_context_fits(
            &work,
            &compact_session,
            &[],
            &provider,
            Some(&model_context),
        )?);

        let unrelated_provider = PredictionProviderConfig {
            provider_id: Some("other".into()),
            ..provider.clone()
        };
        assert_eq!(
            model_aware_excerpt_budget(
                &unrelated_provider,
                Some(&model_context),
                &header,
                PromptFormat::Qwen,
            )?,
            MAX_EXCERPT_TOKENS
        );
        Ok(())
    }

    #[test]
    fn model_context_smaller_than_required_fim_header_fails_explicitly() {
        let provider = PredictionProviderConfig {
            provider_id: Some("local".into()),
            provider: "openai-compatible".into(),
            base_url: "http://localhost".into(),
            api_key: None,
            model: "qwen2.5-coder".into(),
        };
        let model_context = PredictionModelContext {
            provider_id: "local".into(),
            model_id: "qwen2.5-coder".into(),
            context_window: Some(128),
        };
        let error = model_aware_excerpt_budget(
            &provider,
            Some(&model_context),
            &"required invariant ".repeat(64),
            PromptFormat::Qwen,
        )
        .expect_err("required session context must not be truncated");
        assert!(error.to_string().contains("Required FIM session context"));
    }

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
                if let Some(header_end) =
                    request.windows(4).position(|window| window == b"\r\n\r\n")
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
            session_id: Some("session-1".to_string()),
            path: "src/retry.rs".to_string(),
            cursor: DisplayPosition { line: 4, col: 7 },
            prefix: "pub fn retry".to_string(),
            suffix: " {\n}".to_string(),
            work_context: "Active step: implement the retry policy".to_string(),
        };
        let provider = PredictionProviderConfig {
            provider_id: None,
            provider: "openai-compatible".to_string(),
            base_url: format!("http://{address}/v1"),
            api_key: None,
            model: "test-fim".to_string(),
        };

        let result =
            PredictionEngine::predict(&work, &request, &[], &provider).unwrap();
        let body = server.join().unwrap();
        assert_eq!(result.replacement, "retry_with_backoff()");
        assert!(body.contains("Preserve idempotency"));
        assert!(body.contains("Active step: implement the retry policy"));
        assert!(body.contains("pub fn retry"));
        assert!(body.contains("\"suffix\":\" {\\n}\""));
    }

    #[test]
    fn generic_prediction_sends_only_the_bounded_utf8_suffix() {
        let (address, server) = spawn_stub(r#"{"choices":[{"text":"ok"}]}"#);
        let work = PredictionWorkContext {
            work_kind: WorkKind::ProductChange,
            mode: AssistanceMode::Assist,
            phase_title: "Implement".to_string(),
            primary_issue: None,
            active_invariants: Vec::new(),
        };
        let suffix = "é".repeat(40_000);
        let request = PredictionRequest {
            request_id: "bounded-suffix".to_string(),
            session_id: None,
            path: "src/main.rs".to_string(),
            cursor: DisplayPosition { line: 0, col: 11 },
            prefix: "fn main() {".to_string(),
            suffix: suffix.clone(),
            work_context: String::new(),
        };
        let provider = PredictionProviderConfig {
            provider_id: None,
            provider: "openai-compatible".to_string(),
            base_url: format!("http://{address}/v1"),
            api_key: None,
            model: "unknown-fim-format".to_string(),
        };

        let result = PredictionEngine::predict(&work, &request, &[], &provider)
            .expect("bounded generic prediction");
        let body: Value = serde_json::from_str(&server.join().unwrap()).unwrap();
        let sent_suffix = body["suffix"].as_str().unwrap();
        let prompt = body["prompt"].as_str().unwrap();

        assert_eq!(result.replacement, "ok");
        assert!(sent_suffix.len() <= MAX_EXCERPT_TOKENS * BYTES_PER_TOKEN_GUESS);
        assert!(sent_suffix.len() < suffix.len());
        assert!(prompt.ends_with(&format!("Suffix:\n{sent_suffix}\n")));
    }

    #[test]
    fn fim_provider_request_includes_project_instructions_session_and_open_buffers()
    {
        let (address, server) =
            spawn_stub(r#"{"choices":[{"text":"retry_with_backoff()"}]}"#);
        let work = PredictionWorkContext {
            work_kind: WorkKind::ProductChange,
            mode: AssistanceMode::Assist,
            phase_title: "Implement retry policy".to_string(),
            primary_issue: Some("#142 Improve retries".to_string()),
            active_invariants: vec!["Preserve idempotency".to_string()],
        };
        let request = PredictionRequest {
            request_id: "fim-context-1".to_string(),
            session_id: Some("session-1".to_string()),
            path: "src/retry.rs".to_string(),
            cursor: DisplayPosition { line: 4, col: 7 },
            prefix: "pub fn retry".to_string(),
            suffix: " {\n}".to_string(),
            work_context:
                "root policy\nnested source policy\nDecision: preserve retries"
                    .to_string(),
        };
        let open_buffers = vec![OpenBufferContext {
            path: "src/client.rs".to_string(),
            relevant_excerpt: "pub struct UnsavedRequest;".to_string(),
        }];
        let provider = PredictionProviderConfig {
            provider_id: None,
            provider: "openai-compatible".to_string(),
            base_url: format!("http://{address}/v1"),
            api_key: None,
            model: "qwen2.5-coder:7b".to_string(),
        };

        let result =
            PredictionEngine::predict(&work, &request, &open_buffers, &provider)
                .expect("FIM provider request");
        let body: Value = serde_json::from_str(&server.join().unwrap())
            .expect("provider JSON request body");
        let prompt = body["prompt"].as_str().expect("FIM prompt");

        assert_eq!(result.replacement, "retry_with_backoff()");
        assert!(prompt.contains("// root policy"));
        assert!(prompt.contains("// nested source policy"));
        assert!(prompt.contains("// Decision: preserve retries"));
        assert!(prompt.contains("// Relevant open buffer: src/client.rs"));
        assert!(prompt.contains("// pub struct UnsavedRequest;"));
        assert!(prompt.contains("<|fim_prefix|>pub fn retry"));
    }

    #[test]
    fn host_fim_request_sends_persisted_objective_and_live_context() -> Result<()> {
        use ahead_rpc::ahead::AheadRequest;

        let temporary = tempfile::tempdir()?;
        let workspace = temporary.path();
        std::fs::create_dir_all(workspace.join(".ahead"))?;
        std::fs::create_dir_all(workspace.join("src"))?;
        std::fs::create_dir_all(workspace.join("docs"))?;
        std::fs::create_dir_all(workspace.join("unrelated"))?;
        std::fs::write(
            workspace.join("AGENTS.md"),
            "Workspace instruction: preserve public retry semantics.",
        )?;
        std::fs::write(
            workspace.join("src/AGENTS.md"),
            "Nested instruction: keep retry delays bounded.",
        )?;
        std::fs::write(
            workspace.join("docs/AGENTS.md"),
            "Docs instruction: use stable public examples.",
        )?;
        std::fs::write(
            workspace.join("unrelated/AGENTS.md"),
            "Unrelated instruction must not enter this prediction.",
        )?;

        let (address, server) =
            spawn_stub(r#"{"choices":[{"text":"retry_with_backoff()"}]}"#);
        let connection_name = format!("ahead-fim-test-{}", uuid::Uuid::new_v4());
        std::fs::write(
            workspace.join(".ahead/settings.toml"),
            format!(
                r#"
                [ai]
                active_connection = "{connection_name}"

                [[ai.connections]]
                name = "{connection_name}"
                provider_id = "{connection_name}"
                provider = "openai-compatible"
                base_url = "http://{address}/v1"
                model = "qwen2.5-coder:7b"
                "#
            ),
        )?;

        let database_path = workspace.join(".ahead/session.db");
        let host = crate::ahead::host::AheadSessionHost::new(
            crate::ahead::store::SessionStore::open(&database_path)?,
        );
        host.set_workspace(workspace.to_path_buf());
        let objective =
            "Add bounded exponential backoff without retrying committed responses.";
        let session = host.start_work(
            Some(WorkKind::ProductChange),
            "Safe retries".to_string(),
            objective.to_string(),
            None,
        )?;
        let item = host.handle_request(AheadRequest::WorkItemCreate {
            session_id: session.session.id.clone(),
            title: "Add bounded exponential backoff".to_string(),
        })?;
        let item_id = item["id"].as_str().context("work item id")?.to_string();
        host.handle_request(AheadRequest::WorkItemNote {
            item_id: item_id.clone(),
            kind: "invariant".to_string(),
            body_markdown: "Preserve retry idempotency.".to_string(),
        })?;
        host.handle_request(AheadRequest::WorkItemNote {
            item_id,
            kind: "decision".to_string(),
            body_markdown: "Do not retry a committed response.".to_string(),
        })?;
        host.handle_request(AheadRequest::ConversationSummarize {
            session_id: session.session.id.clone(),
            phase: "plan".to_string(),
            summary_markdown: "Retry only when no response has been committed."
                .to_string(),
            message_id_range: "message-1..message-2".to_string(),
        })?;
        drop(host);
        let store = crate::ahead::store::SessionStore::open(&database_path)?;
        store.set_agent_runtime_state(
            &session.session.id,
            &ahead_rpc::ahead::AgentRuntimeState {
                plan: vec![ahead_rpc::ahead::AgentPlanEntry {
                    content: "Keep each retry delay bounded".to_string(),
                    status: "in_progress".to_string(),
                    priority: "high".to_string(),
                }],
                ..Default::default()
            },
        )?;
        drop(store);

        let host = crate::ahead::host::AheadSessionHost::new(
            crate::ahead::store::SessionStore::open(&database_path)?,
        );
        host.set_workspace(workspace.to_path_buf());
        let open_buffers = [
            OpenBufferContext {
                path: "src/client.rs".to_string(),
                relevant_excerpt: "pub struct UnsavedRequest;".to_string(),
            },
            OpenBufferContext {
                path: "docs/guide.md".to_string(),
                relevant_excerpt: "Retry examples in progress.".to_string(),
            },
        ];
        let result = host.request_prediction(
            PredictionRequest {
                request_id: "host-fim-1".to_string(),
                session_id: Some(session.session.id),
                path: "src/retry.rs".to_string(),
                cursor: DisplayPosition { line: 4, col: 7 },
                prefix: "pub fn retry".to_string(),
                suffix: " {\n}".to_string(),
                work_context:
                    "Caller context: preserve the existing request identity."
                        .to_string(),
            },
            &open_buffers,
        )?;
        let body: Value =
            serde_json::from_str(&server.join().expect("provider stub thread"))?;
        let prompt = body["prompt"].as_str().context("provider prompt")?;
        let objective_line = format!("Objective: {objective}");

        assert_eq!(result.replacement, "retry_with_backoff()");
        for expected in [
            "Session: Safe retries",
            objective_line.as_str(),
            "Product Change · Questions & Outline",
            "// Current agent plan:\n// - [in_progress] Keep each retry delay bounded",
            "Workspace instruction: preserve public retry semantics.",
            "Nested instruction: keep retry delays bounded.",
            "Docs instruction: use stable public examples.",
            "Add bounded exponential backoff",
            "Preserve retry idempotency.",
            "Do not retry a committed response.",
            "Retry only when no response has been committed.",
            "Caller context: preserve the existing request identity.",
            "Relevant open buffer: src/client.rs",
            "pub struct UnsavedRequest;",
            "Relevant open buffer: docs/guide.md",
            "Retry examples in progress.",
            "pub fn retry",
        ] {
            assert!(
                prompt.contains(expected),
                "provider prompt omitted: {expected}"
            );
        }
        assert!(
            !prompt
                .contains("Unrelated instruction must not enter this prediction.")
        );
        Ok(())
    }

    #[test]
    fn host_fim_follows_the_selected_session_and_refreshes_its_decisions()
    -> Result<()> {
        use ahead_rpc::ahead::AheadRequest;
        let temporary = tempfile::tempdir()?;
        let workspace = temporary.path();
        std::fs::create_dir(workspace.join(".ahead"))?;
        let host = crate::ahead::host::AheadSessionHost::in_memory()?;
        host.set_workspace(workspace.to_path_buf());
        let first = host.start_work(
            Some(WorkKind::ProductChange),
            "FIRST_SESSION".into(),
            "FIRST_OBJECTIVE".into(),
            None,
        )?;
        let second = host.start_work(
            Some(WorkKind::ProductChange),
            "SECOND_SESSION".into(),
            "SECOND_OBJECTIVE".into(),
            None,
        )?;
        let item = host.handle_request(AheadRequest::WorkItemCreate {
            session_id: first.session.id.clone(),
            title: "Retry policy".into(),
        })?;
        let item_id = item["id"].as_str().context("work item id")?.to_string();
        let notifications = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = notifications.clone();
        host.set_notification_sink(std::sync::Arc::new(move |notification| {
            captured.lock().unwrap().push(notification)
        }));
        let connection = format!("fim-session-test-{}", uuid::Uuid::new_v4());
        for (session_id, expected, excluded, decision) in [
            (
                Some(first.session.id.as_str()),
                "FIRST_OBJECTIVE",
                "SECOND_OBJECTIVE",
                "INITIAL_DECISION",
            ),
            (
                Some(first.session.id.as_str()),
                "FIRST_OBJECTIVE",
                "SECOND_OBJECTIVE",
                "REVISED_DECISION",
            ),
            (
                Some(second.session.id.as_str()),
                "SECOND_OBJECTIVE",
                "FIRST_OBJECTIVE",
                "",
            ),
            (None, "Editor-only prediction", "SECOND_OBJECTIVE", ""),
        ] {
            if !decision.is_empty() {
                host.handle_request(AheadRequest::WorkItemNote {
                    item_id: item_id.clone(),
                    kind: "decision".into(),
                    body_markdown: decision.into(),
                })?;
            }
            let (address, server) = spawn_stub(r#"{"choices":[{"text":"ok()"}]}"#);
            std::fs::write(
                workspace.join(".ahead/settings.toml"),
                format!(
                    "[ai]\nactive_connection = '{connection}'\n[[ai.connections]]\nprovider_id = '{connection}'\nname = '{connection}'\nprovider = 'openai-compatible'\nbase_url = 'http://{address}/v1'\nmodel = 'qwen2.5-coder:7b'\n"
                ),
            )?;
            host.request_prediction(
                PredictionRequest {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    session_id: session_id.map(str::to_string),
                    path: "main.rs".into(),
                    cursor: DisplayPosition { line: 0, col: 0 },
                    prefix: String::new(),
                    suffix: String::new(),
                    work_context: String::new(),
                },
                &[],
            )?;
            let body: Value =
                serde_json::from_str(&server.join().expect("provider stub"))?;
            let prompt = body["prompt"].as_str().context("provider prompt")?;
            assert!(
                prompt.contains(expected),
                "missing selected context: {expected}"
            );
            assert!(
                !prompt.contains(excluded),
                "leaked another session: {excluded}"
            );
            if !decision.is_empty() {
                assert!(prompt.contains(decision));
            }
            if session_id != Some(first.session.id.as_str()) {
                assert!(!prompt.contains("INITIAL_DECISION"));
            }
        }
        assert_eq!(notifications.lock().unwrap().iter().filter(|notification| matches!(
            notification, ahead_rpc::ahead::AheadNotification::PredictionContextChanged { session_id: Some(id) } if id == &first.session.id
        )).count(), 2);
        Ok(())
    }

    #[test]
    fn editor_only_fim_request_includes_agents_hierarchy() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let workspace = temporary.path();
        std::fs::create_dir_all(workspace.join(".ahead"))?;
        std::fs::create_dir_all(workspace.join("src"))?;
        std::fs::write(workspace.join("AGENTS.md"), "Root FIM instruction.")?;
        std::fs::write(workspace.join("src/AGENTS.md"), "Nested FIM instruction.")?;

        let (address, server) =
            spawn_stub(r#"{"choices":[{"text":"completed()"}]}"#);
        let connection_name = format!("ahead-fim-test-{}", uuid::Uuid::new_v4());
        std::fs::write(
            workspace.join(".ahead/settings.toml"),
            format!(
                r#"
                [ai]
                active_connection = "{connection_name}"

                [[ai.connections]]
                name = "{connection_name}"
                provider_id = "{connection_name}"
                provider = "openai-compatible"
                base_url = "http://{address}/v1"
                model = "qwen2.5-coder:7b"
                "#
            ),
        )?;

        let host = crate::ahead::host::AheadSessionHost::in_memory()?;
        host.set_workspace(workspace.to_path_buf());
        let result = host.request_prediction(
            PredictionRequest {
                request_id: "editor-only-fim".to_string(),
                session_id: None,
                path: "src/main.rs".to_string(),
                cursor: DisplayPosition { line: 0, col: 7 },
                prefix: "fn main".to_string(),
                suffix: "() {}".to_string(),
                work_context: String::new(),
            },
            &[],
        )?;
        let body: Value =
            serde_json::from_str(&server.join().expect("provider stub thread"))?;
        let prompt = body["prompt"].as_str().context("FIM prompt")?;

        assert_eq!(result.replacement, "completed()");
        assert!(prompt.contains("Root FIM instruction."));
        assert!(prompt.contains("Nested FIM instruction."));
        assert!(prompt.contains("<|fim_prefix|>fn main"));
        Ok(())
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
            session_id: Some("sess-1".to_string()),
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
            session_id: Some("sess-2".to_string()),
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
        assert_eq!(infer_prompt_format("glm-4:9b"), Some(PromptFormat::Glm));
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
        assert!(
            format_fim_prompt(PromptFormat::Codestral, "a", "b")
                .starts_with("[SUFFIX]")
        );
    }

    #[test]
    fn clean_fim_completion_truncates_leaked_tokens() {
        assert_eq!(clean_fim_completion("ok()<|fim_suffix|>trailing"), "ok()");
        assert_eq!(clean_fim_completion("ok()"), "ok()");
    }

    #[test]
    fn cursor_excerpt_bounds_cover_small_buffers_fully() {
        let text = "fn a() {}\nfn b() {}\n";
        assert_eq!(
            cursor_excerpt_bounds(text, 5, MAX_EXCERPT_TOKENS),
            (0, text.len())
        );
    }

    #[test]
    fn cursor_excerpt_bounds_window_around_cursor() {
        let text = (0..400)
            .map(|i| format!("line {i:03}\n"))
            .collect::<String>();
        let cursor = text.find("line 200").unwrap();
        let (start, end) = cursor_excerpt_bounds(&text, cursor, 60);
        assert!(start <= cursor && cursor <= end);
        assert!(end - start < text.len());
        assert!(text.is_char_boundary(start) && text.is_char_boundary(end));
        assert!(start == 0 || text[..start].ends_with('\n'));
    }

    #[test]
    fn cursor_excerpt_caps_oversized_utf8_lines() {
        let text = "é".repeat(40_000);
        let cursor = text.len() / 2;
        let (start, end) = cursor_excerpt_bounds(&text, cursor, 10);

        assert!(text.is_char_boundary(start));
        assert!(text.is_char_boundary(end));
        assert!(start <= cursor && cursor <= end);
        assert!(end - start <= 10 * BYTES_PER_TOKEN_GUESS);
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
        let open_buffers = vec![OpenBufferContext {
            path: "src/client.rs".to_string(),
            relevant_excerpt: "pub struct Request;".to_string(),
        }];
        let header = session_fim_header(
            &work,
            comment_prefix_for_path("src/retry.rs"),
            "root instructions\nnested instructions",
            &open_buffers,
        );
        assert!(header.starts_with("// AHEAD"));
        assert!(header.contains("Preserve idempotency"));
        assert!(header.contains("// root instructions"));
        assert!(header.contains("// nested instructions"));
        assert!(header.contains("// Relevant open buffer: src/client.rs"));
        let unknown_extension = session_fim_header(
            &work,
            comment_prefix_for_path("notes.xyz"),
            "root instructions",
            &[],
        );
        assert!(unknown_extension.contains("root instructions"));
        assert!(unknown_extension.contains("[AHEAD FIM context]"));
    }

    /// Minimal HTTP stub: reads one request with Content-Length framing and
    /// answers with a fixed JSON body, returning the raw request body.
    fn spawn_stub(
        response_body: &'static str,
    ) -> (std::net::SocketAddr, std::thread::JoinHandle<String>) {
        spawn_stub_before_response(response_body, || {})
    }

    fn spawn_stub_before_response(
        response_body: &'static str,
        before_response: impl FnOnce() + Send + 'static,
    ) -> (std::net::SocketAddr, std::thread::JoinHandle<String>) {
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
                if let Some(header_end) =
                    request.windows(4).position(|window| window == b"\r\n\r\n")
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
            before_response();
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
    fn prediction_cancellation_closes_the_provider_before_timeout() {
        use std::io::{BufRead, Read, Write};
        use std::time::Duration;

        for (provider_name, model) in [
            ("openai-compatible", "generic-model"),
            ("openai-compatible", "qwen2.5-coder:7b"),
            ("ollama", "qwen2.5-coder:7b"),
        ] {
            for partial_body in [false, true] {
                let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
                    .expect("local FIM provider");
                let address = listener.local_addr().expect("provider address");
                let (ready_sender, ready_receiver) = std::sync::mpsc::channel();
                let server = std::thread::spawn(move || {
                    let (stream, _) =
                        listener.accept().expect("prediction connected");
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .expect("bounded provider reads");
                    let mut reader = std::io::BufReader::new(stream);
                    let mut content_length = 0;
                    loop {
                        let mut line = String::new();
                        assert!(
                            reader.read_line(&mut line).expect("request header") > 0
                        );
                        if line == "\r\n" {
                            break;
                        }
                        if let Some((name, value)) = line.split_once(':')
                            && name.eq_ignore_ascii_case("content-length")
                        {
                            content_length =
                                value.trim().parse().expect("body size");
                        }
                    }
                    reader
                        .read_exact(&mut vec![0; content_length])
                        .expect("request body");
                    if partial_body {
                        reader.get_mut().write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{",
                        ).expect("partial provider response");
                    }
                    ready_sender.send(()).expect("provider received request");
                    let result = reader.read(&mut [0]);
                    assert!(
                        matches!(result, Ok(0))
                            || result.is_err_and(|error| error.kind()
                                == std::io::ErrorKind::ConnectionReset),
                        "cancellation must close the connection before the five-second inference timeout",
                    );
                });
                let cancellation = PredictionCancellation::default();
                let request_cancellation = cancellation.clone();
                let (result_sender, result_receiver) = std::sync::mpsc::channel();
                let worker = std::thread::spawn(move || {
                    let work = PredictionWorkContext {
                        work_kind: WorkKind::ProductChange,
                        mode: AssistanceMode::Assist,
                        phase_title: "Edit".into(),
                        primary_issue: None,
                        active_invariants: Vec::new(),
                    };
                    let request = PredictionRequest {
                        request_id: "cancelled-fim".into(),
                        session_id: None,
                        path: "main.rs".into(),
                        cursor: DisplayPosition { line: 0, col: 3 },
                        prefix: "fn ".into(),
                        suffix: String::new(),
                        work_context: String::new(),
                    };
                    let provider = PredictionProviderConfig {
                        provider_id: None,
                        provider: provider_name.into(),
                        base_url: format!("http://{address}/v1"),
                        api_key: None,
                        model: model.into(),
                    };
                    result_sender
                        .send(PredictionEngine::predict_cancellable(
                            &work,
                            &request,
                            &[],
                            &provider,
                            &request_cancellation,
                        ))
                        .expect("prediction result");
                });
                ready_receiver
                    .recv_timeout(Duration::from_secs(3))
                    .expect("provider request");
                cancellation.cancel();
                let result = result_receiver
                    .recv_timeout(Duration::from_secs(2))
                    .expect("cancellation must wake inference promptly");
                assert!(
                    result
                        .expect_err("cancelled prediction")
                        .to_string()
                        .contains("cancelled")
                );
                worker.join().expect("prediction worker");
                server.join().expect("provider observed connection closure");
            }
        }
    }

    #[test]
    fn host_prediction_rejects_context_changed_during_inference() -> Result<()> {
        use ahead_rpc::ahead::AheadRequest;
        for change in ["instructions", "provider", "decision"] {
            let temporary = tempfile::tempdir()?;
            let workspace = temporary.path();
            std::fs::create_dir(workspace.join(".ahead"))?;
            std::fs::write(workspace.join("AGENTS.md"), "Initial instruction.")?;
            let host = std::sync::Arc::new(
                crate::ahead::host::AheadSessionHost::in_memory()?,
            );
            host.set_workspace(workspace.to_path_buf());
            let session = host.start_work(
                Some(WorkKind::ProductChange),
                "Test".into(),
                "Retry safely".into(),
                None,
            )?;
            let item = host.handle_request(AheadRequest::WorkItemCreate {
                session_id: session.session.id.clone(),
                title: "Retries".into(),
            })?;
            let item_id = item["id"].as_str().context("item id")?.to_string();
            let changed_workspace = workspace.to_path_buf();
            let changed_host = host.clone();
            let (address, server) = spawn_stub_before_response(
                r#"{"choices":[{"text":"obsolete_completion()"}]}"#,
                move || match change {
                    "instructions" => std::fs::write(
                        changed_workspace.join("AGENTS.md"),
                        "New instruction.",
                    )
                    .expect("change instruction"),
                    "provider" => {
                        let path = changed_workspace.join(".ahead/settings.toml");
                        let config =
                            std::fs::read_to_string(&path).expect("provider config");
                        std::fs::write(
                            path,
                            config.replace("generic-model", "changed-model"),
                        )
                        .expect("change provider model");
                    }
                    "decision" => {
                        changed_host
                            .handle_request(AheadRequest::WorkItemNote {
                                item_id,
                                kind: "decision".into(),
                                body_markdown: "Never retry committed responses"
                                    .into(),
                            })
                            .expect("change durable decision");
                    }
                    _ => unreachable!(),
                },
            );
            let connection = format!("isolated-fim-{}", uuid::Uuid::new_v4());
            std::fs::write(
                workspace.join(".ahead/settings.toml"),
                format!(
                    "[ai]\nactive_connection = '{connection}'\n[[ai.connections]]\nname = '{connection}'\nprovider_id = '{connection}'\nprovider = 'openai-compatible'\nbase_url = 'http://{address}/v1'\nmodel = 'generic-model'\n",
                ),
            )?;
            let result = host.request_prediction(
                PredictionRequest {
                    request_id: "stale-host-context".into(),
                    session_id: Some(session.session.id),
                    path: "main.rs".into(),
                    cursor: DisplayPosition { line: 0, col: 3 },
                    prefix: "fn ".into(),
                    suffix: String::new(),
                    work_context: String::new(),
                },
                &[],
            );
            server.join().expect("provider response");
            assert!(
                result
                    .expect_err("stale prediction must not escape the host")
                    .to_string()
                    .contains("prediction context changed"),
                "{change}"
            );
        }
        Ok(())
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
            session_id: Some("session-1".to_string()),
            path: "src/retry.rs".to_string(),
            cursor: DisplayPosition { line: 4, col: 7 },
            prefix: "pub fn retry".to_string(),
            suffix: " {\n}".to_string(),
            work_context: String::new(),
        };
        let (address, server) =
            spawn_stub(r#"{"choices":[{"text":"retry_with_backoff()"}]}"#);
        let provider = PredictionProviderConfig {
            provider_id: None,
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
            session_id: Some("session-1".to_string()),
            path: "src/retry.rs".to_string(),
            cursor: DisplayPosition { line: 1, col: 0 },
            prefix: "pub fn ".to_string(),
            suffix: String::new(),
            work_context: String::new(),
        };
        let (address, server) = spawn_stub(r#"{"response":"ok()"}"#);
        let provider = PredictionProviderConfig {
            provider_id: None,
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

    #[test]
    fn prediction_provider_selects_an_active_workspace_connection() {
        let workspace = std::env::temp_dir().join(format!(
            "ahead-prediction-providers-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(workspace.join(".ahead")).expect("workspace");
        std::fs::write(
            workspace.join(".ahead/settings.toml"),
            r#"
                [ai]
                active_connection = "Hosted"

                [[ai.connections]]
                name = "Local"
                provider_id = "local"
                provider = "ollama"
                base_url = "http://127.0.0.1:11434/v1"
                models = ["qwen"]

                [[ai.connections]]
                name = "Hosted"
                provider_id = "hosted"
                base_url = "https://models.example.test/v1"
                model = "reasoning"
                api_key = "secret"
            "#,
        )
        .expect("settings");

        let provider = PredictionProviderConfig::from_workspace(Some(&workspace))
            .expect("load workspace provider");
        assert_eq!(provider.provider, "openai-compatible");
        assert_eq!(provider.base_url, "https://models.example.test/v1");
        assert_eq!(provider.model, "reasoning");
        assert_eq!(provider.api_key.as_deref(), Some("secret"));

        let selected_model = PredictionModelContext {
            provider_id: "local".into(),
            model_id: "qwen2.5-coder:32b".into(),
            context_window: Some(8_192),
        };
        let provider =
            PredictionProviderConfig::from_workspace_with_user_home_and_model(
                Some(&workspace),
                None,
                Some(&selected_model),
            )
            .expect("resolve the session-selected provider");
        assert_eq!(provider.provider_id.as_deref(), Some("local"));
        assert_eq!(provider.provider, "ollama");
        assert_eq!(provider.model, "qwen2.5-coder:32b");
        std::fs::remove_dir_all(workspace).expect("cleanup");
    }

    #[test]
    fn prediction_provider_without_active_connection_uses_config_order() {
        let workspace = tempfile::tempdir().expect("workspace");
        let ahead = workspace.path().join(".ahead");
        std::fs::create_dir(&ahead).expect("workspace AHEAD directory");
        std::fs::write(
            ahead.join("settings.toml"),
            "[[ai.connections]]\nname = 'Zulu'\nprovider_id = 'zulu'\nbase_url = 'https://zulu.example.test/v1'\nmodel = 'zulu-model'\n\n[[ai.connections]]\nname = 'Alpha'\nprovider_id = 'alpha'\nbase_url = 'https://alpha.example.test/v1'\nmodel = 'alpha-model'\n",
        )
        .expect("provider settings");

        let provider = PredictionProviderConfig::from_workspace_with_user_home(
            Some(workspace.path()),
            None,
        )
        .expect("load provider settings");

        assert_eq!(provider.base_url, "https://zulu.example.test/v1");
        assert_eq!(provider.model, "zulu-model");
    }

    #[test]
    fn prediction_provider_uses_ahead_user_home_and_skips_a_missing_home() {
        let user_home = tempfile::tempdir().expect("user home");
        let user_ahead = user_home.path().join(".ahead");
        std::fs::create_dir(&user_ahead).expect("user AHEAD directory");
        std::fs::write(
            user_ahead.join("settings.toml"),
            "[ai]\nactive_connection = 'Hosted'\n[[ai.connections]]\nname = 'Hosted'\nprovider_id = 'hosted'\nbase_url = 'https://models.example.test/v1'\nmodel = 'private-default'\napi_key = 'private-token'\n",
        )
        .expect("user provider settings");

        let workspace = tempfile::tempdir().expect("workspace");
        let workspace_ahead = workspace.path().join(".ahead");
        std::fs::create_dir(&workspace_ahead).expect("workspace AHEAD directory");
        std::fs::write(
            workspace_ahead.join("config.toml"),
            "[[ai.connections]]\nname = 'Hosted'\nprovider_id = 'hosted'\nbase_url = 'https://models.example.test/v1'\nmodel = 'workspace-override'\n",
        )
        .expect("workspace provider config");

        let provider = PredictionProviderConfig::from_workspace_with_user_home(
            Some(workspace.path()),
            Some(user_home.path()),
        )
        .expect("merge user and workspace providers");
        assert_eq!(provider.model, "workspace-override");
        assert_eq!(provider.api_key.as_deref(), Some("private-token"));

        let missing_home = workspace.path().join("missing-user-home");
        let provider = PredictionProviderConfig::from_workspace_with_user_home(
            Some(workspace.path()),
            Some(missing_home.as_path()),
        )
        .expect("missing optional user home");
        assert_eq!(provider.model, "workspace-override");
        assert_eq!(provider.api_key, None);
    }

    #[test]
    fn prediction_key_is_not_inherited_by_a_different_endpoint() {
        let workspace = tempfile::tempdir().expect("workspace");
        let ahead = workspace.path().join(".ahead");
        std::fs::create_dir(&ahead).expect("settings directory");
        std::fs::write(
            ahead.join("settings.toml"),
            "[ai]\nactive_connection = 'Shared'\n[[ai.connections]]\nname = 'Shared'\nprovider_id = 'shared-secret-boundary'\nbase_url = 'https://safe.example/v1'\nmodel = 'safe'\napi_key = 'private-token'\n",
        )
        .expect("private settings");
        let shared = ahead.join("config.toml");
        std::fs::write(
            &shared,
            "[ai]\n[[ai.connections]]\nname = 'Shared'\nprovider_id = 'shared-secret-boundary'\nbase_url = 'https://safe.example/v1'\nmodel = 'shared'\n",
        )
        .expect("same-endpoint override");
        let provider =
            PredictionProviderConfig::from_workspace(Some(workspace.path()))
                .expect("same-endpoint provider");
        assert_eq!(provider.api_key.as_deref(), Some("private-token"));
        assert_eq!(provider.model, "shared");

        std::fs::write(
            &shared,
            "[ai]\n[[ai.connections]]\nname = 'Shared'\nprovider_id = 'shared-secret-boundary'\nbase_url = 'https://other.example/v1'\nmodel = 'other'\n",
        )
        .expect("different-endpoint override");
        let provider =
            PredictionProviderConfig::from_workspace(Some(workspace.path()))
                .expect("different-endpoint provider");
        assert_eq!(provider.base_url, "https://other.example/v1");
        assert_eq!(provider.api_key, None);
    }

    #[cfg(unix)]
    #[test]
    fn prediction_provider_reports_symlinked_workspace_settings() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().expect("workspace");
        let ahead = workspace.path().join(".ahead");
        std::fs::create_dir(&ahead).expect("settings directory");
        std::fs::write(
            ahead.join("config.toml"),
            "[ai]\nactive_connection = 'Safe'\n[[ai.connections]]\nname = 'Safe'\nprovider_id = 'ahead-symlink-regression-safe'\nbase_url = 'http://127.0.0.1:1234/v1'\nmodel = 'safe'\n",
        )
        .expect("shared provider");
        let outside = workspace.path().join("outside.toml");
        std::fs::write(
            &outside,
            "[ai]\nactive_connection = 'Outside'\n[[ai.connections]]\nname = 'Outside'\nbase_url = 'https://outside.example/v1'\nmodel = 'outside'\napi_key = 'secret'\n",
        )
        .expect("outside provider");
        symlink(&outside, ahead.join("settings.toml"))
            .expect("symlink provider settings");

        let error = PredictionProviderConfig::from_workspace(Some(workspace.path()))
            .expect_err("symlinked provider settings must fail closed");
        assert!(format!("{error:#}").contains("settings.toml"));
        std::fs::remove_file(ahead.join("settings.toml"))
            .expect("remove provider symlink");
        std::fs::write(ahead.join("settings.toml"), "[ai\n")
            .expect("write invalid provider settings");
        let error = PredictionProviderConfig::from_workspace(Some(workspace.path()))
            .expect_err("invalid provider TOML must be reported");
        assert!(format!("{error:#}").contains("invalid TOML"));
    }
}
