use codex_protocol::ThreadId;
use codex_protocol::protocol::ThreadHistoryMode;
use std::any::Any;
use std::future::Future;
use std::pin::Pin;

use crate::AppendThreadItemsParams;
use crate::ArchiveThreadParams;
use crate::CreateThreadParams;
use crate::DeleteThreadParams;
use crate::ListThreadsParams;
use crate::LoadThreadHistoryParams;
use crate::MoveThreadToSectionParams;
use crate::ReadThreadParams;
use crate::ResumeThreadParams;
use crate::RevertThreadParams;
use crate::StoredModelContext;
use crate::StoredThread;
use crate::StoredThreadHistory;
use crate::ThreadPage;
use crate::ThreadStoreError;
use crate::ThreadStoreResult;
use crate::UpdateThreadMetadataParams;

/// Future returned by [`ThreadStore`] operations.
pub type ThreadStoreFuture<'a, T> = Pin<Box<dyn Future<Output = ThreadStoreResult<T>> + Send + 'a>>;

/// Why thread persistence is being requested.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PersistContext {
    /// Standard persistence makes the thread and all queued items durable and readable.
    Standard,
    /// A turn is about to begin sampling after its input has been recorded.
    TurnStart,
}

/// Storage-neutral thread persistence boundary.
pub trait ThreadStore: Any + Send + Sync {
    /// Return this store as [`Any`] for implementation-owned escape hatches.
    fn as_any(&self) -> &dyn Any;

    /// Returns the history mode to use when history does not carry a persisted mode.
    ///
    /// The in-memory test store uses the legacy layout. AHEAD's Turso store overrides this with
    /// its paginated history mode.
    fn default_history_mode(&self) -> ThreadHistoryMode {
        ThreadHistoryMode::Legacy
    }

    /// Creates a new live thread.
    fn create_thread(&self, params: CreateThreadParams) -> ThreadStoreFuture<'_, ()>;

    /// Reopens an existing thread for live appends.
    fn resume_thread(&self, params: ResumeThreadParams) -> ThreadStoreFuture<'_, ()>;

    /// Appends raw rollout items to a live thread.
    ///
    /// Implementations should apply the shared rollout persistence policy before writing durable
    /// replay history and before updating any implementation-owned projections.
    fn append_items(&self, params: AppendThreadItemsParams) -> ThreadStoreFuture<'_, ()>;

    /// Materializes the thread if persistence is lazy, then persists all queued items.
    ///
    /// Standard persistence must complete before returning. Turn-start persistence may complete
    /// in the background when the implementation enqueues it before returning, fences it with
    /// subsequent flush or shutdown operations, and surfaces failures through those operations.
    fn persist_thread(
        &self,
        thread_id: ThreadId,
        context: PersistContext,
    ) -> ThreadStoreFuture<'_, ()>;

    /// Flushes all queued items and returns once they are durable/readable.
    fn flush_thread(&self, thread_id: ThreadId) -> ThreadStoreFuture<'_, ()>;

    /// Flushes pending items and closes the live thread writer.
    fn shutdown_thread(&self, thread_id: ThreadId) -> ThreadStoreFuture<'_, ()>;

    /// Discards the live thread writer without forcing pending in-memory items to become durable.
    ///
    /// Core calls this when session initialization fails after a live writer has been created.
    /// Implementations should release any live writer resources for the thread while preserving
    /// already-durable thread data.
    fn discard_thread(&self, thread_id: ThreadId) -> ThreadStoreFuture<'_, ()>;

    /// Loads persisted history for resume, fork, rollback, and memory jobs.
    fn load_history(
        &self,
        params: LoadThreadHistoryParams,
    ) -> ThreadStoreFuture<'_, StoredThreadHistory>;

    /// Loads the persisted rollout items needed to reconstruct the latest model-visible context.
    ///
    /// Implementations that cannot perform a targeted read may return the full persisted history.
    fn load_latest_model_context(
        &self,
        _params: LoadThreadHistoryParams,
    ) -> ThreadStoreFuture<'_, StoredModelContext> {
        Box::pin(async {
            Err(ThreadStoreError::Unsupported {
                operation: "load_latest_model_context",
            })
        })
    }

    /// Reverts a paginated thread's durable history so it ends immediately before
    /// `before_turn_id`.
    ///
    /// Callers must close the thread's live writer first. The logical thread id and semantic
    /// metadata stay unchanged.
    ///
    /// Stores without paginated revert support can retain this default implementation.
    fn revert_thread(&self, _params: RevertThreadParams) -> ThreadStoreFuture<'_, ()> {
        Box::pin(async {
            Err(ThreadStoreError::Unsupported {
                operation: "revert_thread",
            })
        })
    }

    /// Reads a thread summary and optionally its persisted history.
    fn read_thread(&self, params: ReadThreadParams) -> ThreadStoreFuture<'_, StoredThread>;

    /// Lists stored threads matching the supplied filters.
    fn list_threads(&self, params: ListThreadsParams) -> ThreadStoreFuture<'_, ThreadPage>;

    /// Applies a literal metadata patch and returns the updated thread when one was materialized.
    ///
    /// `None` means the update succeeded without materializing a thread, for example because the
    /// implementation filtered the patch to a no-op. Callers that require a `StoredThread` must
    /// perform a fallback read.
    ///
    /// Implementations should apply the supplied fields directly. Policy such as deciding whether
    /// an append-derived preview should be emitted belongs above the store.
    fn update_thread_metadata(
        &self,
        params: UpdateThreadMetadataParams,
    ) -> ThreadStoreFuture<'_, Option<StoredThread>>;

    /// Moves a thread to, within, or out of a server-ordered section.
    fn move_thread_to_section(
        &self,
        _params: MoveThreadToSectionParams,
    ) -> ThreadStoreFuture<'_, ()> {
        Box::pin(async {
            Err(ThreadStoreError::Unsupported {
                operation: "thread/section/move",
            })
        })
    }

    /// Archives a thread.
    fn archive_thread(&self, params: ArchiveThreadParams) -> ThreadStoreFuture<'_, ()>;

    /// Unarchives a thread and returns its updated metadata.
    fn unarchive_thread(&self, params: ArchiveThreadParams) -> ThreadStoreFuture<'_, StoredThread>;

    /// Deletes a thread's persisted rollout data and associated metadata.
    fn delete_thread(&self, params: DeleteThreadParams) -> ThreadStoreFuture<'_, ()>;
}
