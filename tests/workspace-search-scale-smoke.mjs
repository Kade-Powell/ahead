import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { performance } from "node:perf_hooks";

const binary = path.resolve(process.argv[2] ?? "target/debug/ahead");
const fileCount = 10_000;
const filesPerDirectory = Number(
  process.env.AHEAD_SEARCH_SCALE_FILES_PER_DIRECTORY ?? 1_000,
);
assert(
  Number.isSafeInteger(filesPerDirectory)
    && filesPerDirectory > 0
    && fileCount % filesPerDirectory === 0,
  `AHEAD_SEARCH_SCALE_FILES_PER_DIRECTORY must be a positive divisor of ${fileCount}`,
);
const directoryCount = fileCount / filesPerDirectory;
const workspace = await mkdtemp(path.join(tmpdir(), "ahead-search-scale-"));
const profile = `${workspace}-profile`;
const flatDirectories = process.env.AHEAD_SEARCH_SCALE_FLAT_DIRECTORIES === "1";
const searchRoot = flatDirectories ? workspace : path.join(workspace, "src");
const home = path.join(profile, "home");
const compareRipgrep = process.env.AHEAD_SEARCH_SCALE_RIPGREP === "1";
const smokeEnv = {
  PATH: process.env.PATH ?? "/usr/bin:/bin:/usr/sbin:/sbin",
  HOME: home,
  TMPDIR: tmpdir(),
  AHEAD_DATA_HOME: path.join(profile, "data"),
  AHEAD_HOME: path.join(profile, "runtime"),
  AHEAD_USER_HOME: path.join(profile, "user"),
};
for (const directory of [
  home,
  smokeEnv.AHEAD_DATA_HOME,
  smokeEnv.AHEAD_HOME,
  smokeEnv.AHEAD_USER_HOME,
]) {
  await mkdir(directory, { recursive: true });
}

let passed = false;
let child;
let lines;
let stderr = "";
let exited = false;
let exitResult;
let nextId = 0;
const pending = new Map();

function ripgrepCount(pattern) {
  const started = performance.now();
  const result = spawnSync("rg", [
    "--count-matches",
    "--fixed-strings",
    pattern,
    searchRoot,
  ], {
    encoding: "utf8",
    env: { PATH: process.env.PATH ?? "/usr/bin:/bin:/usr/sbin:/sbin" },
    maxBuffer: 4 * 1024 * 1024,
  });
  if (result.error) throw result.error;
  if (result.status !== 0 && result.status !== 1) {
    throw new Error(`ripgrep failed (${result.status}): ${result.stderr}`);
  }
  const count = result.stdout.split(/\r?\n/).reduce((total, line) => {
    const separator = line.lastIndexOf(":");
    const fileCount = Number(line.slice(separator + 1));
    return separator >= 0 && Number.isInteger(fileCount) ? total + fileCount : total;
  }, 0);
  return { count, elapsedMs: performance.now() - started };
}

function medianElapsed(samples) {
  const elapsedTimes = samples
    .map(({ elapsedMs }) => elapsedMs)
    .sort((left, right) => left - right);
  return elapsedTimes[Math.floor(elapsedTimes.length / 2)];
}

