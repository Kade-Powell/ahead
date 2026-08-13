# Pi Integration Development

Audience: AHEAD framework and tooling maintainers

The published Pi README is practitioner-facing. This document contains the source-build, validation, and packaging information needed to change the adapter.

## Build from this repository

Requirements: Rust with `wasm32-unknown-unknown`, Node 22 or newer, npm, and Pi 0.84.1 or a compatible release.

```sh
cd integrations/pi
npm install
npm run build
```

For a source-checkout dogfood run from the repository root:

```sh
pi --no-extensions -e ./integrations/pi/src/index.ts
```

Or install the working tree into the current project through Pi:

```sh
pi install -l ./integrations/pi
pi
```

Pi may ask the user to trust the project-local extension. Project trust is not a sandbox.

## Validation and package verification

`npm test` builds the Rust core for `wasm32-unknown-unknown`, regenerates instructions and references, checks formatting with Oxfmt, runs Oxlint source and type-aware analysis, runs TypeScript checking and the Rust/WASM-facing and guided-mode tests, creates the exact npm tarball, verifies its allowlisted contents, loads the extracted package through the real Pi binary, and confirms that the packaged extension persists a valid run.

Use `npm run format`, `npm run format:check`, `npm run lint`, and `npm run typecheck` for individual JavaScript and TypeScript quality gates. Oxc configuration is repository-wide so the root generators and Pi integration follow the same policy.

The npm package contains only its practitioner README, package metadata, TypeScript runtime, generated phase instructions, runtime reference catalog, and compiled WASM engine. Build scripts, tests, source specs, maintainer documents, development dependencies, and other repository files are excluded.
