import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = fileURLToPath(new URL("..", import.meta.url));
const packageJson = JSON.parse(await readFile(join(packageRoot, "package.json"), "utf8"));
const temporaryRoot = await mkdtemp(join(tmpdir(), "ahead-pi-package-"));

try {
  const packDirectory = join(temporaryRoot, "pack");
  await mkdir(packDirectory);
  const packed = run("npm", ["pack", "--json", "--pack-destination", packDirectory], {
    cwd: packageRoot,
    capture: true,
    env: { ...process.env, npm_config_cache: join(temporaryRoot, "npm-cache") },
  });
  const json = packed.stdout.match(/(\[\s*\{\s*"id"[\s\S]*\])\s*$/)?.[1];
  assert.ok(json, "npm pack did not emit JSON package metadata");
  const metadata = JSON.parse(json)[0];
  assert.equal(metadata.name, packageJson.name);
  assert.equal(metadata.version, packageJson.version);
  assert.ok(metadata.unpackedSize < 2_000_000, `package is unexpectedly large: ${metadata.unpackedSize} bytes`);

  const paths = new Set(metadata.files.map((file) => file.path));
  for (const required of [
    "README.md",
    "package.json",
    "dist/ahead_wasm.wasm",
    "generated/product-change/manifest.json",
    "src/engine.ts",
    "src/guidance.ts",
    "src/index.ts",
    "src/storage.ts",
    "src/types.ts",
  ]) {
    assert.ok(paths.has(required), `packed artifact is missing ${required}`);
  }
  assert.equal(
    [...paths].filter((path) => path.startsWith("generated/product-change/") && path.endsWith(".md")).length,
    13,
    "packed artifact must contain all 13 Product Change instruction bundles",
  );
  for (const path of paths) {
    assert.ok(!path.startsWith("node_modules/"), `package leaked node_modules content: ${path}`);
    assert.ok(!path.startsWith("test/"), `package leaked test content: ${path}`);
    assert.ok(!path.startsWith("scripts/"), `package leaked build scripts: ${path}`);
  }

  const tarball = join(packDirectory, metadata.filename);
  const extracted = join(temporaryRoot, "extracted");
  const project = join(temporaryRoot, "project");
  const piHome = join(temporaryRoot, "pi-home");
  await mkdir(extracted);
  await mkdir(project);
  await mkdir(piHome);
  run("tar", ["-xzf", tarball, "-C", extracted]);

  const extractedPackage = join(extracted, "package");
  const extractedJson = JSON.parse(await readFile(join(extractedPackage, "package.json"), "utf8"));
  assert.equal(extractedJson.name, "ahead-pi");
  assert.notEqual(extractedJson.private, true);

  const piBinary = join(packageRoot, "node_modules", ".bin", "pi");
  run(
    piBinary,
    ["--no-extensions", "-p", "-e", extractedPackage, "/ahead Packaged extension smoke"],
    {
      cwd: project,
      env: {
        ...process.env,
        AHEAD_HUMAN_IDENTITY: "package-smoke",
        PI_CODING_AGENT_DIR: piHome,
      },
    },
  );

  const current = JSON.parse(await readFile(join(project, ".ahead", "current.json"), "utf8"));
  const runState = JSON.parse(
    await readFile(join(project, ".ahead", "runs", current.run_id, "run.json"), "utf8"),
  );
  assert.equal(runState.workflow_id, "product-change");
  assert.equal(runState.events[0].type, "run_started");
  assert.deepEqual(runState.events[0].actor, { identity: "package-smoke", kind: "human" });

  console.log(
    `Verified ${metadata.name}@${metadata.version}: ${metadata.files.length} files, ${metadata.unpackedSize} unpacked bytes, packaged Pi smoke passed`,
  );
} finally {
  await rm(temporaryRoot, { recursive: true, force: true });
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd,
    env: options.env,
    encoding: "utf8",
    maxBuffer: 20 * 1024 * 1024,
    stdio: options.capture ? ["ignore", "pipe", "pipe"] : "pipe",
  });
  if (result.status !== 0) {
    const output = [result.stdout, result.stderr].filter(Boolean).join("\n");
    throw new Error(`${command} ${args.join(" ")} failed with status ${result.status}\n${output}`);
  }
  return result;
}