try {
  for (let directoryIndex = 0; directoryIndex < directoryCount; directoryIndex++) {
    const relativeDirectory = directoryCount === 1
      ? (flatDirectories ? "." : "src")
      : path.join(
        flatDirectories ? "." : "src",
        `part-${directoryIndex.toString().padStart(2, "0")}`,
      );
    const directory = path.join(workspace, relativeDirectory);
    await mkdir(directory, { recursive: true });
    const writes = [];
    for (let fileIndex = 0; fileIndex < filesPerDirectory; fileIndex++) {
      const globalIndex = directoryIndex * filesPerDirectory + fileIndex;
      const content = globalIndex === 0
        ? "ordinary text AHEAD_SEARCH_HOT\nAHEAD_SEARCH_MULTILINE\nNEXT_LINE\n"
        : "ordinary text AHEAD_SEARCH_HOT\n";
      writes.push(writeFile(
        path.join(directory, `file-${globalIndex.toString().padStart(5, "0")}.txt`),
        content,
      ));
    }
    await Promise.all(writes);
  }

  child = spawn(binary, ["--proxy"], {
    cwd: workspace,
    detached: true,
    stdio: ["pipe", "pipe", "pipe"],
    env: smokeEnv,
  });
  child.stderr.on("data", (chunk) => { stderr = (stderr + chunk).slice(-5000); });
  child.once("exit", (code, signal) => {
    exited = true;
    exitResult = { code, signal };
    for (const waiter of pending.values()) {
      clearTimeout(waiter.timer);
      waiter.reject(new Error(`Proxy exited before reply\n${stderr}`));
    }
    pending.clear();
  });
  lines = createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    const message = JSON.parse(line);
    const waiter = pending.get(message.id);
    if (!waiter || message.method) return;
    pending.delete(message.id);
    clearTimeout(waiter.timer);
    if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
    else waiter.resolve(message.result);
  });
  child.stdin.write(JSON.stringify({
    method: "initialize",
    params: { workspace, window_id: 1, tab_id: 1 },
  }) + "\n");

  async function request(method, params) {
    const id = ++nextId;
    const result = await new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`Proxy request timed out: ${method}\n${stderr}`));
      }, 120_000);
      pending.set(id, { resolve, reject, timer });
      child.stdin.write(JSON.stringify({
        id,
        method,
        params: typeof params === "function" ? params(id) : params,
      }) + "\n");
    });
    return result;
  }

  async function search(pattern) {
    const result = await request("global_search", {
      pattern,
      case_sensitive: true,
      whole_word: false,
      is_regex: false,
    });
    assert.equal(result.method, "global_search_response");
    return Object.values(result.params.matches)
      .reduce((total, fileMatches) => total + fileMatches.length, 0);
  }

  const indexStarted = performance.now();
  const indexResult = await request("workspace_files", (requestId) => ({ request_id: requestId }));
  const indexBuildMs = performance.now() - indexStarted;
  assert.equal(indexResult.method, "workspace_files_response");
  assert.equal(indexResult.params.files.length, fileCount);

  const noHitSamples = { ahead: [], ripgrep: [] };
  const measureAheadNoHit = async () => {
    const started = performance.now();
    const count = await search("AHEAD_SEARCH_ABSENT");
    return { count, elapsedMs: performance.now() - started };
  };
  const measureRipgrepNoHit = () => ripgrepCount("AHEAD_SEARCH_ABSENT");
  const comparisonRounds = compareRipgrep ? 5 : 3;
  if (compareRipgrep) {
    assert.equal((await measureAheadNoHit()).count, 0);
    assert.equal(measureRipgrepNoHit().count, 0);
  }
  for (let round = 0; round < comparisonRounds; round++) {
    const measures = round % 2 === 0
      ? [measureAheadNoHit, measureRipgrepNoHit]
      : [measureRipgrepNoHit, measureAheadNoHit];
    for (const measure of measures) {
      const sample = await measure();
      assert.equal(sample.count, 0);
      noHitSamples[measure === measureAheadNoHit ? "ahead" : "ripgrep"]
        .push(sample);
    }
  }

  const resultLimitStarted = performance.now();
  assert.equal(await search("AHEAD_SEARCH_MULTILINE\nNEXT_LINE"), 1);
  assert.equal(await search("AHEAD_SEARCH_HOT"), 5_000);
  const resultLimitMs = performance.now() - resultLimitStarted;
  const ripgrepAllMatches = compareRipgrep
    ? ripgrepCount("AHEAD_SEARCH_HOT")
    : undefined;
  if (ripgrepAllMatches) assert.equal(ripgrepAllMatches.count, fileCount);

  const outsideSearchFile = path.join(profile, "outside-search-secret.txt");
  const indexedFile = directoryCount === 1
    ? path.join(searchRoot, "file-00000.txt")
    : path.join(
      searchRoot,
      "part-00",
      "file-00000.txt",
    );
  await writeFile(outsideSearchFile, "AHEAD_SEARCH_OUTSIDE_SECRET\n");
  await rm(indexedFile);
  await symlink(outsideSearchFile, indexedFile);
  assert.equal(await search("AHEAD_SEARCH_OUTSIDE_SECRET"), 0,
    "workspace search must not follow an indexed file replaced with an outside symlink");

  child.stdin.write(JSON.stringify({ method: "shutdown", params: {} }) + "\n");
  const shutdownDeadline = performance.now() + 15_000;
  while (!exited && performance.now() < shutdownDeadline) {
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  assert.deepEqual(exitResult, { code: 0, signal: null }, stderr);
  passed = true;
  const medianNoHitSummary = compareRipgrep
    ? `; median no-hit over ${comparisonRounds} alternating samples: AHEAD ${medianElapsed(noHitSamples.ahead).toFixed(0)}ms, ripgrep ${medianElapsed(noHitSamples.ripgrep).toFixed(0)}ms`
    : `; median no-hit over ${comparisonRounds} samples: ${medianElapsed(noHitSamples.ahead).toFixed(0)}ms`;
  const ripgrepSummary = compareRipgrep
    ? `${medianNoHitSummary}; ripgrep counted ${ripgrepAllMatches.count} hot matches in ${ripgrepAllMatches.elapsedMs.toFixed(0)}ms`
    : medianNoHitSummary;
  console.log(
    `PASS workspace search: ${fileCount} files across ${directoryCount} ${flatDirectories ? "flat " : "nested "}directories (${filesPerDirectory} per directory); index build ${indexBuildMs.toFixed(0)}ms; ${resultLimitMs.toFixed(0)}ms to return 5,000 capped matches; stale-index symlink escape rejected${ripgrepSummary}`,
  );
} finally {
  if (!exited && child?.pid) {
    try { process.kill(-child.pid, "SIGKILL"); }
    catch (error) { if (error.code !== "ESRCH") throw error; }
  }
  lines?.close();
  if (passed) {
    await rm(workspace, { recursive: true });
    await rm(profile, { recursive: true });
  } else {
    console.error(`Disposable search project retained at ${workspace} with profile ${profile}`);
  }
}
