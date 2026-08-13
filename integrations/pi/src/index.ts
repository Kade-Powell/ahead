import { execFile } from "node:child_process";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import type {
  ExtensionAPI,
  ExtensionCommandContext,
  ExtensionContext,
  Theme,
} from "@earendil-works/pi-coding-agent";
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import { Type } from "typebox";
import { projectConfigIssues, projectConfigMarkdown, runProjectConfigWizard } from "./config.js";
import { AheadEngine, AheadEngineError } from "./engine.js";
import {
  buildArtifactTemplate,
  buildHeaderLines,
  nextAction,
  phaseGuide,
  phasePosition,
} from "./guidance.js";
import {
  findReference,
  loadReferenceIndex,
  readReference,
  relevantReferences,
  type ReferenceEntry,
} from "./reference.js";
import { showReferenceViewer } from "./reference-viewer.js";
import {
  collectReviewSnapshot,
  extractFindingIds,
  extractReviewFingerprint,
  openInConfiguredEditor,
  reviewDispositionTemplate,
  reviewRequest,
  reviewSnapshotMarkdown,
  validateAiReviewArtifact,
  validateReviewDisposition,
} from "./review.js";
import {
  loadRecommendedSkills,
  recommendedSkillsMarkdown,
  relevantRecommendedSkills,
} from "./skills.js";
import { humanActor, projectRoot, RunStore } from "./storage.js";
import type {
  Actor,
  Capability,
  EventAction,
  Run,
  RunState,
  WorkflowDefinition,
  WorkItem,
} from "./types.js";

const wasmPath =
  process.env.AHEAD_WASM_PATH || fileURLToPath(new URL("../dist/ahead_wasm.wasm", import.meta.url));
const instructionDirectory = fileURLToPath(new URL("../generated", import.meta.url));
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
const ReferenceParams = Type.Object({
  topic: Type.Optional(
    Type.String({
      description: "Reference id, path, or title; omit to list phase-relevant references",
    }),
  ),
});

