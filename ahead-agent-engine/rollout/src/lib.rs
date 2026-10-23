//! History policy for the managed AHEAD agent.

use serde::de::Error as _;
use serde_json::Value;

mod metadata;
mod model_context;
pub(crate) mod policy;

pub use codex_history::CompactedItem;
pub use codex_history::ResponseItemEnvelope;
pub use codex_history::RolloutItem;
pub use codex_history::RolloutLine;
pub(crate) use codex_protocol::protocol;

/// Decodes one legacy rollout record with the persistence-boundary JSON workaround.
pub fn decode_rollout_line(value: Value) -> serde_json::Result<RolloutLine> {
    let Value::Object(mut fields) = value else {
        return serde_json::from_value(value);
    };
    let timestamp = fields
        .remove("timestamp")
        .ok_or_else(|| serde_json::Error::missing_field("timestamp"))
        .and_then(serde_json::from_value)?;
    let ordinal = fields
        .remove("ordinal")
        .map(serde_json::from_value::<Option<u64>>)
        .transpose()?
        .flatten();
    let item = serde_json::from_value(Value::Object(fields))?;

    Ok(RolloutLine {
        timestamp,
        ordinal,
        item,
    })
}

pub use metadata::forked_from_ordinal_exclusive;
pub use model_context::ModelContextScan;
pub use model_context::ModelContextScanProgress;
pub use policy::persisted_rollout_items;
