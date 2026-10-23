import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const binary = path.resolve(process.argv[2] ?? "target/debug/ahead");
const root = await mkdtemp(path.join(tmpdir(), "ahead-mcp-smoke-"));
const workspace = path.join(root, "project");
const profile = path.join(root, "profile");
const userHome = path.join(profile, "user");
const environment = {
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
for (const directory of [workspace, userHome, environment.AHEAD_DATA_HOME, environment.AHEAD_HOME]) {
  await mkdir(directory, { recursive: true, mode: 0o700 });
}

const modelRequests = [];
let modelScenario = "modern";
let modelScenarioStart = 0;
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
    assert.equal(request.method, "POST", `Unexpected model request ${request.method} ${request.url}`);
    modelRequests.push(JSON.parse(body));
    const turn = modelRequests.length - modelScenarioStart - 1;
    const modernCalls = [
      { id: "mcp-search", type: "tool_search_call", call_id: "mcp-search", execution: "client", arguments: { query: "echo", limit: 1 } },
      { id: "mcp-call", type: "function_call", call_id: "mcp-call", namespace: "mcp__echo", name: "echo", arguments: JSON.stringify({ text: "AHEAD_MCP_RUNTIME_SENTINEL", payload: { depth: 1 } }) },
      { id: "resource-list", type: "function_call", call_id: "resource-list", name: "list_mcp_resources", arguments: "{}" },
      { id: "resource-templates", type: "function_call", call_id: "resource-templates", name: "list_mcp_resource_templates", arguments: "{}" },
      { id: "resource-read", type: "function_call", call_id: "resource-read", name: "read_mcp_resource", arguments: JSON.stringify({ server: "echo", uri: "memo://ahead-smoke" }) },
    ];
    const legacyCalls = [
      { id: "legacy-search", type: "tool_search_call", call_id: "legacy-search", execution: "client", arguments: { query: "echo", limit: 1 } },
      { id: "legacy-call", type: "function_call", call_id: "legacy-call", namespace: "mcp__echo", name: "echo", arguments: JSON.stringify({ text: "AHEAD_MCP_LEGACY_SENTINEL" }) },
    ];
    const mrtrCalls = [
      { id: "mrtr-search", type: "tool_search_call", call_id: "mrtr-search", execution: "client", arguments: { query: "stateful", limit: 1 } },
      { id: "mrtr-call", type: "function_call", call_id: "mrtr-call", namespace: "mcp__echo", name: "stateful", arguments: JSON.stringify({ text: "AHEAD_MCP_MRTR_SENTINEL" }) },
    ];
    const cancelCalls = [
      { id: "cancel-search", type: "tool_search_call", call_id: "cancel-search", execution: "client", arguments: { query: "echo", limit: 1 } },
      { id: "cancel-call", type: "function_call", call_id: "cancel-call", namespace: "mcp__echo", name: "echo", arguments: JSON.stringify({ text: "AHEAD_MCP_CANCEL_SENTINEL" }) },
    ];
    const calls = modelScenario === "modern" ? modernCalls
      : modelScenario === "legacy" ? legacyCalls
      : modelScenario === "mrtr" ? mrtrCalls
      : cancelCalls;
    const finalText = modelScenario === "modern" ? "MCP_RUNTIME_OK"
      : modelScenario === "legacy" ? "MCP_LEGACY_OK"
      : modelScenario === "mrtr" ? "MCP_MRTR_OK"
      : "MCP_CANCEL_SHOULD_NOT_CONTINUE";
    const item = turn < calls.length
      ? calls[turn]
      : { id: "mcp-answer", type: "message", role: "assistant", content: [{ type: "output_text", text: finalText }] };
    const addedItem = item.type === "function_call"
      ? { ...item, arguments: "" }
      : item.type === "message" ? { ...item, content: [] } : item;
    const events = [
      { type: "response.output_item.added", output_index: 0, item: addedItem },
      { type: "response.output_item.done", output_index: 0, item },
      { type: "response.completed", response: { id: `mcp-response-${modelRequests.length}`, end_turn: turn === calls.length } },
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

const ahead = path.join(workspace, ".ahead");
await mkdir(ahead, { mode: 0o700 });
const mcpServer = path.join(ahead, "mcp_echo.py");
let trace = path.join(ahead, "mcp_calls.jsonl");
const fixture = fileURLToPath(new URL("./fixtures/editor-smoke/mcp_echo.py", import.meta.url));
await writeFile(mcpServer, await readFile(fixture));
await writeFile(path.join(ahead, "config.toml"),
  `[mcp.servers.echo]\ncommand = "python3"\nargs = ["-u", ${JSON.stringify(mcpServer)}, ${JSON.stringify(trace)}]\n`);
await writeFile(path.join(ahead, "settings.toml"),
  `[ai]\nactive_connection = "Mock"\n[[ai.connections]]\nname = "Mock"\nprovider_id = "mock"\nbase_url = "http://127.0.0.1:${model.address().port}/v1"\nmodel = "ahead-test"\n`);
await writeFile(path.join(environment.AHEAD_DATA_HOME, "trusted-workspaces.toml"),
  `trusted = [${JSON.stringify(await realpath(workspace))}]\nblocked = []\n`);

const clients = [];
const notifications = [];
function startProxy() {
  const child = spawn(binary, ["--proxy"], { cwd: workspace, detached: true, stdio: ["pipe", "pipe", "pipe"], env: environment });
  const pending = new Map();
  let nextId = 0;
  let stderr = "";
  let exit;
  const exited = new Promise((resolve) => child.once("exit", (code, signal) => {
    exit = { code, signal };
    for (const waiter of pending.values()) {
      clearTimeout(waiter.timer);
      waiter.reject(new Error(`Proxy exited: ${code ?? signal}\n${stderr}`));
    }
    pending.clear();
    resolve(exit);
  }));
  child.stderr.on("data", (chunk) => { stderr = (stderr + chunk).slice(-6000); });
  const lines = createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    let message;
    try { message = JSON.parse(line); }
    catch (error) { for (const waiter of pending.values()) waiter.reject(error); return; }
    if (message.method === "ahead_notification") {
      notifications.push(message.params?.notification);
      return;
    }
    const waiter = pending.get(message.id);
    if (!waiter || message.method) return;
    pending.delete(message.id);
    clearTimeout(waiter.timer);
    if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
    else waiter.resolve(message.result.params.response);
  });
  child.stdin.write(JSON.stringify({ method: "initialize", params: { workspace, window_id: clients.length + 1, tab_id: 1 } }) + "\n");

  const client = {
    async request(method, params) {
      const id = ++nextId;
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          pending.delete(id);
          reject(new Error(`Request timed out: ${method}\n${stderr}`));
        }, 20000);
        pending.set(id, { resolve, reject, timer });
        child.stdin.write(JSON.stringify({ id, method: "ahead_request", params: { request: { method, params } } }) + "\n");
      });
    },
    async stop() {
      child.stdin.write(JSON.stringify({ method: "shutdown", params: {} }) + "\n");
      await waitUntil(() => exit, "proxy shutdown");
      const result = exit;
      assert.deepEqual(result, { code: 0, signal: null }, stderr);
      lines.close();
    },
    get stderr() { return stderr; },
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

