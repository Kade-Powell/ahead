use crate::auth::SharedAuthProvider;
use crate::endpoint::session::EndpointSession;
use crate::error::ApiError;
use crate::provider::Provider;
use codex_client::HttpTransport;
use codex_client::RequestTelemetry;
use codex_protocol::openai_models::{
    ApplyPatchToolType, InputModality, ModelInfo, ModelsResponse, ReasoningEffort,
    ReasoningEffortPreset,
};
use http::HeaderMap;
use http::Method;
use http::header::ETAG;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
struct OpenAiCompatibleModelsResponse {
    data: Vec<OpenAiCompatibleModel>,
}

#[derive(Deserialize)]
struct OpenAiCompatibleModel {
    id: serde_json::Value,
    #[serde(default)]
    name: Option<serde_json::Value>,
    #[serde(default)]
    description: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "deserialize_optional_token_count")]
    context_length: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_token_count")]
    context_window: Option<u64>,
    #[serde(default)]
    architecture: Option<serde_json::Value>,
    #[serde(default)]
    supported_parameters: Option<serde_json::Value>,
    #[serde(default)]
    supports_tools: Option<serde_json::Value>,
    #[serde(default)]
    supports_tool_calls: Option<serde_json::Value>,
    #[serde(default)]
    reasoning: Option<serde_json::Value>,
}

fn deserialize_optional_token_count<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
    }))
}

fn input_modalities(architecture: Option<serde_json::Value>) -> Vec<InputModality> {
    let Some(architecture) = architecture else {
        return vec![InputModality::Text];
    };
    let Some(input_modalities) = architecture
        .get("input_modalities")
        .and_then(serde_json::Value::as_array)
    else {
        return vec![InputModality::Text];
    };
    let mut modalities = Vec::new();
    for modality in input_modalities {
        let Some(modality) = modality.as_str() else {
            continue;
        };
        let modality = match modality {
            "text" => Some(InputModality::Text),
            "image" => Some(InputModality::Image),
            "audio" => Some(InputModality::Audio),
            _ => None,
        };
        if let Some(modality) = modality
            && !modalities.contains(&modality)
        {
            modalities.push(modality);
        }
    }
    if modalities.is_empty() {
        modalities.push(InputModality::Text);
    }
    modalities
}

fn tool_call_support_from_parameters(
    supported_parameters: Option<&serde_json::Value>,
) -> Option<bool> {
    let parameters = supported_parameters?.as_array()?;
    Some(
        parameters
            .iter()
            .any(|parameter| parameter.as_str() == Some("tools")),
    )
}

fn merge_tool_call_support(left: Option<bool>, right: Option<bool>) -> Option<bool> {
    match (left, right) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (Some(true), _) | (_, Some(true)) => Some(true),
        (None, None) => None,
    }
}

fn model_tool_call_support(model: &OpenAiCompatibleModel) -> Option<bool> {
    [
        tool_call_support_from_parameters(model.supported_parameters.as_ref()),
        model
            .supports_tools
            .as_ref()
            .and_then(serde_json::Value::as_bool),
        model
            .supports_tool_calls
            .as_ref()
            .and_then(serde_json::Value::as_bool),
    ]
    .into_iter()
    .fold(None, merge_tool_call_support)
}

fn reasoning_effort(value: Option<&serde_json::Value>) -> Option<ReasoningEffort> {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|effort| !effort.is_empty())
        .and_then(|effort| effort.parse().ok())
}

fn reasoning_efforts(value: Option<&serde_json::Value>) -> Vec<ReasoningEffortPreset> {
    let Some(efforts) = value
        .and_then(|reasoning| reasoning.get("supported_efforts"))
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };

    let mut supported_efforts = Vec::new();
    for value in efforts {
        let Some(effort) = reasoning_effort(Some(value)) else {
            continue;
        };
        if supported_efforts
            .iter()
            .any(|preset: &ReasoningEffortPreset| preset.effort == effort)
        {
            continue;
        }
        supported_efforts.push(ReasoningEffortPreset {
            description: effort.to_string(),
            effort,
        });
    }
    supported_efforts
}

fn context_window(model: &OpenAiCompatibleModel) -> Option<i64> {
    [model.context_length, model.context_window]
        .into_iter()
        .flatten()
        .find_map(|tokens| i64::try_from(tokens).ok().filter(|tokens| *tokens > 0))
}

