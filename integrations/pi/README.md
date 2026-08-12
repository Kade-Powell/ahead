# AHEAD for Pi

Status: guided-mode dogfood v0.2

The Pi integration runs the Rust AHEAD state machine as WebAssembly, injects generated phase instructions into any Pi model, persists the event/evidence chain, and presents AHEAD as a guided mode with human-owned gates.

It does not own model authentication. Use whichever provider Pi already supports and your organization permits, including an existing GitHub Copilot configuration. AHEAD never receives the model credential.

## Install from npm

Install the current version globally in Pi:

```sh
pi install npm:ahead-pi
```

Pin an exact version for a team or project:

```sh
pi install -l npm:ahead-pi@0.2.0
```

Or try it for one session without changing settings:

```sh
pi -e npm:ahead-pi
```

## Build from this repository

Requirements: Rust with `wasm32-unknown-unknown`, Node 22 or newer, npm, and Pi 0.80.6 or a compatible release.

```sh
cd integrations/pi
npm install
npm run build
```

For a quick source-checkout dogfood run from the repository root:

```sh
pi --no-extensions -e ./integrations/pi/src/index.ts
```

Or install the package into the current project through Pi:

```sh
pi install -l ./integrations/pi
pi
```

Pi may ask you to trust the project-local extension. Review it before accepting; project trust is not a sandbox.

## Guided mode

1. Start Pi in the repository where the engineering work will occur.
2. Run `/ahead <short title>` once to enter AHEAD mode and start a Product Change run.
3. Work through normal conversation. The persistent widget shows the current goal, what the human owns, what AI may do, required evidence, and the next valid action.
4. Run `/ahead` again whenever you want the contextual action menu. It opens the right guided editor, requests the right AI contribution, accepts and advances a human gate, or returns to an earlier phase.
5. Keep the `.ahead` records with the work so another session or independent reviewer resumes the same authoritative run.

AHEAD remains active across Pi sessions until an accountable human completes the outcome phase and closes the run. Restarting Pi does not leave the mode or reset the workflow.

The normal implementation handoff is:

```text
HUMAN IMPLEMENTS AND SELF-CHECKS
              ↓
AI REVIEWS THE EXACT CURRENT CHANGESET
              ↓
HUMAN DISPOSES MATERIAL AI FINDINGS
              ↓
READY FOR INDEPENDENT HUMAN REVIEW
              ↓
INDEPENDENT HUMAN REVIEWS AND ACCEPTS
```

A draft branch or draft PR may exist earlier. The handoff gate is requesting human review or marking the PR ready, not ordinary draft pushes.

## Human commands

| Command | Effect |
|---|---|
| `/ahead [title]` | Enter, resume, or perform the next guided AHEAD action |
| `/ahead-help` | Show commands and authority boundaries |

`/ahead-start`, `/ahead-status`, `/ahead-record`, `/ahead-accept`, `/ahead-advance`, and `/ahead-return` remain available as advanced recovery and inspection commands. Normal use should not require memorizing them.

## AI tools

- `ahead_get_context` reads authoritative state.
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

This is local self-attestation, not cryptographic identity. The initial version is single-writer, implements only Product Change, and has no GitHub/CI workflow enforcement yet. Review records ask for the exact commit or diff, but v0.2 does not yet cryptographically bind that changeset to the review; a changed changeset must be returned and reviewed again by the humans involved. See [Executable AHEAD workflows](https://github.com/Kade-Powell/ahead/blob/main/docs/design/executable-workflows.md) for the architecture and trust boundaries.

## Package and release verification

`npm test` builds the Rust core for `wasm32-unknown-unknown`, regenerates instructions, runs Rust/WASM-facing and guided-mode tests, creates the exact npm tarball, verifies its allowlisted contents, loads the extracted package through the real Pi binary, and confirms that the packaged extension persists a valid run.

The npm package contains only its README, package metadata, TypeScript runtime, generated phase instructions, and compiled WASM engine. Build scripts, tests, source specs, development dependencies, and repository files are excluded.

See [Releasing the Pi extension](https://github.com/Kade-Powell/ahead/blob/main/docs/releasing-pi.md) for versioning, first-publish authentication, trusted publishing, and rollback rules.
