import { createHash } from "node:crypto";
import { execFileSync, spawnSync } from "node:child_process";
import { constants as fsConstants } from "node:fs";
import { lstat, mkdir, mkdtemp, readFile, realpath, rename, rm, stat, writeFile, copyFile } from "node:fs/promises";
import path from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const maxArchiveBytes = 512 * 1024 * 1024;

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function tomlString(text, key, section = "") {
  let currentSection = "";
  for (const line of text.split(/\r?\n/u)) {
    const heading = line.match(/^\s*\[([^\]]+)\]\s*$/u);
    if (heading) {
      currentSection = heading[1];
      continue;
    }
    if (currentSection !== section) continue;
    const field = line.match(new RegExp(`^\\s*${key}\\s*=\\s*"([^"]*)"\\s*(?:#.*)?$`, "u"));
    if (field) return field[1];
  }
  return undefined;
}

function inside(parent, child) {
  return child === parent || child.startsWith(`${parent}${path.sep}`);
}

async function resolveInside(parent, relativePath, description) {
  assert(typeof relativePath === "string" && relativePath.length > 0, `${description} is missing`);
  assert(!path.isAbsolute(relativePath), `${description} must be relative`);
  const resolved = await realpath(path.resolve(parent, relativePath));
  assert(inside(parent, resolved), `${description} escapes the pinned source checkout`);
  return resolved;
}

async function readRegularFile(parent, relativePath, description) {
  const filePath = path.resolve(parent, relativePath);
  const metadata = await lstat(filePath);
  assert(metadata.isFile() && !metadata.isSymbolicLink(), `${description} must be a regular file`);
  const resolved = await realpath(filePath);
  assert(inside(parent, resolved), `${description} escapes the pinned source checkout`);
  return readFile(resolved, "utf8");
}

