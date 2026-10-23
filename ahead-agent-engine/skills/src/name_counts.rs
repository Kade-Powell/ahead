use std::collections::HashMap;
use std::collections::HashSet;

use codex_utils_absolute_path::AbsolutePathBuf;

use crate::SkillMetadata;

/// Counts how often each exact skill name appears, excluding disabled paths.
pub fn build_skill_name_counts(
    skills: &[SkillMetadata],
    disabled_paths: &HashSet<AbsolutePathBuf>,
) -> HashMap<String, usize> {
    let mut exact_counts: HashMap<String, usize> = HashMap::new();
    for skill in skills {
        if disabled_paths.contains(&skill.path_to_skills_md) {
            continue;
        }
        *exact_counts.entry(skill.name.clone()).or_insert(0) += 1;
    }
    exact_counts
}
