#!/usr/bin/env node
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { appendFileSync } from "node:fs";
import { copyFile, mkdir, mkdtemp, readFile, writeFile, chmod } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const source = fileURLToPath(import.meta.url);

// The disposable executable is this same script, copied onto the proxy's PATH.
if (path.basename(source) === "rust-analyzer") {
  const record = (event) => appendFileSync(process.env.AHEAD_LSP_TEST_LOG, JSON.stringify({ event, pid: process.pid }) + "\n");
  const reply = (id, result) => {
    const body = JSON.stringify({ jsonrpc: "2.0", id, result });
    process.stdout.write(`Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`);
  };
  let pending = Buffer.alloc(0);
  record("started");
  process.stdin.on("data", (chunk) => {
    pending = Buffer.concat([pending, chunk]);
    while (true) {
      const headerEnd = pending.indexOf("\r\n\r\n");
      if (headerEnd < 0) return;
      const length = Number(/Content-Length: (\d+)/i.exec(pending.subarray(0, headerEnd).toString())?.[1]);
      assert(Number.isInteger(length) && length > 0, "invalid LSP frame");
      const end = headerEnd + 4 + length;
      if (pending.length < end) return;
      const message = JSON.parse(pending.subarray(headerEnd + 4, end));
      pending = pending.subarray(end);
      record(message.method);
      if (message.method === "initialize") {
        const initialize = () => reply(message.id, { capabilities: { textDocumentSync: { openClose: true, change: 2 } } });
        if (process.env.AHEAD_LSP_TEST_MODE === "initializing") setTimeout(initialize, 1000);
        else initialize();
      }
      if (message.method === "shutdown" && process.env.AHEAD_LSP_TEST_MODE !== "unresponsive") {
        // Expose a proxy that exits immediately after merely queueing shutdown.
        setTimeout(() => { record("shutdown-replied"); reply(message.id, null); }, 150);
      }
      if (message.method === "exit" && process.env.AHEAD_LSP_TEST_MODE !== "unresponsive") process.exit(0);
    }
  });
  process.stdin.on("end", () => {
    record("stdin-closed");
    if (process.env.AHEAD_LSP_TEST_MODE === "unresponsive") setInterval(() => {}, 1000);
    else process.exit(2);
  });
} else {
  assert.notEqual(process.platform, "win32", "This process-group smoke currently supports Unix only");
  const binary = path.resolve(process.argv[2] ?? "target/debug/ahead");
  const directory = await mkdtemp(path.join(tmpdir(), "ahead-lsp-shutdown-"));
  console.log(`Disposable workspaces: ${directory}`);
  const bin = path.join(directory, "bin");
  await mkdir(bin);
  const server = path.join(bin, "rust-analyzer");
  await copyFile(source, server);
  await chmod(server, 0o700);

  for (const mode of ["shutdown", "stdin-eof", "initializing", "unresponsive"]) {
    const workspace = path.join(directory, mode);
    await mkdir(workspace);
    const file = path.join(workspace, "main.rs");
    await writeFile(file, "fn main() {}\n");
    const log = path.join(workspace, "server.jsonl");
    const proxy = spawn(binary, ["--proxy"], {
      cwd: workspace,
      detached: true,
      stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env, PATH: `${bin}${path.delimiter}${path.dirname(process.execPath)}${path.delimiter}${process.env.PATH}`, AHEAD_LSP_TEST_LOG: log, AHEAD_LSP_TEST_MODE: mode },
    });
    let stderr = "";
    let stdout = "";
    let exited = false;
    proxy.stdout.on("data", (chunk) => { stdout = (stdout + chunk).slice(-8000); });
    proxy.stderr.on("data", (chunk) => { stderr = (stderr + chunk).slice(-4000); });
    proxy.stdin.on("error", () => {}); // Exit status and missing shutdown frames fail below.
    const exit = new Promise((resolve, reject) => {
      proxy.on("error", reject);
      proxy.on("exit", (code, signal) => { exited = true; resolve({ code, signal }); });
    });
    const records = async () => {
      try { return (await readFile(log, "utf8")).trim().split("\n").filter(Boolean).map(JSON.parse); }
      catch (error) { if (error.code === "ENOENT") return []; throw error; }
    };
    const notify = (method, params) => proxy.stdin.write(JSON.stringify({ method, params }) + "\n");
    try {
      notify("initialize", { workspace, window_id: 1, tab_id: 1 });
      notify("editor_snapshot", { path: file, content: "fn main() { let unsaved = 1; }\n" });
      const readyDeadline = Date.now() + 15000;
      const readyEvent = mode === "initializing" ? "initialize" : "textDocument/didOpen";
      while (!(await records()).some((record) => record.event === readyEvent)) {
        assert(!exited && Date.now() < readyDeadline, `server did not open the buffer: ${stderr}\n${stdout}`);
        await delay(20);
      }
      const started = Date.now();
      if (mode === "stdin-eof" || mode === "initializing") proxy.stdin.end();
      else notify("shutdown", {});
      const timer = setTimeout(() => proxy.kill("SIGKILL"), 12000);
      const result = await exit.finally(() => clearTimeout(timer));
      assert.equal(result.code, 0, `${mode}: proxy failed (${result.signal}): ${stderr}`);
      const events = await records();
      const shutdown = events.findIndex((record) => record.event === "shutdown");
      assert(shutdown >= 0, `${mode}: missing LSP shutdown request`);
      if (mode !== "unresponsive") {
        const replied = events.findIndex((record) => record.event === "shutdown-replied");
        const stopped = events.findIndex((record) => record.event === "exit");
        assert(replied > shutdown && stopped > replied, `${mode}: proxy exited before the shutdown/exit handshake`);
      } else {
        assert(Date.now() - started >= 4500, "unresponsive server must get its graceful shutdown deadline");
      }
      assert.throws(() => process.kill(events[0].pid, 0), { code: "ESRCH" }, `${mode}: language server still running`);
      assert.equal(await readFile(file, "utf8"), "fn main() {}\n", "shutdown must not save the unsaved buffer");
      console.log(`PASS ${mode}: proxy waits for server cleanup; source file unchanged`);
    } finally {
      // Only this smoke's detached process group, including a failed fixture.
      try { process.kill(-proxy.pid, "SIGKILL"); }
      catch (error) { if (error.code !== "ESRCH") throw error; }
    }
  }
  console.log(`Disposable workspaces retained: ${directory}`);
}
