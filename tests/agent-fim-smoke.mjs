import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";

const binary = path.resolve(process.argv[2] ?? "target/debug/ahead");
const smokeRoot = await mkdtemp(path.join(tmpdir(), "ahead-fim-smoke-"));
const workspace = path.join(smokeRoot, "project");
const profile = path.join(smokeRoot, "profile");
const home = path.join(profile, "home");
const dataHome = path.join(profile, "data");
const runtimeHome = path.join(profile, "runtime");
const sourcePath = path.join(workspace, "src", "main.rs");
const activeEditorPath = path.join(workspace, "src", "main.md");
const relatedEditorPath = path.join(workspace, "src", "helper.md");
const expectedObjective = "AHEAD_FIM_SESSION_OBJECTIVE_7bf5";
const expectedDecision = "AHEAD_FIM_DECISION_INITIAL_61d3";
const revisedDecision = "AHEAD_FIM_DECISION_REVISED_4ca8";
const secondSessionObjective = "AHEAD_FIM_SECOND_OBJECTIVE_7c21";
const expectedCallerContext = "AHEAD_FIM_CALLER_CONTEXT_2f0a";
const expectedRootInstructions = "AHEAD_FIM_ROOT_INSTRUCTIONS_91d2";
const expectedNestedInstructions = "AHEAD_FIM_NESTED_INSTRUCTIONS_6ac4";
const expectedRelatedBuffer = "AHEAD_FIM_UNSAVED_RELATED_BUFFER_48ae";
const activeBufferPrefix = "AHEAD_FIM_UNSAVED_ACTIVE_BUFFER_92e7";
const expectedPrefix = "AHEAD_FIM_PREFIX_4291";
const expectedSuffix = "AHEAD_FIM_SUFFIX_8e13";

const providerRequests = [];
let child;
let lines;
let stderr = "";
let exited = false;
const pending = new Map();
let nextId = 0;

const provider = createServer(async (request, response) => {
  try {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    providerRequests.push({
      method: request.method,
      url: request.url,
      body: JSON.parse(Buffer.concat(chunks).toString("utf8")),
    });
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({ choices: [{ text: "\nfn generated() {}\n" }] }));
  } catch (error) {
    response.writeHead(500, { "content-type": "text/plain" });
    response.end(String(error));
  }
});

async function waitUntil(predicate, description, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    assert(Date.now() < deadline, `Timed out: ${description}\n${stderr}`);
    await delay(20);
  }
}

function rpc(method, params, timeoutMs = 30000) {
  const id = ++nextId;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`Timed out: ${method}\n${stderr}`));
    }, timeoutMs);
    pending.set(id, { resolve, reject, timer, unwrapAhead: true });
    child.stdin.write(`${JSON.stringify({
      id,
      method: "ahead_request",
      params: { request: { method, params } },
    })}\n`);
  });
}

function proxyRequest(method, params, timeoutMs = 30000) {
  const id = ++nextId;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`Timed out: ${method}\n${stderr}`));
    }, timeoutMs);
    pending.set(id, { resolve, reject, timer, unwrapAhead: false });
    child.stdin.write(`${JSON.stringify({ id, method, params })}\n`);
  });
}

function startProxy() {
  const environment = {
    PATH: process.env.PATH ?? "/usr/bin:/bin:/usr/sbin:/sbin",
    HOME: home,
    TMPDIR: profile,
    LANG: process.env.LANG ?? "C.UTF-8",
    NO_PROXY: "127.0.0.1,localhost,::1",
    AHEAD_DATA_HOME: dataHome,
    AHEAD_HOME: runtimeHome,
    AHEAD_USER_HOME: home,
    RUST_LOG: "error",
  };
  child = spawn(binary, ["--proxy"], {
    cwd: workspace,
    detached: true,
    stdio: ["pipe", "pipe", "pipe"],
    env: environment,
  });
  child.stderr.on("data", (chunk) => {
    stderr = (stderr + chunk).slice(-6000);
  });
  child.once("exit", () => {
    exited = true;
    for (const waiter of pending.values()) {
      clearTimeout(waiter.timer);
      waiter.reject(new Error(`Proxy exited before replying\n${stderr}`));
    }
    pending.clear();
  });
  lines = createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    let message;
    try {
      message = JSON.parse(line);
    } catch (error) {
      stderr = `${stderr}\nInvalid proxy output: ${error}`.slice(-6000);
      return;
    }
    const waiter = pending.get(message.id);
    if (!waiter || message.method) return;
    pending.delete(message.id);
    clearTimeout(waiter.timer);
    if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
    else waiter.resolve(waiter.unwrapAhead ? message.result.params.response : message.result);
  });
  child.stdin.write(`${JSON.stringify({
    method: "initialize",
    params: { workspace, window_id: 1, tab_id: 1 },
  })}\n`);
}

