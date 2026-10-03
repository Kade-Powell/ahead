//! AHEAD's independent host for the Zed language-extension contract.
//!
//! This crate intentionally owns the host boundary instead of depending on
//! Zed's GPL extension-host implementation. The WIT files under `wit/` are
//! the compatibility contract; language servers and file icon themes are
//! exposed by the public API.

mod host;
mod icon_theme;

pub use icon_theme::{IconTheme, load_icon_theme};

pub use host::{
    ExtensionHost, LanguageDefinition, LanguageServerCommand,
    LanguageServerDiscovery, LanguageServerExtension, WorktreeContext,
    discover_language_server_extensions, install_extension_from_url,
    install_extension_package, language_id_for_path,
};
