use crate::config::Config;
use crate::session::turn_context::TurnContext;
use ahead_agent_skills::HostSkillsLoadInput;
use ahead_agent_skills::detect_implicit_skill_invocation;
use codex_otel::sanitize_metric_tag_value;
use codex_protocol::protocol::SkillScope;
use codex_skills::SkillMetadata;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use std::collections::HashSet;
use tokio::sync::Mutex;

#[derive(Debug, Default)]
struct ImplicitSkillInvocations(Mutex<HashSet<String>>);

pub(crate) fn skills_load_input_from_config(config: &Config) -> HostSkillsLoadInput {
    HostSkillsLoadInput::new(config.cwd.clone(), config.config_layer_stack.clone())
}

pub(crate) fn emit_explicit_skill_invocations(
    turn_context: &TurnContext,
    mentioned_skills: &[SkillMetadata],
    injected_skills: &[SkillMetadata],
) {
    let injected_skill_paths = injected_skills
        .iter()
        .map(|skill| &skill.path_to_skills_md)
        .collect::<HashSet<_>>();
    let model_slug_tag = sanitize_metric_tag_value(turn_context.model_info().slug.as_str());
    let reasoning_effort = turn_context.effective_reasoning_effort_for_tracing();
    for skill in mentioned_skills {
        let skill_name_tag = sanitize_metric_tag_value(skill.name.as_str());
        let status = if injected_skill_paths.contains(&skill.path_to_skills_md) {
            "ok"
        } else {
            "error"
        };
        turn_context.session_telemetry.counter(
            "ahead.skill.invoked",
            /*inc*/ 1,
            &[
                ("status", status),
                ("skill", skill_name_tag.as_str()),
                ("invoke_type", "explicit"),
                ("model_slug", model_slug_tag.as_str()),
                ("reasoning_effort", reasoning_effort.as_str()),
            ],
        );
    }
}

pub(crate) async fn maybe_emit_implicit_skill_invocation(
    turn_context: &TurnContext,
    command: &str,
    workdir: &PathUri,
    native_workdir: Option<&AbsolutePathBuf>,
    environment_id: &str,
) {
    let Some(invocation) = detect_implicit_skill_invocation(
        turn_context.runtime_data.as_ref(),
        environment_id,
        command,
        workdir,
        native_workdir,
    ) else {
        return;
    };
    let skill_name = invocation.skill_name.clone();
    let skill_scope = match &invocation.scope {
        SkillScope::User => "user",
        SkillScope::Repo => "repo",
        SkillScope::System => "system",
        SkillScope::Admin => "admin",
    };
    let skill_path = invocation.path.to_string_lossy();
    let seen_key = format!("{skill_scope}:{skill_path}:{skill_name}");
    let inserted = {
        let skill_invocations = turn_context
            .runtime_data
            .get_or_init(ImplicitSkillInvocations::default);
        let mut seen_skills = skill_invocations.0.lock().await;
        seen_skills.insert(seen_key)
    };
    if !inserted {
        return;
    }
    let skill_name_tag = sanitize_metric_tag_value(skill_name.as_str());
    let model_slug_tag = sanitize_metric_tag_value(turn_context.model_info().slug.as_str());
    let reasoning_effort = turn_context.effective_reasoning_effort_for_tracing();

    turn_context.session_telemetry.counter(
        "ahead.skill.invoked",
        /*inc*/ 1,
        &[
            ("status", "ok"),
            ("skill", skill_name_tag.as_str()),
            ("invoke_type", "implicit"),
            ("model_slug", model_slug_tag.as_str()),
            ("reasoning_effort", reasoning_effort.as_str()),
        ],
    );
}
