//! AHEAD-owned command attribution records.
//!
//! Keeping the narrow record here lets the retained loop shed Codex plugin
//! identities without coupling editor-owned attribution to the copied runtime.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCommandAttribution {
    pub actor_id: String,
    pub normalized_relative_path: String,
}

impl ToolCommandAttribution {
    pub fn serialized_fields(&self) -> (String, String) {
        (self.actor_id.clone(), self.normalized_relative_path.clone())
    }
}
