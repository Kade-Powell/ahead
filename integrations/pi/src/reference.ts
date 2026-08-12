import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const referenceDirectory = fileURLToPath(new URL("../generated/reference/", import.meta.url));

export interface ReferenceEntry {
  id: string;
  path: string;
  title: string;
  summary: string;
  phases: string[];
}

interface ReferenceIndex {
  generated_from: string[];
  references: ReferenceEntry[];
}

let indexPromise: Promise<ReferenceIndex> | undefined;

export async function loadReferenceIndex(): Promise<ReferenceIndex> {
  indexPromise ??= readFile(`${referenceDirectory}index.json`, "utf8")
    .then((content) => JSON.parse(content) as ReferenceIndex);
  return indexPromise;
}

export async function relevantReferences(phaseId?: string): Promise<ReferenceEntry[]> {
  const { references } = await loadReferenceIndex();
  if (!phaseId) return references.filter((entry) => entry.phases.includes("*"));
  return references.filter((entry) => entry.phases.includes("*") || entry.phases.includes(phaseId));
}

export async function findReference(topic: string): Promise<ReferenceEntry | undefined> {
  const normalized = normalize(topic);
  const { references } = await loadReferenceIndex();
  return references.find((entry) =>
    normalize(entry.id) === normalized
    || normalize(entry.path) === normalized
    || normalize(entry.title) === normalized
  ) ?? references.find((entry) =>
    normalize(entry.id).includes(normalized)
    || normalize(entry.path).includes(normalized)
    || normalize(entry.title).includes(normalized)
  );
}

export async function readReference(entry: ReferenceEntry): Promise<string> {
  if (entry.path.includes("..") || entry.path.startsWith("/")) {
    throw new Error(`Invalid packaged AHEAD reference path: ${entry.path}`);
  }
  return readFile(`${referenceDirectory}${entry.path}`, "utf8");
}

function normalize(value: string): string {
  return value.trim().toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
}
