use std::collections::{HashMap, HashSet};

use ahead_rpc::ahead::{MCP_FORM_ACTION_KEY, MCP_FORM_SKIP_VALUE};
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, NaiveDate};
use codex_protocol::approvals::ElicitationAction;
use regex::Regex;
use serde_json::{Map, Value};
use url::Url;

use crate::acp_client::{HarnessUserInputOption, HarnessUserInputQuestion};

#[derive(Clone)]
pub(super) struct McpForm {
    fields: Vec<Field>,
}

#[derive(Clone)]
struct Field {
    name: String,
    id: String,
    schema: Value,
    required: bool,
    options: Vec<String>,
    multi: bool,
}

impl McpForm {
    pub(super) fn new(
        server: &str,
        message: &str,
        schema: &Value,
    ) -> Result<(Self, Vec<HarnessUserInputQuestion>)> {
        ensure!(
            !server.is_empty()
                && server.len() <= 256
                && !server.chars().any(char::is_control)
                && !message.trim().is_empty()
                && message.len() <= 4096,
            "invalid MCP form text"
        );
        ensure!(
            schema.to_string().len() <= 32_768,
            "MCP form schema is too large"
        );
        let root = schema.as_object().context("MCP form must be an object")?;
        ensure!(
            root.get("type").and_then(Value::as_str) == Some("object"),
            "MCP form must be an object"
        );
        ensure!(
            root.keys().all(|key| matches!(
                key.as_str(),
                "type"
                    | "properties"
                    | "required"
                    | "title"
                    | "description"
                    | "additionalProperties"
            )),
            "unsupported MCP form constraint"
        );
        ensure!(
            root.get("additionalProperties")
                .is_none_or(|value| value == false),
            "MCP form may not request additional properties"
        );
        let properties = schema
            .get("properties")
            .and_then(Value::as_object)
            .context("MCP form has no properties")?;
        ensure!(properties.len() <= 16, "MCP form has too many fields");
        let required = match schema.get("required") {
            Some(value) => value
                .as_array()
                .context("invalid MCP required fields")?
                .clone(),
            None => Vec::new(),
        };
        let required = required
            .into_iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .context("invalid required field")
            })
            .collect::<Result<HashSet<_>>>()?;
        ensure!(
            required.iter().all(|field| properties.contains_key(field)),
            "MCP form names an unknown required field"
        );
        let mut fields = Vec::with_capacity(properties.len());
        let mut questions = Vec::with_capacity(properties.len());
        for (index, (name, property)) in properties.iter().enumerate() {
            ensure!(
                !name.is_empty() && name.len() <= 128,
                "invalid MCP field name"
            );
            let property = property
                .as_object()
                .context("MCP field must be an object")?;
            ensure!(
                !name.chars().any(char::is_control),
                "invalid MCP field name"
            );
            let kind = property
                .get("type")
                .and_then(Value::as_str)
                .context("MCP field has no type")?;
            ensure!(
                matches!(
                    kind,
                    "string" | "number" | "integer" | "boolean" | "array"
                ),
                "unsupported MCP field type"
            );
            validate_property_schema(property, kind)?;
            let multi = kind == "array";
            let options = if multi {
                let items = property
                    .get("items")
                    .and_then(Value::as_object)
                    .context("MCP array has no items")?;
                ensure!(
                    items.get("type").and_then(Value::as_str) == Some("string"),
                    "MCP array items must be strings"
                );
                string_options(items)?
            } else if kind == "string" {
                string_options(property)?
            } else if kind == "boolean" {
                ["Yes", "No"]
                    .into_iter()
                    .map(|value| HarnessUserInputOption {
                        value: value.to_string(),
                        label: value.to_string(),
                        description: value.to_string(),
                    })
                    .collect()
            } else {
                Vec::new()
            };
            if multi {
                ensure!(!options.is_empty(), "MCP array needs select options");
            }
            let title = property
                .get("title")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .unwrap_or(name);
            let description = property
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("");
            ensure!(
                title.len() <= 256 && description.len() <= 1024,
                "MCP field text is too long"
            );
            let is_required = required.contains(name);
            let id = if multi {
                format!("mcp_multi_{index}")
            } else {
                format!("mcp_field_{index}")
            };
            let field_options =
                options.iter().map(|option| option.value.clone()).collect();
            let mut display_options = options;
            if !is_required {
                display_options.push(HarnessUserInputOption {
                    value: MCP_FORM_SKIP_VALUE.to_string(),
                    label: "Skip".to_string(),
                    description: "Do not send this field to the MCP server"
                        .to_string(),
                });
            }
            let field = Field {
                name: name.clone(),
                id: id.clone(),
                schema: Value::Object(property.clone()),
                required: is_required,
                options: field_options,
                multi,
            };
            questions.push(HarnessUserInputQuestion {
                id: id.clone(),
                header: format!("MCP server: {server} · {title}{}", if is_required { " (required)" } else { " (optional)" }),
                question: if index == 0 {
                    format!("{message}\n{description}\nReview your answers before sending. Do not enter passwords, tokens, or payment details here.")
                } else {
                    description.to_string()
                },
                external_url: None,
                options: display_options,
                default_answers: valid_default_answers(&field),
                allows_other: field.options.is_empty(),
                is_secret: false,
            });
            fields.push(field);
        }
        ensure!(!fields.is_empty(), "empty MCP form");
        Ok((Self { fields }, questions))
    }

    pub(super) fn response(
        &self,
        answers: &HashMap<String, Vec<String>>,
    ) -> Result<(ElicitationAction, Option<Value>)> {
        ensure!(
            answers.len() <= self.fields.len() + 1,
            "too many MCP form answers"
        );
        if let Some(action) = answers.get(MCP_FORM_ACTION_KEY) {
            ensure!(answers.len() == 1, "MCP form action cannot include answers");
            return match action.as_slice() {
                [value] if value == "decline" => {
                    Ok((ElicitationAction::Decline, None))
                }
                [value] if value == "cancel" => {
                    Ok((ElicitationAction::Cancel, None))
                }
                _ => bail!("invalid MCP form action"),
            };
        }
        ensure!(
            answers
                .keys()
                .all(|id| self.fields.iter().any(|field| &field.id == id)),
            "unknown MCP form field"
        );
        let mut content = Map::new();
        for field in &self.fields {
            let selections = answers
                .get(&field.id)
                .context(format!("Answer {} or choose Skip", field.name))?;
            ensure!(
                !selections.is_empty() && selections.len() <= 32,
                "invalid number of answers for {}",
                field.name
            );
            ensure!(
                selections.iter().all(|answer| answer.len() <= 4096),
                "MCP answer is too long"
            );
            if selections.len() == 1
                && selections[0] == MCP_FORM_SKIP_VALUE
                && !field.required
            {
                continue;
            }
            ensure!(
                !selections
                    .iter()
                    .any(|answer| answer == MCP_FORM_SKIP_VALUE),
                "Skip cannot be combined with answers"
            );
            let kind = field.schema["type"]
                .as_str()
                .context("missing field type")?;
            let value = if field.multi {
                ensure!(
                    selections.iter().all(|value| field.options.contains(value)),
                    "invalid selection for {}",
                    field.name
                );
                let unique = selections.iter().collect::<HashSet<_>>();
                ensure!(
                    unique.len() == selections.len(),
                    "duplicate selection for {}",
                    field.name
                );
                let count = selections.len() as u64;
                if let Some(minimum) = field.schema["minItems"].as_u64() {
                    ensure!(
                        count >= minimum,
                        "{} needs more selections",
                        field.name
                    );
                }
                if let Some(maximum) = field.schema["maxItems"].as_u64() {
                    ensure!(
                        count <= maximum,
                        "{} has too many selections",
                        field.name
                    );
                }
                Value::Array(selections.iter().cloned().map(Value::String).collect())
            } else {
                ensure!(
                    selections.len() == 1,
                    "choose one answer for {}",
                    field.name
                );
                let answer = &selections[0];
                match kind {
                    "boolean" => match answer.as_str() {
                        "Yes" => Value::Bool(true),
                        "No" => Value::Bool(false),
                        _ => bail!("invalid boolean for {}", field.name),
                    },
                    "integer" => Value::Number(
                        answer
                            .parse::<i64>()
                            .with_context(|| {
                                format!("{} must be an integer", field.name)
                            })?
                            .into(),
                    ),
                    "number" => {
                        let number = answer.parse::<f64>().with_context(|| {
                            format!("{} must be a number", field.name)
                        })?;
                        ensure!(number.is_finite(), "{} must be finite", field.name);
                        Value::Number(
                            serde_json::Number::from_f64(number)
                                .context("invalid number")?,
                        )
                    }
                    "string" => {
                        ensure!(
                            field.options.is_empty()
                                || field.options.contains(answer),
                            "invalid selection for {}",
                            field.name
                        );
                        Value::String(answer.clone())
                    }
                    _ => bail!("unsupported field type"),
                }
            };
            validate_value(&field.name, &field.schema, &value)?;
            content.insert(field.name.clone(), value);
        }
        Ok((ElicitationAction::Accept, Some(Value::Object(content))))
    }
}

