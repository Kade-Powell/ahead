#![allow(clippy::manual_clamp)]

pub mod config;
pub mod directory;
pub mod encoding;
pub mod meta;
pub mod search;
#[cfg(any(unix, windows))]
#[cfg_attr(windows, path = "secure_fs_windows.rs")]
pub mod secure_fs;
pub mod workspace_trust;