async function waitUntil(predicate, description) {
  const deadline = Date.now() + 30000;
  while (!await predicate()) {
    assert(Date.now() < deadline, `Timed out: ${description}`);
    await delay(25);
  }
}

let approvalsAnswered = 0;
async function answerApproval(client, sessionId, operation, callId, expectedServerMethod) {
  await waitUntil(() => notifications.filter((notification) => notification?.method === "agent_user_input_requested").length > approvalsAnswered, `${operation} approval prompt`);
  const notification = notifications.filter((item) => item?.method === "agent_user_input_requested")[approvalsAnswered++].params;
  assert.equal(notification.session_id, sessionId);
  const question = notification.request.questions[0];
  assert.equal(question.header, "Approve MCP tool?");
  assert(question.id.startsWith(`mcp_tool_call_approval_${callId}`), question.id);
  assert(question.question.includes(operation), question.question);
  assert(question.options.some((option) => option.label === "Allow"));
  const traceLines = await readFile(trace, "utf8").catch(() => "");
  assert(!traceLines.split("\n").some((line) => line.includes(`\"method\": \"${expectedServerMethod}\"`)),
    `${expectedServerMethod} ran before its approval`);
  await client.request("agent_user_input_answer", {
    session_id: sessionId,
    request_id: notification.request.request_id,
    answers: { [question.id]: ["Allow"] },
  });
}

