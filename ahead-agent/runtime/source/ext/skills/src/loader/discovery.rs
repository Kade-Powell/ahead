use std::io;

use codex_exec_server::ExecutorFileSystem;
use codex_exec_server::WalkEntryKind;
use codex_exec_server::WalkOptions;
use codex_utils_path_uri::PathUri;

use super::MAX_SKILLS_DIRS_PER_ROOT;
use super::SKILLS_FILENAME;
const MAX_SKILLS_ENTRIES_PER_ROOT: usize = 20_000;
pub(super) const MAX_CONCURRENT_SKILL_LOADS: usize = 2;

pub(super) enum DirectorySymlinkPolicy {
    Follow,
    Ignore,
}

pub(super) struct SkillDiscoveryOptions {
    pub directory_symlinks: DirectorySymlinkPolicy,
}

pub(super) struct SkillDiscovery {
    pub skills: Vec<DiscoveredSkill>,
    pub warnings: Vec<String>,
}

pub(super) struct DiscoveredSkill {
    pub path: PathUri,
}

pub(super) async fn discover_skills(
    file_system: &dyn ExecutorFileSystem,
    root: &PathUri,
    options: SkillDiscoveryOptions,
) -> SkillDiscovery {
    let empty_discovery = || SkillDiscovery {
        skills: Vec::new(),
        warnings: Vec::new(),
    };
    let walk = match file_system
        .walk(
            root,
            WalkOptions {
                max_depth: 2,
                max_directories: MAX_SKILLS_DIRS_PER_ROOT,
                max_entries: MAX_SKILLS_ENTRIES_PER_ROOT,
                follow_directory_symlinks: matches!(
                    options.directory_symlinks,
                    DirectorySymlinkPolicy::Follow
                ),
                prune_hidden_directories: true,
            },
            /*sandbox*/ None,
        )
        .await
    {
        Ok(walk) => walk,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return empty_discovery(),
        Err(error) => {
            let mut discovery = empty_discovery();
            discovery
                .warnings
                .push(format!("failed to walk skills root {root}: {error:#}"));
            return discovery;
        }
    };

    let mut warnings = walk
        .errors
        .into_iter()
        .map(|error| {
            format!(
                "failed to scan skill path {}: {}",
                error.path, error.message
            )
        })
        .collect::<Vec<_>>();
    if walk.truncated {
        warnings.push(format!(
            "skills scan reached its traversal limit (root: {root})"
        ));
    }

    let mut skill_files = Vec::new();
    for entry in walk.entries {
        if has_hidden_ancestor_below_root(&entry.path, root) {
            continue;
        }
        match entry.kind {
            WalkEntryKind::Directory => {}
            WalkEntryKind::File => {
                if entry.path.basename().as_deref() == Some(SKILLS_FILENAME)
                    && entry
                        .path
                        .parent()
                        .and_then(|parent| parent.parent())
                        .as_ref()
                        == Some(root)
                {
                    skill_files.push(entry.path);
                }
            }
        }
    }
    let skills = skill_files
        .into_iter()
        .map(|path| DiscoveredSkill { path })
        .collect();

    SkillDiscovery { skills, warnings }
}

fn has_hidden_ancestor_below_root(path: &PathUri, root: &PathUri) -> bool {
    let mut ancestor = path.parent();
    while let Some(current) = ancestor {
        if &current == root {
            return false;
        }
        if current.basename().is_some_and(|name| name.starts_with('.')) {
            return true;
        }
        ancestor = current.parent();
    }
    false
}
