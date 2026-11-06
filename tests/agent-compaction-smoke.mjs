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
      response.writeHead(200, {
        "Content-Type": "application/json",
        "Content-Length": Buffer.byteLength(payload),
      });
      response.end(payload);
      return;
    }

    assert.equal(request.method, "POST", `Unexpected model request: ${request.method} ${request.url}`);
    requests.push(JSON.parse(body));
    const sequence = requests.length;
    const text = sequence === 1
      ? "Initial answer before compaction."
      : sequence === 2
        ? "AHEAD_COMPACTION_SUMMARY_SENTINEL: preserve the user's open work."
        : "Second turn completed after compaction.";
    const message = {
      id: `answer-${sequence}`,
      type: "message",
      role: "assistant",
      content: [{ type: "output_text", text }],
    };
    const events = [
      { type: "response.output_item.added", output_index: 0, item: message },
      { type: "response.output_item.done", output_index: 0, item: message },
      {
        type: "response.completed",
        // Some OpenAI-compatible providers omit usage. The runtime must still
        // estimate context from retained history before deciding to compact.
        response: { id: `response-${sequence}`, status: "completed" },
      },
    ];
    const payload = events
      .map((event) => `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`)
      .join("");
    response.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Content-Length": Buffer.byteLength(payload),
    });
    response.end(payload);
  });
});

const observedUsage = [];
const compactedTurns = new Set();
await new Promise((resolve, reject) => {
  model.once("error", reject);
  model.listen(0, "127.0.0.1", resolve);
});

const workspace = await mkdtemp(path.join(tmpdir(), "ahead-compaction-smoke-"));
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
for (const directory of [
  smokeEnv.HOME,
  smokeEnv.AHEAD_DATA_HOME,
  smokeEnv.AHEAD_HOME,
  smokeEnv.AHEAD_USER_HOME,
  path.join(workspace, ".ahead"),
]) {
  await mkdir(directory, { recursive: true });
}

await writeFile(
  path.join(workspace, ".ahead", "settings.toml"),
  `[ai]\nactive_connection = "Mock"\n[[ai.connections]]\nname = "Mock"\nprovider_id = "mock"\nbase_url = "http://127.0.0.1:${model.address().port}/v1"\nmodel = "gpt-5.6-sol"\n`,
);
const workspaceInstructions = path.join(workspace, "AGENTS.md");
await writeFile(workspaceInstructions, "Short disposable project instructions.\n");

let child;
let lines;
let stderr = "";
let exited = false;
let exitResult;
let nextId = 0;
const pending = new Map();

function rpc(method, params) {
  const id = ++nextId;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`Request timed out: ${method}\n${stderr}`));
    }, 30_000);
    pending.set(id, { resolve, reject, timer });
    child.stdin.write(JSON.stringify({
      id,
      method: "ahead_request",
      params: { request: { method, params } },
    }) + "\n");
  });
}

function startProxy() {
  child = spawn(binary, ["--proxy"], {
    cwd: workspace,
    detached: true,
    stdio: ["pipe", "pipe", "pipe"],
    env: smokeEnv,
  });
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
    let message;
    try {
      message = JSON.parse(line);
    } catch (error) {
      for (const waiter of pending.values()) {
        clearTimeout(waiter.timer);
        waiter.reject(error);
      }
      pending.clear();
      return;
    }
    const notification = message.params?.notification;
    if (message.method === "ahead_notification" && notification) {
      if (notification.method === "agent_usage") {
        observedUsage.push(notification.params);
      } else if (notification.method === "agent_context_compacted") {
        compactedTurns.add(notification.params?.turn_id);
      }
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
    params: { workspace, window_id: 1, tab_id: 1 },
  }) + "\n");
}

async function waitUntil(predicate, description) {
  const deadline = Date.now() + 60_000;
  while (!await predicate()) {
    assert(Date.now() < deadline, `Timed out: ${description}\n${stderr}`);
    await delay(30);
  }
}

