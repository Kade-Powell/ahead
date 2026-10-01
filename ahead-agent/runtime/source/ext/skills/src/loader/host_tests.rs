use std::fs;
use std::sync::Arc;

use codex_exec_server::LOCAL_FS;
use codex_protocol::protocol::SkillScope;
use codex_skills::SkillMetadata;
use codex_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::HostSkillRoot;
use super::MAX_SKILL_FILE_SIZE_BYTES;
use super::load_host_skill_root;

fn write_skill(root: &TempDir, directory: &str, frontmatter: &str) -> AbsolutePathBuf {
    let skill_directory = root.path().join(directory);
    fs::create_dir_all(&skill_directory).expect("create skill directory");
    let skill_path = skill_directory.join("SKILL.md");
    fs::write(
        &skill_path,
        format!("---\n{frontmatter}\n---\n\n# Instructions\n"),
    )
    .expect("write skill");
    AbsolutePathBuf::from_absolute_path(fs::canonicalize(skill_path).expect("canonical skill path"))
        .expect("absolute skill path")
}

fn root_for(root: &TempDir, scope: SkillScope) -> HostSkillRoot {
    HostSkillRoot::host(
        AbsolutePathBuf::from_absolute_path(root.path()).expect("absolute root"),
        scope,
        Arc::clone(&LOCAL_FS),
    )
}

#[tokio::test]
async fn loads_skill_invocation_policy_from_standard_frontmatter() {
    let root = TempDir::new().expect("temp dir");
    let skill_path = write_skill(
        &root,
        "demo",
        "name: demo\ndescription: Demo skill\ndisable-model-invocation: true\nmetadata:\n  short-description: Short demo",
    );

    let snapshot = load_host_skill_root(root_for(&root, SkillScope::User)).await;

    assert_eq!(snapshot.errors, Vec::new());
    assert_eq!(
        snapshot.skills,
        vec![SkillMetadata {
            name: "demo".to_string(),
            description: "Demo skill".to_string(),
            short_description: Some("Short demo".to_string()),
            disable_model_invocation: true,
            path_to_skills_md: skill_path,
            scope: SkillScope::User,
        }]
    );
}

#[tokio::test]
async fn rejects_skill_names_that_do_not_match_the_package_directory() {
    let root = TempDir::new().expect("temp dir");
    write_skill(&root, "demo", "name: other\ndescription: Demo skill");

    let snapshot = load_host_skill_root(root_for(&root, SkillScope::Repo)).await;

    assert!(snapshot.skills.is_empty());
    assert_eq!(snapshot.errors.len(), 1);
}

#[tokio::test]
async fn rejects_oversized_skill_files_with_bounded_diagnostics() {
    let root = TempDir::new().expect("temp dir");
    let skill_directory = root.path().join("large");
    fs::create_dir_all(&skill_directory).expect("create skill directory");
    fs::write(
        skill_directory.join("SKILL.md"),
        format!(
            "---\nname: large\ndescription: Large skill\n---\n{}",
            "x".repeat(MAX_SKILL_FILE_SIZE_BYTES)
        ),
    )
    .expect("write oversized skill");

    let snapshot = load_host_skill_root(root_for(&root, SkillScope::Repo)).await;

    assert!(snapshot.skills.is_empty());
    assert_eq!(snapshot.errors.len(), 1);
    assert!(snapshot.errors[0].message.contains("102400-byte limit"));
    assert!(
        !snapshot.errors[0]
            .message
            .contains(root.path().to_str().expect("root path is UTF-8"))
    );
}

#[tokio::test]
async fn scans_only_direct_child_skills_and_skips_hidden_directories() {
    let root = TempDir::new().expect("temp dir");
    let visible_path = write_skill(
        &root,
        "visible",
        "name: visible\ndescription: Visible skill",
    );
    write_skill(&root, ".hidden", "name: hidden\ndescription: Hidden skill");
    write_skill(
        &root,
        "nested/ignored",
        "name: ignored\ndescription: Nested skill",
    );

    let snapshot = load_host_skill_root(root_for(&root, SkillScope::Repo)).await;

    assert_eq!(snapshot.errors, Vec::new());
    assert_eq!(snapshot.skills.len(), 1);
    assert_eq!(snapshot.skills[0].path_to_skills_md, visible_path);
}

#[cfg(unix)]
#[tokio::test]
async fn follows_project_and_user_symlinks_but_not_system_symlinks() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().expect("temp dir");
    let target = TempDir::new().expect("target temp dir");
    let target_skill = write_skill(&target, "demo", "name: demo\ndescription: Symlinked skill");
    symlink(target.path().join("demo"), root.path().join("alias")).expect("create skill symlink");

    for scope in [SkillScope::Repo, SkillScope::User] {
        let snapshot = load_host_skill_root(root_for(&root, scope)).await;
        assert_eq!(snapshot.skills.len(), 1);
        assert_eq!(snapshot.skills[0].path_to_skills_md, target_skill);
    }

    let system_snapshot = load_host_skill_root(root_for(&root, SkillScope::System)).await;
    assert_eq!(system_snapshot.skills, Vec::new());
}
