use std::collections::HashSet;
use std::io;
use std::sync::Arc;

use codex_exec_server::LOCAL_FS;
use codex_protocol::protocol::SkillScope;
use codex_utils_absolute_path::AbsolutePathBuf;
use tokio::sync::Semaphore;

use crate::host_snapshot::HostSkillsSnapshot;
use crate::host_snapshot::host_skill_resource_id;
use crate::loader::HostSkillRoot;
use crate::loader::load_and_merge_host_skill_roots;

#[tokio::test]
async fn reads_package_resources_without_leaving_the_selected_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    #[cfg(unix)]
    let outside = tempfile::tempdir()?;
    let skill_directory = root.path().join("demo");
    let references_directory = skill_directory.join("references");
    std::fs::create_dir_all(&references_directory)?;
    std::fs::write(
        skill_directory.join("SKILL.md"),
        "---\nname: demo\ndescription: Demo skill.\n---\n# Demo\n",
    )?;
    std::fs::write(
        references_directory.join("guide.md"),
        "Reference content owned by the package.\n",
    )?;
    #[cfg(unix)]
    {
        let outside_file = outside.path().join("outside.md");
        std::fs::write(&outside_file, "outside the package")?;
        std::os::unix::fs::symlink(&outside_file, references_directory.join("escape.md"))?;
    }

    let root = AbsolutePathBuf::try_from(std::fs::canonicalize(root.path())?)?;
    let outcome = load_and_merge_host_skill_roots(
        vec![HostSkillRoot::host(
            root,
            SkillScope::Repo,
            Arc::clone(&LOCAL_FS),
        )],
        &Semaphore::new(/*permits*/ 1),
    )
    .await;
    let skill = outcome
        .skills
        .first()
        .ok_or("host skill was not discovered")?;
    let package = host_skill_resource_id(skill);
    let snapshot = HostSkillsSnapshot::new(Arc::new(outcome.clone()));

    let main_resource = snapshot.read_package_resource(&package, None).await?;
    assert!(main_resource.contains("# Demo"));
    let nested_resource = format!("{package}/references/guide.md");
    assert_eq!(
        snapshot
            .read_package_resource(&package, Some(&nested_resource))
            .await?,
        "Reference content owned by the package.\n"
    );

    for invalid_resource in [
        format!("{package}/../outside.md"),
        format!(r"{package}\references\..\outside.md"),
    ] {
        assert!(
            snapshot
                .read_package_resource(&package, Some(&invalid_resource))
                .await
                .is_err()
        );
    }
    assert!(
        snapshot
            .read_package_resource("unknown-host-skill", None)
            .await
            .is_err()
    );
    #[cfg(unix)]
    assert!(
        snapshot
            .read_package_resource(&package, Some(&format!("{package}/references/escape.md")))
            .await
            .is_err()
    );

    let disabled_path = skill.path_to_skills_md.clone();
    let disabled_outcome = outcome.with_disabled_paths(HashSet::from([disabled_path]));
    let disabled_snapshot = HostSkillsSnapshot::new(Arc::new(disabled_outcome));
    let error = disabled_snapshot
        .read_package_resource(&package, None)
        .await
        .err()
        .ok_or("disabled host skill was readable")?;
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);

    Ok(())
}