fn valid_default_answers(field: &Field) -> Vec<String> {
    let Some(default) = field.schema.get("default") else {
        return Vec::new();
    };
    let answers = match default {
        Value::String(value) => vec![value.clone()],
        Value::Number(value) => vec![value.to_string()],
        Value::Bool(value) => vec![if *value { "Yes" } else { "No" }.to_string()],
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    };
    if answers.is_empty() || answers.iter().any(|answer| answer.trim().is_empty()) {
        return Vec::new();
    }
    let form = McpForm {
        fields: vec![field.clone()],
    };
    let response =
        form.response(&HashMap::from([(field.id.clone(), answers.clone())]));
    match response {
        Ok((ElicitationAction::Accept, Some(Value::Object(content))))
            if content.contains_key(&field.name) =>
        {
            answers
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
fn action_answer(action: &str) -> HashMap<String, Vec<String>> {
    HashMap::from([(MCP_FORM_ACTION_KEY.to_string(), vec![action.to_string()])])
}

fn validate_property_schema(
    property: &Map<String, Value>,
    kind: &str,
) -> Result<()> {
    let allowed: &[&str] = match kind {
        "string" => &[
            "type",
            "title",
            "description",
            "default",
            "minLength",
            "maxLength",
            "pattern",
            "format",
            "enum",
            "oneOf",
        ],
        "number" | "integer" => &[
            "type",
            "title",
            "description",
            "default",
            "minimum",
            "maximum",
        ],
        "boolean" => &["type", "title", "description", "default"],
        "array" => &[
            "type",
            "title",
            "description",
            "default",
            "items",
            "minItems",
            "maxItems",
        ],
        _ => bail!("unsupported MCP field type"),
    };
    ensure!(
        property.keys().all(|key| allowed.contains(&key.as_str())),
        "unsupported MCP field constraint"
    );
    for key in ["title", "description"] {
        if let Some(value) = property.get(key) {
            ensure!(value.as_str().is_some(), "invalid MCP field label");
        }
    }
    if let Some(default) = property.get("default") {
        ensure!(
            match kind {
                "string" => default.is_string(),
                "number" => default.is_number(),
                "integer" => default.as_i64().is_some(),
                "boolean" => default.is_boolean(),
                "array" => default
                    .as_array()
                    .is_some_and(|values| values.iter().all(Value::is_string)),
                _ => false,
            },
            "invalid MCP field default"
        );
    }
    match kind {
        "string" => {
            for key in ["minLength", "maxLength"] {
                if let Some(value) = property.get(key) {
                    ensure!(
                        value.as_u64().is_some_and(|value| value <= 4096),
                        "invalid MCP string length"
                    );
                }
            }
            if let (Some(minimum), Some(maximum)) =
                (property.get("minLength"), property.get("maxLength"))
            {
                ensure!(
                    minimum.as_u64() <= maximum.as_u64(),
                    "invalid MCP string length range"
                );
            }
            if let Some(pattern) = property.get("pattern") {
                let pattern =
                    pattern.as_str().context("invalid MCP string pattern")?;
                ensure!(pattern.len() <= 1024, "MCP validation pattern is too long");
                Regex::new(pattern).context("invalid MCP string pattern")?;
            }
            if let Some(format) = property.get("format") {
                ensure!(
                    matches!(
                        format.as_str(),
                        Some("email" | "uri" | "date" | "date-time")
                    ),
                    "unsupported MCP string format"
                );
            }
            ensure!(
                !(property.contains_key("enum") && property.contains_key("oneOf")),
                "ambiguous MCP string options"
            );
        }
        "number" | "integer" => {
            for key in ["minimum", "maximum"] {
                if let Some(value) = property.get(key) {
                    ensure!(
                        if kind == "integer" {
                            value.as_i64().is_some()
                        } else {
                            value.as_f64().is_some_and(f64::is_finite)
                        },
                        "invalid MCP number bound"
                    );
                }
            }
            if let (Some(minimum), Some(maximum)) =
                (property.get("minimum"), property.get("maximum"))
            {
                ensure!(
                    minimum.as_f64() <= maximum.as_f64(),
                    "invalid MCP number range"
                );
            }
        }
        "array" => {
            for key in ["minItems", "maxItems"] {
                if let Some(value) = property.get(key) {
                    ensure!(
                        value.as_u64().is_some_and(|value| value <= 32),
                        "invalid MCP selection count"
                    );
                }
            }
            if let (Some(minimum), Some(maximum)) =
                (property.get("minItems"), property.get("maxItems"))
            {
                ensure!(
                    minimum.as_u64() <= maximum.as_u64(),
                    "invalid MCP selection range"
                );
            }
            let items = property
                .get("items")
                .and_then(Value::as_object)
                .context("MCP array has no items")?;
            ensure!(
                items.keys().all(|key| matches!(
                    key.as_str(),
                    "type" | "enum" | "anyOf" | "oneOf"
                )),
                "unsupported MCP array item constraint"
            );
        }
        _ => {}
    }
    Ok(())
}

fn string_options(
    schema: &Map<String, Value>,
) -> Result<Vec<HarnessUserInputOption>> {
    ensure!(
        ["enum", "oneOf", "anyOf"]
            .iter()
            .filter(|key| schema.contains_key(**key))
            .count()
            <= 1,
        "ambiguous MCP select options"
    );
    let options = if let Some(values) = schema.get("enum") {
        let options = values
            .as_array()
            .context("invalid MCP enum")?
            .iter()
            .map(|value| {
                let value =
                    value.as_str().context("MCP enum values must be strings")?;
                Ok(HarnessUserInputOption {
                    value: value.to_string(),
                    label: value.to_string(),
                    description: value.to_string(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        ensure!(!options.is_empty(), "empty MCP enum");
        options
    } else if let Some(values) = schema.get("oneOf").or_else(|| schema.get("anyOf"))
    {
        let options = values
            .as_array()
            .context("invalid MCP select options")?
            .iter()
            .map(|value| {
                let option =
                    value.as_object().context("invalid MCP select option")?;
                ensure!(
                    option.keys().all(|key| matches!(
                        key.as_str(),
                        "const" | "title" | "description"
                    )),
                    "unsupported MCP select option"
                );
                let value = option
                    .get("const")
                    .and_then(Value::as_str)
                    .context("MCP select values must be strings")?;
                let title = option
                    .get("title")
                    .map(|title| title.as_str().context("invalid MCP option title"))
                    .transpose()?
                    .unwrap_or(value);
                let description = option
                    .get("description")
                    .map(|description| {
                        description
                            .as_str()
                            .context("invalid MCP option description")
                    })
                    .transpose()?
                    .unwrap_or(title);
                ensure!(
                    !title.is_empty()
                        && title.len() <= 256
                        && description.len() <= 1024,
                    "MCP option text is too long"
                );
                Ok(HarnessUserInputOption {
                    value: value.to_string(),
                    label: title.to_string(),
                    description: description.to_string(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        ensure!(!options.is_empty(), "empty MCP select options");
        options
    } else {
        Vec::new()
    };
    ensure!(
        options.len() <= 32
            && options.iter().all(|option| !option.value.is_empty()
                && option.value.len() <= 256
                && option.value != MCP_FORM_SKIP_VALUE),
        "invalid MCP select options"
    );
    ensure!(
        options
            .iter()
            .map(|option| &option.value)
            .collect::<HashSet<_>>()
            .len()
            == options.len(),
        "duplicate MCP select option"
    );
    Ok(options)
}

fn validate_value(name: &str, schema: &Value, value: &Value) -> Result<()> {
    if let Some(text) = value.as_str() {
        let length = text.chars().count() as u64;
        if let Some(minimum) = schema["minLength"].as_u64() {
            ensure!(length >= minimum, "{} is too short", name);
        }
        if let Some(maximum) = schema["maxLength"].as_u64() {
            ensure!(length <= maximum, "{} is too long", name);
        }
        if let Some(pattern) = schema["pattern"].as_str() {
            ensure!(pattern.len() <= 1024, "MCP validation pattern is too long");
            ensure!(
                Regex::new(pattern)
                    .context("invalid MCP validation pattern")?
                    .is_match(text),
                "{} does not match the requested pattern",
                name
            );
        }
        match schema["format"].as_str() {
            None => {}
            Some("email") => ensure!(
                text.split_once('@').is_some_and(|(local, domain)| !local
                    .is_empty()
                    && domain.contains('.')
                    && !domain.contains('@')
                    && !domain.starts_with('.')
                    && !domain.ends_with('.')
                    && !text.contains(char::is_whitespace)),
                "{} must be an email address",
                name
            ),
            Some("uri") => {
                Url::parse(text)
                    .with_context(|| format!("{} must be a URI", name))?;
            }
            Some("date") => {
                NaiveDate::parse_from_str(text, "%Y-%m-%d")
                    .with_context(|| format!("{} must be a date", name))?;
            }
            Some("date-time") => {
                DateTime::parse_from_rfc3339(text)
                    .with_context(|| format!("{} must be a date-time", name))?;
            }
            Some(_) => bail!("unsupported MCP string format"),
        }
    } else if let Some(number) = value.as_f64() {
        if let Some(minimum) = schema["minimum"].as_f64() {
            ensure!(number >= minimum, "{} is too small", name);
        }
        if let Some(maximum) = schema["maximum"].as_f64() {
            ensure!(number <= maximum, "{} is too large", name);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn form_submission_is_typed_and_rejects_bad_input() {
        let schema = json!({"type":"object","properties":{"name":{"type":"string","minLength":2},"count":{"type":"integer","minimum":1},"enabled":{"type":"boolean"}},"required":["name","count"]});
        let (form, questions) =
            McpForm::new("sample", "Provide settings", &schema).expect("form");
        assert_eq!(questions.len(), 3);
        let field_id = |name: &str| {
            form.fields
                .iter()
                .find(|field| field.name == name)
                .expect("form field")
                .id
                .clone()
        };
        let mut answers = HashMap::from([
            (field_id("count"), vec!["2".to_string()]),
            (field_id("enabled"), vec![MCP_FORM_SKIP_VALUE.to_string()]),
            (field_id("name"), vec!["Al".to_string()]),
        ]);
        assert_eq!(
            form.response(&answers).expect("response"),
            (
                ElicitationAction::Accept,
                Some(json!({"count":2,"name":"Al"}))
            )
        );
        answers.insert(field_id("count"), vec!["0".to_string()]);
        assert!(form.response(&answers).is_err());
        assert_eq!(
            form.response(&action_answer("cancel")).expect("cancel"),
            (ElicitationAction::Cancel, None)
        );
    }

    #[test]
    fn multi_select_and_schema_constraints_fail_closed() {
        let schema = json!({"type":"object","properties":{"colors":{"type":"array","items":{"type":"string","enum":["red","blue"]},"minItems":1,"maxItems":2}},"required":["colors"]});
        let (form, questions) =
            McpForm::new("palette", "Choose colors", &schema).expect("form");
        assert_eq!(questions[0].id, "mcp_multi_0");
        let answers = HashMap::from([(
            "mcp_multi_0".to_string(),
            vec!["red".to_string(), "blue".to_string()],
        )]);
        assert_eq!(
            form.response(&answers).expect("selection"),
            (
                ElicitationAction::Accept,
                Some(json!({"colors":["red","blue"]}))
            )
        );
        let bad =
            HashMap::from([("mcp_multi_0".to_string(), vec!["green".to_string()])]);
        assert!(form.response(&bad).is_err());
        let nested = json!({"type":"object","properties":{"value":{"type":"string","allOf":[]}}});
        assert!(McpForm::new("sample", "Input", &nested).is_err());
        let malformed = json!({"type":"object","properties":{"value":{"type":"string"}},"required":"value"});
        assert!(McpForm::new("sample", "Input", &malformed).is_err());
    }

    #[test]
    fn form_defaults_are_preselected_only_when_they_can_be_submitted() {
        let schema = json!({
            "type":"object",
            "properties":{
                "name":{"type":"string","minLength":2,"default":"Maya"},
                "count":{"type":"integer","minimum":1,"default":2},
                "ratio":{"type":"number","default":1.5},
                "enabled":{"type":"boolean","default":false},
                "colors":{"type":"array","items":{"type":"string","enum":["red","blue"]},"minItems":1,"default":["red"]},
                "invalid":{"type":"string","minLength":2,"default":"x"},
                "invalid_select":{"type":"string","enum":["red"],"default":"blue"},
                "skip_literal":{"type":"string","default":"Skip"}
            },
            "required":["name","count","colors"]
        });
        let (form, questions) =
            McpForm::new("sample", "Input", &schema).expect("form");
        let defaults = form
            .fields
            .iter()
            .map(|field| {
                let question = questions
                    .iter()
                    .find(|question| question.id == field.id)
                    .expect("matching question");
                (field.name.as_str(), question.default_answers.clone())
            })
            .collect::<HashMap<_, _>>();
        assert_eq!(defaults["name"], vec!["Maya"]);
        assert_eq!(defaults["count"], vec!["2"]);
        assert_eq!(defaults["ratio"], vec!["1.5"]);
        assert_eq!(defaults["enabled"], vec!["No"]);
        assert_eq!(defaults["colors"], vec!["red"]);
        assert!(defaults["invalid"].is_empty());
        assert!(defaults["invalid_select"].is_empty());
        assert_eq!(defaults["skip_literal"], vec!["Skip"]);
    }

    #[test]
    fn titled_options_keep_wire_values_distinct_from_skip() {
        let schema = json!({
            "type": "object",
            "properties": {
                "choice": {"type": "string", "oneOf": [
                    {"const": "prod", "title": "Production", "description": "Use live resources"},
                    {"const": "Skip", "title": "Literal Skip"}
                ]},
                "note": {"type": "string"}
            },
            "required": ["choice"]
        });
        let (form, questions) =
            McpForm::new("sample", "Choose", &schema).expect("form");
        let choice = form
            .fields
            .iter()
            .find(|field| field.name == "choice")
            .expect("choice");
        let note = form
            .fields
            .iter()
            .find(|field| field.name == "note")
            .expect("note");
        let choice_question = questions
            .iter()
            .find(|question| question.id == choice.id)
            .expect("choice question");
        assert_eq!(choice_question.options[0].value, "prod");
        assert_eq!(choice_question.options[0].label, "Production");
        assert_eq!(choice_question.options[0].description, "Use live resources");
        let note_question = questions
            .iter()
            .find(|question| question.id == note.id)
            .expect("note question");
        assert_eq!(note_question.options[0].label, "Skip");
        assert_eq!(note_question.options[0].value, MCP_FORM_SKIP_VALUE);

        let mut answers = HashMap::from([
            (choice.id.clone(), vec!["prod".to_string()]),
            (note.id.clone(), vec!["Skip".to_string()]),
        ]);
        assert_eq!(
            form.response(&answers).expect("literal answer"),
            (
                ElicitationAction::Accept,
                Some(json!({"choice":"prod","note":"Skip"}))
            )
        );
        answers.insert(note.id.clone(), vec![MCP_FORM_SKIP_VALUE.to_string()]);
        assert_eq!(
            form.response(&answers).expect("omitted answer"),
            (ElicitationAction::Accept, Some(json!({"choice":"prod"})))
        );
    }
}
