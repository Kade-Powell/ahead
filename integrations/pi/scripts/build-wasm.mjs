import { copyFile, mkdir } from "node:fs/promises";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
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

const source = join(root, "target", "wasm32-unknown-unknown", "release", "ahead_wasm.wasm");
const destinationDirectory = join(root, "integrations", "pi", "dist");
await mkdir(destinationDirectory, { recursive: true });
await copyFile(source, join(destinationDirectory, "ahead_wasm.wasm"));
console.log("Copied AHEAD workflow engine to integrations/pi/dist/ahead_wasm.wasm");
