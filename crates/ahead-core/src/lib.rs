use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const RUN_API_VERSION: &str = "ahead.run/v0";
pub const ENGINE_API_VERSION: &str = "ahead.engine/v0";
const PRODUCT_CHANGE_SPEC: &str = include_str!("../../../spec/workflows/product-change-v0.1.json");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowDefinition {
    pub id: String,
    pub version: String,
    pub title: String,
    pub initial_phase: String,
    pub phases: Vec<PhaseDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PhaseDefinition {
    pub id: String,
    pub title: String,
    pub owner: ActorKind,
    pub artifacts: Vec<ArtifactDefinition>,
    pub gate: GateDefinition,
    pub next: Option<String>,
    pub returns_to: Vec<String>,
    #[serde(default)]
    pub ai_unlock_artifacts: Vec<String>,
    #[serde(default)]
    pub ai_capabilities: Vec<Capability>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactDefinition {
    pub kind: String,
    pub title: String,
    pub actor: ActorRule,
    pub required: bool,
    #[serde(default)]
    pub independent_of: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GateDefinition {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub accepted_by_artifact: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    Human,
    Ai,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActorRule {
    Human,
    Ai,
    Any,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    Inspect,
    Search,
    Analyze,
    Modify,
    Execute,
    Record,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Actor {
    pub kind: ActorKind,
    pub identity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Run {
    pub api_version: String,
    pub id: String,
    pub title: String,
    pub workflow_id: String,
    pub workflow_version: String,
    pub owner: String,
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Event {
    pub sequence: u64,
    pub timestamp: String,
    pub actor: Actor,
    #[serde(flatten)]
    pub action: Action,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    RunStarted {
        phase: String,
    },
    ArtifactRecorded {
        phase: String,
        kind: String,
        path: String,
    },
    GateAccepted {
        phase: String,
        gate: String,
    },
    PhaseTransitioned {
        from: String,
        to: String,
        direction: TransitionDirection,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    RunClosed {
        phase: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransitionDirection {
    Advance,
    Return,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunState {
    pub run_id: String,
    pub title: String,
    pub workflow_id: String,
    pub workflow_version: String,
    pub phase: PhaseState,
    pub artifacts: Vec<ArtifactState>,
    pub gate: GateState,
    pub blockers: Vec<String>,
    pub allowed_ai_capabilities: Vec<Capability>,
    pub return_targets: Vec<String>,
    pub can_advance: bool,
    pub closed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PhaseState {
    pub id: String,
    pub title: String,
    pub visit: u32,
    pub next: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactState {
    pub kind: String,
    pub title: String,
    pub actor: ActorRule,
    pub required: bool,
    pub present: bool,
    pub path: Option<String>,
    pub recorded_by: Option<Actor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GateState {
    pub id: String,
    pub title: String,
    pub accepted: bool,
    pub accepted_by: Option<Actor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolDecision {
    pub allowed: bool,
    pub reason: String,
    pub capability: Capability,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApplyInput {
    pub actor: Actor,
    pub timestamp: String,
    pub action: Action,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateRunInput {
    pub id: String,
    pub title: String,
    pub owner: Actor,
    pub timestamp: String,
    #[serde(default = "default_workflow_id")]
    pub workflow_id: String,
}

fn default_workflow_id() -> String {
    "product-change".to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AheadError {
    pub code: String,
    pub message: String,
}

impl AheadError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, AheadError>;

pub fn workflow(workflow_id: &str) -> Result<WorkflowDefinition> {
    if workflow_id != "product-change" {
        return Err(AheadError::new(
            "unknown_workflow",
            format!("unknown workflow: {workflow_id}"),
        ));
    }

    serde_json::from_str(PRODUCT_CHANGE_SPEC).map_err(|error| {
        AheadError::new(
            "invalid_embedded_workflow",
            format!("embedded product-change workflow is invalid: {error}"),
        )
    })
}

pub fn create_run(input: CreateRunInput) -> Result<Run> {
    validate_actor(&input.owner)?;
    if input.owner.kind != ActorKind::Human {
        return Err(AheadError::new(
            "human_owner_required",
            "a human must own and start an AHEAD run",
        ));
    }
    validate_nonempty("run id", &input.id)?;
    validate_nonempty("run title", &input.title)?;
    validate_nonempty("timestamp", &input.timestamp)?;

    let definition = workflow(&input.workflow_id)?;
    Ok(Run {
        api_version: RUN_API_VERSION.to_owned(),
        id: input.id,
        title: input.title,
        workflow_id: definition.id.clone(),
        workflow_version: definition.version,
        owner: input.owner.identity.clone(),
        events: vec![Event {
            sequence: 1,
            timestamp: input.timestamp,
            actor: input.owner,
            action: Action::RunStarted {
                phase: definition.initial_phase,
            },
        }],
    })
}

pub fn apply(run: &Run, input: ApplyInput) -> Result<Run> {
    validate_run(run)?;
    validate_actor(&input.actor)?;
    validate_nonempty("timestamp", &input.timestamp)?;

    let mut candidate = run.clone();
    candidate.events.push(Event {
        sequence: candidate.events.len() as u64 + 1,
        timestamp: input.timestamp,
        actor: input.actor,
        action: input.action,
    });
    validate_run(&candidate)?;
    Ok(candidate)
}

pub fn validate_run(run: &Run) -> Result<RunState> {
    if run.api_version != RUN_API_VERSION {
        return Err(AheadError::new(
            "unsupported_run_version",
            format!("expected {RUN_API_VERSION}, got {}", run.api_version),
        ));
    }
    validate_nonempty("run id", &run.id)?;
    validate_nonempty("run title", &run.title)?;
    validate_nonempty("owner", &run.owner)?;
    let definition = workflow(&run.workflow_id)?;
    if run.workflow_version != definition.version {
        return Err(AheadError::new(
            "workflow_version_mismatch",
            format!(
                "run uses workflow version {}, engine provides {}",
                run.workflow_version, definition.version
            ),
        ));
    }
    if run.events.is_empty() {
        return Err(AheadError::new("missing_start", "run has no start event"));
    }

    let mut replay = ReplayState::new(&definition);
    for (index, event) in run.events.iter().enumerate() {
        let expected_sequence = index as u64 + 1;
        if event.sequence != expected_sequence {
            return Err(AheadError::new(
                "invalid_sequence",
                format!(
                    "event sequence {} should be {expected_sequence}",
                    event.sequence
                ),
            ));
        }
        validate_nonempty("event timestamp", &event.timestamp)?;
        validate_actor(&event.actor)?;
        replay_event(&definition, run, &mut replay, event, index == 0)?;
    }

    materialize_state(run, &definition, &replay)
}

pub fn derive_state(run: &Run) -> Result<RunState> {
    validate_run(run)
}

pub fn tool_allowed(run: &Run, capability: Capability) -> Result<ToolDecision> {
    let state = derive_state(run)?;
    if state.closed {
        return Ok(ToolDecision {
            allowed: false,
            reason: "the AHEAD run is closed".to_owned(),
            capability,
        });
    }
    let allowed = state.allowed_ai_capabilities.contains(&capability);
    let reason = if allowed {
        format!(
            "capability is allowed during phase {} visit {}",
            state.phase.id, state.phase.visit
        )
    } else if !state.blockers.is_empty() {
        format!(
            "capability is not available during phase {}; blockers: {}",
            state.phase.id,
            state.blockers.join("; ")
        )
    } else {
        format!("capability is not allowed during phase {}", state.phase.id)
    };
    Ok(ToolDecision {
        allowed,
        reason,
        capability,
    })
}

#[derive(Debug, Clone)]
struct RecordedArtifact {
    path: String,
    actor: Actor,
}

#[derive(Debug, Clone)]
struct ReplayState {
    phase: String,
    visit: u32,
    visits: BTreeMap<String, u32>,
    artifacts: BTreeMap<String, RecordedArtifact>,
    gate_actor: Option<Actor>,
    closed: bool,
}

impl ReplayState {
    fn new(definition: &WorkflowDefinition) -> Self {
        Self {
            phase: definition.initial_phase.clone(),
            visit: 0,
            visits: BTreeMap::new(),
            artifacts: BTreeMap::new(),
            gate_actor: None,
            closed: false,
        }
    }

    fn activate(&mut self, phase: &str) {
        let visit = self.visits.entry(phase.to_owned()).or_insert(0);
        *visit += 1;
        self.phase = phase.to_owned();
        self.visit = *visit;
        self.artifacts.clear();
        self.gate_actor = None;
    }
}

fn replay_event(
    definition: &WorkflowDefinition,
    run: &Run,
    replay: &mut ReplayState,
    event: &Event,
    first: bool,
) -> Result<()> {
    if replay.closed {
        return Err(AheadError::new(
            "run_already_closed",
            "events cannot be appended after closure",
        ));
    }

    match &event.action {
        Action::RunStarted { phase } => {
            if !first {
                return Err(AheadError::new(
                    "duplicate_start",
                    "run_started must be the first and only start event",
                ));
            }
            require_human(event, "start an AHEAD run")?;
            if event.actor.identity != run.owner {
                return Err(AheadError::new(
                    "owner_mismatch",
                    "run owner must match the human who started it",
                ));
            }
            if phase != &definition.initial_phase {
                return Err(AheadError::new(
                    "invalid_initial_phase",
                    format!("workflow must start in {}", definition.initial_phase),
                ));
            }
            replay.activate(phase);
        }
        Action::ArtifactRecorded { phase, kind, path } => {
            ensure_not_first(first)?;
            ensure_current_phase(replay, phase)?;
            if replay.gate_actor.is_some() {
                return Err(AheadError::new(
                    "gate_already_accepted",
                    "return to or advance from the phase before recording different evidence",
                ));
            }
            validate_artifact_path(path)?;
            let phase_definition = find_phase(definition, phase)?;
            let artifact = phase_definition
                .artifacts
                .iter()
                .find(|candidate| candidate.kind == *kind)
                .ok_or_else(|| {
                    AheadError::new(
                        "artifact_not_allowed",
                        format!("artifact {kind} is not defined for phase {phase}"),
                    )
                })?;
            validate_artifact_actor(artifact, &event.actor)?;
            if event.actor.kind == ActorKind::Ai {
                let locked = phase_definition
                    .ai_unlock_artifacts
                    .iter()
                    .filter(|required| !replay.artifacts.contains_key(*required))
                    .cloned()
                    .collect::<Vec<_>>();
                if !locked.is_empty() {
                    return Err(AheadError::new(
                        "ai_assistance_locked",
                        format!(
                            "AI assistance is locked until the human records: {}",
                            locked.join(", ")
                        ),
                    ));
                }
                if !phase_definition
                    .ai_capabilities
                    .contains(&Capability::Record)
                {
                    return Err(AheadError::new(
                        "ai_record_not_allowed",
                        format!("AI may not record artifacts during phase {phase}"),
                    ));
                }
            }
            if let Some(independent_of) = &artifact.independent_of {
                if let Some(prior) = latest_artifact_across_run(run, event.sequence, independent_of)
                {
                    if prior.actor.identity == event.actor.identity {
                        return Err(AheadError::new(
                            "independent_reviewer_required",
                            format!(
                                "{} must be recorded by someone other than {}",
                                artifact.title, prior.actor.identity
                            ),
                        ));
                    }
                } else {
                    return Err(AheadError::new(
                        "independence_source_missing",
                        format!("cannot establish independence without {independent_of}"),
                    ));
                }
            }
            replay.artifacts.insert(
                kind.clone(),
                RecordedArtifact {
                    path: path.clone(),
                    actor: event.actor.clone(),
                },
            );
        }
        Action::GateAccepted { phase, gate } => {
            ensure_not_first(first)?;
            ensure_current_phase(replay, phase)?;
            require_human(event, "accept an AHEAD gate")?;
            let phase_definition = find_phase(definition, phase)?;
            if gate != &phase_definition.gate.id {
                return Err(AheadError::new(
                    "wrong_gate",
                    format!("expected gate {}, got {gate}", phase_definition.gate.id),
                ));
            }
            if replay.gate_actor.is_some() {
                return Err(AheadError::new(
                    "gate_already_accepted",
                    "the current phase gate has already been accepted",
                ));
            }
            let missing = missing_required(phase_definition, &replay.artifacts);
            if !missing.is_empty() {
                return Err(AheadError::new(
                    "required_artifacts_missing",
                    format!("missing required artifacts: {}", missing.join(", ")),
                ));
            }
            if let Some(kind) = &phase_definition.gate.accepted_by_artifact {
                let artifact = replay.artifacts.get(kind).ok_or_else(|| {
                    AheadError::new(
                        "gate_actor_source_missing",
                        format!("gate acceptance identity requires artifact {kind}"),
                    )
                })?;
                if artifact.actor.identity != event.actor.identity {
                    return Err(AheadError::new(
                        "gate_actor_mismatch",
                        format!(
                            "gate {} must be accepted by {}, who recorded {}",
                            phase_definition.gate.id, artifact.actor.identity, kind
                        ),
                    ));
                }
            }
            replay.gate_actor = Some(event.actor.clone());
        }
        Action::PhaseTransitioned {
            from,
            to,
            direction,
            reason,
        } => {
            ensure_not_first(first)?;
            ensure_current_phase(replay, from)?;
            require_human(event, "transition an AHEAD phase")?;
            let phase_definition = find_phase(definition, from)?;
            find_phase(definition, to)?;
            match direction {
                TransitionDirection::Advance => {
                    if reason.is_some() {
                        return Err(AheadError::new(
                            "advance_reason_not_allowed",
                            "advance transitions do not use a return reason",
                        ));
                    }
                    if replay.gate_actor.is_none() {
                        return Err(AheadError::new(
                            "gate_not_accepted",
                            format!("gate {} has not been accepted", phase_definition.gate.id),
                        ));
                    }
                    if phase_definition.next.as_deref() != Some(to.as_str()) {
                        return Err(AheadError::new(
                            "invalid_advance",
                            format!("{to} is not the next phase after {from}"),
                        ));
                    }
                }
                TransitionDirection::Return => {
                    if !phase_definition.returns_to.contains(to) {
                        return Err(AheadError::new(
                            "invalid_return",
                            format!("phase {from} cannot return to {to}"),
                        ));
                    }
                    let reason = reason.as_deref().unwrap_or_default().trim();
                    if reason.is_empty() {
                        return Err(AheadError::new(
                            "return_reason_required",
                            "a return transition requires a reason",
                        ));
                    }
                }
            }
            replay.activate(to);
        }
        Action::RunClosed { phase } => {
            ensure_not_first(first)?;
            ensure_current_phase(replay, phase)?;
            require_human(event, "close an AHEAD run")?;
            let phase_definition = find_phase(definition, phase)?;
            if phase_definition.next.is_some() {
                return Err(AheadError::new(
                    "not_final_phase",
                    format!("phase {phase} is not the final phase"),
                ));
            }
            if replay.gate_actor.is_none() {
                return Err(AheadError::new(
                    "gate_not_accepted",
                    format!("gate {} has not been accepted", phase_definition.gate.id),
                ));
            }
            replay.closed = true;
        }
    }
    Ok(())
}

fn materialize_state(
    run: &Run,
    definition: &WorkflowDefinition,
    replay: &ReplayState,
) -> Result<RunState> {
    let phase = find_phase(definition, &replay.phase)?;
    let artifacts = phase
        .artifacts
        .iter()
        .map(|artifact| {
            let recorded = replay.artifacts.get(&artifact.kind);
            ArtifactState {
                kind: artifact.kind.clone(),
                title: artifact.title.clone(),
                actor: artifact.actor.clone(),
                required: artifact.required,
                present: recorded.is_some(),
                path: recorded.map(|item| item.path.clone()),
                recorded_by: recorded.map(|item| item.actor.clone()),
            }
        })
        .collect::<Vec<_>>();

    let missing = missing_required(phase, &replay.artifacts);
    let locked = phase
        .ai_unlock_artifacts
        .iter()
        .filter(|kind| !replay.artifacts.contains_key(*kind))
        .cloned()
        .collect::<Vec<_>>();
    let mut blockers = missing
        .iter()
        .map(|kind| format!("required artifact missing: {kind}"))
        .collect::<Vec<_>>();
    if replay.gate_actor.is_none() && missing.is_empty() && !replay.closed {
        blockers.push(format!("human gate not accepted: {}", phase.gate.id));
    }
    if !locked.is_empty() {
        blockers.push(format!(
            "AI assistance locked until human records: {}",
            locked.join(", ")
        ));
    }

    let allowed_ai_capabilities = if replay.closed || !locked.is_empty() {
        Vec::new()
    } else {
        let mut unique = BTreeSet::new();
        unique.extend(phase.ai_capabilities.iter().cloned());
        unique.into_iter().collect()
    };

    Ok(RunState {
        run_id: run.id.clone(),
        title: run.title.clone(),
        workflow_id: run.workflow_id.clone(),
        workflow_version: run.workflow_version.clone(),
        phase: PhaseState {
            id: phase.id.clone(),
            title: phase.title.clone(),
            visit: replay.visit,
            next: phase.next.clone(),
        },
        artifacts,
        gate: GateState {
            id: phase.gate.id.clone(),
            title: phase.gate.title.clone(),
            accepted: replay.gate_actor.is_some(),
            accepted_by: replay.gate_actor.clone(),
        },
        blockers,
        allowed_ai_capabilities,
        return_targets: phase.returns_to.clone(),
        can_advance: replay.gate_actor.is_some() && !replay.closed,
        closed: replay.closed,
    })
}

fn find_phase<'a>(definition: &'a WorkflowDefinition, phase: &str) -> Result<&'a PhaseDefinition> {
    definition
        .phases
        .iter()
        .find(|candidate| candidate.id == phase)
        .ok_or_else(|| AheadError::new("unknown_phase", format!("unknown phase: {phase}")))
}

fn missing_required(
    phase: &PhaseDefinition,
    artifacts: &BTreeMap<String, RecordedArtifact>,
) -> Vec<String> {
    phase
        .artifacts
        .iter()
        .filter(|artifact| artifact.required && !artifacts.contains_key(&artifact.kind))
        .map(|artifact| artifact.kind.clone())
        .collect()
}

fn latest_artifact_across_run<'a>(
    run: &'a Run,
    before_sequence: u64,
    kind: &str,
) -> Option<&'a Event> {
    run.events.iter().rev().find(|event| {
        event.sequence < before_sequence
            && matches!(
                &event.action,
                Action::ArtifactRecorded { kind: candidate, .. } if candidate == kind
            )
    })
}

fn validate_artifact_actor(artifact: &ArtifactDefinition, actor: &Actor) -> Result<()> {
    let allowed = match artifact.actor {
        ActorRule::Human => actor.kind == ActorKind::Human,
        ActorRule::Ai => actor.kind == ActorKind::Ai,
        ActorRule::Any => matches!(actor.kind, ActorKind::Human | ActorKind::Ai),
    };
    if allowed {
        Ok(())
    } else {
        Err(AheadError::new(
            "artifact_actor_not_allowed",
            format!(
                "artifact {} requires actor {:?}; got {:?}",
                artifact.kind, artifact.actor, actor.kind
            ),
        ))
    }
}

fn require_human(event: &Event, action: &str) -> Result<()> {
    if event.actor.kind == ActorKind::Human {
        Ok(())
    } else {
        Err(AheadError::new(
            "human_action_required",
            format!("only a human may {action}"),
        ))
    }
}

fn ensure_not_first(first: bool) -> Result<()> {
    if first {
        Err(AheadError::new(
            "missing_start",
            "run_started must be the first event",
        ))
    } else {
        Ok(())
    }
}

fn ensure_current_phase(replay: &ReplayState, phase: &str) -> Result<()> {
    if replay.phase == phase {
        Ok(())
    } else {
        Err(AheadError::new(
            "stale_phase",
            format!(
                "event targets phase {phase}, current phase is {}",
                replay.phase
            ),
        ))
    }
}

fn validate_actor(actor: &Actor) -> Result<()> {
    validate_nonempty("actor identity", &actor.identity)
}

fn validate_nonempty(label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(AheadError::new(
            "invalid_value",
            format!("{label} cannot be empty"),
        ))
    } else {
        Ok(())
    }
}

fn validate_artifact_path(path: &str) -> Result<()> {
    validate_nonempty("artifact path", path)?;
    if path.starts_with('/') || path.split('/').any(|part| part == "..") {
        return Err(AheadError::new(
            "unsafe_artifact_path",
            "artifact paths must be relative and may not contain '..'",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn human(identity: &str) -> Actor {
        Actor {
            kind: ActorKind::Human,
            identity: identity.to_owned(),
        }
    }

    fn ai(identity: &str) -> Actor {
        Actor {
            kind: ActorKind::Ai,
            identity: identity.to_owned(),
        }
    }

    fn new_run() -> Run {
        create_run(CreateRunInput {
            id: "run-1".to_owned(),
            title: "Dogfood AHEAD".to_owned(),
            owner: human("implementer@example.com"),
            timestamp: "2026-08-12T12:00:00Z".to_owned(),
            workflow_id: "product-change".to_owned(),
        })
        .unwrap()
    }

    fn apply_action(run: &Run, actor: Actor, action: Action) -> Result<Run> {
        apply(
            run,
            ApplyInput {
                actor,
                timestamp: "2026-08-12T12:01:00Z".to_owned(),
                action,
            },
        )
    }

    fn complete_single_artifact_phase(
        run: Run,
        phase: &str,
        kind: &str,
        artifact_actor: Actor,
        gate: &str,
        next: &str,
    ) -> Run {
        let run = apply_action(
            &run,
            artifact_actor,
            Action::ArtifactRecorded {
                phase: phase.to_owned(),
                kind: kind.to_owned(),
                path: format!(".ahead/{phase}-{kind}.md"),
            },
        )
        .unwrap();
        let run = apply_action(
            &run,
            human("owner@example.com"),
            Action::GateAccepted {
                phase: phase.to_owned(),
                gate: gate.to_owned(),
            },
        )
        .unwrap();
        apply_action(
            &run,
            human("owner@example.com"),
            Action::PhaseTransitioned {
                from: phase.to_owned(),
                to: next.to_owned(),
                direction: TransitionDirection::Advance,
                reason: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn starts_in_define_with_ai_locked() {
        let state = derive_state(&new_run()).unwrap();
        assert_eq!(state.phase.id, "define");
        assert_eq!(state.phase.visit, 1);
        assert!(state.allowed_ai_capabilities.is_empty());
        assert!(!state.can_advance);
    }

    #[test]
    fn ai_cannot_write_human_artifact_or_accept_gate() {
        let run = new_run();
        let error = apply_action(
            &run,
            ai("model"),
            Action::ArtifactRecorded {
                phase: "define".to_owned(),
                kind: "problem".to_owned(),
                path: ".ahead/problem.md".to_owned(),
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "artifact_actor_not_allowed");

        let error = apply_action(
            &run,
            ai("model"),
            Action::GateAccepted {
                phase: "define".to_owned(),
                gate: "framing-accepted".to_owned(),
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "human_action_required");
    }

    #[test]
    fn requires_artifact_then_human_gate_before_advance() {
        let run = new_run();
        let error = apply_action(
            &run,
            human("implementer@example.com"),
            Action::GateAccepted {
                phase: "define".to_owned(),
                gate: "framing-accepted".to_owned(),
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "required_artifacts_missing");

        let run = apply_action(
            &run,
            human("implementer@example.com"),
            Action::ArtifactRecorded {
                phase: "define".to_owned(),
                kind: "problem".to_owned(),
                path: ".ahead/problem.md".to_owned(),
            },
        )
        .unwrap();
        assert!(tool_allowed(&run, Capability::Inspect).unwrap().allowed);
        assert!(!tool_allowed(&run, Capability::Modify).unwrap().allowed);

        let run = apply_action(
            &run,
            human("implementer@example.com"),
            Action::GateAccepted {
                phase: "define".to_owned(),
                gate: "framing-accepted".to_owned(),
            },
        )
        .unwrap();
        let run = apply_action(
            &run,
            human("implementer@example.com"),
            Action::PhaseTransitioned {
                from: "define".to_owned(),
                to: "research".to_owned(),
                direction: TransitionDirection::Advance,
                reason: None,
            },
        )
        .unwrap();
        assert_eq!(derive_state(&run).unwrap().phase.id, "research");
    }

    #[test]
    fn return_creates_new_visit_and_does_not_reuse_old_evidence() {
        let mut run = new_run();
        for (phase, kind, gate, next, actor) in [
            (
                "define",
                "problem",
                "framing-accepted",
                "research",
                human("owner"),
            ),
            (
                "research",
                "research",
                "research-reviewed",
                "questions",
                ai("model"),
            ),
        ] {
            run = apply_action(
                &run,
                actor,
                Action::ArtifactRecorded {
                    phase: phase.to_owned(),
                    kind: kind.to_owned(),
                    path: format!(".ahead/{phase}-{kind}.md"),
                },
            )
            .unwrap();
            run = apply_action(
                &run,
                human("owner"),
                Action::GateAccepted {
                    phase: phase.to_owned(),
                    gate: gate.to_owned(),
                },
            )
            .unwrap();
            run = apply_action(
                &run,
                human("owner"),
                Action::PhaseTransitioned {
                    from: phase.to_owned(),
                    to: next.to_owned(),
                    direction: TransitionDirection::Advance,
                    reason: None,
                },
            )
            .unwrap();
        }

        let run = apply_action(
            &run,
            human("owner"),
            Action::PhaseTransitioned {
                from: "questions".to_owned(),
                to: "research".to_owned(),
                direction: TransitionDirection::Return,
                reason: Some("evidence gap".to_owned()),
            },
        )
        .unwrap();
        let state = derive_state(&run).unwrap();
        assert_eq!(state.phase.id, "research");
        assert_eq!(state.phase.visit, 2);
        assert!(!state.artifacts[0].present);
        assert!(!state.can_advance);
    }

    #[test]
    fn human_first_artifact_unlocks_ai_assistance() {
        let run = complete_single_artifact_phase(
            new_run(),
            "define",
            "problem",
            human("owner@example.com"),
            "framing-accepted",
            "research",
        );
        let run = complete_single_artifact_phase(
            run,
            "research",
            "research",
            ai("model"),
            "research-reviewed",
            "questions",
        );
        let run = complete_single_artifact_phase(
            run,
            "questions",
            "unknowns",
            human("owner@example.com"),
            "unknowns-disposed",
            "options",
        );

        let error = apply_action(
            &run,
            ai("model"),
            Action::ArtifactRecorded {
                phase: "options".to_owned(),
                kind: "ai-challenge".to_owned(),
                path: ".ahead/ai-challenge.md".to_owned(),
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "ai_assistance_locked");

        let run = apply_action(
            &run,
            human("owner@example.com"),
            Action::ArtifactRecorded {
                phase: "options".to_owned(),
                kind: "human-option".to_owned(),
                path: ".ahead/human-option.md".to_owned(),
            },
        )
        .unwrap();
        let state = derive_state(&run).unwrap();
        assert!(state.allowed_ai_capabilities.contains(&Capability::Record));

        apply_action(
            &run,
            ai("model"),
            Action::ArtifactRecorded {
                phase: "options".to_owned(),
                kind: "ai-challenge".to_owned(),
                path: ".ahead/ai-challenge.md".to_owned(),
            },
        )
        .unwrap();
    }

    #[test]
    fn independent_review_rejects_implementer_identity() {
        let mut run = new_run();
        run.events.push(Event {
            sequence: 2,
            timestamp: "2026-08-12T12:02:00Z".to_owned(),
            actor: human("implementer@example.com"),
            action: Action::ArtifactRecorded {
                phase: "define".to_owned(),
                kind: "problem".to_owned(),
                path: ".ahead/problem.md".to_owned(),
            },
        });
        run.events.push(Event {
            sequence: 3,
            timestamp: "2026-08-12T12:03:00Z".to_owned(),
            actor: human("implementer@example.com"),
            action: Action::GateAccepted {
                phase: "define".to_owned(),
                gate: "framing-accepted".to_owned(),
            },
        });
        // Build a valid prefix through the human-review phase with the workflow API.
        let mut run = apply_action(
            &run,
            human("implementer@example.com"),
            Action::PhaseTransitioned {
                from: "define".to_owned(),
                to: "research".to_owned(),
                direction: TransitionDirection::Advance,
                reason: None,
            },
        )
        .unwrap();
        let definition = workflow("product-change").unwrap();
        while derive_state(&run).unwrap().phase.id != "human-review" {
            let state = derive_state(&run).unwrap();
            let phase = definition
                .phases
                .iter()
                .find(|item| item.id == state.phase.id)
                .unwrap();
            for artifact in phase.artifacts.iter().filter(|item| item.required) {
                let actor = match artifact.actor {
                    ActorRule::Ai => ai("model"),
                    ActorRule::Human | ActorRule::Any => human("implementer@example.com"),
                };
                run = apply_action(
                    &run,
                    actor,
                    Action::ArtifactRecorded {
                        phase: phase.id.clone(),
                        kind: artifact.kind.clone(),
                        path: format!(".ahead/{}-{}.md", phase.id, artifact.kind),
                    },
                )
                .unwrap();
            }
            run = apply_action(
                &run,
                human("implementer@example.com"),
                Action::GateAccepted {
                    phase: phase.id.clone(),
                    gate: phase.gate.id.clone(),
                },
            )
            .unwrap();
            run = apply_action(
                &run,
                human("implementer@example.com"),
                Action::PhaseTransitioned {
                    from: phase.id.clone(),
                    to: phase.next.clone().unwrap(),
                    direction: TransitionDirection::Advance,
                    reason: None,
                },
            )
            .unwrap();
        }

        let error = apply_action(
            &run,
            human("implementer@example.com"),
            Action::ArtifactRecorded {
                phase: "human-review".to_owned(),
                kind: "human-review".to_owned(),
                path: ".ahead/human-review.md".to_owned(),
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "independent_reviewer_required");

        let reviewed = apply_action(
            &run,
            human("reviewer@example.com"),
            Action::ArtifactRecorded {
                phase: "human-review".to_owned(),
                kind: "human-review".to_owned(),
                path: ".ahead/human-review.md".to_owned(),
            },
        )
        .unwrap();
        assert!(derive_state(&reviewed).unwrap().artifacts[0].present);

        let error = apply_action(
            &reviewed,
            human("implementer@example.com"),
            Action::GateAccepted {
                phase: "human-review".to_owned(),
                gate: "human-review-accepted".to_owned(),
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "gate_actor_mismatch");

        apply_action(
            &reviewed,
            human("reviewer@example.com"),
            Action::GateAccepted {
                phase: "human-review".to_owned(),
                gate: "human-review-accepted".to_owned(),
            },
        )
        .unwrap();
    }

    #[test]
    fn product_change_happy_path_closes_only_after_all_thirteen_phases() {
        let definition = workflow("product-change").unwrap();
        let mut run = new_run();
        let mut visited = Vec::new();

        loop {
            let state = derive_state(&run).unwrap();
            visited.push(state.phase.id.clone());
            let phase = definition
                .phases
                .iter()
                .find(|candidate| candidate.id == state.phase.id)
                .unwrap();
            let gate_actor = if phase.id == "human-review" {
                human("reviewer@example.com")
            } else {
                human("implementer@example.com")
            };

            for artifact in phase.artifacts.iter().filter(|artifact| artifact.required) {
                let actor = match artifact.actor {
                    ActorRule::Ai => ai("model"),
                    ActorRule::Human | ActorRule::Any if phase.id == "human-review" => {
                        human("reviewer@example.com")
                    }
                    ActorRule::Human | ActorRule::Any => human("implementer@example.com"),
                };
                run = apply_action(
                    &run,
                    actor,
                    Action::ArtifactRecorded {
                        phase: phase.id.clone(),
                        kind: artifact.kind.clone(),
                        path: format!(
                            ".ahead/{:04}-{}-{}.md",
                            run.events.len() + 1,
                            phase.id,
                            artifact.kind
                        ),
                    },
                )
                .unwrap();
            }
            run = apply_action(
                &run,
                gate_actor.clone(),
                Action::GateAccepted {
                    phase: phase.id.clone(),
                    gate: phase.gate.id.clone(),
                },
            )
            .unwrap();

            if let Some(next) = &phase.next {
                run = apply_action(
                    &run,
                    gate_actor,
                    Action::PhaseTransitioned {
                        from: phase.id.clone(),
                        to: next.clone(),
                        direction: TransitionDirection::Advance,
                        reason: None,
                    },
                )
                .unwrap();
            } else {
                run = apply_action(
                    &run,
                    gate_actor,
                    Action::RunClosed {
                        phase: phase.id.clone(),
                    },
                )
                .unwrap();
                break;
            }
        }

        assert_eq!(visited.len(), 13);
        assert_eq!(visited.first().unwrap(), "define");
        assert_eq!(visited.last().unwrap(), "outcome");
        assert!(derive_state(&run).unwrap().closed);
    }
}
