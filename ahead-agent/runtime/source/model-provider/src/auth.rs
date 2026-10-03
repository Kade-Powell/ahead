use std::sync::Arc;

use ahead_model_auth::AuthHeaders;
use ahead_model_auth::AuthManager;
use ahead_model_auth::CodexAuth;
use codex_api::AuthProvider;
use codex_api::SharedAuthProvider;
use codex_model_provider_info::ModelProviderInfo;
use codex_protocol::error::CodexErr;
use http::HeaderMap;

use crate::bearer_auth_provider::BearerAuthProvider;

const BEDROCK_API_KEY_UNSUPPORTED_MESSAGE: &str =
    "Bedrock API key auth is only supported by the Amazon Bedrock model provider";

#[derive(Clone, Debug)]
struct HeaderAuthProvider {
    auth: AuthHeaders,
}

impl AuthProvider for HeaderAuthProvider {
    fn add_auth_headers(&self, headers: &mut HeaderMap) {
        headers.extend(self.auth.headers().clone());
    }
}

// Some providers are meant to send no auth headers. Examples include local OSS
// providers and custom test providers with `requires_openai_auth = false`.
#[derive(Clone, Debug)]
struct UnauthenticatedAuthProvider;

impl AuthProvider for UnauthenticatedAuthProvider {
    fn add_auth_headers(&self, _headers: &mut HeaderMap) {}
}

pub fn unauthenticated_auth_provider() -> SharedAuthProvider {
    Arc::new(UnauthenticatedAuthProvider)
}

/// Returns the provider-scoped auth manager when this provider uses command-backed auth.
///
/// Providers without custom auth continue using the caller-supplied base manager, when present.
pub(crate) fn auth_manager_for_provider(
    auth_manager: Option<Arc<AuthManager>>,
    provider: &ModelProviderInfo,
) -> Option<Arc<AuthManager>> {
    match provider.auth.clone() {
        Some(config) => Some(AuthManager::external_bearer_only(config)),
        None => auth_manager,
    }
}

pub(crate) fn resolve_provider_auth(
    auth: Option<&CodexAuth>,
    provider: &ModelProviderInfo,
) -> codex_protocol::error::Result<SharedAuthProvider> {
    if let Some(auth) = bearer_auth_for_provider(provider)? {
        return Ok(Arc::new(auth));
    }

    if !provider.requires_openai_auth && provider.auth.is_none() {
        return Ok(unauthenticated_auth_provider());
    }

    if matches!(
        auth,
        Some(CodexAuth::BedrockApiKey(_) | CodexAuth::BedrockAccessKeys(_))
    ) {
        return Err(CodexErr::UnsupportedOperation(
            BEDROCK_API_KEY_UNSUPPORTED_MESSAGE.to_string(),
        ));
    }

    Ok(match auth {
        Some(auth) => auth_provider_from_auth(auth),
        None => unauthenticated_auth_provider(),
    })
}

fn bearer_auth_for_provider(
    provider: &ModelProviderInfo,
) -> codex_protocol::error::Result<Option<BearerAuthProvider>> {
    if let Some(api_key) = provider.api_key()? {
        return Ok(Some(BearerAuthProvider::new(api_key)));
    }

    if let Some(token) = provider.experimental_bearer_token.clone() {
        return Ok(Some(BearerAuthProvider::new(token.into_inner())));
    }

    Ok(None)
}

