import { createHash } from "node:crypto";
import { mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const specDirectory = join(root, "spec", "workflows");
const commonPath = join(root, "policy", "common.md");
const methodsIndexPath = join(root, "policy", "methods", "index.json");
const recommendedSkillsPath = join(root, "recommendations", "skills-v0.1.json");
const generatedDirectory = join(root, "integrations", "pi", "generated");
const referenceOutputDir = join(root, "integrations", "pi", "generated", "reference");

const common = (await readFile(commonPath, "utf8")).trim();
const methodsIndex = JSON.parse(await readFile(methodsIndexPath, "utf8"));
const methods = await loadMethods(methodsIndex);
await rm(generatedDirectory, { recursive: true, force: true });
await mkdir(generatedDirectory, { recursive: true });

const specPaths = (await readdir(specDirectory))
  .filter((name) => name.endsWith(".json"))
  .toSorted()
  .map((name) => join(specDirectory, name));
const specs = await Promise.all(
  specPaths.map(async (specPath) => {
    const specText = await readFile(specPath, "utf8");
    return { specPath, specText, spec: JSON.parse(specText) };
  }),
);
validateMethodApplications(
  methods,
  specs.map(({ spec }) => spec),
);
let generatedPhaseCount = 0;
for (const { specPath, specText, spec } of specs) {
  const outputDir = join(generatedDirectory, spec.id);
  await mkdir(outputDir, { recursive: true });

  for (const phase of spec.phases) {
    const fragment = await readPhaseFragment(spec.id, phase.id);
    const applicableMethods = methods.filter((method) =>
      method.applies.some(
        (application) => application.workflow === spec.id && application.phases.includes(phase.id),
      ),
    );
    const methodInstructions = applicableMethods
      .map((method) => `### ${method.title}\n\n${method.content}`)
      .join("\n\n");
    const digest = createHash("sha256")
      .update(specText)
      .update("\0")
      .update(common)
      .update("\0")
      .update(fragment)
      .update("\0")
      .update(methodInstructions)
      .digest("hex");
    const artifacts = phase.artifacts
      .map((artifact) => {
        const status = artifact.required ? "required" : "optional";
        const identity = artifact.independent_of
          ? `; actor identity must differ from latest ${artifact.independent_of}`
          : artifact.same_as
            ? `; actor identity must match latest ${artifact.same_as}`
            : "";
        return `- \`${artifact.kind}\`: ${artifact.title} (${status}; actor: ${artifact.actor}${identity})`;
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

${methodInstructions ? `## Applicable AHEAD methods\n\n${methodInstructions}\n` : ""}

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
    generatedPhaseCount += 1;
  }

  await writeFile(
    join(outputDir, "manifest.json"),
    `${JSON.stringify(
      {
        workflow: spec.id,
        version: spec.version,
        generated: spec.phases.map((phase) => `${phase.id}.md`),
        sources: [
          relative(root, specPath),
          relative(root, commonPath),
          `policy/${spec.id}/<phase>.md or policy/shared/<phase>.md`,
          relative(root, methodsIndexPath),
          ...applicableMethodSources(spec.id, spec.phases, methods),
        ],
      },
      null,
      2,
    )}\n`,
    "utf8",
  );
}

await writeFile(
  join(generatedDirectory, "recommended-skills.json"),
  await readFile(recommendedSkillsPath, "utf8"),
  "utf8",
);

const referencePaths = [
  "CONSTITUTION.md",
  ...(await collectMarkdown(join(root, "docs", "guide"))).map((path) => relative(root, path)),
  ...(await collectMarkdown(join(root, "docs", "evidence"))).map((path) => relative(root, path)),
].toSorted();
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
    ...referenceMetadata(relativePath),
    phases: applicablePhases(relativePath),
    workflows: applicableWorkflows(relativePath),
  });
}
await writeFile(
  join(referenceOutputDir, "index.json"),
  `${JSON.stringify(
    {
      api_version: "ahead.references/v0.1",
      generated_from: referencePaths,
      references: referenceEntries,
    },
    null,
    2,
  )}\n`,
  "utf8",
);

console.log(
  `Generated ${generatedPhaseCount} AHEAD Pi instruction bundles for ${specPaths.length} workflows and ${referenceEntries.length} on-demand references`,
);

async function readPhaseFragment(workflowId, phaseId) {
  const workflowPath = join(root, "policy", workflowId, `${phaseId}.md`);
  try {
    return (await readFile(workflowPath, "utf8")).trim();
  } catch (error) {
    if (!isNodeError(error) || error.code !== "ENOENT") {
      throw error;
    }
  }

  const sharedPath = join(root, "policy", "shared", `${phaseId}.md`);
  return (await readFile(sharedPath, "utf8")).trim();
}

