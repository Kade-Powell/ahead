import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";

const binary = path.resolve(process.argv[2] ?? "target/debug/ahead");
const original = "# AHEAD_ANCHOR_NEEDLE\nprint('before')\n";
const codeModePatch = "*** Begin Patch\n*** Update File: src/main.py\n@@\n # AHEAD_ANCHOR_NEEDLE\n-print('before')\n+print('AHEAD_ANCHOR_EDIT')\n*** End Patch";
const directOriginal = "print('before')\n";
const directPatch = "*** Begin Patch\n*** Update File: src/direct.py\n@@\n-print('before')\n+print('AHEAD_FUNCTION_PATCH')\n*** End Patch";
const rootInstruction = "AHEAD_ROOT_AGENTS_MARKER";
const sourceInstruction = "AHEAD_SOURCE_AGENTS_MARKER";
const unrelatedInstruction = "AHEAD_UNRELATED_AGENTS_MARKER";
const projectSkillBody = "AHEAD_PROJECT_SKILL_BODY_SENTINEL";
const requests = [];
let responseCount = 0;
const model = createServer((request, response) => {
  let body = "";
  request.setEncoding("utf8");
  request.on("data", (chunk) => { body += chunk; });
  request.on("end", () => {
    if (request.method === "GET" && request.url?.split("?")[0].endsWith("/models")) {
      // The code-mode connection reports no provider models so AHEAD resolves the
      // slug from its own catalog (Code Mode); the direct connection reports a
      // provider model, which uses the standard function-schema tool list.
      const directConnection = request.url.startsWith("/direct/");
      const payload = JSON.stringify(directConnection
        ? {
          object: "list",
          data: [{ id: "mock-tool-capable", supports_tools: true, supported_parameters: ["tools"] }],
        }
        : { models: [] });
      response.writeHead(200, { "Content-Type": "application/json", "Content-Length": Buffer.byteLength(payload) });
      response.end(payload);
      return;
    }
    assert.equal(request.method, "POST", `Unexpected model request: ${request.method} ${request.url}`);
    requests.push(JSON.parse(body));
    responseCount++;
    const code = `const result = await tools.apply_patch(${JSON.stringify(codeModePatch)}); text(result);`;
    const item = responseCount === 1
      ? { id: "shell-source-write", type: "function_call", call_id: "shell-source-write", name: "exec_command", arguments: JSON.stringify({ cmd: "touch src/SHELL_WRITE_SHOULD_BE_DENIED" }) }
      : responseCount === 2
        ? { id: "shell-generated-write", type: "function_call", call_id: "shell-generated-write", name: "exec_command", arguments: JSON.stringify({ cmd: "cargo check --locked --offline" }) }
        : responseCount === 3
          ? { id: "anchor-patch", type: "custom_tool_call", call_id: "anchor-patch", name: "exec", input: code }
          : responseCount === 5
            ? { id: "direct-function-patch", type: "function_call", call_id: "direct-function-patch", name: "apply_patch", arguments: JSON.stringify({ patch: directPatch }) }
          : { id: "anchor-answer", type: "message", role: "assistant", content: [{ type: "output_text", text: "Updated main.py." }] };
    const events = [
      { type: "response.output_item.added", output_index: 0, item },
      { type: "response.output_item.done", output_index: 0, item },
      { type: "response.completed", response: { id: `anchor-response-${responseCount}`, end_turn: responseCount === 4 || responseCount === 6 } },
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

const smokeParent = path.resolve(process.env.AHEAD_ATTRIBUTION_SMOKE_ROOT ?? "target");
await mkdir(smokeParent, { recursive: true });
const workspace = await mkdtemp(path.join(smokeParent, "ahead-file-attribution-"));
const cargoPath = spawnSync("rustup", ["which", "cargo"], { encoding: "utf8" });
assert.equal(cargoPath.status, 0, `Could not locate the installed Cargo toolchain: ${cargoPath.stderr}`);
const toolchainDirectory = path.dirname(cargoPath.stdout.trim());
const profile = `${workspace}-profile`;
const home = path.join(profile, "home");
const gitConfig = path.join(workspace, "gitconfig");
await writeFile(gitConfig, "");
const smokeEnv = {
  PATH: [toolchainDirectory, process.env.PATH ?? "/usr/bin:/bin:/usr/sbin:/sbin"].join(path.delimiter),
  HOME: home,
  CARGO_HOME: path.join(workspace, "target", "cargo-home"),
  RUSTC: path.join(toolchainDirectory, "rustc"),
  TMPDIR: tmpdir(),
  LANG: process.env.LANG,
  LC_ALL: process.env.LC_ALL,
  RUST_LOG: "error",
  GIT_CONFIG_GLOBAL: gitConfig,
  GIT_CONFIG_NOSYSTEM: "1",
  GIT_COMMITTER_NAME: "Human Developer",
  GIT_COMMITTER_EMAIL: "human@example.invalid",
  AHEAD_DATA_HOME: path.join(profile, "data"),
  AHEAD_HOME: path.join(profile, "runtime"),
  AHEAD_USER_HOME: path.join(profile, "user"),
};
for (const directory of [home, smokeEnv.AHEAD_DATA_HOME, smokeEnv.AHEAD_HOME, smokeEnv.AHEAD_USER_HOME]) {
  await mkdir(directory, { recursive: true });
}
await mkdir(path.join(workspace, ".ahead"));
await mkdir(path.join(workspace, "src"));
await mkdir(path.join(workspace, "data"));
await mkdir(path.join(workspace, "target"));
await writeFile(path.join(workspace, "AGENTS.md"), rootInstruction);
await writeFile(path.join(workspace, "src", "AGENTS.md"), sourceInstruction);
await writeFile(path.join(workspace, "data", "AGENTS.md"), unrelatedInstruction);
const skillDirectory = path.join(workspace, ".agents", "skills", "calendar");
await mkdir(skillDirectory, { recursive: true });
await writeFile(path.join(skillDirectory, "SKILL.md"),
  `---\nname: calendar\ndescription: Disposable attribution smoke skill\n---\n${projectSkillBody}\n`);
await writeFile(path.join(workspace, "src", "main.py"), original);
await writeFile(path.join(workspace, "src", "direct.py"), directOriginal);
await writeFile(path.join(workspace, "Cargo.toml"),
  "[package]\nname = \"ahead-attribution-smoke\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n");
await writeFile(path.join(workspace, "Cargo.lock"),
  "version = 3\n\n[[package]]\nname = \"ahead-attribution-smoke\"\nversion = \"0.1.0\"\n");
await writeFile(path.join(workspace, "src", "lib.rs"), "pub fn smoke() -> u32 { 42 }\n");
const git = (args) => {
  const result = spawnSync("git", args, {
    cwd: workspace,
    env: smokeEnv,
    encoding: "utf8",
  });
  assert.equal(result.status, 0, `git ${args.join(" ")} failed: ${result.stderr}`);
  return result.stdout.trim();
};
git(["init", "--quiet"]);
git(["config", "user.name", "Human Developer"]);
git(["config", "user.email", "human@example.invalid"]);
await writeFile(path.join(workspace, ".ahead", "settings.toml"),
  `[ai]\nactive_connection = "Mock"\n[[ai.connections]]\nname = "Mock"\nprovider_id = "mock"\nbase_url = "http://127.0.0.1:${model.address().port}/v1"\nmodel = "gpt-5.6-sol"\n[[ai.connections]]\nname = "Direct"\nprovider_id = "mock-direct"\nbase_url = "http://127.0.0.1:${model.address().port}/direct/v1"\nmodel = "mock-tool-capable"\n`);

let child;
let lines;
let stderr = "";
let exited = false;
let exitResult;
let nextId = 0;
const pending = new Map();
const notifications = [];
const snapshotReplies = [];
const snapshotReplyErrors = [];
let activeBufferSnapshot = { path: "src/main.py", content: original };
function rpc(method, params, transport = "ahead") {
  const id = ++nextId;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`Request timed out: ${method}\n${stderr}`));
    }, 30000);
    pending.set(id, { resolve, reject, timer, transport });
    const request = transport === "proxy"
      ? { id, method, params }
      : { id, method: "ahead_request", params: { request: { method, params } } };
    child.stdin.write(JSON.stringify(request) + "\n");
  });
}
function startProxy() {
  child = spawn(binary, ["--proxy"], { cwd: workspace, detached: true, stdio: ["pipe", "pipe", "pipe"], env: smokeEnv });
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
    const notification = message.params?.notification;
    if (message.method === "ahead_notification"
      && notification?.method === "agent_buffer_snapshots_requested") {
      snapshotReplies.push(rpc("agent_buffer_snapshots_response", {
        ...notification.params,
        buffers: [{ ...activeBufferSnapshot, symbols: [] }],
      }).catch((error) => snapshotReplyErrors.push(error)));
      return;
    }
    if (message.method) {
      notifications.push(message);
      return;
    }
    const waiter = pending.get(message.id);
    if (!waiter) return;
    pending.delete(message.id);
    clearTimeout(waiter.timer);
    if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
    else waiter.resolve(waiter.transport === "proxy"
      ? message.result
      : message.result.params.response);
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
let history;
let runtimeState;
try {
  startProxy();
  const view = await rpc("start_work", {
    work_kind: "product-change", title: "File attribution fixture", starting_point: "Edit one line", harness: "ahead",
  });
  const session_id = view.session.id;
  const request = {
    session_id,
    thread_id: "file-attribution-fixture",
    harness: "ahead",
    model: "gpt-5.6-sol",
    model_provider: "mock",
    user_message: "/calendar Replace the second line in src/main.py with print('AHEAD_ANCHOR_EDIT').",
    session_context: "Disposable file-attribution test",
    context: { active_path: "src/main.py", caret: { line: 1, col: 0 }, selection: null, file_content: original, visible_end: null, attached_anchor_ids: [], attached_files: [], attached_memories: [] },
    invariants: [],
    cwd: workspace,
    expected_policy_sha256: view.session.policy.sha256,
    read_only: false,
  };
  await rpc("agent_turn_start", { request });
  await waitUntil(async () => {
    history = await rpc("conversation_messages_page", { session_id, before: null, limit: 10 });
    return history.messages.some((message) => message.role === "agent" && ["complete", "failed", "cancelled"].includes(message.status));
  }, "managed file edit turn");
  const agent = history.messages.find((message) => message.role === "agent");
  assert.equal(agent.status, "complete", JSON.stringify({ agent, requests: requests.length, stderr }));
  assert.equal(requests.length, 4, "both shell probes and the patch should each be followed by a model continuation");
  const firstRequest = JSON.stringify(requests[0]);
  assert(firstRequest.includes(rootInstruction), "root AGENTS.md should reach the managed model turn");
  assert(firstRequest.includes(sourceInstruction), "src/AGENTS.md should reach the managed model turn");
  assert(!firstRequest.includes(unrelatedInstruction), "non-ancestor data/AGENTS.md must not reach the model turn");
  assert(firstRequest.includes(projectSkillBody), "a selected .agents/skills skill body should reach the managed model turn");
  const tools = requests[0].tools ?? requests[0].input.filter((item) => item.type === "additional_tools").flatMap((item) => item.tools ?? []);
  const toolNames = tools.flatMap((tool) => tool.type === "namespace" ? tool.tools.map((nested) => nested.name) : [tool.name]).filter(Boolean);
  assert(toolNames.includes("exec"), `Code Mode exec was not advertised: ${JSON.stringify(toolNames)}`);
  assert(JSON.stringify(requests[0]).includes("apply_patch"), "the model request omitted the apply_patch contract");
  await assert.rejects(readFile(path.join(workspace, "src", "SHELL_WRITE_SHOULD_BE_DENIED")),
    "managed shell unexpectedly wrote into the workspace");
  const shellOutputs = requests.flatMap(({ input }) => input
    .filter((item) => item.type === "function_call_output"));
  const sourceWriteOutput = String(shellOutputs.find(({ call_id }) =>
    call_id === "shell-source-write")?.output ?? "");
  const generatedWriteOutput = String(shellOutputs.find(({ call_id }) =>
    call_id === "shell-generated-write")?.output ?? "");
  assert.match(sourceWriteOutput, /Operation not permitted/,
    `Expected shell source writes to be denied: ${sourceWriteOutput.slice(0, 500)}`);
  assert.match(generatedWriteOutput, /Process exited with code 0/,
    `Expected generated-output writes to remain available for builds/tests: ${generatedWriteOutput.slice(0, 500)}`);
  assert.match(generatedWriteOutput, /Finished `dev` profile/,
    `Expected cargo check to finish inside the managed shell: ${generatedWriteOutput.slice(0, 500)}`);
  await readFile(path.join(workspace, "target", ".rustc_info.json"));
  runtimeState = await rpc("agent_runtime_state", { session_id });
  const commandCalls = (runtimeState?.tool_calls ?? []).filter(({ kind }) => kind === "commandExecution");
  assert.equal(commandCalls.length, 2,
    `Expected both shell calls in session state: ${JSON.stringify(runtimeState?.tool_calls ?? [])}`);
  const deniedSourceCall = commandCalls.find(({ title }) => title.includes("SHELL_WRITE_SHOULD_BE_DENIED"));
  assert(deniedSourceCall?.status === "failed",
    `Expected the denied source write to be visible as failed: ${JSON.stringify(commandCalls)}`);
  const generatedOutputCall = commandCalls.find(({ title }) => title.includes("cargo check"));
  assert(generatedOutputCall?.status === "completed",
    `Expected the generated-output write to be visible as completed: ${JSON.stringify(commandCalls)}`);
  assert((runtimeState?.tool_calls ?? []).some(({ kind, status }) => kind === "fileChange" && status === "completed"),
    `Expected the follow-up patch event: ${JSON.stringify(runtimeState?.tool_calls ?? [])}`);
  assert.equal(await readFile(path.join(workspace, "src", "main.py"), "utf8"), "# AHEAD_ANCHOR_NEEDLE\nprint('AHEAD_ANCHOR_EDIT')\n");
  const anchors = await rpc("list_anchors_for_paths", { paths: ["src/main.py"] });
  assert.equal(anchors.length, 1, `Expected one persisted CodeAnchor: ${JSON.stringify(anchors)}`);
  assert.equal(anchors[0].session_id, session_id);
  assert.equal(anchors[0].actor_id, "ahead");
  assert.equal(anchors[0].path, "src/main.py");
  assert.equal(anchors[0].surrounding_context, "print('AHEAD_ANCHOR_EDIT')");
  await rpc("git_stage_paths", { paths: ["src/main.py"], staged: true }, "proxy");
  await rpc("git_commit", { message: "Commit AHEAD-authored edit", diffs: [] }, "proxy");
  assert.equal(git(["show", "-s", "--format=%an", "HEAD"]), "ahead");
  assert.equal(git(["show", "-s", "--format=%cn", "HEAD"]), "Human Developer");
  assert(
    git(["show", "-s", "--format=%B", "HEAD"]).split(/\r?\n/).includes(`Ahead-Session: ${session_id}`),
    "commit omitted the matching AHEAD session trailer",
  );
  assert.equal((await rpc("list_anchors_for_paths", { paths: ["src/main.py"] })).length, 0,
    "committed CodeAnchor was not cleared");

  const directRequest = {
    ...request,
    // AHEAD selects Code Mode from the model catalog; the provider-model
    // connection keeps the direct tool schema, which is what the standard
    // apply_patch turn exercises.
    model: "mock-tool-capable",
    model_provider: "mock-direct",
    user_message: "Use the standard apply_patch tool to change src/direct.py to print('AHEAD_FUNCTION_PATCH').",
    context: {
      ...request.context,
      active_path: "src/direct.py",
      file_content: directOriginal,
    },
  };
  activeBufferSnapshot = { path: "src/direct.py", content: directOriginal };
  const { turn_id: directTurnId } = await rpc("agent_turn_start", { request: directRequest });
  await waitUntil(async () => {
    history = await rpc("conversation_messages_page", { session_id, before: null, limit: 10 });
    return history.messages.some((message) => message.turn_id === directTurnId
      && message.role === "agent" && ["complete", "failed", "cancelled"].includes(message.status));
  }, "standard function-schema apply_patch turn");
  const directAgent = history.messages.find((message) => message.turn_id === directTurnId && message.role === "agent");
  assert.equal(directAgent.status, "complete", JSON.stringify({ directAgent, requests: requests.length, stderr }));
  assert.equal(requests.length, 6, "the direct patch tool should be followed by one model continuation");
  const directTools = requests[4].tools ?? requests[4].input
    .filter((item) => item.type === "additional_tools").flatMap((item) => item.tools ?? []);
  const directPatchTool = directTools.find((tool) => tool.name === "apply_patch");
  assert.equal(directPatchTool?.type, "function",
    `the advertised OpenAI-compatible patch tool should use the standard function schema: ${JSON.stringify(directPatchTool)}`);
  assert.equal(directPatchTool?.parameters?.properties?.patch?.type, "string",
    `the patch function should accept its complete patch as a string: ${JSON.stringify(directPatchTool?.parameters)}`);
  assert.equal(await readFile(path.join(workspace, "src", "direct.py"), "utf8"), "print('AHEAD_FUNCTION_PATCH')\n");
  runtimeState = await rpc("agent_runtime_state", { session_id });
  assert((runtimeState?.tool_calls ?? []).some(({ kind, status }) => kind === "fileChange" && status === "completed"),
    `the direct patch should emit a completed fileChange: ${JSON.stringify(runtimeState?.tool_calls ?? [])}`);
  const directAnchors = await rpc("list_anchors_for_paths", { paths: ["src/direct.py"] });
  assert.equal(directAnchors.length, 1, `expected one direct-patch CodeAnchor: ${JSON.stringify(directAnchors)}`);
  assert.equal(directAnchors[0].session_id, session_id);
  assert.equal(directAnchors[0].actor_id, "ahead");
  assert.equal(directAnchors[0].surrounding_context, "print('AHEAD_FUNCTION_PATCH')");
  await rpc("git_stage_paths", { paths: ["src/direct.py"], staged: true }, "proxy");
  await rpc("git_commit", { message: "Commit direct apply_patch edit", diffs: [] }, "proxy");
  assert.equal(git(["show", "--pretty=", "--name-only", "HEAD"]), "src/direct.py");
  assert.equal(git(["show", "-s", "--format=%an", "HEAD"]), "ahead");
  assert.equal(git(["show", "-s", "--format=%cn", "HEAD"]), "Human Developer");
  assert(
    git(["show", "-s", "--format=%B", "HEAD"]).split(/\r?\n/).includes(`Ahead-Session: ${session_id}`),
    "the direct-patch commit omitted its AHEAD session trailer",
  );
  assert.equal((await rpc("list_anchors_for_paths", { paths: ["src/direct.py"] })).length, 0,
    "the direct-patch CodeAnchor was not cleared after commit");
  await Promise.all(snapshotReplies);
  assert.deepEqual(snapshotReplyErrors, [], "editor buffer snapshot requests should be answered for each active file");

  child.stdin.write(JSON.stringify({ method: "shutdown", params: {} }) + "\n");
  await waitUntil(() => exited, "proxy shutdown");
  assert.deepEqual(exitResult, { code: 0, signal: null }, stderr);
  passed = true;
  const commandExecutionNotifications = notifications.filter((notification) =>
    JSON.stringify(notification).includes('"commandExecution"')).length;
  const notificationMethods = [...new Set(notifications.map(({ method }) => method))];
  console.log(`PASS shell-write denial/generated output, Code Mode edit, and standard function-schema apply_patch -> Turso CodeAnchor -> staged GitCommit; commandExecution notifications: ${commandExecutionNotifications}; notifications: ${notificationMethods.join(", ")}; ahead author, human committer, session trailer, and anchor cleanup verified (tools: ${toolNames.join(", ")})`);
} finally {
  if (!exited && child?.pid) process.kill(-child.pid, "SIGKILL");
  lines?.close();
  model.closeAllConnections();
  await new Promise((resolve) => model.close(resolve));
  if (passed) {
    await rm(workspace, { recursive: true });
    await rm(profile, { recursive: true });
  } else {
    console.error(`Disposable failure project retained at ${workspace} with profile ${profile}`);
    console.error(JSON.stringify({
      requestCount: requests.length,
      requests: requests.map(({ tools, input }) => ({
        tools: tools?.map((tool) => tool.name),
        inputTypes: input?.map((item) => item.type),
        functionCallOutputs: input?.filter((item) => item.type === "function_call_output")
          .map((item) => String(item.output).slice(0, 240)),
      })),
      advertisedTools: requests[0]?.input?.filter((item) => item.type === "additional_tools")
        .flatMap((item) => item.tools ?? []).map(({ name }) => name),
      messages: history?.messages?.map(({ role, status, content }) => ({ role, status, content: content?.slice?.(0, 160) })),
      toolCalls: runtimeState?.tool_calls?.map(({ kind, status, title }) => ({ kind, status, title })),
      commandExecutionNotifications: notifications.filter((notification) =>
        JSON.stringify(notification).includes('"commandExecution"')).length,
      stderr: stderr.slice(-1600),
    }));
  }
}
