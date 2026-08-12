import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import type {
  ExtensionAPI,
  ExtensionCommandContext,
  ExtensionContext,
} from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { AheadEngine, AheadEngineError } from "./engine.js";
import { humanActor, projectRoot, RunStore } from "./storage.js";
import type { Actor, Capability, EventAction, Run, RunState } from "./types.js";

const wasmPath =
  process.env.AHEAD_WASM_PATH || fileURLToPath(new URL("../dist/ahead_wasm.wasm", import.meta.url));
const instructionDirectory = fileURLToPath(new URL("../generated/product-change", import.meta.url));
const enginePromise = AheadEngine.load(wasmPath);
const instructions = new Map<string, string>();

const toolCapabilities: Record<string, Capability> = {
  read: "inspect",
  grep: "inspect",
  find: "inspect",
  ls: "inspect",
  edit: "modify",
  write: "modify",
  bash: "execute",
};

const EmptyParams = Type.Object({});
const RecordArtifactParams = Type.Object({
  kind: Type.String({ description: "Artifact kind permitted for AI in the active phase" }),
  content: Type.String({ description: "Complete Markdown artifact content", maxLength: 100_000 }),
});

export default function aheadExtension(pi: ExtensionAPI): void {
  pi.registerCommand("ahead-start", {
    description: "Start a Product Change workflow owned by the current human",
    handler: async (args, ctx) => command(ctx, async () => {
      const engine = await enginePromise;
      const store = storeFor(ctx);
      const current = await store.loadCurrent();
      if (current && !engine.deriveState(current).closed) {
        throw new AheadEngineError(
          "active_run_exists",
          `run ${current.id} is still active; close it before starting another`,
        );
      }
      const title = args.trim() || (ctx.hasUI ? await ctx.ui.input("AHEAD Product Change", "Run title") : undefined);
      if (!title?.trim()) return;
      const owner = humanActor(store.projectRoot);
      const run = engine.createRun({
        id: store.newRunId(),
        title: title.trim(),
        owner,
        timestamp: new Date().toISOString(),
        workflow_id: "product-change",
      });
      await store.save(run);
      await refreshUi(ctx, run);
      ctx.ui.notify(`Started AHEAD run ${run.id}. Record the human-owned problem with /ahead-record problem.`, "info");
    }),
  });

  pi.registerCommand("ahead-status", {
    description: "Show the active AHEAD phase, evidence, gate, and blockers",
    handler: async (_args, ctx) => command(ctx, async () => {
      const run = await requireRun(ctx);
      const state = (await enginePromise).deriveState(run);
      await refreshUi(ctx, run);
      ctx.ui.notify(formatState(state), state.blockers.length ? "warning" : "info");
    }),
  });

  pi.registerCommand("ahead-record", {
    description: "Write and record a human-owned artifact for the active phase",
    handler: async (args, ctx) => command(ctx, async () => {
      if (!ctx.hasUI) throw new Error("/ahead-record requires interactive or RPC UI support");
      const engine = await enginePromise;
      const store = storeFor(ctx);
      const run = await requireRun(ctx);
      const state = engine.deriveState(run);
      const allowed = state.artifacts.filter((artifact) => artifact.actor !== "ai");
      let kind = args.trim();
      if (!kind) {
        kind = (await ctx.ui.select(
          `Record human artifact · ${state.phase.title}`,
          allowed.map((artifact) => artifact.kind),
        )) ?? "";
      }
      const artifact = allowed.find((candidate) => candidate.kind === kind);
      if (!artifact) {
        throw new AheadEngineError(
          "artifact_not_human_owned",
          `human cannot record ${kind || "that artifact"} in phase ${state.phase.id}; choose: ${allowed.map((item) => item.kind).join(", ")}`,
        );
      }
      const content = await ctx.ui.editor(
        `AHEAD · ${artifact.title}`,
        artifactTemplate(run, state, artifact.kind, artifact.title),
      );
      if (!content?.trim()) return;
      const path = store.artifactPath(run, state.phase.id, artifact.kind);
      const action: EventAction = {
        type: "artifact_recorded",
        phase: state.phase.id,
        kind: artifact.kind,
        path: path.relative,
      };
      const updated = engine.applyEvent(run, humanActor(store.projectRoot), action);
      await store.writeArtifact(path.absolute, content);
      await store.save(updated);
      await refreshUi(ctx, updated);
      ctx.ui.notify(`Recorded ${artifact.kind} as ${path.relative}`, "info");
    }),
  });

  pi.registerCommand("ahead-accept", {
    description: "Human acceptance of the active phase gate",
    handler: async (_args, ctx) => command(ctx, async () => {
      if (!ctx.hasUI) throw new Error("/ahead-accept requires interactive or RPC UI support");
      const engine = await enginePromise;
      const store = storeFor(ctx);
      const run = await requireRun(ctx);
      const state = engine.deriveState(run);
      const confirmed = await ctx.ui.confirm(
        `Accept ${state.gate.id}?`,
        `${state.gate.title}\n\nThis records human acceptance as ${humanActor(store.projectRoot).identity}.`,
      );
      if (!confirmed) return;
      const updated = engine.applyEvent(run, humanActor(store.projectRoot), {
        type: "gate_accepted",
        phase: state.phase.id,
        gate: state.gate.id,
      });
      await store.save(updated);
      await refreshUi(ctx, updated);
      ctx.ui.notify(`Accepted gate ${state.gate.id}. Use /ahead-advance when ready.`, "info");
    }),
  });

  pi.registerCommand("ahead-advance", {
    description: "Human transition to the next phase, or close the final phase",
    handler: async (_args, ctx) => command(ctx, async () => {
      if (!ctx.hasUI) throw new Error("/ahead-advance requires interactive or RPC UI support");
      const engine = await enginePromise;
      const store = storeFor(ctx);
      const run = await requireRun(ctx);
      const state = engine.deriveState(run);
      const destination = state.phase.next ?? "closed";
      const confirmed = await ctx.ui.confirm(
        state.phase.next ? `Advance to ${state.phase.next}?` : "Close this AHEAD run?",
        `Current phase: ${state.phase.title}\nDestination: ${destination}\nActor: ${humanActor(store.projectRoot).identity}`,
      );
      if (!confirmed) return;
      const action: EventAction = state.phase.next
        ? {
            type: "phase_transitioned",
            from: state.phase.id,
            to: state.phase.next,
            direction: "advance",
          }
        : { type: "run_closed", phase: state.phase.id };
      const updated = engine.applyEvent(run, humanActor(store.projectRoot), action);
      await store.save(updated);
      await refreshUi(ctx, updated);
      ctx.ui.notify(state.phase.next ? `Advanced to ${state.phase.next}.` : "AHEAD run closed.", "info");
    }),
  });

  pi.registerCommand("ahead-return", {
    description: "Human return to an allowed earlier phase with a recorded reason",
    handler: async (args, ctx) => command(ctx, async () => {
      if (!ctx.hasUI) throw new Error("/ahead-return requires interactive or RPC UI support");
      const engine = await enginePromise;
      const store = storeFor(ctx);
      const run = await requireRun(ctx);
      const state = engine.deriveState(run);
      if (!state.return_targets.length) {
        throw new AheadEngineError("no_return_target", `phase ${state.phase.id} has no return transition`);
      }
      let target = args.trim();
      if (!target) target = (await ctx.ui.select("Return to which phase?", state.return_targets)) ?? "";
      if (!state.return_targets.includes(target)) {
        throw new AheadEngineError(
          "invalid_return",
          `phase ${state.phase.id} can return only to: ${state.return_targets.join(", ")}`,
        );
      }
      const reason = await ctx.ui.editor(`Why return to ${target}?`);
      if (!reason?.trim()) return;
      const confirmed = await ctx.ui.confirm(
        `Return to ${target}?`,
        "This creates a new phase visit. Earlier artifacts remain in history but will not satisfy the reopened gate.",
      );
      if (!confirmed) return;
      const updated = engine.applyEvent(run, humanActor(store.projectRoot), {
        type: "phase_transitioned",
        from: state.phase.id,
        to: target,
        direction: "return",
        reason: reason.trim(),
      });
      await store.save(updated);
      await refreshUi(ctx, updated);
      ctx.ui.notify(`Returned to ${target}. New evidence and human gate acceptance are required.`, "warning");
    }),
  });

  pi.registerCommand("ahead-help", {
    description: "Show AHEAD Pi commands and the human/AI boundary",
    handler: async (_args, ctx) => {
      ctx.ui.notify(
        [
          "/ahead-start [title] — start a human-owned Product Change run",
          "/ahead-status — show phase, evidence, gate, and blockers",
          "/ahead-record [kind] — human writes an artifact",
          "/ahead-accept — human accepts the current gate",
          "/ahead-advance — human advances or closes",
          "/ahead-return [phase] — human reopens an allowed earlier phase",
          "",
          "AI can inspect context and record only AI-permitted artifacts. It cannot accept gates or transition the run.",
        ].join("\n"),
        "info",
      );
    },
  });

  pi.registerTool({
    name: "ahead_get_context",
    label: "AHEAD context",
    description: "Read the authoritative active AHEAD workflow state, phase contract, artifacts, gate, and blockers.",
    promptSnippet: "Read the active AHEAD workflow state and human/AI boundaries.",
    parameters: EmptyParams,
    async execute(_toolCallId, _params, _signal, _onUpdate, ctx) {
      return toolResult(async () => {
        const run = await requireRun(ctx);
        const state = (await enginePromise).deriveState(run);
        return { run, state };
      });
    },
  });

  pi.registerTool({
    name: "ahead_record_artifact",
    label: "Record AHEAD artifact",
    description: "Persist an AI-owned or shared artifact permitted by the active phase. Cannot record human-owned artifacts.",
    promptSnippet: "Record an AI-permitted artifact in the active AHEAD run.",
    parameters: RecordArtifactParams,
    async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
      return toolResult(async () => {
        const engine = await enginePromise;
        const store = storeFor(ctx);
        const run = await requireRun(ctx);
        const state = engine.deriveState(run);
        const artifact = state.artifacts.find((candidate) => candidate.kind === params.kind);
        if (!artifact || artifact.actor === "human") {
          throw new AheadEngineError(
            "artifact_not_ai_owned",
            `AI cannot record ${params.kind} in phase ${state.phase.id}`,
          );
        }
        const path = store.artifactPath(run, state.phase.id, artifact.kind);
        const updated = engine.applyEvent(run, aiActor(ctx), {
          type: "artifact_recorded",
          phase: state.phase.id,
          kind: artifact.kind,
          path: path.relative,
        });
        await store.writeArtifact(path.absolute, params.content);
        await store.save(updated);
        await refreshUi(ctx, updated);
        return {
          recorded: artifact.kind,
          path: path.relative,
          state: engine.deriveState(updated),
        };
      });
    },
  });

  pi.registerTool({
    name: "ahead_request_transition",
    label: "Request AHEAD transition",
    description: "Report whether a human can advance the active AHEAD phase. This tool never accepts a gate or transitions state.",
    parameters: EmptyParams,
    async execute(_toolCallId, _params, _signal, _onUpdate, ctx) {
      return toolResult(async () => {
        const state = (await enginePromise).deriveState(await requireRun(ctx));
        return {
          requested: true,
          transitioned: false,
          message: state.can_advance
            ? `The gate is accepted. Ask the human to use /ahead-advance to ${state.phase.next ?? "close the run"}.`
            : "The phase cannot advance. The human must resolve the blockers and accept the gate.",
          blockers: state.blockers,
        };
      });
    },
  });

  pi.registerTool({
    name: "ahead_validate",
    label: "Validate AHEAD run",
    description: "Replay and validate the active AHEAD event log against the embedded workflow contract.",
    parameters: EmptyParams,
    async execute(_toolCallId, _params, _signal, _onUpdate, ctx) {
      return toolResult(async () => ({ valid: true, state: (await enginePromise).validateRun(await requireRun(ctx)) }));
    },
  });

  pi.on("session_start", async (_event, ctx) => {
    try {
      await refreshUi(ctx);
    } catch (error) {
      ctx.ui.setStatus("ahead", "AHEAD · invalid state");
      ctx.ui.notify(errorMessage(error), "error");
    }
  });

  pi.on("before_agent_start", async (event, ctx) => {
    const run = await storeFor(ctx).loadCurrent();
    if (!run) return;
    const state = (await enginePromise).deriveState(run);
    const phaseInstructions = await loadInstructions(state.phase.id);
    const liveContext = [
      "# Live AHEAD run",
      `- Run: ${run.id} — ${run.title}`,
      `- Phase: ${state.phase.id} visit ${state.phase.visit}`,
      `- Gate accepted: ${state.gate.accepted}`,
      `- Current blockers: ${state.blockers.length ? state.blockers.join("; ") : "none"}`,
      `- Allowed AI capabilities: ${state.allowed_ai_capabilities.length ? state.allowed_ai_capabilities.join(", ") : "none"}`,
    ].join("\n");
    return { systemPrompt: `${event.systemPrompt}\n\n${phaseInstructions}\n\n${liveContext}\n` };
  });

  pi.on("tool_call", async (event, ctx) => {
    if (event.toolName.startsWith("ahead_")) return;
    const run = await storeFor(ctx).loadCurrent();
    if (!run) return;
    const capability = toolCapabilities[event.toolName];
    if (!capability) {
      return {
        block: true,
        reason: `AHEAD blocked unclassified tool ${event.toolName}. The Pi adapter must map every model-invoked tool to an explicit workflow capability.`,
      };
    }
    try {
      const decision = (await enginePromise).toolAllowed(run, capability);
      if (!decision.allowed) return { block: true, reason: `AHEAD: ${decision.reason}` };
    } catch (error) {
      return { block: true, reason: `AHEAD state validation failed: ${errorMessage(error)}` };
    }
  });
}

