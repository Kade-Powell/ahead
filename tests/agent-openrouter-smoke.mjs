import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { randomBytes } from "node:crypto";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";

const binary = path.resolve(process.argv[2] ?? "target/debug/ahead");
const apiKey = process.env.OPENROUTER_API_KEY;
assert(apiKey, "Set OPENROUTER_API_KEY to run this live-provider smoke");

const root = await mkdtemp(path.join(tmpdir(), "ahead-openrouter-smoke-"));
const workspace = path.join(root, "project");
const profile = path.join(root, "profile");
const userHome = path.join(profile, "user");
const dataHome = path.join(profile, "data");
const runtimeHome = path.join(profile, "runtime");
const cacheHome = path.join(profile, "cache");
const configHome = path.join(profile, "config");
const model = process.env.AHEAD_OPENROUTER_MODEL
  ?? "nvidia/nemotron-3-ultra-550b-a55b:free";
const providerId = "openrouter";
const apiKeySetting = JSON.stringify(apiKey);
const originalSource = "value = 7\n";
const editedSource = "value = 8\n";
const expectedPrefix = "AHEAD_OPENROUTER_SMOKE_TOKEN";
const expectedToken = randomBytes(18).toString("hex");
const markerPath = path.join(workspace, "smoke-data.txt");
const workspaceInstructions = path.join(workspace, "AGENTS.md");
const sourcePath = path.join(workspace, "src", "main.py");
const prompt = `This is a synthetic integration test. Use managed Code Mode tools to call file_search for the exact text ${expectedPrefix} and read the matching line. Then use the nested apply_patch tool to change only src/main.py from "value = 7" to "value = 8". Do not use a shell, exec_command, or another tool to edit files. Do not modify any other file. Reply with only the random token found by file_search.`;

const { OPENROUTER_API_KEY: _apiKey, ...inheritedEnvironment } = process.env;
const smokeEnvironment = {
  PATH: inheritedEnvironment.PATH,
  LANG: inheritedEnvironment.LANG,
  LC_ALL: inheritedEnvironment.LC_ALL,
  HTTPS_PROXY: inheritedEnvironment.HTTPS_PROXY,
  HTTP_PROXY: inheritedEnvironment.HTTP_PROXY,
  ALL_PROXY: inheritedEnvironment.ALL_PROXY,
  NO_PROXY: inheritedEnvironment.NO_PROXY,
  HOME: userHome,
  TMPDIR: profile,
  AHEAD_DATA_HOME: dataHome,
  AHEAD_HOME: runtimeHome,
  AHEAD_USER_HOME: userHome,
  GIT_CONFIG_GLOBAL: path.join(root, "gitconfig"),
  GIT_CONFIG_NOSYSTEM: "1",
  GIT_COMMITTER_NAME: "Human Developer",
  GIT_COMMITTER_EMAIL: "human@example.invalid",
  RUST_LOG: "error",
};

let child;
let lines;
let stderr = "";
let exited = false;
let exitResult;
const pending = new Map();
const notificationReplies = [];
const notificationReplyErrors = [];
const observedNotifications = [];
let nextId = 0;
function sanitize(value) {
  return String(value).replaceAll(apiKey, "[REDACTED]");
}
function rpc(method, params, transport = "ahead") {
  const id = ++nextId;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`Timed out: ${method}\n${stderr}`));
    }, 180_000);
    pending.set(id, { resolve, reject, timer, transport });
    const request = transport === "proxy"
      ? { id, method, params }
      : { id, method: "ahead_request", params: { request: { method, params } } };
    child.stdin.write(JSON.stringify(request) + "\n");
  });
}

async function waitForTurnMessage(sessionId, turnId, predicate, description) {
  const deadline = Date.now() + 120_000;
  while (Date.now() < deadline) {
    const page = await rpc("conversation_messages_page", {
      session_id: sessionId,
      before: null,
      limit: 12,
    });
    const message = page.messages.find((item) =>
      item.turn_id === turnId && item.role === "agent");
    if (message && predicate(message)) return message;
    if (message && ["complete", "failed", "cancelled"].includes(message.status)) {
      throw new Error(`${description}: turn ended as ${message.status} before the expected state`);
    }
    await delay(100);
  }
  throw new Error(`${description}: timed out waiting for provider output`);
}

