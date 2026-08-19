import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile, readdir } from "node:fs/promises";
import { join, relative } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const piRoot = fileURLToPath(new URL("..", import.meta.url));
const repositoryRoot = join(piRoot, "..", "..");
const sourceRoot = join(repositoryRoot, "skills");
const generatedRoot = join(piRoot, "generated", "skills");

const skillIds = ["research", "to-tickets", "diagnosing-bugs"];

test("AHEAD-owned skills are copied unchanged into the Pi package", async () => {
  for (const skillId of skillIds) {
    for (const name of ["SKILL.md", "LICENSE.ahead", "LICENSE.mattpocock"]) {
      const source = await readFile(join(sourceRoot, skillId, name), "utf8");
      const generated = await readFile(join(generatedRoot, skillId, name), "utf8");
      assert.equal(generated, source, `${skillId}/${name} drifted during packaging`);
    }
  }

  const packageJson = JSON.parse(await readFile(join(piRoot, "package.json"), "utf8"));
  assert.deepEqual(packageJson.pi.skills, ["./generated/skills"]);
  assert.ok(
    !(await readdir(generatedRoot)).some((name) => name.endsWith(".md")),
    "top-level Markdown would be mis-discovered as another package skill",
  );
});

test("bundled skill provenance pins the reviewed source and adapted hashes", async () => {
  const manifest = JSON.parse(await readFile(join(sourceRoot, "manifest.json"), "utf8"));
  assert.equal(manifest.api_version, "ahead.bundled-skills/v0.1");
  assert.match(manifest.source_revision, /^[a-f0-9]{40}$/);
  assert.equal(manifest.source_license, "MIT");
  assert.deepEqual(
    manifest.skills.map((skill) => skill.id),
    skillIds,
  );

  for (const skill of manifest.skills) {
    assert.equal(skill.adapted_hash, await adaptedHash(join(sourceRoot, skill.id)));
  }
});

test("diagnosing-bugs preserves AHEAD human-model-first ordering", async () => {
  const content = await readFile(join(sourceRoot, "diagnosing-bugs", "SKILL.md"), "utf8");
  const humanModel = content.indexOf("human characterizes observed versus expected behavior");
  const feedbackLoop = content.indexOf("## 1. Build the tightest safe feedback loop");
  assert.ok(humanModel >= 0);
  assert.ok(feedbackLoop > humanModel);
  assert.match(content, /human selects which hypotheses to test/i);
  assert.match(content, /human selects\s+the\s+correction/i);
  assert.match(content, /owns the first implementation/i);
  assert.match(content, /No safe reproduction path/);
});

test("to-tickets requires human plan and publication approval", async () => {
  const content = await readFile(join(sourceRoot, "to-tickets", "SKILL.md"), "utf8");
  assert.match(content, /human defines or affirms[\s\S]*first-pass plan/i);
  assert.match(content, /wait for explicit human approval before[\s\S]*tracker records/i);
  assert.match(content, /(?:does not|never) authorize implementation/i);
  assert.match(content, /disable-model-invocation: true/);
});

async function adaptedHash(directory) {
  const paths = (await collectFiles(directory)).toSorted((left, right) =>
    left.localeCompare(right),
  );
  const lines = [];
  for (const path of paths) {
    const content = await readFile(path);
    const digest = createHash("sha256").update(content).digest("hex");
    lines.push(`${digest}  ./${relative(directory, path)}\n`);
  }
  return createHash("sha256").update(lines.join("")).digest("hex");
}

async function collectFiles(directory) {
  const paths = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      paths.push(...(await collectFiles(path)));
    } else if (entry.isFile()) {
      paths.push(path);
    }
  }
  return paths;
}
