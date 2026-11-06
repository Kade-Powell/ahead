use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer};
use thiserror::Error;

const MAX_NAME_LEN: usize = 64;
const MAX_DESCRIPTION_LEN: usize = 1024;

#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    license: Option<FrontmatterString>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    compatibility: Option<FrontmatterString>,
    #[serde(
        default,
        rename = "allowed-tools",
        deserialize_with = "deserialize_optional_string"
    )]
    allowed_tools: Option<FrontmatterString>,
    #[serde(default, rename = "disable-model-invocation")]
    disable_model_invocation: bool,
    #[serde(default)]
    metadata: BTreeMap<FrontmatterString, FrontmatterString>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct FrontmatterString(String);

impl<'de> Deserialize<'de> for FrontmatterString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FrontmatterStringVisitor;

        impl Visitor<'_> for FrontmatterStringVisitor {
            type Value = FrontmatterString;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a string")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(FrontmatterString(value.to_owned()))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(FrontmatterString(value))
            }
        }

        deserializer.deserialize_any(FrontmatterStringVisitor)
    }
}

/// Validated metadata parsed from a `SKILL.md` frontmatter block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSkillFrontmatter {
    pub name: String,
    pub description: String,
    pub short_description: Option<String>,
    pub disable_model_invocation: bool,
}

/// Error produced while parsing or validating `SKILL.md` metadata.
#[derive(Debug, Error)]
pub enum SkillParseError {
    #[error("missing YAML frontmatter delimited by ---")]
    MissingFrontmatter,
    #[error("invalid YAML: {0}")]
    InvalidYaml(#[source] serde_yaml::Error),
    #[error("missing field `{0}`")]
    MissingField(&'static str),
    #[error("invalid {field}: {reason}")]
    InvalidField { field: &'static str, reason: String },
}

/// Parses and validates the metadata frontmatter from `SKILL.md` contents.
pub fn parse_skill_frontmatter_metadata(
    contents: &str,
    directory_name: &str,
) -> Result<ParsedSkillFrontmatter, SkillParseError> {
    let frontmatter = extract_frontmatter(contents).ok_or(SkillParseError::MissingFrontmatter)?;

    let parsed: SkillFrontmatter =
        serde_yaml::from_str(&frontmatter).map_err(SkillParseError::InvalidYaml)?;

    let name = parsed
        .name
        .as_deref()
        .ok_or(SkillParseError::MissingField("name"))?;
    validate_name(name, directory_name)?;
    let description = parsed
        .description
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or(SkillParseError::MissingField("description"))?;
    validate_len(description, MAX_DESCRIPTION_LEN, "description")?;
    if let Some(compatibility) = parsed.compatibility.as_ref() {
        if compatibility.0.trim().is_empty() {
            return Err(SkillParseError::InvalidField {
                field: "compatibility",
                reason: "must not be empty".to_string(),
            });
        }
        validate_len(&compatibility.0, 500, "compatibility")?;
    }
    let _license = parsed.license;
    let _allowed_tools = parsed.allowed_tools;
    let short_description = parsed
        .metadata
        .get(&FrontmatterString("short-description".to_string()))
        .map(|value| value.0.as_str())
        .map(sanitize_single_line)
        .filter(|value| !value.is_empty());

    Ok(ParsedSkillFrontmatter {
        name: name.to_string(),
        description: sanitize_single_line(description),
        short_description,
        disable_model_invocation: parsed.disable_model_invocation,
    })
}

fn deserialize_optional_string<'de, D>(
    deserializer: D,
) -> Result<Option<FrontmatterString>, D::Error>
where
    D: Deserializer<'de>,
{
    FrontmatterString::deserialize(deserializer).map(Some)
}

fn validate_name(name: &str, directory_name: &str) -> Result<(), SkillParseError> {
    validate_len(name, MAX_NAME_LEN, "name")?;
    if name != directory_name {
        return Err(SkillParseError::InvalidField {
            field: "name",
            reason: "must match the parent directory name".to_string(),
        });
    }
    if name.starts_with('-')
        || name.ends_with('-')
        || name.contains("--")
        || !name
            .chars()
            .all(|character| character == '-' || character.is_lowercase() || character.is_numeric())
    {
        return Err(SkillParseError::InvalidField {
            field: "name",
            reason: "must contain only lowercase letters, digits, and single hyphens".to_string(),
        });
    }
    Ok(())
}

fn sanitize_single_line(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn validate_len(
    value: &str,
    max_len: usize,
    field_name: &'static str,
) -> Result<(), SkillParseError> {
    if value.is_empty() {
        return Err(SkillParseError::MissingField(field_name));
    }
    if value.chars().count() > max_len {
        return Err(SkillParseError::InvalidField {
            field: field_name,
            reason: format!("exceeds maximum length of {max_len} characters"),
        });
    }
    Ok(())
}

fn extract_frontmatter(contents: &str) -> Option<String> {
    let mut lines = contents.lines();
    if !matches!(lines.next(), Some(line) if line.trim() == "---") {
        return None;
    }

    let mut frontmatter_lines = Vec::new();
    let mut found_closing = false;
    for line in lines.by_ref() {
        if line.trim() == "---" {
            found_closing = true;
            break;
        }
        frontmatter_lines.push(line);
    }

    if frontmatter_lines.is_empty() || !found_closing {
        return None;
    }

    Some(frontmatter_lines.join("\n"))
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod tests;
