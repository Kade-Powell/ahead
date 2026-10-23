use codex_context_fragments::ContextualUserFragment;
use codex_skills::SkillMetadata;
use sha2::Digest;
use sha2::Sha256;

use crate::HostSkillsSnapshot;
use crate::fragments::SkillInstructions;
use crate::fragments::SkillResourceAccess;
use crate::fragments::neutralize_skill_envelope_tags;
use crate::host_snapshot::host_skill_resource_id;

/// Prompt fragments and read outcomes for a set of selected host skills.
pub struct HostSkillPrompts {
    pub fragments: Vec<Box<dyn ContextualUserFragment + Send>>,
    pub injected: Vec<SkillMetadata>,
    pub provenance: Vec<HostSkillPromptProvenance>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostSkillPromptProvenance {
    pub skill_id: String,
    pub name: String,
    pub scope: String,
    pub source_sha256: String,
    pub prompt_sha256: String,
}

fn truncate_main_prompt_contents(
    contents: &str,
    wrapper_bytes: usize,
) -> (String, bool) {
    const MAX_SKILL_PROMPT_BODY_BYTES: usize = 8_200;

    let contents = neutralize_skill_envelope_tags(contents);
    let maximum_content_bytes =
        MAX_SKILL_PROMPT_BODY_BYTES.saturating_sub(wrapper_bytes);
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
    pub async fn load_skill_prompts(
        &self,
        selected_skills: &[SkillMetadata],
    ) -> HostSkillPrompts {
        let mut prompts = HostSkillPrompts {
            fragments: Vec::with_capacity(selected_skills.len()),
            injected: Vec::with_capacity(selected_skills.len()),
            provenance: Vec::with_capacity(selected_skills.len()),
            warnings: Vec::new(),
        };

        for skill in selected_skills {
            match self.read_skill_text(skill).await {
                Ok(contents) => {
                    let resource_id = host_skill_resource_id(skill);
                    let source_sha256 =
                        format!("{:x}", Sha256::digest(contents.as_bytes()));
                    let mut fragment = SkillInstructions {
                        name: skill.name.clone(),
                        path: resource_id.clone(),
                        contents: String::new(),
                        resource_access: Some(SkillResourceAccess {
                            package: resource_id.clone(),
                            main_resource: resource_id.clone(),
                        }),
                    };
                    let (contents, truncated) = truncate_main_prompt_contents(
                        &contents,
                        fragment.body().len(),
                    );
                    if truncated {
                        prompts.warnings.push(format!(
                            "Skill `{}` exceeded the main prompt context limit and was truncated.",
                            skill.name
                        ));
                    }
                    fragment.contents = contents;
                    let prompt_sha256 = format!(
                        "{:x}",
                        Sha256::digest(fragment.render().as_bytes())
                    );
                    prompts.fragments.push(Box::new(fragment));
                    prompts.injected.push(skill.clone());
                    prompts.provenance.push(HostSkillPromptProvenance {
                        skill_id: resource_id,
                        name: skill.name.clone(),
                        scope: match skill.scope {
                            codex_protocol::protocol::SkillScope::User => "user",
                            codex_protocol::protocol::SkillScope::Repo => "project",
                            codex_protocol::protocol::SkillScope::System => "system",
                            codex_protocol::protocol::SkillScope::Admin => "admin",
                        }
                        .to_string(),
                        source_sha256,
                        prompt_sha256,
                    });
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
