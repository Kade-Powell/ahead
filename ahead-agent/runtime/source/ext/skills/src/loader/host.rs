use std::collections::HashMap;
use std::sync::Arc;

use codex_exec_server::ExecutorFileSystem;
use codex_protocol::protocol::SkillScope;
use codex_skills::ParsedSkillFrontmatter;
use codex_skills::SkillError;
use codex_skills::SkillMetadata;
use codex_skills::parse_skill_frontmatter_metadata;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use futures::StreamExt;
use tracing::error;

use super::discovery::DirectorySymlinkPolicy;
use super::discovery::MAX_CONCURRENT_SKILL_LOADS;
use super::discovery::SkillDiscovery;
use super::discovery::SkillDiscoveryOptions;
use super::discovery::discover_skills;

const MAX_SKILL_FILE_SIZE_BYTES: usize = 100 * 1024;

/// A resolved host skill root ready for filesystem discovery.
pub(crate) struct HostSkillRoot {
    pub(crate) path: AbsolutePathBuf,
    pub(crate) scope: SkillScope,
    pub(crate) file_system: Arc<dyn ExecutorFileSystem>,
}

impl HostSkillRoot {
    pub(crate) fn host(
        path: AbsolutePathBuf,
        scope: SkillScope,
        file_system: Arc<dyn ExecutorFileSystem>,
    ) -> Self {
        Self {
            path,
            scope,
            file_system,
        }
    }
}

/// Skills and errors loaded from one canonical host root.
#[derive(Clone)]
pub(crate) struct HostSkillRootSnapshot {
    pub(crate) skills: Vec<SkillMetadata>,
    pub(crate) skill_discovery_path_by_path: Arc<HashMap<AbsolutePathBuf, AbsolutePathBuf>>,
    pub(crate) errors: Vec<SkillError>,
    pub(crate) file_system: Arc<dyn ExecutorFileSystem>,
}

struct ResolvedDiscoveredSkill {
    discovery_path: PathUri,
    path: AbsolutePathBuf,
    path_uri: PathUri,
}

pub(crate) async fn load_host_skill_root(root: HostSkillRoot) -> HostSkillRootSnapshot {
    let canonical_root =
        canonicalize_for_skill_identity(root.file_system.as_ref(), &root.path).await;
    let (skills, skill_discovery_path_by_path, errors) =
        load_skills_under_root(&root, &canonical_root).await;
    HostSkillRootSnapshot {
        skills,
        skill_discovery_path_by_path,
        errors,
        file_system: root.file_system,
    }
}

async fn load_skills_under_root(
    skill_root: &HostSkillRoot,
    root: &AbsolutePathBuf,
) -> (
    Vec<SkillMetadata>,
    Arc<HashMap<AbsolutePathBuf, AbsolutePathBuf>>,
    Vec<SkillError>,
) {
    let file_system = skill_root.file_system.as_ref();
    let directory_symlinks = match skill_root.scope {
        SkillScope::User | SkillScope::Repo | SkillScope::Admin => DirectorySymlinkPolicy::Follow,
        SkillScope::System => DirectorySymlinkPolicy::Ignore,
    };
    let SkillDiscovery { skills, warnings } = discover_skills(
        file_system,
        &PathUri::from_abs_path(root),
        SkillDiscoveryOptions { directory_symlinks },
    )
    .await;
    for warning in warnings {
        error!("{warning}");
    }
    if skills.is_empty() {
        return (Vec::new(), Arc::default(), Vec::new());
    }

    let resolved_skills = futures::stream::iter(skills)
        .map(|skill| async move {
            let path_uri = match file_system
                .canonicalize(&skill.path, /*sandbox*/ None)
                .await
            {
                Ok(path) => path,
                Err(_) => skill.path.clone(),
            };
            let path = match path_uri.to_abs_path() {
                Ok(path) => path,
                Err(error) => {
                    error!("failed to convert discovered skill path {path_uri}: {error}");
                    return None;
                }
            };
            Some(ResolvedDiscoveredSkill {
                discovery_path: skill.path,
                path,
                path_uri,
            })
        })
        .buffered(MAX_CONCURRENT_SKILL_LOADS)
        .filter_map(futures::future::ready)
        .collect::<Vec<_>>()
        .await;
    let skill_results = futures::stream::iter(resolved_skills)
        .map(|skill| async move {
            let discovery_path = skill
                .discovery_path
                .to_abs_path()
                .unwrap_or_else(|_| skill.path.clone());
            let result =
                parse_skill_file(file_system, &skill.path, &skill.path_uri, skill_root.scope).await;
            (skill.path, discovery_path, result)
        })
        .buffered(MAX_CONCURRENT_SKILL_LOADS)
        .collect::<Vec<_>>();
    let skill_results = skill_results.await;

    let mut loaded_skills = Vec::new();
    let mut skill_discovery_path_by_path = HashMap::new();
    let mut errors = Vec::new();
    for (path, discovery_path, result) in skill_results {
        match result {
            Ok(skill) => {
                skill_discovery_path_by_path
                    .insert(skill.path_to_skills_md.clone(), discovery_path);
                loaded_skills.push(skill);
            }
            Err(message) if skill_root.scope != SkillScope::System => {
                errors.push(SkillError { path, message });
            }
            Err(_) => {}
        }
    }
    (
        loaded_skills,
        Arc::new(skill_discovery_path_by_path),
        errors,
    )
}

