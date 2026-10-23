import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";

const binary = path.resolve(process.argv[2] ?? "target/debug/ahead");
const requests = [];
const model = createServer((request, response) => {
  let body = "";
  request.setEncoding("utf8");
  request.on("data", (chunk) => { body += chunk; });
  request.on("end", () => {
    if (request.method === "GET" && request.url?.startsWith("/v1/models")) {
      const payload = JSON.stringify({ models: [] });
      response.writeHead(200, { "Content-Type": "application/json", "Content-Length": Buffer.byteLength(payload) });
      response.end(payload);
      return;
    }
    assert.equal(request.method, "POST", `Unexpected model request: ${request.method} ${request.url}`);
    requests.push(JSON.parse(body));
    const sequence = requests.length;
    const call = { id: `code-cell-${sequence}`, type: "custom_tool_call", call_id: `code-cell-${sequence}`, name: "exec", input: "const results = await Promise.all(Array.from({ length: 8 }, () => tools.file_search({ pattern: 'AHEAD_CODE_MODE_NEEDLE' }))); text(results); text(21 * 2);" };
    const message = { id: `answer-${sequence}`, type: "message", role: "assistant", content: [{ type: "output_text", text: "The cell returned 42." }] };
    const events = [1, 3, 4].includes(sequence) ? [
      { type: "response.output_item.added", output_index: 0, item: call },
      { type: "response.output_item.done", output_index: 0, item: call },
      { type: "response.completed", response: { id: "first", end_turn: false } },
    ] : [
      { type: "response.output_item.added", output_index: 0, item: message },
      { type: "response.output_item.done", output_index: 0, item: message },
      { type: "response.completed", response: { id: "second", end_turn: true } },
    ];
    const payload = events.map((event) => `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`).join("");
    response.writeHead(200, { "Content-Type": "text/event-stream", "Content-Length": Buffer.byteLength(payload) });
    response.end(payload);
  });
});
await new Promise((resolve, reject) => {
  model.once("error", reject);
  model.listen(0, "127.0.0.1", resolve);
});
const workspace = await mkdtemp(path.join(tmpdir(), "ahead-code-mode-"));
const profile = `${workspace}-profile`;
const smokeEnv = {
  PATH: process.env.PATH ?? "/usr/bin:/bin:/usr/sbin:/sbin",
  HOME: path.join(profile, "home"),
  TMPDIR: tmpdir(),
  LANG: process.env.LANG,
  LC_ALL: process.env.LC_ALL,
  RUST_LOG: "error",
  AHEAD_DATA_HOME: path.join(profile, "data"),
  AHEAD_HOME: path.join(profile, "runtime"),
  AHEAD_USER_HOME: path.join(profile, "user"),
};
for (const directory of [smokeEnv.HOME, smokeEnv.AHEAD_DATA_HOME, smokeEnv.AHEAD_HOME, smokeEnv.AHEAD_USER_HOME]) await mkdir(directory, { recursive: true });
await mkdir(path.join(workspace, ".ahead"));
await writeFile(path.join(workspace, "main.py"), "# AHEAD_CODE_MODE_NEEDLE\nprint('disposable')\n");
await writeFile(path.join(workspace, ".ahead", "settings.toml"),
  `[ai]\nactive_connection = "Mock"\n[[ai.connections]]\nname = "Mock"\nprovider_id = "mock"\nbase_url = "http://127.0.0.1:${model.address().port}/v1"\nmodel = "gpt-5.6-sol"\n`);

let child;
let lines;
let stderr = "";
let exited = false;
let exitResult;
const pending = new Map();
const snapshotReplies = [];
let snapshotRequests = 0;
let holdSnapshots = false;
let nextId = 0;
function rpc(method, params) {
  const id = ++nextId;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`Request timed out: ${method}\n${stderr}`));
    }, 30000);
    pending.set(id, { resolve, reject, timer });
    child.stdin.write(JSON.stringify({ id, method: "ahead_request", params: { request: { method, params } } }) + "\n");
  });
}

