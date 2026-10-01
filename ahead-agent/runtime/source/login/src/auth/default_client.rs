//! Default AHEAD HTTP client: shared `User-Agent`, `originator`, optional residency header, and
//! `HttpClient` construction.
//!
//! Use [`crate::default_client`] or [`ahead_model_auth::default_client`] from other crates in this
//! workspace.

use codex_http_client::BuildRouteAwareHttpClientError;
use codex_http_client::ClientRouteClass;
use codex_http_client::HttpClient;
use codex_http_client::HttpClientBuilder;
use codex_http_client::HttpClientFactory;
use codex_http_client::OutboundProxyPolicy;
pub use codex_http_client::RequestBuilder as CodexRequestBuilder;
use http::HeaderMap;
use http::HeaderValue;
use http::header::USER_AGENT;
use std::sync::LazyLock;
use std::sync::RwLock;

pub const DEFAULT_ORIGINATOR: &str = "ahead";
pub const AHEAD_INTERNAL_ORIGINATOR_OVERRIDE_ENV_VAR: &str = "AHEAD_INTERNAL_ORIGINATOR_OVERRIDE";
pub const RESIDENCY_HEADER_NAME: &str = "x-openai-internal-codex-residency";

pub use codex_config::ResidencyRequirement;

#[derive(Debug, Clone)]
pub struct Originator {
    pub value: String,
    pub header_value: HeaderValue,
}
static REQUIREMENTS_RESIDENCY: LazyLock<RwLock<Option<ResidencyRequirement>>> =
    LazyLock::new(|| RwLock::new(None));
static ROUTE_AWARE_CLIENT_BUILD_PERMIT: tokio::sync::Semaphore =
    tokio::sync::Semaphore::const_new(1);

fn get_originator_value() -> Originator {
    let value = std::env::var(AHEAD_INTERNAL_ORIGINATOR_OVERRIDE_ENV_VAR)
        .ok()
        .unwrap_or(DEFAULT_ORIGINATOR.to_string());

    match HeaderValue::from_str(&value) {
        Ok(header_value) => Originator {
            value,
            header_value,
        },
        Err(e) => {
            tracing::error!("Unable to turn originator override {value} into header value: {e}");
            Originator {
                value: DEFAULT_ORIGINATOR.to_string(),
                header_value: HeaderValue::from_static(DEFAULT_ORIGINATOR),
            }
        }
    }
}

pub fn set_default_client_residency_requirement(enforce_residency: Option<ResidencyRequirement>) {
    let Ok(mut guard) = REQUIREMENTS_RESIDENCY.write() else {
        tracing::warn!("Failed to acquire requirements residency lock");
        return;
    };
    *guard = enforce_residency;
}

/// Returns the current process-wide residency requirement.
pub fn read_default_client_residency_requirement() -> Option<ResidencyRequirement> {
    REQUIREMENTS_RESIDENCY.read().ok().and_then(|guard| *guard)
}

pub fn originator() -> Originator {
    get_originator_value()
}

/// Adds a valid, non-default thread originator override to request headers.
///
/// The default client already supplies the process originator. Thread-scoped callers should use
/// this helper to override that value only when the thread originator differs.
pub fn add_originator_header(headers: &mut HeaderMap, originator_value: &str) {
    let default_originator = originator();
    if originator_value == default_originator.value.as_str() {
        return;
    }

    match HeaderValue::from_str(originator_value) {
        Ok(header_value) => {
            headers.insert("originator", header_value);
        }
        Err(err) => {
            tracing::warn!("ignoring invalid thread originator header value: {err}");
        }
    }
}

pub fn is_first_party_originator(originator_value: &str) -> bool {
    originator_value == DEFAULT_ORIGINATOR
        || originator_value == "codex_cli_rs"
        || originator_value == "codex-tui"
        || originator_value == "codex_vscode"
        || originator_value.starts_with("Codex ")
}

