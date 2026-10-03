use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use codex_protocol::{ThreadId, protocol::ThreadHistoryMode};
use codex_rollout::{
    ModelContextScan, ModelContextScanProgress, RolloutItem, persisted_rollout_items,
};
use codex_thread_store::{
    AppendThreadItemsParams, ArchiveThreadParams, CreateThreadParams,
    DeleteThreadParams, InMemoryThreadStore, ListThreadsParams,
    LoadThreadHistoryParams, PersistContext, ReadThreadParams, ResumeThreadParams,
    SortDirection, StoredModelContext, StoredThread, StoredThreadHistory,
    ThreadMetadataPatch, ThreadPage, ThreadRelationFilter, ThreadSortKey,
    ThreadStore, ThreadStoreError, ThreadStoreFuture, ThreadStoreResult,
    UpdateThreadMetadataParams, canonical_session_meta_line,
};
use parking_lot::Mutex;

use crate::{HarnessStore, NativeThreadHeader};
use crate::{
    NativeThreadHeaderPageRequest, NativeThreadRelationFilter,
    NativeThreadSortDirection, NativeThreadTimestampSort,
};

#[derive(Default)]
struct HydrationState {
    summaries: HashSet<ThreadId>,
    histories: HashSet<ThreadId>,
    list_timestamps: HashMap<
        ThreadId,
        (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>),
    >,
}

/// Persists the native loop's replay history in AHEAD's Turso session store.
pub(crate) struct TursoThreadStore {
    inner: InMemoryThreadStore,
    store: Arc<dyn HarnessStore>,
    hydrated: tokio::sync::Mutex<HydrationState>,
    persistence_errors: Mutex<HashMap<String, String>>,
}

