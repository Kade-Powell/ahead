import { execFileSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { mkdir, readFile, readdir, rename, rm, unlink, writeFile } from "node:fs/promises";
import { dirname, join, relative, resolve } from "node:path";
import type { Actor, Run } from "./types.js";

interface CurrentRunPointer {
  api_version: "ahead.current/v0";
  run_id: string;
}

export class RunStore {
  readonly aheadDirectory: string;
  readonly projectRoot: string;

  constructor(rootPath: string) {
    this.projectRoot = rootPath;
    this.aheadDirectory = join(rootPath, ".ahead");
  }

  newRunId(): string {
    const date = new Date().toISOString().slice(0, 10).replaceAll("-", "");
    return `${date}-${randomUUID().slice(0, 8)}`;
  }

  async loadCurrent(): Promise<Run | undefined> {
    try {
      const pointer = parseCurrentRunPointer(
        await readFile(join(this.aheadDirectory, "current.json"), "utf8"),
      );
      return this.load(pointer.run_id);
    } catch (error) {
      if (isMissing(error)) {
        return undefined;
      }
      throw error;
    }
  }

  async load(runId: string): Promise<Run> {
    return parseRun(await readFile(this.runPath(runId), "utf8"));
  }

  async save(run: Run, makeCurrent = true): Promise<void> {
    await atomicJson(this.runPath(run.id), run);
    if (makeCurrent) {
      const pointer: CurrentRunPointer = { api_version: "ahead.current/v0", run_id: run.id };
      await atomicJson(join(this.aheadDirectory, "current.json"), pointer);
    }
  }

  async listRunIds(): Promise<string[]> {
    try {
      const entries = await readdir(join(this.aheadDirectory, "runs"), { withFileTypes: true });
      return entries
        .filter((entry) => entry.isDirectory() && isSafeRunId(entry.name))
        .map((entry) => entry.name)
        .toSorted((left, right) => right.localeCompare(left));
    } catch (error) {
      if (isMissing(error)) {
        return [];
      }
      throw error;
    }
  }

  async saveCurrentForResume(runId: string): Promise<void> {
    await this.load(runId);
    await this.clearCurrent(runId);
  }

  async resume(runId: string): Promise<Run> {
    const run = await this.load(runId);
    const pointer: CurrentRunPointer = { api_version: "ahead.current/v0", run_id: runId };
    await atomicJson(join(this.aheadDirectory, "current.json"), pointer);
    return run;
  }

  async discardCurrent(runId: string): Promise<void> {
    const directory = this.runDirectory(runId);
    await this.clearCurrent(runId);
    await rm(directory, { recursive: true, force: false });
  }

  artifactPath(run: Run, phase: string, kind: string): { absolute: string; relative: string } {
    const sequence = String(run.events.length + 1).padStart(4, "0");
    const absolute = join(
      this.aheadDirectory,
      "runs",
      run.id,
      "artifacts",
      `${sequence}-${phase}-${kind}.md`,
    );
    return { absolute, relative: relative(this.projectRoot, absolute) };
  }

  async writeArtifact(path: string, content: string): Promise<void> {
    const resolved = resolve(path);
    const artifactsRoot = resolve(this.aheadDirectory, "runs");
    if (!resolved.startsWith(`${artifactsRoot}/`)) {
      throw new Error("artifact path escaped .ahead/runs");
    }
    await mkdir(dirname(resolved), { recursive: true });
    await writeFile(resolved, `${content.trim()}\n`, { encoding: "utf8", flag: "wx" });
  }

  async readArtifact(path: string): Promise<string> {
    const resolved = resolve(this.projectRoot, path);
    const artifactsRoot = resolve(this.aheadDirectory, "runs");
    if (!resolved.startsWith(`${artifactsRoot}/`)) {
      throw new Error("artifact path escaped .ahead/runs");
    }
    return readFile(resolved, "utf8");
  }

  private runPath(runId: string): string {
    return join(this.runDirectory(runId), "run.json");
  }

  private runDirectory(runId: string): string {
    if (!isSafeRunId(runId)) {
      throw new Error("unsafe AHEAD run id");
    }
    return join(this.aheadDirectory, "runs", runId);
  }

  private async clearCurrent(expectedRunId: string): Promise<void> {
    const path = join(this.aheadDirectory, "current.json");
    let pointer: CurrentRunPointer;
    try {
      pointer = parseCurrentRunPointer(await readFile(path, "utf8"));
    } catch (error) {
      if (isMissing(error)) {
        return;
      }
      throw error;
    }
    if (pointer.run_id !== expectedRunId) {
      throw new Error(
        `active AHEAD run changed from ${expectedRunId} to ${pointer.run_id}; stop or resume again`,
      );
    }
    await unlink(path);
  }
}

export function humanActor(cwd: string): Actor {
  const explicit = process.env.AHEAD_HUMAN_IDENTITY?.trim();
  if (explicit) {
    return { kind: "human", identity: explicit };
  }
  for (const key of ["user.email", "user.name"]) {
    try {
      const value = execFileSync("git", ["config", key], {
        cwd,
        encoding: "utf8",
        stdio: ["ignore", "pipe", "ignore"],
      }).trim();
      if (value) {
        return { kind: "human", identity: value };
      }
    } catch {
      // Fall through to the next local identity source.
    }
  }
  return { kind: "human", identity: process.env.USER?.trim() || "local-human" };
}

export function projectRoot(cwd: string): string {
  try {
    const root = execFileSync("git", ["rev-parse", "--show-toplevel"], {
      cwd,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
    if (root) {
      return root;
    }
  } catch {
    // AHEAD can also persist beside work that is not yet in Git.
  }
  return resolve(cwd);
}

async function atomicJson(path: string, value: unknown): Promise<void> {
  await mkdir(dirname(path), { recursive: true });
  const temporary = `${path}.${process.pid}.${randomUUID()}.tmp`;
  await writeFile(temporary, `${JSON.stringify(value, null, 2)}\n`, "utf8");
  await rename(temporary, path);
}

function isMissing(error: unknown): boolean {
  return !!error && typeof error === "object" && "code" in error && error.code === "ENOENT";
}

function isSafeRunId(runId: string): boolean {
  return runId !== "." && runId !== ".." && /^[A-Za-z0-9._-]+$/.test(runId);
}

function parseCurrentRunPointer(content: string): CurrentRunPointer {
  const value: unknown = JSON.parse(content);
  if (
    !isRecord(value) ||
    value.api_version !== "ahead.current/v0" ||
    typeof value.run_id !== "string" ||
    value.run_id.length === 0
  ) {
    throw new Error("invalid .ahead/current.json");
  }
  return { api_version: value.api_version, run_id: value.run_id };
}

function parseRun(content: string): Run {
  const value: unknown = JSON.parse(content);
  if (!isRun(value)) {
    throw new Error("invalid AHEAD run record");
  }
  return value;
}

function isRun(value: unknown): value is Run {
  return (
    isRecord(value) &&
    typeof value.api_version === "string" &&
    typeof value.id === "string" &&
    typeof value.title === "string" &&
    typeof value.workflow_id === "string" &&
    typeof value.workflow_version === "string" &&
    typeof value.owner === "string" &&
    Array.isArray(value.events)
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