/// Create an HTTP client with default `originator` and `User-Agent` headers set.
///
/// This supported default path preserves the transport's existing proxy behavior and does not opt into
/// Codex's route-aware system/PAC resolution.
pub fn create_client() -> HttpClient {
    build_default_client(default_http_client_builder())
}

/// Create the default HTTP client without request URL or response-header diagnostics.
///
/// This preserves the default client's legacy custom-CA fallback and transport proxy behavior while
/// avoiding diagnostics that could expose credentials embedded in request URLs or headers.
pub fn create_client_without_request_logging() -> HttpClient {
    build_default_client(default_http_client_builder().without_request_logging())
}

/// Builds the default AHEAD HTTP client for a concrete outbound route.
///
/// When route-aware proxy handling is disabled, or the client is running inside the Codex
/// sandbox, this preserves the default client's existing proxy behavior. Otherwise it resolves
/// the destination through the shared system/PAC-aware routing policy.
pub fn create_client_for_route(
    http_client_factory: &HttpClientFactory,
    request_url: &str,
    route_class: ClientRouteClass,
) -> Result<HttpClient, BuildRouteAwareHttpClientError> {
    if matches!(
        http_client_factory.outbound_proxy_policy(),
        OutboundProxyPolicy::ReqwestDefault
    ) {
        return Ok(create_client());
    }
    if is_sandboxed() {
        // Preserve the sandbox's existing no-proxy policy; sandboxed command egress is routed
        // separately through network-proxy.
        return Ok(create_client());
    }

    default_http_client_builder().build_respecting_outbound_proxy_policy(
        http_client_factory,
        request_url,
        route_class,
    )
}

/// Builds the default AHEAD HTTP client for a concrete outbound route without blocking the
/// async runtime worker that initiated the request.
pub async fn create_client_for_route_async(
    http_client_factory: HttpClientFactory,
    request_url: String,
    route_class: ClientRouteClass,
) -> std::io::Result<HttpClient> {
    let permit = ROUTE_AWARE_CLIENT_BUILD_PERMIT
        .acquire()
        .await
        .map_err(std::io::Error::other)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        create_client_for_route(&http_client_factory, &request_url, route_class)
            .map_err(std::io::Error::from)
    })
    .await
    .map_err(std::io::Error::other)?
}

fn default_http_client_builder() -> HttpClientBuilder {
    HttpClientBuilder::new().default_headers(default_headers())
}

// These legacy constructors intentionally preserve the infallible behavior of `create_client`.
// New endpoint-aware call sites use `create_client_for_route` and propagate construction errors.
#[allow(deprecated)]
fn build_default_client(builder: HttpClientBuilder) -> HttpClient {
    if is_sandboxed() {
        builder.build_direct_with_custom_ca_fallback()
    } else {
        builder.build_with_transport_default_proxy_and_custom_ca_fallback()
    }
}

pub fn default_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("originator", originator().header_value);
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(concat!("AHEAD/", env!("CARGO_PKG_VERSION"))),
    );
    if let Ok(guard) = REQUIREMENTS_RESIDENCY.read()
        && let Some(requirement) = guard.as_ref()
        && !headers.contains_key(RESIDENCY_HEADER_NAME)
    {
        let value = match requirement {
            ResidencyRequirement::Us => HeaderValue::from_static("us"),
        };
        headers.insert(RESIDENCY_HEADER_NAME, value);
    }
    headers
}

fn is_sandboxed() -> bool {
    std::env::var("CODEX_SANDBOX").as_deref() == Ok("seatbelt")
}

#[cfg(test)]
mod tests {
    use super::default_headers;
    use http::header::USER_AGENT;

    #[test]
    fn default_headers_use_ahead_identity() {
        let headers = default_headers();

        assert!(
            headers
                .get("originator")
                .is_some_and(|value| value == "ahead")
        );
        assert!(
            headers
                .get(USER_AGENT)
                .is_some_and(|value| value.as_bytes().starts_with(b"AHEAD/"))
        );
    }
}
