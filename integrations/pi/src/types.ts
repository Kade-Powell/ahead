export type ActorKind = "human" | "ai" | "system";
export type ActorRule = "human" | "ai" | "any";
export type Capability = "inspect" | "search" | "analyze" | "modify" | "execute" | "record";

export interface Actor {
  kind: ActorKind;
  identity: string;
}

export interface ArtifactDefinition {
  kind: string;
  title: string;
  actor: ActorRule;
  required: boolean;
  independent_of?: string;
  same_as?: string;
}

export interface PhaseDefinition {
  id: string;
  title: string;
  owner: ActorKind;
  artifacts: ArtifactDefinition[];
  gate: { id: string; title: string; accepted_by_artifact?: string };
  next: string | null;
  returns_to: string[];
  ai_unlock_artifacts: string[];
  ai_capabilities: Capability[];
}

export interface WorkflowDefinition {
  id: string;
  version: string;
  title: string;
  initial_phase: string;
  phases: PhaseDefinition[];
}

export interface Event {
  sequence: number;
  timestamp: string;
  actor: Actor;
  type: "run_started" | "artifact_recorded" | "gate_accepted" | "phase_transitioned" | "run_closed";
  phase?: string;
  kind?: string;
  path?: string;
  gate?: string;
  from?: string;
  to?: string;
  direction?: "advance" | "return";
  reason?: string;
}

export interface Run {
  api_version: string;
  id: string;
  title: string;
  workflow_id: string;
  workflow_version: string;
  owner: string;
  events: Event[];
}

export interface ArtifactState extends ArtifactDefinition {
  present: boolean;
  path: string | null;
  recorded_by: Actor | null;
}

export interface RunState {
  run_id: string;
  title: string;
  workflow_id: string;
  workflow_version: string;
  phase: { id: string; title: string; visit: number; next: string | null };
  artifacts: ArtifactState[];
  gate: { id: string; title: string; accepted: boolean; accepted_by: Actor | null };
  blockers: string[];
  allowed_ai_capabilities: Capability[];
  return_targets: string[];
  can_advance: boolean;
  closed: boolean;
}

export interface EngineErrorShape {
  code: string;
  message: string;
}

export type EventAction =
  | { type: "artifact_recorded"; phase: string; kind: string; path: string }
  | { type: "gate_accepted"; phase: string; gate: string }
  | {
      type: "phase_transitioned";
      from: string;
      to: string;
      direction: "advance" | "return";
      reason?: string;
    }
  | { type: "run_closed"; phase: string };
