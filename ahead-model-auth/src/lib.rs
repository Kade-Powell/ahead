//! AHEAD model authentication and shared HTTP client support.
//!
//! Managed AHEAD sessions receive credentials from the selected `.ahead` model
//! connection. This crate
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

pub use ahead_auth::*;
pub use auth_headers::AuthHeaders;
