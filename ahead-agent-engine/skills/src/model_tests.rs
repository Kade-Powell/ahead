use codex_utils_absolute_path::test_support::PathBufExt;
use codex_utils_absolute_path::test_support::test_path_buf;

use super::*;

#[test]
fn implicit_invocation_follows_standard_frontmatter_flag() {
    let skill = SkillMetadata {
        name: "demo".to_string(),
        description: "Demo skill".to_string(),
        short_description: None,
        disable_model_invocation: false,
        path_to_skills_md: test_path_buf("/tmp/skills/demo/SKILL.md").abs(),
        scope: codex_protocol::protocol::SkillScope::User,
    };

    assert!(skill.allows_implicit_invocation());
    assert!(
        !SkillMetadata {
            disable_model_invocation: true,
            ..skill
        }
        .allows_implicit_invocation()
    );
}
