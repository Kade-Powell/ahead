import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { cp, mkdtemp, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath, pathToFileURL } from "node:url";

// Opt-in: uses an already-built AHEAD and already-installed language servers.
// Every write stays in a fresh fixture copy; no agent/model request is sent.
const binary = path.resolve(process.argv[2] ?? "target/debug/ahead");
const fixture = fileURLToPath(new URL("./fixtures/editor-smoke/", import.meta.url));
const workspace = await mkdtemp(path.join(tmpdir(), "ahead-lsp-smoke-"));
await cp(fixture, workspace, {
  recursive: true,
  filter: (source) => !["target", "node_modules", "__pycache__", ".ahead"].includes(path.basename(source)),
});
const proxy = spawn(binary, ["--proxy"], { cwd: workspace, detached: true, stdio: ["pipe", "pipe", "pipe"] });
const messages = [];
const waiters = new Set();
let requestId = 0;
let stderr = "";
let failure;
proxy.stderr.on("data", (chunk) => { stderr = (stderr + chunk).slice(-4000); });
proxy.on("error", (error) => { failure = error; });
proxy.on("exit", (code, signal) => {
  failure = new Error(`Proxy exited: ${code ?? signal}\n${stderr}`);
  for (const waiter of waiters) waiter();
});
const lines = createInterface({ input: proxy.stdout });
lines.on("line", (line) => {
  try { messages.push(JSON.parse(line)); }
  catch (error) { failure = new Error(`Invalid proxy JSON: ${error.message}`); }
  for (const waiter of waiters) waiter();
});
function waitFor(predicate, label, after = messages.length) {
  return new Promise((resolve, reject) => {
    const finish = (error, value) => {
      clearTimeout(timer);
      waiters.delete(check);
      if (error) reject(error); else resolve(value);
    };
    const check = () => {
      if (failure) return finish(failure);
      const message = messages.slice(after).find(predicate);
      if (message) finish(null, message);
    };
    const timer = setTimeout(() => {
      const relevant = messages.filter((message) => message.error || ["publish_diagnostics", "server_status", "show_message"].includes(message.method));
      finish(new Error(`Timed out: ${label}\n${JSON.stringify(relevant.slice(-8))}\n${stderr}`));
    }, 30000);
    waiters.add(check);
    check();
  });
}
function notify(method, params) { proxy.stdin.write(JSON.stringify({ method, params }) + "\n"); }
async function request(method, params, expectError = false) {
  const id = ++requestId;
  const reply = waitFor((message) => message.id === id && !message.method, method);
  proxy.stdin.write(JSON.stringify({ id, method, params }) + "\n");
  const message = await reply;
  if (expectError) {
    assert(message.error, `${method}: expected an error`);
    return message.error;
  }
  assert.equal(message.error, undefined, JSON.stringify(message.error));
  return message.result;
}
async function snapshot(relative, content, acceptDiagnostics) {
  const file = path.join(workspace, relative);
  const uri = pathToFileURL(file).href;
  if (!acceptDiagnostics) {
    notify("editor_snapshot", { path: file, content });
    return;
  }
  const diagnostics = waitFor((message) => message.method === "publish_diagnostics"
    && message.params.diagnostics.uri === uri
    && acceptDiagnostics(message.params.diagnostics.diagnostics), `${relative}: diagnostics for ${content.length}-character snapshot`);
  notify("editor_snapshot", { path: file, content });
  return (await diagnostics).params.diagnostics.diagnostics;
}
async function completion(relative, position, expected) {
  const id = ++requestId;
  const response = waitFor((message) => message.method === "completion_response" && message.params.request_id === id, `${relative}: completion`);
  notify("completion", { request_id: id, path: path.join(workspace, relative), input: "", position });
  const params = (await response).params;
  const reply = params.resp;
  const items = Array.isArray(reply) ? reply : reply.items;
  const item = items.find((item) => expected instanceof RegExp ? expected.test(item.label) : item.label === expected);
  assert(item, `${relative}: missing ${expected}`);
  return { plugin_id: params.plugin_id, completion_item: item };
}
async function closeBuffer(relative) {
  const file = path.join(workspace, relative);
  const uri = pathToFileURL(file).href;
  const cleared = waitFor((message) => message.method === "publish_diagnostics"
    && message.params.diagnostics.uri === uri && message.params.diagnostics.diagnostics.length === 0, `${relative}: diagnostics cleared on close`);
  notify("close_editor_buffer", { path: file });
  await cleared;
}
async function definition(relative, position, suffix) {
  const response = await request("get_definition", { request_id: ++requestId, path: path.join(workspace, relative), position });
  assert.equal(response.method, "get_definition_response");
  const definition = response.params.definition;
  const locations = Array.isArray(definition) ? definition : [definition];
  assert(locations.some((location) => (location.uri ?? location.targetUri)?.endsWith(suffix)), `${relative}: wrong definition ${JSON.stringify(locations)}`);
}
try {
  notify("initialize", { workspace, window_id: 1, tab_id: 1 });
  const ts = "typescript/src/main.ts";
  const originalTs = await readFile(path.join(workspace, ts), "utf8");
  await snapshot(ts, originalTs.replace("calculate(21)", 'calculate("21")'), (items) => items.some((item) => item.code === 2345));
  await snapshot(ts, originalTs, (items) => items.length === 0);
  const tsPrefix = 'import { calculate } from "./math.ts";\nconst result = calculate(21);\nresult.dou';
  await snapshot(ts, tsPrefix, (items) => items.some((item) => item.message.includes("Property 'dou'")));
  const tsCompletion = await completion(ts, { line: 2, character: 10 }, "doubled");
  await snapshot(ts, tsPrefix + "bled", (items) => items.length === 0);
  await definition(ts, { line: 1, character: 19 }, "/typescript/src/math.ts");
  await snapshot(ts, tsPrefix.replace("calculate(21)", 'calculate("21")') + "bled", (items) => items.some((item) => item.code === 2345));
  await snapshot(ts, tsPrefix + "bled", (items) => items.length === 0);
  console.log("PASS TypeScript: shortened Unicode snapshot, completion, definition, diagnostics and repair");

  const unimported = "export {};\ncalc\n";
  await snapshot(ts, unimported, (items) => items.some((item) => item.code === 2304));
  const candidate = await completion(ts, { line: 1, character: 4 }, "calculate");
  const resolved = await request("completion_resolve", candidate);
  assert.equal(resolved.method, "completion_resolve_response");
  const item = resolved.params.item;
  assert(item.additionalTextEdits?.some((edit) => edit.newText.includes("import") && edit.newText.includes("math")), "Missing resolved auto-import");
  const primary = item.textEdit ?? { range: { start: { line: 1, character: 0 }, end: { line: 1, character: 4 } }, newText: item.insertText ?? item.label };
  const offset = (position) => unimported.split("\n").slice(0, position.line).reduce((length, line) => length + line.length + 1, 0) + position.character;
  const edits = [...item.additionalTextEdits, { range: primary.range ?? primary.replace, newText: primary.newText }]
    .map((edit) => ({ start: offset(edit.range.start), end: offset(edit.range.end), text: edit.newText }))
    .sort((left, right) => right.start - left.start || right.end - left.end);
  let imported = unimported;
  for (const edit of edits) imported = imported.slice(0, edit.start) + edit.text + imported.slice(edit.end);
  await snapshot(ts, imported, (items) => items.length === 0);
  console.log("PASS TypeScript: server-specific completion resolve supplies a working auto-import");
  await snapshot(ts, originalTs.replace("calculate(21)", 'calculate("21")'), (items) => items.some((item) => item.code === 2345));
  await closeBuffer(ts);
  await snapshot(ts, originalTs.replace("calculate(21)", 'calculate("reopened")'), (items) => items.some((item) => item.code === 2345));
  await snapshot(ts, originalTs, (items) => items.length === 0);
  console.log("PASS TypeScript: close clears unsaved diagnostics; reopen and repair work");

  const js = "javascript/src/main.mjs";
  const jsText = 'import { triple } from "./math.mjs";\nconsole.log(triple(14));\n';
  await snapshot(js, jsText);
  const jsCompletion = await completion(js, { line: 1, character: 15 }, "triple");
  assert.equal(jsCompletion.plugin_id, tsCompletion.plugin_id, "TypeScript and JavaScript must share one server instance");
  await definition(js, { line: 1, character: 15 }, "/javascript/src/math.mjs");
  await snapshot(js, jsText.replace("triple(14)", 'triple("14")'), (items) => items.some((item) => item.code === 2345));
  await snapshot(js, jsText, (items) => items.length === 0);
  console.log("PASS JavaScript: shared adapter, completion, definition, diagnostics and repair");

  const py = "python/main.py";
  const pyText = "from arithmetic import double\nprint(double(21))\n";
  await snapshot(py, pyText);
  await completion(py, { line: 1, character: 9 }, "double");
  await definition(py, { line: 1, character: 9 }, "/python/arithmetic.py");
  await snapshot(py, pyText.replace("double(21)", 'double("21")'), (items) => items.some((item) => item.message.includes("cannot be assigned")));
  await snapshot(py, pyText, (items) => items.length === 0);
  console.log("PASS Python: completion, definition, diagnostics and repair");
  await snapshot(py, pyText.replace("double(21)", 'double("close")'), (items) => items.some((item) => item.message.includes("cannot be assigned")));
  await closeBuffer(py);
  await snapshot(py, pyText.replace("double(21)", 'double("reopened")'), (items) => items.some((item) => item.message.includes("cannot be assigned")));
  await snapshot(py, pyText, (items) => items.length === 0);
  console.log("PASS Python: close clears unsaved diagnostics; reopen and repair work");

  const rust = "src/main.rs";
  const originalRust = await readFile(path.join(workspace, rust), "utf8");
  const rustLines = originalRust.split("\n");
  const rustLine = rustLines.findIndex((line) => line.includes("math::double(21)"));
  assert(rustLine >= 0, "Rust fixture must call math::double(21)");
  const rustPosition = { line: rustLine, character: rustLines[rustLine].indexOf("double") + 3 };
  const rustError = originalRust.replace("double(21)", 'double("21")');
  await snapshot(rust, rustError, (items) => items.some((item) => item.code === "E0308"));
  await snapshot(rust, originalRust, (items) => items.length === 0);
  const rustCompletion = await completion(rust, rustPosition, /^double(?:\(|$)/);
  await definition(rust, rustPosition, "/src/math.rs");
  await snapshot(rust, rustError, (items) => items.some((item) => item.code === "E0308"));
  await closeBuffer(rust);
  await snapshot(rust, rustError, (items) => items.some((item) => item.code === "E0308"));
  console.log("PASS Rust: completion, definition, unsaved diagnostics, repair, close and reopen");

  await snapshot(ts, tsPrefix, (items) => items.some((item) => item.message.includes("Property 'dou'")));
  await snapshot(py, pyText.replace("double(21)", 'double("restart")'), (items) => items.some((item) => item.message.includes("cannot be assigned")));
  const beforeRestart = messages.length;
  notify("restart_language_servers", {});
  for (const name of ["vtsls", "basedpyright-langserver", "rust-analyzer"]) {
    await waitFor((message) => message.method === "server_status"
      && message.params.params.server_name === name && message.params.params.health === "ok", `${name}: ready after restart`, beforeRestart);
  }
  for (const [relative, errorText] of [[ts, "Property 'dou'"], [py, "cannot be assigned"], [rust, "expected i32"]]) {
    const uri = pathToFileURL(path.join(workspace, relative)).href;
    await waitFor((message) => message.method === "publish_diagnostics"
      && message.params.diagnostics.uri === uri
      && message.params.diagnostics.diagnostics.some((item) => item.message.includes(errorText)), `${relative}: unsaved diagnostics replayed`, beforeRestart);
    assert(messages.slice(beforeRestart).some((message) => message.method === "publish_diagnostics"
      && message.params.diagnostics.uri === uri && message.params.diagnostics.diagnostics.length === 0), `${relative}: retired diagnostics must clear`);
  }
  const restartedTs = await completion(ts, { line: 2, character: 10 }, "doubled");
  assert.notEqual(restartedTs.plugin_id, tsCompletion.plugin_id, "restart must create a new server instance");
  const restartedJs = await completion(js, { line: 1, character: 15 }, "triple");
  assert.equal(restartedTs.plugin_id, restartedJs.plugin_id, "TS/JS must still share the replacement server");
  await completion(py, { line: 1, character: 9 }, "double");
  await definition(py, { line: 1, character: 9 }, "/python/arithmetic.py");
  await snapshot(ts, tsPrefix + "bled", (items) => items.length === 0);
  await snapshot(py, pyText, (items) => items.length === 0);
  const restartedRust = await completion(rust, rustPosition, /^double(?:\(|$)/);
  assert.notEqual(restartedRust.plugin_id, rustCompletion.plugin_id, "Rust restart must create a new instance");
  await definition(rust, rustPosition, "/src/math.rs");
  await snapshot(rust, originalRust, (items) => items.length === 0);
  assert.equal(await readFile(path.join(workspace, rust), "utf8"), originalRust, "Rust restart must not save the unsaved buffer");
  assert.equal(await readFile(path.join(workspace, ts), "utf8"), originalTs, "restart must not save the unsaved buffer");
  const obsolete = await request("completion_resolve", candidate, true);
  assert.match(obsolete.message, /plugin doesn't exist/);
  console.log("PASS restart: fresh processes, TS/JS sharing, unsaved replay, diagnostic clear/repair, completion, definition and stale resolve rejection");
  for (const name of ["vtsls", "basedpyright-langserver", "rust-analyzer"]) {
    assert(messages.some((message) => message.method === "server_status"
      && message.params.params.server_name === name && message.params.params.health === "ok"), `${name}: missing initialized status`);
    const latestStatus = messages.filter((message) => message.method === "server_status" && message.params.params.server_name === name).at(-1);
    assert.equal(latestStatus.params.params.health, "ok", `${name}: retired process replaced current status`);
  }
  const shutdown = new Promise((resolve) => proxy.once("exit", (code, signal) => resolve({ code, signal })));
  notify("shutdown", {});
  const deadline = setTimeout(() => proxy.kill("SIGKILL"), 10000);
  const stopped = await shutdown.finally(() => clearTimeout(deadline));
  assert.equal(stopped.code, 0, `proxy shutdown failed: ${stopped.signal ?? stderr}`);
  assert.throws(() => process.kill(-proxy.pid, 0), { code: "ESRCH" }, "language-server process group survived graceful shutdown");
  console.log(`PASS standard LSP readiness and graceful proxy shutdown. Disposable workspace retained: ${workspace}`);
} catch (error) {
  console.error(error);
  console.error(`Disposable workspace retained: ${workspace}`);
  process.exitCode = 1;
} finally {
  lines.close();
  if (proxy.pid) {
    try { process.kill(-proxy.pid, "SIGTERM"); }
    catch (error) { if (error.code !== "ESRCH") throw error; }
  }
}
