use codex_protocol::protocol::SkillScope;
use codex_utils_absolute_path::AbsolutePathBuf;

/// Metadata for one skill materialized on the host filesystem.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub short_description: Option<String>,
    pub disable_model_invocation: bool,
    /// Path to the SKILL.md file that declares this skill.
    pub path_to_skills_md: AbsolutePathBuf,
    pub scope: SkillScope,
}

impl SkillMetadata {
    pub fn allows_implicit_invocation(&self) -> bool {
        !self.disable_model_invocation
    }
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
