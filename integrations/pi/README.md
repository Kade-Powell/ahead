# AHEAD for Pi

Audience: AHEAD practitioners

Status: guided-mode dogfood v0.3

The Pi integration runs the Rust AHEAD state machine as WebAssembly, injects generated phase instructions into any Pi model, persists the event/evidence chain, and presents AHEAD as a guided mode with human-owned gates.

It does not own model authentication. Use whichever provider Pi already supports and your organization permits, including an existing GitHub Copilot configuration. AHEAD never receives the model credential.

## Install from npm

Install the current version globally in Pi:

```sh
pi install npm:ahead-pi
```

Pin an exact version for a team or project:

```sh
pi install -l npm:ahead-pi@0.5.0
```

Or try it for one session without changing settings:

```sh
pi -e npm:ahead-pi
```

## Install with oh-my-pi

[oh-my-pi (OMP)](https://github.com/can1357/oh-my-pi) can load Pi-compatible extension packages. `ahead-pi` declares its extension entry point in the `omp` and legacy `pi` package manifests, so install it through OMP's plugin manager:

```sh
omp install ahead-pi
```

Restart OMP after installing the package, then start a session in the repository where the engineering work will occur. Use the same AHEAD commands as in Pi, starting with:

```text
/ahead <short title>
```

Pin a version with the normal OMP package syntax:

```sh
omp install ahead-pi@0.5.0
```

Do not pass the npm package name to `omp -e`. In OMP, `-e` / `--extension` is a direct filesystem loader for local extension files or directories, not a remote npm-package loader. For a local checkout, use the extension entry file directly:

```sh
omp -e ./integrations/pi/src/index.ts
```

OMP loads extensions in-process and uses its Pi compatibility remapping for the Pi package imports used by AHEAD. If the package installs but `/ahead` is missing, inspect OMP's extension-load log:

```sh
tail -F ~/.omp/logs/omp.$(date +%F).log
```

Errors mentioning `legacy-pi-coding-agent-shim`, `@oh-my-pi/pi-coding-agent`, or a `/$bunfs/` path indicate an OMP loader/runtime compatibility issue rather than an AHEAD command or configuration problem. Check [OMP issue #2166](https://github.com/can1357/oh-my-pi/issues/2166) and report the exact error with the OMP version and platform. The OMP binary's extension loader and compatibility layer are maintained by OMP, not by AHEAD.

The bundled AHEAD extension commands and workflow state work through OMP. AHEAD's Pi-specific Agent Skills are currently exposed through Pi's `pi.skills` manifest; use the extension commands and `/ahead-guide` in OMP, and do not assume `/skill:research`, `/skill:to-tickets`, or `/skill:diagnosing-bugs` are available unless OMP's skill discovery lists them.

## Guided mode

1. Start Pi in the repository where the engineering work will occur.
2. Run `/ahead <short title>` or `/ahead <work-item-url>` once, explicitly choose the workflow that fits the dominant outcome, and enter AHEAD mode. A new run never defaults to Product Change. Advanced noninteractive use passes `/ahead-start <workflow-id> :: <title-or-work-item-url>`.
3. Work through normal conversation. A compact header above the chat shows the phase goal, required evidence, and whether the human or AI owns the next action. The full human/AI boundary continues to govern every model turn without occupying the conversation.
4. Run `/ahead` again only when you want the contextual action menu. It opens the right guided editor, requests the right AI contribution, accepts and advances a human gate, returns to an earlier phase, or opens applicable framework guidance.

Human-owned artifact editors use structured Markdown forms. Every expected input has a required response field, and an incomplete or removed field blocks saving with a specific list of what remains. A field may say `Not applicable — reason` when justified. Completing the form makes omissions visible; it does not substitute for accountable judgment or gate review.
5. Keep the `.ahead` records with the work so another session or independent reviewer resumes the same authoritative run.

AHEAD remains active across Pi sessions until an accountable human completes the outcome phase or runs `/ahead-stop`. Restarting Pi does not leave the mode or reset the workflow.

`/ahead-stop` defaults to discarding the unfinished AHEAD run record and its `.ahead` artifacts; it never deletes, resets, or reverts source code or other repository changes. The human may instead explicitly save the run for later. `/ahead-resume` restores that exact workflow, phase, evidence, and unmet gates.

During implementation, the engineer can ask questions at any time. The guided help form captures the engineer's current model and first attempt, then asks AI for explanation, evidence, hints, debugging help, or bounded next steps without handing over human ownership. Normal conversation follows the same rule. A bounded mechanical edit still requires clear human intent and later human inspection and understanding.

Run `/ahead-skills` to inspect optional third-party skills AHEAD has reviewed for the active phase. Recommendations pin the reviewed source and provide an opt-in install command; the extension never installs them. AHEAD's human ownership and gates override any conflicting skill guidance.

The package itself includes three AHEAD-owned Agent Skills: `/skill:research`, `/skill:to-tickets`, and `/skill:diagnosing-bugs`. Pi loads their descriptions at startup and their full instructions only when selected. They are thin interfaces to AHEAD's canonical methods, not extra workflow authority. `to-tickets` requires explicit invocation and human approval of the complete breakdown before any tracker write. `diagnosing-bugs` uses AHEAD's human-model-first order. Projects should keep domain language, tracker configuration, and repository-specific review rules in their own instructions rather than copying these skills locally.

## Work items and sprint-ahead planning

`/ahead-work-item <url>` links an existing GitHub, Jira, Azure Boards, Linear, or other HTTP(S) work item. With no URL, the command can link an item interactively or create a GitHub issue in the current repository after the human reviews and confirms a body seeded with the available approved plan. The provider-neutral URL appears in the AHEAD header and run state.

Repositories may configure a required boundary in `.ahead/config.json`. This example requires a work item before Product Change enters implementation:

```json
{
  "api_version": "ahead.config/v0",
  "work_items": {
    "required_before_phase": {
      "product-change": "implement"
    }
  }
}
```

When a project has no config, starting new work offers a setup wizard or the option to continue without one. `/ahead-config` opens the wizard directly. It can apply the recommended boundaries, ask for each workflow's boundary, or make all work items optional. Rerunning it previews and confirms replacement, then preserves the exact prior file under `.ahead/backups/`; this also provides a safe path from an invalid or unsupported config version to the current schema.

The boundary is configured separately for each workflow and copied into new runs for deterministic replay. Configuration changes never reinterpret active or saved runs. When an approved plan enters `implement`, `/ahead` offers to save it as a ready-to-implement handoff for a later sprint. This preserves the plan and work-item link without claiming the unfinished workflow is complete.

## Agent profile and framework references

AHEAD behaves as a dynamic policy profile layered onto whichever model Pi is already using. Every model turn receives a compact binding agent profile, the active phase contract, current workflow state, and human/AI boundary. The full framework is not injected into every prompt.

The Constitution, practitioner guide, and evidence library are copied into the published package at build time and indexed with audience and authority metadata. Maintainer and tooling-development documents are excluded. Run `/ahead-guide` to read practitioner references applicable to the active phase, `/ahead-guide all` to browse the complete runtime set, or `/ahead-guide <topic>` to open a specific document. AI uses `ahead_get_reference` when it needs the same source material. This preserves traceability without filling the context window with unrelated documents.

The normal implementation handoff is:

```text
HUMAN IMPLEMENTS AND SELF-CHECKS
              ↓
AI REVIEWS THE EXACT CURRENT CHANGESET
              ↓
HUMAN RECORDS A SEPARATE DISPOSITION FOR EVERY MATERIAL AI FINDING
              ↓
READY FOR INDEPENDENT HUMAN REVIEW
              ↓
INDEPENDENT HUMAN REVIEWS AND ACCEPTS
```

A draft branch or draft PR may exist earlier. The handoff gate is requesting human review or marking the PR ready, not ordinary draft pushes.

`/ahead-review` opens the first editor-neutral review workbench. It fingerprints the current Git changeset, shows the changed paths and diff in Pi, can open a selected path in VS Code or Zed when detected or configured with `AHEAD_EDITOR=vscode` or `AHEAD_EDITOR=zed`, requests snapshot-bound AI findings, and opens the required human disposition or independent-review record. Any engineering change produces a new fingerprint and requires review again. `.ahead/**` evidence is excluded from that fingerprint so recording the review does not invalidate it.

VS Code is detected from its terminal environment. For a deterministic Zed terminal, add `"AHEAD_EDITOR": "zed"` under Zed's `terminal.env` settings; Zed's CLI must be installed. `AHEAD_EDITOR=none` disables external opening.

Human-owned plans and other artifacts open in that detected editor as a temporary Markdown draft. Save your work, then close only that temporary tab; AHEAD copies the draft into `.ahead/runs/` after the tab closes and the form passes validation. The host editor stays open, and saving alone does not record the artifact.

## Human commands

| Command | Effect |
|---|---|
| `/ahead [title]` | Choose a workflow for new work, or open the active workflow's action menu |
| `/ahead-work-item [url]` | Link a provider-neutral work item or create a confirmed GitHub issue |
| `/ahead-config` | Set up, inspect, replace, or migrate project policy with a recoverable backup |
| `/ahead-guide [topic]` | Read phase-relevant or requested AHEAD Markdown |
| `/ahead-skills` | Inspect optional skills reviewed for the active phase; never installs them |
| `/ahead-review` | Inspect the exact diff and complete the AI-to-human review handoff |
| `/ahead-stop` | Leave AHEAD mode; discard the unfinished record by default or explicitly save it |
| `/ahead-resume [run-id]` | Resume an unfinished run that was explicitly saved |
| `/ahead-help` | Show commands and authority boundaries |

`/ahead-start`, `/ahead-status`, `/ahead-record`, `/ahead-accept`, `/ahead-advance`, and `/ahead-return` remain available as advanced recovery and inspection commands. Normal use should not require memorizing them.

## AI tools

- `ahead_get_context` reads authoritative state.
- `ahead_get_work_item` reads the linked reference and resolves GitHub issue context through `gh`.
- `ahead_get_reference` lists or reads the packaged framework Markdown on demand.
- `ahead_get_recommended_skills` lists reviewed optional skills without installing them.
- `ahead_get_review_snapshot` captures the exact current changeset and fingerprint.
- `ahead_record_artifact` records only AI/shared artifacts allowed in the current phase.
- `ahead_request_transition` reports readiness but cannot change state.
- `ahead_validate` replays the event log.

Built-in Pi tool mappings are deliberately explicit:

| Pi tool | AHEAD capability |
|---|---|
| `read`, `grep`, `find`, `ls` | `inspect` |
| `edit`, `write` | `modify` |
| `bash` | `execute` |

An unknown model-invoked tool is denied until this adapter classifies it. Human `!` shell commands are not model tool calls and are not intercepted.

## Identity and current limits

Human identity is resolved from `AHEAD_HUMAN_IDENTITY`, Git `user.email`, Git `user.name`, then the local username. Set the environment variable when another reviewer uses the same machine or Git identity:

```sh
AHEAD_HUMAN_IDENTITY=reviewer@example.com pi -e ./integrations/pi/src/index.ts
```

This is local self-attestation, not cryptographic identity. The initial version is single-writer, implements all six pilot workflows, and has no GitHub/CI workflow enforcement yet. v0.3 binds local review records to a SHA-256 fingerprint of the selected base, merge base, HEAD, tracked diff, working-tree status, and untracked-file hashes. That fingerprint is not signed or remotely attested; a changed changeset must be reviewed again.

## Contributing

Source builds, architecture, package verification, and release procedures are maintained separately in the [AHEAD development guide](https://github.com/Kade-Powell/ahead/blob/main/docs/development/README.md).
