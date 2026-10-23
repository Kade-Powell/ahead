import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";

// Real proxy and Turso reopen; the model is a loopback fixture with no tools.
// No provider, credentials, download, or existing project is used.
const binary = path.resolve(process.argv[2] ?? "target/debug/ahead");
const workspace = await mkdtemp(path.join(tmpdir(), "ahead-agent-shutdown-"));
const profile = `${workspace}-profile`;
const smokeEnv = { ...process.env, AHEAD_DATA_HOME: path.join(profile, "data"), AHEAD_HOME: path.join(profile, "runtime"), AHEAD_USER_HOME: path.join(profile, "user") };
for (const directory of [smokeEnv.AHEAD_DATA_HOME, smokeEnv.AHEAD_HOME, smokeEnv.AHEAD_USER_HOME]) await mkdir(directory, { recursive: true });
const original = "print('unchanged')\n";
await writeFile(path.join(workspace, "main.py"), original);
const clients = [];
let modelRequests = 0;
const model = createServer((request, response) => {
  request.resume();
  request.on("end", () => {
    if (request.method === "GET" && request.url?.startsWith("/v1/models")) {
      const payload = JSON.stringify({ models: [] });
      response.writeHead(200, { "Content-Type": "application/json", "Content-Length": Buffer.byteLength(payload) });
      response.end(payload);
      return;
    }
    assert.equal(request.method, "POST", `Unexpected model request: ${request.method} ${request.url}`);
    modelRequests++;
    response.writeHead(200, { "Content-Type": "text/event-stream" });
    const event = (type, fields) => response.write(`event: ${type}\ndata: ${JSON.stringify({ type, ...fields })}\n\n`);
    event("response.output_item.added", {
      output_index: 0, item: { id: "message-1", type: "message", role: "assistant", content: [] },
    });
    event("response.output_text.delta", {
      item_id: "message-1", output_index: 0, content_index: 0, delta: "Partial answer before close",
    });
    // Keep the response open until AHEAD cancels the turn on shutdown.
  });
});
await new Promise((resolve, reject) => {
  model.once("error", reject);
  model.listen(0, "127.0.0.1", resolve);
});
await mkdir(path.join(workspace, ".ahead"));
await writeFile(path.join(workspace, "AGENTS.md"), "Follow the disposable project instructions.\n");
await writeFile(path.join(workspace, ".ahead", "settings.toml"),
  `[ai]\nactive_connection = "Mock"\n[[ai.connections]]\nname = "Mock"\nprovider_id = "mock"\nbase_url = "http://127.0.0.1:${model.address().port}/v1"\nmodel = "ahead-test"\n`);

async function waitUntil(predicate, description) {
  const deadline = Date.now() + 15000;
  while (!await predicate()) {
    assert(Date.now() < deadline, `Timed out: ${description}`);
    await delay(20);
  }
}

function start() {
  const child = spawn(binary, ["--proxy"], { cwd: workspace, detached: true, stdio: ["pipe", "pipe", "pipe"], env: smokeEnv });
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
  child.stderr.on("data", (chunk) => { stderr = (stderr + chunk).slice(-6000); });
  child.on("error", fail);
  child.stdin.on("error", fail);
  let exit;
  const exited = new Promise((resolve) => child.once("exit", (code, signal) => {
    exit = { code, signal };
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
          reject(new Error(`Request timed out: ${method}\n${stderr}`));
        }, 15000);
        pending.set(id, { resolve, reject, timer });
        child.stdin.write(JSON.stringify({ id, method: "ahead_request", params: { request: { method, params } } }) + "\n");
      });
      assert.equal(result.method, "ahead_response");
      return result.params.response;
    },
    async stop(eof) {
      if (eof) child.stdin.end();
      else child.stdin.write(JSON.stringify({ method: "shutdown", params: {} }) + "\n");
      await waitUntil(() => exit, "proxy shutdown");
      assert.deepEqual(exit, { code: 0, signal: null }, stderr);
      assert(!/did not finish|shutdown deadline elapsed/.test(stderr), stderr);
    },
    async cleanup() {
      if (!exit && child.pid) {
        process.kill(-child.pid, "SIGKILL");
        await exited;
      }
      lines.close();
      return exit;
    },
  };
  clients.push(client);
  return client;
}