/// Builds request-header auth for a first-party Codex auth snapshot.
pub fn auth_provider_from_auth(auth: &CodexAuth) -> SharedAuthProvider {
    match auth {
        CodexAuth::Headers(auth) => Arc::new(HeaderAuthProvider { auth: auth.clone() }),
        CodexAuth::BedrockApiKey(_) | CodexAuth::BedrockAccessKeys(_) => {
            unreachable!("{BEDROCK_API_KEY_UNSUPPORTED_MESSAGE}")
        }
        CodexAuth::ApiKey(_)
        | CodexAuth::Chatgpt(_)
        | CodexAuth::ChatgptAuthTokens(_)
        | CodexAuth::AgentIdentity(_)
        | CodexAuth::PersonalAccessToken(_) => Arc::new(BearerAuthProvider {
            token: auth.get_token().ok(),
            account_id: auth.get_account_id(),
            is_fedramp_account: auth.is_fedramp_account(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use ahead_model_auth::auth::BedrockApiKeyAuth;
    use codex_model_provider_info::WireApi;
    use codex_model_provider_info::create_oss_provider_with_base_url;
    use codex_protocol::config_types::ModelProviderAuthInfo;
    use codex_protocol::error::CodexErrorDetails;
    use http::header::AUTHORIZATION;
    use pretty_assertions::assert_eq;
    use std::num::NonZeroU64;

    use super::*;

    #[test]
    fn unauthenticated_auth_provider_adds_no_headers() {
        let provider =
            create_oss_provider_with_base_url("http://localhost:11434/v1", WireApi::Responses);
        let auth = resolve_provider_auth(/*auth*/ None, &provider).expect("auth should resolve");

        assert!(auth.to_auth_headers().is_empty());
    }

    #[test]
    fn custom_provider_does_not_inherit_ambient_auth_headers() {
        let provider =
            create_oss_provider_with_base_url("http://localhost:11434/v1", WireApi::Responses);
        let mut ambient_headers = HeaderMap::new();
        ambient_headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer ambient-token"),
        );
        ambient_headers.insert(
            "ChatGPT-Account-ID",
            HeaderValue::from_static("account-123"),
        );
        let ambient_auth = CodexAuth::Headers(AuthHeaders::new(ambient_headers));

        let auth =
            resolve_provider_auth(Some(&ambient_auth), &provider).expect("auth should resolve");

        assert!(auth.to_auth_headers().is_empty());
    }

    #[test]
    fn custom_provider_does_not_inherit_ambient_bedrock_auth() {
        let provider =
            create_oss_provider_with_base_url("http://localhost:11434/v1", WireApi::Responses);
        let ambient_auth = CodexAuth::BedrockApiKey(BedrockApiKeyAuth {
            api_key: "bedrock-api-key-test".to_string(),
            region: "us-east-1".to_string(),
        });

        let auth =
            resolve_provider_auth(Some(&ambient_auth), &provider).expect("auth should resolve");

        assert!(auth.to_auth_headers().is_empty());
    }

    #[test]
    fn custom_provider_uses_explicit_bearer_instead_of_ambient_auth() {
        let mut provider =
            create_oss_provider_with_base_url("http://localhost:11434/v1", WireApi::Responses);
        provider.experimental_bearer_token = Some("provider-token".into());
        let ambient_auth = CodexAuth::BedrockApiKey(BedrockApiKeyAuth {
            api_key: "bedrock-api-key-test".to_string(),
            region: "us-east-1".to_string(),
        });

        let headers = resolve_provider_auth(Some(&ambient_auth), &provider)
            .expect("auth should resolve")
            .to_auth_headers();

        assert_eq!(
            headers.get(AUTHORIZATION),
            Some(&HeaderValue::from_static("Bearer provider-token"))
        );
        assert_eq!(headers.len(), 1);
    }

    #[test]
    fn custom_provider_uses_command_resolved_auth() {
        let mut provider =
            create_oss_provider_with_base_url("http://localhost:11434/v1", WireApi::Responses);
        provider.auth = Some(ModelProviderAuthInfo {
            command: "print-token".to_string(),
            args: Vec::new(),
            timeout_ms: NonZeroU64::new(5_000).expect("timeout should be non-zero"),
            refresh_interval_ms: 300_000,
            cwd: std::env::current_dir()
                .expect("current directory should be available")
                .try_into()
                .expect("current directory should be absolute"),
        });
        let command_auth = CodexAuth::from_api_key("command-token");

        let headers = resolve_provider_auth(Some(&command_auth), &provider)
            .expect("auth should resolve")
            .to_auth_headers();

        assert_eq!(
            headers.get(AUTHORIZATION),
            Some(&HeaderValue::from_static("Bearer command-token"))
        );
    }

    #[test]
    fn openai_provider_preserves_ambient_auth_headers() {
        let provider = ModelProviderInfo::create_openai_provider(/*base_url*/ None);
        let mut expected = HeaderMap::new();
        expected.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer ambient-token"),
        );
        expected.insert(
            "ChatGPT-Account-ID",
            HeaderValue::from_static("account-123"),
        );
        let ambient_auth = CodexAuth::Headers(AuthHeaders::new(expected.clone()));

        let auth =
            resolve_provider_auth(Some(&ambient_auth), &provider).expect("auth should resolve");

        assert_eq!(auth.to_auth_headers(), expected);
    }

    #[test]
    fn header_auth_adds_predefined_headers() {
        let mut expected = HeaderMap::new();
        expected.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer external"),
        );
        expected.insert("x-external-auth", HeaderValue::from_static("enabled"));
        let auth = CodexAuth::Headers(AuthHeaders::new(expected.clone()));

        let actual = auth_provider_from_auth(&auth).to_auth_headers();

        assert_eq!(actual, expected);
    }

    #[test]
    fn openai_provider_rejects_bedrock_api_key_auth() {
        let provider = ModelProviderInfo::create_openai_provider(/*base_url*/ None);
        let auth = CodexAuth::BedrockApiKey(BedrockApiKeyAuth {
            api_key: "bedrock-api-key-test".to_string(),
            region: "us-east-1".to_string(),
        });

        match resolve_provider_auth(Some(&auth), &provider) {
            Err(err) => match err.details() {
                CodexErrorDetails::UnsupportedOperation(message) => {
                    assert_eq!(message, BEDROCK_API_KEY_UNSUPPORTED_MESSAGE);
                }
                details => panic!("unexpected auth error: {details:?}"),
            },
            Ok(_) => panic!("Bedrock API key auth should be rejected"),
        }
    }
}
