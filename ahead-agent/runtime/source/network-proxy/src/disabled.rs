//! Fail-closed compatibility surface for AHEAD's retained loop.
//!
//! AHEAD does not ship Codex's managed proxy, MITM, or
//! remote network-policy service. Protocol/config types remain available while
//! copied call sites are removed; every attempt to start the runtime fails.

use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    io::{self, Write},
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, anyhow, bail};
use codex_utils_absolute_path::AbsolutePathBuf;
use serde::{Deserialize, Serialize};

use crate::{
    EnvironmentNetworkPolicy, NetworkDomainPermissions, NetworkMode, NetworkProxyConfig,
    NetworkUnixSocketPermissions, RemoteNetworkProxyLaunchConfig,
};

const REMOVED_MESSAGE: &str = "managed network proxying is not part of the AHEAD agent runtime";

pub fn normalize_host(host: &str) -> String {
    let host = host.trim();
    if host.starts_with('[')
        && let Some(end) = host.find(']')
    {
        return normalize_dns_host_or_ip_literal(&host[1..end]);
    }
    if host.bytes().filter(|byte| *byte == b':').count() == 1 {
        return normalize_dns_host_or_ip_literal(host.split(':').next().unwrap_or_default());
    }
    normalize_dns_host_or_ip_literal(host)
}

