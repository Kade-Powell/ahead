//! Deterministic workflow definitions, event validation, and state derivation for AHEAD.
//!
//! The engine preserves human ownership by validating event actors, required artifacts,
//! phase gates, review independence, and phase-specific AI capabilities.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Serialization version for persisted [`Run`] records.
pub const RUN_API_VERSION: &str = "ahead.run/v0";
/// Request protocol version exposed by the WebAssembly engine adapter.
pub const ENGINE_API_VERSION: &str = "ahead.engine/v0";
/// Stable identifiers of all workflow definitions embedded in this engine build.
const WORKFLOW_IDS: &[&str] = &[
    "product-change",
    "corrective-debugging",
    "operational-stabilization",
    "decision",
    "investigation",
    "internal-improvement",
];
/// Current embedded specification for the product-change workflow.
const PRODUCT_CHANGE_SPEC: &str = include_str!("../../../spec/workflows/product-change-v0.2.json");
/// Historical product-change definition retained for replay of persisted runs.
const PRODUCT_CHANGE_V0_1_SPEC: &str =
    include_str!("../../../spec/workflows/legacy/product-change-v0.1.json");
/// Embedded specification for the corrective-debugging workflow.
const CORRECTIVE_DEBUGGING_SPEC: &str =
    include_str!("../../../spec/workflows/corrective-debugging-v0.1.json");
/// Embedded specification for the operational-stabilization workflow.
const OPERATIONAL_STABILIZATION_SPEC: &str =
    include_str!("../../../spec/workflows/operational-stabilization-v0.1.json");
/// Embedded specification for the decision workflow.
const DECISION_SPEC: &str = include_str!("../../../spec/workflows/decision-v0.1.json");
/// Embedded specification for the investigation workflow.
const INVESTIGATION_SPEC: &str = include_str!("../../../spec/workflows/investigation-v0.1.json");
/// Embedded specification for the internal-improvement workflow.
const INTERNAL_IMPROVEMENT_SPEC: &str =
    include_str!("../../../spec/workflows/internal-improvement-v0.1.json");

/// Declarative definition of an AHEAD workflow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowDefinition {
    /// Stable workflow identifier.
    pub id: String,
    /// Version of the workflow definition.
    pub version: String,
    /// Human-readable workflow title.
    pub title: String,
    /// Identifier of the first phase in a new run.
    pub initial_phase: String,
    /// Ordered phase definitions available to the workflow.
    pub phases: Vec<PhaseDefinition>,
}

/// Rules, evidence, capabilities, and transitions for one workflow phase.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PhaseDefinition {
    /// Stable phase identifier within the workflow.
    pub id: String,
    /// Human-readable phase title.
    pub title: String,
    /// Kind of actor accountable for the phase.
    pub owner: ActorKind,
    /// Evidence artifacts that may be recorded during the phase.
    pub artifacts: Vec<ArtifactDefinition>,
    /// Human gate that controls advancement from the phase.
    pub gate: GateDefinition,
    /// Next phase on a forward transition, or `None` for the final phase.
    pub next: Option<String>,
    /// Earlier phases to which a human may return the run.
    pub returns_to: Vec<String>,
    /// Human-authored artifact kinds required before AI assistance unlocks.
    #[serde(default)]
    pub ai_unlock_artifacts: Vec<String>,
    /// Capabilities available to AI after the phase's unlock requirements are met.
    #[serde(default)]
    pub ai_capabilities: Vec<Capability>,
}

/// Definition of an evidence artifact accepted during a phase.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactDefinition {
    /// Stable artifact kind used in events and validation.
    pub kind: String,
    /// Human-readable artifact title.
    pub title: String,
    /// Rule identifying which actor kinds may record the artifact.
    pub actor: ActorRule,
    /// Whether the artifact must exist before the phase gate can be accepted.
    pub required: bool,
    /// Artifact kind whose author must differ from this artifact's author.
    #[serde(default)]
    pub independent_of: Option<String>,
    /// Artifact kind whose author must also author this artifact.
    #[serde(default)]
    pub same_as: Option<String>,
}

/// Definition of the human approval gate for a phase.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GateDefinition {
    /// Stable gate identifier used in events.
    pub id: String,
    /// Human-readable gate title.
    pub title: String,
    /// Artifact whose author must personally accept the gate, when specified.
    #[serde(default)]
    pub accepted_by_artifact: Option<String>,
}

