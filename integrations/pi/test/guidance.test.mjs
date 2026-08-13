import assert from "node:assert/strict";
import test from "node:test";
import {
  buildArtifactTemplate,
  buildHeaderLines,
  nextAction,
  phaseGuide,
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
    title: id
      .split("-")
      .map((word) => `${word[0].toUpperCase()}${word.slice(1)}`)
      .join(" "),
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
    policy: { work_items: { required_before_phase: null } },
    work_item: null,
    work_item_required_for_next_phase: false,
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
    state("research", [artifact("research", "Evidence and gaps", "any", true)], {
      allowed_ai_capabilities: ["inspect", "search", "analyze", "record"],
    }),
    workflow,
  );
  assert.deepEqual(action, {
    actor: "ai",
    artifactKind: "research",
    label: "Ask AI to produce Evidence and gaps",
  });
});

test("shared evidence can remain human-led even when AI recording is available", () => {
  const current = state("research", [artifact("research", "Evidence and gaps", "any", true)], {
    allowed_ai_capabilities: ["inspect", "search", "analyze", "record"],
  });
  const action = nextAction(current, workflow);
  assert.equal(action.actor, "ai");
  assert.equal(current.artifacts[0].actor, "any");
});

test("human first pass precedes optional AI challenge and final human option", () => {
  const artifacts = [
    artifact("human-option", "Human first-pass option", "human", true, true),
    artifact("ai-challenge", "AI challenge", "ai", false),
    artifact("options", "Human-evaluated options", "human", true),
  ];
  assert.deepEqual(
    nextAction(
      state("options", artifacts, {
        allowed_ai_capabilities: ["inspect", "search", "analyze", "record"],
      }),
      workflow,
    ),
    {
      actor: "ai",
      artifactKind: "ai-challenge",
      label: "Ask AI to challenge the human option and expand alternatives",
      optional: true,
    },
  );
});

test("AI review is explicitly bound in guidance to the current changeset", () => {
  const action = nextAction(
    state("ai-review", [artifact("ai-review", "AI review findings", "ai", true)]),
    workflow,
  );
  assert.equal(action.actor, "ai");
  assert.match(action.label, /exact current changeset/);
  const guide = phaseGuide("product-change", "ai-review");
  assert.match(guide.artifactPrompts["ai-review"].join("\n"), /stable AR findings/);
  assert.match(guide.artifactPrompts["review-disposition"].join("\n"), /fixed, invalid/);
});

test("implementation guidance keeps the engineer first while making help explicit", () => {
  const current = state("implement", [artifact("changeset", "Linked changeset", "human", true)], {
    allowed_ai_capabilities: ["inspect", "analyze", "modify", "execute"],
  });
  const action = nextAction(current, workflow);
  assert.deepEqual(action, {
    actor: "human",
    artifactKind: "changeset",
    label: "Implement first, then record the exact changeset",
  });
  const guide = phaseGuide("product-change", "implement");
  assert.match(guide.human, /Make the first attempt, ask questions freely/);
  assert.match(guide.ai, /Do not turn a question into taking over/);
});

test("active AHEAD header is compact and makes the next owner visible", () => {
  const current = state("human-review", [
    artifact("human-review", "Independent human review", "human", true),
  ]);
  const lines = buildHeaderLines(run, current, workflow);
  const header = lines.join("\n");
  assert.equal(lines.length, 4);
  assert.match(header, /AHEAD · Product Change · 9\/13 · Human Review/);
  assert.match(header, /Next: You → Independent reviewer records/);
  assert.doesNotMatch(header, /HUMAN LEADS · AI ASSISTS/);
  assert.doesNotMatch(header, /Run \/ahead/);
});

test("linked work item is visible and satisfies the configured planning boundary", () => {
  const current = state(
    "plan",
    [
      artifact("first-pass-plan", "Human first-pass implementation plan", "human", true, true),
      artifact("plan", "Approved plan", "human", true, true),
    ],
    {
      gate: {
        id: "plan-gate",
        title: "plan gate",
        accepted: true,
        accepted_by: { kind: "human", identity: "planner@example.com" },
      },
      policy: { work_items: { required_before_phase: "implement" } },
      work_item: {
        provider: "github",
        url: "https://github.com/example/project/issues/42",
        external_id: "42",
      },
      work_item_required_for_next_phase: false,
    },
  );
  const header = buildHeaderLines(run, current, workflow).join("\n");
  assert.match(header, /Work item: https:\/\/github.com\/example\/project\/issues\/42/);
  assert.match(header, /Next: You → Continue to Implement/);
});

test("accepted plan routes the human to a missing required work item", () => {
  const current = state("plan", [], {
    gate: {
      id: "plan-gate",
      title: "plan gate",
      accepted: true,
      accepted_by: { kind: "human", identity: "planner@example.com" },
    },
    policy: { work_items: { required_before_phase: "implement" } },
    work_item_required_for_next_phase: true,
  });
  assert.deepEqual(nextAction(current, workflow), {
    actor: "human",
    label: "Link or create the required work item before implement",
  });
});

test("human artifact editor explains the expected record instead of showing a blank form", () => {
  const current = state("define", [
    artifact("problem", "Problem and success signals", "human", true),
  ]);
  const template = buildArtifactTemplate(run, current, "problem", "Problem and success signals");
  assert.match(template, /Who experiences the problem/);
  assert.match(template, /observable signals/);
  assert.match(template, /human-owned record in your own words/);
});

test("workflow-specific guides expose debugging and operational authority boundaries", () => {
  const debugging = phaseGuide("corrective-debugging", "model");
  assert.match(debugging.human, /currently think is happening/);
  assert.match(debugging.ai, /After the human model/);

  const operations = phaseGuide("operational-stabilization", "execute-observe");
  assert.match(operations.human, /authorized actor/);
  assert.match(operations.ai, /no workflow authority to execute/);
});