impl TursoThreadStore {
    pub(crate) fn new(store: Arc<dyn HarnessStore>) -> Self {
        Self {
            inner: InMemoryThreadStore::default(),
            store,
            hydrated: tokio::sync::Mutex::new(HydrationState::default()),
            persistence_errors: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn persistence_error(&self, thread_id: &str) -> Option<String> {
        self.persistence_errors.lock().get(thread_id).cloned()
    }

    fn record_persistence_error(&self, thread_id: &str, error: &ThreadStoreError) {
        self.persistence_errors
            .lock()
            .entry(thread_id.to_string())
            .or_insert_with(|| error.to_string());
    }

    async fn ensure_header(&self, thread_id: ThreadId) -> ThreadStoreResult<bool> {
        let mut hydrated = self.hydrated.lock().await;
        if hydrated.summaries.contains(&thread_id) {
            return Ok(true);
        }
        let header = self
            .store
            .load_native_thread_header(&thread_id.to_string())
            .map_err(store_error)?;
        let Some(header) = header else {
            return Ok(false);
        };
        Self::hydrate_header(&self.inner, header).await?;
        hydrated.summaries.insert(thread_id);
        Ok(true)
    }

    async fn hydrate(&self, thread_id: ThreadId) -> ThreadStoreResult<()> {
        if !self.ensure_header(thread_id).await? {
            return Ok(());
        }
        let mut hydrated = self.hydrated.lock().await;
        if hydrated.histories.contains(&thread_id) {
            return Ok(());
        }
        let snapshot = self
            .store
            .load_native_thread(&thread_id.to_string())
            .map_err(store_error)?;
        let Some(snapshot) = snapshot else {
            return Ok(());
        };
        self.hydrate_items(thread_id, snapshot.rollout_items)
            .await?;
        hydrated.histories.insert(thread_id);
        Ok(())
    }

    async fn hydrate_header(
        store: &InMemoryThreadStore,
        header: NativeThreadHeader,
    ) -> ThreadStoreResult<()> {
        let thread_id =
            ThreadId::from_string(&header.thread_id).map_err(store_error)?;
        let archived = header.archived;
        let archived_at = header.archived_at;
        let created_at = header.created_at;
        let updated_at = header.updated_at;
        if archived != archived_at.is_some() {
            return Err(store_error(
                "native thread archive flag and timestamp disagree",
            ));
        }
        let params: CreateThreadParams =
            serde_json::from_value(header.create_params).map_err(store_error)?;
        if params.thread_id != thread_id {
            return Err(ThreadStoreError::InvalidRequest {
                message: format!(
                    "stored native thread id {} does not match requested id {thread_id}",
                    params.thread_id
                ),
            });
        }
        let patches: Vec<ThreadMetadataPatch> = header
            .metadata_patches
            .into_iter()
            .map(|patch| serde_json::from_value(patch).map_err(store_error))
            .collect::<ThreadStoreResult<_>>()?;
        if patches.iter().any(|patch| patch.project_id.is_some()) {
            return Err(ThreadStoreError::Unsupported {
                operation: "projects",
            });
        }
        store.create_thread(params).await?;
        for patch in patches {
            store
                .update_thread_metadata(UpdateThreadMetadataParams {
                    thread_id,
                    patch,
                    include_archived: true,
                })
                .await?;
        }
        if created_at.is_some() || updated_at.is_some() {
            store
                .update_thread_metadata(UpdateThreadMetadataParams {
                    thread_id,
                    patch: ThreadMetadataPatch {
                        created_at,
                        updated_at,
                        ..ThreadMetadataPatch::default()
                    },
                    include_archived: true,
                })
                .await?;
        }
        if let Some(archived_at) = archived_at {
            store
                .archive_thread(ArchiveThreadParams { thread_id })
                .await?;
            store.restore_archive_time(thread_id, archived_at).await?;
        }
        Ok(())
    }

    async fn hydrate_items(
        &self,
        thread_id: ThreadId,
        rollout_items: Vec<serde_json::Value>,
    ) -> ThreadStoreResult<()> {
        if !rollout_items.is_empty() {
            let items: Vec<RolloutItem> =
                serde_json::from_value(rollout_items.into()).map_err(store_error)?;
            self.inner
                .append_items(AppendThreadItemsParams { thread_id, items })
                .await?;
        }
        Ok(())
    }
}

fn store_error(error: impl std::fmt::Display) -> ThreadStoreError {
    ThreadStoreError::Internal {
        message: error.to_string(),
    }
}

fn indexed_header_page_request(
    params: &ListThreadsParams,
) -> Option<NativeThreadHeaderPageRequest> {
    // ponytail: assigned project/section and recency filters still need indexed summaries.
    if params.section.as_ref().is_some_and(Option::is_some)
        || params.project_id.as_ref().is_some_and(Option::is_some)
    {
        return None;
    }

    let sort = match params.sort_key {
        ThreadSortKey::CreatedAt => NativeThreadTimestampSort::CreatedAt,
        ThreadSortKey::UpdatedAt => NativeThreadTimestampSort::UpdatedAt,
        ThreadSortKey::RecencyAt | ThreadSortKey::SectionPosition => return None,
    };
    let direction = match params.sort_direction {
        SortDirection::Asc => NativeThreadSortDirection::Asc,
        SortDirection::Desc => NativeThreadSortDirection::Desc,
    };
    let allowed_sources = params
        .allowed_sources
        .iter()
        .map(|source| serde_json::to_value(source).ok())
        .collect::<Option<Vec<_>>>()?;
    let relation_filter =
        params.relation_filter.as_ref().map(|filter| match filter {
            ThreadRelationFilter::DirectChildrenOf(parent) => {
                NativeThreadRelationFilter::DirectChildrenOf(parent.to_string())
            }
            ThreadRelationFilter::DescendantsOf(ancestor) => {
                NativeThreadRelationFilter::DescendantsOf(ancestor.to_string())
            }
        });
    if params
        .cwd_filters
        .as_ref()
        .is_some_and(|paths| paths.iter().any(|path| path.to_str().is_none()))
    {
        return None;
    }

    Some(NativeThreadHeaderPageRequest {
        archived: params.archived,
        sort,
        direction,
        cursor: params.cursor.clone(),
        allowed_sources,
        model_providers: params
            .model_providers
            .clone()
            .filter(|providers| !providers.is_empty())
            .unwrap_or_default(),
        cwd_filters: params.cwd_filters.clone(),
        search_term: params.search_term.clone(),
        relation_filter,
        limit: params.page_size.saturating_add(1),
    })
}

impl ThreadStore for TursoThreadStore {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn default_history_mode(&self) -> ThreadHistoryMode {
        ThreadHistoryMode::Paginated
    }

    fn create_thread(
        &self,
        params: CreateThreadParams,
    ) -> ThreadStoreFuture<'_, ()> {
        Box::pin(async move {
            let mut hydrated = self.hydrated.lock().await;
            let create_params =
                serde_json::to_value(&params).map_err(store_error)?;
            self.store
                .create_native_thread(&params.thread_id.to_string(), create_params)
                .map_err(store_error)?;
            if let Err(error) = self.inner.create_thread(params.clone()).await {
                self.store
                    .delete_native_thread(&params.thread_id.to_string())
                    .map_err(store_error)?;
                return Err(error);
            }
            hydrated.summaries.insert(params.thread_id);
            if params.history_mode == ThreadHistoryMode::Legacy {
                hydrated.histories.insert(params.thread_id);
            }
            Ok(())
        })
    }

    fn resume_thread(
        &self,
        params: ResumeThreadParams,
    ) -> ThreadStoreFuture<'_, ()> {
        Box::pin(async move {
            self.ensure_header(params.thread_id).await?;
            let summary = self
                .inner
                .read_thread(ReadThreadParams {
                    thread_id: params.thread_id,
                    include_archived: params.include_archived,
                    include_history: false,
                })
                .await?;
            if summary.history_mode == ThreadHistoryMode::Legacy
                && params.history.is_none()
            {
                self.hydrate(params.thread_id).await?;
            }
            let has_full_history = summary.history_mode == ThreadHistoryMode::Legacy
                && params.history.is_some();
            let thread_id = params.thread_id;
            self.inner.resume_thread(params).await?;
            if has_full_history {
                self.hydrated.lock().await.histories.insert(thread_id);
            }
            Ok(())
        })
    }

