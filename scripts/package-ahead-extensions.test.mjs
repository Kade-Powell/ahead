import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { chmod, mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { packageAheadExtensions } from "./package-ahead-extensions.mjs";

const sourceRepository = "https://github.com/zed-industries/zed";

async function fixture(t) {
  const root = await mkdtemp(path.join(os.tmpdir(), "ahead-publisher-test-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const sourceRoot = path.join(root, "zed-source");
  const extensionRoot = path.join(sourceRoot, "extensions", "html");
  await mkdir(extensionRoot, { recursive: true });
  await writeFile(path.join(extensionRoot, "extension.toml"), [
    'id = "html"',
    'name = "HTML"',
    'version = "0.3.2"',
    `repository = "${sourceRepository}"`,
    "",
  ].join("\n"));
  await writeFile(path.join(extensionRoot, "Cargo.toml"), [
    "[package]",
    'license = "Apache-2.0"',
    "",
  ].join("\n"));
  await writeFile(path.join(extensionRoot, "LICENSE-APACHE"), "Apache license fixture\n");
  execFileSync("git", ["-C", sourceRoot, "init", "--quiet"]);
  execFileSync("git", ["-C", sourceRoot, "config", "user.email", "ahead-test@example.invalid"]);
  execFileSync("git", ["-C", sourceRoot, "config", "user.name", "AHEAD test"]);
  execFileSync("git", ["-C", sourceRoot, "remote", "add", "origin", sourceRepository]);
  execFileSync("git", ["-C", sourceRoot, "add", "."]);
  execFileSync("git", ["-C", sourceRoot, "commit", "--quiet", "-m", "fixture"]);
  const revision = execFileSync("git", ["-C", sourceRoot, "rev-parse", "HEAD"], { encoding: "utf8" }).trim();

  const configPath = path.join(root, "sources.json");
  await writeFile(configPath, JSON.stringify({
    source_repository: sourceRepository,
    source_revision: revision,
    zed_extension_cli_revision: "a".repeat(40),
    extensions: [{
      id: "html",
      version: "0.3.2",
      source_path: ".",
      repository: sourceRepository,
      revision,
      path: "extensions/html",
      license: "Apache-2.0",
      license_file: "LICENSE-APACHE",
      runtime_dependencies: ["@zed-industries/vscode-langservers-extracted"],
      requested_capabilities: [{
        kind: "npm:install",
        package: "@zed-industries/vscode-langservers-extracted",
      }],
    }],
  }));

  const cliPath = path.join(root, "fake-zed-extension.mjs");
  await writeFile(cliPath, `#!/usr/bin/env node
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
const args = process.argv.slice(2);
const output = args[args.indexOf("--output-dir") + 1];
await mkdir(output, { recursive: true });
await writeFile(path.join(output, "archive.tar.gz"), "fixture archive\\n");
await writeFile(path.join(output, "manifest.json"), JSON.stringify({
  name: "HTML",
  version: "0.3.2",
  schema_version: 1,
  repository: "${sourceRepository}",
  wasm_api_version: "0.7.0",
  provides: ["languages", "language-servers"],
}));
`);
  await chmod(cliPath, 0o755);
  return { root, sourceRoot, configPath, cliPath };
}

test("packages reviewed source and writes the release URL and archive digest", async (t) => {
  const state = await fixture(t);
  const outputDirectory = path.join(state.root, "dist");
  const registryPath = path.join(state.root, "registry.json");
  await writeFile(registryPath, JSON.stringify({ data: [] }));
  const releaseTag = "ahead-extensions-123";
  const [entry] = await packageAheadExtensions({
    sourceRoot: state.sourceRoot,
    releaseTag,
    cliPath: state.cliPath,
    outputDirectory,
    registryPath,
    configPath: state.configPath,
  });

  const archivePath = path.join(outputDirectory, "html-0.3.2.tar.gz");
  assert.equal(entry.download_url, `https://github.com/Kade-Powell/ahead/releases/download/${releaseTag}/html-0.3.2.tar.gz`);
  assert.equal(entry.archive_sha256, "b82d14bd3717287c78a2e1351107a49a925192cae59c0f844437eed8a0d6caef");
  const registry = JSON.parse(await readFile(registryPath, "utf8"));
  assert.equal(registry.data[0].source_revision, entry.source_revision);
  assert.deepEqual(registry.data[0].requested_capabilities, [{
    kind: "npm:install",
    package: "@zed-industries/vscode-langservers-extracted",
  }]);
  assert.equal(await readFile(archivePath, "utf8"), "fixture archive\n");
});

test("rejects a source checkout that differs from the reviewed pin", async (t) => {
  const state = await fixture(t);
  const config = JSON.parse(await readFile(state.configPath, "utf8"));
  config.source_revision = "b".repeat(40);
  await writeFile(state.configPath, JSON.stringify(config));
  const registryPath = path.join(state.root, "registry.json");
  await writeFile(registryPath, JSON.stringify({ data: [] }));

  await assert.rejects(packageAheadExtensions({
    sourceRoot: state.sourceRoot,
    releaseTag: "ahead-extensions-123",
    cliPath: state.cliPath,
    outputDirectory: path.join(state.root, "dist"),
    registryPath,
    configPath: state.configPath,
  }), /does not match the reviewed source revision/u);
  assert.deepEqual(JSON.parse(await readFile(registryPath, "utf8")), { data: [] });
});

test("packages a reviewed extension from a pinned registry submodule", async (t) => {
  const state = await fixture(t);
  const extensionRevision = execFileSync("git", ["-C", state.sourceRoot, "rev-parse", "HEAD"], { encoding: "utf8" }).trim();
  const registryRoot = path.join(state.root, "zed-extensions");
  await mkdir(registryRoot);
  execFileSync("git", ["-C", registryRoot, "init", "--quiet"]);
  execFileSync("git", ["-C", registryRoot, "config", "user.email", "ahead-test@example.invalid"]);
  execFileSync("git", ["-C", registryRoot, "config", "user.name", "AHEAD test"]);
  execFileSync("git", ["-C", registryRoot, "remote", "add", "origin", "https://github.com/zed-industries/extensions"]);
  execFileSync("git", [
    "-c", "protocol.file.allow=always", "-C", registryRoot,
    "submodule", "add", "--quiet", state.sourceRoot, "extensions/zed",
  ]);
  execFileSync("git", ["-C", path.join(registryRoot, "extensions/zed"), "config", "remote.origin.url", "git@github.com:zed-industries/zed.git"]);
  execFileSync("git", ["-C", registryRoot, "add", ".gitmodules", "extensions/zed"]);
  execFileSync("git", ["-C", registryRoot, "commit", "--quiet", "-m", "pin html source"]);
  const registryRevision = execFileSync("git", ["-C", registryRoot, "rev-parse", "HEAD"], { encoding: "utf8" }).trim();

  const config = JSON.parse(await readFile(state.configPath, "utf8"));
  config.source_repository = "https://github.com/zed-industries/extensions";
  config.source_revision = registryRevision;
  config.extensions[0].source_path = "extensions/zed";
  config.extensions[0].revision = extensionRevision;
  await writeFile(state.configPath, JSON.stringify(config));

  const registryPath = path.join(state.root, "registry.json");
  await writeFile(registryPath, JSON.stringify({ data: [] }));
  const packageRequest = {
    sourceRoot: registryRoot,
    releaseTag: "ahead-extensions-submodule",
    cliPath: state.cliPath,
    outputDirectory: path.join(state.root, "dist"),
    registryPath,
    configPath: state.configPath,
  };
  const [entry] = await packageAheadExtensions(packageRequest);

  assert.equal(entry.source_repository, sourceRepository);
  assert.equal(entry.source_revision, extensionRevision);

  await writeFile(
    path.join(registryRoot, "extensions/zed/extensions/html/extension.toml"),
    'id = "html"\nname = "Tampered"\nversion = "0.3.2"\nrepository = "https://github.com/zed-industries/zed"\n',
  );
  await assert.rejects(packageAheadExtensions(packageRequest), /local modifications/u);
  assert.equal(JSON.parse(await readFile(registryPath, "utf8")).data[0].archive_sha256, entry.archive_sha256);
});

test("rejects local changes inside a pinned extension source", async (t) => {
  const state = await fixture(t);
  await writeFile(
    path.join(state.sourceRoot, "extensions", "html", "extension.toml"),
    'id = "html"\nname = "Tampered"\nversion = "0.3.2"\nrepository = "https://github.com/zed-industries/zed"\n',
  );
  const registryPath = path.join(state.root, "registry.json");
  await writeFile(registryPath, JSON.stringify({ data: [] }));

  await assert.rejects(packageAheadExtensions({
    sourceRoot: state.sourceRoot,
    releaseTag: "ahead-extensions-123",
    cliPath: state.cliPath,
    outputDirectory: path.join(state.root, "dist"),
    registryPath,
    configPath: state.configPath,
  }), /local modifications/u);
  assert.deepEqual(JSON.parse(await readFile(registryPath, "utf8")), { data: [] });
});
