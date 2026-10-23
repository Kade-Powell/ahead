use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::RwLock;

use codex_protocol::auth::AuthMode;
use codex_protocol::auth::RefreshTokenFailedError;
use codex_protocol::auth::RefreshTokenFailedReason;
use codex_protocol::config_types::ModelProviderAuthInfo;
use thiserror::Error;

use crate::AuthHeaders;
use crate::external_bearer::BearerTokenRefresher;

pub const OPENAI_API_KEY_ENV_VAR: &str = "OPENAI_API_KEY";

#[derive(Clone, PartialEq, Eq)]
pub struct ApiKeyAuth {
    api_key: String,
}

impl std::fmt::Debug for ApiKeyAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ApiKeyAuth")
            .field("api_key", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct BedrockApiKeyAuth {
    pub api_key: String,
    pub region: String,
}

impl std::fmt::Debug for BedrockApiKeyAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BedrockApiKeyAuth")
            .field("api_key", &"<redacted>")
            .field("region", &self.region)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct BedrockAccessKeysAuth {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

impl std::fmt::Debug for BedrockAccessKeysAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BedrockAccessKeysAuth")
            .field("access_key_id", &"<redacted>")
            .field("secret_access_key", &"<redacted>")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AheadAuth {
    ApiKey(ApiKeyAuth),
    Headers(AuthHeaders),
    BedrockApiKey(BedrockApiKeyAuth),
    BedrockAccessKeys(BedrockAccessKeysAuth),
}

impl AheadAuth {
    pub fn from_api_key(api_key: &str) -> Self {
        Self::ApiKey(ApiKeyAuth {
            api_key: api_key.to_string(),
        })
    }

    pub fn auth_mode(&self) -> AuthMode {
        match self {
            Self::ApiKey(_) => AuthMode::ApiKey,
            Self::Headers(_) => AuthMode::Headers,
            Self::BedrockApiKey(_) => AuthMode::BedrockApiKey,
            Self::BedrockAccessKeys(_) => AuthMode::BedrockAccessKeys,
        }
    }

    pub fn api_key(&self) -> Option<&str> {
        match self {
            Self::ApiKey(auth) => Some(&auth.api_key),
            _ => None,
        }
    }

    pub fn get_token(&self) -> Result<String, std::io::Error> {
        match self {
            Self::ApiKey(auth) => Ok(auth.api_key.clone()),
            Self::Headers(_) => Err(std::io::Error::other(
                "authentication does not expose a bearer token",
            )),
            Self::BedrockApiKey(_) | Self::BedrockAccessKeys(_) => Err(
                std::io::Error::other("Bedrock authentication is provider-specific"),
            ),
        }
    }

    #[doc(hidden)]
    pub fn create_dummy_headers_auth_for_testing() -> Self {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_static("Bearer ahead-test-token"),
        );
        Self::Headers(AuthHeaders::new(headers))
    }
}

pub fn read_openai_api_key_from_env() -> Option<String> {
    read_non_empty_env_var(OPENAI_API_KEY_ENV_VAR)
}

fn read_non_empty_env_var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[derive(Debug, Error)]
pub enum RefreshTokenError {
    #[error("{0}")]
    Permanent(#[from] RefreshTokenFailedError),
    #[error(transparent)]
    Transient(#[from] std::io::Error),
}

impl RefreshTokenError {
    pub fn failed_reason(&self) -> Option<RefreshTokenFailedReason> {
        match self {
            Self::Permanent(error) => Some(error.reason),
            Self::Transient(_) => None,
        }
    }
}

pub type ExternalAuthFuture<'a, T> =
    Pin<Box<dyn Future<Output = std::io::Result<T>> + Send + 'a>>;

pub trait ExternalAuth: Send + Sync {
    fn resolve(&self) -> ExternalAuthFuture<'_, AheadAuth>;
    fn refresh(&self) -> ExternalAuthFuture<'_, AheadAuth>;

    fn classify_error(&self, error: std::io::Error) -> RefreshTokenError {
        RefreshTokenError::Transient(error)
    }
}

pub struct AuthManager {
    auth: RwLock<Option<AheadAuth>>,
    external_auth: RwLock<Option<Arc<dyn ExternalAuth>>>,
}

impl std::fmt::Debug for AuthManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthManager")
            .field("auth", &self.auth_cached().map(|auth| auth.auth_mode()))
            .field("has_external_auth", &self.has_external_auth())
            .finish()
    }
}

impl AuthManager {
    fn empty() -> Self {
        Self {
            auth: RwLock::new(None),
            external_auth: RwLock::new(None),
        }
    }

    pub fn shared_empty() -> Arc<Self> {
        Arc::new(Self::empty())
    }

    pub fn from_auth_for_testing(auth: AheadAuth) -> Arc<Self> {
        let manager = Self::empty();
        if let Ok(mut current) = manager.auth.write() {
            *current = Some(auth);
        }
        Arc::new(manager)
    }

    pub fn external_bearer_only(config: ModelProviderAuthInfo) -> Arc<Self> {
        let manager = Arc::new(Self::empty());
        if let Ok(mut external_auth) = manager.external_auth.write() {
            *external_auth = Some(Arc::new(BearerTokenRefresher::new(config)));
        }
        manager
    }

    pub fn auth_cached(&self) -> Option<AheadAuth> {
        self.auth.read().ok().and_then(|auth| auth.clone())
    }

    pub async fn auth(&self) -> Option<AheadAuth> {
        if let Some(provider) = self.external_auth_provider() {
            match provider.resolve().await {
                Ok(auth) => self.set_cached_auth(Some(auth)),
                Err(error) => {
                    tracing::warn!(%error, "failed to resolve provider authentication");
                }
            }
        }
        self.auth_cached()
    }

    pub async fn set_external_auth(
        &self,
        external_auth: Arc<dyn ExternalAuth>,
    ) -> Result<(), RefreshTokenError> {
        let auth = external_auth
            .resolve()
            .await
            .map_err(|error| external_auth.classify_error(error))?;
        self.external_auth
            .write()
            .map_err(|_| {
                RefreshTokenError::Transient(std::io::Error::other(
                    "external auth lock is poisoned",
                ))
            })?
            .replace(external_auth);
        self.set_cached_auth(Some(auth));
        Ok(())
    }

    pub fn has_external_auth(&self) -> bool {
        self.external_auth_provider().is_some()
    }

    pub fn auth_mode(&self) -> Option<AuthMode> {
        self.auth_cached().as_ref().map(AheadAuth::auth_mode)
    }

    pub fn unauthorized_recovery(self: &Arc<Self>) -> UnauthorizedRecovery {
        UnauthorizedRecovery {
            manager: Arc::clone(self),
            attempted: false,
        }
    }

    pub async fn refresh_token(&self) -> Result<(), RefreshTokenError> {
        let Some(provider) = self.external_auth_provider() else {
            return Ok(());
        };
        let auth = provider
            .refresh()
            .await
            .map_err(|error| provider.classify_error(error))?;
        self.set_cached_auth(Some(auth));
        Ok(())
    }

    fn external_auth_provider(&self) -> Option<Arc<dyn ExternalAuth>> {
        self.external_auth
            .read()
            .ok()
            .and_then(|provider| provider.clone())
    }

    fn set_cached_auth(&self, auth: Option<AheadAuth>) {
        let Ok(mut current) = self.auth.write() else {
            return;
        };
        if *current == auth {
            return;
        }
        *current = auth;
    }
}

pub struct UnauthorizedRecovery {
    manager: Arc<AuthManager>,
    attempted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnauthorizedRecoveryStepResult {
    auth_state_changed: Option<bool>,
}

impl UnauthorizedRecoveryStepResult {
    pub fn auth_state_changed(&self) -> Option<bool> {
        self.auth_state_changed
    }
}

impl UnauthorizedRecovery {
    pub fn has_next(&self) -> bool {
        !self.attempted && self.manager.has_external_auth()
    }

    pub fn unavailable_reason(&self) -> &'static str {
        if self.has_next() {
            "ready"
        } else if self.attempted {
            "recovery_exhausted"
        } else {
            "not_refreshable_auth"
        }
    }

    pub fn mode_name(&self) -> &'static str {
        "external"
    }

    pub fn step_name(&self) -> &'static str {
        if self.attempted {
            "done"
        } else {
            "external_refresh"
        }
    }

    pub async fn next(
        &mut self,
    ) -> Result<UnauthorizedRecoveryStepResult, RefreshTokenError> {
        if !self.has_next() {
            return Err(RefreshTokenError::Permanent(RefreshTokenFailedError::new(
                RefreshTokenFailedReason::Other,
                "No more recovery steps available.".to_string(),
            )));
        }
        self.manager.refresh_token().await?;
        self.attempted = true;
        Ok(UnauthorizedRecoveryStepResult {
            auth_state_changed: Some(true),
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn api_key_auth_debug_redacts_secret() {
        let secret = "ahead-test-api-key";
        let rendered = format!("{:?}", super::AheadAuth::from_api_key(secret));

        assert!(!rendered.contains(secret));
        assert!(rendered.contains("<redacted>"));
    }
}