fn normalize_dns_host_or_ip_literal(host: &str) -> String {
    let host = host.to_ascii_lowercase();
    let host = host.trim_end_matches('.');
    if host.parse::<IpAddr>().is_ok() {
        return host.to_string();
    }
    for delimiter in ["%25", "%"] {
        if let Some((ip, scope)) = host.split_once(delimiter)
            && ip.parse::<IpAddr>().is_ok()
        {
            return format!("{ip}%{scope}");
        }
    }
    host.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct MitmHookConfig {
    pub host: String,
    #[serde(rename = "match", default)]
    pub matcher: MitmHookMatchConfig,
    #[serde(default)]
    pub actions: MitmHookActionsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct MitmHookMatchConfig {
    pub methods: Vec<String>,
    pub path_prefixes: Vec<String>,
    pub query: BTreeMap<String, Vec<String>>,
    pub headers: BTreeMap<String, Vec<String>>,
    pub body: Option<MitmHookBodyConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct MitmHookActionsConfig {
    pub strip_request_headers: Vec<String>,
    pub inject_request_headers: Vec<InjectedHeaderConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct InjectedHeaderConfig {
    pub name: String,
    pub secret_env_var: Option<String>,
    pub secret_file: Option<String>,
    pub prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct MitmHookBodyConfig(pub serde_json::Value);

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NetworkProxyConstraints {
    pub enabled: Option<bool>,
    pub mode: Option<NetworkMode>,
    pub allow_upstream_proxy: Option<bool>,
    pub dangerously_allow_non_loopback_proxy: Option<bool>,
    pub dangerously_allow_all_unix_sockets: Option<bool>,
    pub allowed_domains: Option<Vec<String>>,
    pub allowlist_expansion_enabled: Option<bool>,
    pub denied_domains: Option<Vec<String>>,
    pub denylist_expansion_enabled: Option<bool>,
    pub allow_unix_sockets: Option<Vec<String>>,
    pub allow_local_binding: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PartialNetworkProxyConfig {
    pub enabled: Option<bool>,
    pub mode: Option<NetworkMode>,
    pub allow_upstream_proxy: Option<bool>,
    pub dangerously_allow_non_loopback_proxy: Option<bool>,
    pub dangerously_allow_all_unix_sockets: Option<bool>,
    #[serde(default)]
    pub domains: Option<NetworkDomainPermissions>,
    #[serde(default)]
    pub unix_sockets: Option<NetworkUnixSocketPermissions>,
    pub allow_local_binding: Option<bool>,
    pub mitm: Option<bool>,
    pub dangerously_allow_plaintext_credential_injection: Option<bool>,
    #[serde(default)]
    pub mitm_hooks: Option<Vec<MitmHookConfig>>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{REMOVED_MESSAGE}")]
pub struct NetworkProxyConstraintError;

impl NetworkProxyConstraintError {
    pub fn into_anyhow(self) -> anyhow::Error {
        anyhow!(self)
    }
}

#[derive(Clone)]
pub struct ConfigState {
    pub config: NetworkProxyConfig,
    pub constraints: NetworkProxyConstraints,
}

pub fn build_config_state(
    config: NetworkProxyConfig,
    constraints: NetworkProxyConstraints,
) -> Result<ConfigState> {
    validate_policy_against_constraints(&config, &constraints)
        .map_err(NetworkProxyConstraintError::into_anyhow)?;
    Ok(ConfigState {
        config,
        constraints,
    })
}

pub fn validate_policy_against_constraints(
    config: &NetworkProxyConfig,
    _constraints: &NetworkProxyConstraints,
) -> std::result::Result<(), NetworkProxyConstraintError> {
    if config.enabled || config.mitm || !config.mitm_hooks.is_empty() {
        Err(NetworkProxyConstraintError)
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkProxyAuditMetadata {
    pub conversation_id: Option<String>,
    pub app_version: Option<String>,
    pub user_account_id: Option<String>,
    pub auth_mode: Option<String>,
    pub originator: Option<String>,
    pub user_email: Option<String>,
    pub terminal_type: Option<String>,
    pub model: Option<String>,
    pub slug: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkProtocol {
    Http,
    HttpsConnect,
    Socks5Tcp,
    Socks5Udp,
}

impl NetworkProtocol {
    pub const fn as_policy_protocol(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::HttpsConnect => "https_connect",
            Self::Socks5Tcp => "socks5_tcp",
            Self::Socks5Udp => "socks5_udp",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NetworkPolicyDecision {
    Deny,
    Ask,
}

impl NetworkPolicyDecision {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Deny => "deny",
            Self::Ask => "ask",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkDecisionSource {
    BaselinePolicy,
    ModeGuard,
    ProxyState,
    Decider,
}

impl NetworkDecisionSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BaselinePolicy => "baseline_policy",
            Self::ModeGuard => "mode_guard",
            Self::ProxyState => "proxy_state",
            Self::Decider => "decider",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkPolicyAuditEvent {
    pub timestamp: String,
    pub scope: String,
    pub decision: String,
    pub source: String,
    pub reason: String,
    pub protocol: NetworkProtocol,
    pub host: String,
    pub port: u16,
    pub method: Option<String>,
    pub client: Option<String>,
    pub policy_override: bool,
}

pub type NetworkPolicyAuditObserver = Arc<dyn Fn(NetworkPolicyAuditEvent) + Send + Sync + 'static>;

#[derive(Clone, Debug)]
pub struct NetworkPolicyRequest {
    pub protocol: NetworkProtocol,
    pub host: String,
    pub port: u16,
    pub environment_id: Option<String>,
    pub client_addr: Option<String>,
    pub method: Option<String>,
    pub command: Option<String>,
    pub exec_policy_hint: Option<String>,
    pub execution_id: Option<String>,
    pub disconnect: Option<crate::NetworkRequestDisconnect>,
}

pub struct NetworkPolicyRequestArgs {
    pub protocol: NetworkProtocol,
    pub host: String,
    pub port: u16,
    pub environment_id: Option<String>,
    pub client_addr: Option<String>,
    pub method: Option<String>,
    pub command: Option<String>,
    pub exec_policy_hint: Option<String>,
}

impl NetworkPolicyRequest {
    pub fn new(args: NetworkPolicyRequestArgs) -> Self {
        Self {
            protocol: args.protocol,
            host: args.host,
            port: args.port,
            environment_id: args.environment_id,
            client_addr: args.client_addr,
            method: args.method,
            command: args.command,
            exec_policy_hint: args.exec_policy_hint,
            execution_id: None,
            disconnect: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetworkDecision {
    Allow,
    Deny {
        reason: String,
        source: NetworkDecisionSource,
        decision: NetworkPolicyDecision,
    },
}

impl NetworkDecision {
    pub fn deny(reason: impl Into<String>) -> Self {
        Self::deny_with_source(reason, NetworkDecisionSource::Decider)
    }

    pub fn ask(reason: impl Into<String>) -> Self {
        Self::Deny {
            reason: reason.into(),
            source: NetworkDecisionSource::Decider,
            decision: NetworkPolicyDecision::Ask,
        }
    }

    pub fn deny_with_source(reason: impl Into<String>, source: NetworkDecisionSource) -> Self {
        Self::Deny {
            reason: reason.into(),
            source,
            decision: NetworkPolicyDecision::Deny,
        }
    }

    pub fn ask_with_source(reason: impl Into<String>, source: NetworkDecisionSource) -> Self {
        Self::Deny {
            reason: reason.into(),
            source,
            decision: NetworkPolicyDecision::Ask,
        }
    }
}

pub trait NetworkPolicyDecider: Send + Sync + 'static {
    fn decide(&self, request: NetworkPolicyRequest) -> NetworkPolicyDeciderFuture<'_>;
}

pub type NetworkPolicyDeciderFuture<'a> =
    Pin<Box<dyn Future<Output = NetworkDecision> + Send + 'a>>;

impl<D: NetworkPolicyDecider + ?Sized> NetworkPolicyDecider for Arc<D> {
    fn decide(&self, request: NetworkPolicyRequest) -> NetworkPolicyDeciderFuture<'_> {
        Box::pin(async move { (**self).decide(request).await })
    }
}

impl<F, Fut> NetworkPolicyDecider for F
where
    F: Fn(NetworkPolicyRequest) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = NetworkDecision> + Send + 'static,
{
    fn decide(&self, request: NetworkPolicyRequest) -> NetworkPolicyDeciderFuture<'_> {
        Box::pin((self)(request))
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct BlockedRequest {
    pub host: String,
    pub reason: String,
    pub client: Option<String>,
    pub method: Option<String>,
    pub mode: Option<NetworkMode>,
    pub protocol: String,
    #[serde(skip)]
    pub execution_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    pub timestamp: i64,
}

pub struct BlockedRequestArgs {
    pub host: String,
    pub reason: String,
    pub client: Option<String>,
    pub method: Option<String>,
    pub mode: Option<NetworkMode>,
    pub protocol: String,
    pub decision: Option<String>,
    pub source: Option<String>,
    pub port: Option<u16>,
}

impl BlockedRequest {
    pub fn new(args: BlockedRequestArgs) -> Self {
        Self {
            host: args.host,
            reason: args.reason,
            client: args.client,
            method: args.method,
            mode: args.mode,
            protocol: args.protocol,
            execution_id: None,
            decision: args.decision,
            source: args.source,
            port: args.port,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
        }
    }
}

pub trait ConfigReloader: Send + Sync {
    fn source_label(&self) -> String;
    fn maybe_reload(&self) -> ConfigReloaderFuture<'_, Option<ConfigState>>;
    fn reload_now(&self) -> ConfigReloaderFuture<'_, ConfigState>;
}

pub type ConfigReloaderFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

pub trait BlockedRequestObserver: Send + Sync + 'static {
    fn on_blocked_request(&self, request: BlockedRequest) -> BlockedRequestObserverFuture<'_>;
}

pub type BlockedRequestObserverFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

impl<O: BlockedRequestObserver + ?Sized> BlockedRequestObserver for Arc<O> {
    fn on_blocked_request(&self, request: BlockedRequest) -> BlockedRequestObserverFuture<'_> {
        Box::pin(async move { (**self).on_blocked_request(request).await })
    }
}

impl<F, Fut> BlockedRequestObserver for F
where
    F: Fn(BlockedRequest) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    fn on_blocked_request(&self, request: BlockedRequest) -> BlockedRequestObserverFuture<'_> {
        Box::pin((self)(request))
    }
}

#[derive(Clone)]
pub struct NetworkProxyState {
    state: Arc<Mutex<ConfigState>>,
    audit_metadata: NetworkProxyAuditMetadata,
    environment_id: Option<String>,
    execution_id: Option<String>,
    policy_audit_observer: Option<NetworkPolicyAuditObserver>,
}

impl std::fmt::Debug for NetworkProxyState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NetworkProxyState")
            .finish_non_exhaustive()
    }
}

impl NetworkProxyState {
    pub fn from_remote_launch_config(_launch: RemoteNetworkProxyLaunchConfig) -> Result<Self> {
        bail!(REMOVED_MESSAGE)
    }

    pub fn with_reloader(state: ConfigState, _reloader: Arc<dyn ConfigReloader>) -> Self {
        Self::with_reloader_and_audit_metadata(
            state,
            _reloader,
            NetworkProxyAuditMetadata::default(),
        )
    }

    pub fn with_reloader_and_blocked_observer(
        state: ConfigState,
        reloader: Arc<dyn ConfigReloader>,
        _observer: Option<Arc<dyn BlockedRequestObserver>>,
    ) -> Self {
        Self::with_reloader(state, reloader)
    }

    pub fn with_reloader_and_audit_metadata(
        state: ConfigState,
        _reloader: Arc<dyn ConfigReloader>,
        audit_metadata: NetworkProxyAuditMetadata,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(state)),
            audit_metadata,
            environment_id: None,
            execution_id: None,
            policy_audit_observer: None,
        }
    }

    pub fn with_reloader_and_audit_metadata_and_blocked_observer(
        state: ConfigState,
        reloader: Arc<dyn ConfigReloader>,
        audit_metadata: NetworkProxyAuditMetadata,
        _observer: Option<Arc<dyn BlockedRequestObserver>>,
    ) -> Self {
        Self::with_reloader_and_audit_metadata(state, reloader, audit_metadata)
    }

    pub async fn set_blocked_request_observer(
        &self,
        _observer: Option<Arc<dyn BlockedRequestObserver>>,
    ) {
    }

    pub fn set_policy_audit_observer(&mut self, observer: NetworkPolicyAuditObserver) {
        self.policy_audit_observer = Some(observer);
    }

    pub fn audit_metadata(&self) -> &NetworkProxyAuditMetadata {
        &self.audit_metadata
    }

    pub async fn current_cfg(&self) -> Result<NetworkProxyConfig> {
        Ok(self
            .state
            .lock()
            .map_err(|_| anyhow!("network state poisoned"))?
            .config
            .clone())
    }

    pub async fn enabled(&self) -> Result<bool> {
        Ok(false)
    }

    pub async fn replace_config_state(&self, state: ConfigState) -> Result<()> {
        *self
            .state
            .lock()
            .map_err(|_| anyhow!("network state poisoned"))? = state;
        Ok(())
    }

    pub async fn host_blocked(&self, _host: &str, _port: u16) -> Result<HostBlockDecision> {
        Ok(HostBlockDecision::Blocked(HostBlockReason::NotAllowed))
    }

    pub async fn add_allowed_domain(&self, _host: &str) -> Result<()> {
        bail!(REMOVED_MESSAGE)
    }

    pub async fn add_denied_domain(&self, _host: &str) -> Result<()> {
        bail!(REMOVED_MESSAGE)
    }

    pub async fn method_allowed(&self, _method: &str) -> Result<bool> {
        Ok(false)
    }

    pub async fn allow_upstream_proxy(&self) -> Result<bool> {
        Ok(false)
    }

    pub async fn allow_local_binding(&self) -> Result<bool> {
        Ok(false)
    }

    pub async fn network_mode(&self) -> Result<NetworkMode> {
        Ok(NetworkMode::default())
    }

    pub async fn set_network_mode(&self, _mode: NetworkMode) -> Result<()> {
        bail!(REMOVED_MESSAGE)
    }

    pub fn environment_id(&self) -> Option<&str> {
        self.environment_id.as_deref()
    }

    pub fn execution_id(&self) -> Option<String> {
        self.execution_id.clone()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostBlockReason {
    Denied,
    NotAllowed,
    NotAllowedLocal,
}

impl HostBlockReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Denied => "denied",
            Self::NotAllowed => "not_allowed",
            Self::NotAllowedLocal => "not_allowed_local",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostBlockDecision {
    Allowed,
    Blocked(HostBlockReason),
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedNetworkSandboxContext {
    #[serde(default)]
    pub loopback_ports: Vec<u16>,
    #[serde(default)]
    pub allow_local_binding: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedManagedNetwork {
    pub env: HashMap<String, String>,
    pub sandbox_context: ManagedNetworkSandboxContext,
}

#[derive(Debug, Default)]
pub struct Args {}

#[derive(Clone, Default)]
pub struct NetworkProxyBuilder {
    state: Option<Arc<NetworkProxyState>>,
    http_addr: Option<SocketAddr>,
    socks_addr: Option<SocketAddr>,
}

impl NetworkProxyBuilder {
    pub fn state(mut self, state: Arc<NetworkProxyState>) -> Self {
        self.state = Some(state);
        self
    }

    pub fn http_addr(mut self, address: SocketAddr) -> Self {
        self.http_addr = Some(address);
        self
    }

    pub fn socks_addr(mut self, address: SocketAddr) -> Self {
        self.socks_addr = Some(address);
        self
    }

    pub fn managed_by_codex(self, _managed: bool) -> Self {
        self
    }

    pub fn policy_decider<D: NetworkPolicyDecider>(self, _decider: D) -> Self {
        self
    }

    pub fn policy_decider_arc(self, _decider: Arc<dyn NetworkPolicyDecider>) -> Self {
        self
    }

    pub fn blocked_request_observer<O: BlockedRequestObserver>(self, _observer: O) -> Self {
        self
    }

    pub fn blocked_request_observer_arc(self, _observer: Arc<dyn BlockedRequestObserver>) -> Self {
        self
    }

    pub async fn build(self) -> Result<NetworkProxy> {
        let _ = self;
        bail!(REMOVED_MESSAGE)
    }
}

#[derive(Clone, Debug)]
pub struct NetworkProxy {
    state: Arc<NetworkProxyState>,
    http_addr: SocketAddr,
    socks_addr: SocketAddr,
}

impl PartialEq for NetworkProxy {
    fn eq(&self, other: &Self) -> bool {
        self.http_addr == other.http_addr && self.socks_addr == other.socks_addr
    }
}

impl Eq for NetworkProxy {}

impl NetworkProxy {
    pub fn builder() -> NetworkProxyBuilder {
        NetworkProxyBuilder::default()
    }

    pub fn http_addr(&self) -> SocketAddr {
        self.http_addr
    }

    pub fn socks_addr(&self) -> SocketAddr {
        self.socks_addr
    }

    pub fn network_proxy_restricting_sid(&self, _environment_id: Option<&str>) -> Option<String> {
        None
    }

    pub async fn current_cfg(&self) -> Result<NetworkProxyConfig> {
        self.state.current_cfg().await
    }

    pub async fn remote_launch_config(&self) -> Result<RemoteNetworkProxyLaunchConfig> {
        bail!(REMOVED_MESSAGE)
    }

    pub fn remote_policy_decider(&self) -> Option<Arc<dyn NetworkPolicyDecider>> {
        None
    }

    pub async fn add_allowed_domain(&self, host: &str) -> Result<()> {
        self.state.add_allowed_domain(host).await
    }

    pub async fn add_denied_domain(&self, host: &str) -> Result<()> {
        self.state.add_denied_domain(host).await
    }

    pub fn allow_local_binding(&self) -> bool {
        false
    }

    pub fn allow_unix_sockets(&self) -> Arc<[String]> {
        Arc::from(Vec::<String>::new())
    }

    pub fn dangerously_allow_all_unix_sockets(&self) -> bool {
        false
    }

    pub fn managed_mitm_ca_trust_bundle_path(&self) -> Option<AbsolutePathBuf> {
        None
    }

    pub fn apply_to_env(&self, _env: &mut HashMap<String, String>) {}

    pub fn apply_to_env_for_environment(
        &self,
        _env: &mut HashMap<String, String>,
        _environment_id: &str,
    ) -> Result<()> {
        bail!(REMOVED_MESSAGE)
    }

    pub fn apply_to_env_for_optional_environment(
        &self,
        env: &mut HashMap<String, String>,
        environment_id: Option<&str>,
    ) -> Result<()> {
        if environment_id.is_some() {
            bail!(REMOVED_MESSAGE)
        }
        self.apply_to_env(env);
        Ok(())
    }

    pub fn prepare_for_optional_environment(
        &self,
        _env: HashMap<String, String>,
        _environment_id: Option<&str>,
    ) -> Result<PreparedManagedNetwork> {
        bail!(REMOVED_MESSAGE)
    }

    pub fn prepare_for_remote_environment(
        &self,
        _env: HashMap<String, String>,
        _environment_id: &str,
    ) -> Result<PreparedManagedNetwork> {
        bail!(REMOVED_MESSAGE)
    }

    pub fn for_execution(
        &self,
        _environment_id: &str,
        _execution_id: &str,
        _attribution_token: String,
        _environment_policy: Option<EnvironmentNetworkPolicy>,
        _fallback_policy_decider: Option<Arc<dyn NetworkPolicyDecider>>,
    ) -> Result<Self> {
        bail!(REMOVED_MESSAGE)
    }

    pub async fn replace_config_state(&self, state: ConfigState) -> Result<()> {
        self.state.replace_config_state(state).await
    }

    pub async fn run(&self) -> Result<NetworkProxyHandle> {
        bail!(REMOVED_MESSAGE)
    }
}

#[derive(Debug)]
pub struct NetworkProxyHandle;

impl NetworkProxyHandle {
    pub async fn shutdown(self) -> Result<()> {
        Ok(())
    }
}

pub const PROXY_ATTRIBUTION_TOKEN_ENV_KEY: &str = "CODEX_NETWORK_PROXY_ATTRIBUTION";
pub const PROXY_ACTIVE_ENV_KEY: &str = "CODEX_NETWORK_PROXY_ACTIVE";
pub const ALLOW_LOCAL_BINDING_ENV_KEY: &str = "CODEX_NETWORK_ALLOW_LOCAL_BINDING";
pub const PROXY_URL_ENV_KEYS: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "WS_PROXY",
    "WSS_PROXY",
    "ALL_PROXY",
    "FTP_PROXY",
];
pub const ALL_PROXY_ENV_KEYS: &[&str] = &["ALL_PROXY", "all_proxy"];
pub const NO_PROXY_ENV_KEYS: &[&str] = &["NO_PROXY", "no_proxy"];
pub const PROXY_ENV_KEYS: &[&str] = &[
    PROXY_ACTIVE_ENV_KEY,
    ALLOW_LOCAL_BINDING_ENV_KEY,
    PROXY_ATTRIBUTION_TOKEN_ENV_KEY,
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "http_proxy",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
];
pub const DEFAULT_NO_PROXY_VALUE: &str = "localhost,127.0.0.1,::1";
pub const CUSTOM_CA_ENV_KEYS: [&str; 11] = [
    "CODEX_CA_CERTIFICATE",
    "SSL_CERT_FILE",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "NODE_EXTRA_CA_CERTS",
    "GIT_SSL_CAINFO",
    "CARGO_HTTP_CAINFO",
    "PIP_CERT",
    "BUNDLE_SSL_CA_CERT",
    "npm_config_cafile",
    "NPM_CONFIG_CAFILE",
];
#[cfg(target_os = "macos")]
pub const PROXY_GIT_SSH_COMMAND_ENV_KEY: &str = "GIT_SSH_COMMAND";
#[cfg(target_os = "macos")]
pub const CODEX_PROXY_GIT_SSH_COMMAND_MARKER: &str = "CODEX_PROXY_GIT_SSH_COMMAND=1 ";

pub fn proxy_url_env_value<'a>(
    env: &'a HashMap<String, String>,
    canonical_key: &str,
) -> Option<&'a str> {
    env.get(canonical_key).map(String::as_str).or_else(|| {
        env.get(&canonical_key.to_ascii_lowercase())
            .map(String::as_str)
    })
}

pub fn has_proxy_url_env_vars(env: &HashMap<String, String>) -> bool {
    PROXY_URL_ENV_KEYS
        .iter()
        .any(|key| proxy_url_env_value(env, key).is_some_and(|value| !value.trim().is_empty()))
}

pub fn is_managed_mitm_ca_trust_bundle_path(_path: &str) -> bool {
    false
}

pub fn is_managed_proxy_env_var(key: &str, value: &str) -> bool {
    PROXY_ENV_KEYS.contains(&key)
        || (CUSTOM_CA_ENV_KEYS.contains(&key) && is_managed_mitm_ca_trust_bundle_path(value))
}

pub fn strip_managed_proxy_env(env: &mut HashMap<String, String>) {
    env.retain(|key, value| !is_managed_proxy_env_var(key, value));
}

pub fn write_attribution_frame(writer: &mut impl Write, token: &str) -> io::Result<()> {
    if token.is_empty() || token.len() > 128 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid network proxy attribution token length",
        ));
    }
    writer.write_all(b"\0CDXPXY1")?;
    writer.write_all(&(token.len() as u16).to_be_bytes())?;
    writer.write_all(token.as_bytes())
}