export default function aheadExtension(pi: ExtensionAPI): void {
  pi.registerCommand("ahead", {
    description: "Enter AHEAD mode or open its action menu",
    handler: async (args, ctx) =>
      command(ctx, async () => {
        await openAheadMode(pi, args, ctx);
      }),
  });

  pi.registerCommand("ahead-work-item", {
    description: "Link an existing work item or create a GitHub issue",
    handler: async (args, ctx) =>
      command(ctx, async () => {
        await manageWorkItem(ctx, args.trim());
      }),
  });

  pi.registerCommand("ahead-config", {
    description: "Configure or migrate this project's AHEAD policy",
    handler: async (_args, ctx) =>
      command(ctx, async () => {
        await manageProjectConfig(ctx);
      }),
  });

  pi.registerCommand("ahead-guide", {
    description: "Read the AHEAD framework guidance relevant to the active phase",
    handler: async (args, ctx) =>
      command(ctx, async () => {
        await showAheadGuide(ctx, args);
      }),
  });

  pi.registerCommand("ahead-skills", {
    description: "Inspect optional skills reviewed for the active AHEAD phase",
    handler: async (_args, ctx) =>
      command(ctx, async () => {
        await showRecommendedSkills(ctx);
      }),
  });

  pi.registerCommand("ahead-review", {
    description: "Inspect and perform the current changeset review handoff",
    handler: async (_args, ctx) =>
      command(ctx, async () => {
        await openReviewWorkbench(pi, ctx);
      }),
  });

  pi.registerCommand("ahead-stop", {
    description: "Exit AHEAD mode; discard the unfinished record or explicitly save it",
    handler: async (_args, ctx) =>
      command(ctx, async () => {
        await stopAheadMode(ctx);
      }),
  });

  pi.registerCommand("ahead-resume", {
    description: "Resume unfinished AHEAD work that was explicitly saved",
    handler: async (args, ctx) =>
      command(ctx, async () => {
        await resumeSavedRun(ctx, args.trim());
      }),
  });

  pi.registerCommand("ahead-start", {
    description: "Advanced: start with <workflow-id> :: <title-or-work-item-url>",
    handler: async (args, ctx) =>
      command(ctx, async () => {
        await startRun(ctx, args);
      }),
  });

  pi.registerCommand("ahead-status", {
    description: "Advanced: show the raw active AHEAD phase contract",
    handler: async (_args, ctx) =>
      command(ctx, async () => {
        const run = await requireRun(ctx);
        const state = (await enginePromise).deriveState(run);
        await refreshUi(ctx, run);
        ctx.ui.notify(formatState(state), state.blockers.length ? "warning" : "info");
      }),
  });

  pi.registerCommand("ahead-record", {
    description: "Advanced: record a human-owned artifact directly",
    handler: async (args, ctx) =>
      command(ctx, async () => {
        await recordHumanArtifact(ctx, args.trim());
      }),
  });

  pi.registerCommand("ahead-accept", {
    description: "Advanced: accept the active gate without advancing",
    handler: async (_args, ctx) =>
      command(ctx, async () => {
        if (!ctx.hasUI) {
          throw new Error("/ahead-accept requires interactive or RPC UI support");
        }
        const engine = await enginePromise;
        const store = storeFor(ctx);
        const run = await requireRun(ctx);
        const state = engine.deriveState(run);
        const confirmed = await ctx.ui.confirm(
          `Accept ${state.gate.id}?`,
          `${state.gate.title}\n\nThis records human acceptance as ${humanActor(store.projectRoot).identity}.`,
        );
        if (!confirmed) {
          return;
        }
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
    handler: async (_args, ctx) =>
      command(ctx, async () => {
        if (!ctx.hasUI) {
          throw new Error("/ahead-advance requires interactive or RPC UI support");
        }
        const engine = await enginePromise;
        const store = storeFor(ctx);
        const run = await requireRun(ctx);
        const state = engine.deriveState(run);
        const destination = state.phase.next ?? "closed";
        const confirmed = await ctx.ui.confirm(
          state.phase.next ? `Advance to ${state.phase.next}?` : "Close this AHEAD run?",
          `Current phase: ${state.phase.title}\nDestination: ${destination}\nActor: ${humanActor(store.projectRoot).identity}`,
        );
        if (!confirmed) {
          return;
        }
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
        ctx.ui.notify(
          state.phase.next ? `Advanced to ${state.phase.next}.` : "AHEAD run closed.",
          "info",
        );
      }),
  });

  pi.registerCommand("ahead-return", {
    description: "Advanced: return to an earlier phase with a reason",
    handler: async (args, ctx) =>
      command(ctx, async () => {
        await returnToEarlierPhase(ctx, args);
      }),
  });

  pi.registerCommand("ahead-help", {
    description: "Explain guided AHEAD mode and its authority boundary",
    handler: async (_args, ctx) => {
      ctx.ui.notify(
        [
          "/ahead [title] — choose a workflow for new work, or open the active action menu",
          "/ahead-work-item [url] — link an existing work item or create a GitHub issue",
          "/ahead-config — configure, replace, or migrate this project's AHEAD policy",
          "/ahead-guide [topic] — read the applicable AHEAD framework Markdown",
          "/ahead-skills — inspect optional reviewed skills relevant to this phase",
          "/ahead-review — inspect the exact changeset and review handoff",
          "/ahead-stop — leave AHEAD mode; discard the unfinished record by default or explicitly save it",
          "/ahead-resume [run-id] — resume an unfinished run that you explicitly saved",
          "",
          "Once started, the repository run remains in AHEAD mode until an accountable human closes it or uses /ahead-stop.",
          "Use normal conversation to think and work with AI. AHEAD remains active and guides every turn.",
          "The compact header shows the goal, required evidence, and next owner. /ahead reopens the action menu when you need a recorded action.",
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
    description:
      "Read the authoritative active AHEAD workflow state, phase contract, artifacts, gate, and blockers.",
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
    name: "ahead_get_work_item",
    label: "AHEAD work item",
    description:
      "Read the linked provider-neutral work-item reference and resolve GitHub issue context when available.",
    promptSnippet: "Read the human-linked work item when it is relevant to the active phase.",
    parameters: EmptyParams,
    async execute(_toolCallId, _params, _signal, _onUpdate, ctx) {
      return toolResult(async () => {
        const store = storeFor(ctx);
        const run = await requireRun(ctx);
        const state = (await enginePromise).deriveState(run);
        if (!state.work_item) {
          throw new AheadEngineError("work_item_missing", "this AHEAD run has no linked work item");
        }
        if (state.work_item.provider !== "github") {
          return {
            resolved: false,
            work_item: state.work_item,
            instruction:
              "No provider adapter is installed for this work item. Treat the URL as a human-selected coordination reference.",
          };
        }
        const issue: unknown = JSON.parse(
          await executeFile(
            "gh",
            [
              "issue",
              "view",
              state.work_item.url,
              "--json",
              "number,title,body,state,url,labels,assignees",
            ],
            store.projectRoot,
          ),
        );
        return { resolved: true, work_item: state.work_item, issue };
      });
    },
  });

  pi.registerTool({
    name: "ahead_get_recommended_skills",
    label: "AHEAD recommended skills",
    description:
      "List optional third-party skills reviewed for the active phase. Never installs a skill.",
    promptSnippet:
      "Discover reviewed optional skills only when they may help the active AHEAD work.",
    parameters: EmptyParams,
    async execute(_toolCallId, _params, _signal, _onUpdate, ctx) {
      return toolResult(async () => {
        const catalog = await loadRecommendedSkills();
        const run = await storeFor(ctx).loadCurrent();
        const state = run ? (await enginePromise).deriveState(run) : undefined;
        const phaseId = state && !state.closed ? state.phase.id : undefined;
        const workflowId = state && !state.closed ? state.workflow_id : undefined;
        return {
          reviewed_at: catalog.reviewed_at,
          phase: phaseId ?? null,
          workflow: workflowId ?? null,
          recommended: relevantRecommendedSkills(catalog, workflowId, phaseId),
          available: catalog.skills,
          instruction:
            "Do not install automatically. Explain why a skill applies, show the pinned source and command, and let the human opt in. AHEAD remains authoritative.",
        };
      });
    },
  });

  pi.registerTool({
    name: "ahead_get_review_snapshot",
    label: "AHEAD review snapshot",
    description:
      "Capture the exact current Git changeset, merge base, status, paths, diff, and stable fingerprint without modifying it.",
    promptSnippet: "Bind review findings to the exact current AHEAD changeset fingerprint.",
    parameters: EmptyParams,
    async execute(_toolCallId, _params, _signal, _onUpdate, ctx) {
      return toolResult(async () => collectReviewSnapshot(storeFor(ctx).projectRoot));
    },
  });

  pi.registerTool({
    name: "ahead_get_reference",
    label: "AHEAD framework reference",
    description:
      "List or read packaged AHEAD Constitution, philosophy, acceptable-use, engineering-practice, workflow, and evidence Markdown.",
    promptSnippet:
      "Retrieve relevant AHEAD framework guidance when the phase or policy is unclear.",
    parameters: ReferenceParams,
    async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
      return toolResult(async () => {
        const run = await storeFor(ctx).loadCurrent();
        const state = run ? (await enginePromise).deriveState(run) : undefined;
        const phase = state && !state.closed ? state.phase.id : undefined;
        const workflowId = state && !state.closed ? state.workflow_id : undefined;
        if (!params.topic?.trim()) {
          const index = await loadReferenceIndex();
          return {
            phase: phase ?? null,
            workflow: workflowId ?? null,
            recommended: await relevantReferences(workflowId, phase),
            available: index.references.map(({ id, title, path, audience, authority }) => ({
              id,
              title,
              path,
              audience,
              authority,
            })),
            instruction: "Request one reference by id, path, or title. Load only what is relevant.",
          };
        }
        const entry = await findReference(params.topic);
        if (!entry) {
          throw new Error(`No packaged AHEAD reference matches ${params.topic}`);
        }
        return { reference: entry, content: await readReference(entry) };
      });
    },
  });

  pi.registerTool({
    name: "ahead_record_artifact",
    label: "Record AHEAD artifact",
    description:
      "Persist an AI-owned or shared artifact permitted by the active phase. Cannot record human-owned artifacts.",
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
        if (artifact.kind === "ai-review") {
          const snapshot = await collectReviewSnapshot(store.projectRoot);
          const errors = validateAiReviewArtifact(params.content, snapshot.fingerprint);
          if (errors.length) {
            throw new AheadEngineError("invalid_ai_review", errors.join("; "));
          }
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
    description:
      "Report whether a human can advance the active AHEAD phase. This tool never accepts a gate or transitions state.",
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
    description:
      "Replay and validate the active AHEAD event log against the embedded workflow contract.",
    parameters: EmptyParams,
    async execute(_toolCallId, _params, _signal, _onUpdate, ctx) {
      return toolResult(async () => ({
        valid: true,
        state: (await enginePromise).validateRun(await requireRun(ctx)),
      }));
    },
  });

  pi.on("session_start", async (_event, ctx) => {
    try {
      await refreshUi(ctx);
      const run = await storeFor(ctx).loadCurrent();
      if (run && !(await enginePromise).deriveState(run).closed && ctx.hasUI) {
        const state = (await enginePromise).deriveState(run);
        ctx.ui.notify(
          `AHEAD mode resumed · ${state.phase.title}. Continue in normal conversation; /ahead is available when you need the action menu.`,
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
    if (!run) {
      return undefined;
    }
    const engine = await enginePromise;
    const state = engine.deriveState(run);
    if (state.closed) {
      return undefined;
    }
    const workflow = engine.getWorkflow(run.workflow_id);
    const guidance = phaseGuide(run.workflow_id, state.phase.id);
    const action = nextAction(state, workflow);
    const phaseInstructions = await loadInstructions(run.workflow_id, state.phase.id);
    const liveContext = [
      "# Live AHEAD run",
      `- Run: ${run.id} — ${run.title}`,
      `- Workflow: ${workflow.title} (${workflow.id})`,
      `- Phase: ${state.phase.id} visit ${state.phase.visit}`,
      `- Work item: ${state.work_item?.url ?? "none"}`,
      `- Work-item boundary: ${state.policy.work_items.required_before_phase ?? "not required"}`,
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
      "- Humans may ask questions at any phase. During implementation, help them understand or solve the problem without taking over; if their first attempt or current model is missing, ask for it.",
      "- When AHEAD policy or rationale is unclear, use ahead_get_reference to retrieve only the applicable packaged Markdown.",
      "- When a linked work item is relevant, use ahead_get_work_item for its available provider context. Treat it as coordination input, not as a substitute for human framing, evidence, or approval.",
      "- Use ahead_get_recommended_skills only when an optional reviewed skill could materially help. Never install one without the human's explicit choice; AHEAD remains authoritative.",
    ].join("\n");
    return { systemPrompt: `${event.systemPrompt}\n\n${phaseInstructions}\n\n${liveContext}\n` };
  });

  pi.on("tool_call", async (event, ctx) => {
    if (event.toolName.startsWith("ahead_")) {
      return undefined;
    }
    const run = await storeFor(ctx).loadCurrent();
    if (!run) {
      return undefined;
    }
    if ((await enginePromise).deriveState(run).closed) {
      return undefined;
    }
    const capability = toolCapabilities[event.toolName];
    if (!capability) {
      return {
        block: true,
        reason: `AHEAD blocked unclassified tool ${event.toolName}. The Pi adapter must map every model-invoked tool to an explicit workflow capability.`,
      };
    }
    try {
      const decision = (await enginePromise).toolAllowed(run, capability);
      if (!decision.allowed) {
        return { block: true, reason: `AHEAD: ${decision.reason}` };
      }
    } catch (error) {
      return { block: true, reason: `AHEAD state validation failed: ${errorMessage(error)}` };
    }
    return undefined;
  });
}

interface GuidedAction {
  label: string;
  run: () => Promise<void>;
}

async function openAheadMode(
  pi: ExtensionAPI,
  args: string,
  ctx: ExtensionCommandContext,
): Promise<void> {
  const engine = await enginePromise;
  const store = storeFor(ctx);
  let run = await store.loadCurrent();

  if (run && engine.deriveState(run).closed) {
    if (!ctx.hasUI) {
      ctx.ui.notify(
        "The current AHEAD run is complete. Start new work in an interactive Pi session.",
        "info",
      );
      return;
    }
    const choice = await ctx.ui.select("AHEAD work is complete", [
      "Start new AHEAD work",
      "View the completed run",
    ]);
    if (choice === "View the completed run") {
      ctx.ui.notify(formatState(engine.deriveState(run)), "info");
      return;
    }
    if (choice !== "Start new AHEAD work") {
      return;
    }
    run = undefined;
  }

  if (!run) {
    if (!args.trim() && ctx.hasUI) {
      const saved = await loadResumableRuns(ctx);
      if (saved.length > 0) {
        const choice = await ctx.ui.select("No active AHEAD run", [
          "Resume explicitly saved AHEAD work",
          "Start new AHEAD work",
        ]);
        if (choice === "Resume explicitly saved AHEAD work") {
          run = await resumeSavedRun(ctx, "", saved);
        } else if (choice !== "Start new AHEAD work") {
          return;
        }
      }
    }
  }

  if (!run) {
    run = await startRun(ctx, args);
    if (!run) {
      return;
    }
  }

  if (isHttpUrl(args.trim())) {
    run = await linkWorkItem(ctx, store, run, workItemFromUrl(args));
  }

  await refreshUi(ctx, run);
  if (!ctx.hasUI) {
    ctx.ui.notify(formatState(engine.deriveState(run)), "info");
    return;
  }

  const state = engine.deriveState(run);
  const workflow = engine.getWorkflow(run.workflow_id);
  const guidance = phaseGuide(run.workflow_id, state.phase.id);
  const action = nextAction(state, workflow);
  const actions: GuidedAction[] = [];
  const missingRequired = state.artifacts.filter(
    (artifact) => artifact.required && !artifact.present,
  );
  if (action.artifactKind) {
    const actionArtifact = state.artifacts.find(
      (artifact) => artifact.kind === action.artifactKind,
    );
    actions.push({
      label: action.label,
      run:
        action.actor === "ai"
          ? state.phase.id === "ai-review"
            ? async () => openReviewWorkbench(pi, ctx)
            : async () => requestAiAssistance(pi, state, action.artifactKind)
          : async () => recordHumanArtifact(ctx, action.artifactKind ?? ""),
    });
    if (action.actor === "ai" && actionArtifact?.actor === "any") {
      actions.push({
        label: `Write ${actionArtifact.title} yourself`,
        run: async () => recordHumanArtifact(ctx, actionArtifact.kind),
      });
    }
  }

  if (action.optional) {
    const nextHumanArtifact = missingRequired.find((artifact) => artifact.actor !== "ai");
    if (nextHumanArtifact) {
      actions.push({
        label: `Continue without optional AI challenge · Write ${nextHumanArtifact.title}`,
        run: async () => recordHumanArtifact(ctx, nextHumanArtifact.kind),
      });
    } else if (missingRequired.length === 0) {
      actions.push({
        label: `Continue without optional AI contribution · Accept ${state.gate.title}`,
        run: async () => acceptAndContinue(ctx),
      });
    }
  }

  if (state.work_item_required_for_next_phase && state.gate.accepted) {
    actions.push({
      label: action.label,
      run: async () => manageWorkItem(ctx, ""),
    });
  } else if (missingRequired.length === 0 && !action.artifactKind) {
    actions.push({
      label: state.gate.accepted ? action.label : `Accept and continue · ${state.gate.title}`,
      run: async () => acceptAndContinue(ctx),
    });
  }

  if (
    state.allowed_ai_capabilities.length > 0 &&
    action.actor !== "ai" &&
    !missingRequired.some((artifact) => artifact.actor === "ai") &&
    state.phase.id !== "implement"
  ) {
    actions.push({
      label: `Ask AI to assist · ${state.phase.title}`,
      run: async () => requestAiAssistance(pi, state),
    });
  }

  if (state.phase.id === "implement") {
    if (!state.artifacts.some((artifact) => artifact.kind === "changeset" && artifact.present)) {
      actions.push({
        label: "Save this ready-to-implement run for a later sprint",
        run: async () => saveImplementationHandoff(ctx),
      });
    }
    actions.push({
      label: "Ask AI for help understanding or solving a problem",
      run: async () => askImplementationQuestion(pi, ctx, state),
    });
  }

  if (state.phase.id === "ai-review" || state.phase.id === "human-review") {
    actions.push({
      label: "Open the changeset review workbench",
      run: async () => openReviewWorkbench(pi, ctx),
    });
  }

  if (state.return_targets.length > 0) {
    actions.push({
      label: "Return to an earlier phase",
      run: async () => returnToEarlierPhase(ctx, ""),
    });
  }

  if (!state.work_item_required_for_next_phase || !state.gate.accepted) {
    actions.push({
      label: state.work_item
        ? "View or replace the linked work item"
        : "Link or create a work item",
      run: async () => manageWorkItem(ctx, ""),
    });
  }

  actions.push({
    label: "Configure project AHEAD policy for future runs",
    run: async () => manageProjectConfig(ctx),
  });

  actions.push({
    label: "Read AHEAD framework guidance for this phase",
    run: async () => showAheadGuide(ctx, ""),
  });

  actions.push({
    label: "Inspect optional skills reviewed for this phase",
    run: async () => showRecommendedSkills(ctx),
  });

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

  actions.push({
    label: "Stop AHEAD mode",
    run: async () => stopAheadMode(ctx),
  });

  const selected = await ctx.ui.select(
    `AHEAD mode · ${state.phase.title}\nNext (${action.actor === "human" ? "you" : "AI"}): ${action.label}`,
    actions.map((candidate) => candidate.label),
  );
  const chosen = actions.find((candidate) => candidate.label === selected);
  if (chosen) {
    await chosen.run();
  }
}

async function stopAheadMode(ctx: ExtensionCommandContext): Promise<void> {
  if (!ctx.hasUI) {
    throw new Error("/ahead-stop requires interactive or RPC UI support");
  }
  const store = storeFor(ctx);
  const run = await requireRun(ctx);
  const state = (await enginePromise).deriveState(run);
  if (state.closed) {
    ctx.ui.notify("This AHEAD run is already complete.", "info");
    return;
  }

  const discard = "Stop and discard the unfinished AHEAD record";
  const save = "Stop and save the unfinished run for later";
  const selected = await ctx.ui.select("Stop AHEAD mode", [discard, save]);
  if (selected === discard) {
    const confirmed = await ctx.ui.confirm(
      "Discard this unfinished AHEAD record?",
      [
        `${run.title} · ${state.phase.title}`,
        "",
        "This removes only this run's .ahead workflow state and artifacts.",
        "It does not delete, reset, or revert source code or other repository changes.",
      ].join("\n"),
    );
    if (!confirmed) {
      return;
    }
    await store.discardCurrent(run.id);
    await refreshUi(ctx);
    ctx.ui.notify(
      "AHEAD mode stopped and its unfinished workflow record was discarded. Repository changes were left untouched.",
      "info",
    );
  } else if (selected === save) {
    await store.saveCurrentForResume(run.id);
    await refreshUi(ctx);
    ctx.ui.notify(
      `AHEAD mode stopped and run ${run.id} was saved. Resume it with /ahead-resume ${run.id}.`,
      "info",
    );
  }
}

async function saveImplementationHandoff(ctx: ExtensionCommandContext): Promise<void> {
  if (!ctx.hasUI) {
    throw new Error("Saving an implementation handoff requires interactive or RPC UI support");
  }
  const store = storeFor(ctx);
  const run = await requireRun(ctx);
  const state = (await enginePromise).deriveState(run);
  if (state.phase.id !== "implement") {
    throw new AheadEngineError(
      "implementation_handoff_unavailable",
      "a ready-to-implement handoff can be saved only after the approved plan enters implementation",
    );
  }
  const confirmed = await ctx.ui.confirm(
    "Save this implementation handoff?",
    [
      `${run.title} · ${run.id}`,
      `Work item: ${state.work_item?.url ?? "not linked"}`,
      "",
      "AHEAD mode will stop without discarding the approved plan or workflow state.",
      "The implementing engineer can resume this run in a later sprint.",
    ].join("\n"),
  );
  if (!confirmed) {
    return;
  }
  await store.saveCurrentForResume(run.id);
  await refreshUi(ctx);
  ctx.ui.notify(
    `Saved ready-to-implement AHEAD run ${run.id}. Resume it with /ahead-resume ${run.id}.`,
    "info",
  );
}

async function resumeSavedRun(
  ctx: ExtensionCommandContext,
  requestedRunId: string,
  supplied?: Run[],
): Promise<Run | undefined> {
  const store = storeFor(ctx);
  const current = await store.loadCurrent();
  if (current && !(await enginePromise).deriveState(current).closed) {
    throw new AheadEngineError(
      "active_run_exists",
      `run ${current.id} is still active; stop it before resuming another run`,
    );
  }

  const saved = supplied ?? (await loadResumableRuns(ctx));
  if (saved.length === 0) {
    ctx.ui.notify("No saved unfinished AHEAD runs are available.", "info");
    return undefined;
  }

  let selected = requestedRunId
    ? saved.find((candidate) => candidate.id === requestedRunId)
    : undefined;
  if (requestedRunId && !selected) {
    throw new AheadEngineError(
      "saved_run_not_found",
      `saved unfinished run ${requestedRunId} was not found`,
    );
  }
  if (!selected) {
    if (!ctx.hasUI) {
      throw new Error("/ahead-resume requires a run id without interactive UI");
    }
    const options = saved.map(savedRunOption);
    const choice = await ctx.ui.select("Resume saved AHEAD work", options);
    selected = saved.find((candidate) => savedRunOption(candidate) === choice);
  }
  if (!selected) {
    return undefined;
  }

  const resumed = await store.resume(selected.id);
  const state = (await enginePromise).deriveState(resumed);
  await refreshUi(ctx, resumed);
  ctx.ui.notify(
    `AHEAD mode resumed · ${state.phase.title}. Existing artifacts and unmet gates were preserved.`,
    "info",
  );
  return resumed;
}

async function loadResumableRuns(ctx: ExtensionContext): Promise<Run[]> {
  const store = storeFor(ctx);
  const engine = await enginePromise;
  const resumable: Run[] = [];
  const invalid: string[] = [];
  for (const runId of await store.listRunIds()) {
    try {
      const run = await store.load(runId);
      if (!engine.deriveState(run).closed) {
        resumable.push(run);
      }
    } catch {
      invalid.push(runId);
    }
  }
  if (invalid.length > 0 && ctx.hasUI) {
    ctx.ui.notify(`Ignored invalid saved AHEAD runs: ${invalid.join(", ")}`, "warning");
  }
  return resumable;
}

function savedRunOption(run: Run): string {
  return `${run.title} · ${run.workflow_id} · ${run.id}`;
}

async function manageProjectConfig(ctx: ExtensionCommandContext): Promise<void> {
  if (!ctx.hasUI) {
    throw new Error("/ahead-config requires interactive or RPC UI support");
  }
  const store = storeFor(ctx);
  const workflows = (await enginePromise).listWorkflows();
  const inspection = await store.inspectProjectConfig();
  if (inspection.status === "missing") {
    await runProjectConfigWizard(ctx, store, workflows, false);
    return;
  }

  const issues =
    inspection.status === "valid" ? projectConfigIssues(inspection.config, workflows) : [];
  const problem =
    inspection.status === "invalid"
      ? inspection.error
      : issues.length > 0
        ? `invalid .ahead/config.json: ${issues.join("; ")}`
        : undefined;
  if (problem) {
    const view = "View the current file and validation error";
    const replace = "Run setup wizard and preserve the current file as a backup";
    const selected = await ctx.ui.select(`AHEAD configuration needs attention\n${problem}`, [
      view,
      replace,
    ]);
    if (selected === view) {
      await showReferenceViewer(
        ctx,
        "AHEAD configuration · invalid",
        [
          "# AHEAD project configuration needs attention",
          "",
          problem,
          "",
          "The setup wizard can replace this file while preserving its exact contents under `.ahead/backups/`.",
          "",
          "~~~json",
          inspection.content,
          "~~~",
        ].join("\n"),
      );
    } else if (selected === replace) {
      await runProjectConfigWizard(ctx, store, workflows, true);
    }
    return;
  }
  if (inspection.status !== "valid") {
    throw new Error("AHEAD project configuration inspection returned an inconsistent result");
  }

  const view = "View current configuration";
  const replace = "Run setup wizard and replace configuration";
  const selected = await ctx.ui.select("AHEAD project configuration", [view, replace]);
  if (selected === view) {
    await showReferenceViewer(
      ctx,
      "AHEAD project configuration",
      projectConfigMarkdown(inspection.config),
    );
  } else if (selected === replace) {
    await runProjectConfigWizard(ctx, store, workflows, true);
  }
}

async function ensureProjectConfiguration(
  ctx: ExtensionCommandContext,
  store: RunStore,
  workflows: WorkflowDefinition[],
): Promise<boolean> {
  const inspection = await store.inspectProjectConfig();
  if (inspection.status === "valid") {
    const issues = projectConfigIssues(inspection.config, workflows);
    if (issues.length === 0) {
      return true;
    }
    const problem = `invalid .ahead/config.json: ${issues.join("; ")}`;
    if (!ctx.hasUI) {
      throw new Error(`${problem}; run /ahead-config in an interactive session`);
    }
    ctx.ui.notify(problem, "warning");
    return repairProjectConfiguration(ctx, store, workflows);
  }
  if (inspection.status === "missing") {
    if (!ctx.hasUI) {
      return true;
    }
    const configure = "Run AHEAD project setup wizard";
    const without = "Continue without project configuration";
    const selected = await ctx.ui.select("No .ahead/config.json was found", [configure, without]);
    if (selected === configure) {
      return runProjectConfigWizard(ctx, store, workflows, false);
    }
    return selected === without;
  }

  if (!ctx.hasUI) {
    throw new Error(`${inspection.error}; run /ahead-config in an interactive session`);
  }
  ctx.ui.notify(inspection.error, "warning");
  return repairProjectConfiguration(ctx, store, workflows);
}

async function repairProjectConfiguration(
  ctx: ExtensionCommandContext,
  store: RunStore,
  workflows: WorkflowDefinition[],
): Promise<boolean> {
  const repair = "Run setup wizard and preserve the current file as a backup";
  const cancel = "Cancel this new AHEAD run";
  const selected = await ctx.ui.select("AHEAD project configuration must be repaired", [
    repair,
    cancel,
  ]);
  if (selected !== repair) {
    return false;
  }
  return runProjectConfigWizard(ctx, store, workflows, true);
}

async function manageWorkItem(ctx: ExtensionCommandContext, requestedUrl: string): Promise<void> {
  const engine = await enginePromise;
  const store = storeFor(ctx);
  let run = await requireRun(ctx);
  let state = engine.deriveState(run);

  if (requestedUrl) {
    run = await linkWorkItem(ctx, store, run, workItemFromUrl(requestedUrl));
    await refreshUi(ctx, run);
    state = engine.deriveState(run);
    ctx.ui.notify(`Linked work item to AHEAD run ${run.id}: ${state.work_item?.url}`, "info");
    return;
  }
  if (!ctx.hasUI) {
    throw new Error("/ahead-work-item requires a URL without interactive UI");
  }

  const linkExisting = state.work_item
    ? "Replace the linked work item"
    : "Link an existing work item";
  const createGitHub = "Create a GitHub issue in this repository";
  const show = "Show the linked work item";
  const choices = state.work_item
    ? [show, linkExisting, createGitHub]
    : [linkExisting, createGitHub];
  const selected = await ctx.ui.select("AHEAD work item", choices);
  if (selected === show && state.work_item) {
    ctx.ui.notify(`${state.work_item.title ?? "Linked work item"}\n${state.work_item.url}`, "info");
    return;
  }
  if (selected === linkExisting) {
    const url = await ctx.ui.input("Link an existing work item", "https://…");
    if (!url?.trim()) {
      return;
    }
    run = await linkWorkItem(ctx, store, run, workItemFromUrl(url));
  } else if (selected === createGitHub) {
    const body = await ctx.ui.editor(
      `Create GitHub issue · ${run.title}`,
      await workItemBodyTemplate(store, run, state),
    );
    if (!body?.trim()) {
      return;
    }
    const confirmed = await ctx.ui.confirm(
      "Create this GitHub issue?",
      [
        `Repository: ${store.projectRoot}`,
        `Title: ${run.title}`,
        "",
        "This writes to GitHub using the current gh authentication.",
      ].join("\n"),
    );
    if (!confirmed) {
      return;
    }
    const url = (
      await executeFile(
        "gh",
        ["issue", "create", "--title", run.title, "--body", body],
        store.projectRoot,
      )
    ).trim();
    run = await linkWorkItem(ctx, store, run, workItemFromUrl(url, run.title));
  } else {
    return;
  }

  state = engine.deriveState(run);
  await refreshUi(ctx, run);
  ctx.ui.notify(`Linked work item to AHEAD run ${run.id}: ${state.work_item?.url}`, "info");
}

async function linkWorkItem(
  ctx: ExtensionCommandContext,
  store: RunStore,
  run: Run,
  workItem: WorkItem,
): Promise<Run> {
  const engine = await enginePromise;
  const current = engine.deriveState(run).work_item;
  if (current && current.url !== workItem.url && ctx.hasUI) {
    const replace = await ctx.ui.confirm(
      "Replace the linked work item?",
      `${current.url}\n\nwill be replaced by\n\n${workItem.url}`,
    );
    if (!replace) {
      return run;
    }
  }
  if (current?.url === workItem.url) {
    return run;
  }
  const updated = engine.applyEvent(run, humanActor(store.projectRoot), {
    type: "work_item_linked",
    work_item: workItem,
  });
  await store.save(updated);
  return updated;
}

function workItemFromUrl(value: string, title?: string): WorkItem {
  let url: URL;
  try {
    url = new URL(value.trim());
  } catch {
    throw new AheadEngineError("invalid_work_item_url", "work item must be an absolute URL");
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") {
    throw new AheadEngineError("invalid_work_item_url", "work item URL must use HTTP or HTTPS");
  }
  if (url.username || url.password) {
    throw new AheadEngineError(
      "invalid_work_item_url",
      "work item URL cannot contain embedded credentials",
    );
  }
  const host = url.hostname.toLowerCase();
  const githubIssue =
    host === "github.com" ? /^\/[^/]+\/[^/]+\/issues\/(\d+)\/?$/.exec(url.pathname) : null;
  const provider = workItemProvider(host);
  return {
    provider,
    url: url.toString(),
    ...(githubIssue?.[1] ? { external_id: githubIssue[1] } : {}),
    ...(title?.trim() ? { title: title.trim() } : {}),
  };
}

function workItemProvider(host: string): string {
  if (host === "github.com") {
    return "github";
  }
  if (host === "linear.app") {
    return "linear";
  }
  if (host === "dev.azure.com") {
    return "azure-devops";
  }
  if (host.endsWith(".atlassian.net")) {
    return "jira";
  }
  return host;
}

async function workItemBodyTemplate(store: RunStore, run: Run, state: RunState): Promise<string> {
  const planPath = run.events
    .toReversed()
    .find((event) => event.type === "artifact_recorded" && event.kind === "plan")?.path;
  const plan = planPath
    ? (await store.readArtifact(planPath)).trim()
    : "<!-- Link or summarize the approved AHEAD plan when it is ready. -->";
  return [
    "## Outcome",
    "",
    "<!-- Describe the human-owned outcome this work item should coordinate. -->",
    "",
    "## Scope and acceptance",
    "",
    "<!-- Record the accepted scope, constraints, and observable success signals. -->",
    "",
    "## Plan",
    "",
    plan,
    "",
    "---",
    "",
    `AHEAD run: \`${run.id}\``,
    `Workflow: \`${run.workflow_id}\``,
    `Current phase: \`${state.phase.id}\``,
    "",
  ].join("\n");
}

function executeFile(file: string, args: string[], cwd: string): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(file, args, { cwd, encoding: "utf8" }, (error, stdout, stderr) => {
      if (error) {
        reject(new Error(stderr.trim() || error.message));
        return;
      }
      resolve(stdout);
    });
  });
}

async function showRecommendedSkills(ctx: ExtensionCommandContext): Promise<void> {
  if (!ctx.hasUI) {
    throw new Error("Inspecting recommended skills requires interactive or RPC UI support");
  }
  const catalog = await loadRecommendedSkills();
  const run = await storeFor(ctx).loadCurrent();
  const state = run ? (await enginePromise).deriveState(run) : undefined;
  const phaseId = state && !state.closed ? state.phase.id : undefined;
  const workflowId = state && !state.closed ? state.workflow_id : undefined;
  const relevant = relevantRecommendedSkills(catalog, workflowId, phaseId);
  await showReferenceViewer(
    ctx,
    relevant.length ? `AHEAD skills · ${state?.phase.title}` : "AHEAD recommended skills",
    recommendedSkillsMarkdown(catalog, relevant.length ? relevant : catalog.skills),
  );
}

async function openReviewWorkbench(pi: ExtensionAPI, ctx: ExtensionCommandContext): Promise<void> {
  if (!ctx.hasUI) {
    throw new Error("The review workbench requires interactive or RPC UI support");
  }
  const store = storeFor(ctx);
  const run = await requireRun(ctx);
  const state = (await enginePromise).deriveState(run);
  const snapshot = await collectReviewSnapshot(store.projectRoot);
  const aiReview = state.artifacts.find((artifact) => artifact.kind === "ai-review");
  const disposition = state.artifacts.find((artifact) => artifact.kind === "review-disposition");
  const options = ["View snapshot and changed files", "View exact terminal diff"];
  if (snapshot.changed_files.length) {
    options.push("Open a changed file in the configured editor");
  }
  if (state.phase.id === "ai-review" && !aiReview?.present) {
    options.push("Request AI review of this exact snapshot");
  }
  if (state.phase.id === "ai-review" && aiReview?.present && !disposition?.present) {
    options.push("Disposition the AI findings as the implementing human");
  }
  if (state.phase.id === "human-review") {
    options.push("Record the independent human review of this snapshot");
  }
  const selected = await ctx.ui.select(
    `AHEAD review · ${state.phase.title} · ${snapshot.changed_files.length} changed files`,
    options,
  );
  if (selected === "View snapshot and changed files") {
    await showReferenceViewer(ctx, "AHEAD exact review snapshot", reviewSnapshotMarkdown(snapshot));
  } else if (selected === "View exact terminal diff") {
    await showReferenceViewer(
      ctx,
      `AHEAD diff · ${snapshot.fingerprint.slice(0, 12)}`,
      `${reviewSnapshotMarkdown(snapshot)}\n\n## Diff\n\n\`\`\`diff\n${snapshot.diff || "No tracked diff."}\n\`\`\``,
    );
  } else if (selected === "Open a changed file in the configured editor") {
    const path = await ctx.ui.select("Open changed file", snapshot.changed_files);
    if (path && !openInConfiguredEditor(store.projectRoot, { path })) {
      ctx.ui.notify(
        `No supported editor was detected. Open ${path} from the repository, or set AHEAD_EDITOR=vscode.`,
        "info",
      );
    }
  } else if (selected === "Request AI review of this exact snapshot") {
    pi.sendUserMessage(reviewRequest(snapshot));
  } else if (selected === "Disposition the AI findings as the implementing human") {
    await recordHumanArtifact(ctx, "review-disposition");
  } else if (selected === "Record the independent human review of this snapshot") {
    await recordHumanArtifact(ctx, "human-review");
  }
}

async function askImplementationQuestion(
  pi: ExtensionAPI,
  ctx: ExtensionCommandContext,
  state: RunState,
): Promise<void> {
  if (!ctx.hasUI) {
    throw new Error("Implementation coaching requires interactive or RPC UI support");
  }
  const question = await ctx.ui.editor(
    "AHEAD implementation help · human first",
    [
      "## What are you trying to understand or solve?",
      "",
      "",
      "## What do you currently think is happening or should happen?",
      "",
      "",
      "## What have you tried or inspected so far?",
      "",
      "",
      "## What kind of help would be useful?",
      "",
      "<!-- Ask for explanation, a hint, competing approaches, debugging help, or a bounded suggestion. -->",
      "",
    ].join("\n"),
  );
  if (!question?.trim()) {
    return;
  }
  pi.sendUserMessage(
    [
      `AHEAD mode: help me with this ${state.phase.title} question while I remain the implementer.`,
      "Use my current model and first attempt below. Help me understand or solve the problem with questions, explanation, evidence, hints, and bounded next steps.",
      "Do not convert this question into autonomous implementation or author my human-owned records. If I later request a bounded mechanical edit, explain it so I can inspect and own it.",
      "",
      question.trim(),
    ].join("\n"),
  );
}

async function showAheadGuide(ctx: ExtensionCommandContext, requestedTopic: string): Promise<void> {
  if (!ctx.hasUI) {
    throw new Error("Reading AHEAD framework guidance requires interactive or RPC UI support");
  }
  const run = await storeFor(ctx).loadCurrent();
  const state = run ? (await enginePromise).deriveState(run) : undefined;
  const phase = state && !state.closed ? state.phase.id : undefined;
  const workflowId = state && !state.closed ? state.workflow_id : undefined;
  const index = await loadReferenceIndex();
  let entry =
    requestedTopic.trim() && requestedTopic.trim().toLowerCase() !== "all"
      ? await findReference(requestedTopic)
      : undefined;

  if (requestedTopic.trim() && requestedTopic.trim().toLowerCase() !== "all" && !entry) {
    throw new Error(`No packaged AHEAD reference matches ${requestedTopic.trim()}`);
  }

  if (!entry) {
    const recommended =
      requestedTopic.trim().toLowerCase() === "all"
        ? index.references
        : await relevantReferences(workflowId, phase);
    const browseAll = "Browse all practitioner and evidence Markdown";
    const selected = await ctx.ui.select(
      phase ? `AHEAD guidance · ${phase}` : "AHEAD framework guidance",
      [
        ...recommended.map(referenceOption),
        ...(recommended.length < index.references.length ? [browseAll] : []),
      ],
    );
    if (!selected) {
      return;
    }
    if (selected === browseAll) {
      return showAheadGuide(ctx, "all");
    }
    entry = recommended.find((candidate) => referenceOption(candidate) === selected);
  }
  if (!entry) {
    return;
  }

  await showReferenceViewer(ctx, `AHEAD reference · ${entry.title}`, await readReference(entry));
}

function referenceOption(entry: ReferenceEntry): string {
  const classification = entry.audience === "evidence" ? "evidence" : entry.authority;
  return `${classification} · ${entry.title}`;
}

async function startRun(ctx: ExtensionCommandContext, request: string): Promise<Run | undefined> {
  const engine = await enginePromise;
  const store = storeFor(ctx);
  const current = await store.loadCurrent();
  if (current && !engine.deriveState(current).closed) {
    throw new AheadEngineError(
      "active_run_exists",
      `run ${current.id} is still active; use /ahead to continue it`,
    );
  }

  const workflows = engine.listWorkflows();
  if (!(await ensureProjectConfiguration(ctx, store, workflows))) {
    return undefined;
  }
  const parsed = parseStartRequest(
    request,
    workflows.map((workflow) => workflow.id),
  );
  let workflow = parsed.workflowId
    ? workflows.find((candidate) => candidate.id === parsed.workflowId)
    : undefined;
  if (parsed.workflowId && !workflow) {
    throw new AheadEngineError(
      "unknown_workflow",
      `unknown workflow ${parsed.workflowId}; choose one of: ${workflows.map((candidate) => candidate.id).join(", ")}`,
    );
  }
  if (!workflow && ctx.hasUI) {
    const selected = await ctx.ui.select(
      "Choose the AHEAD workflow that fits this work",
      workflows.map((candidate) => candidate.title),
    );
    workflow = workflows.find((candidate) => candidate.title === selected);
    if (!workflow) {
      return undefined;
    }
  }
  if (!workflow) {
    throw new AheadEngineError(
      "workflow_required",
      `choose a workflow explicitly: ${workflows.map((candidate) => candidate.id).join(", ")}. Noninteractive usage: /ahead-start <workflow-id> :: <title-or-work-item-url>`,
    );
  }

  const linkedTitle = parsed.workItemUrl
    ? await resolveWorkItemTitle(parsed.workItemUrl, store.projectRoot)
    : undefined;
  const title =
    parsed.title ||
    linkedTitle ||
    (ctx.hasUI
      ? await ctx.ui.input(`Enter AHEAD mode · ${workflow.title}`, "What work are you doing?")
      : parsed.workItemUrl);
  if (!title?.trim()) {
    return undefined;
  }

  const owner = humanActor(store.projectRoot);
  let run = engine.createRun({
    id: store.newRunId(),
    title: title.trim(),
    owner,
    timestamp: new Date().toISOString(),
    workflow_id: workflow.id,
    policy: await store.policyForWorkflow(workflow.id),
  });
  if (parsed.workItemUrl) {
    run = engine.applyEvent(run, owner, {
      type: "work_item_linked",
      work_item: workItemFromUrl(parsed.workItemUrl, title.trim()),
    });
  }
  await store.save(run);
  await refreshUi(ctx, run);
  const state = engine.deriveState(run);
  ctx.ui.notify(
    [
      `AHEAD mode started · ${workflow.title} · ${run.title}`,
      "Human leads · AI assists",
      ...(state.work_item ? [`Work item: ${state.work_item.url}`] : []),
      ...(state.policy.work_items.required_before_phase
        ? [
            `Project policy requires a work item before ${state.policy.work_items.required_before_phase}.`,
          ]
        : []),
      "This run remains active until an accountable human closes the outcome or uses /ahead-stop.",
      "Continue in normal conversation. /ahead is available when you need the action menu.",
    ].join("\n"),
    "info",
  );
  return run;
}

async function recordHumanArtifact(
  ctx: ExtensionCommandContext,
  requestedKind: string,
): Promise<void> {
  if (!ctx.hasUI) {
    throw new Error("Recording a human artifact requires interactive or RPC UI support");
  }
  const engine = await enginePromise;
  const store = storeFor(ctx);
  const run = await requireRun(ctx);
  const state = engine.deriveState(run);
  const allowed = state.artifacts.filter(
    (artifact) => artifact.actor !== "ai" && !artifact.present,
  );
  let kind = requestedKind.trim();
  if (!kind) {
    kind =
      (await ctx.ui.select(
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

  const template = await humanArtifactTemplate(store, state, run, artifact.kind, artifact.title);
  const content = await ctx.ui.editor(
    `AHEAD mode · ${artifact.title} · write in your own words`,
    template,
  );
  if (!content?.trim()) {
    return;
  }
  await validateHumanReviewArtifact(store, state, artifact.kind, content);
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
    `Saved ${artifact.title}. AHEAD mode remains active; continue the conversation.`,
    "info",
  );
}

async function humanArtifactTemplate(
  store: RunStore,
  state: RunState,
  run: Run,
  kind: string,
  title: string,
): Promise<string> {
  if (kind === "review-disposition") {
    const reviewArtifact = state.artifacts.find((artifact) => artifact.kind === "ai-review");
    if (!reviewArtifact?.path) {
      throw new AheadEngineError(
        "ai_review_missing",
        "the AI review must be recorded before human disposition",
      );
    }
    const aiReview = await store.readArtifact(reviewArtifact.path);
    const snapshot = await collectReviewSnapshot(store.projectRoot);
    return reviewDispositionTemplate(snapshot, extractFindingIds(aiReview));
  }
  if (kind === "human-review") {
    const snapshot = await collectReviewSnapshot(store.projectRoot);
    return `${buildArtifactTemplate(run, state, kind, title)}\n\nAHEAD-Review-Snapshot: ${snapshot.fingerprint}\n`;
  }
  return buildArtifactTemplate(run, state, kind, title);
}

async function validateHumanReviewArtifact(
  store: RunStore,
  state: RunState,
  kind: string,
  content: string,
): Promise<void> {
  if (kind !== "review-disposition" && kind !== "human-review") {
    return;
  }
  const snapshot = await collectReviewSnapshot(store.projectRoot);
  if (kind === "human-review") {
    if (extractReviewFingerprint(content) !== snapshot.fingerprint) {
      throw new AheadEngineError(
        "review_snapshot_stale",
        "the human review must identify the exact current AHEAD review snapshot",
      );
    }
    return;
  }
  const reviewArtifact = state.artifacts.find((artifact) => artifact.kind === "ai-review");
  if (!reviewArtifact?.path) {
    throw new AheadEngineError("ai_review_missing", "the AI review artifact is missing");
  }
  const aiReview = await store.readArtifact(reviewArtifact.path);
  const aiFingerprint = extractReviewFingerprint(aiReview);
  if (!aiFingerprint || aiFingerprint !== snapshot.fingerprint) {
    throw new AheadEngineError(
      "review_snapshot_stale",
      "the changeset changed after AI review; return to implementation and review the new snapshot",
    );
  }
  const errors = validateReviewDisposition(
    content,
    snapshot.fingerprint,
    extractFindingIds(aiReview),
  );
  if (errors.length) {
    throw new AheadEngineError("review_disposition_incomplete", errors.join("; "));
  }
}

function requestAiAssistance(pi: ExtensionAPI, state: RunState, requiredKind?: string): void {
  const guidance = phaseGuide(state.workflow_id, state.phase.id);
  const artifact = requiredKind
    ? state.artifacts.find((candidate) => candidate.kind === requiredKind)
    : undefined;
  const request = artifact
    ? [
        `AHEAD mode: perform the ${artifact.required ? "required" : "recommended"} ${state.phase.title} work for the exact current run and evidence.`,
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
  if (!ctx.hasUI) {
    throw new Error("Accepting an AHEAD gate requires interactive or RPC UI support");
  }
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
    ? (engine.getWorkflow(run.workflow_id).phases.find((phase) => phase.id === state.phase.next)
        ?.title ?? state.phase.next)
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
  if (!confirmed) {
    return;
  }

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
    if (state.gate.accepted && state.work_item_required_for_next_phase) {
      await store.save(updated);
      await refreshUi(ctx, updated);
      ctx.ui.notify(
        `Accepted ${state.gate.title}. Link or create the required work item before entering ${state.phase.next}.`,
        "info",
      );
      return;
    }
    throw new AheadEngineError(
      "cannot_advance",
      state.blockers.join("; ") || "the phase cannot advance",
    );
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
    ctx.ui.notify(
      "AHEAD work complete. The accountable human accepted the outcome and closed the run.",
      "info",
    );
  } else if (nextState.phase.id === "implement") {
    ctx.ui.notify(
      [
        "READY FOR IMPLEMENTATION",
        "The human-approved plan and any configured work-item boundary are satisfied.",
        "Continue now, or use /ahead and save this ready-to-implement run for a later sprint.",
        `Work item: ${nextState.work_item?.url ?? "not linked"}`,
      ].join("\n"),
      "info",
    );
  } else if (nextState.phase.id === "human-review") {
    ctx.ui.notify(
      [
        "READY FOR INDEPENDENT HUMAN REVIEW",
        "The AI review is recorded and its material findings were disposed by a human.",
        "A draft branch may already exist, but a human must now request review or mark the PR ready.",
        "The independent reviewer opens this repository; AHEAD resumes at Human Review and guides the review record.",
      ].join("\n"),
      "info",
    );
  } else {
    ctx.ui.notify(
      `Continued to ${nextState.phase.title}. AHEAD mode remains active; continue the conversation.`,
      "info",
    );
  }
}

async function returnToEarlierPhase(
  ctx: ExtensionCommandContext,
  requestedTarget: string,
): Promise<void> {
  if (!ctx.hasUI) {
    throw new Error("Returning an AHEAD phase requires interactive or RPC UI support");
  }
  const engine = await enginePromise;
  const store = storeFor(ctx);
  const run = await requireRun(ctx);
  const state = engine.deriveState(run);
  if (!state.return_targets.length) {
    throw new AheadEngineError(
      "no_return_target",
      `phase ${state.phase.id} has no return transition`,
    );
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
  if (!reason?.trim()) {
    return;
  }
  const confirmed = await ctx.ui.confirm(
    `Return to ${target}?`,
    "This opens a new phase visit. Earlier artifacts remain as history but cannot satisfy the reopened gate.",
  );
  if (!confirmed) {
    return;
  }
  const updated = engine.applyEvent(run, humanActor(store.projectRoot), {
    type: "phase_transitioned",
    from: state.phase.id,
    to: target,
    direction: "return",
    reason: reason.trim(),
  });
  await store.save(updated);
  await refreshUi(ctx, updated);
  ctx.ui.notify(
    `Returned to ${target}. AHEAD mode remains active with fresh evidence and gate requirements.`,
    "warning",
  );
}

function storeFor(ctx: ExtensionContext): RunStore {
  return new RunStore(projectRoot(ctx.cwd));
}

async function requireRun(ctx: ExtensionContext): Promise<Run> {
  const run = await storeFor(ctx).loadCurrent();
  if (!run) {
    throw new AheadEngineError("no_active_run", "no active AHEAD run; use /ahead [title]");
  }
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
    if (ctx.mode === "tui") {
      ctx.ui.setHeader(undefined);
    }
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
  if (state.closed) {
    ctx.ui.setWidget("ahead", undefined);
    if (ctx.mode === "tui") {
      ctx.ui.setHeader(undefined);
    }
    return;
  }

  const lines = buildHeaderLines(run, state, workflow);
  if (ctx.mode === "tui") {
    ctx.ui.setWidget("ahead", undefined);
    ctx.ui.setHeader((_tui, theme) => aheadHeader(lines, theme));
  } else {
    ctx.ui.setWidget("ahead", lines, { placement: "aboveEditor" });
  }
}

function aheadHeader(lines: string[], theme: Theme) {
  return {
    render(width: number): string[] {
      const [heading = "AHEAD", ...fields] = lines;
      const styledHeading = `${theme.fg("accent", theme.bold("AHEAD"))}${theme.fg("muted", heading.slice("AHEAD".length))}`;
      const ruleWidth = Math.max(0, width - visibleWidth(styledHeading) - 1);
      const headingLine = `${styledHeading}${
        ruleWidth > 0 ? ` ${theme.fg("borderMuted", "─".repeat(ruleWidth))}` : ""
      }`;

      return [
        truncateToWidth(headingLine, width),
        ...fields.map((field) => formatAheadHeaderField(field, width, theme)),
      ];
    },
    invalidate() {},
  };
}

function formatAheadHeaderField(field: string, width: number, theme: Theme): string {
  const separator = field.indexOf(":");
  if (separator < 0) {
    return truncateToWidth(theme.fg("text", field), width);
  }

  const label = field.slice(0, separator).toUpperCase().padEnd(9);
  let value = field.slice(separator + 1).trimStart();
  let styledValue = theme.fg("text", value);
  if (label.trim() === "NEXT") {
    const actor = value.startsWith("You →") ? "You →" : value.startsWith("AI →") ? "AI →" : "";
    if (actor) {
      value = value.slice(actor.length);
      styledValue = `${theme.fg(actor.startsWith("You") ? "success" : "accent", theme.bold(actor))}${theme.fg("text", value)}`;
    }
  }

  return truncateToWidth(`${theme.fg("muted", theme.bold(label))} ${styledValue}`, width);
}

async function loadInstructions(workflowId: string, phase: string): Promise<string> {
  const cacheKey = `${workflowId}/${phase}`;
  const cached = instructions.get(cacheKey);
  if (cached) {
    return cached;
  }
  const content = await readFile(`${instructionDirectory}/${cacheKey}.md`, "utf8");
  instructions.set(cacheKey, content);
  return content;
}

function parseStartRequest(
  request: string,
  workflowIds: string[],
): { workflowId?: string; title: string; workItemUrl?: string } {
  const trimmed = request.trim();
  const separator = trimmed.indexOf("::");
  if (separator >= 0) {
    const value = trimmed.slice(separator + 2).trim();
    return {
      workflowId: trimmed.slice(0, separator).trim(),
      title: isHttpUrl(value) ? "" : value,
      ...(isHttpUrl(value) ? { workItemUrl: value } : {}),
    };
  }
  if (workflowIds.includes(trimmed)) {
    return { workflowId: trimmed, title: "" };
  }
  if (isHttpUrl(trimmed)) {
    return { title: "", workItemUrl: trimmed };
  }
  return { title: trimmed };
}

function isHttpUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return url.protocol === "https:" || url.protocol === "http:";
  } catch {
    return false;
  }
}

async function resolveWorkItemTitle(value: string, root: string): Promise<string | undefined> {
  const workItem = workItemFromUrl(value);
  if (workItem.provider !== "github") {
    return undefined;
  }
  try {
    const title = await executeFile(
      "gh",
      ["issue", "view", workItem.url, "--json", "title", "--jq", ".title"],
      root,
    );
    return title.trim() || undefined;
  } catch {
    return undefined;
  }
}

function formatState(state: RunState): string {
  const artifacts = state.artifacts
    .map(
      (artifact) =>
        `${artifact.present ? "✓" : artifact.required ? "○" : "·"} ${artifact.kind} (${artifact.actor})`,
    )
    .join("\n");
  return [
    `${state.title} · ${state.workflow_id}@${state.workflow_version}`,
    `Phase: ${state.phase.title} (${state.phase.id}) · visit ${state.phase.visit}`,
    `Work item: ${state.work_item?.url ?? "none"}`,
    `Work item required before: ${state.policy.work_items.required_before_phase ?? "not required"}`,
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
    return {
      content: [{ type: "text" as const, text: JSON.stringify(result, null, 2) }],
      details: result,
    };
  } catch (error) {
    const message = errorMessage(error);
    return {
      content: [{ type: "text" as const, text: `AHEAD error: ${message}` }],
      details: { error: message },
      isError: true,
    };
  }
}

function errorMessage(error: unknown): string {
  if (error instanceof AheadEngineError) {
    return `${error.code}: ${error.message}`;
  }
  return error instanceof Error ? error.message : String(error);
}
