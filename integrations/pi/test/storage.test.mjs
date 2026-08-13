import assert from "node:assert/strict";
import { access, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { RunStore } from "../src/storage.ts";

function run(id) {
  return {
    api_version: "ahead.run/v0",
    id,
    title: `Run ${id}`,
    workflow_id: "investigation",
    workflow_version: "0.1.0",
    owner: "human@example.com",
    events: [
      {
        sequence: 1,
        timestamp: "2026-08-13T12:00:00Z",
        actor: { kind: "human", identity: "human@example.com" },
        type: "run_started",
        phase: "frame",
      },
    ],
  };
}

test("saved unfinished work leaves AHEAD mode and resumes the same run", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "ahead-storage-save-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const store = new RunStore(root);
  const saved = run("saved-run");
  await store.save(saved);

  await store.saveCurrentForResume(saved.id);
  assert.equal(await store.loadCurrent(), undefined);
  assert.deepEqual(await store.listRunIds(), [saved.id]);

  await store.resume(saved.id);
  assert.deepEqual(await store.loadCurrent(), saved);
});

test("discarding unfinished AHEAD state leaves repository work untouched", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "ahead-storage-discard-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const store = new RunStore(root);
  const discarded = run("discarded-run");
  const repositoryWork = join(root, "work-in-progress.txt");
  await writeFile(repositoryWork, "keep me\n", "utf8");
  await store.save(discarded);
  const artifact = store.artifactPath(discarded, "frame", "question");
  await store.writeArtifact(artifact.absolute, "unfinished AHEAD record");

  await store.discardCurrent(discarded.id);
  assert.equal(await store.loadCurrent(), undefined);
  await assert.rejects(store.load(discarded.id), { code: "ENOENT" });
  assert.equal(await readFile(repositoryWork, "utf8"), "keep me\n");
});

test("discard refuses to remove a run after the active pointer changes", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "ahead-storage-race-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const store = new RunStore(root);
  const first = run("first-run");
  const second = run("second-run");
  await store.save(first);
  await store.save(second);

  await assert.rejects(store.discardCurrent(".."), /unsafe AHEAD run id/);
  await assert.rejects(store.discardCurrent(first.id), /active AHEAD run changed/);
  await access(join(root, ".ahead", "runs", first.id, "run.json"));
  assert.equal((await store.loadCurrent())?.id, second.id);
});

test("project configuration resolves a workflow-specific work-item boundary", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "ahead-storage-config-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const store = new RunStore(root);

  assert.deepEqual(await store.policyForWorkflow("product-change"), {
    work_items: { required_before_phase: null },
  });

  await mkdir(join(root, ".ahead"), { recursive: true });
  await writeFile(
    join(root, ".ahead", "config.json"),
    JSON.stringify({
      api_version: "ahead.config/v0",
      work_items: {
        required_before_phase: {
          "product-change": "implement",
          investigation: "conclude",
        },
      },
    }),
    { encoding: "utf8", flag: "wx" },
  );

  assert.deepEqual(await store.policyForWorkflow("product-change"), {
    work_items: { required_before_phase: "implement" },
  });
  assert.deepEqual(await store.policyForWorkflow("decision"), {
    work_items: { required_before_phase: null },
  });
});

test("invalid project work-item configuration fails closed", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "ahead-storage-invalid-config-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const store = new RunStore(root);
  await mkdir(join(root, ".ahead"), { recursive: true });
  await writeFile(
    join(root, ".ahead", "config.json"),
    JSON.stringify({
      api_version: "ahead.config/v0",
      work_items: { required_before_phase: { "product-change": "" } },
    }),
    { encoding: "utf8", flag: "wx" },
  );

  await assert.rejects(store.loadProjectConfig(), /every work-item boundary/);
});

test("project configuration inspection distinguishes missing and valid files", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "ahead-storage-inspect-config-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const store = new RunStore(root);
  const config = {
    api_version: "ahead.config/v0",
    work_items: { required_before_phase: { "product-change": "implement" } },
  };

  assert.deepEqual(await store.inspectProjectConfig(), { status: "missing" });
  const saved = await store.saveProjectConfig(config, { overwrite: false });

  assert.equal(saved.path, join(".ahead", "config.json"));
  assert.equal(saved.backup, undefined);
  const inspection = await store.inspectProjectConfig();
  assert.equal(inspection.status, "valid");
  assert.deepEqual(inspection.config, config);
});

test("project configuration replacement requires intent and preserves the prior file", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "ahead-storage-replace-config-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const store = new RunStore(root);
  const original = `${JSON.stringify(
    {
      api_version: "ahead.config/v0",
      work_items: { required_before_phase: { decision: "publish" } },
    },
    null,
    4,
  )}\n`;
  await mkdir(join(root, ".ahead"), { recursive: true });
  await writeFile(join(root, ".ahead", "config.json"), original, "utf8");
  const replacement = {
    api_version: "ahead.config/v0",
    work_items: { required_before_phase: { investigation: "conclude" } },
  };

  await assert.rejects(
    store.saveProjectConfig(replacement, { overwrite: false }),
    /explicit replacement is required/,
  );
  const saved = await store.saveProjectConfig(replacement, { overwrite: true });

  assert.ok(saved.backup);
  assert.equal(await readFile(join(root, saved.backup), "utf8"), original);
  assert.deepEqual(await store.loadProjectConfig(), replacement);
});

test("unsupported project configuration can be recovered without losing its source", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "ahead-storage-migrate-config-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const store = new RunStore(root);
  const unsupported = '{"api_version":"ahead.config/v99","legacy":true}\n';
  await mkdir(join(root, ".ahead"), { recursive: true });
  await writeFile(join(root, ".ahead", "config.json"), unsupported, "utf8");

  const inspection = await store.inspectProjectConfig();
  assert.equal(inspection.status, "invalid");
  assert.match(inspection.error, /expected api_version ahead.config\/v0/);

  const saved = await store.saveProjectConfig(
    {
      api_version: "ahead.config/v0",
      work_items: { required_before_phase: {} },
    },
    { overwrite: true },
  );
  assert.ok(saved.backup);
  assert.equal(await readFile(join(root, saved.backup), "utf8"), unsupported);
  assert.deepEqual(await store.loadProjectConfig(), {
    api_version: "ahead.config/v0",
    work_items: { required_before_phase: {} },
  });
});
