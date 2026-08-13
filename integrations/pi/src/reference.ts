import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const referenceDirectory = fileURLToPath(new URL("../generated/reference/", import.meta.url));

export interface ReferenceEntry {
  id: string;
  path: string;
  title: string;
  summary: string;
  audience: "practitioner" | "evidence";
  authority: "binding" | "guidance" | "supporting";
  distribution: "agent-and-human";
  phases: string[];
  workflows: string[];
}

interface ReferenceIndex {
  api_version: "ahead.references/v0.1";
  generated_from: string[];
  references: ReferenceEntry[];
}

let indexPromise: Promise<ReferenceIndex> | undefined;

export async function loadReferenceIndex(): Promise<ReferenceIndex> {
  indexPromise ??= readFile(`${referenceDirectory}index.json`, "utf8").then(parseReferenceIndex);
  return indexPromise;
}

export async function relevantReferences(
  workflowId?: string,
  phaseId?: string,
): Promise<ReferenceEntry[]> {
  const { references } = await loadReferenceIndex();
  return references.filter(
    (entry) =>
      (entry.workflows.includes("*") || (!!workflowId && entry.workflows.includes(workflowId))) &&
      (entry.phases.includes("*") || (!!phaseId && entry.phases.includes(phaseId))),
  );
}

export async function findReference(topic: string): Promise<ReferenceEntry | undefined> {
  const normalized = normalize(topic);
  const { references } = await loadReferenceIndex();
  return (
    references.find(
      (entry) =>
        normalize(entry.id) === normalized ||
        normalize(entry.path) === normalized ||
        normalize(entry.title) === normalized,
    ) ??
    references.find(
      (entry) =>
        normalize(entry.id).includes(normalized) ||
        normalize(entry.path).includes(normalized) ||
        normalize(entry.title).includes(normalized),
    )
  );
}

export async function readReference(entry: ReferenceEntry): Promise<string> {
  if (entry.path.includes("..") || entry.path.startsWith("/")) {
    throw new Error(`Invalid packaged AHEAD reference path: ${entry.path}`);
  }
  return readFile(`${referenceDirectory}${entry.path}`, "utf8");
}

function normalize(value: string): string {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
}

function parseReferenceIndex(content: string): ReferenceIndex {
  const value: unknown = JSON.parse(content);
  if (!isRecord(value)) {
    throw new Error("Invalid packaged AHEAD reference index");
  }
  const apiVersion = value.api_version;
  const generatedFrom = value.generated_from;
  const references = value.references;
  if (
    apiVersion !== "ahead.references/v0.1" ||
    !Array.isArray(generatedFrom) ||
    !generatedFrom.every((entry) => typeof entry === "string") ||
    !Array.isArray(references) ||
    !references.every(isReferenceEntry)
  ) {
    throw new Error("Invalid packaged AHEAD reference index");
  }
  return { api_version: apiVersion, generated_from: generatedFrom, references };
}

function isReferenceEntry(value: unknown): value is ReferenceEntry {
  return (
    isRecord(value) &&
    typeof value.id === "string" &&
    typeof value.path === "string" &&
    typeof value.title === "string" &&
    typeof value.summary === "string" &&
    (value.audience === "practitioner" || value.audience === "evidence") &&
    (value.authority === "binding" ||
      value.authority === "guidance" ||
      value.authority === "supporting") &&
    value.distribution === "agent-and-human" &&
    Array.isArray(value.phases) &&
    value.phases.every((phase) => typeof phase === "string") &&
    Array.isArray(value.workflows) &&
    value.workflows.every((workflow) => typeof workflow === "string")
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