async fn parse_skill_file(
    file_system: &dyn ExecutorFileSystem,
    path: &AbsolutePathBuf,
    path_uri: &PathUri,
    scope: SkillScope,
) -> Result<SkillMetadata, String> {
    let parent = path
        .parent()
        .ok_or_else(|| "skill directory name must be valid UTF-8".to_string())?;
    let directory_name = parent
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "skill directory name must be valid UTF-8".to_string())?;
    let metadata = file_system
        .get_metadata(path_uri, Default::default(), /*sandbox*/ None)
        .await
        .map_err(|error| format!("failed to inspect SKILL.md: {error}"))?;
    if !metadata.is_file {
        return Err("SKILL.md is not a file".to_string());
    }
    if metadata.size > MAX_SKILL_FILE_SIZE_BYTES as u64 {
        return Err(format!(
            "SKILL.md exceeds the {MAX_SKILL_FILE_SIZE_BYTES}-byte limit"
        ));
    }

    let capacity = usize::try_from(metadata.size)
        .map_err(|_| "SKILL.md exceeds the supported size limit".to_string())?;
    let mut stream = file_system
        .read_file_stream(path_uri, /*sandbox*/ None)
        .await
        .map_err(|error| format!("failed to read SKILL.md: {error}"))?;
    let mut contents = Vec::with_capacity(capacity);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("failed to read SKILL.md: {error}"))?;
        let Some(new_size) = contents.len().checked_add(chunk.len()) else {
            return Err("SKILL.md exceeds the supported size limit".to_string());
        };
        if new_size > MAX_SKILL_FILE_SIZE_BYTES {
            return Err(format!(
                "SKILL.md exceeds the {MAX_SKILL_FILE_SIZE_BYTES}-byte limit"
            ));
        }
        contents.extend_from_slice(&chunk);
    }
    let contents = String::from_utf8(contents)
        .map_err(|error| format!("SKILL.md is not valid UTF-8: {error}"))?;
    let ParsedSkillFrontmatter {
        name,
        description,
        short_description,
        disable_model_invocation,
    } = parse_skill_frontmatter_metadata(&contents, directory_name)
        .map_err(|error| error.to_string())?;

    Ok(SkillMetadata {
        name,
        description,
        short_description,
        disable_model_invocation,
        path_to_skills_md: path.clone(),
        scope,
    })
}

async fn canonicalize_for_skill_identity(
    file_system: &dyn ExecutorFileSystem,
    path: &AbsolutePathBuf,
) -> AbsolutePathBuf {
    let path_uri = PathUri::from_abs_path(path);
    file_system
        .canonicalize(&path_uri, /*sandbox*/ None)
        .await
        .and_then(|path| path.to_abs_path())
        .unwrap_or_else(|_| path.clone())
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
