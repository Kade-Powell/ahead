import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  findReference,
  loadReferenceIndex,
  readReference,
  relevantReferences,
} from "../src/reference.ts";

const piRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const repositoryRoot = dirname(dirname(piRoot));

test("every canonical framework Markdown source is copied exactly into the package", async () => {
  const index = await loadReferenceIndex();
  assert.ok(index.generated_from.includes("CONSTITUTION.md"));
  assert.ok(index.generated_from.includes("docs/acceptable-ai-use.md"));
  assert.ok(index.generated_from.includes("docs/references/pragmatic-programmer-page-index.md"));
  assert.ok(index.generated_from.includes("docs/workflows/product-change.md"));

  for (const path of index.generated_from) {
    const source = await readFile(join(repositoryRoot, path), "utf8");
    const packaged = await readFile(join(piRoot, "generated", "reference", path), "utf8");
    assert.equal(packaged, source, `${path} must not drift while being packaged`);
  }
});

test("active phases recommend a small applicable set while retaining the full catalog", async () => {
  const index = await loadReferenceIndex();
  const implement = await relevantReferences("product-change", "implement");
  assert.ok(implement.length < index.references.length);
  assert.ok(implement.some((entry) => entry.id === "constitution"));
  assert.ok(implement.some((entry) => entry.id === "acceptable-ai-use"));
  assert.ok(implement.some((entry) => entry.id === "workflows:product-change"));
  assert.ok(!implement.some((entry) => entry.id === "releasing-pi"));
  assert.ok(!implement.some((entry) => entry.id === "workflows:operational-stabilization"));

  const operations = await relevantReferences("operational-stabilization", "respond");
  assert.ok(operations.some((entry) => entry.id === "workflows:operational-stabilization"));
  assert.ok(!operations.some((entry) => entry.id === "workflows:product-change"));
});

test("references resolve by human-friendly topic and retain the binding source text", async () => {
  const entry = await findReference("acceptable AI use");
  assert.equal(entry?.id, "acceptable-ai-use");
  const content = await readReference(entry);
  assert.match(content, /Human supplies intent, context, and an initial model/);
  assert.match(content, /Submitting work the engineer does not understand/);
});

test("generated implementation profile encodes coaching and on-demand reference behavior", async () => {
  const content = await readFile(
    join(piRoot, "generated", "product-change", "implement.md"),
    "utf8",
  );
  assert.match(content, /The engineer implements and makes the first attempt/);
  assert.match(content, /do not turn a request for help into autonomous implementation/);
  assert.match(content, /Use `ahead_get_reference`/);
});

test("generated profiles contain only methods mapped to their phase", async () => {
  const debugging = await readFile(
    join(piRoot, "generated", "corrective-debugging", "investigate.md"),
    "utf8",
  );
  assert.match(debugging, /### Corrective debugging/);
  assert.match(debugging, /### Guided questioning/);
  assert.match(debugging, /### Research and evidence/);

  const publish = await readFile(join(piRoot, "generated", "decision", "publish.md"), "utf8");
  assert.doesNotMatch(publish, /## Applicable AHEAD methods/);
});

test("every executable workflow has a generated instruction manifest", async () => {
  for (const workflow of [
    "product-change",
    "corrective-debugging",
    "operational-stabilization",
    "decision",
    "investigation",
    "internal-improvement",
  ]) {
    const manifest = JSON.parse(
      await readFile(join(piRoot, "generated", workflow, "manifest.json"), "utf8"),
    );
    assert.equal(manifest.workflow, workflow);
    assert.ok(manifest.generated.length > 0);
  }
});
