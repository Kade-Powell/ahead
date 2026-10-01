import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { chmod, mkdtemp, readFile, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";

// Opt-in crash probe: only kills proxies it starts in this disposable project.
// No model, language server, credentials, package install or real project is used.
const binary = path.resolve(process.argv[2] ?? "target/debug/ahead");
const workspace = await mkdtemp(path.join(tmpdir(), "ahead-recovery-smoke-"));
const original = "print('saved')\n";
await writeFile(path.join(workspace, "main.py"), original);
const clients = [];

function start() {
  const child = spawn(binary, ["--proxy"], { cwd: workspace, detached: true, stdio: ["pipe", "pipe", "pipe"] });
  const pending = new Map();
  let nextId = 0;
  let stderr = "";
  let failure;
  const fail = (error) => {
    failure = error;
    for (const { reject, timer } of pending.values()) {
      clearTimeout(timer);
      reject(error);
    }
    pending.clear();
  };
  child.stderr.on("data", (chunk) => { stderr = (stderr + chunk).slice(-4000); });
  child.on("error", fail);
  child.stdin.on("error", fail);
  const exited = new Promise((resolve) => child.once("exit", (code, signal) => {
    fail(new Error(`Proxy exited: ${code ?? signal}\n${stderr}`));
    resolve();
  }));
  const lines = createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    let message;
    try { message = JSON.parse(line); } catch (error) { fail(error); return; }
    const waiter = pending.get(message.id);
    if (!waiter || message.method) return;
    pending.delete(message.id);
    clearTimeout(waiter.timer);
    if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
    else waiter.resolve(message.result);
  });
  child.stdin.write(JSON.stringify({ method: "initialize", params: { workspace, window_id: clients.length + 1, tab_id: 1 } }) + "\n");
  const client = {
    async request(method, params) {
      if (failure) throw failure;
      const id = ++nextId;
      const result = await new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          pending.delete(id);
          reject(new Error(`Recovery request timed out: ${method}\n${stderr}`));
        }, 30000);
        pending.set(id, { resolve, reject, timer });
        child.stdin.write(JSON.stringify({ id, method: "ahead_request", params: { request: { method, params } } }) + "\n");
      });
      assert.equal(result.method, "ahead_response");
      return result.params.response;
    },
    async kill() {
      if (child.pid && child.exitCode === null && child.signalCode === null) {
        process.kill(-child.pid, "SIGKILL");
        await exited;
      }
      lines.close();
    },
  };
  clients.push(client);
  return client;
}

try {
  const first = start();
  assert.deepEqual(await first.request("list_editor_recoveries"), []);
  const databasePath = path.join(workspace, ".ahead", "session.db");
  if (process.platform !== "win32") {
    assert.equal((await stat(databasePath)).mode & 0o777, 0o600, "new database must be owner-only");
  }
  const snapshot = {
    buffer_id: randomUUID(), revision: 1, path: "main.py", contents: "print('unsaved 🐍')\n",
    saved_sha256: createHash("sha256").update(original).digest("hex"),
  };
  assert.equal(await first.request("write_editor_recovery", { snapshot }), true);
  const second = start();
  assert.deepEqual(await second.request("list_editor_recoveries"), [], "live owner must not be stolen");
  assert.equal(await second.request("read_editor_recovery", { buffer_id: snapshot.buffer_id }), null);
  assert.equal(await second.request("write_editor_recovery", { snapshot: { ...snapshot, revision: 2 } }), false);
  await first.kill();
  assert.deepEqual(await second.request("list_editor_recoveries"), [{ buffer_id: snapshot.buffer_id, path: snapshot.path, revision: 1 }]);
  assert.deepEqual(await second.request("read_editor_recovery", { buffer_id: snapshot.buffer_id }), snapshot);
  assert.equal(await readFile(path.join(workspace, "main.py"), "utf8"), original);
  console.log("PASS acknowledged Unicode buffer survives SIGKILL; live owner and disk file remain protected");

  const cleared = { ...snapshot, revision: 2, contents: null };
  assert.equal(await second.request("write_editor_recovery", { snapshot: cleared }), true);
  assert.equal(await second.request("write_editor_recovery", { snapshot }), false, "stale write must not resurrect discarded text");
  await second.kill();
  if (process.platform !== "win32") await chmod(databasePath, 0o644);
  const third = start();
  assert.deepEqual(await third.request("list_editor_recoveries"), [], "explicitly cleared backup must stay cleared after restart");
  assert.equal(await third.request("read_editor_recovery", { buffer_id: snapshot.buffer_id }), null);
  if (process.platform !== "win32") {
    assert.equal((await stat(databasePath)).mode & 0o777, 0o600, "reopened supported database must be owner-only");
  }
  console.log("PASS acknowledged discard survives restart and fences stale snapshots");
  if (process.platform !== "win32") console.log("PASS new and reopened database permissions are owner-only");
  console.log(`Disposable project retained at ${workspace}`);
} finally {
  for (const client of clients) await client.kill();
}