function startProxy() {
  child = spawn(binary, ["--proxy"], { cwd: workspace, detached: true, stdio: ["pipe", "pipe", "pipe"], env: smokeEnv });
  stderr = "";
  exited = false;
  exitResult = undefined;
  child.stderr.on("data", (chunk) => { stderr = (stderr + chunk).slice(-6000); });
  child.once("exit", (code, signal) => {
    exited = true;
    exitResult = { code, signal };
    for (const waiter of pending.values()) {
      clearTimeout(waiter.timer);
      waiter.reject(new Error(`Proxy exited before its reply\n${stderr}`));
    }
    pending.clear();
  });
  lines = createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    const message = JSON.parse(line);
    const notice = message.params?.notification;
    if (message.method === "ahead_notification" && notice?.method === "agent_buffer_snapshots_requested") {
      snapshotRequests++;
      if (!holdSnapshots) snapshotReplies.push(rpc("agent_buffer_snapshots_response", { ...notice.params, buffers: [] }));
      return;
    }
    const waiter = pending.get(message.id);
    if (!waiter || message.method) return;
    pending.delete(message.id);
    clearTimeout(waiter.timer);
    if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
    else waiter.resolve(message.result.params.response);
  });
  child.stdin.write(JSON.stringify({ method: "initialize", params: { workspace, window_id: 1, tab_id: 1 } }) + "\n");
}

async function waitUntil(predicate, description) {
  const deadline = Date.now() + 60000;
  while (!await predicate()) {
    assert(Date.now() < deadline, `Timed out: ${description}\n${stderr}`);
    await delay(30);
  }
}

