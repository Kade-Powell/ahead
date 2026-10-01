//! Test-only helpers exposed for retained runtime integration tests.

use std::sync::Arc;

use codex_http_client::{HttpClientFactory, OutboundProxyPolicy};

use crate::{AuthManager, AuthRouteConfig, CodexAuth};

pub fn auth_manager_from_optional_auth(auth: Option<CodexAuth>) -> Arc<AuthManager> {
    AuthManager::from_optional_auth_for_testing(auth)
}

pub fn transport_default_auth_route_config() -> AuthRouteConfig {
    AuthRouteConfig::from_http_client_factory(HttpClientFactory::new(
        OutboundProxyPolicy::ReqwestDefault,
    ))
}
