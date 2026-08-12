import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const piRoot = dirname(dirname(fileURLToPath(import.meta.url)));

async function loadEngine() {
  const bytes = await readFile(join(piRoot, "dist", "ahead_wasm.wasm"));
  const { instance } = await WebAssembly.instantiate(bytes, {});
  const wasm = instance.exports;
  const call = (operation, input) => {
    const request = new TextEncoder().encode(
      JSON.stringify({ api_version: "ahead.engine/v0", operation, input }),
    );
    const inputPointer = wasm.ahead_alloc(request.length);
    new Uint8Array(wasm.memory.buffer, inputPointer, request.length).set(request);
    let outputPointer = 0;
    let outputLength = 0;
    try {
      const packed = wasm.ahead_dispatch(inputPointer, request.length);
      outputPointer = Number(packed >> 32n);
      outputLength = Number(packed & 0xffff_ffffn);
      const output = new Uint8Array(wasm.memory.buffer, outputPointer, outputLength).slice();
      return JSON.parse(new TextDecoder().decode(output));
    } finally {
      wasm.ahead_dealloc(inputPointer, request.length);
      if (outputPointer && outputLength) wasm.ahead_dealloc(outputPointer, outputLength);
    }
  };
  return { wasm, call };
}

const human = { kind: "human", identity: "human@example.com" };
const ai = { kind: "ai", identity: "github-copilot/test-model" };

test("raw JSON ABI exports a valid Product Change workflow", async () => {
  const { wasm, call } = await loadEngine();
  assert.equal(typeof wasm.ahead_dispatch, "function");
  const response = call("get_workflow", { workflow_id: "product-change" });
  assert.equal(response.ok, true);
  assert.equal(response.result.phases.length, 13);
  assert.equal(response.result.phases[0].id, "define");
});

test("WASM boundary enforces human ownership, gates, and capabilities", async () => {
  const { call } = await loadEngine();
  const created = call("create_run", {
    id: "wasm-smoke",
    title: "WASM smoke test",
    owner: human,
    timestamp: "2026-08-12T12:00:00Z",
    workflow_id: "product-change",
  });
  assert.equal(created.ok, true);
  let run = created.result;

  let response = call("derive_state", { run });
  assert.deepEqual(response.result.allowed_ai_capabilities, []);

  response = call("apply_event", {
    run,
    event: {
      actor: ai,
      timestamp: "2026-08-12T12:01:00Z",
      action: {
        type: "artifact_recorded",
        phase: "define",
        kind: "problem",
        path: ".ahead/ai-problem.md",
      },
    },
  });
  assert.equal(response.ok, false);
  assert.equal(response.error.code, "artifact_actor_not_allowed");

  response = call("apply_event", {
    run,
    event: {
      actor: human,
      timestamp: "2026-08-12T12:02:00Z",
      action: {
        type: "artifact_recorded",
        phase: "define",
        kind: "problem",
        path: ".ahead/problem.md",
      },
    },
  });
  assert.equal(response.ok, true);
  run = response.result;

  response = call("tool_allowed", { run, capability: "inspect" });
  assert.equal(response.result.allowed, true);
  response = call("tool_allowed", { run, capability: "modify" });
  assert.equal(response.result.allowed, false);

  response = call("apply_event", {
    run,
    event: {
      actor: ai,
      timestamp: "2026-08-12T12:03:00Z",
      action: { type: "gate_accepted", phase: "define", gate: "framing-accepted" },
    },
  });
  assert.equal(response.ok, false);
  assert.equal(response.error.code, "human_action_required");
});
