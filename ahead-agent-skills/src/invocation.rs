use ahead_runtime_api::RuntimeData;
use codex_exec_server::LOCAL_ENVIRONMENT_ID;
use codex_protocol::protocol::SkillScope;
use codex_skills::detect_implicit_skill_invocation_for_command;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use std::path::PathBuf;

use crate::HostSkillsSnapshot;

#[derive(Clone, Debug)]
pub struct HostSkillInvocation {
    pub skill_name: String,
    pub path: PathBuf,
    pub scope: SkillScope,
}

/// Identifies the host skill referenced by a local command.
pub fn detect_implicit_skill_invocation(
    turn_store: &RuntimeData,
    environment_id: &str,
    command: &str,
    _workdir: &PathUri,
    native_workdir: Option<&AbsolutePathBuf>,
) -> Option<HostSkillInvocation> {
    if environment_id == LOCAL_ENVIRONMENT_ID
        && let Some(native_workdir) = native_workdir
        && let Some(host_snapshot) = turn_store.get::<HostSkillsSnapshot>()
        && let Some(skill) = detect_implicit_skill_invocation_for_command(
            host_snapshot.outcome(),
            command,
            native_workdir,
        )
    {
        return Some(HostSkillInvocation {
            skill_name: skill.name,
            path: skill.path_to_skills_md.to_path_buf(),
            scope: skill.scope,
        });
    }

    None
}
