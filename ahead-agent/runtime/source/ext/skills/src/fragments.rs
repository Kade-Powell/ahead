use codex_extension_api::ContextualUserFragment;
use codex_protocol::models::ContentItemKind;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SkillInstructions {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) contents: String,
    pub(crate) resource_access: Option<SkillResourceAccess>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SkillResourceAccess {
    pub(crate) package: String,
    pub(crate) main_resource: String,
}

fn escape_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

impl ContextualUserFragment for SkillInstructions {
    fn role(&self) -> &'static str {
        "user"
    }

    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("skills.selected_skill_instructions".to_string())
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<skill>", "</skill>")
    }

    fn body(&self) -> String {
        let name = escape_xml_text(&self.name);
        let path = escape_xml_text(&self.path);
        let contents = neutralize_skill_envelope_tags(&self.contents);
        let resource_access = self
            .resource_access
            .as_ref()
            .map(|access| {
                let metadata = serde_json::json!({
                    "package": access.package,
                    "main_resource": access.main_resource,
                });
                format!(
                    "\n<resource_access>{}</resource_access>",
                    escape_xml_text(&metadata.to_string())
                )
            })
            .unwrap_or_default();
        format!("\n<name>{name}</name>\n<path>{path}</path>{resource_access}\n{contents}\n")
    }
}

pub(crate) fn neutralize_skill_envelope_tags(contents: &str) -> String {
    contents
        .replace("</skill", "&lt;/skill")
        .replace("<skill", "&lt;skill")
}

#[cfg(test)]
#[path = "fragments_tests.rs"]
mod tests;
