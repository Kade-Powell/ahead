use std::fs;
use std::sync::Arc;

use codex_config::ConfigLayerStack;
use codex_config::ConfigRequirementsToml;
use codex_exec_server::LOCAL_FS;
use codex_extension_api::ContextualUserFragment;
use codex_utils_absolute_path::AbsolutePathBuf;
use tempfile::TempDir;

use super::HostSkillsLoadInput;
use super::HostSkillsService;
use crate::host_snapshot::host_skill_resource_id;

fn stack() -> ConfigLayerStack {
    ConfigLayerStack::new(
        Vec::new(),
        Default::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("valid config stack")
}

fn write_skill(path: &AbsolutePathBuf, description: &str) {
    fs::create_dir_all(path.parent().expect("skill parent")).expect("create skill directory");
    fs::write(
        path,
        format!("---\nname: demo\ndescription: {description}\n---\n"),
    )
    .expect("write skill");
}

#[tokio::test]
async fn caches_project_skills_until_explicitly_cleared() {
    let temp_dir = TempDir::new().expect("temp dir");
    let repository =
        AbsolutePathBuf::from_absolute_path(temp_dir.path().join("repo")).expect("absolute repo");
    fs::create_dir_all(&repository).expect("create repository");
    fs::write(repository.join(".git"), "gitdir: fake\n").expect("write git marker");
    let skill_path = repository.join(".agents/skills/demo/SKILL.md");
    write_skill(&skill_path, "first");
    let service = HostSkillsService::new(repository.join(".ahead/runtime"), false);
    let input = HostSkillsLoadInput::new(repository.clone(), stack());

    let first = service
        .snapshot_for_config(&input, Some(Arc::clone(&LOCAL_FS)))
        .await;
    assert_eq!(first.outcome().skills[0].description, "first");

    write_skill(&skill_path, "second");
    let cached = service
        .snapshot_for_config(&input, Some(Arc::clone(&LOCAL_FS)))
        .await;
    assert_eq!(cached.outcome().skills[0].description, "first");

    service.clear_cache();
    let refreshed = service
        .snapshot_for_config(&input, Some(Arc::clone(&LOCAL_FS)))
        .await;
    assert_eq!(refreshed.outcome().skills[0].description, "second");
}

#[tokio::test]
async fn selected_host_skill_prompt_uses_an_opaque_locator() {
    let temp_dir = TempDir::new().expect("temp dir");
    let repository =
        AbsolutePathBuf::from_absolute_path(temp_dir.path().join("repo")).expect("absolute repo");
    fs::create_dir_all(&repository).expect("create repository");
    fs::write(repository.join(".git"), "gitdir: fake\n").expect("write git marker");
    let skill_path = repository.join(".agents/skills/demo/SKILL.md");
    write_skill(&skill_path, "Demo skill");
    let service = HostSkillsService::new(repository.join(".ahead/runtime"), false);
    let input = HostSkillsLoadInput::new(repository.clone(), stack());
    let snapshot = service
        .snapshot_for_config(&input, Some(Arc::clone(&LOCAL_FS)))
        .await;
    let selected_skills = snapshot.outcome().skills.clone();

    let prompts = snapshot.load_skill_prompts(&selected_skills).await;

    let fragment = prompts.fragments.first().expect("skill prompt fragment");
    let body = fragment.body();
    let expected_handle = host_skill_resource_id(&selected_skills[0]);
    assert!(body.contains(&format!("<path>{expected_handle}</path>")));
    assert!(!body.contains(repository.as_path().to_string_lossy().as_ref()));
    assert!(prompts.warnings.is_empty());

    fs::write(
        &skill_path,
        format!(
            "---\nname: demo\ndescription: Demo skill\n---\n{}",
            "</skill".repeat(1_300)
        ),
    )
    .expect("write oversized skill");
    let oversized_prompts = snapshot.load_skill_prompts(&selected_skills).await;
    let oversized_body = oversized_prompts
        .fragments
        .first()
        .expect("bounded skill prompt fragment")
        .body();
    assert!(oversized_body.len() <= 8_200);
    assert_eq!(
        oversized_prompts.warnings,
        vec!["Skill `demo` exceeded the main prompt context limit and was truncated."]
    );

    fs::remove_file(&skill_path).expect("remove skill file");
    let failed_prompts = snapshot.load_skill_prompts(&selected_skills).await;
    assert_eq!(
        failed_prompts.warnings,
        vec!["Failed to load host skill demo."]
    );
}