/// Classification of an actor participating in an AHEAD run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    /// A human participant accountable for decisions and gates.
    Human,
    /// An AI assistant operating within phase-specific boundaries.
    Ai,
    /// A deterministic system or previously authorized automation.
    System,
}

/// Actor-kind constraint applied to an artifact definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActorRule {
    /// Only a human may record the artifact.
    Human,
    /// Only an AI assistant may record the artifact.
    Ai,
    /// Either a human or AI assistant may record the artifact.
    Any,
}

/// A category of tool use that a workflow phase may permit for AI assistance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    /// Read or inspect available project material.
    Inspect,
    /// Search available project material.
    Search,
    /// Analyze evidence without changing project state.
    Analyze,
    /// Modify project files or other mutable project state.
    Modify,
    /// Execute a command or tool action.
    Execute,
    /// Record an artifact in the workflow run.
    Record,
}

/// Identified participant responsible for an event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Actor {
    /// Classification of the participant.
    pub kind: ActorKind,
    /// Stable identity attributable to the participant.
    pub identity: String,
}

/// Append-only event record for one execution of a workflow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Run {
    /// Serialization version for this run record.
    pub api_version: String,
    /// Unique run identifier.
    pub id: String,
    /// Human-readable run title.
    pub title: String,
    /// Identifier of the workflow being executed.
    pub workflow_id: String,
    /// Workflow definition version against which events are validated.
    pub workflow_version: String,
    /// Identity of the human who owns the run.
    pub owner: String,
    /// Ordered, append-only events that constitute the run.
    pub events: Vec<Event>,
}

/// Attributed action in a workflow run's event history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Event {
    /// One-based position of the event in the run.
    pub sequence: u64,
    /// Caller-provided timestamp for the event.
    pub timestamp: String,
    /// Participant responsible for the event.
    pub actor: Actor,
    /// Workflow action performed by the participant.
    #[serde(flatten)]
    pub action: Action,
}

/// State-changing operation represented by a workflow event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Starts a run in the workflow's initial phase.
    RunStarted {
        /// Initial phase identifier.
        phase: String,
    },
    /// Records an evidence artifact for the active phase visit.
    ArtifactRecorded {
        /// Phase in which the artifact is recorded.
        phase: String,
        /// Artifact kind defined by the phase.
        kind: String,
        /// Repository-relative path to the persisted artifact.
        path: String,
    },
    /// Records a human's acceptance of the active phase gate.
    GateAccepted {
        /// Phase whose gate is accepted.
        phase: String,
        /// Gate identifier defined by the phase.
        gate: String,
    },
    /// Moves a run forward or returns it to an allowed earlier phase.
    PhaseTransitioned {
        /// Phase being exited.
        from: String,
        /// Phase being entered.
        to: String,
        /// Whether the transition advances or returns the run.
        direction: TransitionDirection,
        /// Human rationale required for a return transition.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Closes a run after its final human gate has been accepted.
    RunClosed {
        /// Final phase in which the run is closed.
        phase: String,
    },
}

/// Direction of a workflow phase transition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransitionDirection {
    /// Move to the phase's declared next phase.
    Advance,
    /// Revisit an explicitly allowed earlier phase.
    Return,
}

/// Materialized state derived by replaying a valid [`Run`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunState {
    /// Identifier of the source run.
    pub run_id: String,
    /// Human-readable source run title.
    pub title: String,
    /// Identifier of the active workflow.
    pub workflow_id: String,
    /// Version of the active workflow.
    pub workflow_version: String,
    /// State of the current phase visit.
    pub phase: PhaseState,
    /// Artifact requirements and recorded evidence for the current visit.
    pub artifacts: Vec<ArtifactState>,
    /// State of the current human gate.
    pub gate: GateState,
    /// Conditions currently preventing completion or advancement.
    pub blockers: Vec<String>,
    /// AI capabilities currently unlocked by the workflow.
    pub allowed_ai_capabilities: Vec<Capability>,
    /// Earlier phases to which the human may return the run.
    pub return_targets: Vec<String>,
    /// Whether the accepted gate permits a forward transition or closure.
    pub can_advance: bool,
    /// Whether the run has been closed.
    pub closed: bool,
}

