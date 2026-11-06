use crate::config::Config;
use codex_config::config_toml::ConfigToml;
use codex_config::types::WindowsSandboxModeToml;
use codex_protocol::config_types::WindowsSandboxLevel;

pub trait WindowsSandboxLevelExt {
    fn from_config(config: &Config) -> WindowsSandboxLevel;
}

impl WindowsSandboxLevelExt for WindowsSandboxLevel {
    fn from_config(config: &Config) -> WindowsSandboxLevel {
        match config.permissions.windows_sandbox_mode {
            Some(WindowsSandboxModeToml::Elevated) => WindowsSandboxLevel::Elevated,
            Some(WindowsSandboxModeToml::Unelevated) => WindowsSandboxLevel::RestrictedToken,
            None => WindowsSandboxLevel::Disabled,
        }
    }
}

pub fn resolve_windows_sandbox_mode(cfg: &ConfigToml) -> Option<WindowsSandboxModeToml> {
    cfg.windows.as_ref().and_then(|windows| windows.sandbox)
}

pub fn resolve_windows_sandbox_private_desktop(cfg: &ConfigToml) -> bool {
    cfg.windows
        .as_ref()
        .and_then(|windows| windows.sandbox_private_desktop)
        .unwrap_or(true)
}

#[cfg(test)]
#[path = "windows_sandbox_tests.rs"]
mod tests;
