use std::{
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
};

use crate::store::InstructionFileSource;
pub use ahead_rpc::ahead::MemoryScope;
use ahead_rpc::ahead::TurnEditorContext;
use ahead_runtime_api::{
    LoadUserInstructionsFuture, LoadedUserInstructions, UserInstructionsProvider,
};
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

const TARGET_INSTRUCTIONS_MAX_BYTES: usize = 32 * 1024;
const TARGET_INSTRUCTION_FILE_MAX_BYTES: usize = 1024 * 1024;

struct TargetInstructionEntry {
    source_path: PathBuf,
    source: String,
    sha256: String,
    contents: String,
    targets: Vec<String>,
}

#[derive(Debug, Default)]
pub(crate) struct TargetInstructionContext {
    pub(crate) prompt: String,
    pub(crate) sources: Vec<InstructionFileSource>,
    truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySource {
    pub scope: MemoryScope,
    pub path: PathBuf,
}

/// The runtime API requires a user-instruction provider. AHEAD relies on the
/// retained core's repository `AGENTS.md` discovery and has no private global
/// instruction-file location.
#[derive(Debug, Default)]
pub(crate) struct EmptyUserInstructionsProvider;

impl UserInstructionsProvider for EmptyUserInstructionsProvider {
    fn load_user_instructions(&self) -> LoadUserInstructionsFuture<'_> {
        Box::pin(async { LoadedUserInstructions::default() })
    }
}

pub(crate) fn user_home() -> Option<PathBuf> {
    std::env::var_os("AHEAD_USER_HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
}

fn user_root() -> Option<PathBuf> {
    user_home().map(|home| home.join(".ahead"))
}

pub fn memory_sources(workspace: &Path) -> Vec<MemorySource> {
    let mut sources = vec![MemorySource {
        scope: MemoryScope::Project,
        path: workspace.join(".ahead/memories/MEMORY.md"),
    }];
    if let Some(root) = user_root() {
        sources.push(MemorySource {
            scope: MemoryScope::User,
            path: root.join("memories/MEMORY.md"),
        });
    }
    sources
}

/// Loads nested project instructions only for editor targets captured in this
/// turn. Workspace-ancestor instructions are already loaded by the core.
pub(crate) fn target_instruction_context(
    workspace: &Path,
    context: &TurnEditorContext,
) -> Result<TargetInstructionContext> {
    let workspace_root = workspace
        .canonicalize()
        .context("AHEAD workspace is unavailable for target instructions")?;
    let mut targets = Vec::<(PathBuf, String)>::new();
    add_target_path(
        &workspace_root,
        workspace,
        &context.active_path,
        true,
        &mut targets,
    );
    for file in &context.attached_files {
        add_target_path(&workspace_root, workspace, &file.path, false, &mut targets);
    }
    target_instruction_context_for_paths(
        &workspace_root,
        &workspace_root,
        targets,
        false,
    )
}

/// Loads the full project `AGENTS.md` hierarchy for file targets used by an
/// editor inference request. Unlike native agent turns, this path has no core
/// instruction loader, so it includes the repository hierarchy itself.
pub fn project_instruction_prompt_for_targets(
    workspace: &Path,
    target_paths: &[&str],
) -> Result<String> {
    let workspace_root = workspace
        .canonicalize()
        .context("AHEAD workspace is unavailable for project instructions")?;
    let instruction_root = project_instruction_root(&workspace_root);
    let mut targets = Vec::<(PathBuf, String)>::new();
    for target_path in target_paths {
        add_target_path(&workspace_root, workspace, target_path, true, &mut targets);
    }
    let context = target_instruction_context_for_paths(
        &workspace_root,
        &instruction_root,
        targets,
        true,
    )?;
    if context.truncated {
        bail!(
            "applicable AGENTS.md instructions exceed AHEAD's 32 KiB prediction context limit"
        );
    }
    Ok(context.prompt)
}

fn target_instruction_context_for_paths(
    workspace_root: &Path,
    instruction_root: &Path,
    targets: Vec<(PathBuf, String)>,
    include_workspace_root: bool,
) -> Result<TargetInstructionContext> {
    if targets.is_empty() {
        return Ok(TargetInstructionContext::default());
    }

    let mut entries = Vec::<TargetInstructionEntry>::new();
    let mut remaining = TARGET_INSTRUCTIONS_MAX_BYTES;
    let mut truncated = false;
    'targets: for (target_path, target_label) in targets {
        let mut directories = Vec::new();
        let target = workspace_root.join(&target_path);
        let mut directory = target.parent();
        while let Some(path) = directory {
            if !path.starts_with(instruction_root)
                || (!include_workspace_root && path == workspace_root)
            {
                break;
            }
            directories.push(path.strip_prefix(instruction_root)?.to_path_buf());
            if path == instruction_root {
                break;
            }
            directory = path.parent();
        }
        directories.reverse();

        for directory in directories {
            let Some(source_path) = ahead_core::search::resolve_open_buffer_path(
                instruction_root,
                &directory.join("AGENTS.md"),
            ) else {
                continue;
            };
            let Some(relative_source) =
                source_path.strip_prefix(instruction_root).ok()
            else {
                continue;
            };
            let source_path = relative_source.to_path_buf();

            if let Some(entry) = entries
                .iter_mut()
                .find(|entry| entry.source_path == source_path)
            {
                if !entry.targets.contains(&target_label) {
                    entry.targets.push(target_label.clone());
                }
                continue;
            }
            let source = path_label(&source_path);
            let Some((sha256, contents, source_truncated)) =
                read_target_instruction(
                    &instruction_root.join(&source_path),
                    &source,
                    remaining,
                )?
            else {
                continue;
            };
            remaining = remaining.saturating_sub(contents.len());
            entries.push(TargetInstructionEntry {
                source_path,
                source,
                sha256,
                contents,
                targets: vec![target_label.clone()],
            });
            if source_truncated {
                truncated = true;
                break 'targets;
            }
        }
    }

    if entries.is_empty() {
        return Ok(TargetInstructionContext::default());
    }
    let sources = entries
        .iter()
        .map(|entry| InstructionFileSource {
            path: entry.source.clone(),
            content_sha256: entry.sha256.clone(),
            targets: entry.targets.clone(),
        })
        .collect();
    let scope_note = if include_workspace_root {
        "Applicable files are discovered from the workspace root inward."
    } else {
        "Workspace-ancestor instructions are already included separately."
    };
    let mut prompt = format!(
        "[AHEAD target-scoped project instructions]\nEach source applies only to its listed target paths. For any target, applicable sources are discovered from the outer directory inward; the nearest file takes precedence when instructions conflict. {scope_note}\n\n"
    );
    for entry in entries {
        let targets = entry
            .targets
            .iter()
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()?
            .join(", ");
        prompt.push_str(&format!(
            "Source: {}\nSHA-256: `{}`\nTargets: {}\n\n{}\n\n---\n\n",
            serde_json::to_string(&entry.source)?,
            entry.sha256,
            targets,
            entry.contents
        ));
    }
    if truncated {
        prompt.push_str(
            "Additional target-scoped instruction content was omitted or truncated at AHEAD's 32 KiB per-turn budget. Read the applicable AGENTS.md before acting if more detail is required.\n",
        );
    }
    Ok(TargetInstructionContext {
        prompt,
        sources,
        truncated,
    })
}