let passed = false;
let lastHistory;
try {
  startProxy();
  const view = await rpc("start_work", {
    work_kind: "product-change", title: "Code Mode fixture", starting_point: "Run one cell", harness: "ahead",
  });
  const session_id = view.session.id;
  const request = {
    session_id, thread_id: "code-mode-fixture", harness: "ahead", model: "gpt-5.6-sol", model_provider: "mock",
    user_message: "Run a cell that prints 42", session_context: "Disposable Code Mode test",
    context: { active_path: "", caret: { line: 0, col: 0 }, selection: null, file_content: "", visible_end: null, attached_anchor_ids: [], attached_files: [], attached_memories: [] },
    invariants: [], cwd: workspace, expected_policy_sha256: view.session.policy.sha256, read_only: true,
  };
  await rpc("agent_turn_start", { request });
  await waitUntil(async () => {
    lastHistory = await rpc("conversation_messages_page", { session_id, before: null, limit: 10 });
    return lastHistory.messages.some((message) => message.role === "agent" && ["complete", "failed", "cancelled"].includes(message.status));
  }, "terminal managed turn");
  const agent = lastHistory.messages.find((message) => message.role === "agent");
  assert.equal(agent.status, "complete", JSON.stringify({ agent, requests: requests.length, stderr }));
  assert.equal(requests.length, 2, "one Code Mode call should lead to one continuation");
  const tools = requests[0].tools ?? requests[0].input.find((item) => item.type === "additional_tools")?.tools;
  const toolNames = tools?.flatMap((tool) => tool.type === "namespace" ? tool.tools.map((nested) => nested.name) : [tool.name]);
  assert(toolNames?.includes("exec"), `Code Mode exec was not advertised: ${JSON.stringify(toolNames)}`);
  assert(toolNames?.includes("wait"), `Code Mode wait was not advertised: ${JSON.stringify(toolNames)}`);
  assert.equal(snapshotReplies.length, 8, "concurrent delegated searches should each request an editor-buffer snapshot");
  await Promise.all(snapshotReplies);
  const outputs = requests[1].input.filter((item) => item.type === "custom_tool_call_output");
  assert.equal(outputs.length, 1, `Expected one Code Mode result: ${JSON.stringify(requests[1].input)}`);
  assert(JSON.stringify(outputs[0]).includes("42"), `Cell result was not 42: ${JSON.stringify(outputs[0])}`);
  assert(JSON.stringify(outputs[0]).includes("AHEAD_CODE_MODE_NEEDLE"), `Delegated search result was missing: ${JSON.stringify(outputs[0])}`);

  holdSnapshots = true;
  const { turn_id: cancelledTurn } = await rpc("agent_turn_start", { request: { ...request, user_message: "Cancel this cell during search" } });
  await waitUntil(() => snapshotRequests >= 16, "concurrent Code Mode searches waiting on editor snapshots");
  assert.equal((await rpc("agent_turn_cancel", { session_id })).cancelled, true);
  await waitUntil(async () => {
    lastHistory = await rpc("conversation_messages_page", { session_id, before: null, limit: 10 });
    return lastHistory.messages.some((message) => message.turn_id === cancelledTurn && message.role === "agent" && message.status === "cancelled");
  }, "cancelled Code Mode turn");
  assert.equal(requests.length, 3, "cancelled cell must not resume the model");

  child.stdin.write(JSON.stringify({ method: "shutdown", params: {} }) + "\n");
  await waitUntil(() => exited, "proxy shutdown before retry");
  assert.deepEqual(exitResult, { code: 0, signal: null }, stderr);
  lines.close();
  startProxy();
  lastHistory = await rpc("conversation_messages_page", { session_id, before: null, limit: 10 });
  assert(lastHistory.messages.some((message) => message.turn_id === cancelledTurn && message.role === "agent" && message.status === "cancelled"), "cancelled turn was not restored from Turso");

  holdSnapshots = false;
  const { turn_id: retriedTurn } = await rpc("agent_turn_retry", { session_id, turn_id: cancelledTurn });
  assert.notEqual(retriedTurn, cancelledTurn);
  await waitUntil(async () => {
    lastHistory = await rpc("conversation_messages_page", { session_id, before: null, limit: 10 });
    return lastHistory.messages.some((message) => message.turn_id === retriedTurn && message.role === "agent" && ["complete", "failed", "cancelled"].includes(message.status));
  }, "retried Code Mode turn");
  assert.equal(lastHistory.messages.find((message) => message.turn_id === retriedTurn && message.role === "agent").status, "complete");
  assert.equal(requests.length, 5, "retry should run one cell and one model continuation");
  assert.equal(snapshotRequests, 24);
  assert.equal(snapshotReplies.length, 16, "the cancelled snapshots must not receive late replies");
  await Promise.all(snapshotReplies);
  assert(JSON.stringify(requests[4].input).includes("AHEAD_CODE_MODE_NEEDLE"), "retried cell lost the delegated search result");
  assert(JSON.stringify(requests[4].input).includes("42"), "retried cell lost its computed result");
  child.stdin.write(JSON.stringify({ method: "shutdown", params: {} }) + "\n");
  await waitUntil(() => exited, "proxy shutdown");
  assert.deepEqual(exitResult, { code: 0, signal: null }, stderr);
  passed = true;
  console.log("PASS managed Code Mode cell, eight concurrent delegated searches, cancellation, retry after Turso reopen, and shutdown");
} finally {
  if (!exited && child.pid) process.kill(-child.pid, "SIGKILL");
  lines.close();
  model.closeAllConnections();
  await new Promise((resolve) => model.close(resolve));
  if (passed) {
    await rm(workspace, { recursive: true });
    await rm(profile, { recursive: true });
  } else console.error(`Disposable failure project retained at ${workspace} with profile ${profile}\n${JSON.stringify({ requests: requests.map(({ model, tools, input }) => ({ model, tools: tools?.map((tool) => tool.name), toolItems: input?.filter((item) => item.type?.includes("tool_call")) })), lastHistory, stderr })}`);
}
