use super::*;
use crate::ModelsManagerConfig;
use crate::cache::FileModelsCache;
use crate::cache::ModelsCache;
use crate::cache::ModelsCacheEntry;
use crate::cache::ModelsCacheError;
use crate::cache::ModelsCacheFuture;
use ahead_model_auth::AheadAuth;
use ahead_model_auth::AuthManager;
use ahead_model_auth::ExternalAuth;
use chrono::Utc;
use codex_http_client::HttpClientFactory;
use codex_http_client::OutboundProxyPolicy;
use codex_protocol::openai_models::ModelsResponse;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use tempfile::tempdir;

#[path = "model_info_overrides_tests.rs"]
mod model_info_overrides_tests;

const DEFAULT_HTTP_CLIENT_FACTORY: HttpClientFactory =
    HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault);

fn remote_model(slug: &str, display: &str, priority: i32) -> ModelInfo {
    remote_model_with_visibility(slug, display, priority, "list")
}

fn remote_model_with_visibility(
    slug: &str,
    display: &str,
    priority: i32,
    visibility: &str,
) -> ModelInfo {
    serde_json::from_value(json!({
            "slug": slug,
            "display_name": display,
            "description": format!("{display} desc"),
            "default_reasoning_level": "medium",
            "supported_reasoning_levels": [{"effort": "low", "description": "low"}, {"effort": "medium", "description": "medium"}],
            "shell_type": "shell_command",
            "visibility": visibility,
            "minimal_client_version": [0, 1, 0],
            "supported_in_api": true,
            "priority": priority,
            "upgrade": null,
            "model_messages": {
                "instructions_template": "base instructions",
                "instructions_variables": null,
                "approvals": null,
                "permissions": null
            },
            "support_verbosity": false,
            "default_verbosity": null,
            "apply_patch_tool_type": null,
            "truncation_policy": {"mode": "bytes", "limit": 10_000},
            "supports_image_detail_original": false,
            "context_window": 272_000,
            "max_context_window": 272_000,
            "experimental_supported_tools": [],
        }))
        .expect("valid model")
}

fn assert_models_contain(actual: &[ModelInfo], expected: &[ModelInfo]) {
    for model in expected {
        assert!(
            actual.iter().any(|candidate| candidate.slug == model.slug),
            "expected model {} in cached list",
            model.slug
        );
    }
}

#[derive(Debug)]
struct TestModelsEndpoint {
    can_fetch_models: bool,
    responses: Mutex<VecDeque<Vec<ModelInfo>>>,
    fetch_count: AtomicUsize,
    observed_proxy_policy: Mutex<Option<OutboundProxyPolicy>>,
}

#[derive(Debug)]
struct TestModelsCache {
    entry: Mutex<Option<ModelsCacheEntry>>,
    load_error: bool,
    store_error: bool,
    stored_entries: Mutex<Vec<ModelsCacheEntry>>,
}

impl TestModelsCache {
    fn with_entry(entry: ModelsCacheEntry) -> Arc<Self> {
        Arc::new(Self {
            entry: Mutex::new(Some(entry)),
            load_error: false,
            store_error: false,
            stored_entries: Mutex::new(Vec::new()),
        })
    }

    fn failing(load_error: bool, store_error: bool) -> Arc<Self> {
        Arc::new(Self {
            entry: Mutex::new(None),
            load_error,
            store_error,
            stored_entries: Mutex::new(Vec::new()),
        })
    }

    fn stored_entries(&self) -> Vec<ModelsCacheEntry> {
        self.stored_entries
            .lock()
            .expect("stored entries lock should not be poisoned")
            .clone()
    }
}

impl ModelsCache for TestModelsCache {
    fn load<'a>(
        &'a self,
        _client_version: &'a str,
    ) -> ModelsCacheFuture<'a, Result<Option<ModelsCacheEntry>, ModelsCacheError>> {
        Box::pin(async move {
            if self.load_error {
                return Err(ModelsCacheError::new("test load failure"));
            }
            Ok(self
                .entry
                .lock()
                .expect("cache entry lock should not be poisoned")
                .clone())
        })
    }

    fn store<'a>(
        &'a self,
        entry: &'a ModelsCacheEntry,
    ) -> ModelsCacheFuture<'a, Result<(), ModelsCacheError>> {
        Box::pin(async move {
            if self.store_error {
                return Err(ModelsCacheError::new("test store failure"));
            }
            self.stored_entries
                .lock()
                .expect("stored entries lock should not be poisoned")
                .push(entry.clone());
            Ok(())
        })
    }

    fn refresh_ttl<'a>(
        &'a self,
        _client_version: &'a str,
    ) -> ModelsCacheFuture<'a, Result<(), ModelsCacheError>> {
        Box::pin(async move {
            let refreshed = {
                let mut entry = self
                    .entry
                    .lock()
                    .expect("cache entry lock should not be poisoned");
                let entry = entry
                    .as_mut()
                    .ok_or_else(|| ModelsCacheError::new("cache not found"))?;
                entry.fetched_at = Utc::now();
                entry.clone()
            };
            self.store(&refreshed).await
        })
    }
}

impl TestModelsEndpoint {
    fn new(responses: Vec<Vec<ModelInfo>>) -> Arc<Self> {
        Arc::new(Self {
            can_fetch_models: true,
            responses: Mutex::new(responses.into()),
            fetch_count: AtomicUsize::new(0),
            observed_proxy_policy: Mutex::new(None),
        })
    }