let passed = false;
try {
  const client = startProxy();
  const declarations = await client.request("list_mcp_server_declarations", { workspace });
  const echo = declarations.find((declaration) => declaration.id === "echo");
  assert(echo?.declared && echo.fingerprint, JSON.stringify(declarations));
  await client.request("set_mcp_server_approval", {
    workspace,
    server_id: "echo",
    expected_fingerprint: echo.fingerprint,
    action: "ApproveAndEnable",
  });
  const view = await client.request("start_work", {
    work_kind: "product-change", title: "MCP runtime fixture", starting_point: "Exercise managed MCP", harness: "ahead",
  });
  const session_id = view.session.id;
  await client.request("agent_turn_start", { request: {
    session_id,
    thread_id: "mcp-runtime-fixture",
    harness: "ahead",
    model: "ahead-test",
    model_provider: "mock",
    user_message: "Use the echo MCP tool once with AHEAD_MCP_RUNTIME_SENTINEL, then list resources and templates, read memo://ahead-smoke, and report the results.",
    session_context: "Disposable managed MCP runtime test",
    context: { active_path: "", caret: { line: 0, col: 0 }, selection: null, file_content: "", visible_end: null, attached_anchor_ids: [], attached_files: [], attached_memories: [] },
    invariants: [],
    cwd: workspace,
    expected_policy_sha256: view.session.policy.sha256,
    read_only: false,
  } });

  await answerApproval(client, session_id, "echo", "mcp-call", "tools/call");
  await answerApproval(client, session_id, "list_mcp_resources", "resource-list:echo", "resources/list");
  await answerApproval(client, session_id, "list_mcp_resource_templates", "resource-templates:echo", "resources/templates/list");
  await answerApproval(client, session_id, "read_mcp_resource", "resource-read:echo", "resources/read");

  let history;
  await waitUntil(async () => {
    history = await client.request("conversation_messages_page", { session_id, before: null, limit: 10 });
    return history.messages.some((message) => message.role === "agent" && ["complete", "failed", "cancelled"].includes(message.status));
  }, "managed MCP turn completion");
  const agent = history.messages.find((message) => message.role === "agent");
  assert.equal(agent.status, "complete", JSON.stringify({ agent, stderr: client.stderr }));
  assert(agent.content.includes("MCP_RUNTIME_OK"), agent.content);
  assert.equal(modelRequests.length, 6, "tool search, MCP/resource calls and final answer should be six model requests");
  assert(JSON.stringify(modelRequests[1]).includes("mcp__echo"), "MCP tool was not advertised after discovery");
  assert(JSON.stringify(modelRequests[2]).includes("AHEAD_MCP_RUNTIME_SENTINEL"), "MCP result did not return to the model");
  assert(JSON.stringify(modelRequests[5]).includes("MCP_RESOURCE_SENTINEL"), "MCP resource content did not return to the model");
  const calls = (await readFile(trace, "utf8")).trim().split("\n").map((line) => JSON.parse(line));
  assert.deepEqual(calls.map((call) => call.method), ["tools/call", "resources/list", "resources/templates/list", "resources/read"]);
  assert.equal(calls[0].params.name, "echo");
  assert.equal(calls[0].params.arguments.text, "AHEAD_MCP_RUNTIME_SENTINEL");
  assert.equal(calls[3].params.uri, "memo://ahead-smoke");

  await client.stop();
  trace = path.join(ahead, "mcp_legacy_calls.jsonl");
  await writeFile(path.join(ahead, "config.toml"),
    `[mcp.servers.echo]\ncommand = "python3"\nargs = ["-u", ${JSON.stringify(mcpServer)}, ${JSON.stringify(trace)}, "--form"]\n`);
  modelScenario = "legacy";
  modelScenarioStart = modelRequests.length;
  const legacyClient = startProxy();
  const legacyDeclarations = await legacyClient.request("list_mcp_server_declarations", { workspace });
  const legacyEcho = legacyDeclarations.find((declaration) => declaration.id === "echo");
  assert(legacyEcho?.declared && legacyEcho.fingerprint, JSON.stringify(legacyDeclarations));
  await legacyClient.request("set_mcp_server_approval", {
    workspace,
    server_id: "echo",
    expected_fingerprint: legacyEcho.fingerprint,
    action: "ApproveAndEnable",
  });
  const legacyView = await legacyClient.request("start_work", {
    work_kind: "product-change", title: "MCP legacy fallback fixture", starting_point: "Exercise fallback", harness: "ahead",
  });
  const legacySessionId = legacyView.session.id;
  await legacyClient.request("agent_turn_start", { request: {
    session_id: legacySessionId,
    thread_id: "mcp-legacy-fixture",
    harness: "ahead",
    model: "ahead-test",
    model_provider: "mock",
    user_message: "Call the echo tool once with AHEAD_MCP_LEGACY_SENTINEL, then report its result.",
    session_context: "Disposable legacy MCP fallback test",
    context: { active_path: "", caret: { line: 0, col: 0 }, selection: null, file_content: "", visible_end: null, attached_anchor_ids: [], attached_files: [], attached_memories: [] },
    invariants: [],
    cwd: workspace,
    expected_policy_sha256: legacyView.session.policy.sha256,
    read_only: false,
  } });
  await answerApproval(legacyClient, legacySessionId, "echo", "legacy-call", "tools/call");
  let legacyHistory;
  await waitUntil(async () => {
    legacyHistory = await legacyClient.request("conversation_messages_page", { session_id: legacySessionId, before: null, limit: 10 });
    return legacyHistory.messages.some((message) => message.role === "agent" && ["complete", "failed", "cancelled"].includes(message.status));
  }, "legacy MCP fallback turn completion");
  const legacyAgent = legacyHistory.messages.find((message) => message.role === "agent");
  assert.equal(legacyAgent.status, "complete", JSON.stringify({ legacyAgent, stderr: legacyClient.stderr }));
  assert(legacyAgent.content.includes("MCP_LEGACY_OK"), legacyAgent.content);
  assert.equal(modelRequests.length - modelScenarioStart, 3);
  assert(JSON.stringify(modelRequests[modelScenarioStart + 1]).includes("mcp__echo"));
  assert(JSON.stringify(modelRequests[modelScenarioStart + 2]).includes("AHEAD_MCP_LEGACY_SENTINEL"));
  const legacyCalls = (await readFile(trace, "utf8")).trim().split("\n").map((line) => JSON.parse(line));
  assert.equal(legacyCalls.length, 1);
  assert.equal(legacyCalls[0].method, "tools/call");
  assert.equal(legacyCalls[0].params.arguments.text, "AHEAD_MCP_LEGACY_SENTINEL");
  await legacyClient.stop();

  trace = path.join(ahead, "mcp_mrtr_calls.jsonl");
  await writeFile(path.join(ahead, "config.toml"),
    `[mcp.servers.echo]\ncommand = "python3"\nargs = ["-u", ${JSON.stringify(mcpServer)}, ${JSON.stringify(trace)}]\n`);
  modelScenario = "mrtr";
  modelScenarioStart = modelRequests.length;
  const mrtrClient = startProxy();
  const mrtrDeclarations = await mrtrClient.request("list_mcp_server_declarations", { workspace });
  const mrtrEcho = mrtrDeclarations.find((declaration) => declaration.id === "echo");
  assert(mrtrEcho?.declared && mrtrEcho.fingerprint, JSON.stringify(mrtrDeclarations));
  await mrtrClient.request("set_mcp_server_approval", {
    workspace,
    server_id: "echo",
    expected_fingerprint: mrtrEcho.fingerprint,
    action: "ApproveAndEnable",
  });
  const mrtrView = await mrtrClient.request("start_work", {
    work_kind: "product-change", title: "MCP MRTR fixture", starting_point: "Exercise request-state retry", harness: "ahead",
  });
  const mrtrSessionId = mrtrView.session.id;
  await mrtrClient.request("agent_turn_start", { request: {
    session_id: mrtrSessionId,
    thread_id: "mcp-mrtr-fixture",
    harness: "ahead",
    model: "ahead-test",
    model_provider: "mock",
    user_message: "Call the stateful MCP tool and report its result.",
    session_context: "Disposable MCP request-state retry test",
    context: { active_path: "", caret: { line: 0, col: 0 }, selection: null, file_content: "", visible_end: null, attached_anchor_ids: [], attached_files: [], attached_memories: [] },
    invariants: [],
    cwd: workspace,
    expected_policy_sha256: mrtrView.session.policy.sha256,
    read_only: false,
  } });
  await answerApproval(mrtrClient, mrtrSessionId, "stateful", "mrtr-call", "tools/call");
  let mrtrHistory;
  await waitUntil(async () => {
    mrtrHistory = await mrtrClient.request("conversation_messages_page", { session_id: mrtrSessionId, before: null, limit: 10 });
    return mrtrHistory.messages.some((message) => message.role === "agent" && ["complete", "failed", "cancelled"].includes(message.status));
  }, "MCP request-state retry turn completion");
  const mrtrAgent = mrtrHistory.messages.find((message) => message.role === "agent");
  assert.equal(mrtrAgent.status, "complete", JSON.stringify({ mrtrAgent, stderr: mrtrClient.stderr }));
  assert(mrtrAgent.content.includes("MCP_MRTR_OK"), mrtrAgent.content);
  assert.equal(modelRequests.length - modelScenarioStart, 3, "stateful MCP result should return to the model before its final answer");
  const mrtrCallsOnServer = (await readFile(trace, "utf8")).trim().split("\n").map((line) => JSON.parse(line));
  assert.deepEqual(mrtrCallsOnServer.map((call) => call.method), ["tools/call", "tools/call"]);
  assert.equal(mrtrCallsOnServer[0].params.arguments.text, "AHEAD_MCP_MRTR_SENTINEL");
  assert.equal(mrtrCallsOnServer[0].params.requestState, undefined);
  assert.equal(mrtrCallsOnServer[1].params.requestState, "AHEAD_MCP_MRTR_STATE");
  await mrtrClient.stop();

  trace = path.join(ahead, "mcp_cancel_calls.jsonl");
  await writeFile(path.join(ahead, "config.toml"),
    `[mcp.servers.echo]\ncommand = "python3"\nargs = ["-u", ${JSON.stringify(mcpServer)}, ${JSON.stringify(trace)}]\n`);
  modelScenario = "cancel";
  modelScenarioStart = modelRequests.length;
  const cancelClient = startProxy();
  const cancelDeclarations = await cancelClient.request("list_mcp_server_declarations", { workspace });
  const cancelEcho = cancelDeclarations.find((declaration) => declaration.id === "echo");
  assert(cancelEcho?.declared && cancelEcho.fingerprint, JSON.stringify(cancelDeclarations));
  await cancelClient.request("set_mcp_server_approval", {
    workspace,
    server_id: "echo",
    expected_fingerprint: cancelEcho.fingerprint,
    action: "ApproveAndEnable",
  });
  const cancelView = await cancelClient.request("start_work", {
    work_kind: "product-change", title: "MCP cancellation fixture", starting_point: "Exercise in-flight cancellation", harness: "ahead",
  });
  const cancelSessionId = cancelView.session.id;
  const { turn_id: cancelledTurn } = await cancelClient.request("agent_turn_start", { request: {
    session_id: cancelSessionId,
    thread_id: "mcp-cancel-fixture",
    harness: "ahead",
    model: "ahead-test",
    model_provider: "mock",
    user_message: "Call the echo tool with AHEAD_MCP_CANCEL_SENTINEL, then report the result.",
    session_context: "Disposable in-flight MCP cancellation test",
    context: { active_path: "", caret: { line: 0, col: 0 }, selection: null, file_content: "", visible_end: null, attached_anchor_ids: [], attached_files: [], attached_memories: [] },
    invariants: [],
    cwd: workspace,
    expected_policy_sha256: cancelView.session.policy.sha256,
    read_only: false,
  } });
  await answerApproval(cancelClient, cancelSessionId, "echo", "cancel-call", "tools/call");
  await waitUntil(async () => {
    const lines = await readFile(trace, "utf8").catch(() => "");
    return lines.includes("AHEAD_MCP_CANCEL_SENTINEL");
  }, "in-flight MCP tool request");
  const cancelCallsOnServer = (await readFile(trace, "utf8")).trim().split("\n").map((line) => JSON.parse(line));
  const pendingCall = cancelCallsOnServer.find((call) => call.method === "tools/call");
  assert(pendingCall?.request_id !== undefined, JSON.stringify(cancelCallsOnServer));
  assert.equal((await cancelClient.request("agent_turn_cancel", { session_id: cancelSessionId })).cancelled, true);
  await waitUntil(async () => {
    const lines = await readFile(trace, "utf8").catch(() => "");
    return lines.split("\n").some((line) => line.includes("notifications/cancelled"));
  }, "identified MCP cancellation notification");
  const cancelNotification = (await readFile(trace, "utf8"))
    .trim().split("\n").map((line) => JSON.parse(line))
    .find((call) => call.method === "notifications/cancelled");
  assert.equal(cancelNotification.params.requestId, pendingCall.request_id,
    "MCP cancellation must carry the ID of the active tools/call request");
  let cancelHistory;
  await waitUntil(async () => {
    cancelHistory = await cancelClient.request("conversation_messages_page", { session_id: cancelSessionId, before: null, limit: 10 });
    return cancelHistory.messages.some((message) => message.turn_id === cancelledTurn && message.role === "agent" && ["complete", "failed", "cancelled"].includes(message.status));
  }, "cancelled MCP turn completion");
  const cancelledAgent = cancelHistory.messages.find((message) => message.turn_id === cancelledTurn && message.role === "agent");
  assert.equal(cancelledAgent.status, "cancelled", JSON.stringify({ cancelledAgent, stderr: cancelClient.stderr }));
  assert.equal(modelRequests.length - modelScenarioStart, 2, "cancelled MCP call must not resume the model");
  await cancelClient.stop();
  passed = true;
  console.log("PASS managed MCP discovery/resources, legacy fallback, request-state retry, approvals, identified cancellation and model continuation");
} finally {
  for (const client of clients) await client.cleanup();
  model.closeAllConnections();
  await new Promise((resolve) => model.close(resolve));
  if (passed) await rm(root, { recursive: true, force: true });
  else console.error(`Disposable MCP workspace retained at ${root}`);
}