try {
  await Promise.all([
    mkdir(workspace, { recursive: true, mode: 0o700 }),
    mkdir(path.join(userHome, ".ahead"), { recursive: true, mode: 0o700 }),
    mkdir(dataHome, { recursive: true, mode: 0o700 }),
    mkdir(runtimeHome, { recursive: true, mode: 0o700 }),
    mkdir(cacheHome, { recursive: true, mode: 0o700 }),
    mkdir(configHome, { recursive: true, mode: 0o700 }),
    mkdir(path.dirname(sourcePath), { recursive: true, mode: 0o700 }),
  ]);
  await writeFile(path.join(userHome, ".ahead", "settings.toml"),
    `[ai]\nactive_connection = "OpenRouter"\n[[ai.connections]]\nname = "OpenRouter"\nprovider_id = "${providerId}"\nbase_url = "https://openrouter.ai/api/v1"\nmodel = "${model}"\napi_key = ${apiKeySetting}\n`,
    { mode: 0o600 });
  await writeFile(path.join(dataHome, "trusted-workspaces.toml"),
    `trusted = [${JSON.stringify(workspace)}]\nblocked = []\n`,
    { mode: 0o600 });
  await writeFile(markerPath, `${expectedPrefix}_${expectedToken}\n`,
    { mode: 0o600 });
  await writeFile(sourcePath, originalSource, { mode: 0o600 });
  await writeFile(smokeEnvironment.GIT_CONFIG_GLOBAL, "", { mode: 0o600 });
  const git = (args) => {
    const result = spawnSync("git", args, {
      cwd: workspace,
      env: smokeEnvironment,
      encoding: "utf8",
    });
    assert.equal(result.status, 0, `git ${args.join(" ")} failed: ${result.stderr}`);
    return result.stdout.trim();
  };
  git(["init", "--quiet"]);
  git(["config", "user.name", "Human Developer"]);
  git(["config", "user.email", "human@example.invalid"]);
  git(["add", "src/main.py", "smoke-data.txt"]);
  git(["commit", "--quiet", "-m", "Disposable smoke baseline"]);

  child = spawn(binary, ["--proxy"], {
    cwd: workspace,
    detached: true,
    stdio: ["pipe", "pipe", "pipe"],
    env: smokeEnvironment,
  });
  child.stderr.on("data", (chunk) => {
    stderr = sanitize((stderr + chunk).slice(-4000));
  });
  child.once("exit", (code, signal) => {
    exited = true;
    exitResult = { code, signal };
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
    } catch {
      return;
    }
    const notification = message.params?.notification;
    if (message.method === "ahead_notification" && notification) {
      const requestQuestions = notification.params?.request?.questions;
      const call = notification.params?.call;
      observedNotifications.push({
        method: notification.method,
        turnId: notification.params?.turn_id,
        call: call ? { title: call.title, kind: call.kind, status: call.status } : undefined,
        questions: Array.isArray(requestQuestions)
          ? requestQuestions.map((question) => ({
            id: question.id,
            header: question.header,
            options: question.options?.map((option) => option.label),
          }))
          : undefined,
      });
      if (observedNotifications.length > 50) observedNotifications.shift();
    }
    if (message.method === "ahead_notification"
      && notification?.method === "agent_buffer_snapshots_requested") {
      notificationReplies.push(readFile(sourcePath, "utf8")
        .then((content) => rpc("agent_buffer_snapshots_response", {
          ...notification.params,
          buffers: [{ path: "src/main.py", content, symbols: [] }],
        }))
        .catch((error) => notificationReplyErrors.push(sanitize(error))));
      return;
    }
    const waiter = pending.get(message.id);
    if (!waiter || message.method) return;
    pending.delete(message.id);
    clearTimeout(waiter.timer);
    if (message.error) waiter.reject(new Error(sanitize(JSON.stringify(message.error))));
    else waiter.resolve(waiter.transport === "proxy"
      ? message.result
      : message.result.params.response);
  });
  child.stdin.write(JSON.stringify({
    method: "initialize",
    params: { workspace, window_id: 1, tab_id: 1 },
  }) + "\n");

  const view = await rpc("start_work", {
    work_kind: "product-change",
    title: "OpenRouter provider smoke",
    starting_point: "Verify one managed provider turn",
    harness: "ahead",
  });
  const sessionId = view.session.id;
  const turnRequest = {
    session_id: sessionId,
    thread_id: "openrouter-provider-smoke",
    harness: "ahead",
    model,
    model_provider: providerId,
    user_message: prompt,
    session_context: "Disposable synthetic OpenRouter acceptance smoke.",
    context: {
      active_path: "src/main.py",
      caret: { line: 0, col: 0 },
      selection: null,
      file_content: originalSource,
      visible_end: null,
      attached_anchor_ids: [],
      attached_files: [],
      attached_memories: [],
    },
    invariants: [],
    cwd: workspace,
    expected_policy_sha256: view.session.policy.sha256,
    read_only: false,
  };
  const { turn_id: turnId } = await rpc("agent_turn_start", { request: turnRequest });

  const deadline = Date.now() + 180_000;
  let sawStreaming = false;
  let agentMessage;
  let lastProgressAt = Date.now();
  let latestRuntimeState;
  while (Date.now() < deadline) {
    const page = await rpc("conversation_messages_page", {
      session_id: sessionId,
      before: null,
      limit: 12,
    });
    agentMessage = page.messages.find((message) =>
      message.turn_id === turnId && message.role === "agent");
    sawStreaming ||= agentMessage?.status === "streaming";
    if (agentMessage && ["complete", "failed", "cancelled"].includes(agentMessage.status)) {
      break;
    }
    if (Date.now() - lastProgressAt >= 15_000) {
      latestRuntimeState = await rpc("agent_runtime_state", { session_id: sessionId });
      const toolCalls = (latestRuntimeState?.tool_calls ?? []).map(({ title, kind, status }) => ({
        title,
        kind,
        status,
      }));
      console.log(`OPENROUTER_PROGRESS ${sanitize(JSON.stringify({
        status: agentMessage?.status,
        contentLength: agentMessage?.content?.length ?? 0,
        notifications: observedNotifications,
        toolCalls,
      }))}`);
      lastProgressAt = Date.now();
    }
    await delay(200);
  }
  assert(agentMessage, "Managed OpenRouter turn did not create an agent message");
  assert.equal(
    agentMessage.status,
    "complete",
    sanitize(JSON.stringify({
      status: agentMessage.status,
      content: agentMessage.content,
      stderr,
      notifications: observedNotifications,
      toolCalls: (latestRuntimeState?.tool_calls ?? []).map(({ title, kind, status }) => ({
        title,
        kind,
        status,
      })),
    })),
  );
  await Promise.all(notificationReplies);
  assert.deepEqual(notificationReplyErrors, [],
    "Synthetic editor did not answer every buffer-snapshot request");
  assert(sawStreaming, "Managed OpenRouter answer was never observed in streaming state");
  assert(agentMessage.content.includes(expectedToken),
    "Final answer did not contain the synthetic value returned by managed file_search");
  assert.equal(await readFile(sourcePath, "utf8"), editedSource,
    "Managed OpenRouter turn did not apply the requested source edit");
  const runtimeState = await rpc("agent_runtime_state", { session_id: sessionId });
  const observedToolCalls = (runtimeState?.tool_calls ?? []).map(({ title, kind, status }) => ({
    tool: kind === "commandExecution" ? "shell" : title,
    kind,
    status,
  }));
  assert(observedToolCalls.some(({ tool, kind, status }) =>
    tool === "file_search" && kind === "dynamicToolCall" && status === "completed"),
  `OpenRouter did not complete managed file_search: ${JSON.stringify(observedToolCalls)}`);
  assert(!observedToolCalls.some(({ kind }) => kind === "commandExecution"),
    `OpenRouter bypassed the managed tool path with shell commands: ${JSON.stringify(observedToolCalls)}`);
  assert(observedToolCalls.some(({ kind, status }) =>
    kind === "fileChange" && status === "completed"),
  `OpenRouter did not complete the managed file-change tool: ${JSON.stringify(observedToolCalls)}`);
  const anchors = await rpc("list_anchors_for_paths", { paths: ["src/main.py"] });
  assert.equal(anchors.length, 1,
    `Expected one persisted CodeAnchor; observed tool calls: ${JSON.stringify(observedToolCalls)}`);
  assert.equal(anchors[0].session_id, sessionId);
  assert.equal(anchors[0].actor_id, "ahead");
  assert.equal(anchors[0].path, "src/main.py");
  await rpc("git_stage_paths", { paths: ["src/main.py"], staged: true }, "proxy");
  await rpc("git_commit", { message: "Commit authenticated AHEAD edit", diffs: [] }, "proxy");
  assert.equal(git(["show", "-s", "--format=%an", "HEAD"]), "ahead");
  assert.equal(git(["show", "-s", "--format=%cn", "HEAD"]), "Human Developer");
  assert(
    git(["show", "-s", "--format=%B", "HEAD"]).split(/\r?\n/).includes(`Ahead-Session: ${sessionId}`),
    "Commit omitted the matching AHEAD session trailer",
  );
  assert.equal((await rpc("list_anchors_for_paths", { paths: ["src/main.py"] })).length, 0,
    "Committed CodeAnchor was not cleared");

  await writeFile(
    workspaceInstructions,
    `Updated project instructions.\n${"context ".repeat(6_000)}`,
    { mode: 0o600 },
  );
  const compactionTurn = await rpc("agent_turn_start", {
    request: {
      ...turnRequest,
      model_context_window: 10_000,
      user_message: "Reply exactly AHEAD_OPENROUTER_COMPACTION_OK.",
    },
  });
  const compactedMessage = await waitForTurnMessage(
    sessionId,
    compactionTurn.turn_id,
    (message) => message.status === "complete",
    "Authenticated automatic compaction turn",
  );
  assert(compactedMessage.content.includes("AHEAD_OPENROUTER_COMPACTION_OK"),
    "Authenticated turn did not complete after compaction");
  assert(observedNotifications.some((notification) =>
    notification.method === "agent_context_compacted"
      && notification.turnId === compactionTurn.turn_id),
  "Authenticated turn did not emit its automatic compaction event");

  const cancellationRequest = {
    ...turnRequest,
    user_message: "For this cancellation smoke, write a very long numbered plain-text list. Do not call any tools. Begin immediately and keep generating as many entries as you can.",
    context: { ...turnRequest.context, file_content: editedSource },
  };
  const { turn_id: cancelledTurnId } = await rpc("agent_turn_start", {
    request: cancellationRequest,
  });
  await waitForTurnMessage(
    sessionId,
    cancelledTurnId,
    (message) => message.status === "streaming" && message.content.length > 0,
    "Authenticated cancellation turn",
  );
  assert.equal((await rpc("agent_turn_cancel", { session_id: sessionId })).cancelled, true);
  const cancelledMessage = await waitForTurnMessage(
    sessionId,
    cancelledTurnId,
    (message) => message.status === "cancelled",
    "Authenticated cancellation",
  );
  assert.equal(cancelledMessage.status, "cancelled");

  const { turn_id: retriedTurnId } = await rpc("agent_turn_retry", {
    session_id: sessionId,
    turn_id: cancelledTurnId,
  });
  assert.notEqual(retriedTurnId, cancelledTurnId);
  await waitForTurnMessage(
    sessionId,
    retriedTurnId,
    (message) => message.status === "streaming" && message.content.length > 0,
    "Authenticated retried turn",
  );
  assert.equal((await rpc("agent_turn_cancel", { session_id: sessionId })).cancelled, true);
  const retriedMessage = await waitForTurnMessage(
    sessionId,
    retriedTurnId,
    (message) => message.status === "cancelled",
    "Authenticated retried cancellation",
  );
  assert.equal(retriedMessage.status, "cancelled");

  child.stdin.write(JSON.stringify({ method: "shutdown", params: {} }) + "\n");
  const shutdownDeadline = Date.now() + 10_000;
  while (!exited && Date.now() < shutdownDeadline) await delay(20);
  assert(exited, "Proxy did not shut down cleanly");
  assert.deepEqual(exitResult, { code: 0, signal: null }, stderr);
  console.log("PASS authenticated OpenRouter managed edit, Turso CodeAnchor and GitCommit; compaction, streaming, cancellation and retry");
} catch (error) {
  process.exitCode = 1;
  console.error(sanitize(error?.stack ?? error));
} finally {
  if (child && !exited && child.pid) {
    try {
      process.kill(-child.pid, "SIGKILL");
    } catch {}
  }
  lines?.close();
  await rm(root, { recursive: true, force: true });
}