    fn without_refresh(responses: Vec<Vec<ModelInfo>>) -> Arc<Self> {
        Arc::new(Self {
            can_fetch_models: false,
            responses: Mutex::new(responses.into()),
            fetch_count: AtomicUsize::new(0),
            observed_proxy_policy: Mutex::new(None),
        })
    }

    fn fetch_count(&self) -> usize {
        self.fetch_count.load(Ordering::SeqCst)
    }

    fn observed_proxy_policy(&self) -> Option<OutboundProxyPolicy> {
        *self
            .observed_proxy_policy
            .lock()
            .expect("observed proxy policy lock should not be poisoned")
    }

    async fn list_models(&self) -> CoreResult<(Vec<ModelInfo>, Option<String>)> {
        self.fetch_count.fetch_add(1, Ordering::SeqCst);
        let models = self
            .responses
            .lock()
            .expect("responses lock should not be poisoned")
            .pop_front()
            .unwrap_or_default();
        Ok((models, None))
    }
}

#[derive(Debug)]
struct TestExternalApiKeyAuth;

impl ExternalAuth for TestExternalApiKeyAuth {
    fn resolve(&self) -> ahead_model_auth::ExternalAuthFuture<'_, AheadAuth> {
        Box::pin(async { Ok(AheadAuth::from_api_key("test-external-api-key")) })
    }

    fn refresh(&self) -> ahead_model_auth::ExternalAuthFuture<'_, AheadAuth> {
        Box::pin(async { Ok(AheadAuth::from_api_key("test-external-api-key")) })
    }
}

#[derive(Debug)]
struct TestUnresolvedExternalApiKeyAuth;

impl ExternalAuth for TestUnresolvedExternalApiKeyAuth {
    fn resolve(&self) -> ahead_model_auth::ExternalAuthFuture<'_, AheadAuth> {
        Box::pin(async { Err(std::io::Error::other("unresolved test auth")) })
    }

    fn refresh(&self) -> ahead_model_auth::ExternalAuthFuture<'_, AheadAuth> {
        Box::pin(async { Err(std::io::Error::other("unresolved test auth")) })
    }
}

impl ModelsEndpointClient for TestModelsEndpoint {
    fn can_fetch_models(&self) -> ModelsEndpointFuture<'_, bool> {
        Box::pin(async { self.can_fetch_models })
    }

    fn list_models<'a>(
        &'a self,
        _client_version: &'a str,
        http_client_factory: HttpClientFactory,
    ) -> ModelsEndpointFuture<'a, CoreResult<(Vec<ModelInfo>, Option<String>)>> {
        Box::pin(async move {
            *self
                .observed_proxy_policy
                .lock()
                .expect("observed proxy policy lock should not be poisoned") =
                Some(http_client_factory.outbound_proxy_policy());
            TestModelsEndpoint::list_models(self).await
        })
    }
}

fn openai_manager_for_tests(
    codex_home: std::path::PathBuf,
    endpoint_client: Arc<dyn ModelsEndpointClient>,
) -> OpenAiModelsManager {
    OpenAiModelsManager::new(codex_home, endpoint_client)
}

async fn mutate_file_cache_for_test<F>(codex_home: &Path, f: F)
where
    F: FnOnce(&mut ModelsCacheEntry),
{
    let client_version = crate::client_version_to_whole();
    let cache = FileModelsCache::new(codex_home.join(MODEL_CACHE_FILE), DEFAULT_MODEL_CACHE_TTL);
    let mut entry = cache
        .load(&client_version)
        .await
        .expect("cache load succeeds")
        .expect("cache entry exists");
    f(&mut entry);
    cache.store(&entry).await.expect("cache store succeeds");
}

fn static_manager_for_tests(model_catalog: ModelsResponse) -> StaticModelsManager {
    StaticModelsManager::new(model_catalog)
}

#[tokio::test]
async fn file_cache_implements_models_cache_contract() {
    let codex_home = tempdir().expect("temp dir");
    let cache = FileModelsCache::new(
        codex_home.path().join(MODEL_CACHE_FILE),
        DEFAULT_MODEL_CACHE_TTL,
    );
    let client_version = crate::client_version_to_whole();
    let entry = ModelsCacheEntry {
        fetched_at: Utc::now(),
        etag: Some("file-etag".to_string()),
        client_version: Some(client_version.clone()),
        models: vec![remote_model(
            "file-cached",
            "File Cached",
            /*priority*/ 0,
        )],
    };

    cache.store(&entry).await.expect("cache store succeeds");

    assert_eq!(
        cache
            .load(&client_version)
            .await
            .expect("cache load succeeds"),
        Some(entry)
    );
}

#[tokio::test]
async fn file_cache_refresh_ttl_renews_expired_entry_without_serving_it_stale() {
    let codex_home = tempdir().expect("temp dir");
    let cache = FileModelsCache::new(
        codex_home.path().join(MODEL_CACHE_FILE),
        DEFAULT_MODEL_CACHE_TTL,
    );
    let client_version = crate::client_version_to_whole();
    let expired_at = Utc::now() - chrono::Duration::hours(1);
    let entry = ModelsCacheEntry {
        fetched_at: expired_at,
        etag: Some("expired-etag".to_string()),
        client_version: Some(client_version.clone()),
        models: vec![remote_model(
            "expired-file-cache",
            "Expired File Cache",
            /*priority*/ 0,
        )],
    };
    cache.store(&entry).await.expect("cache store succeeds");

    assert_eq!(
        cache
            .load(&client_version)
            .await
            .expect("cache load succeeds"),
        None,
        "an expired entry must not be returned before revalidation"
    );

    cache
        .refresh_ttl(&client_version)
        .await
        .expect("TTL refresh succeeds");

    let refreshed = cache
        .load(&client_version)
        .await
        .expect("cache load succeeds")
        .expect("revalidated entry is fresh");
    assert!(refreshed.fetched_at > expired_at);
    let mut expected = entry;
    expected.fetched_at = refreshed.fetched_at;
    assert_eq!(refreshed, expected);
}

