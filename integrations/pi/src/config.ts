import type { AheadProjectConfig, RunStore } from "./storage.js";
import type { PhaseDefinition, WorkflowDefinition } from "./types.js";

interface ProjectConfigCommandContext {
  readonly hasUI: boolean;
  readonly ui: {
    select(title: string, options: string[]): Promise<string | undefined>;
    confirm(title: string, message: string): Promise<boolean>;
    notify(message: string, type?: "info" | "warning" | "error"): void;
  };
}

const recommendedWorkItemBoundaries: Readonly<Record<string, string>> = {
  "product-change": "implement",
  "corrective-debugging": "implement",
  "internal-improvement": "implement",
  decision: "publish",
  investigation: "conclude",
  "operational-stabilization": "outcome",
};

export async function runProjectConfigWizard(
  ctx: ProjectConfigCommandContext,
  store: RunStore,
  workflows: WorkflowDefinition[],
  overwrite: boolean,
): Promise<boolean> {
  if (!ctx.hasUI) {
    throw new Error("/ahead-config requires interactive or RPC UI support");
  }

  const recommended = "Recommended planning boundaries";
  const custom = "Choose a boundary for each workflow";
  const optional = "Do not require work items";
  const choice = await ctx.ui.select("Configure AHEAD for this project", [
    recommended,
    custom,
    optional,
  ]);
  if (!choice) {
    return false;
  }

  let config: AheadProjectConfig;
  if (choice === recommended) {
    config = recommendedProjectConfig(workflows);
  } else if (choice === custom) {
    const selected = await chooseWorkItemBoundaries(ctx, workflows);
    if (!selected) {
      return false;
    }
    config = projectConfig(selected);
  } else if (choice === optional) {
    config = projectConfig({});
  } else {
    return false;
  }

  const confirmed = await ctx.ui.confirm(
    overwrite ? "Replace the AHEAD project configuration?" : "Create this AHEAD configuration?",
    [
      `Project: ${store.projectRoot}`,
      "",
      JSON.stringify(config, null, 2),
      "",
      ...(overwrite
        ? ["The existing file will be preserved under .ahead/backups/ before replacement."]
        : []),
      "Existing runs keep the policy snapshot with which they started.",
    ].join("\n"),
  );
  if (!confirmed) {
    return false;
  }

  const saved = await store.saveProjectConfig(config, { overwrite });
  ctx.ui.notify(
    [
      `Saved AHEAD project configuration: ${saved.path}`,
      ...(saved.backup ? [`Previous file preserved at: ${saved.backup}`] : []),
      "The policy applies to new AHEAD runs; active and saved runs are unchanged.",
    ].join("\n"),
    "info",
  );
  return true;
}

export function recommendedProjectConfig(workflows: WorkflowDefinition[]): AheadProjectConfig {
  const boundaries: Record<string, string> = {};
  for (const workflow of workflows) {
    const phaseId = recommendedWorkItemBoundaries[workflow.id];
    if (!phaseId) {
      throw new Error(`no recommended work-item boundary exists for workflow ${workflow.id}`);
    }
    if (!workflow.phases.some((phase) => phase.id === phaseId)) {
      throw new Error(
        `recommended work-item boundary ${phaseId} does not exist in workflow ${workflow.id}`,
      );
    }
    boundaries[workflow.id] = phaseId;
  }
  return projectConfig(boundaries);
}

export function projectConfigMarkdown(config: AheadProjectConfig): string {
  return [
    "# AHEAD project configuration",
    "",
    "This policy is read when a new run starts and is then snapshotted into that run.",
    "",
    "```json",
    JSON.stringify(config, null, 2),
    "```",
  ].join("\n");
}

export function projectConfigIssues(
  config: AheadProjectConfig,
  workflows: WorkflowDefinition[],
): string[] {
  const issues: string[] = [];
  for (const [workflowId, phaseId] of Object.entries(config.work_items.required_before_phase)) {
    const workflow = workflows.find((candidate) => candidate.id === workflowId);
    if (!workflow) {
      issues.push(`work-item policy names unknown workflow ${workflowId}`);
      continue;
    }
    if (phaseId === workflow.initial_phase) {
      issues.push(`work-item boundary for ${workflowId} cannot be its initial phase ${phaseId}`);
    } else if (!workflow.phases.some((phase) => phase.id === phaseId)) {
      issues.push(`work-item policy for ${workflowId} names unknown phase ${phaseId}`);
    }
  }
  return issues;
}

async function chooseWorkItemBoundaries(
  ctx: ProjectConfigCommandContext,
  workflows: WorkflowDefinition[],
): Promise<Record<string, string> | undefined> {
  const boundaries: Record<string, string> = {};
  for (const workflow of workflows) {
    const phases = workflow.phases.filter((phase) => phase.id !== workflow.initial_phase);
    const notRequired = "Do not require a work item";
    const options = [notRequired, ...phases.map(phaseOption)];
    const selected = await ctx.ui.select(`Work-item policy · ${workflow.title}`, options);
    if (!selected) {
      return undefined;
    }
    if (selected === notRequired) {
      continue;
    }
    const phase = phases.find((candidate) => phaseOption(candidate) === selected);
    if (!phase) {
      throw new Error(`unknown work-item boundary selected for ${workflow.id}`);
    }
    boundaries[workflow.id] = phase.id;
  }
  return boundaries;
}

function phaseOption(phase: PhaseDefinition): string {
  return `Require before ${phase.title} (${phase.id})`;
}

function projectConfig(requiredBeforePhase: Record<string, string>): AheadProjectConfig {
  return {
    api_version: "ahead.config/v0",
    work_items: { required_before_phase: requiredBeforePhase },
  };
}
