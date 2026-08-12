import type { ArtifactState, Run, RunState, WorkflowDefinition } from "./types.js";

export interface PhaseGuide {
  objective: string;
  human: string;
  ai: string;
  artifactPrompts: Record<string, string[]>;
  handoff?: string;
}

const guides: Record<string, PhaseGuide> = {
  define: {
    objective: "Agree on the problem and the observable outcome before solution work begins.",
    human: "Describe the users, current problem, desired outcome, scope, constraints, and success signals.",
    ai: "Explain the prompts. After your first statement, clarify ambiguity and expose assumptions without redefining the problem.",
    artifactPrompts: {
      problem: [
        "Who experiences the problem, and what happens today?",
        "What outcome should change, for whom, and why now?",
        "What is in scope, out of scope, or constrained?",
        "What observable signals would demonstrate success or failure?",
        "Which assumptions or uncertainties should remain visible?",
      ],
    },
  },
  research: {
    objective: "Build enough evidence to understand the problem before choosing a solution.",
    human: "Set the research boundary, judge source relevance, and identify what evidence is material.",
    ai: "Inspect authorized sources, organize evidence, surface contradictions, and state confidence and gaps.",
    artifactPrompts: {
      research: [
        "What sources, code, behavior, history, or measurements were examined?",
        "What findings are supported, contradicted, or still uncertain?",
        "How directly does each finding apply to this work?",
        "What important evidence is still missing?",
      ],
    },
  },
  questions: {
    objective: "Dispose the unknowns that could materially change the decision or plan.",
    human: "Decide which unknowns must be answered and which uncertainty can be accepted explicitly.",
    ai: "Challenge gaps, perform authorized follow-up research, and distinguish evidence from inference.",
    artifactPrompts: {
      unknowns: [
        "Which questions could materially change the work?",
        "What answer or evidence resolved each question?",
        "Which unknowns remain, and why is proceeding still acceptable?",
      ],
      "question-research": [
        "What follow-up question was investigated?",
        "What evidence was found, and what remains uncertain?",
      ],
    },
  },
  options: {
    objective: "Understand viable approaches and tradeoffs before committing to one.",
    human: "Produce the first option, then evaluate alternatives and tradeoffs in the system's real context.",
    ai: "After the human first pass, challenge assumptions and add materially different alternatives.",
    artifactPrompts: {
      "human-option": [
        "What is your first workable approach?",
        "Why might it fit the problem and constraints?",
        "What risks, costs, and uncertainties do you already see?",
      ],
      "ai-challenge": [
        "Which assumptions or failure modes does the first option overlook?",
        "What materially different alternatives deserve consideration?",
      ],
      options: [
        "Which options were considered?",
        "What are their tradeoffs, failure modes, reversibility, and operational consequences?",
        "Which options were rejected, and why?",
      ],
    },
  },
  decision: {
    objective: "Make an accountable and explainable choice.",
    human: "Choose the approach and own its rationale, tradeoffs, remaining uncertainty, and reversibility.",
    ai: "Test the recorded decision for contradictions, weak evidence, and hidden consequences.",
    artifactPrompts: {
      decision: [
        "What was decided?",
        "Why does it best fit the evidence and constraints?",
        "What tradeoffs and risks are accepted?",
        "What remains unknown, and how reversible is the decision?",
      ],
    },
  },
  plan: {
    objective: "Create an implementable plan with verification, rollout, and recovery.",
    human: "Write the first-pass sequence and approve the final plan after challenges are resolved.",
    ai: "After the human first pass, identify missing dependencies, tests, edge cases, rollout evidence, recovery, and decision points.",
    artifactPrompts: {
      "first-pass-plan": [
        "What sequence of changes do you currently expect?",
        "Which systems, boundaries, and dependencies are involved?",
        "How will you test, release, observe, and recover?",
      ],
      "ai-plan-review": [
        "What is missing, risky, ambiguous, or ordered incorrectly?",
        "Which tests, edge cases, rollout checks, and recovery steps should be added?",
      ],
      plan: [
        "What is the final ordered implementation sequence?",
        "What tests and observable evidence are required?",
        "How will rollout and recovery work?",
        "Which deviations require returning to decision or plan?",
      ],
    },
  },
  implement: {
    objective: "Produce a change the engineer understands and can defend.",
    human: "Make the first attempt, ask questions freely, own the implementation, understand every lasting change, run the planned checks, and record deviations.",
    ai: "Coach, explain, help diagnose, and suggest bounded next steps within the approved plan. Do not turn a question into taking over the implementation.",
    artifactPrompts: {
      changeset: [
        "What exact commit, branch, pull request, or diff identifies the current change?",
        "Can you explain the important behavior and design choices?",
      ],
      tests: [
        "Which planned and additional checks ran?",
        "What passed, failed, or was not run?",
        "What evidence covers boundaries, failures, and regressions?",
      ],
      "plan-deviations": [
        "Where did implementation differ from the approved plan?",
        "Why was each deviation acceptable, or state explicitly that there were none?",
      ],
    },
  },
  "ai-review": {
    objective: "Add an AI review of the exact current changeset before independent human review.",
    human: "Validate and dispose every material finding. Return to implementation when a change is required.",
    ai: "Review without modifying: correctness, security, tests, architecture, plan compliance, operations, and maintainability.",
    artifactPrompts: {
      "ai-review": [
        "What exact commit or diff was reviewed?",
        "What findings were identified, with evidence and severity?",
        "What disposition is proposed for each finding?",
        "What could not be assessed?",
      ],
    },
  },
  "human-review": {
    objective: "Obtain independent final engineering judgment on the current change.",
    human: "A reviewer other than the implementer examines the exact change and material evidence, then accepts or returns it.",
    ai: "Retrieve evidence and answer targeted questions. It cannot approve the change or replace reviewer judgment.",
    handoff: "READY FOR INDEPENDENT HUMAN REVIEW",
    artifactPrompts: {
      "human-review": [
        "Who reviewed the change, and what exact commit or diff was reviewed?",
        "What code, tests, evidence, risks, and operational consequences were examined?",
        "Which findings must be addressed or explicitly accepted?",
        "Does the reviewer understand and accept the current change?",
      ],
    },
  },
  deploy: {
    objective: "Authorize and record deployment or explicitly establish that it is not applicable.",
    human: "Own the release decision and production risk. Record the exact version, target, actor, time, authorization, and result.",
    ai: "Analyze readiness evidence. It cannot authorize deployment or claim a version is live.",
    artifactPrompts: {
      deployment: [
        "What exact version was released to which target?",
        "Who authorized and performed it, and when?",
        "What did the deployment system report?",
        "If deployment is not applicable, why?",
      ],
    },
  },
  verify: {
    objective: "Demonstrate the intended outcome using observed evidence, not just test or deployment status.",
    human: "Select adequate checks and decide whether the original success signals are demonstrated.",
    ai: "Suggest checks and analyze authorized observations while separating code, deployment, and observed behavior.",
    artifactPrompts: {
      verification: [
        "What pre-change and post-change behavior was compared?",
        "Which tests, deployment facts, runtime observations, and user-visible signals were checked?",
        "Did the original success and failure signals occur?",
        "What remains uncertain or needs continued observation?",
      ],
    },
  },
  "ai-audit": {
    objective: "Compare the result with the original intent and expose weak evidence or divergence.",
    human: "Review and dispose material audit findings; reopen work when the evidence demands it.",
    ai: "Audit the full chain from problem through observed outcome without changing or approving the work.",
    artifactPrompts: {
      "ai-audit": [
        "Where did the result diverge from the problem, decision, plan, or reviews?",
        "Which claims have weak or missing evidence?",
        "Which findings require follow-up or reopening?",
      ],
    },
  },
  outcome: {
    objective: "Make the accountable outcome decision and preserve learning.",
    human: "Accept, roll back, follow up, abandon, or reopen the work, including remaining uncertainty.",
    ai: "Organize evidence and summarize learning. It cannot choose or accept the outcome.",
    artifactPrompts: {
      outcome: [
        "What outcome decision is being made?",
        "Which evidence supports it?",
        "What uncertainty, debt, monitoring, or follow-up remains?",
        "What should the team or AHEAD process learn from this run?",
      ],
    },
  },
};

