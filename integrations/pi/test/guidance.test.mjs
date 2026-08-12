import assert from "node:assert/strict";
import test from "node:test";
import {
  buildArtifactTemplate,
  buildWidgetLines,
  nextAction,
} from "../src/guidance.ts";

const phaseIds = [
  "define",
  "research",
  "questions",
  "options",
  "decision",
  "plan",
  "implement",
  "ai-review",
  "human-review",
  "deploy",
  "verify",
  "ai-audit",
  "outcome",
];

const workflow = {
  id: "product-change",
  version: "0.1.0",
  title: "Product Change",
  initial_phase: "define",
  phases: phaseIds.map((id, index) => ({
    id,
    title: id.split("-").map((word) => `${word[0].toUpperCase()}${word.slice(1)}`).join(" "),
    owner: "human",
    artifacts: [],
    gate: { id: `${id}-gate`, title: `${id} gate` },
    next: phaseIds[index + 1] ?? null,
    returns_to: [],
    ai_unlock_artifacts: [],
    ai_capabilities: [],
  })),
};

const run = {
  api_version: "ahead.run/v0",
  id: "guided-test",
  title: "Guided mode test",
  workflow_id: "product-change",
  workflow_version: "0.1.0",
  owner: "human@example.com",
  events: [],
};

function state(phase, artifacts, overrides = {}) {
  const definition = workflow.phases.find((candidate) => candidate.id === phase);
  return {
    run_id: run.id,
    title: run.title,
    workflow_id: workflow.id,
    workflow_version: workflow.version,
    phase: { id: phase, title: definition.title, visit: 1, next: definition.next },
    artifacts,
    gate: { id: `${phase}-gate`, title: `${phase} gate`, accepted: false, accepted_by: null },
    blockers: [],
    allowed_ai_capabilities: [],
    return_targets: [],
    can_advance: false,
    closed: false,
    ...overrides,
  };
}

function artifact(kind, title, actor, required, present = false) {
  return { kind, title, actor, required, present, path: null, recorded_by: null };
}

test("define begins with a clearly human-owned problem record", () => {
  const action = nextAction(
    state("define", [artifact("problem", "Problem and success signals", "human", true)]),
    workflow,
  );
  assert.deepEqual(action, {
    actor: "human",
    artifactKind: "problem",
    label: "Write Problem and success signals",
  });
});

test("shared research is routed to AI in the documented Product Change sequence", () => {
  const action = nextAction(
    state("research", [artifact("research", "Evidence and gaps", "any", true)]),
    workflow,
  );
  assert.deepEqual(action, {
    actor: "ai",
    artifactKind: "research",
    label: "Ask AI to produce Evidence and gaps",
  });
});

test("human first pass precedes optional AI challenge and final human option", () => {
  const artifacts = [
    artifact("human-option", "Human first-pass option", "human", true, true),
    artifact("ai-challenge", "AI challenge", "ai", false),
    artifact("options", "Human-evaluated options", "human", true),
  ];
  assert.deepEqual(nextAction(state("options", artifacts), workflow), {
    actor: "ai",
    artifactKind: "ai-challenge",
    label: "Ask AI to challenge the human option and expand alternatives",
    optional: true,
  });
});

test("AI review is explicitly bound in guidance to the current changeset", () => {
  const action = nextAction(
    state("ai-review", [artifact("ai-review", "AI review findings", "ai", true)]),
    workflow,
  );
  assert.equal(action.actor, "ai");
  assert.match(action.label, /exact current changeset/);
});

test("human-review widget makes the independent handoff and authority boundary visible", () => {
  const current = state(
    "human-review",
    [artifact("human-review", "Independent human review", "human", true)],
  );
  const widget = buildWidgetLines(run, current, workflow).join("\n");
  assert.match(widget, /READY FOR INDEPENDENT HUMAN REVIEW/);
  assert.match(widget, /HUMAN LEADS · AI ASSISTS/);
  assert.match(widget, /Next \(you\): Independent reviewer records/);
});

test("human artifact editor explains the expected record instead of showing a blank form", () => {
  const current = state(
    "define",
    [artifact("problem", "Problem and success signals", "human", true)],
  );
  const template = buildArtifactTemplate(run, current, "problem", "Problem and success signals");
  assert.match(template, /Who experiences the problem/);
  assert.match(template, /observable signals/);
  assert.match(template, /human-owned record in your own words/);
});