#[tokio::test]
async fn manager_without_cache_reuses_a_recent_catalog_for_online_if_uncached() {
    let remote_models = vec![remote_model("remote", "Remote", /*priority*/ 0)];
    let endpoint = TestModelsEndpoint::new(vec![remote_models.clone(), remote_models.clone()]);
    let manager = OpenAiModelsManager::new_without_cache(endpoint.clone());
    let mut expected = load_remote_models_from_file().expect("bundled models should parse");
    expected.extend(remote_models);

    let catalog = manager
        .raw_model_catalog(
            RefreshStrategy::OnlineIfUncached,
            DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await;
    let second_catalog = manager
        .raw_model_catalog(
            RefreshStrategy::OnlineIfUncached,
            DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await;

    assert_eq!(catalog.models, expected);
    assert_eq!(second_catalog, catalog);
    assert_eq!(manager.get_remote_models().await, expected);
    assert_eq!(endpoint.fetch_count(), 1);
}

#[tokio::test]
async fn online_refresh_bypasses_the_in_memory_catalog_ttl() {
    let first_models = vec![remote_model("first", "First", /*priority*/ 0)];
    let second_models = vec![remote_model("second", "Second", /*priority*/ 0)];
    let endpoint = TestModelsEndpoint::new(vec![first_models, second_models.clone()]);
    let manager = OpenAiModelsManager::new_without_cache(endpoint.clone());

    manager
        .raw_model_catalog(
            RefreshStrategy::OnlineIfUncached,
            DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await;
    let refreshed = manager
        .raw_model_catalog(RefreshStrategy::Online, DEFAULT_HTTP_CLIENT_FACTORY)
        .await;

    assert!(refreshed.models.iter().any(|model| model.slug == "second"));
    assert_eq!(endpoint.fetch_count(), 2);
}

#[tokio::test]
async fn model_info_uses_the_refreshed_provider_catalog() {
    let dynamic_slug = "provider/model-with-reasoning-metadata";
    let endpoint = TestModelsEndpoint::new(vec![vec![remote_model(
        dynamic_slug,
        "Provider Model",
        /*priority*/ 0,
    )]]);
    let manager = OpenAiModelsManager::new_without_cache(endpoint.clone());

    manager
        .raw_model_catalog(
            RefreshStrategy::OnlineIfUncached,
            DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await;
    let model = manager
        .get_model_info(dynamic_slug, &ModelsManagerConfig::default())
        .await;

    assert!(!model.used_fallback_model_metadata);
    assert_eq!(model.display_name, "Provider Model");
    assert_eq!(model.context_window, Some(272_000));
    assert_eq!(model.supported_reasoning_levels.len(), 2);
    assert_eq!(endpoint.fetch_count(), 1);
}

#[tokio::test]
async fn injected_cache_hit_avoids_remote_fetch() {
    let cached_models = vec![remote_model("cached", "Cached", /*priority*/ 0)];
    let cache = TestModelsCache::with_entry(ModelsCacheEntry {
        fetched_at: Utc::now(),
        etag: Some("cached-etag".to_string()),
        client_version: Some(crate::client_version_to_whole()),
        models: cached_models.clone(),
    });
    let endpoint = TestModelsEndpoint::new(vec![vec![remote_model(
        "remote", "Remote", /*priority*/ 0,
    )]]);
    let manager = OpenAiModelsManager::new_with_cache(cache, endpoint.clone());
    let mut expected = load_remote_models_from_file().expect("bundled models should parse");
    expected.extend(cached_models);

    let catalog = manager
        .raw_model_catalog(
            RefreshStrategy::OnlineIfUncached,
            DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await;

    assert_eq!(catalog.models, expected);
    assert_eq!(endpoint.fetch_count(), 0);
}

#[tokio::test]
async fn injected_cache_read_error_falls_back_and_persists_remote_models() {
    let remote_models = vec![remote_model("remote", "Remote", /*priority*/ 0)];
    let cache = TestModelsCache::failing(/*load_error*/ true, /*store_error*/ false);
    let endpoint = TestModelsEndpoint::new(vec![remote_models.clone()]);
    let manager = OpenAiModelsManager::new_with_cache(cache.clone(), endpoint.clone());
    let mut expected = load_remote_models_from_file().expect("bundled models should parse");
    expected.extend(remote_models.clone());

    let catalog = manager
        .raw_model_catalog(
            RefreshStrategy::OnlineIfUncached,
            DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await;

    assert_eq!(catalog.models, expected);
    assert_eq!(endpoint.fetch_count(), 1);
    let stored_entries = cache.stored_entries();
    assert_eq!(
        stored_entries,
        vec![ModelsCacheEntry {
            fetched_at: stored_entries[0].fetched_at,
            etag: None,
            client_version: Some(crate::client_version_to_whole()),
            models: remote_models,
        }]
    );
}

#[tokio::test]
async fn injected_cache_write_error_does_not_fail_remote_refresh() {
    let remote_models = vec![remote_model("remote", "Remote", /*priority*/ 0)];
    let cache = TestModelsCache::failing(/*load_error*/ false, /*store_error*/ true);
    let endpoint = TestModelsEndpoint::new(vec![remote_models.clone()]);
    let manager = OpenAiModelsManager::new_with_cache(cache, endpoint.clone());
    let mut expected = load_remote_models_from_file().expect("bundled models should parse");
    expected.extend(remote_models);

    let catalog = manager
        .raw_model_catalog(
            RefreshStrategy::OnlineIfUncached,
            DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await;

    assert_eq!(catalog.models, expected);
    assert_eq!(endpoint.fetch_count(), 1);
}

#[tokio::test]
async fn injected_cache_ttl_refresh_preserves_cached_payload() {
    let cached_models = vec![remote_model("cached", "Cached", /*priority*/ 0)];
    let cached_at = Utc::now() - chrono::Duration::minutes(1);
    let cache = TestModelsCache::with_entry(ModelsCacheEntry {
        fetched_at: cached_at,
        etag: Some("cached-etag".to_string()),
        client_version: Some(crate::client_version_to_whole()),
        models: cached_models.clone(),
    });
    let manager =
        OpenAiModelsManager::new_with_cache(cache.clone(), TestModelsEndpoint::new(Vec::new()));

    manager
        .raw_model_catalog(
            RefreshStrategy::OnlineIfUncached,
            DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await;
    manager
        .refresh_if_new_etag("cached-etag".to_string(), DEFAULT_HTTP_CLIENT_FACTORY)
        .await;

    let stored_entries = cache.stored_entries();
    assert_eq!(stored_entries.len(), 1);
    assert!(stored_entries[0].fetched_at > cached_at);
    assert_eq!(stored_entries[0].etag.as_deref(), Some("cached-etag"));
    assert_eq!(
        stored_entries[0].client_version,
        Some(crate::client_version_to_whole())
    );
    assert_eq!(stored_entries[0].models, cached_models);
}

#[tokio::test]
async fn static_manager_preserves_supported_requested_model_when_fallback_is_allowed() {
    let manager = static_manager_for_tests(ModelsResponse {
        models: vec![
            remote_model("provider-default", "Default", /*priority*/ 0),
            remote_model("provider-supported", "Supported", /*priority*/ 1),
        ],
    });
    let requested_model = Some("provider-supported".to_string());

    let model = manager
        .get_default_model(
            &requested_model,
            /*allow_provider_model_fallback*/ true,
            RefreshStrategy::Offline,
            DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await;

    assert_eq!(model, "provider-supported");
}

#[tokio::test]
async fn static_manager_falls_back_from_unsupported_requested_model_when_allowed() {
    let manager = static_manager_for_tests(ModelsResponse {
        models: vec![
            remote_model("provider-default", "Default", /*priority*/ 0),
            remote_model("provider-supported", "Supported", /*priority*/ 1),
        ],
    });
    let requested_model = Some("unsupported".to_string());

    let model = manager
        .get_default_model(
            &requested_model,
            /*allow_provider_model_fallback*/ true,
            RefreshStrategy::Offline,
            DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await;

    assert_eq!(model, "provider-default");
}

#[tokio::test]
async fn static_manager_preserves_unsupported_requested_model_when_fallback_is_disabled() {
    let manager = static_manager_for_tests(ModelsResponse {
        models: vec![remote_model(
            "provider-default",
            "Default",
            /*priority*/ 0,
        )],
    });
    let requested_model = Some("unsupported".to_string());

    let model = manager
        .get_default_model(
            &requested_model,
            /*allow_provider_model_fallback*/ false,
            RefreshStrategy::Offline,
            DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await;

    assert_eq!(model, "unsupported");
}

#[tokio::test]
async fn static_manager_uses_empty_default_when_fallback_is_allowed_and_catalog_is_empty() {
    let manager = static_manager_for_tests(ModelsResponse { models: Vec::new() });
    let requested_model = Some("unsupported".to_string());

    let model = manager
        .get_default_model(
            &requested_model,
            /*allow_provider_model_fallback*/ true,
            RefreshStrategy::Offline,
            DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await;

    assert_eq!(model, "");
}

#[tokio::test]
async fn dynamic_manager_preserves_requested_model_when_fallback_is_allowed() {
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(Vec::new());
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint.clone());
    let requested_model = Some("unsupported".to_string());

    let model = manager
        .get_default_model(
            &requested_model,
            /*allow_provider_model_fallback*/ true,
            RefreshStrategy::Online,
            DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await;

    assert_eq!(model, "unsupported");
    assert_eq!(endpoint.fetch_count(), 0);
}

#[tokio::test]
async fn get_model_info_tracks_fallback_usage() {
    let codex_home = tempdir().expect("temp dir");
    let config = ModelsManagerConfig::default();
    let manager = openai_manager_for_tests(
        codex_home.path().to_path_buf(),
        TestModelsEndpoint::new(Vec::new()),
    );
    let known_slug = manager
        .get_remote_models()
        .await
        .first()
        .expect("bundled models should include at least one model")
        .slug
        .clone();

    let known = manager.get_model_info(known_slug.as_str(), &config).await;
    assert!(!known.used_fallback_model_metadata);
    assert_eq!(known.slug, known_slug);

    let unknown = manager
        .get_model_info("model-that-does-not-exist", &config)
        .await;
    assert!(unknown.used_fallback_model_metadata);
    assert_eq!(unknown.slug, "model-that-does-not-exist");
}

#[tokio::test]
async fn get_model_info_applies_long_context_override_to_bundled_gpt_5_6_models() {
    let codex_home = tempdir().expect("temp dir");
    let manager = openai_manager_for_tests(
        codex_home.path().to_path_buf(),
        TestModelsEndpoint::new(Vec::new()),
    );
    let config = ModelsManagerConfig {
        model_context_window: Some(1_000_000),
        ..Default::default()
    };

    for slug in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
        let model_info = manager.get_model_info(slug, &config).await;
        let mut expected = manager
            .get_model_info(slug, &ModelsManagerConfig::default())
            .await;
        expected.context_window = Some(872_000);

        assert_eq!(model_info, expected);
    }
}

#[tokio::test]
async fn get_model_info_uses_custom_catalog() {
    let config = ModelsManagerConfig::default();
    let mut overlay = remote_model("gpt-overlay", "Overlay", /*priority*/ 0);
    overlay.supports_image_detail_original = true;

    let manager = static_manager_for_tests(ModelsResponse {
        models: vec![overlay],
    });

    let model_info = manager
        .get_model_info("gpt-overlay-experiment", &config)
        .await;

    assert_eq!(model_info.slug, "gpt-overlay-experiment");
    assert_eq!(model_info.display_name, "Overlay");
    assert_eq!(model_info.context_window, Some(272_000));
    assert!(model_info.supports_image_detail_original);
    assert!(!model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn get_model_info_matches_namespaced_suffix() {
    let config = ModelsManagerConfig::default();
    let mut remote = remote_model("gpt-image", "Image", /*priority*/ 0);
    remote.supports_image_detail_original = true;
    let manager = static_manager_for_tests(ModelsResponse {
        models: vec![remote],
    });
    let namespaced_model = "custom/gpt-image".to_string();

    let model_info = manager.get_model_info(&namespaced_model, &config).await;

    assert_eq!(model_info.slug, namespaced_model);
    assert!(model_info.supports_image_detail_original);
    assert!(!model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn get_model_info_matches_hyphenated_provider_namespace_suffix() {
    let config = ModelsManagerConfig::default();
    let remote = remote_model("gpt-image", "Image", /*priority*/ 0);
    let manager = static_manager_for_tests(ModelsResponse {
        models: vec![remote],
    });
    let namespaced_model = "openai-codex/gpt-image".to_string();

    let model_info = manager.get_model_info(&namespaced_model, &config).await;

    assert_eq!(model_info.slug, namespaced_model);
    assert!(!model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn get_model_info_rejects_multi_segment_namespace_suffix_matching() {
    let codex_home = tempdir().expect("temp dir");
    let config = ModelsManagerConfig::default();
    let manager = openai_manager_for_tests(
        codex_home.path().to_path_buf(),
        TestModelsEndpoint::new(Vec::new()),
    );
    let known_slug = manager
        .get_remote_models()
        .await
        .first()
        .expect("bundled models should include at least one model")
        .slug
        .clone();
    let namespaced_model = format!("ns1/ns2/{known_slug}");

    let model_info = manager.get_model_info(&namespaced_model, &config).await;

    assert_eq!(model_info.slug, namespaced_model);
    assert!(model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn refresh_available_models_sorts_by_priority() {
    let remote_models = vec![
        remote_model("priority-low", "Low", /*priority*/ 1),
        remote_model("priority-high", "High", /*priority*/ 0),
    ];
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![remote_models.clone()]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint.clone());

    let available = manager
        .list_models(
            RefreshStrategy::Online,
            HttpClientFactory::new(OutboundProxyPolicy::RespectSystemProxy),
        )
        .await;
    assert_models_contain(&manager.get_remote_models().await, &remote_models);
    assert_eq!(
        endpoint.observed_proxy_policy(),
        Some(OutboundProxyPolicy::RespectSystemProxy)
    );
    let high_idx = available
        .iter()
        .position(|model| model.model == "priority-high")
        .expect("priority-high should be listed");
    let low_idx = available
        .iter()
        .position(|model| model.model == "priority-low")
        .expect("priority-low should be listed");
    assert!(
        high_idx < low_idx,
        "higher priority should be listed before lower priority"
    );
    assert_eq!(endpoint.fetch_count(), 1, "expected a single model fetch");
}

#[tokio::test]
async fn refresh_available_models_merges_remote_catalog_with_bundled_models() {
    let remote_models = vec![remote_model(
        "provider-visible-model",
        "Provider Visible",
        /*priority*/ 0,
    )];
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![remote_models.clone()]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint.clone());

    manager
        .refresh_available_models(
            RefreshStrategy::OnlineIfUncached,
            &DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await
        .expect("refresh succeeds");

    assert_models_contain(&manager.get_remote_models().await, &remote_models);
    assert_eq!(endpoint.fetch_count(), 1, "expected a single model fetch");
}

#[tokio::test]
async fn refresh_available_models_merges_cached_remote_models_with_bundled_models() {
    let remote_models = vec![remote_model(
        "provider-cached-model",
        "Provider Cached",
        /*priority*/ 0,
    )];
    let codex_home = tempdir().expect("temp dir");
    let fetch_endpoint = TestModelsEndpoint::new(vec![remote_models.clone()]);
    let fetch_manager =
        openai_manager_for_tests(codex_home.path().to_path_buf(), fetch_endpoint.clone());

    fetch_manager
        .refresh_available_models(
            RefreshStrategy::OnlineIfUncached,
            &DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await
        .expect("initial refresh succeeds");

    let cache_endpoint = TestModelsEndpoint::new(Vec::new());
    let cache_manager =
        openai_manager_for_tests(codex_home.path().to_path_buf(), cache_endpoint.clone());

    cache_manager
        .refresh_available_models(
            RefreshStrategy::OnlineIfUncached,
            &DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await
        .expect("cached refresh succeeds");

    assert_models_contain(&cache_manager.get_remote_models().await, &remote_models);
    assert_eq!(
        cache_endpoint.fetch_count(),
        0,
        "fresh cache should avoid a model fetch"
    );
}

#[tokio::test]
async fn get_model_info_keeps_bundled_metadata_when_remote_models_are_merged() {
    let remote_models = vec![remote_model(
        "provider-remote-model-info",
        "Provider Model Info",
        /*priority*/ 0,
    )];
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![remote_models]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint);
    let bundled_slug = load_remote_models_from_file()
        .expect("bundled models should parse")
        .first()
        .expect("bundled models should contain at least one model")
        .slug
        .clone();

    manager
        .refresh_available_models(
            RefreshStrategy::OnlineIfUncached,
            &DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await
        .expect("refresh succeeds");

    let model_info = manager
        .get_model_info(&bundled_slug, &ModelsManagerConfig::default())
        .await;

    assert_eq!(model_info.slug, bundled_slug);
    assert!(!model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn refresh_available_models_preserves_bundled_catalog_for_empty_remote() {
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![Vec::new()]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint);
    let expected = load_remote_models_from_file().expect("bundled models should parse");

    manager
        .refresh_available_models(
            RefreshStrategy::OnlineIfUncached,
            &DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await
        .expect("refresh succeeds");

    assert_eq!(manager.get_remote_models().await, expected);
}

#[tokio::test]
async fn refresh_available_models_merges_hidden_only_remote_with_bundled_catalog() {
    let hidden_remote = remote_model_with_visibility(
        "provider-hidden-only",
        "Provider Hidden",
        /*priority*/ 0,
        "hide",
    );
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![vec![hidden_remote.clone()]]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint);
    let mut expected = load_remote_models_from_file().expect("bundled models should parse");
    expected.push(hidden_remote);

    manager
        .refresh_available_models(
            RefreshStrategy::OnlineIfUncached,
            &DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await
        .expect("refresh succeeds");

    assert_eq!(manager.get_remote_models().await, expected);
}

#[tokio::test]
async fn refresh_available_models_keeps_merging_for_authorized_endpoint() {
    let remote_models = vec![remote_model(
        "api-auth-visible-remote",
        "API Auth Visible",
        /*priority*/ 0,
    )];
    let codex_home = tempdir().expect("temp dir");
    let endpoint = Arc::new(TestModelsEndpoint {
        can_fetch_models: true,
        responses: Mutex::new(vec![remote_models.clone()].into()),
        fetch_count: AtomicUsize::new(0),
        observed_proxy_policy: Mutex::new(None),
    });
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint.clone());
    let mut expected = load_remote_models_from_file().expect("bundled models should parse");
    expected.extend(remote_models);

    manager
        .refresh_available_models(
            RefreshStrategy::OnlineIfUncached,
            &DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await
        .expect("refresh succeeds");

    assert_eq!(manager.get_remote_models().await, expected);
    assert_eq!(endpoint.fetch_count(), 1, "expected a single model fetch");
}

#[tokio::test]
async fn refresh_available_models_uses_cache_when_fresh() {
    let remote_models = vec![remote_model("cached", "Cached", /*priority*/ 5)];
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![remote_models.clone()]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint.clone());

    manager
        .refresh_available_models(
            RefreshStrategy::OnlineIfUncached,
            &DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await
        .expect("first refresh succeeds");
    assert_models_contain(&manager.get_remote_models().await, &remote_models);

    // Second call should read from cache and avoid the network.
    manager
        .refresh_available_models(
            RefreshStrategy::OnlineIfUncached,
            &DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await
        .expect("cached refresh succeeds");
    assert_models_contain(&manager.get_remote_models().await, &remote_models);
    assert_eq!(
        endpoint.fetch_count(),
        1,
        "cache hit should avoid a second model fetch"
    );
}

#[tokio::test]
async fn refresh_available_models_refetches_when_cache_stale() {
    let initial_models = vec![remote_model("stale", "Stale", /*priority*/ 1)];
    let codex_home = tempdir().expect("temp dir");
    let updated_models = vec![remote_model("fresh", "Fresh", /*priority*/ 9)];
    let endpoint = TestModelsEndpoint::new(vec![initial_models.clone(), updated_models.clone()]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint.clone());

    manager
        .refresh_available_models(
            RefreshStrategy::OnlineIfUncached,
            &DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await
        .expect("initial refresh succeeds");

    // Rewrite cache with an old timestamp so it is treated as stale.
    mutate_file_cache_for_test(codex_home.path(), |cache| {
        cache.fetched_at = Utc::now() - chrono::Duration::hours(1);
    })
    .await;

    manager
        .refresh_available_models(
            RefreshStrategy::OnlineIfUncached,
            &DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await
        .expect("second refresh succeeds");
    assert_models_contain(&manager.get_remote_models().await, &updated_models);
    assert_eq!(
        endpoint.fetch_count(),
        2,
        "stale cache refresh should fetch models again"
    );
}

#[tokio::test]
async fn refresh_available_models_refetches_when_version_mismatch() {
    let initial_models = vec![remote_model("old", "Old", /*priority*/ 1)];
    let codex_home = tempdir().expect("temp dir");
    let updated_models = vec![remote_model("new", "New", /*priority*/ 2)];
    let endpoint = TestModelsEndpoint::new(vec![initial_models.clone(), updated_models.clone()]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint.clone());

    manager
        .refresh_available_models(
            RefreshStrategy::OnlineIfUncached,
            &DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await
        .expect("initial refresh succeeds");

    mutate_file_cache_for_test(codex_home.path(), |cache| {
        let client_version = crate::client_version_to_whole();
        cache.client_version = Some(format!("{client_version}-mismatch"));
    })
    .await;

    manager
        .refresh_available_models(
            RefreshStrategy::OnlineIfUncached,
            &DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await
        .expect("second refresh succeeds");
    assert_models_contain(&manager.get_remote_models().await, &updated_models);
    assert_eq!(
        endpoint.fetch_count(),
        2,
        "version mismatch should fetch models again"
    );
}

#[tokio::test]
async fn refresh_available_models_drops_removed_remote_models() {
    let initial_models = vec![remote_model(
        "remote-old",
        "Remote Old",
        /*priority*/ 1,
    )];
    let codex_home = tempdir().expect("temp dir");
    let refreshed_models = vec![remote_model(
        "remote-new",
        "Remote New",
        /*priority*/ 1,
    )];
    let endpoint = TestModelsEndpoint::new(vec![initial_models, refreshed_models]);
    let manager = OpenAiModelsManager::new_with_cache(
        Arc::new(FileModelsCache::new(
            codex_home.path().join(MODEL_CACHE_FILE),
            Duration::ZERO,
        )),
        endpoint.clone(),
    );

    manager
        .refresh_available_models(
            RefreshStrategy::OnlineIfUncached,
            &DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await
        .expect("initial refresh succeeds");

    manager
        .refresh_available_models(
            RefreshStrategy::OnlineIfUncached,
            &DEFAULT_HTTP_CLIENT_FACTORY,
        )
        .await
        .expect("second refresh succeeds");

    let available = manager
        .try_list_models()
        .expect("models should be available");
    assert!(
        available.iter().any(|preset| preset.model == "remote-new"),
        "new remote model should be listed"
    );
    assert!(
        !available.iter().any(|preset| preset.model == "remote-old"),
        "removed remote model should not be listed"
    );
    assert_eq!(
        endpoint.fetch_count(),
        2,
        "second refresh should fetch models again"
    );
}

#[tokio::test]
async fn refresh_available_models_skips_network_without_auth() {
    let dynamic_slug = "dynamic-model-only-for-test-noauth";
    let codex_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::without_refresh(vec![vec![remote_model(
        dynamic_slug,
        "No Auth",
        /*priority*/ 1,
    )]]);
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint.clone());

    manager
        .refresh_available_models(RefreshStrategy::Online, &DEFAULT_HTTP_CLIENT_FACTORY)
        .await
        .expect("refresh should no-op without auth");
    let cached_remote = manager.get_remote_models().await;
    assert!(
        !cached_remote
            .iter()
            .any(|candidate| candidate.slug == dynamic_slug),
        "remote refresh should be skipped without auth"
    );
    assert_eq!(
        endpoint.fetch_count(),
        0,
        "endpoint that cannot refresh should avoid model fetches"
    );
}

#[derive(Debug)]
struct TestAuthAwareModelsEndpoint {
    auth_manager: Option<Arc<AuthManager>>,
    responses: Mutex<VecDeque<Vec<ModelInfo>>>,
    fetch_count: AtomicUsize,
}

impl TestAuthAwareModelsEndpoint {
    fn new(auth_manager: Option<Arc<AuthManager>>, responses: Vec<Vec<ModelInfo>>) -> Arc<Self> {
        Arc::new(Self {
            auth_manager,
            responses: Mutex::new(responses.into()),
            fetch_count: AtomicUsize::new(0),
        })
    }

    fn fetch_count(&self) -> usize {
        self.fetch_count.load(Ordering::SeqCst)
    }

    async fn can_fetch_models(&self) -> bool {
        match self.auth_manager.as_ref() {
            Some(auth_manager) => auth_manager.auth().await.is_some(),
            None => false,
        }
    }

    async fn list_models(&self) -> CoreResult<(Vec<ModelInfo>, Option<String>)> {
        self.fetch_count.fetch_add(1, Ordering::SeqCst);
        let models = self
            .responses
            .lock()
            .expect("responses lock should not be poisoned")
            .pop_front()
            .unwrap_or_default();
        Ok((models, None))
    }
}

impl ModelsEndpointClient for TestAuthAwareModelsEndpoint {
    fn can_fetch_models(&self) -> ModelsEndpointFuture<'_, bool> {
        Box::pin(TestAuthAwareModelsEndpoint::can_fetch_models(self))
    }

    fn list_models<'a>(
        &'a self,
        _client_version: &'a str,
        _http_client_factory: HttpClientFactory,
    ) -> ModelsEndpointFuture<'a, CoreResult<(Vec<ModelInfo>, Option<String>)>> {
        Box::pin(TestAuthAwareModelsEndpoint::list_models(self))
    }
}

#[tokio::test]
async fn refresh_available_models_fetches_with_resolved_external_api_key() {
    let dynamic_slug = "dynamic-model-only-for-test-external-api-key";
    let codex_home = tempdir().expect("temp dir");
    let auth_manager =
        AuthManager::from_auth_for_testing(AheadAuth::from_api_key("cached-provider-api-key"));
    auth_manager
        .set_external_auth(Arc::new(TestExternalApiKeyAuth))
        .await
        .expect("external API key auth should resolve");
    let endpoint = TestAuthAwareModelsEndpoint::new(
        Some(Arc::clone(&auth_manager)),
        vec![vec![remote_model(
            dynamic_slug,
            "External API Key",
            /*priority*/ 1,
        )]],
    );
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint.clone());

    manager
        .refresh_available_models(RefreshStrategy::Online, &DEFAULT_HTTP_CLIENT_FACTORY)
        .await
        .expect("refresh should fetch with resolved API key auth");
    let cached_remote = manager.get_remote_models().await;

    assert!(
        cached_remote
            .iter()
            .any(|candidate| candidate.slug == dynamic_slug),
        "remote refresh should include models returned with external API key auth"
    );
    assert_eq!(
        endpoint.fetch_count(),
        1,
        "endpoint should fetch models when external API key auth is active"
    );
}

#[tokio::test]
async fn refresh_available_models_uses_cached_provider_key_when_external_override_is_unresolved() {
    let dynamic_slug = "dynamic-model-only-for-test-unresolved-external-api-key";
    let codex_home = tempdir().expect("temp dir");
    let auth_manager =
        AuthManager::from_auth_for_testing(AheadAuth::from_api_key("cached-provider-api-key"));
    auth_manager
        .set_external_auth(Arc::new(TestUnresolvedExternalApiKeyAuth))
        .await
        .expect_err("unresolved external auth should be rejected");
    let endpoint = TestAuthAwareModelsEndpoint::new(
        Some(Arc::clone(&auth_manager)),
        vec![vec![remote_model(
            dynamic_slug,
            "Unresolved External API Key",
            /*priority*/ 1,
        )]],
    );
    let manager = openai_manager_for_tests(codex_home.path().to_path_buf(), endpoint.clone());

    manager
        .refresh_available_models(RefreshStrategy::Online, &DEFAULT_HTTP_CLIENT_FACTORY)
        .await
        .expect("refresh should use cached provider-key auth");

    assert!(
        manager
            .get_remote_models()
            .await
            .iter()
            .any(|candidate| candidate.slug == dynamic_slug),
        "remote refresh should include models fetched with cached provider-key auth"
    );
    assert_eq!(
        endpoint.fetch_count(),
        1,
        "endpoint should fetch models when unresolved external auth falls back to the cached provider key"
    );
}

#[test]
fn build_available_models_picks_default_after_hiding_hidden_models() {
    let manager = static_manager_for_tests(ModelsResponse { models: Vec::new() });

    let hidden_model =
        remote_model_with_visibility("hidden", "Hidden", /*priority*/ 0, "hide");
    let visible_model =
        remote_model_with_visibility("visible", "Visible", /*priority*/ 1, "list");

    let expected_hidden = ModelPreset::from(hidden_model.clone());
    let mut expected_visible = ModelPreset::from(visible_model.clone());
    expected_visible.is_default = true;

    let available = manager.build_available_models(vec![hidden_model, visible_model]);

    assert_eq!(available, vec![expected_hidden, expected_visible]);
}

#[tokio::test]
async fn static_manager_hides_models_unsupported_by_api() {
    let backend_only_model = {
        let mut model = remote_model("backend-only", "Backend Only", /*priority*/ 0);
        model.supported_in_api = false;
        model
    };
    let api_model = remote_model("api-model", "API Model", /*priority*/ 1);
    let manager = StaticModelsManager::new(ModelsResponse {
        models: vec![backend_only_model, api_model],
    });

    let models = manager
        .list_models(RefreshStrategy::Online, DEFAULT_HTTP_CLIENT_FACTORY)
        .await;
    assert_eq!(
        models
            .iter()
            .map(|model| model.model.as_str())
            .collect::<Vec<_>>(),
        vec!["api-model"]
    );
}

#[test]
fn bundled_models_json_roundtrips() {
    let response = crate::bundled_models_response()
        .unwrap_or_else(|err| panic!("bundled models.json should parse: {err}"));

    let serialized =
        serde_json::to_string(&response).expect("bundled models.json should serialize");
    let roundtripped: ModelsResponse =
        serde_json::from_str(&serialized).expect("serialized models.json should deserialize");

    assert_eq!(
        response, roundtripped,
        "bundled models.json should round trip through serde"
    );
    assert!(
        !response.models.is_empty(),
        "bundled models.json should contain at least one model"
    );
    assert!(
        response.models.iter().all(|model| model
            .model_messages
            .as_ref()
            .is_none_or(|messages| messages.instructions_template.is_none())),
        "bundled models should use the shared AHEAD instruction template"
    );
    let sol = serde_json::to_value(
        response
            .models
            .iter()
            .find(|model| model.slug == "gpt-5.6-sol")
            .expect("catalog should retain GPT-5.6-Sol"),
    )
    .expect("serialize retained model metadata");
    assert_eq!(sol["tool_mode"], "code_mode_only");
    assert!(sol["model_messages"]["token_budget"].is_object());
}