function storeFor(ctx: ExtensionContext): RunStore {
  return new RunStore(projectRoot(ctx.cwd));
}

async function requireRun(ctx: ExtensionContext): Promise<Run> {
  const run = await storeFor(ctx).loadCurrent();
  if (!run) throw new AheadEngineError("no_active_run", "no active AHEAD run; use /ahead-start [title]");
  return run;
}

function aiActor(ctx: ExtensionContext): Actor {
  const model = ctx.model;
  return {
    kind: "ai",
    identity: model ? `${model.provider}/${model.id}` : "pi/unknown-model",
  };
}

async function refreshUi(ctx: ExtensionContext, supplied?: Run): Promise<void> {
  const run = supplied ?? (await storeFor(ctx).loadCurrent());
  if (!run) {
    ctx.ui.setStatus("ahead", undefined);
    ctx.ui.setWidget("ahead", undefined);
    return;
  }
  const state = (await enginePromise).deriveState(run);
  ctx.ui.setStatus(
    "ahead",
    state.closed
      ? `AHEAD · ${state.workflow_id} · closed`
      : `AHEAD · ${state.phase.id}#${state.phase.visit} · ${state.blockers.length} blocker${state.blockers.length === 1 ? "" : "s"}`,
  );
  ctx.ui.setWidget(
    "ahead",
    [
      `AHEAD · ${run.title}`,
      state.closed
        ? "Closed"
        : `${state.phase.title} · visit ${state.phase.visit} · gate ${state.gate.accepted ? "accepted" : "open"}`,
      state.blockers.length
        ? `Next: ${state.blockers[0]}`
        : state.phase.next
          ? "Next: /ahead-advance"
          : "Next: /ahead-advance (closes run)",
    ],
    { placement: "aboveEditor" },
  );
}