fn decode_models(body: &[u8]) -> Result<Vec<ModelInfo>, ApiError> {
    match serde_json::from_slice::<ModelsResponse>(body) {
        Ok(response) => Ok(response.models),
        Err(codex_error) => {
            let compatible = serde_json::from_slice::<OpenAiCompatibleModelsResponse>(body)
                .map_err(|_| {
                    ApiError::Stream(format!(
                        "failed to decode models response: {codex_error}; body: {}",
                        String::from_utf8_lossy(body)
                    ))
                })?;
            let mut models: Vec<ModelInfo> = Vec::new();
            for model in compatible.data {
                let Some(slug) = model.id.as_str().map(str::trim) else {
                    continue;
                };
                if slug.is_empty() {
                    continue;
                }
                let context_window = context_window(&model);
                let supports_tool_calls = model_tool_call_support(&model);
                let modalities = input_modalities(model.architecture);
                let default_reasoning_level = model
                    .reasoning
                    .as_ref()
                    .and_then(|reasoning| reasoning.get("default_effort"))
                    .and_then(|effort| reasoning_effort(Some(effort)));
                let supported_reasoning_levels = reasoning_efforts(model.reasoning.as_ref());
                let mut model = ModelInfo::from_openai_compatible_catalog(
                    slug.to_string(),
                    model.name.and_then(|name| name.as_str().map(str::to_owned)),
                    model
                        .description
                        .and_then(|description| description.as_str().map(str::to_owned)),
                    context_window,
                    modalities,
                    supports_tool_calls,
                );
                if supports_tool_calls == Some(true) {
                    model.apply_patch_tool_type = Some(ApplyPatchToolType::Function);
                }
                model.default_reasoning_level = default_reasoning_level;
                model.supported_reasoning_levels = supported_reasoning_levels;
                if let Some(existing) = models
                    .iter_mut()
                    .find(|existing| existing.slug == model.slug)
                {
                    if existing.context_window.is_none() {
                        existing.context_window = model.context_window;
                        existing.max_context_window = model.max_context_window;
                    }
                    if existing.description.is_none() {
                        existing.description = model.description;
                    }
                    if existing.default_reasoning_level.is_none() {
                        existing.default_reasoning_level = model.default_reasoning_level;
                    }
                    if existing.supported_reasoning_levels.is_empty() {
                        existing.supported_reasoning_levels = model.supported_reasoning_levels;
                    }
                    if existing.display_name == existing.slug && model.display_name != model.slug {
                        existing.display_name = model.display_name;
                    }
                    existing.supports_tool_calls = merge_tool_call_support(
                        existing.supports_tool_calls,
                        model.supports_tool_calls,
                    );
                    existing.apply_patch_tool_type = (existing.supports_tool_calls == Some(true))
                        .then_some(ApplyPatchToolType::Function);
                } else {
                    models.push(model);
                }
            }
            Ok(models)
        }
    }
}

pub struct ModelsClient<T: HttpTransport> {
    session: EndpointSession<T>,
}

impl<T: HttpTransport> ModelsClient<T> {
    pub fn new(transport: T, provider: Provider, auth: SharedAuthProvider) -> Self {
        Self {
            session: EndpointSession::new(transport, provider, auth),
        }
    }

    pub fn with_telemetry(self, request: Option<Arc<dyn RequestTelemetry>>) -> Self {
        Self {
            session: self.session.with_request_telemetry(request),
        }
    }

