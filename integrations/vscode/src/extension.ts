import { readFile } from "node:fs/promises";
import { join } from "node:path";
import * as vscode from "vscode";
import { recommendedProjectConfig } from "../../pi/src/config.js";
import { AheadEngine, AheadEngineError } from "../../pi/src/engine.js";
import {
  buildArtifactTemplate,
  insertFieldExamples,
  nextAction,
  phaseGuide,
  phasePosition,
  promptsForArtifact,
  validateArtifactForm,
} from "../../pi/src/guidance.js";
import {
  findReference,
  loadReferenceIndex,
  readReference,
  relevantReferences,
} from "../../pi/src/reference.js";
import {
  collectReviewSnapshot,
  extractFindingIds,
  extractReviewFingerprint,
  reviewDispositionTemplate,
  reviewRequest,
  validateAiReviewArtifact,
  validateReviewDisposition,
} from "../../pi/src/review.js";
import {
  humanActor,
  projectRoot,
  RunStore,
  type AheadProjectConfig,
} from "../../pi/src/storage.js";
import type {
  ArtifactState,
  EventAction,
  Run,
  RunState,
  WorkItem,
  WorkflowDefinition,
} from "../../pi/src/types.js";
import { draftFieldExamples } from "./examples.js";

interface ActiveRun {
  root: string;
  store: RunStore;
  run: Run;
  state: RunState;
  workflow: WorkflowDefinition;
}

interface ReferenceInput {
  topic?: string;
}

interface RecordArtifactInput {
  kind: string;
  content: string;
}

type EmptyInput = Record<string, never>;

interface ViewRow {
  label: string;
  description?: string;
  tooltip?: string;
  icon?: string;
  command?: string;
  arguments?: unknown[];
  children?: ViewRow[];
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const controller = new AheadController(context);
  await controller.activate();
}

export function deactivate(): void {}

