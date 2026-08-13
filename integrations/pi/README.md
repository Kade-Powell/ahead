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
pi install -l npm:ahead-pi@0.3.1
```

Or try it for one session without changing settings:

```sh
pi -e npm:ahead-pi
```

## Guided mode

1. Start Pi in the repository where the engineering work will occur.
2. Run `/ahead <short title>` once, explicitly choose the workflow that fits the dominant outcome, and enter AHEAD mode. A new run never defaults to Product Change. Advanced noninteractive use must pass `/ahead-start <workflow-id> :: <title>`.
3. Work through normal conversation. A compact header above the chat shows the phase goal, required evidence, and whether the human or AI owns the next action. The full human/AI boundary continues to govern every model turn without occupying the conversation.
4. Run `/ahead` again only when you want the contextual action menu. It opens the right guided editor, requests the right AI contribution, accepts and advances a human gate, returns to an earlier phase, or opens applicable framework guidance.
5. Keep the `.ahead` records with the work so another session or independent reviewer resumes the same authoritative run.

AHEAD remains active across Pi sessions until an accountable human completes the outcome phase or runs `/ahead-stop`. Restarting Pi does not leave the mode or reset the workflow.

`/ahead-stop` defaults to discarding the unfinished AHEAD run record and its `.ahead` artifacts; it never deletes, resets, or reverts source code or other repository changes. The human may instead explicitly save the run for later. `/ahead-resume` restores that exact workflow, phase, evidence, and unmet gates.

During implementation, the engineer can ask questions at any time. The guided help form captures the engineer's current model and first attempt, then asks AI for explanation, evidence, hints, debugging help, or bounded next steps without handing over human ownership. Normal conversation follows the same rule. A bounded mechanical edit still requires clear human intent and later human inspection and understanding.

Run `/ahead-skills` to inspect optional third-party skills AHEAD has reviewed for the active phase. Recommendations pin the reviewed source and provide an opt-in install command; the extension never installs them. AHEAD's human ownership and gates override any conflicting skill guidance.

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

`/ahead-review` opens the first editor-neutral review workbench. It fingerprints the current Git changeset, shows the changed paths and diff in Pi, can open a selected path in VS Code when detected or configured with `AHEAD_EDITOR=vscode`, requests snapshot-bound AI findings, and opens the required human disposition or independent-review record. Any engineering change produces a new fingerprint and requires review again. `.ahead/**` evidence is excluded from that fingerprint so recording the review does not invalidate it.

## Human commands

| Command | Effect |
|---|---|
| `/ahead [title]` | Choose a workflow for new work, or open the active workflow's action menu |
| `/ahead-guide [topic]` | Read phase-relevant or requested AHEAD Markdown |
| `/ahead-skills` | Inspect optional skills reviewed for the active phase; never installs them |
| `/ahead-review` | Inspect the exact diff and complete the AI-to-human review handoff |
| `/ahead-stop` | Leave AHEAD mode; discard the unfinished record by default or explicitly save it |
| `/ahead-resume [run-id]` | Resume an unfinished run that was explicitly saved |
| `/ahead-help` | Show commands and authority boundaries |

`/ahead-start`, `/ahead-status`, `/ahead-record`, `/ahead-accept`, `/ahead-advance`, and `/ahead-return` remain available as advanced recovery and inspection commands. Normal use should not require memorizing them.

## AI tools

- `ahead_get_context` reads authoritative state.
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