let passed = false;
try {
  for (const eof of [false, true]) {
    const client = start();
    const view = await client.request("start_work", {
      work_kind: "product-change", title: "Shutdown fixture", starting_point: "Test cancellation", harness: "ahead",
    });
    const session_id = view.session.id;
    const { turn_id: turnId } = await client.request("agent_turn_start", { request: {
      session_id, thread_id: "shutdown-fixture", harness: "ahead", model: "ahead-test", model_provider: "mock",
      user_message: "Send a partial answer and wait", session_context: "Disposable shutdown test",
      context: { active_path: "", caret: { line: 0, col: 0 }, selection: null, file_content: "", visible_end: null, attached_anchor_ids: [], attached_files: [], attached_memories: [] },
      invariants: [], cwd: workspace, expected_policy_sha256: view.session.policy.sha256, read_only: true,
    } });
    const messages = (connection) => connection.request("conversation_messages_page", { session_id, before: null, limit: 10 });
    await waitUntil(async () => (await messages(client)).messages.some((message) => message.content === "Partial answer before close"), "persisted partial answer");
    await client.stop(eof);
    const reopened = start();
    const history = await messages(reopened);
    const agent = history.messages.find((message) => message.role === "agent");
    assert.equal(history.messages.length, 2);
    assert.equal(agent.turn_id, turnId);
    assert.equal(agent.status, "cancelled");
    assert.equal(agent.content, "Partial answer before close");
    assert.equal(await readFile(path.join(workspace, "main.py"), "utf8"), original);
    await reopened.stop(false);
    console.log(`PASS ${eof ? "stdin EOF" : "explicit shutdown"}: cancelled turn and partial answer survive Turso reopen`);
  }
  const crashed = start();
  const view = await crashed.request("start_work", {
    work_kind: "product-change", title: "Crash fixture", starting_point: "Test fail-closed retry", harness: "ahead",
  });
  const session_id = view.session.id;
  const { turn_id } = await crashed.request("agent_turn_start", { request: {
    session_id, thread_id: "crash-fixture", harness: "ahead", model: "ahead-test", model_provider: "mock",
    user_message: "Send a partial answer and wait", session_context: "Disposable crash test",
    context: { active_path: "", caret: { line: 0, col: 0 }, selection: null, file_content: "", visible_end: null, attached_anchor_ids: [], attached_files: [], attached_memories: [] },
    invariants: [], cwd: workspace, expected_policy_sha256: view.session.policy.sha256, read_only: true,
  } });
  await waitUntil(async () => (await crashed.request("conversation_messages_page", { session_id, before: null, limit: 10 })).messages.some((message) => message.turn_id === turn_id && message.content === "Partial answer before close"), "persisted partial answer before crash");
  assert.equal((await crashed.cleanup()).signal, "SIGKILL");
  const reopened = start();
  const history = await reopened.request("conversation_messages_page", { session_id, before: null, limit: 10 });
  const agent = history.messages.find((message) => message.turn_id === turn_id && message.role === "agent");
  assert.equal(agent.status, "failed");
  assert.equal(agent.content, "Partial answer before close");
  assert.equal(modelRequests, 3, "crashed turn must not replay automatically");
  const retry = await reopened.request("agent_turn_retry", { session_id, turn_id });
  assert.ok(retry.turn_id);
  assert.notEqual(retry.turn_id, turn_id);
  await waitUntil(() => modelRequests === 4, "explicit retry using persisted instruction provenance");
  await reopened.stop(false);
  console.log("PASS SIGKILL: partial answer and instruction provenance survive Turso reopen; explicit retry starts once");
  passed = true;
} finally {
  for (const client of clients) await client.cleanup();
  model.closeAllConnections();
  await new Promise((resolve) => model.close(resolve));
  if (passed) {
    await rm(workspace, { recursive: true });
    await rm(profile, { recursive: true });
  } else console.error(`Disposable failure project retained at ${workspace} with profile ${profile}`);
}