/// Finds the instruction boundary for a canonical workspace path. Watchers
/// share this discovery with the loader so nested workspaces use the same root.
pub fn project_instruction_root(workspace_root: &Path) -> PathBuf {
    // The retained chat loader discovers the nearest repository root by `.git`.
    // FIM has no runtime loader, so use the same root when a nested folder is open.
    let mut directory = Some(workspace_root);
    while let Some(path) = directory {
        let marker = path.join(".git");
        if std::fs::symlink_metadata(&marker).is_ok_and(|metadata| {
            let file_type = metadata.file_type();
            file_type.is_file() || file_type.is_dir()
        }) {
            return path.to_path_buf();
        }
        directory = path.parent();
    }
    workspace_root.to_path_buf()
}

fn add_target_path(
    workspace_root: &Path,
    workspace_path: &Path,
    value: &str,
    allow_relative: bool,
    targets: &mut Vec<(PathBuf, String)>,
) {
    if value.is_empty() {
        return;
    }
    let path = Path::new(value);
    let relative = if path.is_absolute() {
        path.strip_prefix(workspace_path)
            .or_else(|_| path.strip_prefix(workspace_root))
            .map(Path::to_path_buf)
            .ok()
    } else if allow_relative {
        Some(path.to_path_buf())
    } else {
        None
    };
    let Some(relative) = relative.filter(|relative| {
        relative
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
    }) else {
        return;
    };
    let Some(resolved) =
        ahead_core::search::resolve_open_buffer_path(workspace_root, &relative)
    else {
        return;
    };
    let Ok(relative) = resolved.strip_prefix(workspace_root) else {
        return;
    };
    let path = relative.to_path_buf();
    if targets.iter().any(|(known, _)| *known == path) {
        return;
    }
    targets.push((path.clone(), path_label(&path)));
}