class AheadController {
  private readonly changed = new vscode.EventEmitter<void>();
  private readonly engine: Promise<AheadEngine>;
  private readonly status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 50);

  constructor(private readonly context: vscode.ExtensionContext) {
    this.engine = AheadEngine.load(context.asAbsolutePath(join("dist", "ahead_wasm.wasm")));
  }

  async activate(): Promise<void> {
    const provider = new AheadTreeProvider(this);
    this.context.subscriptions.push(
      this.changed,
      this.status,
      vscode.window.registerTreeDataProvider("ahead.workflow", provider),
      ...this.commands(),
      ...this.tools(),
    );
    for (const folder of vscode.workspace.workspaceFolders ?? []) {
      const watcher = vscode.workspace.createFileSystemWatcher(
        new vscode.RelativePattern(folder, ".ahead/**/*.json"),
      );
      watcher.onDidCreate(() => this.refresh());
      watcher.onDidChange(() => this.refresh());
      watcher.onDidDelete(() => this.refresh());
      this.context.subscriptions.push(watcher);
    }
    await this.refresh();
  }

  changeEvent(): vscode.Event<void> {
    return this.changed.event;
  }

  async viewRows(): Promise<ViewRow[]> {
    const root = this.workspaceRoot();
    if (!root) {
      return [{ label: "Open a project to use AHEAD", icon: "folder-opened" }];
    }
    const store = new RunStore(root);
    const run = await store.loadCurrent();
    if (!run) {
      const saved = await store.listRunIds();
      return [
        { label: "Start workflow", icon: "play", command: "ahead.start" },
        ...(saved.length
          ? [
              {
                label: `Resume saved work (${saved.length})`,
                icon: "history",
                command: "ahead.resume",
              },
            ]
          : []),
        { label: "Configure project", icon: "settings-gear", command: "ahead.configure" },
      ];
    }

    const active = await this.activeRun();
    const guide = phaseGuide(active.state.workflow_id, active.state.phase.id);
    const position = phasePosition(active.state, active.workflow);
    const action = nextAction(active.state, active.workflow);
    const required = active.state.artifacts.filter((artifact) => artifact.required);
    const actionCommand =
      action.actor === "ai" ? "ahead.askCopilot" : this.humanActionCommand(active.state);
    return [
      {
        label: active.state.closed ? "Complete" : active.state.phase.title,
        description: `${active.workflow.title} · ${position.current}/${position.total}`,
        tooltip: active.run.title,
        icon: active.state.closed ? "pass-filled" : "compass",
      },
      { label: "Goal", description: guide.objective, tooltip: guide.objective, icon: "target" },
      {
        label: action.actor === "ai" ? "AI assists next" : "You lead next",
        description: action.label,
        tooltip: action.label,
        icon: action.actor === "ai" ? "sparkle" : "person",
        command: actionCommand,
        arguments: action.artifactKind ? [action.artifactKind] : undefined,
      },
      {
        label: "Work item",
        description: active.state.work_item?.url ?? this.workItemDescription(active),
        tooltip: active.state.work_item?.url,
        icon: "link",
        command: active.state.work_item ? "ahead.openWorkItem" : "ahead.linkWorkItem",
        arguments: active.state.work_item ? [active.state.work_item.url] : undefined,
      },
      {
        label: "Required evidence",
        description: `${required.filter((artifact) => artifact.present).length}/${required.length}`,
        icon: "checklist",
        children: required.map((artifact) => ({
          label: artifact.title,
          description: `${artifact.present ? "recorded" : "missing"} · ${artifactOwner(artifact)}`,
          icon: artifact.present ? "pass" : "circle-large-outline",
          command: artifact.present
            ? "ahead.openArtifact"
            : artifact.actor === "ai"
              ? "ahead.askCopilot"
              : "ahead.recordHumanArtifact",
          arguments: artifact.present ? [artifact.path] : [artifact.kind],
        })),
      },
      ...(active.state.blockers.length
        ? [
            {
              label: "Blockers",
              description: String(active.state.blockers.length),
              icon: "warning",
              children: active.state.blockers.map((blocker) => ({
                label: blocker,
                tooltip: blocker,
                icon: "circle-slash",
              })),
            },
          ]
        : []),
      {
        label: "Actions",
        icon: "tools",
        children: [
          { label: "Open framework guide", icon: "book", command: "ahead.openGuide" },
          { label: "Link work item", icon: "link", command: "ahead.linkWorkItem" },
          ...(active.state.return_targets.length
            ? [{ label: "Return to earlier phase", icon: "discard", command: "ahead.return" }]
            : []),
          { label: "Stop AHEAD", icon: "debug-stop", command: "ahead.stop" },
        ],
      },
    ];
  }

  async refresh(): Promise<void> {
    try {
      const active = await this.optionalActiveRun();
      if (!active) {
        this.status.hide();
        await vscode.commands.executeCommand("setContext", "ahead.active", false);
      } else {
        this.status.text = active.state.closed
          ? "$(pass-filled) AHEAD complete"
          : `$(compass) AHEAD · ${active.state.phase.title}`;
        this.status.tooltip = `${active.workflow.title}: ${active.run.title}`;
        this.status.command = "ahead.workflow.focus";
        this.status.show();
        await vscode.commands.executeCommand("setContext", "ahead.active", !active.state.closed);
      }
    } catch (error) {
      this.status.text = "$(error) AHEAD · invalid state";
      this.status.tooltip = errorMessage(error);
      this.status.show();
    }
    this.changed.fire();
  }

  async contextForAgent(): Promise<unknown> {
    const active = await this.activeRun();
    const policy = await readFile(
      this.context.asAbsolutePath(
        join("generated", active.state.workflow_id, `${active.state.phase.id}.md`),
      ),
      "utf8",
    );
    return {
      run: active.run,
      state: active.state,
      guidance: phaseGuide(active.state.workflow_id, active.state.phase.id),
      next_action: nextAction(active.state, active.workflow),
      active_phase_policy: policy,
      instruction:
        "The human owns gates and decisions. Use only capabilities in allowed_ai_capabilities; unknown means denied.",
    };
  }

  async referenceForAgent(topic?: string): Promise<unknown> {
    const active = await this.optionalActiveRun();
    if (!topic?.trim()) {
      const index = await loadReferenceIndex();
      return {
        recommended: await relevantReferences(active?.state.workflow_id, active?.state.phase.id),
        available: index.references.map(({ id, title, path, audience, authority }) => ({
          id,
          title,
          path,
          audience,
          authority,
        })),
        instruction: "Load only the reference needed for the current question.",
      };
    }
    const entry = await findReference(topic);
    if (!entry) {
      throw new Error(`No packaged AHEAD reference matches ${topic}`);
    }
    return { reference: entry, content: await readReference(entry) };
  }

  async reviewSnapshot(): Promise<unknown> {
    return collectReviewSnapshot((await this.activeRun()).root);
  }

  async recordAiArtifact(input: RecordArtifactInput): Promise<unknown> {
    const active = await this.activeRun();
    const artifact = active.state.artifacts.find((candidate) => candidate.kind === input.kind);
    if (!artifact || artifact.actor === "human") {
      throw new AheadEngineError(
        "artifact_not_ai_owned",
        `AI cannot record ${input.kind} in phase ${active.state.phase.id}`,
      );
    }
    if (!active.state.allowed_ai_capabilities.includes("record")) {
      throw new AheadEngineError(
        "record_not_allowed",
        `AI record capability is locked in phase ${active.state.phase.id}`,
      );
    }
    if (artifact.kind === "ai-review") {
      const snapshot = await collectReviewSnapshot(active.root);
      const errors = validateAiReviewArtifact(input.content, snapshot.fingerprint);
      if (errors.length) {
        throw new AheadEngineError("invalid_ai_review", errors.join("; "));
      }
    }
    const path = active.store.artifactPath(active.run, active.state.phase.id, artifact.kind);
    const updated = (await this.engine).applyEvent(
      active.run,
      { kind: "ai", identity: "github-copilot/vscode" },
      {
        type: "artifact_recorded",
        phase: active.state.phase.id,
        kind: artifact.kind,
        path: path.relative,
      },
    );
    await active.store.writeArtifact(path.absolute, input.content);
    await active.store.save(updated);
    await this.refresh();
    return {
      recorded: artifact.kind,
      path: path.relative,
      state: (await this.engine).deriveState(updated),
    };
  }

  private commands(): vscode.Disposable[] {
    return [
      this.command("ahead.start", () => this.start()),
      this.command("ahead.recordHumanArtifact", (kind?: unknown) =>
        this.recordHumanArtifact(typeof kind === "string" ? kind : undefined),
      ),
      this.command("ahead.askCopilot", (kind?: unknown) =>
        this.askCopilot(typeof kind === "string" ? kind : undefined),
      ),
      this.command("ahead.acceptAndAdvance", () => this.acceptAndAdvance()),
      this.command("ahead.return", () => this.returnToEarlierPhase()),
      this.command("ahead.linkWorkItem", () => this.linkWorkItem()),
      this.command("ahead.openGuide", () => this.openGuide()),
      this.command("ahead.configure", async () => {
        await this.configure();
      }),
      this.command("ahead.stop", () => this.stop()),
      this.command("ahead.resume", () => this.resume()),
      this.command("ahead.refresh", () => this.refresh()),
      this.command("ahead.openWorkItem", (url?: unknown) => this.openUrl(url)),
      this.command("ahead.openArtifact", (path?: unknown) => this.openArtifact(path)),
    ];
  }

  private tools(): vscode.Disposable[] {
    return [
      vscode.lm.registerTool("ahead_get_context", new ReadTool(() => this.contextForAgent())),
      vscode.lm.registerTool(
        "ahead_get_reference",
        new ReferenceTool((topic) => this.referenceForAgent(topic)),
      ),
      vscode.lm.registerTool(
        "ahead_get_review_snapshot",
        new ReadTool(() => this.reviewSnapshot()),
      ),
      vscode.lm.registerTool("ahead_record_artifact", new RecordTool(this)),
      vscode.lm.registerTool(
        "ahead_request_transition",
        new ReadTool(async () => {
          const { state } = await this.activeRun();
          return {
            transitioned: false,
            can_advance: state.can_advance,
            blockers: state.blockers,
            message: state.can_advance
              ? "Ask the human to use AHEAD: Accept Gate and Continue."
              : "The human must resolve the blockers and accept the gate.",
          };
        }),
      ),
    ];
  }

  private command(
    id: string,
    action: (...arguments_: unknown[]) => Promise<void>,
  ): vscode.Disposable {
    return vscode.commands.registerCommand(id, async (...arguments_: unknown[]) => {
      try {
        await action(...arguments_);
      } catch (error) {
        await vscode.window.showErrorMessage(`AHEAD: ${errorMessage(error)}`);
      }
    });
  }

  private async start(): Promise<void> {
    const root = this.requireWorkspaceRoot();
    const store = new RunStore(root);
    const current = await store.loadCurrent();
    if (current && !(await this.engine).deriveState(current).closed) {
      await vscode.window.showInformationMessage("AHEAD is already active for this project.");
      return;
    }
    if ((await store.inspectProjectConfig()).status === "missing") {
      const setup = await vscode.window.showInformationMessage(
        "This project has no AHEAD configuration.",
        "Run setup",
        "Use defaults",
      );
      if (setup === "Run setup" && !(await this.configure())) {
        return;
      }
      if (!setup) {
        return;
      }
    }
    const engine = await this.engine;
    const workflows = engine.listWorkflows();
    const selected = await vscode.window.showQuickPick(
      workflows.map((workflow) => ({ label: workflow.title, description: workflow.id, workflow })),
      { title: "Choose the AHEAD workflow for this work" },
    );
    if (!selected) {
      return;
    }
    const title = await vscode.window.showInputBox({
      title: `Start AHEAD · ${selected.workflow.title}`,
      prompt: `Short name for this work — e.g. ${titleExample(selected.workflow.id)}`,
      placeHolder: titleExample(selected.workflow.id),
      validateInput: (value) => (value.trim() ? undefined : "A title is required"),
    });
    if (!title?.trim()) {
      return;
    }
    const actor = humanActor(root);
    let run = engine.createRun({
      id: store.newRunId(),
      title: title.trim(),
      owner: actor,
      timestamp: new Date().toISOString(),
      workflow_id: selected.workflow.id,
      policy: await store.policyForWorkflow(selected.workflow.id),
    });
    const workItemUrl = await vscode.window.showInputBox({
      title: "Link existing work (optional)",
      prompt: "Paste a GitHub issue or other work-item URL, or press Enter to skip",
      validateInput: optionalUrlIssue,
    });
    if (workItemUrl?.trim()) {
      run = engine.applyEvent(run, actor, {
        type: "work_item_linked",
        work_item: workItemFromUrl(workItemUrl.trim()),
      });
    }
    await store.save(run);
    await this.refresh();
    await vscode.commands.executeCommand("ahead.workflow.focus");
    await vscode.window.showInformationMessage(
      `AHEAD started: ${selected.workflow.title}. It remains active until you finish or stop it.`,
    );
  }

  private async recordHumanArtifact(requestedKind?: string): Promise<void> {
    const active = await this.activeRun();
    const allowed = active.state.artifacts.filter(
      (artifact) => artifact.actor !== "ai" && !artifact.present,
    );
    const detectedKind = vscode.window.activeTextEditor?.document
      .getText()
      .match(/^Artifact:\s*(\S+)\s*$/m)?.[1];
    const kind = requestedKind ?? detectedKind ?? (await this.pickArtifact(allowed));
    if (!kind) {
      return;
    }
    const artifact = allowed.find((candidate) => candidate.kind === kind);
    if (!artifact) {
      throw new AheadEngineError(
        "artifact_not_human_owned",
        `There is no unrecorded human artifact named ${kind} in ${active.state.phase.title}`,
      );
    }
    const editor = vscode.window.activeTextEditor;
    if (
      !editor ||
      !editor.document.getText().match(new RegExp(`^Artifact:\\s*${escapePattern(kind)}\\s*$`, "m"))
    ) {
      let template = await this.humanArtifactTemplate(active, artifact);
      if (artifact.kind !== "review-disposition") {
        const prompts = promptsForArtifact(
          active.state.workflow_id,
          active.state.phase.id,
          artifact.kind,
        );
        if (prompts.length > 0) {
          const seeded = await vscode.window.withProgress(
            {
              location: vscode.ProgressLocation.Notification,
              title: "AHEAD: drafting example prompts (inspiration only)…",
              cancellable: true,
            },
            async (_progress, token) => {
              if (token.isCancellationRequested) {
                return undefined;
              }
              const draft = await draftFieldExamples(
                {
                  workflowTitle: active.workflow.title,
                  phaseTitle: active.state.phase.title,
                  runTitle: active.run.title,
                  fields: prompts,
                },
                token,
              );
              if (draft.skipped === "no-model") {
                await vscode.window.showInformationMessage(
                  "AHEAD example prompts unavailable: no chat model is available.",
                );
              }
              // Other failures stay silent: the plain template is the fallback.
              return draft.examples;
            },
          );
          if (seeded) {
            template = insertFieldExamples(template, seeded);
          }
        }
      }
      const document = await vscode.workspace.openTextDocument({
        content: template,
        language: "markdown",
      });
      await vscode.window.showTextDocument(document, { preview: false });
      await vscode.window.showInformationMessage(
        `Write ${artifact.title} in your own words, then run “AHEAD: Write or Record Human Artifact” again.`,
      );
      return;
    }
    const content = editor.document.getText().trim();
    if (!content || content.includes("<!-- Write this human-owned record")) {
      throw new Error(
        "Complete the human-owned record and remove its placeholder before recording it",
      );
    }
    if (artifact.kind !== "review-disposition") {
      const formErrors = validateArtifactForm(
        content,
        promptsForArtifact(active.state.workflow_id, active.state.phase.id, artifact.kind),
      );
      if (formErrors.length > 0) {
        throw new Error(
          [
            `Some required fields in ${artifact.title} are still empty:`,
            ...formErrors.map((field) => `• ${field}`),
            "",
            "Your text is preserved in this editor — fill the fields and run “AHEAD: Write or Record Human Artifact” again.",
          ].join("\n"),
        );
      }
    }
    await this.validateHumanReview(active, artifact.kind, content);
    const path = active.store.artifactPath(active.run, active.state.phase.id, artifact.kind);
    const updated = (await this.engine).applyEvent(active.run, humanActor(active.root), {
      type: "artifact_recorded",
      phase: active.state.phase.id,
      kind: artifact.kind,
      path: path.relative,
    });
    await active.store.writeArtifact(path.absolute, content);
    await active.store.save(updated);
    await this.refresh();
    const nextState = (await this.engine).deriveState(updated);
    const nextArtifact = nextState.artifacts.find(
      (candidate) => candidate.required && !candidate.present,
    );
    const nextStep = nextArtifact
      ? `Next: ${nextArtifact.title}.`
      : `Next: accept “${nextState.gate.title}” with AHEAD: Accept Gate and Continue.`;
    await vscode.window.showInformationMessage(`Recorded ${artifact.title}. ${nextStep}`);
  }

  private async askCopilot(requestedKind?: string): Promise<void> {
    const active = await this.activeRun();
    const artifact = requestedKind
      ? active.state.artifacts.find((candidate) => candidate.kind === requestedKind)
      : undefined;
    const guidance = phaseGuide(active.state.workflow_id, active.state.phase.id);
    const query =
      artifact?.kind === "ai-review"
        ? reviewRequest(await collectReviewSnapshot(active.root))
        : [
            `AHEAD mode: assist with ${active.state.phase.title} for the active run.`,
            guidance.ai,
            "Call ahead_get_context first and obey its active capabilities.",
            ...(artifact
              ? [
                  `Produce ${artifact.title} and record it as ${artifact.kind} with ahead_record_artifact when complete.`,
                ]
              : []),
            "Do not author human-owned evidence, accept a gate, or transition the run.",
          ].join("\n");
    await vscode.env.clipboard.writeText(query);
    await vscode.commands.executeCommand("workbench.action.chat.open", {
      query,
      isPartialQuery: true,
    });
  }

  private async acceptAndAdvance(): Promise<void> {
    const active = await this.activeRun();
    const missing = active.state.artifacts.filter(
      (artifact) => artifact.required && !artifact.present,
    );
    if (missing.length) {
      throw new AheadEngineError(
        "required_artifact_missing",
        `Complete first: ${missing.map((artifact) => artifact.title).join(", ")}`,
      );
    }
    const nextTitle = active.state.phase.next
      ? (active.workflow.phases.find((phase) => phase.id === active.state.phase.next)?.title ??
        active.state.phase.next)
      : "close this AHEAD run";
    const confirmed = await vscode.window.showWarningMessage(
      `${active.state.gate.title}\n\nNext: ${nextTitle}\nHuman: ${humanActor(active.root).identity}`,
      { modal: true },
      "Accept and continue",
    );
    if (!confirmed) {
      return;
    }
    const engine = await this.engine;
    const actor = humanActor(active.root);
    let updated = active.run;
    let state = active.state;
    if (!state.gate.accepted) {
      updated = engine.applyEvent(updated, actor, {
        type: "gate_accepted",
        phase: state.phase.id,
        gate: state.gate.id,
      });
      state = engine.deriveState(updated);
    }
    if (!state.can_advance) {
      await active.store.save(updated);
      await this.refresh();
      throw new AheadEngineError(
        "cannot_advance",
        state.blockers.join("; ") || "The phase cannot advance",
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
    await active.store.save(updated);
    await this.refresh();
    const next = engine.deriveState(updated);
    await vscode.window.showInformationMessage(
      next.closed ? "AHEAD work complete." : `AHEAD continued to ${next.phase.title}.`,
    );
  }

  private async returnToEarlierPhase(): Promise<void> {
    const active = await this.activeRun();
    const selected = await vscode.window.showQuickPick(
      active.state.return_targets.map((id) => ({
        label: active.workflow.phases.find((phase) => phase.id === id)?.title ?? id,
        description: id,
      })),
      { title: "Return to which phase?" },
    );
    if (!selected) {
      return;
    }
    const reason = await vscode.window.showInputBox({
      title: `Why return to ${selected.label}?`,
      validateInput: (value) => (value.trim() ? undefined : "A reason is required"),
    });
    if (!reason?.trim()) {
      return;
    }
    const updated = (await this.engine).applyEvent(active.run, humanActor(active.root), {
      type: "phase_transitioned",
      from: active.state.phase.id,
      to: selected.description,
      direction: "return",
      reason: reason.trim(),
    });
    await active.store.save(updated);
    await this.refresh();
  }

  private async linkWorkItem(): Promise<void> {
    const active = await this.activeRun();
    const url = await vscode.window.showInputBox({
      title: "Link an existing work item",
      prompt: "GitHub issue or provider-neutral work-item URL",
      value: active.state.work_item?.url,
      validateInput: requiredUrlIssue,
    });
    if (!url?.trim()) {
      return;
    }
    const updated = (await this.engine).applyEvent(active.run, humanActor(active.root), {
      type: "work_item_linked",
      work_item: workItemFromUrl(url.trim()),
    });
    await active.store.save(updated);
    await this.refresh();
  }

  private async openGuide(): Promise<void> {
    const active = await this.optionalActiveRun();
    const references = await relevantReferences(active?.state.workflow_id, active?.state.phase.id);
    const index = await loadReferenceIndex();
    const entries = references.length ? references : index.references;
    const selected = await vscode.window.showQuickPick(
      entries.map((entry) => ({ label: entry.title, description: entry.path, entry })),
      { title: "Open AHEAD framework guidance" },
    );
    if (!selected) {
      return;
    }
    const document = await vscode.workspace.openTextDocument({
      content: await readReference(selected.entry),
      language: "markdown",
    });
    await vscode.window.showTextDocument(document, { preview: true });
  }

  private async configure(): Promise<boolean> {
    const root = this.requireWorkspaceRoot();
    const store = new RunStore(root);
    const workflows = (await this.engine).listWorkflows();
    const inspection = await store.inspectProjectConfig();
    const selected = await vscode.window.showQuickPick(
      [
        { label: "Recommended planning boundaries", mode: "recommended" },
        { label: "Choose a boundary for each workflow", mode: "custom" },
        { label: "Do not require work items", mode: "optional" },
      ],
      {
        title: inspection.status === "missing" ? "Configure AHEAD" : "Replace AHEAD configuration",
      },
    );
    if (!selected) {
      return false;
    }
    let config: AheadProjectConfig;
    if (selected.mode === "recommended") {
      config = recommendedProjectConfig(workflows);
    } else if (selected.mode === "optional") {
      config = emptyProjectConfig();
    } else {
      const boundaries: Record<string, string> = {};
      for (const workflow of workflows) {
        const choice = await vscode.window.showQuickPick(
          [
            { label: "Do not require a work item", phase: undefined },
            ...workflow.phases
              .filter((phase) => phase.id !== workflow.initial_phase)
              .map((phase) => ({
                label: `Before ${phase.title}`,
                description: phase.id,
                phase: phase.id,
              })),
          ],
          { title: `Work-item policy · ${workflow.title}` },
        );
        if (!choice) {
          return false;
        }
        if (choice.phase) {
          boundaries[workflow.id] = choice.phase;
        }
      }
      config = {
        api_version: "ahead.config/v0",
        work_items: { required_before_phase: boundaries },
      };
    }
    const confirmed = await vscode.window.showWarningMessage(
      `${JSON.stringify(config, null, 2)}\n\nExisting runs keep their current policy snapshot.`,
      { modal: true },
      inspection.status === "missing" ? "Create configuration" : "Replace configuration",
    );
    if (!confirmed) {
      return false;
    }
    const saved = await store.saveProjectConfig(config, {
      overwrite: inspection.status !== "missing",
    });
    await vscode.window.showInformationMessage(
      `Saved ${saved.path}${saved.backup ? `; previous config: ${saved.backup}` : ""}.`,
    );
    await this.refresh();
    return true;
  }

  private async stop(): Promise<void> {
    const active = await this.activeRun();
    const selected = await vscode.window.showQuickPick(
      [
        {
          label: "Discard unfinished run",
          detail: "Default: remove the run instead of retaining incomplete work",
          mode: "discard",
        },
        { label: "Save for resume", detail: "Keep the run but leave AHEAD mode", mode: "save" },
      ],
      { title: "Stop AHEAD" },
    );
    if (!selected) {
      return;
    }
    if (selected.mode === "save") {
      await active.store.saveCurrentForResume(active.run.id);
    } else {
      const confirmed = await vscode.window.showWarningMessage(
        `Discard unfinished AHEAD run “${active.run.title}”?`,
        { modal: true },
        "Discard",
      );
      if (!confirmed) {
        return;
      }
      await active.store.discardCurrent(active.run.id);
    }
    await this.refresh();
  }

  private async resume(): Promise<void> {
    const root = this.requireWorkspaceRoot();
    const store = new RunStore(root);
    if (await store.loadCurrent()) {
      throw new Error("Stop the active AHEAD run before resuming another one");
    }
    const runs = await Promise.all(
      (await store.listRunIds()).map(async (id) => ({ id, run: await store.load(id) })),
    );
    const unfinished = runs.filter(
      ({ run }) => !run.events.some((event) => event.type === "run_closed"),
    );
    const selected = await vscode.window.showQuickPick(
      unfinished.map(({ id, run }) => ({
        label: run.title,
        description: `${run.workflow_id} · ${id}`,
        id,
      })),
      { title: "Resume saved AHEAD work" },
    );
    if (!selected) {
      return;
    }
    await store.resume(selected.id);
    await this.refresh();
  }

  private async openUrl(value?: unknown): Promise<void> {
    if (typeof value === "string") {
      await vscode.env.openExternal(vscode.Uri.parse(value));
    }
  }

  private async openArtifact(value?: unknown): Promise<void> {
    if (typeof value !== "string") {
      return;
    }
    const active = await this.activeRun();
    const document = await vscode.workspace.openTextDocument(
      vscode.Uri.file(join(active.root, value)),
    );
    await vscode.window.showTextDocument(document, { preview: true });
  }

  private async humanArtifactTemplate(active: ActiveRun, artifact: ArtifactState): Promise<string> {
    if (artifact.kind === "review-disposition") {
      const review = active.state.artifacts.find((candidate) => candidate.kind === "ai-review");
      if (!review?.path) {
        throw new Error("The AI review must be recorded before human disposition");
      }
      const content = await active.store.readArtifact(review.path);
      return reviewDispositionTemplate(
        await collectReviewSnapshot(active.root),
        extractFindingIds(content),
      ).replace(
        "\n\nAHEAD-Review-Snapshot:",
        "\n\nArtifact: review-disposition\n\nAHEAD-Review-Snapshot:",
      );
    }
    const template = buildArtifactTemplate(active.run, active.state, artifact.kind, artifact.title);
    if (artifact.kind !== "human-review") {
      return template;
    }
    const snapshot = await collectReviewSnapshot(active.root);
    return `${template}\nAHEAD-Review-Snapshot: ${snapshot.fingerprint}\n`;
  }

  private async validateHumanReview(
    active: ActiveRun,
    kind: string,
    content: string,
  ): Promise<void> {
    if (kind !== "review-disposition" && kind !== "human-review") {
      return;
    }
    const snapshot = await collectReviewSnapshot(active.root);
    if (kind === "human-review") {
      if (extractReviewFingerprint(content) !== snapshot.fingerprint) {
        throw new Error("The human review must identify the exact current review snapshot");
      }
      return;
    }
    const review = active.state.artifacts.find((artifact) => artifact.kind === "ai-review");
    if (!review?.path) {
      throw new Error("The AI review artifact is missing");
    }
    const aiReview = await active.store.readArtifact(review.path);
    if (extractReviewFingerprint(aiReview) !== snapshot.fingerprint) {
      throw new Error("The changeset changed after AI review; return to implementation");
    }
    const errors = validateReviewDisposition(
      content,
      snapshot.fingerprint,
      extractFindingIds(aiReview),
    );
    if (errors.length) {
      throw new Error(errors.join("; "));
    }
  }

  private async pickArtifact(artifacts: ArtifactState[]): Promise<string | undefined> {
    const selected = await vscode.window.showQuickPick(
      artifacts.map((artifact) => ({ label: artifact.title, description: artifact.kind })),
      { title: "Write which human-owned AHEAD record?" },
    );
    return selected?.description;
  }

  private humanActionCommand(state: RunState): string {
    if (
      state.artifacts.some(
        (artifact) => artifact.required && !artifact.present && artifact.actor !== "ai",
      )
    ) {
      return "ahead.recordHumanArtifact";
    }
    if (state.work_item_required_for_next_phase) {
      return "ahead.linkWorkItem";
    }
    return "ahead.acceptAndAdvance";
  }

  private workItemDescription(active: ActiveRun): string {
    const boundary = active.state.policy.work_items.required_before_phase;
    return boundary ? `required before ${boundary}` : "optional";
  }

  private workspaceRoot(): string | undefined {
    const activeFolder = vscode.window.activeTextEditor
      ? vscode.workspace.getWorkspaceFolder(vscode.window.activeTextEditor.document.uri)
      : undefined;
    const folder = activeFolder ?? vscode.workspace.workspaceFolders?.[0];
    return folder ? projectRoot(folder.uri.fsPath) : undefined;
  }

  private requireWorkspaceRoot(): string {
    const root = this.workspaceRoot();
    if (!root) {
      throw new Error("Open a project folder before using AHEAD");
    }
    return root;
  }

  private async optionalActiveRun(): Promise<ActiveRun | undefined> {
    const root = this.workspaceRoot();
    if (!root) {
      return undefined;
    }
    const store = new RunStore(root);
    const run = await store.loadCurrent();
    if (!run) {
      return undefined;
    }
    const engine = await this.engine;
    return {
      root,
      store,
      run,
      state: engine.deriveState(run),
      workflow: engine.getWorkflow(run.workflow_id),
    };
  }

  private async activeRun(): Promise<ActiveRun> {
    const active = await this.optionalActiveRun();
    if (!active) {
      throw new AheadEngineError("no_active_run", "No active AHEAD run; start or resume one first");
    }
    return active;
  }
}

class AheadTreeProvider implements vscode.TreeDataProvider<ViewRow> {
  readonly onDidChangeTreeData: vscode.Event<void>;

  constructor(private readonly controller: AheadController) {
    this.onDidChangeTreeData = controller.changeEvent();
  }

  getTreeItem(row: ViewRow): vscode.TreeItem {
    const item = new vscode.TreeItem(
      row.label,
      row.children?.length
        ? vscode.TreeItemCollapsibleState.Expanded
        : vscode.TreeItemCollapsibleState.None,
    );
    item.id = `${row.label}:${row.description ?? ""}`;
    item.description = row.description;
    item.tooltip = row.tooltip ?? row.description;
    item.iconPath = row.icon ? new vscode.ThemeIcon(row.icon) : undefined;
    if (row.command) {
      item.command = {
        command: row.command,
        title: row.label,
        arguments: row.arguments,
      };
    }
    return item;
  }

  getChildren(row?: ViewRow): vscode.ProviderResult<ViewRow[]> {
    return row?.children ?? this.controller.viewRows();
  }
}

class ReadTool implements vscode.LanguageModelTool<EmptyInput> {
  constructor(private readonly read: () => Promise<unknown>) {}

  async invoke(): Promise<vscode.LanguageModelToolResult> {
    return jsonToolResult(await this.read());
  }
}

class ReferenceTool implements vscode.LanguageModelTool<ReferenceInput> {
  constructor(private readonly read: (topic?: string) => Promise<unknown>) {}

  async invoke(
    options: vscode.LanguageModelToolInvocationOptions<ReferenceInput>,
  ): Promise<vscode.LanguageModelToolResult> {
    return jsonToolResult(await this.read(options.input.topic));
  }
}

class RecordTool implements vscode.LanguageModelTool<RecordArtifactInput> {
  constructor(private readonly controller: AheadController) {}

  prepareInvocation(
    options: vscode.LanguageModelToolInvocationPrepareOptions<RecordArtifactInput>,
  ): vscode.ProviderResult<vscode.PreparedToolInvocation> {
    return {
      invocationMessage: `Recording AHEAD artifact ${options.input.kind}`,
      confirmationMessages: {
        title: `Record AI artifact ${options.input.kind}?`,
        message: new vscode.MarkdownString(
          "This writes a workflow artifact under `.ahead/`. It does not accept or advance the human gate.",
        ),
      },
    };
  }

  async invoke(
    options: vscode.LanguageModelToolInvocationOptions<RecordArtifactInput>,
  ): Promise<vscode.LanguageModelToolResult> {
    return jsonToolResult(await this.controller.recordAiArtifact(options.input));
  }
}

function jsonToolResult(value: unknown): vscode.LanguageModelToolResult {
  return new vscode.LanguageModelToolResult([
    new vscode.LanguageModelTextPart(JSON.stringify(value, null, 2)),
  ]);
}

function artifactOwner(artifact: ArtifactState): string {
  return artifact.actor === "human" ? "you" : artifact.actor === "ai" ? "AI" : "you/AI";
}

function workItemFromUrl(value: string): WorkItem {
  const url = new URL(value);
  const match =
    url.hostname === "github.com"
      ? url.pathname.match(/^\/[^/]+\/[^/]+\/issues\/(\d+)/)
      : undefined;
  return {
    provider: match ? "github" : url.hostname,
    url: url.toString(),
    ...(match ? { external_id: match[1] } : {}),
  };
}

function optionalUrlIssue(value: string): string | undefined {
  return value.trim() ? requiredUrlIssue(value) : undefined;
}

function requiredUrlIssue(value: string): string | undefined {
  try {
    const url = new URL(value);
    return url.protocol === "https:" || url.protocol === "http:"
      ? undefined
      : "Use an HTTP or HTTPS work-item URL";
  } catch {
    return "Enter a complete work-item URL";
  }
}

function emptyProjectConfig(): AheadProjectConfig {
  return {
    api_version: "ahead.config/v0",
    work_items: { required_before_phase: {} },
  };
}

function escapePattern(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function titleExample(workflowId: string): string {
  switch (workflowId) {
    case "product-change":
      return '"Add audit log viewer page"';
    case "internal-improvement":
      return '"Reduce cold-start time"';
    case "corrective-debugging":
      return '"Fix race in worker claim loop"';
    case "operational-stabilization":
      return '"Restore queue throughput after incident"';
    case "decision":
      return '"Choose audit-log retention policy"';
    case "investigation":
      return '"Why are runs flaking on CI?"';
    default:
      return '"Improve X"';
  }
}
