import { readFile } from "node:fs/promises";
import type {
  Actor,
  Capability,
  EngineErrorShape,
  EventAction,
  Run,
  RunPolicy,
  RunState,
  WorkflowDefinition,
} from "./types.js";

const ENGINE_API_VERSION = "ahead.engine/v0";

interface WasmExports extends WebAssembly.Exports {
  memory: WebAssembly.Memory;
  ahead_alloc(length: number): number;
  ahead_dealloc(pointer: number, length: number): void;
  ahead_dispatch(pointer: number, length: number): bigint;
}

interface EngineResponse<T> {
  ok: boolean;
  result?: T;
  error?: EngineErrorShape;
}

export class AheadEngineError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = "AheadEngineError";
  }
}

export class AheadEngine {
  private constructor(private readonly wasm: WasmExports) {}

  static async load(path: string): Promise<AheadEngine> {
    const bytes = await readFile(path);
    const instantiated = await WebAssembly.instantiate(bytes, {});
    const wasm = instantiated.instance.exports;
    if (!isWasmExports(wasm)) {
      throw new Error("AHEAD WebAssembly module does not expose the required engine ABI");
    }
    return new AheadEngine(wasm);
  }

  getWorkflow(workflowId = "product-change"): WorkflowDefinition {
    return this.call("get_workflow", { workflow_id: workflowId });
  }

  listWorkflows(): WorkflowDefinition[] {
    return this.call("list_workflows", {});
  }

  createRun(input: {
    id: string;
    title: string;
    owner: Actor;
    timestamp: string;
    workflow_id: string;
    policy?: RunPolicy;
  }): Run {
    return this.call("create_run", input);
  }

  deriveState(run: Run): RunState {
    return this.call("derive_state", { run });
  }

  validateRun(run: Run): RunState {
    return this.call("validate_run", { run });
  }

  applyEvent(run: Run, actor: Actor, action: EventAction): Run {
    return this.call("apply_event", {
      run,
      event: { actor, timestamp: new Date().toISOString(), action },
    });
  }

  toolAllowed(
    run: Run,
    capability: Capability,
  ): { allowed: boolean; reason: string; capability: Capability } {
    return this.call("tool_allowed", { run, capability });
  }

  // oxlint-disable-next-line typescript/no-unnecessary-type-parameters -- the operation selects the Rust response type
  private call<T>(operation: string, input: unknown): T {
    const request = new TextEncoder().encode(
      JSON.stringify({ api_version: ENGINE_API_VERSION, operation, input }),
    );
    const inputPointer = this.wasm.ahead_alloc(request.length);
    new Uint8Array(this.wasm.memory.buffer, inputPointer, request.length).set(request);

    let outputPointer = 0;
    let outputLength = 0;
    try {
      const packed = this.wasm.ahead_dispatch(inputPointer, request.length);
      outputPointer = Number(packed >> 32n);
      outputLength = Number(packed & 0xffff_ffffn);
      const output = new Uint8Array(this.wasm.memory.buffer, outputPointer, outputLength).slice();
      // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- the versioned Rust ABI owns this response envelope
      const response = JSON.parse(new TextDecoder().decode(output)) as EngineResponse<T>;
      if (!response.ok || response.result === undefined) {
        const error = response.error ?? {
          code: "unknown_engine_error",
          message: "AHEAD engine call failed",
        };
        throw new AheadEngineError(error.code, error.message);
      }
      return response.result;
    } finally {
      this.wasm.ahead_dealloc(inputPointer, request.length);
      if (outputPointer !== 0 && outputLength !== 0) {
        this.wasm.ahead_dealloc(outputPointer, outputLength);
      }
    }
  }
}

function isWasmExports(exports: WebAssembly.Exports): exports is WasmExports {
  return (
    exports.memory instanceof WebAssembly.Memory &&
    typeof exports.ahead_alloc === "function" &&
    typeof exports.ahead_dealloc === "function" &&
    typeof exports.ahead_dispatch === "function"
  );
}
