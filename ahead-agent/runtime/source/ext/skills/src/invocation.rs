use codex_analytics::InvocationType;
use codex_analytics::SkillInvocation;
use codex_analytics::SkillInvocationLocation;
use codex_exec_server::LOCAL_ENVIRONMENT_ID;
use codex_extension_api::ExtensionData;
use codex_skills::detect_implicit_skill_invocation_for_command;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;

use crate::HostSkillsSnapshot;

/// Identifies the host skill referenced by a local command.
pub fn detect_implicit_skill_invocation(
    turn_store: &ExtensionData,
    environment_id: &str,
    command: &str,
    _workdir: &PathUri,
    native_workdir: Option<&AbsolutePathBuf>,
) -> Option<SkillInvocation> {
    if environment_id == LOCAL_ENVIRONMENT_ID
        && let Some(native_workdir) = native_workdir
        && let Some(host_snapshot) = turn_store.get::<HostSkillsSnapshot>()
        && let Some(skill) = detect_implicit_skill_invocation_for_command(
            host_snapshot.outcome(),
            command,
            native_workdir,
        )
    {
        return Some(SkillInvocation {
            skill_name: skill.name,
            location: SkillInvocationLocation::Host {
                path: skill.path_to_skills_md.to_path_buf(),
                scope: skill.scope,
            },
            invocation_type: InvocationType::Implicit,
        });
    }

    None
}
