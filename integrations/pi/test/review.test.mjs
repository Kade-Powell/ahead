import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import {
  collectReviewSnapshot,
  extractFindingIds,
  extractReviewFingerprint,
  reviewDispositionTemplate,
  validateAiReviewArtifact,
  validateReviewDisposition,
} from "../src/review.ts";

const fingerprint = "a".repeat(64);
const snapshot = {
  api_version: "ahead.review-snapshot/v0.1",
  base_ref: "origin/main",
  base_oid: "1".repeat(40),
  merge_base_oid: "2".repeat(40),
  head_oid: "3".repeat(40),
  captured_at: "2026-08-12T12:00:00Z",
  fingerprint,
  status: " M src/index.ts",
  changed_files: ["src/index.ts"],
  untracked_files: [],
  diff: "diff --git a/src/index.ts b/src/index.ts",
};

test("review findings have stable identifiers and a snapshot-bound human template", () => {
  const findings = extractFindingIds("## AR-002\nissue\n## AR-001\nissue\nAR-002 repeated");
  assert.deepEqual(findings, ["AR-001", "AR-002"]);
  const template = reviewDispositionTemplate(snapshot, findings);
  assert.equal(extractReviewFingerprint(template), fingerprint);
  assert.match(template, /fixed \| invalid \| accepted-risk \| follow-up/);
  assert.match(template, /## AR-001/);
});

test("AI review artifacts require structured findings and assessment boundaries", () => {
  const content = `# AI review

AHEAD-Review-Snapshot: ${fingerprint}

## AR-001
- Severity: high
- Category: correctness
- Location: src/index.ts:42
- Evidence: the branch returns before validation
- Impact: invalid state can advance
- Explanation: moving validation before the return should make the test pass

## Areas not assessed

- Deployment configuration was not assessed.`;
  assert.deepEqual(validateAiReviewArtifact(content, fingerprint), []);
  assert.deepEqual(
    validateAiReviewArtifact(content.replace(/^- Evidence:.*$/m, "- Evidence:"), fingerprint),
    ["AR-001 needs a nonempty evidence field"],
  );
});

test("AI review cannot silently convert unstructured findings into no findings", () => {
  const content = `AHEAD-Review-Snapshot: ${fingerprint}

- High: this looks broken

## Areas not assessed

- None.`;
  assert.deepEqual(validateAiReviewArtifact(content, fingerprint), [
    "AI review must contain structured AR findings or a supported no-findings statement",
  ]);
});

test("every material AI finding requires a valid human disposition", () => {
  const content = `AHEAD-Review-Snapshot: ${fingerprint}

## AR-001
- Disposition: fixed
- Rationale and evidence: regression test now passes

## AR-002
- Disposition: invalid
- Rationale and evidence: cited path is unreachable`;
  assert.deepEqual(validateReviewDisposition(content, fingerprint, ["AR-001", "AR-002"]), []);
  assert.deepEqual(validateReviewDisposition(content, "b".repeat(64), ["AR-001", "AR-003"]), [
    "the disposition is not bound to the current AI review snapshot",
    "AR-003 needs fixed, invalid, accepted-risk, or follow-up with rationale",
  ]);
});

test("blank rationale and unconfirmed no-findings records cannot pass", () => {
  const blank = `AHEAD-Review-Snapshot: ${fingerprint}

## AR-001
- Disposition: fixed
- Rationale and evidence:
- Resulting change or follow-up:`;
  assert.deepEqual(validateReviewDisposition(blank, fingerprint, ["AR-001"]), [
    "AR-001 needs fixed, invalid, accepted-risk, or follow-up with rationale",
  ]);
  assert.deepEqual(
    validateReviewDisposition(
      `AHEAD-Review-Snapshot: ${fingerprint}\n\n## No material findings\n- Confirmation:`,
      fingerprint,
      [],
    ),
    ["confirm that the AI review reported no material findings"],
  );
});

test("review snapshots support unborn repositories and exclude AHEAD records", async () => {
  const root = await mkdtemp(join(tmpdir(), "ahead-review-"));
  try {
    execFileSync("git", ["init", "--quiet"], { cwd: root });
    await writeFile(join(root, "idea.txt"), "prototype\n", "utf8");
    const unusualPath = "résumé\nidea.txt";
    await writeFile(join(root, unusualPath), "unusual path\n", "utf8");
    await mkdir(join(root, ".ahead"));
    await writeFile(join(root, ".ahead", "current.json"), "{}\n", "utf8");
    await mkdir(join(root, ".ahead", "local"));
    await writeFile(join(root, ".ahead", "local", "current.json"), "{}\n", "utf8");
    const captured = await collectReviewSnapshot(root);
    assert.equal(captured.base_ref, "empty-tree");
    assert.equal(captured.head_oid, "UNBORN");
    assert.ok(captured.changed_files.includes("idea.txt"));
    assert.ok(captured.changed_files.includes(unusualPath));
    assert.ok(!captured.status.includes(".ahead"));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("review snapshots prefer the target branch over a feature tracking branch", async () => {
  const root = await mkdtemp(join(tmpdir(), "ahead-review-base-"));
  try {
    execFileSync("git", ["init", "--quiet", "--initial-branch=main"], { cwd: root });
    execFileSync("git", ["config", "user.email", "review@example.com"], { cwd: root });
    execFileSync("git", ["config", "user.name", "Review Test"], { cwd: root });
    await writeFile(join(root, "base.txt"), "base\n", "utf8");
    execFileSync("git", ["add", "base.txt"], { cwd: root });
    execFileSync("git", ["commit", "--quiet", "-m", "base"], { cwd: root });
    execFileSync("git", ["switch", "--quiet", "-c", "feature"], { cwd: root });
    await writeFile(join(root, "feature.txt"), "feature\n", "utf8");
    execFileSync("git", ["add", "feature.txt"], { cwd: root });
    execFileSync("git", ["commit", "--quiet", "-m", "feature"], { cwd: root });
    execFileSync("git", ["branch", "--set-upstream-to=main", "feature"], { cwd: root });
    const captured = await collectReviewSnapshot(root);
    assert.equal(captured.base_ref, "main");
    assert.ok(captured.changed_files.includes("feature.txt"));
    assert.match(captured.diff, /feature\.txt/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
