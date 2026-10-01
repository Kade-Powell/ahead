use std::fs;
use std::sync::Arc;

use codex_exec_server::LOCAL_FS;
use codex_protocol::protocol::SkillScope;
use codex_utils_absolute_path::AbsolutePathBuf;
use tempfile::TempDir;
use tokio::sync::Semaphore;

use super::HostSkillRoot;
use super::load_and_merge_host_skill_roots;

fn write_skill(root: &AbsolutePathBuf, directory: &str, name: &str) {
    let skill_directory = root.join(directory);
    fs::create_dir_all(&skill_directory).expect("create skill directory");
    fs::write(
        skill_directory.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {name} description\n---\n"),
    )
    .expect("write skill");
}

fn root(path: AbsolutePathBuf, scope: SkillScope) -> HostSkillRoot {
    HostSkillRoot::host(path, scope, Arc::clone(&LOCAL_FS))
}

#[tokio::test]
async fn same_named_project_and_user_skills_are_both_retained() {
    let temp_dir = TempDir::new().expect("temp dir");
    let base = AbsolutePathBuf::from_absolute_path(temp_dir.path()).expect("absolute temp dir");
    let project_root = base.join("project");
    let user_root = base.join("user");
    write_skill(&project_root, "shared", "shared");
    write_skill(&user_root, "shared", "shared");

    let outcome = load_and_merge_host_skill_roots(
        vec![
            root(user_root, SkillScope::User),
            root(project_root, SkillScope::Repo),
        ],
        &Semaphore::new(2),
    )
    .await;

    assert!(outcome.errors.is_empty());
    assert_eq!(
        outcome
            .skills
            .iter()
            .map(|skill| (skill.name.as_str(), skill.scope))
            .collect::<Vec<_>>(),
        vec![("shared", SkillScope::Repo), ("shared", SkillScope::User),]
    );
}

#[tokio::test]
async fn distinct_project_and_user_skills_are_both_retained() {
    let temp_dir = TempDir::new().expect("temp dir");
    let base = AbsolutePathBuf::from_absolute_path(temp_dir.path()).expect("absolute temp dir");
    let project_root = base.join("project");
    let user_root = base.join("user");
    write_skill(&project_root, "project", "project");
    write_skill(&user_root, "user", "user");

    let outcome = load_and_merge_host_skill_roots(
        vec![
            root(project_root, SkillScope::Repo),
            root(user_root, SkillScope::User),
        ],
        &Semaphore::new(2),
    )
    .await;

    assert_eq!(
        outcome
            .skills
            .iter()
            .map(|skill| (skill.name.as_str(), skill.scope))
            .collect::<Vec<_>>(),
        vec![("project", SkillScope::Repo), ("user", SkillScope::User)]
    );
}
