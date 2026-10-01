mod discovery;
mod host;
mod host_merge;
pub(crate) use host::HostSkillRoot;
pub(crate) use host::HostSkillRootSnapshot;
pub(crate) use host::load_host_skill_root;
#[cfg(test)]
pub(crate) use host_merge::load_and_merge_host_skill_roots;
pub(crate) use host_merge::load_and_merge_host_skill_roots_with_request_snapshots;

pub(crate) const MAX_CONCURRENT_ROOT_SCANS: usize = 8;
pub(super) const SKILLS_FILENAME: &str = "SKILL.md";
pub(super) const MAX_SKILLS_DIRS_PER_ROOT: usize = 2000;