    fn path() -> &'static str {
        "models"
    }

    fn append_client_version_query(req: &mut codex_client::Request, client_version: &str) {
        let separator = if req.url.contains('?') { '&' } else { '?' };
        req.url = format!("{}{}client_version={client_version}", req.url, separator);
    }

    pub fn request_url(provider: &Provider, client_version: &str) -> String {
        let mut request = provider.build_request(Method::GET, Self::path());
        Self::append_client_version_query(&mut request, client_version);
        request.url
    }

    pub async fn list_models(
        &self,
        request_url: String,
        extra_headers: HeaderMap,
    ) -> Result<(Vec<ModelInfo>, Option<String>), ApiError> {
        let resp = self
            .session
            .execute_with(
                Method::GET,
                Self::path(),
                extra_headers,
                /*body*/ None,
                move |req| {
                    req.url.clone_from(&request_url);
                },
            )
            .await?;

        let header_etag = resp
            .headers
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);

        let models = decode_models(&resp.body)?;

        Ok((models, header_etag))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthProvider;
    use crate::provider::RetryConfig;
    use codex_client::Request;
    use codex_client::Response;
    use codex_client::StreamResponse;
    use codex_client::TransportError;
    use http::HeaderMap;
    use http::StatusCode;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Clone)]
    struct CapturingTransport {
        last_request: Arc<Mutex<Option<Request>>>,
        body: Arc<ModelsResponse>,
        etag: Option<String>,
    }

    impl Default for CapturingTransport {
        fn default() -> Self {
            Self {
                last_request: Arc::new(Mutex::new(None)),
                body: Arc::new(ModelsResponse { models: Vec::new() }),
                etag: None,
            }
        }
    }

    impl HttpTransport for CapturingTransport {
        async fn execute(&self, req: Request) -> Result<Response, TransportError> {
            *self.last_request.lock().unwrap() = Some(req);
            let body = serde_json::to_vec(&*self.body).unwrap();
            let mut headers = HeaderMap::new();
            if let Some(etag) = &self.etag {
                headers.insert(ETAG, etag.parse().unwrap());
            }
            Ok(Response {
                status: StatusCode::OK,
                headers,
                body: body.into(),
            })
        }

        async fn stream(&self, _req: Request) -> Result<StreamResponse, TransportError> {
            Err(TransportError::Build("stream should not run".to_string()))
        }
    }

    #[derive(Clone, Default)]
    struct DummyAuth;

    impl AuthProvider for DummyAuth {
        fn add_auth_headers(&self, _headers: &mut HeaderMap) {}
    }

    fn provider(base_url: &str) -> Provider {
        Provider {
            name: "test".to_string(),
            base_url: base_url.to_string(),
            query_params: None,
            headers: HeaderMap::new(),
            retry: RetryConfig {
                max_attempts: 1,
                base_delay: Duration::from_millis(1),
                retry_429: false,
                retry_5xx: true,
                retry_transport: true,
            },
            stream_idle_timeout: Duration::from_secs(1),
        }
    }

    #[tokio::test]
    async fn appends_client_version_query() {
        let response = ModelsResponse { models: Vec::new() };

        let transport = CapturingTransport {
            last_request: Arc::new(Mutex::new(None)),
            body: Arc::new(response),
            etag: None,
        };

        let provider = provider("https://example.com/api/codex");
        let request_url = ModelsClient::<CapturingTransport>::request_url(&provider, "0.99.0");
        let client = ModelsClient::new(transport.clone(), provider, Arc::new(DummyAuth));

        let (models, _) = client
            .list_models(request_url, HeaderMap::new())
            .await
            .expect("request should succeed");

        assert_eq!(models.len(), 0);

        let url = transport
            .last_request
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .url
            .clone();
        assert_eq!(
            url,
            "https://example.com/api/codex/models?client_version=0.99.0"
        );
    }

    #[tokio::test]
    async fn parses_models_response() {
        let response = ModelsResponse {
            models: vec![
                serde_json::from_value(json!({
                    "slug": "gpt-test",
                    "display_name": "gpt-test",
                    "description": "desc",
                    "default_reasoning_level": "medium",
                    "supported_reasoning_levels": [{"effort": "low", "description": "low"}, {"effort": "medium", "description": "medium"}, {"effort": "high", "description": "high"}],
                    "shell_type": "shell_command",
                    "visibility": "list",
                    "minimal_client_version": [0, 99, 0],
                    "supported_in_api": true,
                    "priority": 1,
                    "upgrade": null,
                    "support_verbosity": false,
                    "default_verbosity": null,
                    "apply_patch_tool_type": null,
                    "truncation_policy": {"mode": "bytes", "limit": 10_000},
                    "supports_image_detail_original": false,
                    "context_window": 272_000,
                    "experimental_supported_tools": [],
                }))
                .unwrap(),
            ],
        };

        let transport = CapturingTransport {
            last_request: Arc::new(Mutex::new(None)),
            body: Arc::new(response),
            etag: None,
        };

        let provider = provider("https://example.com/api/codex");
        let request_url = ModelsClient::<CapturingTransport>::request_url(&provider, "0.99.0");
        let client = ModelsClient::new(transport, provider, Arc::new(DummyAuth));

        let (models, _) = client
            .list_models(request_url, HeaderMap::new())
            .await
            .expect("request should succeed");

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].slug, "gpt-test");
        assert_eq!(models[0].supported_in_api, true);
        assert_eq!(models[0].priority, 1);
    }

    #[test]
    fn decodes_openai_compatible_catalog_metadata_without_fallback_models() {
        let models = decode_models(
            br#"{"object":"list","data":[{"id":"qwen"},{"id":42},{"id":null},{"id":"qwen","name":"Qwen 3","description":"Fast model","context_length":131072,"architecture":{"input_modalities":["text","image","image","video",null]},"supported_parameters":["tools"],"reasoning":{"default_effort":"medium","supported_efforts":["low","medium","high","medium",null,4]}},{"id":"text-only","context_window":"65536","supported_parameters":["temperature"]},{"id":"invalid","name":12,"description":false,"context_length":-1,"architecture":"invalid","supported_parameters":12,"supports_tools":"false","reasoning":{"default_effort":[],"supported_efforts":"low"}},{"id":"audio","architecture":{"input_modalities":["audio"]}},{"id":"fallback-context","context_length":18446744073709551615,"context_window":4096},{"id":"openrouter-chat-only","supports_tools":false},{"id":"openrouter-agent","supports_tools":true},{"id":"openai-chat-only","supports_tool_calls":false},{"id":"conflicting-capabilities","supported_parameters":["tools"],"supports_tools":false}]}"#,
        )
        .expect("OpenAI-compatible catalog should decode");

        assert_eq!(models.len(), 9);
        assert_eq!(models[0].slug, "qwen");
        assert_eq!(models[0].display_name, "Qwen 3");
        assert_eq!(models[0].description.as_deref(), Some("Fast model"));
        assert_eq!(models[0].context_window, Some(131_072));
        assert_eq!(
            models[0].default_reasoning_level,
            Some(ReasoningEffort::Medium)
        );
        assert_eq!(
            models[0].supported_reasoning_levels,
            vec![
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Low,
                    description: "low".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Medium,
                    description: "medium".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::High,
                    description: "high".to_string(),
                },
            ]
        );
        assert_eq!(models[0].supports_tool_calls, Some(true));
        assert_eq!(
            models[0].input_modalities,
            vec![InputModality::Text, InputModality::Image]
        );
        assert!(!models[0].used_fallback_model_metadata);
        assert!(!models[0].supports_reasoning_summary_parameter);
        assert_eq!(
            models[0].apply_patch_tool_type,
            Some(ApplyPatchToolType::Function)
        );
        assert_eq!(models[1].display_name, "text-only");
        assert_eq!(models[1].context_window, Some(65_536));
        assert_eq!(models[1].supports_tool_calls, Some(false));
        assert!(models[1].apply_patch_tool_type.is_none());
        assert_eq!(models[2].context_window, None);
        assert_eq!(models[2].input_modalities, vec![InputModality::Text]);
        assert_eq!(models[2].supports_tool_calls, None);
        assert!(models[2].apply_patch_tool_type.is_none());
        assert_eq!(models[2].default_reasoning_level, None);
        assert!(models[2].supported_reasoning_levels.is_empty());
        assert_eq!(models[3].input_modalities, vec![InputModality::Audio]);
        assert_eq!(models[3].supports_tool_calls, None);
        assert_eq!(models[4].context_window, Some(4_096));
        assert_eq!(models[5].supports_tool_calls, Some(false));
        assert_eq!(models[6].supports_tool_calls, Some(true));
        assert_eq!(
            models[6].apply_patch_tool_type,
            Some(ApplyPatchToolType::Function)
        );
        assert_eq!(models[7].supports_tool_calls, Some(false));
        assert!(models[7].apply_patch_tool_type.is_none());
        assert_eq!(models[8].supports_tool_calls, Some(false));
        assert!(models[8].apply_patch_tool_type.is_none());
    }

    #[tokio::test]
    async fn list_models_includes_etag() {
        let response = ModelsResponse { models: Vec::new() };

        let transport = CapturingTransport {
            last_request: Arc::new(Mutex::new(None)),
            body: Arc::new(response),
            etag: Some("\"abc\"".to_string()),
        };

        let provider = provider("https://example.com/api/codex");
        let request_url = ModelsClient::<CapturingTransport>::request_url(&provider, "0.1.0");
        let client = ModelsClient::new(transport, provider, Arc::new(DummyAuth));

        let (models, etag) = client
            .list_models(request_url, HeaderMap::new())
            .await
            .expect("request should succeed");

        assert_eq!(models.len(), 0);
        assert_eq!(etag, Some("\"abc\"".to_string()));
    }
}
