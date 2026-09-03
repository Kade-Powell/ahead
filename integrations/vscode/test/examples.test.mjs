import assert from "node:assert/strict";
import test from "node:test";
import { parseExampleLines } from "../src/examples.ts";
import {
  buildArtifactTemplate,
  insertFieldExamples,
  promptsForArtifact,
  validateArtifactForm,
} from "../../pi/src/guidance.ts";

const run = {
  api_version: "ahead.run/v0",
  id: "vscode-examples-test",
  title: "VSCode examples test",
  workflow_id: "product-change",
  workflow_version: "0.1.0",
  owner: "human@example.com",
  events: [],
};

function state(phase, artifacts) {
  return {
    run_id: run.id,
    title: run.title,
    workflow_id: run.workflow_id,
    workflow_version: run.workflow_version,
    policy: { work_items: { required_before_phase: null } },
    work_item: null,
    work_item_required_for_next_phase: false,
    phase: { id: phase, title: phase, visit: 1, next: null },
    artifacts,
    gate: { id: `${phase}-gate`, title: `${phase} gate`, accepted: false, accepted_by: null },
    blockers: [],
    allowed_ai_capabilities: [],
    return_targets: [],
    can_advance: false,
    closed: false,
  };
}

test("parseExampleLines collects numbered pairs and caps at two per field", () => {
  const text = [
    "1: keep response times flat",
    "1: preserve the current SLA",
    "1: a third line for field one is dropped",
    "2: record p99 before and after",
    "not a field line",
    "9: out of range is ignored",
    "2: second line for field two",
  ].join("\n");
  assert.deepEqual(parseExampleLines(text, 2), [
    ["keep response times flat", "preserve the current SLA"],
    ["record p99 before and after", "second line for field two"],
  ]);
});

test("parseExampleLines returns undefined when nothing parses", () => {
  assert.equal(parseExampleLines("no field lines here", 2), undefined);
  assert.equal(parseExampleLines("", 2), undefined);
});

test("inserted examples are labeled and do not satisfy validation", () => {
  const current = state("define", [
    {
      kind: "problem",
      title: "Problem and success signals",
      actor: "human",
      required: true,
      present: false,
      recorded_by: null,
      path: null,
    },
  ]);
  const prompts = promptsForArtifact("product-change", "define", "problem");
  assert.ok(prompts.length > 0);
  const template = buildArtifactTemplate(run, current, "problem", "Problem and success signals");

  const seeded = insertFieldExamples(
    template,
    prompts.map((_, index) => [`seeded example one for field ${index + 1}`, "seeded example two"]),
  );
  assert.match(seeded, /inspiration only, not requirements/);

  // The mechanical guarantee: examples are comments, so an untouched field
  // still counts as empty and the AI cannot author the artifact by prefilling.
  assert.deepEqual(validateArtifactForm(seeded, prompts), prompts);
});
