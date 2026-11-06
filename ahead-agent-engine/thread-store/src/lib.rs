//! Storage-neutral thread persistence interfaces.
//!
//! Application code treats [`codex_protocol::ThreadId`] as the durable thread handle.
//! AHEAD's managed sessions persist through its Turso-backed implementation;
//! the in-memory implementation supports runtime tests.

mod error;
mod in_memory;
mod live_thread;
mod store;
mod thread_metadata_sync;
mod types;

pub use error::ThreadStoreError;
pub use error::ThreadStoreResult;
pub use in_memory::InMemoryThreadStore;
pub use in_memory::InMemoryThreadStoreCalls;
pub use live_thread::LiveThread;
pub use live_thread::LiveThreadInitGuard;
pub use store::PersistContext;
pub use store::ThreadStore;
pub use store::ThreadStoreFuture;
pub use types::AppendThreadItemsParams;
pub use types::ArchiveThreadParams;
pub use types::ClearableField;
pub use types::CreateThreadParams;
pub use types::DeleteThreadParams;
pub use types::GitInfoPatch;
pub use types::ListThreadsParams;
pub use types::LoadThreadHistoryParams;
pub use types::MoveThreadToSectionParams;
pub use types::ReadThreadParams;
pub use types::ResumeThreadParams;
pub use types::RevertThreadParams;
pub use types::SortDirection;
pub use types::StoredModelContext;
pub use types::StoredThread;
pub use types::StoredThreadHistory;
pub use types::ThreadMetadataPatch;
pub use types::ThreadPage;
pub use types::ThreadPersistenceMetadata;
pub use types::ThreadRelationFilter;
pub use types::ThreadSortKey;
pub use types::UpdateThreadMetadataParams;
pub use types::canonical_session_meta_line;
