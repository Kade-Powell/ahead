import { createHash } from "node:crypto";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const specPath = join(root, "spec", "workflows", "product-change-v0.1.json");
const commonPath = join(root, "policy", "common.md");
const outputDir = join(root, "integrations", "pi", "generated", "product-change");

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
        specPath.slice(root.length + 1),
        commonPath.slice(root.length + 1),
        "policy/product-change/<phase>.md",
      ],
    },
    null,
    2,
  )}\n`,
  "utf8",
);

console.log(`Generated ${spec.phases.length} AHEAD Pi instruction bundles in ${outputDir}`);
