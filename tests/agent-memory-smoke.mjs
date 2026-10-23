import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";

const binary = path.resolve(process.argv[2] ?? "target/debug/ahead");
const root = await mkdtemp(path.join(tmpdir(), "ahead-memory-smoke-"));
const workspace = path.join(root, "project");
const profile = path.join(root, "profile");
const userHome = path.join(profile, "user");
const smokeEnvironment = {
  PATH: process.env.PATH ?? "/usr/bin:/bin:/usr/sbin:/sbin",
  HOME: userHome,
  TMPDIR: tmpdir(),
  LANG: process.env.LANG,
  LC_ALL: process.env.LC_ALL,
  RUST_LOG: "error",
  AHEAD_DATA_HOME: path.join(profile, "data"),
  AHEAD_HOME: path.join(profile, "runtime"),
  AHEAD_USER_HOME: userHome,
};
for (const directory of [workspace, userHome, smokeEnvironment.AHEAD_DATA_HOME,
  smokeEnvironment.AHEAD_HOME]) {
  await mkdir(directory, { recursive: true, mode: 0o700 });
}

const clients = [];
function startProxy() {
  const child = spawn(binary, ["--proxy"], {
    cwd: workspace,
    detached: true,
    stdio: ["pipe", "pipe", "pipe"],
    env: smokeEnvironment,
  });
  const pending = new Map();
  let nextId = 0;
  let stderr = "";
  let exit;
  const exited = new Promise((resolve) => child.once("exit", (code, signal) => {
    exit = { code, signal };
    for (const { reject, timer } of pending.values()) {
      clearTimeout(timer);
      reject(new Error(`Proxy exited: ${code ?? signal}\n${stderr}`));
    }
    pending.clear();
    resolve(exit);
  }));
  child.stderr.on("data", (chunk) => { stderr = (stderr + chunk).slice(-4000); });
  const lines = createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    let message;
    try { message = JSON.parse(line); } catch (error) {
      for (const { reject } of pending.values()) reject(error);
      pending.clear();
      return;
    }
    const waiter = pending.get(message.id);
    if (!waiter || message.method) return;
    pending.delete(message.id);
    clearTimeout(waiter.timer);
    if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
    else waiter.resolve(message.result.params.response);
  });
  child.stdin.write(JSON.stringify({
    method: "initialize",
    params: { workspace, window_id: clients.length + 1, tab_id: 1 },
  }) + "\n");

  const client = {
    async request(method, params) {
      const id = ++nextId;
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          pending.delete(id);
          reject(new Error(`Request timed out: ${method}\n${stderr}`));
        }, 15000);
        pending.set(id, { resolve, reject, timer });
        child.stdin.write(JSON.stringify({
          id,
          method: "ahead_request",
          params: { request: { method, params } },
        }) + "\n");
      });
    },
    async stop() {
      child.stdin.write(JSON.stringify({ method: "shutdown", params: {} }) + "\n");
      const result = await exited;
      assert.deepEqual(result, { code: 0, signal: null }, stderr);
      lines.close();
    },
    async cleanup() {
      if (!exit && child.pid) {
        process.kill(-child.pid, "SIGKILL");
        await exited;
      }
      lines.close();
    },
  };
  clients.push(client);
  return client;
}

let passed = false;
try {
  const client = startProxy();
  const sharedToken = "AHEAD_MEMORY_SHARED_SCOPE_SENTINEL";
  const projectToken = "AHEAD_MEMORY_REVIEWED_PROJECT_SENTINEL";
  await client.request("write_memory", {
    scope: "project", message_id: "project-note-1", content: `Project note ${sharedToken}.`,
  });
  await client.request("write_memory", {
    scope: "user", message_id: "user-note-1", content: `User note ${sharedToken}.`,
  });

  const sharedHits = await client.request("search_memory", { query: sharedToken, limit: 8 });
  const sharedMatches = sharedHits.filter(({ excerpt }) => excerpt.includes(sharedToken));
  assert.equal(sharedMatches.length, 2, JSON.stringify(sharedHits));
  assert(sharedHits.slice(0, 2).every(({ excerpt }) => excerpt.includes(sharedToken)),
    `Expected exact memory lines to rank first: ${JSON.stringify(sharedHits)}`);
  assert.deepEqual(new Set(sharedMatches.map(({ scope }) => scope)), new Set(["project", "user"]));
  assert.deepEqual(new Set(sharedHits.map(({ source }) => source)),
    new Set([".ahead/memories/MEMORY.md", "~/.ahead/memories/MEMORY.md"]));
  assert(!JSON.stringify(sharedHits).includes(workspace));
  assert(!JSON.stringify(sharedHits).includes(userHome));

  const projectSnapshot = await client.request("read_memory", { scope: "project" });
  const userSnapshot = await client.request("read_memory", { scope: "user" });
  assert.equal(projectSnapshot.scope, "project");
  assert.equal(userSnapshot.scope, "user");
  assert(projectSnapshot.content.includes(sharedToken));
  assert(userSnapshot.content.includes(sharedToken));
  const replacement = `# Reviewed project memory\n${projectToken}\n`;
  const result = await client.request("replace_memory", {
    scope: "project", expected_sha256: projectSnapshot.sha256, content: replacement,
  });
  assert.equal(result.indexed, true, JSON.stringify(result));
  await assert.rejects(client.request("replace_memory", {
    scope: "project", expected_sha256: projectSnapshot.sha256, content: "Stale replacement\n",
  }), /changed since review/);
  assert.equal((await client.request("read_memory", { scope: "project" })).content, replacement);
  assert((await client.request("read_memory", { scope: "user" })).content.includes(sharedToken));

  const refreshedSharedHits = await client.request("search_memory", { query: sharedToken, limit: 8 });
  const refreshedSharedMatches = refreshedSharedHits.filter(({ excerpt }) => excerpt.includes(sharedToken));
  assert.equal(refreshedSharedMatches.length, 1, JSON.stringify(refreshedSharedHits));
  assert.equal(refreshedSharedMatches[0].scope, "user");
  const reviewedHits = await client.request("search_memory", { query: projectToken, limit: 8 });
  const reviewedMatches = reviewedHits.filter(({ excerpt }) => excerpt.includes(projectToken));
  assert.equal(reviewedMatches.length, 1, JSON.stringify(reviewedHits));
  assert.equal(reviewedMatches[0].scope, "project");
  await client.stop();

  const reopened = startProxy();
  const reopenedProject = await reopened.request("read_memory", { scope: "project" });
  const reopenedUser = await reopened.request("read_memory", { scope: "user" });
  assert.equal(reopenedProject.content, replacement);
  assert.equal(reopenedUser.content, userSnapshot.content);
  assert(await readFile(path.join(workspace, ".ahead", "session.db")));
  await reopened.stop();
  passed = true;
  console.log("PASS isolated project/user memory append, search, reviewed replacement, stale-snapshot rejection, and Turso reopen");
} finally {
  for (const client of clients) await client.cleanup();
  if (passed) await rm(root, { recursive: true, force: true });
  else console.error(`Disposable memory fixture retained at ${root}`);
}