function sourceRevision(sourceRoot) {
  return execFileSync("git", ["-C", sourceRoot, "rev-parse", "HEAD"], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
}

function normalizeRepositoryUrl(repository) {
  return repository
    .replace(/^git@github\.com:/u, "https://github.com/")
    .replace(/\.git$/u, "")
    .replace(/\/$/u, "");
}

function repositoryRemote(sourceRoot) {
  return execFileSync("git", ["-C", sourceRoot, "remote", "get-url", "origin"], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
}

function assertCleanRepository(sourceRoot, extensionId) {
  const changes = execFileSync("git", [
    "-C", sourceRoot,
    "status", "--porcelain=v1", "--untracked-files=all",
  ], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
  assert(changes.trim().length === 0, `${extensionId} source repository has local modifications`);
}

function parseArguments(argumentsList) {
  const options = {};
  for (let index = 0; index < argumentsList.length; index += 2) {
    const key = argumentsList[index];
    const value = argumentsList[index + 1];
    assert(key?.startsWith("--") && value && !value.startsWith("--"), `invalid argument: ${key ?? "<missing>"}`);
    const name = key.slice(2).replaceAll("-", "_");
    assert(["source_root", "release_tag", "output_dir", "zed_extension"].includes(name), `unknown argument: ${key}`);
    options[name] = value;
  }
  for (const name of ["source_root", "release_tag", "zed_extension"]) {
    assert(options[name], `missing --${name.replaceAll("_", "-")}`);
  }
  return options;
}

async function packageExtension({ extension, sourceRoot, config, cliPath, releaseTag, outputDirectory }) {
  assert(/^[a-z0-9][a-z0-9-]*$/u.test(extension.id), `invalid extension ID: ${extension.id}`);
  assert(/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/u.test(extension.version), `invalid version for ${extension.id}`);
  assert(/^[A-Za-z0-9][A-Za-z0-9._-]{0,100}$/u.test(releaseTag), "invalid GitHub release tag");

  const extensionSourceRoot = await resolveInside(
    sourceRoot,
    extension.source_path ?? ".",
    `${extension.id} source repository path`,
  );
  const expectedRepository = extension.repository ?? config.source_repository;
  const expectedRevision = extension.revision ?? config.source_revision;
  assert(normalizeRepositoryUrl(repositoryRemote(extensionSourceRoot)) === normalizeRepositoryUrl(expectedRepository),
    `${extension.id} source repository mismatch`);
  assert(sourceRevision(extensionSourceRoot) === expectedRevision, `${extension.id} source revision mismatch`);
  assertCleanRepository(extensionSourceRoot, extension.id);

  const extensionRoot = await resolveInside(extensionSourceRoot, extension.path, `${extension.id} source path`);
  const extensionManifest = await readRegularFile(extensionRoot, "extension.toml", `${extension.id} extension.toml`);
  const cargoManifest = await readRegularFile(extensionRoot, "Cargo.toml", `${extension.id} Cargo.toml`);
  assert(tomlString(extensionManifest, "id") === extension.id, `${extension.id} extension.toml ID mismatch`);
  assert(tomlString(extensionManifest, "version") === extension.version, `${extension.id} extension.toml version mismatch`);
  assert(tomlString(extensionManifest, "repository") === expectedRepository, `${extension.id} repository mismatch`);
  assert(tomlString(cargoManifest, "license", "package") === extension.license, `${extension.id} Cargo license mismatch`);
  const licensePath = await resolveInside(extensionSourceRoot, path.join(extension.path, extension.license_file), `${extension.id} license file`);
  assert((await stat(licensePath)).isFile(), `${extension.id} license file is not a regular file`);

  const scratchRoot = await mkdtemp(path.join(tmpdir(), "ahead-extension-build-"));
  try {
    const cliOutput = path.join(scratchRoot, "output");
    const buildScratch = path.join(scratchRoot, "scratch");
    const cli = spawnSync(cliPath, [
      "--scratch-dir", buildScratch,
      "--source-dir", extensionRoot,
      "--output-dir", cliOutput,
    ], { stdio: "inherit" });
    if (cli.error) throw cli.error;
    assert(cli.status === 0, `zed-extension failed for ${extension.id} with status ${cli.status}`);

    const packagePath = path.join(cliOutput, "archive.tar.gz");
    const packageMetadataPath = path.join(cliOutput, "manifest.json");
    const packageStat = await lstat(packagePath);
    assert(packageStat.isFile() && !packageStat.isSymbolicLink(), `${extension.id} package is not a regular file`);
    assert(packageStat.size > 0 && packageStat.size <= maxArchiveBytes, `${extension.id} package size is invalid`);
    const metadata = JSON.parse(await readFile(packageMetadataPath, "utf8"));
    assert(metadata.version === extension.version, `${extension.id} packaged version mismatch`);
    assert(metadata.repository === expectedRepository, `${extension.id} packaged repository mismatch`);
    assert(Number.isInteger(metadata.schema_version) && metadata.schema_version <= 1, `${extension.id} schema is unsupported`);
    const provides = Array.isArray(metadata.provides) ? metadata.provides : [];
    assert(provides.includes("language-servers"), `${extension.id} provides no supported feature`);
    assert(["0.6.0", "0.7.0", "0.8.0"].includes(metadata.wasm_api_version), `${extension.id} Wasm API is unsupported`);

    const archiveName = `${extension.id}-${extension.version}.tar.gz`;
    const destination = path.join(outputDirectory, archiveName);
    await copyFile(packagePath, destination, fsConstants.COPYFILE_EXCL);
    const archive = await readFile(destination);
    return {
      id: extension.id,
      name: metadata.name,
      version: metadata.version,
      description: metadata.description ?? null,
      schema_version: metadata.schema_version,
      wasm_api_version: metadata.wasm_api_version ?? null,
      provides,
      archive_sha256: createHash("sha256").update(archive).digest("hex"),
      download_url: `https://github.com/Kade-Powell/ahead/releases/download/${releaseTag}/${archiveName}`,
      source_repository: expectedRepository,
      source_revision: expectedRevision,
      license: extension.license,
      runtime_dependencies: extension.runtime_dependencies ?? [],
      requested_capabilities: extension.requested_capabilities ?? [],
    };
  } finally {
    await rm(scratchRoot, { recursive: true, force: true });
  }
}

export async function packageAheadExtensions({
  sourceRoot,
  releaseTag,
  cliPath,
  outputDirectory,
  registryPath = path.join(repoRoot, "extensions", "registry.json"),
  configPath = path.join(repoRoot, "extensions", "sources.json"),
}) {
  const sourceRootReal = await realpath(sourceRoot);
  const cliReal = await realpath(cliPath);
  assert((await stat(cliReal)).isFile(), "zed-extension CLI is not a file");
  const config = JSON.parse(await readFile(configPath, "utf8"));
  assert(typeof config.source_repository === "string" && /^https:\/\//u.test(config.source_repository), "invalid pinned source repository");
  assert(/^[a-f0-9]{40}$/u.test(config.source_revision), "invalid pinned source revision");
  assert(/^[a-f0-9]{40}$/u.test(config.zed_extension_cli_revision), "invalid pinned Zed extension CLI revision");
  assert(sourceRevision(sourceRootReal) === config.source_revision, "source checkout does not match the reviewed source revision");
  assert(normalizeRepositoryUrl(repositoryRemote(sourceRootReal)) === normalizeRepositoryUrl(config.source_repository),
    "source checkout does not match the reviewed repository");
  assertCleanRepository(sourceRootReal, "source checkout");
  assert(Array.isArray(config.extensions) && config.extensions.length > 0, "no reviewed extensions are configured");

  await mkdir(outputDirectory, { recursive: true });
  const packages = [];
  for (const extension of config.extensions) {
    packages.push(await packageExtension({
      extension,
      sourceRoot: sourceRootReal,
      config,
      cliPath: cliReal,
      releaseTag,
      outputDirectory,
    }));
  }

  const existing = JSON.parse(await readFile(registryPath, "utf8"));
  assert(Array.isArray(existing.data), "existing registry must contain a data array");
  const ids = new Set();
  const data = [...existing.data];
  for (const extension of packages) {
    assert(!ids.has(extension.id), `duplicate extension ID in build: ${extension.id}`);
    ids.add(extension.id);
    const previous = data.findIndex((entry) => entry.id === extension.id);
    if (previous >= 0) data.splice(previous, 1);
    data.push(extension);
  }
  data.sort((left, right) => left.name.localeCompare(right.name));
  const registryTemporaryPath = `${registryPath}.${process.pid}.tmp`;
  await writeFile(registryTemporaryPath, `${JSON.stringify({ data }, null, 2)}\n`, { flag: "wx" });
  await rename(registryTemporaryPath, registryPath);
  return packages;
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  const outputDirectory = path.resolve(options.output_dir ?? path.join(repoRoot, "dist", "extensions"));
  const packages = await packageAheadExtensions({
    sourceRoot: options.source_root,
    releaseTag: options.release_tag,
    cliPath: options.zed_extension,
    outputDirectory,
  });
  for (const extension of packages) {
    process.stdout.write(`Packaged ${extension.id}@${extension.version} sha256=${extension.archive_sha256}\n`);
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    process.stderr.write(`${error.stack ?? error}\n`);
    process.exitCode = 1;
  });
}
