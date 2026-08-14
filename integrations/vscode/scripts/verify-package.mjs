import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const result = spawnSync("npx", ["vsce", "ls"], {
  cwd: root,
  encoding: "utf8",
  stdio: ["ignore", "pipe", "pipe"],
});
assert.equal(result.status, 0, result.stderr);
const files = new Set(result.stdout.trim().split("\n"));
for (const required of [
  "agents/ahead.agent.md",
  "dist/ahead_wasm.wasm",
  "dist/extension.js",
  "generated/product-change/manifest.json",
  "generated/reference/index.json",
  "LICENSE",
  "package.json",
  "README.md",
  "resources/ahead.svg",
]) {
  assert.ok(files.has(required), `VSIX payload is missing ${required}`);
}

const manifest = JSON.parse(await readFile(join(root, "package.json"), "utf8"));
assert.ok(manifest.contributes.chatAgents.length > 0);
assert.ok(manifest.contributes.languageModelTools.length > 0);
console.log(`Verified ${files.size} VS Code extension payload files`);
