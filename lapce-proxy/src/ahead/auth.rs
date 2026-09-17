//! AHEAD GitHub Authentication and Developer Identity
//!
//! Grounded in Section 2, Section 9, and Section 14 of `ahead-editor-mvp.md`.
//! Supports:
//! - GitHub OAuth Device Flow (RFC 8628) for headless/native desktop login
//! - Bearer token authorization and user profile fetching
//! - Local persistent auth store (`~/.ahead/auth.json`)
//! - Non-blocking offline fallback to local git config

use anyhow::{bail, Context, Result};
use lapce_rpc::ahead::{GitHubDeviceCodeResponse, GitHubUser};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Default client ID for AHEAD editor GitHub OAuth Application
pub const AHEAD_GITHUB_CLIENT_ID: &str = "Ov23liahead01editor";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthRecord {
    pub access_token: String,
    pub token_type: String,
    pub scope: String,
    pub user: GitHubUser,
    pub saved_at: String,
}

pub struct GitHubAuthManager {
    config_dir: PathBuf,
    current_user: Option<GitHubUser>,
    access_token: Option<String>,
}

impl GitHubAuthManager {
    pub fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let config_dir = PathBuf::from(home).join(".ahead");
        let mut mgr = Self {
            config_dir,
            current_user: None,
            access_token: None,
        };
        let _ = mgr.load_saved_auth();
        mgr
    }

    pub fn with_custom_dir(dir: PathBuf) -> Self {
        let mut mgr = Self {
            config_dir: dir,
            current_user: None,
            access_token: None,
        };
        let _ = mgr.load_saved_auth();
        mgr
    }

    /// Returns the currently active developer identity: authenticated GitHub user, or local Git config fallback
    pub fn get_active_user(&self, workspace_path: Option<&Path>) -> GitHubUser {
        if let Some(user) = &self.current_user {
            return user.clone();
        }
        self.resolve_local_git_fallback(workspace_path)
    }

    pub fn get_access_token(&self) -> Option<&str> {
        self.access_token.as_deref()
    }

    pub fn is_authenticated(&self) -> bool {
        self.current_user.as_ref().map(|u| u.is_authenticated).unwrap_or(false)
    }

    /// Sign out: clear token and cached user
    pub fn sign_out(&mut self) -> Result<()> {
        self.current_user = None;
        self.access_token = None;
        let auth_path = self.config_dir.join("auth.json");
        if auth_path.exists() {
            let _ = std::fs::remove_file(auth_path);
        }
        Ok(())
    }

    /// Saves token and user record to ~/.ahead/auth.json
    pub fn save_auth(&mut self, token: String, user: GitHubUser) -> Result<()> {
        std::fs::create_dir_all(&self.config_dir)?;
        let auth_path = self.config_dir.join("auth.json");
        let record = AuthRecord {
            access_token: token.clone(),
            token_type: "bearer".into(),
            scope: "read:user,repo".into(),
            user: user.clone(),
            saved_at: chrono::Utc::now().to_rfc3339(),
        };

        let json = serde_json::to_string_pretty(&record)?;
        std::fs::write(&auth_path, json)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&auth_path, std::fs::Permissions::from_mode(0o600));
        }

        self.access_token = Some(token);
        self.current_user = Some(user);
        Ok(())
    }

    /// Loads saved auth from ~/.ahead/auth.json
    pub fn load_saved_auth(&mut self) -> Result<()> {
        let auth_path = self.config_dir.join("auth.json");
        if !auth_path.exists() {
            return Ok(());
        }

        let content = std::fs::read_to_string(&auth_path)?;
        let record: AuthRecord = serde_json::from_str(&content)?;
        self.access_token = Some(record.access_token);
        self.current_user = Some(record.user);
        Ok(())
    }

    /// Resolves author identity from local Git configuration when offline or unauthenticated
    pub fn resolve_local_git_fallback(&self, workspace_path: Option<&Path>) -> GitHubUser {
        let name = self.read_git_config("user.name", workspace_path)
            .unwrap_or_else(|| {
                std::env::var("USER")
                    .or_else(|_| std::env::var("USERNAME"))
                    .unwrap_or_else(|_| "ahead-developer".into())
            });

        let email = self.read_git_config("user.email", workspace_path);

        GitHubUser {
            login: name.clone().replace(' ', "-").to_lowercase(),
            id: 0,
            name: Some(name),
            avatar_url: None,
            email,
            is_authenticated: false,
        }
    }

    fn read_git_config(&self, key: &str, workspace_path: Option<&Path>) -> Option<String> {
        let mut cmd = std::process::Command::new("git");
        cmd.args(["config", "--get", key]);
        if let Some(dir) = workspace_path {
            cmd.current_dir(dir);
        }
        let output = cmd.output().ok()?;
        if output.status.success() {
            let val = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !val.is_empty() {
                return Some(val);
            }
        }
        None
    }

    /// Parses raw response from https://github.com/login/device/code
    pub fn parse_device_code_response(json: &str) -> Result<GitHubDeviceCodeResponse> {
        #[derive(Deserialize)]
        struct RawResponse {
            device_code: String,
            user_code: String,
            verification_uri: String,
            expires_in: u64,
            interval: u64,
        }

        let raw: RawResponse = serde_json::from_str(json)
            .context("Failed to parse GitHub Device Code response")?;

        Ok(GitHubDeviceCodeResponse {
            device_code: raw.device_code,
            user_code: raw.user_code,
            verification_uri: raw.verification_uri,
            expires_in: raw.expires_in,
            interval: raw.interval,
        })
    }

    /// Parses raw response from https://github.com/login/oauth/access_token
    pub fn parse_access_token_response(json: &str) -> Result<PollTokenResult> {
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct TokenSuccess {
            access_token: String,
            #[serde(default)]
            token_type: String,
            #[serde(default)]
            scope: String,
        }

        #[derive(Deserialize)]
        struct TokenError {
            error: String,
            #[serde(default)]
            error_description: String,
        }

        if let Ok(success) = serde_json::from_str::<TokenSuccess>(json) {
            if !success.access_token.is_empty() {
                return Ok(PollTokenResult::Success(success.access_token));
            }
        }

        if let Ok(err) = serde_json::from_str::<TokenError>(json) {
            match err.error.as_str() {
                "authorization_pending" => return Ok(PollTokenResult::Pending),
                "slow_down" => return Ok(PollTokenResult::SlowDown),
                "expired_token" => bail!("Device authorization expired. Please try again."),
                "access_denied" => bail!("Authorization was cancelled by user."),
                other => bail!("GitHub auth error: {} ({})", other, err.error_description),
            }
        }

        bail!("Unknown response format from GitHub OAuth endpoint: {}", json)
    }

    /// Parses raw response from https://api.github.com/user
    pub fn parse_user_profile(json: &str) -> Result<GitHubUser> {
        #[derive(Deserialize)]
        struct RawUser {
            login: String,
            id: u64,
            name: Option<String>,
            avatar_url: Option<String>,
            email: Option<String>,
        }

        let raw: RawUser = serde_json::from_str(json)
            .context("Failed to parse GitHub User Profile response")?;

        Ok(GitHubUser {
            login: raw.login,
            id: raw.id,
            name: raw.name,
            avatar_url: raw.avatar_url,
            email: raw.email,
            is_authenticated: true,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum PollTokenResult {
    Success(String),
    Pending,
    SlowDown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_device_code_response() -> Result<()> {
        let payload = r#"{
            "device_code": "3584d83530557fdd1f46af8289938c8ef79f9dc5",
            "user_code": "WDJB-MJHT",
            "verification_uri": "https://github.com/login/device",
            "expires_in": 900,
            "interval": 5
        }"#;

        let res = GitHubAuthManager::parse_device_code_response(payload)?;
        assert_eq!(res.device_code, "3584d83530557fdd1f46af8289938c8ef79f9dc5");
        assert_eq!(res.user_code, "WDJB-MJHT");
        assert_eq!(res.verification_uri, "https://github.com/login/device");
        assert_eq!(res.expires_in, 900);
        assert_eq!(res.interval, 5);
        Ok(())
    }

    #[test]
    fn test_parse_access_token_responses() -> Result<()> {
        // Pending
        let pending = r#"{"error": "authorization_pending", "error_description": "Verify code at URI"}"#;
        assert_eq!(
            GitHubAuthManager::parse_access_token_response(pending)?,
            PollTokenResult::Pending
        );

        // Slow down
        let slow_down = r#"{"error": "slow_down", "error_description": "Interval too fast"}"#;
        assert_eq!(
            GitHubAuthManager::parse_access_token_response(slow_down)?,
            PollTokenResult::SlowDown
        );

        // Success
        let success = r#"{"access_token": "ghu_secret_12345", "token_type": "bearer", "scope": "repo"}"#;
        assert_eq!(
            GitHubAuthManager::parse_access_token_response(success)?,
            PollTokenResult::Success("ghu_secret_12345".into())
        );

        // Access denied
        let denied = r#"{"error": "access_denied", "error_description": "User cancelled"}"#;
        assert!(GitHubAuthManager::parse_access_token_response(denied).is_err());
        Ok(())
    }

    #[test]
    fn test_parse_user_profile() -> Result<()> {
        let payload = r#"{
            "login": "octocat",
            "id": 583231,
            "name": "The Octocat",
            "avatar_url": "https://avatars.githubusercontent.com/u/583231?v=4",
            "email": "octocat@github.com"
        }"#;

        let user = GitHubAuthManager::parse_user_profile(payload)?;
        assert_eq!(user.login, "octocat");
        assert_eq!(user.id, 583231);
        assert_eq!(user.name.as_deref(), Some("The Octocat"));
        assert!(user.is_authenticated);
        Ok(())
    }

    #[test]
    fn test_auth_persistence_roundtrip() -> Result<()> {
        let temp_dir = std::env::temp_dir().join(format!("ahead-test-auth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir)?;
        let mut mgr = GitHubAuthManager::with_custom_dir(temp_dir.clone());

        assert!(!mgr.is_authenticated());

        let user = GitHubUser {
            login: "alice".into(),
            id: 101,
            name: Some("Alice Developer".into()),
            avatar_url: None,
            email: Some("alice@example.com".into()),
            is_authenticated: true,
        };

        mgr.save_auth("ghu_test_token".into(), user.clone())?;
        assert!(mgr.is_authenticated());
        assert_eq!(mgr.get_access_token(), Some("ghu_test_token"));

        // New manager instance reading the same directory
        let mgr2 = GitHubAuthManager::with_custom_dir(temp_dir.clone());
        assert!(mgr2.is_authenticated());
        let loaded_user = mgr2.get_active_user(None);
        assert_eq!(loaded_user.login, "alice");
        assert_eq!(loaded_user.id, 101);
        assert!(loaded_user.is_authenticated);

        // Sign out
        let mut mgr3 = mgr2;
        mgr3.sign_out()?;
        assert!(!mgr3.is_authenticated());
        assert_eq!(mgr3.get_access_token(), None);

        let _ = std::fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    #[test]
    fn test_local_git_fallback_when_unauthenticated() {
        let temp_dir = std::env::temp_dir().join(format!("ahead-test-git-{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let mgr = GitHubAuthManager::with_custom_dir(temp_dir.clone());

        let fallback_user = mgr.get_active_user(None);
        assert!(!fallback_user.is_authenticated);
        assert!(!fallback_user.login.is_empty());
        assert_eq!(fallback_user.id, 0);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}