async function loadMethods(index) {
  if (index.api_version !== "ahead.methods/v0.1" || !Array.isArray(index.methods)) {
    throw new Error("invalid AHEAD method index");
  }
  const loaded = [];
  for (const method of index.methods) {
    const path = join(root, method.path);
    loaded.push({ ...method, content: (await readFile(path, "utf8")).trim() });
  }
  return loaded;
}

function applicableMethodSources(workflowId, phases, availableMethods) {
  const phaseIds = new Set(phases.map((phase) => phase.id));
  return availableMethods
    .filter((method) =>
      method.applies.some(
        (application) =>
          application.workflow === workflowId &&
          application.phases.some((phase) => phaseIds.has(phase)),
      ),
    )
    .map((method) => method.path);
}

function validateMethodApplications(availableMethods, workflowSpecs) {
  const phasesByWorkflow = new Map(
    workflowSpecs.map((spec) => [spec.id, new Set(spec.phases.map((phase) => phase.id))]),
  );
  for (const method of availableMethods) {
    for (const application of method.applies) {
      const phaseIds = phasesByWorkflow.get(application.workflow);
      if (!phaseIds) {
        throw new Error(`${method.id} references unknown workflow ${application.workflow}`);
      }
      for (const phase of application.phases) {
        if (!phaseIds.has(phase)) {
          throw new Error(`${method.id} references unknown phase ${application.workflow}:${phase}`);
        }
      }
    }
  }
}

function isNodeError(error) {
  return error instanceof Error && "code" in error;
}

async function collectMarkdown(directory) {
  const paths = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      paths.push(...(await collectMarkdown(path)));
    } else if (entry.isFile() && entry.name.endsWith(".md")) {
      paths.push(path);
    }
  }
  return paths;
}

function referenceId(relativePath) {
  const indexIds = new Map([
    ["docs/guide/README.md", "guide"],
    ["docs/evidence/README.md", "evidence"],
    ["docs/guide/workflows/README.md", "workflows"],
  ]);
  const indexId = indexIds.get(relativePath);
  if (indexId) {
    return indexId;
  }
  return relativePath
    .replace(/\.md$/, "")
    .replace(/^docs\/guide\//, "")
    .replace(/^docs\//, "")
    .replaceAll("/", ":")
    .toLowerCase();
}

function referenceMetadata(relativePath) {
  if (
    relativePath === "CONSTITUTION.md" ||
    relativePath === "docs/guide/acceptable-ai-use.md" ||
    /^docs\/guide\/workflows\/[^/]+\.md$/.test(relativePath)
  ) {
    return {
      audience: "practitioner",
      authority: "binding",
      distribution: "agent-and-human",
    };
  }
  if (relativePath.startsWith("docs/guide/")) {
    return {
      audience: "practitioner",
      authority: "guidance",
      distribution: "agent-and-human",
    };
  }
  if (relativePath.startsWith("docs/evidence/")) {
    return {
      audience: "evidence",
      authority: "supporting",
      distribution: "agent-and-human",
    };
  }
  throw new Error(`runtime reference has no audience classification: ${relativePath}`);
}

function firstSummaryParagraph(content) {
  return (
    content
      .split(/\n\s*\n/)
      .map((paragraph) => paragraph.replace(/^#+\s+.*$/gm, "").trim())
      .find(
        (paragraph) =>
          paragraph &&
          !paragraph.startsWith("Audience:") &&
          !paragraph.startsWith("Status:") &&
          !paragraph.startsWith("```"),
      )
      ?.replace(/\s+/g, " ")
      .slice(0, 240) ?? "AHEAD framework reference."
  );
}

function applicablePhases(relativePath) {
  if (/^docs\/guide\/workflows\/[^/]+\.md$/.test(relativePath)) {
    return ["*"];
  }
  if (
    [
      "CONSTITUTION.md",
      "docs/guide/README.md",
      "docs/guide/rationale.md",
      "docs/guide/acceptable-ai-use.md",
      "docs/guide/engineering-practice.md",
    ].includes(relativePath)
  ) {
    return ["*"];
  }

  if (relativePath === "docs/evidence/evidence-standard.md") {
    return [
      "research",
      "questions",
      "decision",
      "plan",
      "ai-review",
      "human-review",
      "verify",
      "ai-audit",
      "outcome",
    ];
  }
  return [];
}

function applicableWorkflows(relativePath) {
  const match = relativePath.match(/^docs\/guide\/workflows\/([^/]+)\.md$/);
  if (match && match[1] !== "README") {
    return [match[1]];
  }
  return ["*"];
}
