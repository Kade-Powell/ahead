//! Pure thread-metadata shapes derived from rollout history.
//!
//! This crate intentionally has no SQLite dependency. It owns the canonical
//! thread/project metadata types, the rollout-to-metadata extraction used by
//! both the managed in-memory store and legacy readers, and the pinned-section
//! constants. Durable session state is owned by AHEAD's Turso-backed store.

pub mod extract;
pub mod project;
pub mod thread_metadata;

pub use extract::apply_rollout_item;
pub use extract::enum_to_string;
pub use extract::rollout_item_affects_thread_metadata;
pub use project::CreatedProject;
pub use project::Project;
pub use project::ProjectRoot;
pub use project::ProjectSortKey;
pub use project::ProjectsPage;
pub use thread_metadata::Anchor;
pub use thread_metadata::BackfillStats;
pub use thread_metadata::ExtractionOutcome;
pub use thread_metadata::SortDirection;
pub use thread_metadata::SortKey;
pub use thread_metadata::ThreadMetadata;
pub use thread_metadata::ThreadMetadataBuilder;
pub use thread_metadata::ThreadRelationFilter;
pub use thread_metadata::ThreadSection;
pub use thread_metadata::ThreadSectionAppearance;
pub use thread_metadata::ThreadSectionsPage;
pub use thread_metadata::ThreadsPage;
pub use thread_metadata::anchor_from_item;
pub use thread_metadata::datetime_to_epoch_millis;
pub use thread_metadata::datetime_to_epoch_seconds;
pub use thread_metadata::epoch_millis_to_datetime;
pub use thread_metadata::epoch_seconds_to_datetime;

/// Stable UUIDv7 identifying the built-in pinned thread section.
pub const PINNED_THREAD_SECTION_ID: &str = "01984de2-8f74-7c91-a3b2-5c5e937cf318";

/// User-facing name of the built-in pinned thread section.
pub const PINNED_THREAD_SECTION_NAME: &str = "Pinned";