async function stopProxy() {
  if (!child) return;
  if (!exited) {
    child.stdin.write(`${JSON.stringify({ method: "shutdown", params: {} })}\n`);
    try {
      await waitUntil(() => exited, "proxy shutdown");
    } catch {
      process.kill(-child.pid, "SIGKILL");
      await waitUntil(() => exited, "proxy process-group teardown", 5000);
    }
  }
  lines?.close();
  child = undefined;
  lines = undefined;
}

try {
  await Promise.all([
    mkdir(path.join(workspace, ".ahead"), { recursive: true, mode: 0o700 }),
    mkdir(path.dirname(sourcePath), { recursive: true, mode: 0o700 }),
    mkdir(home, { recursive: true, mode: 0o700 }),
    mkdir(dataHome, { recursive: true, mode: 0o700 }),
    mkdir(runtimeHome, { recursive: true, mode: 0o700 }),
  ]);
  await new Promise((resolve, reject) => {
    provider.once("error", reject);
    provider.listen(0, "127.0.0.1", resolve);
  });
  const { port } = provider.address();
  await Promise.all([
    writeFile(path.join(workspace, "AGENTS.md"), `${expectedRootInstructions}\n`, { mode: 0o600 }),
    writeFile(path.join(workspace, "src", "AGENTS.md"), `${expectedNestedInstructions}\n`, { mode: 0o600 }),
    writeFile(sourcePath, `fn main() { ${expectedPrefix}|${expectedSuffix} }\n`, { mode: 0o600 }),
    writeFile(activeEditorPath, "See helper.\nSaved active buffer.\n", { mode: 0o600 }),
    writeFile(relatedEditorPath, "Saved helper buffer.\n", { mode: 0o600 }),
    writeFile(path.join(workspace, ".ahead", "settings.toml"),
      `[ai]\nactive_connection = "Local FIM"\n\n[[ai.connections]]\nname = "Local FIM"\nprovider_id = "local-fim"\nprovider = "openai-compatible"\nbase_url = "http://127.0.0.1:${port}/v1"\nmodel = "deepseek-coder"\n`,
      { mode: 0o600 }),
  ]);

  startProxy();
  const view = await rpc("start_work", {
    work_kind: "product-change",
    title: "AHEAD_FIM_SESSION_TITLE_ee10",
    starting_point: expectedObjective,
    harness: "ahead",
  });
  const workItem = await rpc("work_item_create", {
    session_id: view.session.id,
    title: "AHEAD_FIM_DECISION_ITEM_3f9a",
  });
  await rpc("work_item_note", {
    item_id: workItem.id,
    kind: "decision",
    body_markdown: expectedDecision,
  });
  const prediction = await rpc("request_prediction", {
    request: {
      request_id: "fim-smoke-request",
      session_id: view.session.id,
      path: "src/main.rs",
      cursor: { line: 0, col: expectedPrefix.length },
      prefix: expectedPrefix,
      suffix: expectedSuffix,
      work_context: expectedCallerContext,
    },
  });

  assert.equal(prediction.request_id, "fim-smoke-request");
  assert.match(prediction.replacement, /fn generated\(\) \{\}/);
  assert.equal(providerRequests.length, 1);
  const firstProviderRequest = providerRequests[0];
  assert.equal(firstProviderRequest.method, "POST");
  assert.equal(firstProviderRequest.url, "/v1/completions");
  assert.equal(firstProviderRequest.body.model, "deepseek-coder");
  assert.equal(firstProviderRequest.body.stream, false);
  const prompt = firstProviderRequest.body.prompt;
  for (const expected of [
    "AHEAD Assist",
    "Session: AHEAD_FIM_SESSION_TITLE_ee10",
    `Objective: ${expectedObjective}`,
    expectedDecision,
    expectedCallerContext,
    expectedRootInstructions,
    expectedNestedInstructions,
    expectedPrefix,
    expectedSuffix,
    "<｜fim▁hole｜>",
  ]) {
    assert(prompt.includes(expected), `FIM provider prompt omitted ${expected}`);
  }

  const unsavedActiveBuffer = `See helper.\n${activeBufferPrefix}\n`;
  const unsavedRelatedBuffer = `${expectedRelatedBuffer}\n`;
  child.stdin.write(`${JSON.stringify({
    method: "editor_snapshot",
    params: { path: activeEditorPath, content: unsavedActiveBuffer },
  })}\n`);
  child.stdin.write(`${JSON.stringify({
    method: "editor_snapshot",
    params: { path: relatedEditorPath, content: unsavedRelatedBuffer },
  })}\n`);
  const inlinePrediction = await proxyRequest("get_inline_completions", {
    path: activeEditorPath,
    position: { line: 1, character: activeBufferPrefix.length },
    trigger_kind: 2,
    session_id: view.session.id,
    model_context: null,
  });
  assert.equal(inlinePrediction.method, "get_inline_completions");
  assert(
    inlinePrediction.params.prediction,
    "managed FIM fallback should bind the inline completion to its context snapshot",
  );
  assert(
    inlinePrediction.params.context_paths.includes("src/helper.md"),
    `inline prediction did not report its related unsaved buffer: ${JSON.stringify(inlinePrediction.params.context_paths)}`,
  );
  const inlinePrompt = providerRequests[1]?.body.prompt;
  for (const expected of [
    "Session: AHEAD_FIM_SESSION_TITLE_ee10",
    `Objective: ${expectedObjective}`,
    expectedRootInstructions,
    expectedNestedInstructions,
    activeBufferPrefix,
    "Relevant open buffer: src/helper.md",
    expectedRelatedBuffer,
    expectedDecision,
    "<｜fim▁hole｜>",
  ]) {
    assert(inlinePrompt?.includes(expected), `inline FIM provider prompt omitted ${expected}`);
  }

  await rpc("work_item_note", {
    item_id: workItem.id,
    kind: "decision",
    body_markdown: revisedDecision,
  });
  await rpc("request_prediction", {
    request: {
      request_id: "fim-smoke-revised-decision",
      session_id: view.session.id,
      path: "src/main.rs",
      cursor: { line: 0, col: expectedPrefix.length },
      prefix: expectedPrefix,
      suffix: expectedSuffix,
      work_context: expectedCallerContext,
    },
  });
  const revisedPrompt = providerRequests[2]?.body.prompt;
  assert(revisedPrompt?.includes(revisedDecision), "next FIM request omitted the updated session decision");

  const secondView = await rpc("start_work", {
    work_kind: "product-change",
    title: "AHEAD_FIM_SECOND_SESSION_TITLE_81bd",
    starting_point: secondSessionObjective,
    harness: "ahead",
  });
  await rpc("request_prediction", {
    request: {
      request_id: "fim-smoke-second-session",
      session_id: secondView.session.id,
      path: "src/main.rs",
      cursor: { line: 0, col: expectedPrefix.length },
      prefix: expectedPrefix,
      suffix: expectedSuffix,
      work_context: expectedCallerContext,
    },
  });
  const secondSessionPrompt = providerRequests[3]?.body.prompt;
  assert(secondSessionPrompt?.includes("AHEAD_FIM_SECOND_SESSION_TITLE_81bd"), "FIM prompt omitted the selected session title");
  assert(secondSessionPrompt?.includes(`Objective: ${secondSessionObjective}`), "FIM prompt omitted the selected session objective");
  for (const staleContext of [expectedObjective, expectedDecision, revisedDecision]) {
    assert(!secondSessionPrompt?.includes(staleContext), `FIM prompt leaked prior-session context: ${staleContext}`);
  }
  console.log("PASS session-bound FIM prompt, instruction hierarchy and provider round-trip");
} catch (error) {
  console.error(error?.stack ?? error);
  process.exitCode = 1;
} finally {
  await stopProxy().catch((error) => console.error(error));
  if (provider.listening) {
    await new Promise((resolve) => provider.close(resolve));
  }
  await rm(smokeRoot, { recursive: true, force: true });
}