const fallbackGuide: PhaseGuide = {
  objective: "Complete the active phase using the recorded workflow contract.",
  human: "Own the phase decision, required evidence, and gate.",
  ai: "Assist only within the capabilities and artifact ownership allowed by the active phase.",
  artifactPrompts: {},
};

export function phaseGuide(phaseId: string): PhaseGuide {
  return guides[phaseId] ?? fallbackGuide;
}

export function promptsForArtifact(phaseId: string, kind: string): string[] {
  return phaseGuide(phaseId).artifactPrompts[kind] ?? [
    "What must another engineer understand from this record?",
    "What evidence, uncertainty, and rationale should remain durable?",
  ];
}

export function phasePosition(state: RunState, workflow: WorkflowDefinition): { current: number; total: number } {
  const index = workflow.phases.findIndex((phase) => phase.id === state.phase.id);
  return { current: index < 0 ? 0 : index + 1, total: workflow.phases.length };
}

export interface GuidedNextAction {
  actor: "human" | "ai";
  label: string;
  artifactKind?: string;
  optional?: boolean;
}

export function nextAction(state: RunState, workflow: WorkflowDefinition): GuidedNextAction {
  if (state.closed) return { actor: "human", label: "Work complete; start another run only for new work" };

  const orderedAssist = optionalAssistBeforeFinalHumanRecord(state);
  if (orderedAssist) return orderedAssist;

  const artifact = state.artifacts.find((candidate) => candidate.required && !candidate.present);
  if (artifact) {
    const actor = artifact.actor === "human"
      ? "human"
      : artifact.actor === "ai" || state.phase.id === "research"
        ? "ai"
        : "human";
    if (actor === "human") {
      const label = state.phase.id === "implement" && artifact.kind === "changeset"
        ? "Implement first, then record the exact changeset"
        : state.phase.id === "human-review"
        ? "Independent reviewer records the current human review"
        : `Write ${artifact.title}`;
      return { actor, label, artifactKind: artifact.kind };
    }

    const label = state.phase.id === "ai-review"
      ? "Run AI review of the exact current changeset"
      : state.phase.id === "ai-audit"
        ? "Run AI audit across intent, evidence, and outcome"
        : `Ask AI to produce ${artifact.title}`;
    return { actor, label, artifactKind: artifact.kind };
  }

  if (!state.gate.accepted) return { actor: "human", label: `Review evidence and accept: ${state.gate.title}` };

  const nextPhase = workflow.phases.find((phase) => phase.id === state.phase.next);
  return {
    actor: "human",
    label: nextPhase ? `Continue to ${nextPhase.title}` : "Accept the outcome and close this AHEAD run",
  };
}

