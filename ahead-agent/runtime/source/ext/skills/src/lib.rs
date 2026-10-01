mod fragments;
mod host_outcome;
mod host_prompt;
mod host_roots;
mod host_service;
mod host_snapshot;
mod invocation;
mod loader;

pub use host_outcome::SkillLoadOutcome;
pub use host_prompt::HostSkillPrompts;
pub use host_service::HostSkillsLoadInput;
pub use host_service::HostSkillsRequest;
pub use host_service::HostSkillsService;
pub use host_snapshot::HostSkillsSnapshot;
pub use invocation::detect_implicit_skill_invocation;

/// Recognizes persisted explicit skill prompts without exposing their fragment implementation.
pub fn is_skill_prompt_fragment(text: &str) -> bool {
    <fragments::SkillInstructions as codex_extension_api::ContextualUserFragment>::matches_text(
        text,
    )
}
