import { createHash } from "node:crypto";
import { mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const specPath = join(root, "spec", "workflows", "product-change-v0.1.json");
const commonPath = join(root, "policy", "common.md");
const outputDir = join(root, "integrations", "pi", "generated", "product-change");
const referenceOutputDir = join(root, "integrations", "pi", "generated", "reference");

const specText = await readFile(specPath, "utf8");
const spec = JSON.parse(specText);
const common = (await readFile(commonPath, "utf8")).trim();
await rm(outputDir, { recursive: true, force: true });
await mkdir(outputDir, { recursive: true });

for (const phase of spec.phases) {
  const fragmentPath = join(root, "policy", "product-change", `${phase.id}.md`);
  const fragment = (await readFile(fragmentPath, "utf8")).trim();
  const digest = createHash("sha256")
    .update(specText)
    .update("\0")
    .update(common)
    .update("\0")
    .update(fragment)
    .digest("hex");
  const artifacts = phase.artifacts
    .map((artifact) => {
      const status = artifact.required ? "required" : "optional";
      const independence = artifact.independent_of
        ? `; actor identity must differ from latest ${artifact.independent_of}`
        : "";
      return `- \`${artifact.kind}\`: ${artifact.title} (${status}; actor: ${artifact.actor}${independence})`;
    })
    .join("\n");
  const unlock = phase.ai_unlock_artifacts.length
    ? phase.ai_unlock_artifacts.map((kind) => `\`${kind}\``).join(", ")
    : "none";
  const capabilities = phase.ai_capabilities.length
    ? phase.ai_capabilities.map((capability) => `\`${capability}\``).join(", ")
    : "none";
  const returns = phase.returns_to.length ? phase.returns_to.join(", ") : "none";
  const next = phase.next ?? "close run";
  const gateActor = phase.gate.accepted_by_artifact
    ? `; acceptance identity must match \`${phase.gate.accepted_by_artifact}\``
    : "";
  const output = `<!-- GENERATED FILE. DO NOT EDIT. -->
<!-- workflow=${spec.id}@${spec.version} phase=${phase.id} sha256=${digest} -->

${common}

# Active phase: ${phase.title}

${fragment}

## Enforced phase contract

- Workflow: \`${spec.id}@${spec.version}\`
- Current phase: \`${phase.id}\`
- Human gate: \`${phase.gate.id}\` — ${phase.gate.title}${gateActor}
- Normal next phase: \`${next}\`
- Human-authorized return targets: ${returns}
- AI unlock artifacts: ${unlock}
- AI capabilities after unlock: ${capabilities}

### Phase artifacts

${artifacts}
`;
  await writeFile(join(outputDir, `${phase.id}.md`), output, "utf8");
}

const manifestPath = join(outputDir, "manifest.json");
await writeFile(
  manifestPath,
  `${JSON.stringify(
    {
      workflow: spec.id,
      version: spec.version,
      generated: spec.phases.map((phase) => `${phase.id}.md`),
      sources: [
        relative(root, specPath),
        relative(root, commonPath),
        "policy/product-change/<phase>.md",
      ],
    },
    null,
    2,
  )}\n`,
  "utf8",
);

const referencePaths = [
  "CONSTITUTION.md",
  ...(await collectMarkdown(join(root, "docs"))).map((path) => relative(root, path)),
].sort();
await rm(referenceOutputDir, { recursive: true, force: true });
const referenceEntries = [];
for (const relativePath of referencePaths) {
  const content = await readFile(join(root, relativePath), "utf8");
  const target = join(referenceOutputDir, relativePath);
  await mkdir(dirname(target), { recursive: true });
  await writeFile(target, content, "utf8");
  referenceEntries.push({
    id: referenceId(relativePath),
    path: relativePath,
    title: content.match(/^#\s+(.+)$/m)?.[1]?.trim() ?? relativePath,
    summary: firstSummaryParagraph(content),
    phases: applicablePhases(relativePath),
  });
}
await writeFile(
  join(referenceOutputDir, "index.json"),
  `${JSON.stringify({ generated_from: referencePaths, references: referenceEntries }, null, 2)}\n`,
  "utf8",
);

console.log(
  `Generated ${spec.phases.length} AHEAD Pi instruction bundles and ${referenceEntries.length} on-demand references`,
);

async function collectMarkdown(directory) {
  const paths = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) paths.push(...await collectMarkdown(path));
    else if (entry.isFile() && entry.name.endsWith(".md")) paths.push(path);
  }
  return paths;
}

function referenceId(relativePath) {
  return relativePath
    .replace(/\.md$/, "")
    .replace(/^docs\//, "")
    .replaceAll("/", ":")
    .toLowerCase();
}

function firstSummaryParagraph(content) {
  return content
    .split(/\n\s*\n/)
    .map((paragraph) => paragraph.replace(/^#+\s+.*$/gm, "").trim())
    .find((paragraph) => paragraph && !paragraph.startsWith("Status:") && !paragraph.startsWith("```"))
    ?.replace(/\s+/g, " ")
    .slice(0, 240) ?? "AHEAD framework reference.";
}

function applicablePhases(relativePath) {
  if ([
    "CONSTITUTION.md",
    "docs/rationale.md",
    "docs/acceptable-ai-use.md",
    "docs/engineering-practice.md",
    "docs/workflows/product-change.md",
  ].includes(relativePath)) return ["*"];

  if (relativePath === "docs/evidence/evidence-standard.md") {
    return ["research", "questions", "decision", "plan", "ai-review", "human-review", "verify", "ai-audit", "outcome"];
  }
  if (relativePath === "docs/design/executable-workflows.md") return ["*"];
  return [];
}
