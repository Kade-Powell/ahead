import assert from "node:assert/strict";
import test from "node:test";
import { draftFieldExamples, parseExampleLines } from "../src/examples.ts";
import {
  buildArtifactTemplate,
  insertFieldExamples,
  promptsForArtifact,
  validateArtifactForm,
} from "../src/guidance.ts";

const run = {
  api_version: "ahead.run/v0",
  id: "examples-test",
  title: "Examples test",
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

function artifact(kind, title) {
  return {
    kind,
    title,
    actor: "human",
    required: true,
    present: false,
    recorded_by: null,
    path: null,
  };
}

test("parseExampleLines collects numbered pairs and caps at two per field", () => {
  const message = {
    content: [
      {
        type: "text",
        text: [
          "1: keep response times flat",
          "1: preserve the current SLA",
          "1: a third line for field one is dropped",
          "2: record p99 before and after",
          "not a field line",
          "9: out of range is ignored",
          "2: second line for field two",
        ].join("\n"),
      },
    ],
  };
  assert.deepEqual(parseExampleLines(message, 2), [
    ["keep response times flat", "preserve the current SLA"],
    ["record p99 before and after", "second line for field two"],
  ]);
});

test("parseExampleLines returns undefined when nothing parses", () => {
  assert.equal(
    parseExampleLines({ content: [{ type: "text", text: "no field lines here" }] }, 2),
    undefined,
  );
  assert.equal(parseExampleLines({}, 2), undefined);
  assert.equal(parseExampleLines(undefined, 2), undefined);
  assert.equal(parseExampleLines({ content: "not an array" }, 2), undefined);
});

test("draftFieldExamples reports no-model without calling the model", async () => {
  const result = await draftFieldExamples(
    { model: undefined },
    { workflowTitle: "Product Change", phaseTitle: "Define", runTitle: "x", fields: ["a?"] },
  );
  assert.deepEqual(result, { skipped: "no-model" });
});

test("draftFieldExamples reports no-auth when no credential resolves", async () => {
  const result = await draftFieldExamples(
    {
      model: { provider: "example", id: "example/model" },
      modelRegistry: { getProviderAuth: async () => undefined },
    },
    { workflowTitle: "Product Change", phaseTitle: "Define", runTitle: "x", fields: ["a?"] },
  );
  assert.deepEqual(result, { skipped: "no-auth" });
});

test("inserted examples are quiet plain-text lines and do not satisfy validation", () => {
  const current = state("define", [artifact("problem", "Problem and success signals")]);
  const prompts = promptsForArtifact("product-change", "define", "problem");
  assert.ok(prompts.length > 0);
  const template = buildArtifactTemplate(run, current, "problem", "Problem and success signals");

  const seeded = insertFieldExamples(
    template,
    prompts.map((_, index) => [`seeded example one for field ${index + 1}`, "seeded example two"]),
  );
  assert.match(seeded, /ex\. - seeded example one for field 1/);
  assert.doesNotMatch(seeded, /inspiration only/);
  assert.doesNotMatch(seeded, /<!-- ~/);

  // The mechanical guarantee: untouched example lines never satisfy a field.
  const errors = validateArtifactForm(seeded, prompts);
  assert.equal(errors.length, prompts.length);
  assert.match(errors[0], /replace the “ex\. -” example lines/);
});

test("leftover example lines are rejected even alongside real content", () => {
  const current = state("define", [artifact("problem", "Problem and success signals")]);
  const prompts = promptsForArtifact("product-change", "define", "problem");
  const template = buildArtifactTemplate(run, current, "problem", "Problem and success signals");

  const seeded = insertFieldExamples(
    template,
    prompts.map(() => ["seeded example"]),
  );
  const answered = seeded.replace(
    "<!-- AHEAD-FIELD:1:BEGIN -->\n",
    "<!-- AHEAD-FIELD:1:BEGIN -->\nMaintainers cannot tell which inputs are required.\n",
  );
  const errors = validateArtifactForm(answered, prompts);
  assert.ok(errors.some((error) => /remove the leftover “ex\. -” example lines/.test(error)));
  assert.equal(errors.length, prompts.length);

  const cleaned = answered.replace(/^\s*ex\.\s*-.*$/gm, "");
  assert.deepEqual(validateArtifactForm(cleaned, prompts), prompts.slice(1));
});

test("insertFieldExamples skips empty or unknown fields", () => {
  const current = state("define", [artifact("problem", "Problem and success signals")]);
  const template = buildArtifactTemplate(run, current, "problem", "Problem and success signals");
  assert.equal(insertFieldExamples(template, []), template);
  assert.equal(insertFieldExamples(template, [[], []]), template);
});