/// Materialized state for the current phase visit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PhaseState {
    /// Active phase identifier.
    pub id: String,
    /// Human-readable active phase title.
    pub title: String,
    /// One-based count of visits to this phase.
    pub visit: u32,
    /// Next phase on forward advancement, if one exists.
    pub next: Option<String>,
}

/// Materialized state of an artifact requirement in the current phase visit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactState {
    /// Stable artifact kind.
    pub kind: String,
    /// Human-readable artifact title.
    pub title: String,
    /// Actor-kind rule for recording the artifact.
    pub actor: ActorRule,
    /// Whether the artifact is required for gate acceptance.
    pub required: bool,
    /// Whether the artifact has been recorded in this phase visit.
    pub present: bool,
    /// Path to the recorded artifact, when present.
    pub path: Option<String>,
    /// Participant who recorded the artifact, when present.
    pub recorded_by: Option<Actor>,
}

/// Materialized state of the active phase's human gate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GateState {
    /// Stable gate identifier.
    pub id: String,
    /// Human-readable gate title.
    pub title: String,
    /// Whether a human has accepted the gate.
    pub accepted: bool,
    /// Human who accepted the gate, when accepted.
    pub accepted_by: Option<Actor>,
}

/// Decision describing whether an AI capability is currently allowed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolDecision {
    /// Whether the requested capability is allowed.
    pub allowed: bool,
    /// Human-readable explanation of the decision.
    pub reason: String,
    /// Capability evaluated by the decision.
    pub capability: Capability,
}

/// Attributed event input to append to an existing run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApplyInput {
    /// Participant responsible for the new event.
    pub actor: Actor,
    /// Caller-provided event timestamp.
    pub timestamp: String,
    /// Action to append to the run.
    pub action: Action,
}

/// Human-owned input used to create a workflow run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateRunInput {
    /// Unique identifier for the new run.
    pub id: String,
    /// Human-readable title for the new run.
    pub title: String,
    /// Human who owns and starts the run.
    pub owner: Actor,
    /// Caller-provided timestamp for the initial event.
    pub timestamp: String,
    /// Explicitly selected workflow identifier.
    pub workflow_id: String,
}

/// Structured validation or protocol error returned by the AHEAD engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AheadError {
    /// Stable machine-readable error code.
    pub code: String,
    /// Human-readable description of the failure.
    pub message: String,
}

impl AheadError {
    /// Constructs an engine error from a stable code and descriptive message.
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
        }
    }
}

/// Result type returned by AHEAD engine operations.
pub type Result<T> = std::result::Result<T, AheadError>;

/// Loads and parses an embedded workflow definition by identifier.
///
/// # Errors
///
/// Returns an error if the workflow identifier is unknown or its embedded definition is invalid.
pub fn workflow(workflow_id: &str) -> Result<WorkflowDefinition> {
    let specification = match workflow_id {
        "product-change" => PRODUCT_CHANGE_SPEC,
        "corrective-debugging" => CORRECTIVE_DEBUGGING_SPEC,
        "operational-stabilization" => OPERATIONAL_STABILIZATION_SPEC,
        "decision" => DECISION_SPEC,
        "investigation" => INVESTIGATION_SPEC,
        "internal-improvement" => INTERNAL_IMPROVEMENT_SPEC,
        _ => {
            return Err(AheadError::new(
                "unknown_workflow",
                format!("unknown workflow: {workflow_id}"),
            ));
        }
    };

    serde_json::from_str(specification).map_err(|error| {
        AheadError::new(
            "invalid_embedded_workflow",
            format!("embedded {workflow_id} workflow is invalid: {error}"),
        )
    })
}

/// Loads the exact workflow definition recorded by a persisted run.
fn workflow_version(workflow_id: &str, version: &str) -> Result<WorkflowDefinition> {
    let specification = if (workflow_id, version) == ("product-change", "0.1.0") {
        PRODUCT_CHANGE_V0_1_SPEC
    } else {
        let current = workflow(workflow_id)?;
        if current.version == version {
            return Ok(current);
        }
        return Err(AheadError::new(
            "unsupported_workflow_version",
            format!("engine does not provide {workflow_id}@{version}"),
        ));
    };
    serde_json::from_str(specification).map_err(|error| {
        AheadError::new(
            "invalid_embedded_workflow",
            format!("embedded {workflow_id}@{version} workflow is invalid: {error}"),
        )
    })
}

