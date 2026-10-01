use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use codex_exec_server::ExecutorFileSystem;
use codex_protocol::protocol::SkillScope;
use codex_utils_absolute_path::AbsolutePathBuf;
use futures::StreamExt;
use tokio::sync::Semaphore;

use super::HostSkillRoot;
use super::MAX_CONCURRENT_ROOT_SCANS;
use super::host::HostSkillRootSnapshot;
use super::load_host_skill_root;
use crate::SkillLoadOutcome;
use crate::host_service::RequestSkillRootSnapshots;
use crate::host_service::config_skill_root_cache_key;

#[cfg(test)]
pub(crate) async fn load_and_merge_host_skill_roots(
    roots: Vec<HostSkillRoot>,
    root_scan_slots: &Semaphore,
) -> SkillLoadOutcome {
    load_and_merge_host_skill_roots_with_request_snapshots(roots, root_scan_slots, None).await
}

pub(crate) async fn load_and_merge_host_skill_roots_with_request_snapshots(
    roots: Vec<HostSkillRoot>,
    root_scan_slots: &Semaphore,
    request_root_snapshots: Option<&RequestSkillRootSnapshots>,
) -> SkillLoadOutcome {
    let mut indexed_snapshots = futures::stream::iter(roots.into_iter().enumerate())
        .map(|(root_index, root)| async move {
            let request_snapshot = request_root_snapshots.map(|snapshots| {
                Arc::clone(
                    snapshots
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .entry(config_skill_root_cache_key(&root))
                        .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new())),
                )
            });
            let load = async {
                let _root_scan_slot = root_scan_slots
                    .acquire()
                    .await
                    .unwrap_or_else(|_| unreachable!());
                load_host_skill_root(root).await
            };
            let snapshot = if let Some(snapshot) = request_snapshot {
                snapshot.get_or_init(|| load).await.clone()
            } else {
                load.await
            };
            (root_index, snapshot)
        })
        .buffer_unordered(MAX_CONCURRENT_ROOT_SCANS)
        .collect::<Vec<_>>()
        .await;
    indexed_snapshots.sort_unstable_by_key(|(root_index, _)| *root_index);

    merge_host_skill_root_snapshots(
        indexed_snapshots
            .into_iter()
            .map(|(_, snapshot)| snapshot)
            .collect(),
    )
}

fn merge_host_skill_root_snapshots(snapshots: Vec<HostSkillRootSnapshot>) -> SkillLoadOutcome {
    let mut skills = Vec::new();
    let mut errors = Vec::new();
    let mut skill_discovery_path_by_path = HashMap::new();
    let mut file_systems_by_skill_path =
        HashMap::<AbsolutePathBuf, Arc<dyn ExecutorFileSystem>>::new();

    for snapshot in snapshots {
        for skill in &snapshot.skills {
            let path = skill.path_to_skills_md.clone();
            if let Some(discovery_path) = snapshot.skill_discovery_path_by_path.get(&path) {
                skill_discovery_path_by_path
                    .entry(path.clone())
                    .or_insert_with(|| discovery_path.clone());
            }
            file_systems_by_skill_path
                .entry(path)
                .or_insert_with(|| Arc::clone(&snapshot.file_system));
        }
        skills.extend(snapshot.skills);
        errors.extend(snapshot.errors);
    }

    let mut seen_paths = HashSet::new();
    skills.retain(|skill| seen_paths.insert(skill.path_to_skills_md.clone()));
    let retained_paths = skills
        .iter()
        .map(|skill| skill.path_to_skills_md.clone())
        .collect::<HashSet<_>>();
    skill_discovery_path_by_path.retain(|path, _| retained_paths.contains(path));
    file_systems_by_skill_path.retain(|path, _| retained_paths.contains(path));
    // AHEAD's slash picker carries the source, so same-named skills stay addressable.
    skills.sort_by(|left, right| {
        scope_rank(left.scope)
            .cmp(&scope_rank(right.scope))
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.path_to_skills_md.cmp(&right.path_to_skills_md))
    });
    SkillLoadOutcome::from_parts(
        skills,
        errors,
        skill_discovery_path_by_path,
        file_systems_by_skill_path,
    )
}

fn scope_rank(scope: SkillScope) -> u8 {
    match scope {
        SkillScope::Repo => 0,
        SkillScope::User => 1,
        SkillScope::System => 2,
        SkillScope::Admin => 3,
    }
}

#[cfg(test)]
#[path = "host_merge_tests.rs"]
mod tests;
