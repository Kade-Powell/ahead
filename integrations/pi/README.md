# AHEAD for Pi

Status: dogfood v0.1

The Pi integration runs the Rust AHEAD state machine as WebAssembly, injects generated phase instructions into any Pi model, persists the event/evidence chain, and exposes human gates through explicit slash commands.

It does not own model authentication. Use whichever provider Pi already supports and your organization permits, including an existing GitHub Copilot configuration. AHEAD never receives the model credential.

## Install from npm

After the first public release, install the current version globally in Pi:

```sh
pi install npm:ahead-pi
```

Pin an exact version for a team or project:

```sh
pi install -l npm:ahead-pi@0.1.0
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

## Dogfood loop

1. Start Pi in the repository where the engineering work will occur.
2. Run `/ahead-start <short title>`.
3. Run `/ahead-status` to see the active phase contract.
4. Use `/ahead-record [kind]` for human-owned artifacts.
5. Ask the model for only the assistance allowed in the active phase. It can call `ahead_get_context` and, where allowed, `ahead_record_artifact`.
6. Run `/ahead-accept` only after examining the evidence and owning the gate decision.
7. Run `/ahead-advance`, or `/ahead-return [phase]` with a reason when the work must reopen.
8. Commit appropriate `.ahead` records with the work so review and later adapters can validate them.

The footer and editor widget show the current phase, visit, gate, and first blocker.

## Human commands

| Command | Effect |
|---|---|
| `/ahead-start [title]` | Human starts and owns a Product Change run |
| `/ahead-status` | Show phase, artifacts, capabilities, gate, and blockers |
| `/ahead-record [kind]` | Human writes and records a permitted artifact |
| `/ahead-accept` | Human accepts the current gate after required evidence exists |
| `/ahead-advance` | Human advances, or closes the accepted final phase |
| `/ahead-return [phase]` | Human reopens an allowed earlier phase with a reason |
| `/ahead-help` | Show commands and authority boundaries |

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

This is local self-attestation, not cryptographic identity. The initial version is single-writer, implements only Product Change, and has no GitHub/CI workflow enforcement yet. See [Executable AHEAD workflows](https://github.com/Kade-Powell/ahead/blob/main/docs/design/executable-workflows.md) for the architecture and trust boundaries.

## Package and release verification

`npm test` builds the Rust core for `wasm32-unknown-unknown`, regenerates instructions, runs Rust/WASM-facing tests, creates the exact npm tarball, verifies its allowlisted contents, loads the extracted package through the real Pi binary, and confirms that `/ahead-start` persists a valid run.

The npm package contains only its README, package metadata, TypeScript runtime, generated phase instructions, and compiled WASM engine. Build scripts, tests, source specs, development dependencies, and repository files are excluded.

See [Releasing the Pi extension](https://github.com/Kade-Powell/ahead/blob/main/docs/releasing-pi.md) for versioning, first-publish authentication, trusted publishing, and rollback rules.