/// Loads every workflow definition embedded in this engine build.
///
/// The returned order is stable and suitable for presenting workflow choices to a user.
///
/// # Errors
///
/// Returns an error if any embedded definition is invalid.
pub fn workflows() -> Result<Vec<WorkflowDefinition>> {
    WORKFLOW_IDS.iter().map(|id| workflow(id)).collect()
}

/// Creates a new human-owned run in the selected workflow's initial phase.
///
/// # Errors
///
/// Returns an error when the owner is not a valid human, required input is empty, the workflow is
/// unknown, or its embedded definition is invalid.
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

/// Appends an attributed action after validating both the existing and candidate run.
///
/// # Errors
///
/// Returns an error if the existing run is invalid or the appended action violates workflow,
/// actor, artifact, gate, transition, or closure rules.
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

/// Validates a complete event history and materializes its current workflow state.
///
/// # Errors
///
/// Returns an error when the run metadata or any replayed event violates the selected workflow.
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
    let definition = workflow_version(&run.workflow_id, &run.workflow_version)?;
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

/// Derives the current workflow state from a valid run event history.
///
/// # Errors
///
/// Returns any validation error found while replaying the run.
pub fn derive_state(run: &Run) -> Result<RunState> {
    validate_run(run)
}

/// Evaluates whether an AI capability is unlocked in the run's current phase.
///
/// # Errors
///
/// Returns any validation error found while deriving the run state.
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

/// Artifact data retained while replaying the active phase visit.
#[derive(Debug, Clone)]
struct RecordedArtifact {
    /// Repository-relative artifact path.
    path: String,
    /// Participant who recorded the artifact.
    actor: Actor,
}

/// Mutable accumulator used while replaying a run's events.
#[derive(Debug, Clone)]
struct ReplayState {
    /// Identifier of the active phase.
    phase: String,
    /// One-based visit number for the active phase.
    visit: u32,
    /// Visit count accumulated for every entered phase.
    visits: BTreeMap<String, u32>,
    /// Artifacts recorded during the active phase visit.
    artifacts: BTreeMap<String, RecordedArtifact>,
    /// Human who accepted the active phase gate, when accepted.
    gate_actor: Option<Actor>,
    /// Whether the run has received its closing event.
    closed: bool,
}

impl ReplayState {
    /// Creates an inactive replay accumulator for a workflow definition.
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

    /// Enters a phase as a new visit and clears visit-scoped evidence.
    fn activate(&mut self, phase: &str) {
        let visit = self.visits.entry(phase.to_owned()).or_insert(0);
        *visit += 1;
        phase.clone_into(&mut self.phase);
        self.visit = *visit;
        self.artifacts.clear();
        self.gate_actor = None;
    }
}

/// Validates and applies one event to the replay accumulator.
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
            replay_run_started(definition, run, replay, event, first, phase)?;
        }
        Action::ArtifactRecorded { phase, kind, path } => {
            ensure_not_first(first)?;
            replay_artifact_recorded(definition, run, replay, event, phase, kind, path)?;
        }
        Action::GateAccepted { phase, gate } => {
            ensure_not_first(first)?;
            replay_gate_accepted(definition, replay, event, phase, gate)?;
        }
        Action::PhaseTransitioned {
            from,
            to,
            direction,
            reason,
        } => {
            ensure_not_first(first)?;
            replay_phase_transitioned(
                definition,
                replay,
                event,
                from,
                to,
                direction,
                reason.as_deref(),
            )?;
        }
        Action::RunClosed { phase } => {
            ensure_not_first(first)?;
            replay_run_closed(definition, replay, event, phase)?;
        }
    }
    Ok(())
}

/// Applies the single required start event to a replay accumulator.
fn replay_run_started(
    definition: &WorkflowDefinition,
    run: &Run,
    replay: &mut ReplayState,
    event: &Event,
    first: bool,
    phase: &str,
) -> Result<()> {
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
    if phase != definition.initial_phase {
        return Err(AheadError::new(
            "invalid_initial_phase",
            format!("workflow must start in {}", definition.initial_phase),
        ));
    }
    replay.activate(phase);
    Ok(())
}

