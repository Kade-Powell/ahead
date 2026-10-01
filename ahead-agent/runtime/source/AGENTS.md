# Distilled Codex runtime guidance

This file is adapted from the OpenAI Codex CLI `AGENTS.md` at upstream commit
`316795b3cf2a45e90d121d9f46499d4658b2645c`, with AHEAD's hard-fork seed
changes from commit `7c07b2f88aceedb4ed187a5fafa145044660d07a`. It applies only
to the embedded runtime under `ahead-agent/runtime/source/`; the repository-root
`AGENTS.md` and AHEAD host policy still apply and win on conflicts.

## Runtime boundary

- This source is a deliberately trimmed native runtime in AHEAD's one Cargo
  workspace. Use AHEAD's Cargo/CI commands; Bazel, the Codex TUI, the Codex App
  Server executable, cloud products and release tooling are not part of this
  distilled tree.
- Preserve the model-facing contract: tool names and descriptions, argument
  schemas, result/error shapes, instruction layering, tool-call ordering,
  compaction, streaming, cancellation and model-specific tool selection.
- The managed path is hosted directly by `ahead-agent`. External ACP remains a
  compatibility boundary and does not carry AHEAD's effect or attribution
  guarantees.
- Do not reintroduce generated App Server schemas, a second server hop or
  removed product surfaces without a new AHEAD architecture decision.
- Do not modify `CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR` or
  `CODEX_SANDBOX_ENV_VAR` behavior to make tests pass. Those variables are part
  of the sandbox contract and tests may skip when the host cannot provide the
  required sandbox.

## Rust rules

- Inline `format!` arguments when possible; collapse nested `if` statements and
  use method references over redundant closures.
- Prefer exhaustive `match` statements. Keep public APIs small and document
  new traits with their implementation contract.
- Avoid ambiguous positional booleans and `Option` values in new APIs. Prefer
  enums, named methods or newtypes when they make call sites self-documenting.
- Prefer native async traits with explicit `Send` future bounds over
  `#[async_trait]` or `#[allow(async_fn_in_trait)]`.
- Do not add a one-use helper or grow a central module when a focused existing
  module or a small new module is clearer. Resist adding unrelated behavior to
  `codex-core`.
- Keep fallible operations visible and propagated. Never weaken sandbox,
  path-validation, authorization or persistence errors for convenience.

## Model-visible context and compatibility

- Build context incrementally; do not rewrite history or repeatedly perturb
  stable context in ways that cause cache misses.
- Every injected context fragment needs a bounded size and a hard cap. No item
  may exceed 10,000 tokens; flag a new individual item that can exceed 1,000
  tokens for manual review.
- Treat native tool contracts, `ahead-rpc` DTOs, configuration loading,
  persisted session/rollout data, ACP messages and resume behavior as external
  integration surfaces. Search all callers and compatibility readers before
  changing them.

## Tests and commands

- Prefer integration coverage for agent behavior. Put new multi-case unit tests
  in a sibling `*_tests.rs` file and reuse existing runtime test helpers.
- Test observable behavior, not statically defined constants. Prefer complete
  object equality over field-by-field assertions where practical, and avoid
  mutating process-global environment in tests.
- Run `cargo fmt --all -- --check` and the narrowest affected locked test or
  check, for example `cargo test --locked -p ahead-agent --lib`. The full
  workspace is expensive in this shared target; use it deliberately and report
  the exact scope validated.
- Do not kill a slow Rust build by PID. Cargo lock contention and native
  runtime compilation can be slow; stop only through the owning command/session.

## Scoped maintenance skills

Reusable upstream Codex runtime-maintenance procedures are workspace skills
under the repository root `.agents/skills/ahead-agent-*` directories. They
apply only when editing the AHEAD agent/runtime and cannot grant tools,
permissions, edits or publication authority.