    fn append_items(
        &self,
        params: AppendThreadItemsParams,
    ) -> ThreadStoreFuture<'_, ()> {
        Box::pin(async move {
            let thread_id = params.thread_id.to_string();
            if let Some(error) = self.persistence_error(&thread_id) {
                return Err(store_error(format!(
                    "native thread history is already unsafe to extend: {error}"
                )));
            }
            let result: ThreadStoreResult<()> = async {
                self.ensure_header(params.thread_id).await?;
                let thread = self
                    .inner
                    .read_thread(ReadThreadParams {
                        thread_id: params.thread_id,
                        include_archived: true,
                        include_history: false,
                    })
                    .await?;
                let persisted = persisted_rollout_items(
                    params.items.as_slice(),
                    thread.history_mode,
                );
                let values =
                    serde_json::to_value(&persisted).map_err(store_error)?;
                let values = values.as_array().cloned().ok_or_else(|| {
                    store_error("serialized rollout items were not an array")
                })?;
                self.store
                    .append_native_thread_items(&thread_id, values)
                    .map_err(store_error)?;
                self.inner.append_items(params).await
            }
            .await;
            if let Err(error) = &result {
                self.record_persistence_error(&thread_id, error);
            }
            result
        })
    }

    fn persist_thread(
        &self,
        thread_id: ThreadId,
        context: PersistContext,
    ) -> ThreadStoreFuture<'_, ()> {
        self.inner.persist_thread(thread_id, context)
    }

    fn flush_thread(&self, thread_id: ThreadId) -> ThreadStoreFuture<'_, ()> {
        self.inner.flush_thread(thread_id)
    }

    fn shutdown_thread(&self, thread_id: ThreadId) -> ThreadStoreFuture<'_, ()> {
        self.inner.shutdown_thread(thread_id)
    }

    fn discard_thread(&self, thread_id: ThreadId) -> ThreadStoreFuture<'_, ()> {
        self.inner.discard_thread(thread_id)
    }

    fn load_history(
        &self,
        params: LoadThreadHistoryParams,
    ) -> ThreadStoreFuture<'_, StoredThreadHistory> {
        Box::pin(async move {
            self.ensure_header(params.thread_id).await?;
            let thread = self
                .inner
                .read_thread(ReadThreadParams {
                    thread_id: params.thread_id,
                    include_archived: params.include_archived,
                    include_history: false,
                })
                .await?;
            if thread.history_mode == ThreadHistoryMode::Paginated {
                return self.inner.load_history(params).await;
            }
            self.hydrate(params.thread_id).await?;
            self.inner.load_history(params).await
        })
    }

    fn load_latest_model_context(
        &self,
        params: LoadThreadHistoryParams,
    ) -> ThreadStoreFuture<'_, StoredModelContext> {
        Box::pin(async move {
            self.ensure_header(params.thread_id).await?;
            let thread = self
                .inner
                .read_thread(ReadThreadParams {
                    thread_id: params.thread_id,
                    include_archived: params.include_archived,
                    include_history: false,
                })
                .await?;
            if thread.history_mode == ThreadHistoryMode::Legacy {
                self.hydrate(params.thread_id).await?;
                return self.inner.load_latest_model_context(params).await;
            }

            let header = self
                .store
                .load_native_thread_header(&params.thread_id.to_string())
                .map_err(store_error)?
                .ok_or(ThreadStoreError::ThreadNotFound {
                    thread_id: params.thread_id,
                })?;
            let create_params: CreateThreadParams =
                serde_json::from_value(header.create_params).map_err(store_error)?;
            let metadata = canonical_session_meta_line(&create_params);
            let mut scan = ModelContextScan::default();
            let mut before_ordinal = None;
            'pages: loop {
                let page = self
                    .store
                    .load_native_thread_item_page(
                        &params.thread_id.to_string(),
                        before_ordinal,
                        128,
                    )
                    .map_err(store_error)?;
                for value in page.items {
                    let item: RolloutItem =
                        serde_json::from_value(value).map_err(store_error)?;
                    if scan.push(item) == ModelContextScanProgress::Complete {
                        break 'pages;
                    }
                }
                match page.next_before_ordinal {
                    Some(cursor) => before_ordinal = Some(cursor),
                    None => break,
                }
            }
            Ok(StoredModelContext {
                thread_id: params.thread_id,
                items: scan.finish(metadata),
            })
        })
    }

    fn read_thread(
        &self,
        params: ReadThreadParams,
    ) -> ThreadStoreFuture<'_, StoredThread> {
        Box::pin(async move {
            self.ensure_header(params.thread_id).await?;
            if !params.include_history {
                return self.inner.read_thread(params).await;
            }
            let thread = self
                .inner
                .read_thread(ReadThreadParams {
                    thread_id: params.thread_id,
                    include_archived: params.include_archived,
                    include_history: false,
                })
                .await?;
            if thread.history_mode == ThreadHistoryMode::Paginated {
                return self.inner.read_thread(params).await;
            }
            self.hydrate(params.thread_id).await?;
            self.inner.read_thread(params).await
        })
    }

    fn list_threads(
        &self,
        params: ListThreadsParams,
    ) -> ThreadStoreFuture<'_, ThreadPage> {
        Box::pin(async move {
            if params.page_size == 0 {
                return Err(ThreadStoreError::InvalidRequest {
                    message: "thread page size must be greater than zero"
                        .to_string(),
                });
            }
            if params.sort_key == ThreadSortKey::SectionPosition
                && !matches!(params.section.as_ref(), Some(Some(_)))
            {
                return Err(ThreadStoreError::InvalidRequest {
                    message: "section-position sorting requires a section filter"
                        .to_string(),
                });
            }
            let page_request = indexed_header_page_request(&params);
            let page_headers = page_request
                .as_ref()
                .map(|request| {
                    self.store
                        .list_native_thread_headers_page(request)
                        .map_err(store_error)
                })
                .transpose()?
                .flatten();
            let is_indexed_page = page_headers.is_some();
            let headers = match page_headers {
                Some(headers) => headers,
                None => self
                    .store
                    .list_native_thread_headers()
                    .map_err(store_error)?,
            };
            let page_store = is_indexed_page.then(InMemoryThreadStore::default);
            let mut hydrated = self.hydrated.lock().await;
            let mut titles = HashMap::new();
            for header in headers {
                let thread_id =
                    ThreadId::from_string(&header.thread_id).map_err(store_error)?;
                if let Some(title) =
                    header.metadata_patches.iter().rev().find_map(|patch| {
                        patch.get("title").and_then(serde_json::Value::as_str)
                    })
                {
                    titles.insert(thread_id, title.to_string());
                }
                let timestamps = header.created_at.zip(header.updated_at);
                let target_store = page_store.as_ref().unwrap_or(&self.inner);
                if is_indexed_page {
                    Self::hydrate_header(target_store, header).await?;
                } else if !hydrated.summaries.contains(&thread_id) {
                    Self::hydrate_header(target_store, header).await?;
                    hydrated.summaries.insert(thread_id);
                }
                if let Some((created_at, updated_at)) = timestamps {
                    if is_indexed_page {
                        target_store
                            .update_thread_metadata(UpdateThreadMetadataParams {
                                thread_id,
                                patch: ThreadMetadataPatch {
                                    created_at: Some(created_at),
                                    updated_at: Some(updated_at),
                                    ..ThreadMetadataPatch::default()
                                },
                                include_archived: true,
                            })
                            .await?;
                    } else if hydrated.list_timestamps.get(&thread_id)
                        != Some(&(created_at, updated_at))
                    {
                        target_store
                            .update_thread_metadata(UpdateThreadMetadataParams {
                                thread_id,
                                patch: ThreadMetadataPatch {
                                    created_at: Some(created_at),
                                    updated_at: Some(updated_at),
                                    ..ThreadMetadataPatch::default()
                                },
                                include_archived: true,
                            })
                            .await?;
                        hydrated
                            .list_timestamps
                            .insert(thread_id, (created_at, updated_at));
                    }
                }
            }
            drop(hydrated);
            let mut page_params = params.clone();
            if is_indexed_page {
                page_params.relation_filter = None;
            }
            let mut page = match page_store.as_ref() {
                Some(store) => store.list_threads(page_params).await?,
                None => self.inner.list_threads(page_params).await?,
            };
            page.items.retain(|thread| {
                (params.allowed_sources.is_empty()
                    || params.allowed_sources.contains(&thread.source))
                    && params.model_providers.as_ref().is_none_or(|providers| {
                        providers.is_empty()
                            || providers.contains(&thread.model_provider)
                    })
                    && params
                        .cwd_filters
                        .as_ref()
                        .is_none_or(|paths| paths.contains(&thread.cwd))
                    && params.project_id.as_ref().is_none_or(|project| {
                        thread.project_id.as_ref() == project.as_ref()
                    })
                    && params.search_term.as_ref().is_none_or(|term| {
                        thread
                            .name
                            .as_deref()
                            .is_some_and(|name| name.contains(term))
                            || thread.preview.contains(term)
                            || thread
                                .first_user_message
                                .as_deref()
                                .is_some_and(|message| message.contains(term))
                            || titles
                                .get(&thread.thread_id)
                                .is_some_and(|title| title.contains(term))
                    })
            });
            page.items.sort_by(|left, right| {
                let order = match params.sort_key {
                    ThreadSortKey::CreatedAt => {
                        left.created_at.cmp(&right.created_at)
                    }
                    ThreadSortKey::UpdatedAt => {
                        left.updated_at.cmp(&right.updated_at)
                    }
                    ThreadSortKey::RecencyAt => {
                        left.recency_at.cmp(&right.recency_at)
                    }
                    ThreadSortKey::SectionPosition => left
                        .section_position
                        .unwrap_or(i64::MAX)
                        .cmp(&right.section_position.unwrap_or(i64::MAX)),
                }
                .then_with(|| {
                    left.thread_id.to_string().cmp(&right.thread_id.to_string())
                });
                if params.sort_direction == SortDirection::Desc {
                    order.reverse()
                } else {
                    order
                }
            });
            if let Some(cursor) = params.cursor.as_deref() {
                let invalid_cursor = || ThreadStoreError::InvalidRequest {
                    message: format!("invalid cursor: {cursor}"),
                };
                let (key, id) =
                    cursor.rsplit_once('|').ok_or_else(invalid_cursor)?;
                let id = ThreadId::from_string(id).map_err(|_| invalid_cursor())?;
                let id = id.to_string();
                if params.sort_key == ThreadSortKey::SectionPosition {
                    let position =
                        key.parse::<i64>().map_err(|_| invalid_cursor())?;
                    page.items.retain(|thread| {
                        let order = thread
                            .section_position
                            .unwrap_or(i64::MAX)
                            .cmp(&position)
                            .then_with(|| thread.thread_id.to_string().cmp(&id));
                        match params.sort_direction {
                            SortDirection::Asc => order.is_gt(),
                            SortDirection::Desc => order.is_lt(),
                        }
                    });
                } else {
                    let timestamp = chrono::DateTime::parse_from_rfc3339(key)
                        .map_err(|_| invalid_cursor())?
                        .with_timezone(&chrono::Utc);
                    page.items.retain(|thread| {
                        let time = match params.sort_key {
                            ThreadSortKey::CreatedAt => thread.created_at,
                            ThreadSortKey::UpdatedAt => thread.updated_at,
                            ThreadSortKey::RecencyAt => thread.recency_at,
                            ThreadSortKey::SectionPosition => return false,
                        };
                        let order = time
                            .cmp(&timestamp)
                            .then_with(|| thread.thread_id.to_string().cmp(&id));
                        match params.sort_direction {
                            SortDirection::Asc => order.is_gt(),
                            SortDirection::Desc => order.is_lt(),
                        }
                    });
                }
            }
            if page.items.len() > params.page_size {
                let last = &page.items[params.page_size - 1];
                let key = match params.sort_key {
                    ThreadSortKey::CreatedAt => last.created_at.to_rfc3339(),
                    ThreadSortKey::UpdatedAt => last.updated_at.to_rfc3339(),
                    ThreadSortKey::RecencyAt => last.recency_at.to_rfc3339(),
                    ThreadSortKey::SectionPosition => {
                        last.section_position.unwrap_or(i64::MAX).to_string()
                    }
                };
                page.next_cursor = Some(format!("{key}|{}", last.thread_id));
                page.items.truncate(params.page_size);
            }
            Ok(page)
        })
    }

    fn update_thread_metadata(
        &self,
        params: UpdateThreadMetadataParams,
    ) -> ThreadStoreFuture<'_, Option<StoredThread>> {
        Box::pin(async move {
            let thread_id = params.thread_id.to_string();
            if let Some(error) = self.persistence_error(&thread_id) {
                return Err(store_error(format!(
                    "native thread history is already unsafe to extend: {error}"
                )));
            }
            self.ensure_header(params.thread_id).await?;
            if params.patch.project_id.is_some() {
                return Err(ThreadStoreError::Unsupported {
                    operation: "projects",
                });
            }
            self.inner
                .read_thread(ReadThreadParams {
                    thread_id: params.thread_id,
                    include_archived: params.include_archived,
                    include_history: false,
                })
                .await?;
            let patch = serde_json::to_value(&params.patch).map_err(store_error)?;
            if let Err(error) =
                self.store.append_native_thread_metadata(&thread_id, patch)
            {
                let error = store_error(error);
                self.record_persistence_error(&thread_id, &error);
                return Err(error);
            }
            self.inner.update_thread_metadata(params).await
        })
    }

    fn archive_thread(
        &self,
        params: ArchiveThreadParams,
    ) -> ThreadStoreFuture<'_, ()> {
        Box::pin(async move {
            self.ensure_header(params.thread_id).await?;
            self.inner
                .read_thread(ReadThreadParams {
                    thread_id: params.thread_id,
                    include_archived: false,
                    include_history: false,
                })
                .await?;
            let archived_at = self
                .store
                .set_native_thread_archived(&params.thread_id.to_string(), true)
                .map_err(store_error)?
                .ok_or_else(|| store_error("archive timestamp was not persisted"))?;
            let thread_id = params.thread_id;
            self.inner.archive_thread(params).await?;
            self.inner
                .restore_archive_time(thread_id, archived_at)
                .await
        })
    }

    fn unarchive_thread(
        &self,
        params: ArchiveThreadParams,
    ) -> ThreadStoreFuture<'_, StoredThread> {
        Box::pin(async move {
            self.ensure_header(params.thread_id).await?;
            let archived_at = self
                .store
                .set_native_thread_archived(&params.thread_id.to_string(), false)
                .map_err(store_error)?;
            if archived_at.is_some() {
                return Err(store_error(
                    "unarchive retained a persisted archive timestamp",
                ));
            }
            self.inner.unarchive_thread(params).await
        })
    }

    fn delete_thread(
        &self,
        params: DeleteThreadParams,
    ) -> ThreadStoreFuture<'_, ()> {
        Box::pin(async move {
            self.ensure_header(params.thread_id).await?;
            let thread_id = params.thread_id;
            self.store
                .delete_native_thread(&thread_id.to_string())
                .map_err(store_error)?;
            self.inner.delete_thread(params).await?;
            let mut hydrated = self.hydrated.lock().await;
            hydrated.summaries.remove(&thread_id);
            hydrated.histories.remove(&thread_id);
            hydrated.list_timestamps.remove(&thread_id);
            Ok(())
        })
    }
}

