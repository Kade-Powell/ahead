use std::future::Future;
use std::pin::Pin;

use codex_utils_absolute_path::AbsolutePathBuf;

/// Instructions supplied by the host.
///
/// `source` is the absolute filesystem path that supplied these instructions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Instructions {
    /// Model-visible instruction text.
    pub text: String,
    /// Absolute path that supplied these instructions.
    pub source: AbsolutePathBuf,
}

/// Result of loading host-provided user instructions.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LoadedUserInstructions {
    /// Loaded instructions, or `None` when the provider has no applicable text.
    pub instructions: Option<Instructions>,
    /// Recoverable loading problems that should be surfaced during startup.
    pub warnings: Vec<String>,
}

/// Future returned by a [`UserInstructionsProvider`].
pub type LoadUserInstructionsFuture<'a> =
    Pin<Box<dyn Future<Output = LoadedUserInstructions> + Send + 'a>>;

/// Loads the user instructions that apply when a root thread runtime starts.
///
/// Implementations should return any recoverable loading problems as warnings
/// while still returning usable fallback instructions when available.
pub trait UserInstructionsProvider: Send + Sync {
    /// Loads the snapshot to use for a newly created root runtime.
    fn load_user_instructions(&self) -> LoadUserInstructionsFuture<'_>;
}
