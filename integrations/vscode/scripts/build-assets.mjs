import { spawnSync } from "node:child_process";
import { copyFile, cp, mkdir, rm } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../..", import.meta.url));
const result = spawnSync(
  "cargo",
  ["build", "--locked", "--release", "--target", "wasm32-unknown-unknown", "-p", "ahead-wasm"],
  { cwd: root, stdio: "inherit" },
);
if (result.status !== 0) {
  process.exit(result.status ?? 1);
}

const integration = join(root, "integrations", "vscode");
const dist = join(integration, "dist");
await mkdir(dist, { recursive: true });
await copyFile(
  join(root, "target", "wasm32-unknown-unknown", "release", "ahead_wasm.wasm"),
  join(dist, "ahead_wasm.wasm"),
);
await rm(join(integration, "generated"), { recursive: true, force: true });
await cp(join(root, "integrations", "pi", "generated"), join(integration, "generated"), {
  recursive: true,
});

console.log("Copied the AHEAD engine and generated policy into the VS Code extension");