/// Validates and records one artifact in the active phase visit.
fn replay_artifact_recorded(
    definition: &WorkflowDefinition,
    run: &Run,
    replay: &mut ReplayState,
    event: &Event,
    phase: &str,
    kind: &str,
    path: &str,
) -> Result<()> {
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
        .find(|candidate| candidate.kind == kind)
        .ok_or_else(|| {
            AheadError::new(
                "artifact_not_allowed",
                format!("artifact {kind} is not defined for phase {phase}"),
            )
        })?;
    validate_artifact_actor(artifact, &event.actor)?;
    validate_ai_artifact_access(phase_definition, replay, event, phase)?;
    validate_artifact_identity(run, event, artifact)?;
    replay.artifacts.insert(
        kind.to_owned(),
        RecordedArtifact {
            path: path.to_owned(),
            actor: event.actor.clone(),
        },
    );
    Ok(())
}

/// Validates whether an AI actor may record an artifact in the active phase.
fn validate_ai_artifact_access(
    phase_definition: &PhaseDefinition,
    replay: &ReplayState,
    event: &Event,
    phase: &str,
) -> Result<()> {
    if event.actor.kind != ActorKind::Ai {
        return Ok(());
    }
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
    Ok(())
}

/// Enforces cross-artifact identity relationships.
fn validate_artifact_identity(
    run: &Run,
    event: &Event,
    artifact: &ArtifactDefinition,
) -> Result<()> {
    if let Some(same_as) = &artifact.same_as {
        let prior = latest_artifact_across_run(run, event.sequence, same_as).ok_or_else(|| {
            AheadError::new(
                "same_actor_source_missing",
                format!("cannot establish matching identity without {same_as}"),
            )
        })?;
        if prior.actor.identity != event.actor.identity {
            return Err(AheadError::new(
                "same_actor_required",
                format!(
                    "{} must be recorded by {}, who recorded {}",
                    artifact.title, prior.actor.identity, same_as
                ),
            ));
        }
    }
    if let Some(independent_of) = &artifact.independent_of {
        let prior =
            latest_artifact_across_run(run, event.sequence, independent_of).ok_or_else(|| {
                AheadError::new(
                    "independence_source_missing",
                    format!("cannot establish independence without {independent_of}"),
                )
            })?;
        if prior.actor.identity == event.actor.identity {
            return Err(AheadError::new(
                "independent_reviewer_required",
                format!(
                    "{} must be recorded by someone other than {}",
                    artifact.title, prior.actor.identity
                ),
            ));
        }
    }
    Ok(())
}