function optionalAssistBeforeFinalHumanRecord(state: RunState): GuidedNextAction | undefined {
  const orderedAssist = state.phase.id === "options"
    ? { prerequisite: "human-option", assist: "ai-challenge", final: "options" }
    : state.phase.id === "plan"
      ? { prerequisite: "first-pass-plan", assist: "ai-plan-review", final: "plan" }
      : undefined;
  if (!orderedAssist) return undefined;

  const prerequisite = state.artifacts.find((artifact) => artifact.kind === orderedAssist.prerequisite);
  const assist = state.artifacts.find((artifact) => artifact.kind === orderedAssist.assist);
  const final = state.artifacts.find((artifact) => artifact.kind === orderedAssist.final);
  if (!prerequisite?.present || assist?.present || final?.present) return undefined;

  return {
    actor: "ai",
    artifactKind: assist?.kind,
    label: state.phase.id === "options"
      ? "Ask AI to challenge the human option and expand alternatives"
      : "Ask AI to challenge the human first-pass plan",
    optional: true,
  };
}

export function buildWidgetLines(run: Run, state: RunState, workflow: WorkflowDefinition): string[] {
  if (state.closed) {
    return [
      `AHEAD COMPLETE · ${run.title}`,
      "The accountable human accepted the outcome and closed this run.",
      "Run /ahead only when new work needs a new workflow.",
    ];
  }

  const guide = phaseGuide(state.phase.id);
  const position = phasePosition(state, workflow);
  const required = state.artifacts.filter((artifact) => artifact.required);
  const checklist = required.length
    ? required.map(formatArtifactStatus).join(" · ")
    : "No required artifact";
  const action = nextAction(state, workflow);

  return [
    `AHEAD MODE · ${workflow.title.toUpperCase()} · ${position.current}/${position.total}`,
    `${guide.handoff ?? state.phase.title.toUpperCase()} · HUMAN LEADS · AI ASSISTS`,
    `Goal: ${guide.objective}`,
    `You: ${guide.human}`,
    `AI: ${guide.ai}`,
    `Required: ${checklist}`,
    `Next (${action.actor === "human" ? "you" : "AI"}): ${action.label}`,
    "Run /ahead for the guided action · /ahead-guide for framework docs.",
  ];
}

export function buildArtifactTemplate(
  run: Run,
  state: RunState,
  kind: string,
  title: string,
): string {
  const prompts = promptsForArtifact(state.phase.id, kind);
  return [
    `# ${title}`,
    "",
    `AHEAD run: ${run.id}`,
    `Phase: ${state.phase.id} (visit ${state.phase.visit})`,
    `Artifact: ${kind}`,
    "",
    "## What to cover",
    "",
    ...prompts.map((prompt) => `- ${prompt}`),
    "",
    "## Record",
    "",
    "<!-- Write this human-owned record in your own words. Preserve evidence, uncertainty, and rationale. -->",
    "",
  ].join("\n");
}

function formatArtifactStatus(artifact: ArtifactState): string {
  const owner = artifact.actor === "ai" ? "AI" : artifact.actor === "human" ? "you" : "you/AI";
  return `${artifact.present ? "✓" : "○"} ${artifact.title} [${owner}]`;
}
