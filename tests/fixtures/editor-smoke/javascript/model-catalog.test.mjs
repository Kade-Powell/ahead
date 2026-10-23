import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { access, mkdtemp, readFile, rm } from "node:fs/promises";
import { once } from "node:events";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { after, before, test } from "node:test";

const serverScript = fileURLToPath(new URL("../mcp_form_model.mjs", import.meta.url));
let temporaryDirectory;
const servers = new Map();

async function startCatalog(mode) {
  const portFile = join(temporaryDirectory, `${mode}.port`);
  const traceFile = join(temporaryDirectory, `${mode}.trace`);
  const child = spawn(process.execPath, [serverScript, portFile, traceFile, mode], {
    stdio: "ignore",
  });
  servers.set(mode, { child, traceFile });

  for (let attempt = 0; attempt < 100; attempt++) {
    try {
      await access(portFile);
    } catch (error) {
      if (child.exitCode !== null) {
        throw new Error(`${mode} catalog exited before listening`, { cause: error });
      }
      await new Promise((resolve) => setTimeout(resolve, 20));
      continue;
    }
    const port = Number(await readFile(portFile, "utf8"));
    assert.ok(Number.isInteger(port) && port > 0, `${mode} must publish a valid port`);
    return port;
  }

  throw new Error(`${mode} catalog did not start`);
}

async function fetchCatalog(port) {
  const response = await fetch(`http://127.0.0.1:${port}/v1/models`);
  assert.equal(response.status, 200);
  return response.json();
}

before(async () => {
  temporaryDirectory = await mkdtemp(join(tmpdir(), "ahead-model-catalog-test-"));
  const ports = await Promise.all([
    startCatalog("catalog-a"),
    startCatalog("catalog-b"),
  ]);
  const catalogs = await Promise.all(ports.map(fetchCatalog));
  servers.set("catalogs", catalogs);
});

after(async () => {
  const children = [...servers.values()]
    .filter((entry) => entry && typeof entry === "object" && "child" in entry)
    .map(({ child }) => child);
  const exits = children.map((child) =>
    child.exitCode !== null || child.signalCode !== null
      ? Promise.resolve()
      : once(child, "exit"),
  );
  for (const child of children) child.kill("SIGTERM");
  await Promise.all(exits);
  if (temporaryDirectory) {
    await rm(temporaryDirectory, { recursive: true, force: true });
  }
});

test("provider catalogs keep reasoning metadata scoped to their endpoint", async () => {
  const catalogs = servers.get("catalogs");
  assert.ok(Array.isArray(catalogs));
  const [catalogA, catalogB] = catalogs;

  assert.equal(catalogA.data.length, 121);
  assert.equal(catalogB.data.length, 121);

  const sharedModelA = catalogA.data.find(({ id }) => id === "shared-model");
  const sharedModelB = catalogB.data.find(({ id }) => id === "shared-model");
  assert.ok(sharedModelA);
  assert.ok(sharedModelB);
  assert.equal(sharedModelA.name, "Shared Model");
  assert.deepEqual(sharedModelA.reasoning, {
    default_effort: "medium",
    supported_efforts: ["low", "medium", "high"],
  });
  assert.equal(Object.hasOwn(sharedModelB, "reasoning"), false);

  for (const mode of ["catalog-a", "catalog-b"]) {
    const trace = await readFile(servers.get(mode).traceFile, "utf8");
    assert.equal(JSON.parse(trace.trim()).type, "model_catalog_request");
  }
});
