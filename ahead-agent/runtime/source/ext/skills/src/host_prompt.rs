use codex_extension_api::ContextualUserFragment;
use codex_skills::SkillMetadata;

use crate::HostSkillsSnapshot;
use crate::fragments::SkillInstructions;
use crate::fragments::SkillResourceAccess;
use crate::fragments::neutralize_skill_envelope_tags;
use crate::host_snapshot::host_skill_resource_id;

/// Prompt fragments and read outcomes for a set of selected host skills.
pub struct HostSkillPrompts {
    pub fragments: Vec<Box<dyn ContextualUserFragment + Send>>,
    pub injected: Vec<SkillMetadata>,
    pub warnings: Vec<String>,
}

fn truncate_main_prompt_contents(contents: &str, wrapper_bytes: usize) -> (String, bool) {
    const MAX_SKILL_PROMPT_BODY_BYTES: usize = 8_200;

    let contents = neutralize_skill_envelope_tags(contents);
    let maximum_content_bytes = MAX_SKILL_PROMPT_BODY_BYTES.saturating_sub(wrapper_bytes);
    let mut boundary = contents.len().min(maximum_content_bytes);
    while !contents.is_char_boundary(boundary) {
        boundary -= 1;
    }
    (contents[..boundary].to_string(), boundary < contents.len())
}

impl HostSkillsSnapshot {
    /// Reads selected host skills and builds their model-visible prompt fragments.
    ///
    /// Core calls this through the host snapshot, independent of extension registration.
    #[tracing::instrument(
        level = "trace",
        skip_all,
        fields(selected_skill_count = selected_skills.len())
    )]
    pub async fn load_skill_prompts(&self, selected_skills: &[SkillMetadata]) -> HostSkillPrompts {
        let mut prompts = HostSkillPrompts {
            fragments: Vec::with_capacity(selected_skills.len()),
            injected: Vec::with_capacity(selected_skills.len()),
            warnings: Vec::new(),
        };

        for skill in selected_skills {
            match self.read_skill_text(skill).await {
                Ok(contents) => {
                    let resource_id = host_skill_resource_id(skill);
                    let mut fragment = SkillInstructions {
                        name: skill.name.clone(),
                        path: resource_id.clone(),
                        contents: String::new(),
                        resource_access: Some(SkillResourceAccess {
                            package: resource_id.clone(),
                            main_resource: resource_id,
                        }),
                    };
                    let (contents, truncated) =
                        truncate_main_prompt_contents(&contents, fragment.body().len());
                    if truncated {
                        prompts.warnings.push(format!(
                            "Skill `{}` exceeded the main prompt context limit and was truncated.",
                            skill.name
                        ));
                    }
                    fragment.contents = contents;
                    prompts.fragments.push(Box::new(fragment));
                    prompts.injected.push(skill.clone());
                }
                Err(_) => {
                    prompts
                        .warnings
                        .push(format!("Failed to load host skill {}.", skill.name));
                }
            }
        }

        prompts
    }
}
