use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use codex_config::ConfigLayerSource;
use codex_config::ConfigLayerStack;
use codex_config::default_project_root_markers;
use codex_config::merge_toml_values;
use codex_config::project_root_markers_from_config;
use codex_exec_server::ExecutorFileSystem;
use codex_exec_server::GetMetadataOptions;
use codex_exec_server::LOCAL_FS;
use codex_protocol::protocol::SkillScope;
use codex_skills::system_cache_root_dir;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use dirs::home_dir;
use futures::StreamExt;
use toml::Value as TomlValue;

use crate::loader::HostSkillRoot;

const AGENTS_DIR_NAME: &str = ".agents";
const SKILLS_DIR_NAME: &str = "skills";
const MAX_CONCURRENT_ANCESTOR_PROBES: usize = 256;

pub(crate) async fn resolve_skill_roots(
    repository_file_system: Option<Arc<dyn ExecutorFileSystem>>,
    config_layer_stack: &ConfigLayerStack,
    cwd: &AbsolutePathBuf,
) -> Vec<HostSkillRoot> {
    let home_dir = resolve_user_home(
        std::env::var_os("AHEAD_USER_HOME").map(PathBuf::from),
        home_dir(),
    )
    .and_then(|path| AbsolutePathBuf::from_absolute_path_checked(path).ok());
    resolve_skill_roots_with_home_dir(
        repository_file_system,
        config_layer_stack,
        cwd,
        home_dir.as_ref(),
    )
    .await
}

fn resolve_user_home(
    override_home: Option<PathBuf>,
    platform_home: Option<PathBuf>,
) -> Option<PathBuf> {
    override_home.or(platform_home)
}

async fn resolve_skill_roots_with_home_dir(
    repository_file_system: Option<Arc<dyn ExecutorFileSystem>>,
    config_layer_stack: &ConfigLayerStack,
    cwd: &AbsolutePathBuf,
    home_dir: Option<&AbsolutePathBuf>,
) -> Vec<HostSkillRoot> {
    let mut roots =
        repo_agents_skill_roots(repository_file_system.clone(), config_layer_stack, cwd).await;
    roots.extend(roots_from_layer_stack(config_layer_stack, home_dir));
    dedupe_skill_roots_by_path(&mut roots);
    roots
}

fn roots_from_layer_stack(
    config_layer_stack: &ConfigLayerStack,
    home_dir: Option<&AbsolutePathBuf>,
) -> Vec<HostSkillRoot> {
    let mut roots = Vec::new();

    for layer in config_layer_stack.all_layers_high_to_low() {
        let Some(config_folder) = layer.config_folder() else {
            continue;
        };

        match &layer.name {
            ConfigLayerSource::User { .. } => {
                if let Some(home_dir) = home_dir {
                    roots.push(local_root(
                        home_dir.join(AGENTS_DIR_NAME).join(SKILLS_DIR_NAME),
                        SkillScope::User,
                    ));
                }

                roots.push(local_root(
                    system_cache_root_dir(&config_folder),
                    SkillScope::System,
                ));
            }
            ConfigLayerSource::Project { .. }
            | ConfigLayerSource::System { .. }
            | ConfigLayerSource::PackagedDefaults { .. }
            | ConfigLayerSource::Mdm { .. }
            | ConfigLayerSource::EnterpriseManaged { .. }
            | ConfigLayerSource::SessionFlags
            | ConfigLayerSource::LegacyManagedConfigTomlFromFile { .. }
            | ConfigLayerSource::LegacyManagedConfigTomlFromMdm => {}
        }
    }

    roots
}

fn local_root(path: AbsolutePathBuf, scope: SkillScope) -> HostSkillRoot {
    HostSkillRoot::host(path, scope, Arc::clone(&LOCAL_FS))
}

async fn repo_agents_skill_roots(
    repository_file_system: Option<Arc<dyn ExecutorFileSystem>>,
    config_layer_stack: &ConfigLayerStack,
    cwd: &AbsolutePathBuf,
) -> Vec<HostSkillRoot> {
    let Some(repository_file_system) = repository_file_system else {
        return Vec::new();
    };
    let project_root_markers = project_root_markers_from_stack(config_layer_stack);
    let project_root =
        find_project_root(repository_file_system.as_ref(), cwd, &project_root_markers).await;
    let agents_skills = project_root.join(AGENTS_DIR_NAME).join(SKILLS_DIR_NAME);
    let agents_skills_uri = PathUri::from_abs_path(&agents_skills);
    match repository_file_system
        .get_metadata(
            &agents_skills_uri,
            GetMetadataOptions::default(),
            /*sandbox*/ None,
        )
        .await
    {
        Ok(metadata) if metadata.is_directory => vec![HostSkillRoot::host(
            agents_skills,
            SkillScope::Repo,
            repository_file_system,
        )],
        Ok(_) | Err(_) => Vec::new(),
    }
}

fn project_root_markers_from_stack(config_layer_stack: &ConfigLayerStack) -> Vec<String> {
    let mut merged = TomlValue::Table(toml::map::Map::new());
    for layer in config_layer_stack.layers_low_to_high() {
        if matches!(layer.name, ConfigLayerSource::Project { .. }) {
            continue;
        }
        merge_toml_values(&mut merged, &layer.config);
    }

    match project_root_markers_from_config(&merged) {
        Ok(Some(markers)) => markers,
        Ok(None) => default_project_root_markers(),
        Err(error) => {
            tracing::warn!("invalid project_root_markers: {error}");
            default_project_root_markers()
        }
    }
}

async fn find_project_root(
    repository_file_system: &dyn ExecutorFileSystem,
    cwd: &AbsolutePathBuf,
    project_root_markers: &[String],
) -> AbsolutePathBuf {
    if project_root_markers.is_empty() {
        return cwd.clone();
    }

    let mut probes = Vec::new();
    for ancestor in cwd.ancestors() {
        for marker in project_root_markers {
            probes.push((ancestor.clone(), ancestor.join(marker)));
        }
    }
    let mut results = futures::stream::iter(probes)
        .map(|(ancestor, marker_path)| async move {
            let marker_path_uri = PathUri::from_abs_path(&marker_path);
            let result = repository_file_system
                .get_metadata(
                    &marker_path_uri,
                    GetMetadataOptions::default(),
                    /*sandbox*/ None,
                )
                .await;
            (ancestor, marker_path, result)
        })
        .buffered(MAX_CONCURRENT_ANCESTOR_PROBES);
    while let Some((ancestor, marker_path, result)) = results.next().await {
        match result {
            Ok(_) => return ancestor,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(
                    "failed to stat project root marker {}: {error:#}",
                    marker_path.display()
                );
            }
        }
    }

    cwd.clone()
}

fn dedupe_skill_roots_by_path(roots: &mut Vec<HostSkillRoot>) {
    let mut seen = HashSet::new();
    roots.retain(|root| seen.insert(root.path.clone()));
}

#[cfg(test)]
#[path = "host_roots_tests.rs"]
mod tests;
