use codex_utils_absolute_path::AbsolutePathBuf;

/// A skill document that could not be read, parsed, or validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillError {
    pub path: AbsolutePathBuf,
    pub message: String,
}
