import { readFile } from "node:fs/promises";
import type {
  Actor,
  Capability,
  EngineErrorShape,
  EventAction,
  Run,
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
    const wasm = instantiated.instance.exports as WasmExports;
    for (const name of ["memory", "ahead_alloc", "ahead_dealloc", "ahead_dispatch"]) {
      if (!(name in wasm)) {
        throw new Error(`AHEAD WebAssembly module is missing export: ${name}`);
      }
    }
    return new AheadEngine(wasm);
  }

  getWorkflow(workflowId = "product-change"): WorkflowDefinition {
    return this.call("get_workflow", { workflow_id: workflowId });
  }

  createRun(input: {
    id: string;
    title: string;
    owner: Actor;
    timestamp: string;
    workflow_id?: string;
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

  toolAllowed(run: Run, capability: Capability): { allowed: boolean; reason: string; capability: Capability } {
    return this.call("tool_allowed", { run, capability });
  }

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
      const response = JSON.parse(new TextDecoder().decode(output)) as EngineResponse<T>;
      if (!response.ok || response.result === undefined) {
        const error = response.error ?? { code: "unknown_engine_error", message: "AHEAD engine call failed" };
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