async function loadInstructions(phase: string): Promise<string> {
  const cached = instructions.get(phase);
  if (cached) return cached;
  const content = await readFile(`${instructionDirectory}/${phase}.md`, "utf8");
  instructions.set(phase, content);
  return content;
}

function formatState(state: RunState): string {
  const artifacts = state.artifacts
    .map((artifact) => `${artifact.present ? "✓" : artifact.required ? "○" : "·"} ${artifact.kind} (${artifact.actor})`)
    .join("\n");
  return [
    `${state.title} · ${state.workflow_id}@${state.workflow_version}`,
    `Phase: ${state.phase.title} (${state.phase.id}) · visit ${state.phase.visit}`,
    `Gate: ${state.gate.id} · ${state.gate.accepted ? `accepted by ${state.gate.accepted_by?.identity}` : "open"}`,
    `AI capabilities: ${state.allowed_ai_capabilities.join(", ") || "none"}`,
    "Artifacts:",
    artifacts,
    `Blockers: ${state.blockers.join("; ") || "none"}`,
    `Return targets: ${state.return_targets.join(", ") || "none"}`,
  ].join("\n");
}

function artifactTemplate(run: Run, state: RunState, kind: string, title: string): string {
  return [
    `# ${title}`,
    "",
    `AHEAD run: ${run.id}`,
    `Phase: ${state.phase.id} (visit ${state.phase.visit})`,
    `Artifact: ${kind}`,
    "",
    "<!-- Replace this comment with the human-authored record. Preserve evidence, uncertainty, and rationale. -->",
    "",
  ].join("\n");
}

async function command(ctx: ExtensionCommandContext, action: () => Promise<void>): Promise<void> {
  try {
    await action();
  } catch (error) {
    ctx.ui.notify(errorMessage(error), "error");
  }
}

async function toolResult(action: () => Promise<unknown>) {
  try {
    const result = await action();
    return { content: [{ type: "text" as const, text: JSON.stringify(result, null, 2) }], details: result };
  } catch (error) {
    const message = errorMessage(error);
    return { content: [{ type: "text" as const, text: `AHEAD error: ${message}` }], details: { error: message }, isError: true };
  }
}

function errorMessage(error: unknown): string {
  if (error instanceof AheadEngineError) return `${error.code}: ${error.message}`;
  return error instanceof Error ? error.message : String(error);
}
