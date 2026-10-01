use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, RwLock},
};

use codex_config::{
    ManagedAuthPolicy,
    types::{AuthCredentialsStoreMode, AuthKeyringBackendKind},
};
use codex_protocol::{
    account::PlanType as AccountPlanType,
    auth::{AuthMode, RefreshTokenFailedError, RefreshTokenFailedReason},
    config_types::{ForcedLoginMethod, ModelProviderAuthInfo},
};
use thiserror::Error;
use tokio::sync::watch;

use crate::{AuthHeaders, AuthRouteConfig, external_bearer::BearerTokenRefresher};

pub const OPENAI_API_KEY_ENV_VAR: &str = "OPENAI_API_KEY";
pub const CODEX_API_KEY_ENV_VAR: &str = "CODEX_API_KEY";
pub const CODEX_ACCESS_TOKEN_ENV_VAR: &str = "CODEX_ACCESS_TOKEN";
pub const REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR: &str = "CODEX_REFRESH_TOKEN_URL_OVERRIDE";
pub const REVOKE_TOKEN_URL_OVERRIDE_ENV_VAR: &str = "CODEX_REVOKE_TOKEN_URL_OVERRIDE";
pub const CLIENT_ID_OVERRIDE_ENV_VAR: &str = "CODEX_APP_SERVER_LOGIN_CLIENT_ID";
pub const CLIENT_ID: &str = "";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiKeyAuth {
    api_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatgptAuth {
    token: String,
    account_id: Option<String>,
    email: Option<String>,
    user_id: Option<String>,
    plan_type: Option<AccountPlanType>,
    fedramp: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatgptAuthTokens(ChatgptAuth);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersonalAccessTokenAuth {
    access_token: String,
    account_id: String,
    user_id: String,
    email: Option<String>,
    plan_type: AccountPlanType,
    fedramp: bool,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentIdentityAuthRecord {
    pub agent_runtime_id: String,
    pub agent_private_key: String,
    pub account_id: String,
    pub chatgpt_user_id: String,
    pub email: Option<String>,
    pub plan_type: AccountPlanType,
    pub chatgpt_account_is_fedramp: bool,
    pub task_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentIdentityAuth {
    record: AgentIdentityAuthRecord,
}

impl AgentIdentityAuth {
    pub async fn from_record(
        record: AgentIdentityAuthRecord,
        _auth_api_base_url: &str,
        _auth_route_config: &AuthRouteConfig,
    ) -> std::io::Result<Self> {
        Ok(Self { record })
    }

    pub fn record(&self) -> &AgentIdentityAuthRecord {
        &self.record
    }

    pub fn run_task_id(&self) -> &str {
        self.record.task_id.as_deref().unwrap_or_default()
    }

    pub fn account_id(&self) -> &str {
        &self.record.account_id
    }

    pub fn chatgpt_user_id(&self) -> &str {
        &self.record.chatgpt_user_id
    }

    pub fn is_fedramp_account(&self) -> bool {
        self.record.chatgpt_account_is_fedramp
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AgentIdentityAuthError {
    #[error("{operation} failed after {attempts} attempts: {message}")]
    BootstrapUnavailable {
        operation: &'static str,
        attempts: usize,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentIdentityAuthPolicy {
    JwtOnly,
    ChatGptAuth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexAuth {
    ApiKey(ApiKeyAuth),
    Chatgpt(ChatgptAuth),
    ChatgptAuthTokens(ChatgptAuthTokens),
    Headers(AuthHeaders),
    AgentIdentity(AgentIdentityAuth),
    PersonalAccessToken(PersonalAccessTokenAuth),
    BedrockApiKey(BedrockApiKeyAuth),
    BedrockAccessKeys(BedrockAccessKeysAuth),
}

impl CodexAuth {
    pub fn from_api_key(api_key: &str) -> Self {
        Self::ApiKey(ApiKeyAuth {
            api_key: api_key.to_string(),
        })
    }

    pub fn auth_mode(&self) -> AuthMode {
        match self {
            Self::ApiKey(_) => AuthMode::ApiKey,
            Self::Chatgpt(_) => AuthMode::Chatgpt,
            Self::ChatgptAuthTokens(_) => AuthMode::ChatgptAuthTokens,
            Self::Headers(_) => AuthMode::Headers,
            Self::AgentIdentity(_) => AuthMode::AgentIdentity,
            Self::PersonalAccessToken(_) => AuthMode::PersonalAccessToken,
            Self::BedrockApiKey(_) => AuthMode::BedrockApiKey,
            Self::BedrockAccessKeys(_) => AuthMode::BedrockAccessKeys,
        }
    }

    pub fn api_auth_mode(&self) -> AuthMode {
        self.auth_mode()
    }

    pub fn is_api_key_auth(&self) -> bool {
        matches!(self, Self::ApiKey(_))
    }

    pub fn is_personal_access_token_auth(&self) -> bool {
        matches!(self, Self::PersonalAccessToken(_))
    }

    pub fn is_chatgpt_auth(&self) -> bool {
        matches!(
            self,
            Self::Chatgpt(_)
                | Self::ChatgptAuthTokens(_)
                | Self::Headers(_)
                | Self::AgentIdentity(_)
                | Self::PersonalAccessToken(_)
        )
    }

    pub fn uses_codex_backend(&self) -> bool {
        self.auth_mode().uses_codex_backend()
    }

    pub fn is_external_chatgpt_tokens(&self) -> bool {
        matches!(self, Self::ChatgptAuthTokens(_) | Self::Headers(_))
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
            Self::Chatgpt(auth) => Ok(auth.token.clone()),
            Self::ChatgptAuthTokens(auth) => Ok(auth.0.token.clone()),
            Self::PersonalAccessToken(auth) => Ok(auth.access_token.clone()),
            Self::AgentIdentity(_) | Self::Headers(_) => Err(std::io::Error::other(
                "authentication does not expose a bearer token",
            )),
            Self::BedrockApiKey(_) | Self::BedrockAccessKeys(_) => Err(std::io::Error::other(
                "Bedrock authentication is provider-specific",
            )),
        }
    }

    pub fn get_account_id(&self) -> Option<String> {
        match self {
            Self::Chatgpt(auth) => auth.account_id.clone(),
            Self::ChatgptAuthTokens(auth) => auth.0.account_id.clone(),
            Self::AgentIdentity(auth) => Some(auth.record.account_id.clone()),
            Self::PersonalAccessToken(auth) => Some(auth.account_id.clone()),
            _ => None,
        }
    }

    pub fn get_chatgpt_user_id(&self) -> Option<String> {
        match self {
            Self::Chatgpt(auth) => auth.user_id.clone(),
            Self::ChatgptAuthTokens(auth) => auth.0.user_id.clone(),
            Self::AgentIdentity(auth) => Some(auth.record.chatgpt_user_id.clone()),
            Self::PersonalAccessToken(auth) => Some(auth.user_id.clone()),
            _ => None,
        }
    }

    pub fn get_account_email(&self) -> Option<String> {
        match self {
            Self::Chatgpt(auth) => auth.email.clone(),
            Self::ChatgptAuthTokens(auth) => auth.0.email.clone(),
            Self::AgentIdentity(auth) => auth.record.email.clone(),
            Self::PersonalAccessToken(auth) => auth.email.clone(),
            _ => None,
        }
    }

    pub fn account_plan_type(&self) -> Option<AccountPlanType> {
        match self {
            Self::Chatgpt(auth) => auth.plan_type,
            Self::ChatgptAuthTokens(auth) => auth.0.plan_type,
            Self::AgentIdentity(auth) => Some(auth.record.plan_type),
            Self::PersonalAccessToken(auth) => Some(auth.plan_type),
            _ => None,
        }
    }

    pub fn is_fedramp_account(&self) -> bool {
        match self {
            Self::Chatgpt(auth) => auth.fedramp,
            Self::ChatgptAuthTokens(auth) => auth.0.fedramp,
            Self::AgentIdentity(auth) => auth.record.chatgpt_account_is_fedramp,
            Self::PersonalAccessToken(auth) => auth.fedramp,
            _ => false,
        }
    }

    pub fn is_workspace_account(&self) -> bool {
        false
    }

    #[doc(hidden)]
    pub fn create_dummy_chatgpt_auth_for_testing() -> Self {
        Self::ChatgptAuthTokens(ChatgptAuthTokens(ChatgptAuth {
            token: "ahead-test-token".to_string(),
            account_id: Some("ahead-test-account".to_string()),
            email: None,
            user_id: Some("ahead-test-user".to_string()),
            plan_type: None,
            fedramp: false,
        }))
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

#[derive(Debug, Error)]
#[error("AHEAD model authentication initialization failed")]
pub struct AuthManagerInitializationError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalAuthRefreshReason {
    Unauthorized,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalAuthRefreshContext {
    pub reason: ExternalAuthRefreshReason,
    pub previous_account_id: Option<String>,
}

pub type ExternalAuthFuture<'a, T> = Pin<Box<dyn Future<Output = std::io::Result<T>> + Send + 'a>>;

pub trait ExternalAuth: Send + Sync {
    fn resolve(&self) -> ExternalAuthFuture<'_, CodexAuth>;
    fn refresh(&self, context: ExternalAuthRefreshContext) -> ExternalAuthFuture<'_, CodexAuth>;

    fn classify_error(&self, error: std::io::Error) -> RefreshTokenError {
        RefreshTokenError::Transient(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthConfig {
    pub codex_home: PathBuf,
    pub auth_credentials_store_mode: AuthCredentialsStoreMode,
    pub keyring_backend_kind: AuthKeyringBackendKind,
    pub forced_login_method: Option<ForcedLoginMethod>,
    pub chatgpt_base_url: Option<String>,
    pub forced_chatgpt_workspace_id: Option<Vec<String>>,
    pub managed_auth_policy: ManagedAuthPolicy,
    pub auth_route_config: AuthRouteConfig,
}

impl AuthConfig {
    pub fn validate(&self) -> std::io::Result<()> {
        Ok(())
    }
}

pub trait AuthManagerConfig {
    fn codex_home(&self) -> PathBuf;
    fn cli_auth_credentials_store_mode(&self) -> AuthCredentialsStoreMode;
    fn auth_keyring_backend_kind(&self) -> AuthKeyringBackendKind;
    fn forced_login_method(&self) -> Option<ForcedLoginMethod>;
    fn forced_chatgpt_workspace_id(&self) -> Option<Vec<String>>;
    fn managed_auth_policy(&self) -> ManagedAuthPolicy;
    fn chatgpt_base_url(&self) -> String;
    fn auth_route_config(&self) -> AuthRouteConfig;
}

pub struct AuthManager {
    auth: RwLock<Option<CodexAuth>>,
    external_auth: RwLock<Option<Arc<dyn ExternalAuth>>>,
    auth_change_tx: watch::Sender<u64>,
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
        let (auth_change_tx, _) = watch::channel(0);
        Self {
            auth: RwLock::new(None),
            external_auth: RwLock::new(None),
            auth_change_tx,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        _codex_home: PathBuf,
        _enable_codex_api_key_env: bool,
        _auth_credentials_store_mode: AuthCredentialsStoreMode,
        _forced_chatgpt_workspace_id: Option<Vec<String>>,
        _chatgpt_base_url: Option<String>,
        _keyring_backend_kind: AuthKeyringBackendKind,
        _auth_route_config: AuthRouteConfig,
    ) -> Self {
        Self::empty()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn shared(
        codex_home: PathBuf,
        enable_codex_api_key_env: bool,
        auth_credentials_store_mode: AuthCredentialsStoreMode,
        forced_chatgpt_workspace_id: Option<Vec<String>>,
        chatgpt_base_url: Option<String>,
        keyring_backend_kind: AuthKeyringBackendKind,
        auth_route_config: AuthRouteConfig,
    ) -> Arc<Self> {
        Arc::new(
            Self::new(
                codex_home,
                enable_codex_api_key_env,
                auth_credentials_store_mode,
                forced_chatgpt_workspace_id,
                chatgpt_base_url,
                keyring_backend_kind,
                auth_route_config,
            )
            .await,
        )
    }

    pub async fn shared_from_config(
        _config: &impl AuthManagerConfig,
        _enable_codex_api_key_env: bool,
    ) -> Result<Arc<Self>, AuthManagerInitializationError> {
        Ok(Arc::new(Self::empty()))
    }

    pub async fn shared_from_auth_config(
        _auth_config: AuthConfig,
        _enable_codex_api_key_env: bool,
    ) -> Result<Arc<Self>, AuthManagerInitializationError> {
        Ok(Arc::new(Self::empty()))
    }

    pub fn from_auth_for_testing(auth: CodexAuth) -> Arc<Self> {
        Self::from_optional_auth_for_testing(Some(auth))
    }

    pub fn from_optional_auth_for_testing(auth: Option<CodexAuth>) -> Arc<Self> {
        let manager = Self::empty();
        if let Ok(mut current) = manager.auth.write() {
            *current = auth;
        }
        Arc::new(manager)
    }

    pub fn from_auth_for_testing_with_home(auth: CodexAuth, _codex_home: PathBuf) -> Arc<Self> {
        Self::from_auth_for_testing(auth)
    }

    #[doc(hidden)]
    pub fn from_auth_for_testing_with_agent_identity_authapi_base_url(
        auth: CodexAuth,
        _agent_identity_authapi_base_url: String,
    ) -> Arc<Self> {
        Self::from_auth_for_testing(auth)
    }

    pub fn external_bearer_only(config: ModelProviderAuthInfo) -> Arc<Self> {
        let manager = Arc::new(Self::empty());
        if let Ok(mut external_auth) = manager.external_auth.write() {
            *external_auth = Some(Arc::new(BearerTokenRefresher::new(config)));
        }
        manager
    }

    pub fn auth_cached(&self) -> Option<CodexAuth> {
        self.auth.read().ok().and_then(|auth| auth.clone())
    }

    pub async fn auth(&self) -> Option<CodexAuth> {
        if self.has_external_auth() {
            self.reload().await;
        }
        self.auth_cached()
    }

    pub fn auth_change_receiver(&self) -> watch::Receiver<u64> {
        self.auth_change_tx.subscribe()
    }

    pub fn refresh_failure_for_auth(&self, _auth: &CodexAuth) -> Option<RefreshTokenFailedError> {
        None
    }

    pub async fn reload(&self) -> bool {
        let Some(provider) = self.external_auth_provider() else {
            return false;
        };
        match provider.resolve().await {
            Ok(auth) => self.set_cached_auth(Some(auth)),
            Err(error) => {
                tracing::warn!(%error, "failed to resolve provider authentication");
                false
            }
        }
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

    pub fn clear_external_auth(&self) {
        if let Ok(mut external_auth) = self.external_auth.write() {
            external_auth.take();
        }
        self.set_cached_auth(None);
    }

    pub fn has_external_auth(&self) -> bool {
        self.external_auth_provider().is_some()
    }

    pub fn is_workload_identity_selected(&self) -> bool {
        false
    }

    pub fn is_external_chatgpt_auth_active(&self) -> bool {
        false
    }

    pub fn codex_api_key_env_enabled(&self) -> bool {
        false
    }

    pub fn auth_mode(&self) -> Option<AuthMode> {
        self.auth_cached().as_ref().map(CodexAuth::auth_mode)
    }

    pub fn get_api_auth_mode(&self) -> Option<AuthMode> {
        self.auth_mode()
    }

    pub fn current_auth_uses_codex_backend(&self) -> bool {
        self.auth_cached()
            .as_ref()
            .is_some_and(CodexAuth::uses_codex_backend)
    }

    pub fn unauthorized_recovery(self: &Arc<Self>) -> UnauthorizedRecovery {
        UnauthorizedRecovery {
            manager: Arc::clone(self),
            attempted: false,
        }
    }

    pub async fn refresh_token(&self) -> Result<(), RefreshTokenError> {
        self.refresh_token_from_authority().await
    }

    pub async fn refresh_token_from_authority(&self) -> Result<(), RefreshTokenError> {
        let Some(provider) = self.external_auth_provider() else {
            return Ok(());
        };
        let previous_account_id = self.auth_cached().and_then(|auth| auth.get_account_id());
        let auth = provider
            .refresh(ExternalAuthRefreshContext {
                reason: ExternalAuthRefreshReason::Unauthorized,
                previous_account_id,
            })
            .await
            .map_err(|error| provider.classify_error(error))?;
        self.set_cached_auth(Some(auth));
        Ok(())
    }

    pub async fn logout(&self) -> std::io::Result<bool> {
        self.clear_external_auth();
        Ok(false)
    }

    pub async fn logout_with_revoke(&self) -> std::io::Result<bool> {
        self.logout().await
    }

    fn external_auth_provider(&self) -> Option<Arc<dyn ExternalAuth>> {
        self.external_auth
            .read()
            .ok()
            .and_then(|provider| provider.clone())
    }

    fn set_cached_auth(&self, auth: Option<CodexAuth>) -> bool {
        let Ok(mut current) = self.auth.write() else {
            return false;
        };
        if *current == auth {
            return false;
        }
        *current = auth;
        self.auth_change_tx.send_modify(|revision| *revision += 1);
        true
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

    pub async fn next(&mut self) -> Result<UnauthorizedRecoveryStepResult, RefreshTokenError> {
        if !self.has_next() {
            return Err(RefreshTokenError::Permanent(RefreshTokenFailedError::new(
                RefreshTokenFailedReason::Other,
                "No more recovery steps available.".to_string(),
            )));
        }
        self.manager.refresh_token_from_authority().await?;
        self.attempted = true;
        Ok(UnauthorizedRecoveryStepResult {
            auth_state_changed: Some(true),
        })
    }
}

pub fn oauth_client_id() -> String {
    String::new()
}

pub fn is_workload_identity_selected() -> bool {
    false
}
