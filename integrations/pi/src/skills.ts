import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const catalogPath = fileURLToPath(new URL("../generated/recommended-skills.json", import.meta.url));

export interface RecommendedSkill {
  id: string;
  title: string;
  summary: string;
  source: string;
  skill: string;
  reviewed_ref: string;
  reviewed_url: string;
  license: string;
  install: string;
  workflows: string[];
  phases: string[];
  compatibility: string[];
}

export interface RecommendedSkillCatalog {
  api_version: "ahead.recommended-skills/v0.1";
  reviewed_at: string;
  skills: RecommendedSkill[];
}

let cachedCatalog: RecommendedSkillCatalog | undefined;

export async function loadRecommendedSkills(): Promise<RecommendedSkillCatalog> {
  cachedCatalog ??= parseCatalog(await readFile(catalogPath, "utf8"));
  return cachedCatalog;
}

export function relevantRecommendedSkills(
  catalog: RecommendedSkillCatalog,
  workflowId?: string,
  phaseId?: string,
): RecommendedSkill[] {
  if (!workflowId || !phaseId) {
    return [];
  }
  return catalog.skills.filter(
    (skill) => skill.workflows.includes(workflowId) && skill.phases.includes(phaseId),
  );
}

export function recommendedSkillsMarkdown(
  catalog: RecommendedSkillCatalog,
  skills: RecommendedSkill[] = catalog.skills,
): string {
  const entries = skills.map(
    (skill) => `## ${skill.title}

${skill.summary}

- Reviewed source: ${skill.reviewed_url}
- Reviewed revision: \`${skill.reviewed_ref}\`
- License: ${skill.license}
- Applicable workflows: ${skill.workflows.join(", ")}
- Applicable phases: ${skill.phases.join(", ")}

Compatibility with AHEAD:

${skill.compatibility.map((constraint) => `- ${constraint}`).join("\n")}

Optional install after you inspect the source:

\`\`\`sh
${skill.install}
\`\`\``,
  );
  return `# AHEAD recommended skills

Catalog reviewed ${catalog.reviewed_at}. AHEAD does not bundle or install these skills. The active AHEAD workflow remains authoritative.

${entries.length ? entries.join("\n\n") : "No recommended skill applies to this phase."}`;
}

function parseCatalog(content: string): RecommendedSkillCatalog {
  const value: unknown = JSON.parse(content);
  if (!isRecord(value) || value.api_version !== "ahead.recommended-skills/v0.1") {
    throw new Error("invalid AHEAD recommended-skill catalog");
  }
  if (typeof value.reviewed_at !== "string" || !Array.isArray(value.skills)) {
    throw new Error("invalid AHEAD recommended-skill catalog metadata");
  }
  const skills = value.skills.map(parseSkill);
  return { api_version: value.api_version, reviewed_at: value.reviewed_at, skills };
}

function parseSkill(value: unknown): RecommendedSkill {
  if (!isRecord(value)) {
    throw new Error("invalid AHEAD recommended-skill entry");
  }
  return {
    id: requiredString(value, "id"),
    title: requiredString(value, "title"),
    summary: requiredString(value, "summary"),
    source: requiredString(value, "source"),
    skill: requiredString(value, "skill"),
    reviewed_ref: requiredString(value, "reviewed_ref"),
    reviewed_url: requiredString(value, "reviewed_url"),
    license: requiredString(value, "license"),
    install: requiredString(value, "install"),
    workflows: requiredStringArray(value, "workflows"),
    phases: requiredStringArray(value, "phases"),
    compatibility: requiredStringArray(value, "compatibility"),
  };
}

function requiredString(value: Record<string, unknown>, key: string): string {
  const field = value[key];
  if (typeof field !== "string" || field.length === 0) {
    throw new Error(`invalid recommended-skill ${key}`);
  }
  return field;
}

function requiredStringArray(value: Record<string, unknown>, key: string): string[] {
  const field = value[key];
  if (!isStringArray(field)) {
    throw new Error(`invalid recommended-skill ${key}`);
  }
  return field;
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === "string");
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
