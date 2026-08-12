import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import type {
  ExtensionAPI,
  ExtensionCommandContext,
  ExtensionContext,
} from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { AheadEngine, AheadEngineError } from "./engine.js";
import {
  buildArtifactTemplate,
  buildWidgetLines,
  nextAction,
  phaseGuide,
  phasePosition,
} from "./guidance.js";
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
  pi.registerCommand("ahead", {
    description: "Enter or continue the guided AHEAD mode",
    handler: async (args, ctx) => command(ctx, async () => {
      await openAheadMode(pi, args, ctx);
    }),
  });

  pi.registerCommand("ahead-start", {
    description: "Advanced: start a Product Change run directly",
    handler: async (args, ctx) => command(ctx, async () => {
      await startRun(ctx, args);
    }),
  });

  pi.registerCommand("ahead-status", {
    description: "Advanced: show the raw active AHEAD phase contract",
    handler: async (_args, ctx) => command(ctx, async () => {
      const run = await requireRun(ctx);
      const state = (await enginePromise).deriveState(run);
      await refreshUi(ctx, run);
      ctx.ui.notify(formatState(state), state.blockers.length ? "warning" : "info");
    }),
  });

  pi.registerCommand("ahead-record", {
    description: "Advanced: record a human-owned artifact directly",
    handler: async (args, ctx) => command(ctx, async () => {
      await recordHumanArtifact(ctx, args.trim());
    }),
  });

  pi.registerCommand("ahead-accept", {
    description: "Advanced: accept the active gate without advancing",
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
    description: "Advanced: advance an already accepted gate",
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
    description: "Advanced: return to an earlier phase with a reason",
    handler: async (args, ctx) => command(ctx, async () => {
      await returnToEarlierPhase(ctx, args);
    }),
  });

  pi.registerCommand("ahead-help", {
    description: "Explain guided AHEAD mode and its authority boundary",
    handler: async (_args, ctx) => {
      ctx.ui.notify(
        [
          "/ahead [title] — enter, resume, or act in guided AHEAD mode",
          "",
          "Once started, the repository run remains in AHEAD mode until an accountable human closes the outcome.",
          "Use normal conversation to think and work with AI. Run /ahead whenever you want the next valid action.",
          "The persistent guide explains what you own, what AI may do, required evidence, and what happens next.",
          "",
          "Advanced fallback commands: /ahead-status, /ahead-record, /ahead-accept, /ahead-advance, /ahead-return.",
          "AI can record only AI/shared artifacts allowed in the active phase. It cannot accept gates, transition, approve, deploy, or close the run.",
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
            ? `The gate is accepted. Ask the human to use /ahead to ${state.phase.next ?? "close the run"}.`
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
      const run = await storeFor(ctx).loadCurrent();
      if (run && !(await enginePromise).deriveState(run).closed && ctx.hasUI) {
        const state = (await enginePromise).deriveState(run);
        ctx.ui.notify(
          `AHEAD mode resumed · ${state.phase.title}. Human leads, AI assists. Run /ahead for the next guided action.`,
          "info",
        );
      }
    } catch (error) {
      ctx.ui.setStatus("ahead", "AHEAD · invalid state");
      ctx.ui.notify(errorMessage(error), "error");
    }
  });

  pi.on("before_agent_start", async (event, ctx) => {
    const run = await storeFor(ctx).loadCurrent();
    if (!run) return;
    const engine = await enginePromise;
    const state = engine.deriveState(run);
    if (state.closed) return;
    const workflow = engine.getWorkflow(run.workflow_id);
    const guidance = phaseGuide(state.phase.id);
    const action = nextAction(state, workflow);
    const phaseInstructions = await loadInstructions(state.phase.id);
    const liveContext = [
      "# Live AHEAD run",
      `- Run: ${run.id} — ${run.title}`,
      `- Phase: ${state.phase.id} visit ${state.phase.visit}`,
      `- Gate accepted: ${state.gate.accepted}`,
      `- Current blockers: ${state.blockers.length ? state.blockers.join("; ") : "none"}`,
      `- Allowed AI capabilities: ${state.allowed_ai_capabilities.length ? state.allowed_ai_capabilities.join(", ") : "none"}`,
      `- Human responsibility: ${guidance.human}`,
      `- AI role: ${guidance.ai}`,
      `- Next guided action (${action.actor}): ${action.label}`,
      "",
      "## AHEAD interaction behavior",
      "- AHEAD is an active working mode, not a command checklist. Help the human understand and complete the active phase through normal conversation.",
      "- When asked what to do, explain the current expectation in plain language; do not merely repeat artifact identifiers or slash commands.",
      "- Never author a human-owned artifact, make a human decision, accept a gate, transition the run, approve a change, or claim accountability.",
      "- When required AI-owned work is ready, record it with ahead_record_artifact and explain what the human must validate or decide.",
      "- Treat AI review findings as hypotheses. Independent human review remains required for lasting engineering changes.",
    ].join("\n");
    return { systemPrompt: `${event.systemPrompt}\n\n${phaseInstructions}\n\n${liveContext}\n` };
  });

  pi.on("tool_call", async (event, ctx) => {
    if (event.toolName.startsWith("ahead_")) return;
    const run = await storeFor(ctx).loadCurrent();
    if (!run) return;
    if ((await enginePromise).deriveState(run).closed) return;
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

interface GuidedAction {
  label: string;
  run: () => Promise<void>;
}

async function openAheadMode(pi: ExtensionAPI, args: string, ctx: ExtensionCommandContext): Promise<void> {
  const engine = await enginePromise;
  const store = storeFor(ctx);
  let run = await store.loadCurrent();

  if (run && engine.deriveState(run).closed) {
    if (!ctx.hasUI) {
      ctx.ui.notify("The current AHEAD run is complete. Start new work in an interactive Pi session.", "info");
      return;
    }
    const choice = await ctx.ui.select("AHEAD work is complete", [
      "Start a new Product Change",
      "View the completed run",
    ]);
    if (choice === "View the completed run") {
      ctx.ui.notify(formatState(engine.deriveState(run)), "info");
      return;
    }
    if (choice !== "Start a new Product Change") return;
    run = undefined;
  }

  if (!run) {
    run = await startRun(ctx, args);
    if (!run) return;
  }

  await refreshUi(ctx, run);
  if (!ctx.hasUI) {
    ctx.ui.notify(formatState(engine.deriveState(run)), "info");
    return;
  }

  const state = engine.deriveState(run);
  const workflow = engine.getWorkflow(run.workflow_id);
  const guidance = phaseGuide(state.phase.id);
  const action = nextAction(state, workflow);
  const actions: GuidedAction[] = [];
  const missingRequired = state.artifacts.filter((artifact) => artifact.required && !artifact.present);
  if (action.artifactKind) {
    actions.push({
      label: action.label,
      run: action.actor === "ai"
        ? async () => requestAiAssistance(pi, state, action.artifactKind)
        : async () => recordHumanArtifact(ctx, action.artifactKind ?? ""),
    });
  }

  if (action.optional) {
    const nextHumanArtifact = missingRequired.find((artifact) => artifact.actor === "human");
    if (nextHumanArtifact) {
      actions.push({
        label: `Continue without optional AI challenge · Write ${nextHumanArtifact.title}`,
        run: async () => recordHumanArtifact(ctx, nextHumanArtifact.kind),
      });
    }
  }

  if (missingRequired.length === 0 && !action.artifactKind) {
    actions.push({
      label: state.gate.accepted
        ? action.label
        : `Accept and continue · ${state.gate.title}`,
      run: async () => acceptAndContinue(ctx),
    });
  }

  if (
    state.allowed_ai_capabilities.length > 0
    && action.actor !== "ai"
    && !missingRequired.some((artifact) => artifact.actor === "ai")
  ) {
    actions.push({
      label: `Ask AI to assist · ${state.phase.title}`,
      run: async () => requestAiAssistance(pi, state),
    });
  }

  if (state.return_targets.length > 0) {
    actions.push({
      label: "Return to an earlier phase",
      run: async () => returnToEarlierPhase(ctx, ""),
    });
  }

  actions.push({
    label: "Explain this phase and its expectations",
    run: async () => {
      ctx.ui.notify(
        [
          `${state.phase.title} · Human leads, AI assists`,
          `Goal: ${guidance.objective}`,
          `You: ${guidance.human}`,
          `AI: ${guidance.ai}`,
          `Gate: ${state.gate.title}`,
        ].join("\n"),
        "info",
      );
    },
  });

  const selected = await ctx.ui.select(
    `AHEAD mode · ${state.phase.title}\nNext (${action.actor === "human" ? "you" : "AI"}): ${action.label}`,
    actions.map((candidate) => candidate.label),
  );
  const chosen = actions.find((candidate) => candidate.label === selected);
  if (chosen) await chosen.run();
}

async function startRun(ctx: ExtensionCommandContext, requestedTitle: string): Promise<Run | undefined> {
  const engine = await enginePromise;
  const store = storeFor(ctx);
  const current = await store.loadCurrent();
  if (current && !engine.deriveState(current).closed) {
    throw new AheadEngineError(
      "active_run_exists",
      `run ${current.id} is still active; use /ahead to continue it`,
    );
  }

  const title = requestedTitle.trim()
    || (ctx.hasUI ? await ctx.ui.input("Enter AHEAD mode · Product Change", "What work are you doing?") : undefined);
  if (!title?.trim()) return undefined;

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
  ctx.ui.notify(
    [
      `AHEAD mode started · ${run.title}`,
      "Human leads · AI assists",
      "This run remains active in the repository until an accountable human closes the outcome.",
      "Use /ahead for the next guided action; use normal conversation to think and work with AI.",
    ].join("\n"),
    "info",
  );
  return run;
}

async function recordHumanArtifact(ctx: ExtensionCommandContext, requestedKind: string): Promise<void> {
  if (!ctx.hasUI) throw new Error("Recording a human artifact requires interactive or RPC UI support");
  const engine = await enginePromise;
  const store = storeFor(ctx);
  const run = await requireRun(ctx);
  const state = engine.deriveState(run);
  const allowed = state.artifacts.filter((artifact) => artifact.actor !== "ai" && !artifact.present);
  let kind = requestedKind.trim();
  if (!kind) {
    kind = (await ctx.ui.select(
      `AHEAD mode · Write for ${state.phase.title}`,
      allowed.map((artifact) => artifact.title),
    )) ?? "";
    kind = allowed.find((artifact) => artifact.title === kind)?.kind ?? kind;
  }
  const artifact = allowed.find((candidate) => candidate.kind === kind);
  if (!artifact) {
    throw new AheadEngineError(
      "artifact_not_human_owned",
      `there is no unrecorded human artifact named ${kind || "that"} in ${state.phase.title}`,
    );
  }

  const content = await ctx.ui.editor(
    `AHEAD mode · ${artifact.title} · write in your own words`,
    buildArtifactTemplate(run, state, artifact.kind, artifact.title),
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
  ctx.ui.notify(
    `Saved ${artifact.title}. AHEAD mode remains active; continue the conversation or run /ahead for the next guided action.`,
    "info",
  );
}

function requestAiAssistance(pi: ExtensionAPI, state: RunState, requiredKind?: string): void {
  const guidance = phaseGuide(state.phase.id);
  const artifact = requiredKind
    ? state.artifacts.find((candidate) => candidate.kind === requiredKind)
    : undefined;
  const request = artifact
    ? [
        `AHEAD mode: perform the ${artifact.required ? "required" : "recommended"} ${state.phase.title} work for the exact current evidence and changeset.`,
        `Produce ${artifact.title}.`,
        `Follow the active human/AI boundary: ${guidance.ai}`,
        `Use ahead_get_context first, then record the completed artifact as ${artifact.kind} with ahead_record_artifact.`,
        "Treat findings as hypotheses for human disposition. Do not accept the gate or transition the run.",
      ].join("\n")
    : [
        `AHEAD mode: assist with the active ${state.phase.title} phase.`,
        guidance.ai,
        "Use ahead_get_context before acting. Stay within the allowed capabilities and never author a human-owned artifact or decision.",
        "Explain what you found and what the human must understand or decide next.",
      ].join("\n");
  pi.sendUserMessage(request);
}

async function acceptAndContinue(ctx: ExtensionCommandContext): Promise<void> {
  if (!ctx.hasUI) throw new Error("Accepting an AHEAD gate requires interactive or RPC UI support");
  const engine = await enginePromise;
  const store = storeFor(ctx);
  const run = await requireRun(ctx);
  let state = engine.deriveState(run);
  const missing = state.artifacts.filter((artifact) => artifact.required && !artifact.present);
  if (missing.length > 0) {
    throw new AheadEngineError(
      "required_artifact_missing",
      `complete first: ${missing.map((artifact) => artifact.title).join(", ")}`,
    );
  }

  const destination = state.phase.next
    ? engine.getWorkflow(run.workflow_id).phases.find((phase) => phase.id === state.phase.next)?.title ?? state.phase.next
    : "close this AHEAD run";
  const confirmed = await ctx.ui.confirm(
    `Accept and continue from ${state.phase.title}?`,
    [
      state.gate.title,
      "",
      `Next: ${destination}`,
      `Accountable human: ${humanActor(store.projectRoot).identity}`,
      "",
      "This records human acceptance. AI cannot perform this action.",
    ].join("\n"),
  );
  if (!confirmed) return;

  const actor = humanActor(store.projectRoot);
  let updated = run;
  if (!state.gate.accepted) {
    updated = engine.applyEvent(updated, actor, {
      type: "gate_accepted",
      phase: state.phase.id,
      gate: state.gate.id,
    });
    state = engine.deriveState(updated);
  }
  if (!state.can_advance) {
    throw new AheadEngineError("cannot_advance", state.blockers.join("; ") || "the phase cannot advance");
  }

  const action: EventAction = state.phase.next
    ? {
        type: "phase_transitioned",
        from: state.phase.id,
        to: state.phase.next,
        direction: "advance",
      }
    : { type: "run_closed", phase: state.phase.id };
  updated = engine.applyEvent(updated, actor, action);
  await store.save(updated);
  await refreshUi(ctx, updated);

  const nextState = engine.deriveState(updated);
  if (nextState.closed) {
    ctx.ui.notify("AHEAD work complete. The accountable human accepted the outcome and closed the run.", "info");
  } else if (nextState.phase.id === "human-review") {
    ctx.ui.notify(
      [
        "READY FOR INDEPENDENT HUMAN REVIEW",
        "The AI review is recorded and its material findings were disposed by a human.",
        "A draft branch may already exist, but a human must now request review or mark the PR ready.",
        "The independent reviewer opens this repository, runs /ahead, and records the review.",
      ].join("\n"),
      "info",
    );
  } else {
    ctx.ui.notify(
      `Continued to ${nextState.phase.title}. AHEAD mode remains active; run /ahead for the next guided action.`,
      "info",
    );
  }
}

async function returnToEarlierPhase(ctx: ExtensionCommandContext, requestedTarget: string): Promise<void> {
  if (!ctx.hasUI) throw new Error("Returning an AHEAD phase requires interactive or RPC UI support");
  const engine = await enginePromise;
  const store = storeFor(ctx);
  const run = await requireRun(ctx);
  const state = engine.deriveState(run);
  if (!state.return_targets.length) {
    throw new AheadEngineError("no_return_target", `phase ${state.phase.id} has no return transition`);
  }
  const workflow = engine.getWorkflow(run.workflow_id);
  const targetOptions = state.return_targets.map((target) => ({
    id: target,
    title: workflow.phases.find((phase) => phase.id === target)?.title ?? target,
  }));
  let target = requestedTarget.trim();
  if (!target) {
    const selected = await ctx.ui.select(
      "Return to which phase?",
      targetOptions.map((candidate) => candidate.title),
    );
    target = targetOptions.find((candidate) => candidate.title === selected)?.id ?? "";
  }
  if (!state.return_targets.includes(target)) {
    throw new AheadEngineError(
      "invalid_return",
      `phase ${state.phase.id} can return only to: ${state.return_targets.join(", ")}`,
    );
  }
  const reason = await ctx.ui.editor(
    `Why return to ${workflow.phases.find((phase) => phase.id === target)?.title ?? target}?`,
  );
  if (!reason?.trim()) return;
  const confirmed = await ctx.ui.confirm(
    `Return to ${target}?`,
    "This opens a new phase visit. Earlier artifacts remain as history but cannot satisfy the reopened gate.",
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
  ctx.ui.notify(`Returned to ${target}. AHEAD mode remains active with fresh evidence and gate requirements.`, "warning");
}

function storeFor(ctx: ExtensionContext): RunStore {
  return new RunStore(projectRoot(ctx.cwd));
}

async function requireRun(ctx: ExtensionContext): Promise<Run> {
  const run = await storeFor(ctx).loadCurrent();
  if (!run) throw new AheadEngineError("no_active_run", "no active AHEAD run; use /ahead [title]");
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
  const engine = await enginePromise;
  const state = engine.deriveState(run);
  const workflow = engine.getWorkflow(run.workflow_id);
  const position = phasePosition(state, workflow);
  const action = nextAction(state, workflow);
  ctx.ui.setStatus(
    "ahead",
    state.closed
      ? `AHEAD · complete · ${state.workflow_id}`
      : `AHEAD · ${position.current}/${position.total} · ${state.phase.id} · ${action.actor} action`,
  );
  ctx.ui.setWidget(
    "ahead",
    buildWidgetLines(run, state, workflow),
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