/// Validates and records human acceptance of the active phase gate.
fn replay_gate_accepted(
    definition: &WorkflowDefinition,
    replay: &mut ReplayState,
    event: &Event,
    phase: &str,
    gate: &str,
) -> Result<()> {
    ensure_current_phase(replay, phase)?;
    require_human(event, "accept an AHEAD gate")?;
    let phase_definition = find_phase(definition, phase)?;
    if gate != phase_definition.gate.id {
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
    validate_gate_actor(phase_definition, replay, event)?;
    replay.gate_actor = Some(event.actor.clone());
    Ok(())
}

/// Ensures a gate tied to an artifact is accepted by that artifact's author.
fn validate_gate_actor(
    phase_definition: &PhaseDefinition,
    replay: &ReplayState,
    event: &Event,
) -> Result<()> {
    let Some(kind) = &phase_definition.gate.accepted_by_artifact else {
        return Ok(());
    };
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
    Ok(())
}

/// Validates and applies a forward or return phase transition.
fn replay_phase_transitioned(
    definition: &WorkflowDefinition,
    replay: &mut ReplayState,
    event: &Event,
    from: &str,
    to: &str,
    direction: &TransitionDirection,
    reason: Option<&str>,
) -> Result<()> {
    ensure_current_phase(replay, from)?;
    require_human(event, "transition an AHEAD phase")?;
    let phase_definition = find_phase(definition, from)?;
    find_phase(definition, to)?;
    match direction {
        TransitionDirection::Advance => validate_advance(phase_definition, replay, to, reason)?,
        TransitionDirection::Return => validate_return(phase_definition, from, to, reason)?,
    }
    replay.activate(to);
    Ok(())
}

/// Validates the gate, target, and empty rationale of a forward transition.
fn validate_advance(
    phase_definition: &PhaseDefinition,
    replay: &ReplayState,
    to: &str,
    reason: Option<&str>,
) -> Result<()> {
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
    if phase_definition.next.as_deref() != Some(to) {
        return Err(AheadError::new(
            "invalid_advance",
            format!("{to} is not the next phase after {}", phase_definition.id),
        ));
    }
    Ok(())
}

/// Validates the target and human rationale of a return transition.
fn validate_return(
    phase_definition: &PhaseDefinition,
    from: &str,
    to: &str,
    reason: Option<&str>,
) -> Result<()> {
    if !phase_definition
        .returns_to
        .iter()
        .any(|target| target == to)
    {
        return Err(AheadError::new(
            "invalid_return",
            format!("phase {from} cannot return to {to}"),
        ));
    }
    if reason.unwrap_or_default().trim().is_empty() {
        return Err(AheadError::new(
            "return_reason_required",
            "a return transition requires a reason",
        ));
    }
    Ok(())
}

/// Validates and applies closure after the final human gate.
fn replay_run_closed(
    definition: &WorkflowDefinition,
    replay: &mut ReplayState,
    event: &Event,
    phase: &str,
) -> Result<()> {
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
    Ok(())
}

/// Converts the completed replay accumulator into consumer-facing state.
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

/// Finds a phase definition by its workflow-local identifier.
fn find_phase<'a>(definition: &'a WorkflowDefinition, phase: &str) -> Result<&'a PhaseDefinition> {
    definition
        .phases
        .iter()
        .find(|candidate| candidate.id == phase)
        .ok_or_else(|| AheadError::new("unknown_phase", format!("unknown phase: {phase}")))
}

/// Returns required artifact kinds missing from the active visit.
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

/// Finds the most recent earlier artifact event of the requested kind.
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

/// Ensures an actor satisfies an artifact's actor-kind rule.
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

/// Ensures the event is attributable to a human participant.
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

/// Rejects any non-start action placed at the first event position.
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

/// Ensures an event targets the currently active phase.
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

/// Validates the required identity of a participant.
fn validate_actor(actor: &Actor) -> Result<()> {
    validate_nonempty("actor identity", &actor.identity)
}

/// Ensures a required string contains non-whitespace content.
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

