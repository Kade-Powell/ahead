//! AHEAD's independent host for the Zed language-extension contract.
//!
//! This crate intentionally owns the host boundary instead of depending on
//! Zed's GPL extension-host implementation. The WIT files under `wit/` are
//! the compatibility contract; language-server discovery and installed
//! language grammar assets are exposed by the public API.

mod host;
mod node_runtime;
mod syntax;

pub use node_runtime::{NodeRuntime, NpmInvocation};

pub use host::{
    ExtensionHost, ExtensionSyntax, LanguageComments, LanguageDefinition,
    LanguageServerCommand, LanguageServerDiscovery, LanguageServerExtension,
    LanguageSyntax, WorktreeContext, discover_language_server_extensions,
    extension_syntax_for_path, install_extension_from_url,
    install_extension_package, language_id_for_path, language_syntax_for_path,
    syntax_captures_for_path, uninstall_extension_package,
};
pub use syntax::SyntaxCapture;