fn path_label(path: &Path) -> String {
    path.to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

fn read_target_instruction(
    path: &Path,
    source: &str,
    remaining: usize,
) -> Result<Option<(String, String, bool)>> {
    let file = match codex_exec_server::open_regular_file_no_follow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to read project instructions `{source}`")
            });
        }
    };
    if file.metadata()?.len() > TARGET_INSTRUCTION_FILE_MAX_BYTES as u64 {
        bail!("project instructions `{source}` exceed AHEAD's 1 MiB file limit");
    }
    let mut bytes = Vec::new();
    file.take(TARGET_INSTRUCTION_FILE_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .with_context(|| {
            format!("failed to read project instructions `{source}`")
        })?;
    if bytes.len() > TARGET_INSTRUCTION_FILE_MAX_BYTES {
        bail!("project instructions `{source}` exceed AHEAD's 1 MiB file limit");
    }
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let mut contents = String::from_utf8_lossy(&bytes).into_owned();
    if contents.trim().is_empty() {
        return Ok(None);
    }
    let truncated = contents.len() > remaining;
    if truncated {
        let mut end = remaining;
        while !contents.is_char_boundary(end) {
            end -= 1;
        }
        contents.truncate(end);
    }
    Ok(Some((sha256, contents, truncated)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ahead_rpc::ahead::RepoPath;

    fn turn_context(
        active_path: &str,
        attached_paths: impl IntoIterator<Item = String>,
    ) -> TurnEditorContext {
        TurnEditorContext {
            active_path: active_path.to_string(),
            caret: ahead_rpc::ahead::DisplayPosition { line: 0, col: 0 },
            selection: None,
            file_content: String::new(),
            visible_end: None,
            attached_anchor_ids: Vec::new(),
            attached_files: attached_paths
                .into_iter()
                .map(|path| ahead_rpc::ahead::TurnContextFile {
                    path,
                    content: String::new(),
                })
                .collect(),
            attached_memories: Vec::new(),
        }
    }

    #[test]
    fn loads_nested_agents_instructions_for_only_structured_editor_targets() {
        let temp = tempfile::tempdir().expect("temp dir");
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(workspace.join("src"))
            .expect("create source directory");
        std::fs::create_dir_all(workspace.join("docs"))
            .expect("create docs directory");
        std::fs::create_dir_all(workspace.join("unrelated"))
            .expect("create unrelated directory");
        std::fs::create_dir_all(&outside).expect("create outside directory");
        std::fs::write(workspace.join("AGENTS.md"), "workspace-root instructions")
            .expect("write workspace instructions");
        std::fs::write(workspace.join("src/AGENTS.md"), "source instructions")
            .expect("write source instructions");
        std::fs::write(workspace.join("docs/AGENTS.md"), "docs instructions")
            .expect("write docs instructions");
        std::fs::write(
            workspace.join("unrelated/AGENTS.md"),
            "unrelated instructions",
        )
        .expect("write unrelated instructions");
        std::fs::write(outside.join("AGENTS.md"), "outside instructions")
            .expect("write outside instructions");

        let context = turn_context(
            "src/lib.rs",
            [
                workspace.join("docs/reference.md").display().to_string(),
                outside.join("reference.md").display().to_string(),
            ],
        );
        let instruction_context = target_instruction_context(&workspace, &context)
            .expect("load target instructions");
        let prompt = &instruction_context.prompt;

        assert!(prompt.contains("source instructions"));
        assert!(prompt.contains("docs instructions"));
        assert!(!prompt.contains("workspace-root instructions"));
        assert!(!prompt.contains("unrelated instructions"));
        assert!(!prompt.contains("outside instructions"));
        assert!(prompt.contains("Source: \"src/AGENTS.md\""));
        assert!(prompt.contains("Source: \"docs/AGENTS.md\""));
        assert!(!prompt.contains(&workspace.display().to_string()));
        assert!(prompt.contains("SHA-256:"));
        assert_eq!(instruction_context.sources.len(), 2);
        assert_eq!(instruction_context.sources[0].path, "src/AGENTS.md");
        assert_eq!(
            instruction_context.sources[0].content_sha256,
            format!("{:x}", Sha256::digest(b"source instructions"))
        );
        assert_eq!(
            instruction_context.sources[0].targets,
            vec![RepoPath::from("src/lib.rs")]
        );
        assert_eq!(instruction_context.sources[1].path, "docs/AGENTS.md");
        assert_eq!(
            instruction_context.sources[1].targets,
            vec![RepoPath::from("docs/reference.md")]
        );
        assert!(instruction_context.sources.iter().all(|source| {
            !source.path.contains(&workspace.display().to_string())
                && source
                    .targets
                    .iter()
                    .all(|target| !target.contains(&workspace.display().to_string()))
        }));
    }

    #[test]
    fn loads_workspace_root_through_each_fim_target_ancestor() {
        let temp = tempfile::tempdir().expect("temp dir");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(workspace.join("src/components"))
            .expect("create source directory");
        std::fs::create_dir_all(workspace.join("docs"))
            .expect("create docs directory");
        std::fs::write(workspace.join("AGENTS.md"), "root policy")
            .expect("write root instructions");
        std::fs::write(workspace.join("src/AGENTS.md"), "source policy")
            .expect("write source instructions");
        std::fs::write(
            workspace.join("src/components/AGENTS.md"),
            "component policy",
        )
        .expect("write component instructions");
        std::fs::write(workspace.join("docs/AGENTS.md"), "docs policy")
            .expect("write docs instructions");

        let prompt = project_instruction_prompt_for_targets(
            &workspace,
            &["src/components/button.rs", "docs/guide.md"],
        )
        .expect("load FIM target instructions");

        let root_index = prompt.find("root policy").expect("root policy");
        let source_index = prompt.find("source policy").expect("source policy");
        let component_index =
            prompt.find("component policy").expect("component policy");
        let docs_index = prompt.find("docs policy").expect("docs policy");
        assert!(root_index < source_index && source_index < component_index);
        assert!(root_index < docs_index);
        assert!(!prompt.contains(&workspace.display().to_string()));
        assert!(prompt.contains("Targets: \"src/components/button.rs\""));
        assert!(prompt.contains("Targets: \"docs/guide.md\""));
    }

    #[test]
    fn fim_loads_repository_instructions_above_a_nested_workspace() {
        let temp = tempfile::tempdir().expect("temp dir");
        let repository = temp.path().join("repository");
        let workspace = repository.join("packages/widget");
        std::fs::create_dir_all(workspace.join("src"))
            .expect("create nested workspace");
        std::fs::create_dir_all(repository.join(".git"))
            .expect("create repository marker");
        std::fs::write(repository.join("AGENTS.md"), "repository policy")
            .expect("write repository instructions");
        std::fs::write(repository.join("packages/AGENTS.md"), "packages policy")
            .expect("write parent-package instructions");
        std::fs::write(workspace.join("AGENTS.md"), "workspace policy")
            .expect("write workspace instructions");
        std::fs::write(workspace.join("src/AGENTS.md"), "source policy")
            .expect("write source instructions");

        let prompt =
            project_instruction_prompt_for_targets(&workspace, &["src/main.rs"])
                .expect("load repository instruction hierarchy");
        let repository_index =
            prompt.find("repository policy").expect("repository policy");
        let packages_index =
            prompt.find("packages policy").expect("packages policy");
        let workspace_index =
            prompt.find("workspace policy").expect("workspace policy");
        let source_index = prompt.find("source policy").expect("source policy");
        assert!(
            repository_index < packages_index
                && packages_index < workspace_index
                && workspace_index < source_index
        );
        assert!(prompt.contains("Source: \"AGENTS.md\""));
        assert!(prompt.contains("Source: \"packages/AGENTS.md\""));
        assert!(prompt.contains("Source: \"packages/widget/AGENTS.md\""));
        assert!(prompt.contains("Source: \"packages/widget/src/AGENTS.md\""));
        assert!(prompt.contains("Targets: \"src/main.rs\""));
        assert!(!prompt.contains(&repository.display().to_string()));
    }

    #[test]
    fn project_instruction_root_accepts_git_worktree_marker_files() {
        let temp = tempfile::tempdir().expect("temp dir");
        let repository = temp.path().join("repository");
        let workspace = repository.join("packages/widget");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        std::fs::write(repository.join(".git"), "gitdir: ../.git/worktrees/widget")
            .expect("write worktree marker");

        assert_eq!(project_instruction_root(&workspace), repository);
    }

    #[test]
    fn fim_rejects_instruction_budget_that_would_omit_nearer_rules() {
        let temp = tempfile::tempdir().expect("temp dir");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(workspace.join("src"))
            .expect("create source directory");
        std::fs::write(
            workspace.join("AGENTS.md"),
            "a".repeat(TARGET_INSTRUCTIONS_MAX_BYTES),
        )
        .expect("write root instructions");
        let path = "src/new.rs";
        assert!(project_instruction_prompt_for_targets(&workspace, &[path]).is_ok());

        std::fs::write(workspace.join("src/AGENTS.md"), "nearer rule")
            .expect("write nested instructions");
        let error = project_instruction_prompt_for_targets(&workspace, &[path])
            .expect_err("FIM must not silently omit nearer instructions");
        assert!(error.to_string().contains("prediction context limit"));
    }

    #[cfg(unix)]
    #[test]
    fn skips_symlinked_target_instruction_paths() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temp dir");
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(workspace.join("src"))
            .expect("create source directory");
        std::fs::create_dir_all(&outside).expect("create outside directory");
        std::fs::write(outside.join("AGENTS.md"), "outside instructions")
            .expect("write outside instructions");
        symlink(&outside, workspace.join("linked"))
            .expect("symlink outside target directory");
        symlink(outside.join("AGENTS.md"), workspace.join("src/AGENTS.md"))
            .expect("symlink source instructions");

        let context = turn_context("linked/main.rs", Vec::<String>::new());
        let instruction_context = target_instruction_context(&workspace, &context)
            .expect("load target instructions");
        assert!(instruction_context.prompt.is_empty());
        assert!(instruction_context.sources.is_empty());

        let context = turn_context("src/main.rs", Vec::<String>::new());
        let instruction_context = target_instruction_context(&workspace, &context)
            .expect("load target instructions");
        assert!(instruction_context.prompt.is_empty());
        assert!(instruction_context.sources.is_empty());

        let context = turn_context("../outside/main.rs", Vec::<String>::new());
        let instruction_context = target_instruction_context(&workspace, &context)
            .expect("load target instructions");
        assert!(instruction_context.prompt.is_empty());
        assert!(instruction_context.sources.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn target_instruction_read_rejects_replaced_parent_directory() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("disposable project");
        let temporary_root =
            temp.path().canonicalize().expect("canonical temp root");
        let workspace = temporary_root.join("workspace");
        let outside = temporary_root.join("outside");
        std::fs::create_dir_all(workspace.join("src"))
            .expect("create project directory");
        std::fs::create_dir_all(&outside).expect("create outside directory");
        std::fs::write(workspace.join("src/AGENTS.md"), "inside instructions")
            .expect("write project instructions");
        std::fs::write(outside.join("AGENTS.md"), "outside instructions")
            .expect("write outside instructions");

        let (_, contents, truncated) = read_target_instruction(
            &workspace.join("src/AGENTS.md"),
            "src/AGENTS.md",
            TARGET_INSTRUCTIONS_MAX_BYTES,
        )
        .expect("read regular project instructions")
        .expect("project instructions are non-empty");
        assert_eq!(contents, "inside instructions");
        assert!(!truncated);

        std::fs::remove_file(workspace.join("src/AGENTS.md"))
            .expect("remove prior instruction file");
        std::fs::remove_dir(workspace.join("src"))
            .expect("replace project directory");
        symlink(&outside, workspace.join("src")).expect("link to outside directory");

        assert!(
            read_target_instruction(
                &workspace.join("src/AGENTS.md"),
                "src/AGENTS.md",
                TARGET_INSTRUCTIONS_MAX_BYTES,
            )
            .is_err()
        );
    }

    #[cfg(windows)]
    #[test]
    fn target_instruction_read_rejects_reparse_parent_directory() {
        use std::os::windows::fs::symlink_dir;

        let temp = tempfile::tempdir().expect("disposable project");
        let temporary_root =
            temp.path().canonicalize().expect("canonical temp root");
        let workspace = temporary_root.join("workspace");
        let outside = temporary_root.join("outside");
        std::fs::create_dir_all(workspace.join("src"))
            .expect("create project directory");
        std::fs::create_dir_all(&outside).expect("create outside directory");
        std::fs::write(workspace.join("src/AGENTS.md"), "inside instructions")
            .expect("write project instructions");
        std::fs::write(outside.join("AGENTS.md"), "outside instructions")
            .expect("write outside instructions");

        let (_, contents, truncated) = read_target_instruction(
            &workspace.join("src/AGENTS.md"),
            "src/AGENTS.md",
            TARGET_INSTRUCTIONS_MAX_BYTES,
        )
        .expect("read regular project instructions")
        .expect("project instructions are non-empty");
        assert_eq!(contents, "inside instructions");
        assert!(!truncated);

        std::fs::remove_dir(workspace.join("src"))
            .expect("replace project directory");
        symlink_dir(&outside, workspace.join("src"))
            .expect("create reparse-point directory");

        assert!(
            read_target_instruction(
                &workspace.join("src/AGENTS.md"),
                "src/AGENTS.md",
                TARGET_INSTRUCTIONS_MAX_BYTES,
            )
            .is_err()
        );
    }
}