/// Ensures an artifact path is nonempty, relative, and cannot traverse upward.
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
#[allow(
    clippy::missing_docs_in_private_items,
    clippy::too_many_lines,
    clippy::unwrap_used,
    reason = "test names and fail-fast assertions describe the behavior under test"
)]
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
        run: &Run,
        phase: &str,
        kind: &str,
        artifact_actor: Actor,
        gate: &str,
        next: &str,
    ) -> Run {
        let run = apply_action(
            run,
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
    fn all_embedded_workflows_are_structurally_consistent_and_human_led() {
        let definitions = workflows().unwrap();
        assert_eq!(definitions.len(), 6);
        assert_eq!(
            definitions
                .iter()
                .map(|definition| definition.id.as_str())
                .collect::<Vec<_>>(),
            WORKFLOW_IDS
        );

        for definition in definitions {
            let workflow_artifact_kinds = definition
                .phases
                .iter()
                .flat_map(|phase| phase.artifacts.iter())
                .map(|artifact| artifact.kind.as_str())
                .collect::<BTreeSet<_>>();
            let phase_ids = definition
                .phases
                .iter()
                .map(|phase| phase.id.as_str())
                .collect::<BTreeSet<_>>();
            assert_eq!(phase_ids.len(), definition.phases.len());
            assert!(phase_ids.contains(definition.initial_phase.as_str()));

            for phase in &definition.phases {
                assert_eq!(phase.owner, ActorKind::Human);
                assert!(
                    phase
                        .next
                        .as_deref()
                        .is_none_or(|next| phase_ids.contains(next))
                );
                assert!(
                    phase
                        .returns_to
                        .iter()
                        .all(|target| phase_ids.contains(target.as_str()))
                );

                let artifact_kinds = phase
                    .artifacts
                    .iter()
                    .map(|artifact| artifact.kind.as_str())
                    .collect::<BTreeSet<_>>();
                assert_eq!(artifact_kinds.len(), phase.artifacts.len());
                assert!(phase.ai_unlock_artifacts.iter().all(|kind| {
                    phase.artifacts.iter().any(|artifact| {
                        artifact.kind == *kind && artifact.actor == ActorRule::Human
                    })
                }));
                assert!(phase.artifacts.iter().all(|artifact| {
                    artifact.actor != ActorRule::Ai
                        || phase.ai_capabilities.contains(&Capability::Record)
                }));
                assert!(phase.artifacts.iter().all(|artifact| {
                    artifact.independent_of.is_none() || artifact.same_as.is_none()
                }));
                assert!(phase.artifacts.iter().all(|artifact| {
                    artifact
                        .independent_of
                        .as_deref()
                        .is_none_or(|kind| workflow_artifact_kinds.contains(kind))
                        && artifact
                            .same_as
                            .as_deref()
                            .is_none_or(|kind| workflow_artifact_kinds.contains(kind))
                }));
                assert!(phase.gate.accepted_by_artifact.as_ref().is_none_or(|kind| {
                    phase
                        .artifacts
                        .iter()
                        .any(|artifact| artifact.kind == *kind)
                }));
            }
        }
    }

    #[test]
    fn every_workflow_starts_at_its_human_owned_initial_gate() {
        for definition in workflows().unwrap() {
            let run = create_run(CreateRunInput {
                id: format!("{}-run", definition.id),
                title: definition.title,
                owner: human("owner@example.com"),
                timestamp: "2026-08-12T12:00:00Z".to_owned(),
                workflow_id: definition.id.clone(),
            })
            .unwrap();
            let state = derive_state(&run).unwrap();
            assert_eq!(state.workflow_id, definition.id);
            assert_eq!(state.phase.id, definition.initial_phase);
            assert!(state.gate.accepted_by.is_none());
            assert!(!state.can_advance);
        }
    }

    #[test]
    fn published_product_change_v0_1_runs_remain_replayable() {
        let run = Run {
            api_version: RUN_API_VERSION.to_owned(),
            id: "legacy-product-change".to_owned(),
            title: "Existing work".to_owned(),
            workflow_id: "product-change".to_owned(),
            workflow_version: "0.1.0".to_owned(),
            owner: "owner@example.com".to_owned(),
            events: vec![Event {
                sequence: 1,
                timestamp: "2026-08-12T12:00:00Z".to_owned(),
                actor: human("owner@example.com"),
                action: Action::RunStarted {
                    phase: "define".to_owned(),
                },
            }],
        };
        let state = derive_state(&run).unwrap();
        assert_eq!(state.workflow_version, "0.1.0");
        assert_eq!(state.phase.id, "define");
        assert_eq!(workflow("product-change").unwrap().version, "0.2.0");
    }

    #[test]
    fn operational_intervention_execution_is_never_an_ai_capability() {
        let definition = workflow("operational-stabilization").unwrap();
        for phase_id in ["respond", "execute-observe"] {
            let phase = definition
                .phases
                .iter()
                .find(|phase| phase.id == phase_id)
                .unwrap();
            assert!(!phase.ai_capabilities.contains(&Capability::Execute));
        }
        let execute = definition
            .phases
            .iter()
            .find(|phase| phase.id == "execute-observe")
            .unwrap();
        assert_eq!(execute.artifacts[0].actor, ActorRule::Human);
    }

    #[test]
    fn lasting_change_ai_review_requires_separate_human_disposition() {
        for workflow_id in [
            "product-change",
            "corrective-debugging",
            "internal-improvement",
        ] {
            let definition = workflow(workflow_id).unwrap();
            let phase = definition
                .phases
                .iter()
                .find(|phase| phase.id == "ai-review")
                .unwrap();
            let ai_review = phase
                .artifacts
                .iter()
                .find(|artifact| artifact.kind == "ai-review")
                .unwrap();
            let disposition = phase
                .artifacts
                .iter()
                .find(|artifact| artifact.kind == "review-disposition")
                .unwrap();
            assert!(ai_review.required);
            assert_eq!(ai_review.actor, ActorRule::Ai);
            assert!(disposition.required);
            assert_eq!(disposition.actor, ActorRule::Human);
            assert_eq!(disposition.same_as.as_deref(), Some("changeset"));
            assert_eq!(
                phase.gate.accepted_by_artifact.as_deref(),
                Some("review-disposition")
            );

            let audit = definition
                .phases
                .iter()
                .find(|phase| phase.id == "ai-audit")
                .unwrap();
            let audit_disposition = audit
                .artifacts
                .iter()
                .find(|artifact| artifact.kind == "audit-disposition")
                .unwrap();
            assert!(audit_disposition.required);
            assert_eq!(audit_disposition.actor, ActorRule::Human);
            assert_eq!(
                audit.gate.accepted_by_artifact.as_deref(),
                Some("audit-disposition")
            );
        }
    }

    #[test]
    fn every_workflow_can_complete_only_through_its_required_artifacts_and_human_gates() {
        for definition in workflows().unwrap() {
            let mut run = create_run(CreateRunInput {
                id: format!("{}-complete", definition.id),
                title: definition.title.clone(),
                owner: human("owner@example.com"),
                timestamp: "2026-08-12T12:00:00Z".to_owned(),
                workflow_id: definition.id.clone(),
            })
            .unwrap();

            loop {
                let state = derive_state(&run).unwrap();
                if state.closed {
                    break;
                }
                let phase = definition
                    .phases
                    .iter()
                    .find(|phase| phase.id == state.phase.id)
                    .unwrap();
                for artifact in phase.artifacts.iter().filter(|artifact| artifact.required) {
                    let actor = match artifact.actor {
                        ActorRule::Ai => ai("model"),
                        ActorRule::Human | ActorRule::Any if artifact.independent_of.is_some() => {
                            human("reviewer@example.com")
                        }
                        ActorRule::Human | ActorRule::Any => human("owner@example.com"),
                    };
                    run = apply_action(
                        &run,
                        actor,
                        Action::ArtifactRecorded {
                            phase: phase.id.clone(),
                            kind: artifact.kind.clone(),
                            path: format!(
                                ".ahead/{}/{}/{}.md",
                                definition.id, phase.id, artifact.kind
                            ),
                        },
                    )
                    .unwrap();
                }

                let state = derive_state(&run).unwrap();
                let gate_actor = phase
                    .gate
                    .accepted_by_artifact
                    .as_ref()
                    .and_then(|kind| {
                        state
                            .artifacts
                            .iter()
                            .find(|artifact| artifact.kind == *kind)
                            .and_then(|artifact| artifact.recorded_by.clone())
                    })
                    .unwrap_or_else(|| human("owner@example.com"));
                run = apply_action(
                    &run,
                    gate_actor,
                    Action::GateAccepted {
                        phase: phase.id.clone(),
                        gate: phase.gate.id.clone(),
                    },
                )
                .unwrap();
                run = apply_action(
                    &run,
                    human("owner@example.com"),
                    phase.next.as_ref().map_or_else(
                        || Action::RunClosed {
                            phase: phase.id.clone(),
                        },
                        |next| Action::PhaseTransitioned {
                            from: phase.id.clone(),
                            to: next.clone(),
                            direction: TransitionDirection::Advance,
                            reason: None,
                        },
                    ),
                )
                .unwrap();
            }

            assert!(derive_state(&run).unwrap().closed);
        }
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
            &new_run(),
            "define",
            "problem",
            human("owner@example.com"),
            "framing-accepted",
            "research",
        );
        let run = complete_single_artifact_phase(
            &run,
            "research",
            "research",
            ai("model"),
            "research-reviewed",
            "questions",
        );
        let run = complete_single_artifact_phase(
            &run,
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
    fn review_identity_rules_bind_disposition_and_require_independence() {
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
                if artifact.kind == "review-disposition" {
                    let error = apply_action(
                        &run,
                        human("reviewer@example.com"),
                        Action::ArtifactRecorded {
                            phase: phase.id.clone(),
                            kind: artifact.kind.clone(),
                            path: ".ahead/reviewer-disposition.md".to_owned(),
                        },
                    )
                    .unwrap_err();
                    assert_eq!(error.code, "same_actor_required");
                }
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