#[cfg(test)]
mod indexed_listing_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn indexed_header_page_request_keeps_creation_filters_and_search() {
        let params = ListThreadsParams {
            page_size: 7,
            cursor: Some("cursor".to_string()),
            sort_key: ThreadSortKey::UpdatedAt,
            sort_direction: SortDirection::Desc,
            allowed_sources: vec![codex_protocol::protocol::SessionSource::Cli],
            model_providers: Some(vec!["openai".to_string()]),
            cwd_filters: Some(vec![PathBuf::from("/workspace")]),
            section: Some(None),
            project_id: Some(None),
            archived: false,
            search_term: Some("needle".to_string()),
            relation_filter: None,
        };

        let request = indexed_header_page_request(&params).expect(
            "unsectioned, unassigned threads should use indexed keyset paging",
        );
        assert_eq!(request.allowed_sources, vec![serde_json::json!("cli")]);
        assert_eq!(request.model_providers, vec!["openai"]);
        assert_eq!(request.cwd_filters, Some(vec![PathBuf::from("/workspace")]));
        assert_eq!(request.search_term.as_deref(), Some("needle"));
        assert_eq!(request.cursor.as_deref(), Some("cursor"));
        assert_eq!(request.limit, 8);
    }

    #[test]
    fn indexed_header_page_request_keeps_structured_source_values() {
        let params = ListThreadsParams {
            page_size: 7,
            cursor: None,
            sort_key: ThreadSortKey::UpdatedAt,
            sort_direction: SortDirection::Desc,
            allowed_sources: vec![codex_protocol::protocol::SessionSource::Custom(
                "custom-source".to_string(),
            )],
            model_providers: None,
            cwd_filters: None,
            section: None,
            project_id: None,
            archived: false,
            search_term: None,
            relation_filter: None,
        };

        let request = indexed_header_page_request(&params)
            .expect("structured source values should use indexed keyset paging");
        assert_eq!(
            request.allowed_sources,
            vec![serde_json::json!({"custom": "custom-source"})]
        );
    }

    #[test]
    fn indexed_header_page_request_keeps_spawn_relation_filters() {
        let parent = ThreadId::from_string("00000000-0000-0000-0000-000000000001")
            .expect("valid thread id");
        let mut params = ListThreadsParams {
            page_size: 7,
            cursor: None,
            sort_key: ThreadSortKey::UpdatedAt,
            sort_direction: SortDirection::Desc,
            allowed_sources: Vec::new(),
            model_providers: None,
            cwd_filters: None,
            section: None,
            project_id: None,
            archived: false,
            search_term: None,
            relation_filter: None,
        };

        for relation_filter in [
            ThreadRelationFilter::DirectChildrenOf(parent.clone()),
            ThreadRelationFilter::DescendantsOf(parent),
        ] {
            params.relation_filter = Some(relation_filter.clone());
            let request = indexed_header_page_request(&params)
                .expect("spawn filters should use indexed keyset paging");
            let expected = match relation_filter {
                ThreadRelationFilter::DirectChildrenOf(parent) => {
                    NativeThreadRelationFilter::DirectChildrenOf(parent.to_string())
                }
                ThreadRelationFilter::DescendantsOf(ancestor) => {
                    NativeThreadRelationFilter::DescendantsOf(ancestor.to_string())
                }
            };
            assert_eq!(request.relation_filter, Some(expected));
        }
    }
}