let passed = false;
let history;
try {
  startProxy();
  const view = await rpc("start_work", {
    work_kind: "product-change",
    title: "Automatic compaction fixture",
    starting_point: "Exercise the model-derived context threshold",
    harness: "ahead",
  });
  const session_id = view.session.id;
  const request = {
    session_id,
    thread_id: "compaction-fixture",
    harness: "ahead",
    model: "gpt-5.6-sol",
    model_provider: "mock",
    model_context_window: 10_000,
    session_context: "Disposable automatic compaction smoke test",
    context: {
      active_path: "",
      caret: { line: 0, col: 0 },
      selection: null,
      file_content: "",
      visible_end: null,
      attached_anchor_ids: [],
      attached_files: [],
      attached_memories: [],
    },
    invariants: [],
    cwd: workspace,
    expected_policy_sha256: view.session.policy.sha256,
    read_only: true,
  };

  const firstTurn = await rpc("agent_turn_start", {
    request: { ...request, user_message: "Remember the first disposable turn." },
  });
  const page = () => rpc("conversation_messages_page", {
    session_id,
    before: null,
    limit: 20,
  });
  await waitUntil(async () => {
    history = await page();
    return history.messages.some((message) =>
      message.turn_id === firstTurn.turn_id
      && message.role === "agent"
      && ["complete", "failed", "cancelled"].includes(message.status));
  }, "first managed turn");
  assert.equal(
    history.messages.find((message) => message.turn_id === firstTurn.turn_id && message.role === "agent").status,
    "complete",
    JSON.stringify({ history, requests: requests.length, stderr }),
  );
  assert.equal(requests.length, 1, "the first turn should make one model request");
  await waitUntil(
    () => observedUsage.some((update) =>
      update?.turn_id === firstTurn.turn_id
      && update.usage?.used_tokens > 0
      && update.usage?.context_window === 9_500),
    "estimated context usage after a provider response without usage metadata",
  );
  await writeFile(
    workspaceInstructions,
    `Updated project instructions.\n${"context ".repeat(6_000)}`,
  );
  const secondMessage = "What should the next turn remember?";
  const secondTurn = await rpc("agent_turn_start", {
    request: { ...request, user_message: secondMessage },
  });
  await waitUntil(async () => {
    history = await page();
    return history.messages.some((message) =>
      message.turn_id === secondTurn.turn_id
      && message.role === "agent"
      && ["complete", "failed", "cancelled"].includes(message.status));
  }, "second managed turn after compaction");
  assert.equal(
    history.messages.find((message) => message.turn_id === secondTurn.turn_id && message.role === "agent").status,
    "complete",
    JSON.stringify({ history, requests: requests.length, stderr }),
  );
  assert.equal(requests.length, 3, "the second turn should compact, then sample the model");
  await waitUntil(
    () => compactedTurns.has(secondTurn.turn_id),
    "compaction notification for the second turn",
  );
  assert(
    JSON.stringify(requests[1]).includes("CONTEXT CHECKPOINT COMPACTION"),
    "the second provider request should be the retained runtime's compaction prompt",
  );
  assert(
    JSON.stringify(requests[2]).includes("AHEAD_COMPACTION_SUMMARY_SENTINEL"),
    "the compacted summary should be included in the second turn's model context",
  );
  assert(
    JSON.stringify(requests[2]).includes(secondMessage),
    "the second user message should be sent after compaction",
  );
  assert(
    JSON.stringify(requests[2]).includes("Updated project instructions."),
    "the refreshed workspace instructions should reach the post-compaction request",
  );

  child.stdin.write(JSON.stringify({ method: "shutdown", params: {} }) + "\n");
  await waitUntil(() => exited, "proxy shutdown");
  assert.deepEqual(exitResult, { code: 0, signal: null }, stderr);
  passed = true;
  console.log("PASS estimated context usage, effective model limit, automatic compaction, summary persistence, and shutdown");
} finally {
  if (!exited && child?.pid) process.kill(-child.pid, "SIGKILL");
  lines?.close();
  model.closeAllConnections();
  await new Promise((resolve) => model.close(resolve));
  if (passed) {
    await rm(workspace, { recursive: true });
    await rm(profile, { recursive: true });
  } else {
    console.error(`Disposable failure project retained at ${workspace} with profile ${profile}\n${JSON.stringify({ requests: requests.length, history, stderr })}`);
  }
}
