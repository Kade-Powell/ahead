#!/usr/bin/env node
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { appendFileSync } from "node:fs";
import { mkdir, mkdtemp, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const source = fileURLToPath(import.meta.url);

if (process.argv[2] === "--adapter") {
  const [log, mode] = process.argv.slice(3);
  const record = (event) => appendFileSync(log, JSON.stringify({ event, pid: process.pid }) + "\n");
  let sequence = 0;
  const reply = (request, body) => {
    const message = JSON.stringify({ type: "response", seq: sequence++, request_seq: request.seq, command: request.command, success: true, body });
    process.stdout.write(`Content-Length: ${Buffer.byteLength(message)}\r\n\r\n${message}`);
  };
  record("started");
  process.stderr.on("error", () => {}); // The write callback records this in the fixture log.
  let pending = Buffer.alloc(0);
  process.stdin.on("data", async (chunk) => {
    pending = Buffer.concat([pending, chunk]);
    while (true) {
      const headerEnd = pending.indexOf("\r\n\r\n");
      if (headerEnd < 0) return;
      const length = Number(/Content-Length: (\d+)/i.exec(pending.subarray(0, headerEnd).toString())?.[1]);
      assert(Number.isInteger(length) && length > 0, "invalid DAP frame");
      const end = headerEnd + 4 + length;
      if (pending.length < end) return;
      const request = JSON.parse(pending.subarray(headerEnd + 4, end));
      pending = pending.subarray(end);
      if (request.type !== "request") continue;
      record(request.command);
      if (request.command === "initialize") {
        if (mode === "initializing") continue;
        if (mode === "stderr") {
          process.stdin.pause();
          const drained = await new Promise((resolve) => process.stderr.write(Buffer.alloc(1024 * 1024, "x"), (error) => {
            if (error) record(`stderr-error:${error.code}`);
            resolve(!error);
          }));
          process.stdin.resume();
          if (!drained) continue;
          record("stderr-drained");
        }
        reply(request, {});
      } else if (request.command === "launch") {
        reply(request, null);
      }
      // Deliberately ignore stop requests and stdin EOF to require owned-child cleanup.
    }
  });
  process.stdin.on("end", () => record("stdin-closed"));
  setInterval(() => {}, 1000);
} else {
  assert.notEqual(process.platform, "win32", "This isolated process-group check supports Unix only");
  const binary = path.resolve(process.argv[2] ?? "target/debug/ahead");
  const directory = await mkdtemp(path.join(tmpdir(), "ahead-dap-shutdown-"));
  console.log(`Disposable workspaces: ${directory}`);
  const failures = [];
  for (const mode of ["running", "initializing", "stderr"]) {
    const workspace = path.join(directory, mode);
    await mkdir(workspace);
    const log = path.join(workspace, "adapter.jsonl");
    const proxy = spawn(binary, ["--proxy"], {
      cwd: workspace,
      detached: true,
      stdio: ["pipe", "pipe", "pipe"],
    });
    let output = "";
    let exited = false;
    let connected = false;
    const lines = createInterface({ input: proxy.stdout });
    lines.on("line", (line) => {
      const message = JSON.parse(line);
      if (message.method === "proxy_status" && message.params.status === "Connected") connected = true;
    });
    for (const stream of [proxy.stdout, proxy.stderr]) {
      stream.on("data", (chunk) => { output = (output + chunk).slice(-4000); });
    }
    proxy.stdin.on("error", () => {}); // Report the exit or missing adapter event below.
    const exit = new Promise((resolve, reject) => {
      proxy.on("error", reject);
      proxy.on("exit", (code, signal) => { exited = true; resolve({ code, signal }); });
    });
    exit.catch(() => {}); // The bounded wait below reports startup errors.
    const notify = (method, params) => proxy.stdin.write(JSON.stringify({ method, params }) + "\n");
    const records = async () => {
      try { return (await readFile(log, "utf8")).trim().split("\n").filter(Boolean).map(JSON.parse); }
      catch (error) { if (error.code === "ENOENT") return []; throw error; }
    };
    try {
      notify("initialize", { workspace, window_id: 1, tab_id: 1 });
      const startupDeadline = Date.now() + 30000;
      while (!connected) {
        assert(!exited && Date.now() < startupDeadline, `${mode}: proxy never became ready\n${output}`);
        await delay(20);
      }
      notify("dap_start", {
        config: {
          name: `Disposable ${mode} adapter`,
          program: "unused-debug-target",
          args: [],
          cwd: workspace,
          "debug-adapter": process.execPath,
          "debug-adapter-args": [source, "--adapter", log, mode],
        },
        breakpoints: {},
      });
      const readyEvent = mode === "initializing" ? "initialize" : "launch";
      const deadline = Date.now() + 10000;
      while (!(await records()).some((entry) => entry.event === readyEvent)) {
        assert(!exited && Date.now() < deadline, `${mode}: adapter never reached ${readyEvent}; records=${JSON.stringify(await records())}\n${output}`);
        await delay(20);
      }
      if (mode === "initializing") proxy.stdin.end();
      else notify("shutdown", {});
      const timeout = setTimeout(() => proxy.kill("SIGKILL"), 12000);
      const result = await exit.finally(() => clearTimeout(timeout));
      assert.equal(result.code, 0, `${mode}: proxy failed (${result.signal}): ${output}`);
      const events = await records();
      const adapter = events.find((entry) => entry.event === "started");
      assert(adapter && adapter.pid > 1, "adapter process was recorded");
      assert.throws(() => process.kill(adapter.pid, 0), { code: "ESRCH" }, `${mode}: adapter survived proxy shutdown`);
      if (mode === "stderr") assert(events.some((entry) => entry.event === "stderr-drained"));
      console.log(`PASS ${mode}: adapter started and was reaped before proxy exit`);
    } catch (error) {
      failures.push(error);
      console.error(`FAIL ${error.message}`);
    } finally {
      // Only this check's isolated proxy group; never target existing editor processes.
      if (proxy.pid > 1) {
        try { process.kill(-proxy.pid, "SIGKILL"); }
        catch (error) { if (error.code !== "ESRCH") throw error; }
      }
      await exit;
      lines.close();
    }
  }
  console.log(`Disposable workspaces retained: ${directory}`);
  if (failures.length) throw new AggregateError(failures, `${failures.length} DAP lifecycle checks failed`);
}
