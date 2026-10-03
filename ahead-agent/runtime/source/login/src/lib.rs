//! AHEAD model authentication and shared HTTP client support.
//!
//! The hard fork does not implement Codex product login. Managed AHEAD sessions
//! receive credentials from the selected `.ahead` model connection. This crate
//! retains only provider bearer/header auth, command-backed token refresh, and
//! the shared HTTP client helpers used by the native loop.

#[path = "ahead_auth.rs"]
mod ahead_auth;
#[path = "auth/auth_headers.rs"]
mod auth_headers;
#[path = "auth/default_client.rs"]
pub mod default_client;
#[path = "auth/external_bearer.rs"]
mod external_bearer;
mod outbound_proxy;
pub mod test_support;

pub use ahead_auth::*;
pub use auth_headers::AuthHeaders;
pub use codex_http_client::BuildCustomCaTransportError as BuildLoginHttpClientError;
pub use outbound_proxy::AuthRouteConfig;

/// Compatibility module for retained runtime imports while AHEAD-owned names
/// replace the copied Codex package boundary.
pub mod auth {
    pub use crate::ahead_auth::*;
    pub use crate::auth_headers::AuthHeaders;
    pub use crate::default_client;
}
