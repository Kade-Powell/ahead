import assert from "node:assert/strict";
import { access, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
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
