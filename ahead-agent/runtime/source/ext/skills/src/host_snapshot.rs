use std::io;
use std::sync::Arc;

use crate::SkillLoadOutcome;
use codex_skills::SkillMetadata;
use codex_utils_path_uri::PathUri;
use sha2::Digest;
use sha2::Sha256;

/// Immutable snapshot of host-owned skills and their source filesystems.
#[derive(Debug, Clone)]
pub struct HostSkillsSnapshot {
    outcome: Arc<SkillLoadOutcome>,
}

impl HostSkillsSnapshot {
    pub fn new(outcome: Arc<SkillLoadOutcome>) -> Self {
        Self { outcome }
    }

    pub fn outcome(&self) -> &SkillLoadOutcome {
        self.outcome.as_ref()
    }

    pub async fn read_skill_text(&self, skill: &SkillMetadata) -> io::Result<String> {
        self.outcome.read_skill_text(skill).await
    }

    /// Reads a selected host skill by its opaque package handle.
    ///
    /// `resource` is either the package handle itself or that handle followed by a
    /// package-relative path. Host paths are never accepted as model-visible identifiers.
    pub async fn read_package_resource(
        &self,
        package: &str,
        resource: Option<&str>,
    ) -> io::Result<String> {
        let skill = self
            .outcome
            .skills
            .iter()
            .find(|skill| host_skill_resource_id(skill) == package)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "host skill is not loaded"))?;
        if !self.outcome.is_skill_enabled(skill) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "host skill is disabled",
            ));
        }

        let Some(resource) = resource.filter(|resource| *resource != package) else {
            return self.read_skill_text(skill).await;
        };
        let relative_path = resource
            .strip_prefix(package)
            .and_then(|suffix| suffix.strip_prefix('/'))
            .filter(|path| !path.is_empty())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid skill resource"))?;
        self.read_skill_resource(skill, relative_path).await
    }

    pub(crate) async fn read_skill_resource(
        &self,
        skill: &SkillMetadata,
        relative_path: &str,
    ) -> io::Result<String> {
        self.outcome.read_skill_resource(skill, relative_path).await
    }
}

/// Keeps host paths out of model-visible locators while distinguishing equal names and scopes.
pub(crate) fn host_skill_resource_id(skill: &SkillMetadata) -> String {
    let path_uri = PathUri::from_abs_path(&skill.path_to_skills_md);
    let path_fingerprint = Sha256::digest(path_uri.to_string().as_bytes());
    let scope = match skill.scope {
        codex_protocol::protocol::SkillScope::User => "user",
        codex_protocol::protocol::SkillScope::Repo => "project",
        codex_protocol::protocol::SkillScope::System => "system",
        codex_protocol::protocol::SkillScope::Admin => "admin",
    };
    format!("host-skill:{scope}:{}:{path_fingerprint:x}", skill.name,)
}

#[cfg(test)]
#[path = "host_snapshot_tests.rs"]
mod tests;
