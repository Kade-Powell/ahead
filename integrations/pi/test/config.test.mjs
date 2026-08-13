import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import {
  projectConfigIssues,
  recommendedProjectConfig,
  runProjectConfigWizard,
} from "../src/config.ts";
import { RunStore } from "../src/storage.ts";

function workflow(id, phases) {
  return {
    id,
    version: "0.1.0",
    title: id,
    initial_phase: phases[0],
    phases: phases.map((phase) => ({ id: phase })),
  };
}

test("recommended project configuration places work-item gates at planning boundaries", () => {
  const config = recommendedProjectConfig([
    workflow("product-change", ["define", "plan", "implement"]),
    workflow("corrective-debugging", ["characterize", "plan", "implement"]),
    workflow("internal-improvement", ["baseline", "plan", "implement"]),
    workflow("decision", ["frame", "decide", "publish"]),
    workflow("investigation", ["frame", "synthesize", "conclude"]),
    workflow("operational-stabilization", ["assess", "monitor", "outcome"]),
  ]);

  assert.deepEqual(config.work_items.required_before_phase, {
    "product-change": "implement",
    "corrective-debugging": "implement",
    "internal-improvement": "implement",
    decision: "publish",
    investigation: "conclude",
    "operational-stabilization": "outcome",
  });
});

test("recommended configuration fails closed for a workflow without a reviewed default", () => {
  assert.throws(
    () => recommendedProjectConfig([workflow("local-experiment", ["try", "conclude"])]),
    /no recommended work-item boundary exists for workflow local-experiment/,
  );
});

test("recommended configuration fails if its workflow contract drifts", () => {
  assert.throws(
    () => recommendedProjectConfig([workflow("product-change", ["define", "deliver"])]),
    /recommended work-item boundary implement does not exist/,
  );
});

test("project configuration detects boundaries made stale by workflow changes", () => {
  const workflows = [workflow("product-change", ["define", "plan", "implement"])];

  assert.deepEqual(
    projectConfigIssues(
      {
        api_version: "ahead.config/v0",
        work_items: {
          required_before_phase: {
            "product-change": "build",
            retired: "implement",
          },
        },
      },
      workflows,
    ),
    [
      "work-item policy for product-change names unknown phase build",
      "work-item policy names unknown workflow retired",
    ],
  );
  assert.deepEqual(
    projectConfigIssues(
      {
        api_version: "ahead.config/v0",
        work_items: { required_before_phase: { "product-change": "define" } },
      },
      workflows,
    ),
    ["work-item boundary for product-change cannot be its initial phase define"],
  );
});

test("setup wizard previews and saves the selected project policy", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "ahead-config-wizard-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const store = new RunStore(root);
  const workflows = [workflow("product-change", ["define", "plan", "implement"])];
  const notices = [];
  const ctx = {
    hasUI: true,
    ui: {
      select: async () => "Recommended planning boundaries",
      confirm: async (title, body) => {
        assert.equal(title, "Create this AHEAD configuration?");
        assert.match(body, /"product-change": "implement"/);
        return true;
      },
      notify: (message) => notices.push(message),
    },
  };

  assert.equal(await runProjectConfigWizard(ctx, store, workflows, false), true);
  assert.deepEqual(await store.loadProjectConfig(), {
    api_version: "ahead.config/v0",
    work_items: { required_before_phase: { "product-change": "implement" } },
  });
  assert.match(notices[0], /Saved AHEAD project configuration/);
});
