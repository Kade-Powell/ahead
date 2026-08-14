import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import test from "node:test";

const manifest = JSON.parse(await readFile(new URL("../package.json", import.meta.url), "utf8"));
const agent = await readFile(new URL("../agents/ahead.agent.md", import.meta.url), "utf8");
const packageRoot = fileURLToPath(new URL("..", import.meta.url));

test("Copilot receives assistance tools but no human gate tool", () => {
  const tools = manifest.contributes.languageModelTools.map(({ name }) => name);
  assert.ok(tools.includes("ahead_get_context"));
  assert.ok(tools.includes("ahead_record_artifact"));
  assert.ok(tools.includes("ahead_request_transition"));
  assert.equal(
    tools.some((name) => /accept|advance|close|approve/.test(name)),
    false,
  );
});

test("the AHEAD agent preserves the human authority boundary", () => {
  assert.match(agent, /human leads/i);
  assert.match(
    agent,
    /Never author a human-owned artifact, accept a gate, transition or close a run/i,
  );
  assert.match(agent, /Call `#tool:ahead_get_context` at the start of every turn/i);
  assert.match(
    agent,
    /During implementation, coach, explain, answer questions, and help diagnose/i,
  );
});

test("release validation binds the GitHub tag to the extension version", () => {
  const matching = spawnSync(
    process.execPath,
    ["./scripts/check-release.mjs", `vscode-v${manifest.version}`],
    { cwd: packageRoot, encoding: "utf8" },
  );
  assert.equal(matching.status, 0, matching.stderr);
  assert.match(matching.stdout, new RegExp(`${manifest.publisher}\\.${manifest.name}`));

  const mismatched = spawnSync(process.execPath, ["./scripts/check-release.mjs", "vscode-v0.0.0"], {
    cwd: packageRoot,
    encoding: "utf8",
  });
  assert.equal(mismatched.status, 1);
  assert.match(mismatched.stderr, /does not match extension version/);
});
