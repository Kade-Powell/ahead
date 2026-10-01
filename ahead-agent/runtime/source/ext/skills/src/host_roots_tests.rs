use std::fs;
use std::sync::Arc;

use codex_config::ConfigLayerEntry;
use codex_config::ConfigLayerSource;
use codex_config::ConfigLayerStack;
use codex_config::ConfigRequirementsToml;
use codex_exec_server::LOCAL_FS;
use codex_protocol::protocol::SkillScope;
use codex_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::Semaphore;

use super::resolve_skill_roots_with_home_dir;
use super::resolve_user_home;
use crate::loader::MAX_CONCURRENT_ROOT_SCANS;
use crate::loader::load_and_merge_host_skill_roots;

fn absolute(path: impl Into<std::path::PathBuf>) -> AbsolutePathBuf {
    AbsolutePathBuf::try_from(path.into()).expect("absolute path")
}

fn empty_config() -> toml::Value {
    toml::Value::Table(toml::map::Map::new())
}

fn stack(layers: Vec<ConfigLayerEntry>) -> ConfigLayerStack {
    ConfigLayerStack::new(
        layers,
        Default::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("valid config stack")
}

fn user_layer(config_folder: &AbsolutePathBuf) -> ConfigLayerEntry {
    ConfigLayerEntry::new(
        ConfigLayerSource::User {
            file: config_folder.join("config.toml"),
            profile: None,
        },
        empty_config(),
    )
}

#[test]
fn ahead_user_home_overrides_the_platform_home_for_user_skills() {
    let override_home = std::path::PathBuf::from("ahead-user-home");
    let platform_home = std::path::PathBuf::from("platform-home");

    assert_eq!(
        resolve_user_home(Some(override_home.clone()), Some(platform_home)),
        Some(override_home)
    );
    let platform_home = std::path::PathBuf::from("platform-home");
    assert_eq!(
        resolve_user_home(None, Some(platform_home.clone())),
        Some(platform_home)
    );
}

fn write_skill(root: &AbsolutePathBuf, directory: &str, name: &str) -> AbsolutePathBuf {
    let skill_directory = root.join(directory);
    fs::create_dir_all(&skill_directory).expect("create skill directory");
    let skill_path = skill_directory.join("SKILL.md");
    fs::write(
        &skill_path,
        format!("---\nname: {name}\ndescription: {name} description\n---\n"),
    )
    .expect("write skill");
    AbsolutePathBuf::from_absolute_path(
        dunce::canonicalize(skill_path).expect("canonical skill path"),
    )
    .expect("absolute skill path")
}

#[tokio::test]
async fn discovers_only_the_project_root_agents_skills_directory() {
    let temp_dir = TempDir::new().expect("temp dir");
    let repository = absolute(temp_dir.path().join("repo"));
    let nested = repository.join("nested/workspace");
    fs::create_dir_all(&nested).expect("create nested workspace");
    fs::write(repository.join(".git"), "gitdir: fake\n").expect("write git marker");
    fs::create_dir_all(repository.join(".agents/skills")).expect("create project skills");
    fs::create_dir_all(repository.join(".agent/skills"))
        .expect("create unsupported singular skills root");
    fs::create_dir_all(repository.join(".skills")).expect("create unsupported short skills root");
    fs::create_dir_all(repository.join("nested/.agents/skills"))
        .expect("create nested non-root skills");

    let roots = resolve_skill_roots_with_home_dir(
        Some(Arc::clone(&LOCAL_FS)),
        &stack(Vec::new()),
        &nested,
        None,
    )
    .await;

    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].path, repository.join(".agents/skills"));
    assert_eq!(roots[0].scope, SkillScope::Repo);
}

#[tokio::test]
async fn discovers_the_user_agents_skills_directory() {
    let temp_dir = TempDir::new().expect("temp dir");
    let home = absolute(temp_dir.path().join("home"));
    let config_folder = home.join(".ahead");
    let roots = resolve_skill_roots_with_home_dir(
        None,
        &stack(vec![user_layer(&config_folder)]),
        &home,
        Some(&home),
    )
    .await;

    assert_eq!(roots[0].path, home.join(".agents/skills"));
    assert_eq!(roots[0].scope, SkillScope::User);
}

#[tokio::test]
async fn project_and_user_skills_with_same_name_are_both_retained() {
    let temp_dir = TempDir::new().expect("temp dir");
    let home = absolute(temp_dir.path().join("home"));
    let repository = absolute(temp_dir.path().join("repo"));
    fs::create_dir_all(&repository).expect("create repository");
    fs::write(repository.join(".git"), "gitdir: fake\n").expect("write git marker");
    let project_skill = write_skill(&repository.join(".agents/skills"), "shared", "shared");
    let user_skill = write_skill(&home.join(".agents/skills"), "shared", "shared");

    let roots = resolve_skill_roots_with_home_dir(
        Some(Arc::clone(&LOCAL_FS)),
        &stack(vec![user_layer(&home.join(".ahead"))]),
        &repository,
        Some(&home),
    )
    .await;
    let outcome =
        load_and_merge_host_skill_roots(roots, &Semaphore::new(MAX_CONCURRENT_ROOT_SCANS)).await;

    assert!(outcome.errors.is_empty());
    assert_eq!(
        outcome
            .skills
            .iter()
            .map(|skill| (skill.name.as_str(), skill.scope))
            .collect::<Vec<_>>(),
        vec![("shared", SkillScope::Repo), ("shared", SkillScope::User),]
    );
    assert_eq!(
        outcome
            .skills
            .iter()
            .map(|skill| skill.path_to_skills_md.clone())
            .collect::<Vec<_>>(),
        vec![project_skill, user_skill]
    );
}
