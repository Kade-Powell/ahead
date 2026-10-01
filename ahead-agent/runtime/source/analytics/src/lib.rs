//! Local analytics fact model for the in-editor agent.
//!
//! This crate is what remains of codex's analytics subsystem after the cloud
//! delivery path was removed. It owns two things:
//!
//! * the fact types that describe local turn state (compaction, skills,
//!   images, and so on), which other
//!   crates use as ordinary data structures; and
//! * [`AnalyticsEventsClient`], an inert sink that accepts those facts and
//!   records nothing.
//!
//! There is no HTTP client, no background queue, no on-disk capture, and no
//! shutdown flush: nothing here performs I/O or blocks shutdown.

mod client;
mod facts;

use std::time::SystemTime;
use std::time::UNIX_EPOCH;

pub use client::AnalyticsEventsClient;
pub use facts::AnalyticsJsonRpcError;
pub use facts::CodeModeToolCallFact;
pub use facts::CodeModeToolCallStatus;
pub use facts::CodexCompactionEvent;
pub use facts::CodexErrKind;
pub use facts::CodexTurnSteerEvent;
pub use facts::CompactionImplementation;
pub use facts::CompactionPhase;
pub use facts::CompactionReason;
pub use facts::CompactionStatus;
pub use facts::CompactionStrategy;
pub use facts::CompactionTrigger;
pub use facts::ControlToolCallFact;
pub use facts::ControlToolCallStatus;
pub use facts::HookRunFact;
pub use facts::ImageDetailSetting;
pub use facts::ImagePreparationFact;
pub use facts::ImagePreparationMetadata;
pub use facts::InputError;
pub use facts::InvocationType;
pub use facts::SkillInvocation;
pub use facts::SkillInvocationLocation;
pub use facts::SubAgentThreadStartedInput;
pub use facts::ThreadInitializationMode;
pub use facts::TrackEventsContext;
pub use facts::TurnAnalyticsMetadata;
pub use facts::TurnProfile;
pub use facts::TurnProfileFact;
pub use facts::TurnResolvedConfigFact;
pub use facts::TurnStatus;
pub use facts::TurnSteerRejectionReason;
pub use facts::TurnSteerRequestError;
pub use facts::TurnSteerResult;
pub use facts::TurnTokenUsageFact;
pub use facts::build_track_events_context;

pub fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn now_unix_millis() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}
