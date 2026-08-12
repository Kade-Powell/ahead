# Executable AHEAD Workflows

Status: initial vertical slice v0.1

## Purpose

The executable layer makes AHEAD workflow state durable and makes selected human/AI boundaries enforceable across integrations. It does not turn judgment into a checklist or make workflow artifacts proof of understanding.

The first vertical slice implements the Product Change workflow. The other five pilot workflows remain manual profiles until dogfooding provides evidence about the reusable state model.

## Architecture

```text
CONSTITUTION / ACCEPTABLE-AI-USE
                 │
                 ▼
CANONICAL WORKFLOW SPEC + POLICY FRAGMENTS
                 │
        ┌────────┴────────┐
        ▼                 ▼
RUST WORKFLOW CORE   GENERATED INSTRUCTIONS
        │                 │
        └────────┬────────┘
                 ▼
         INTEGRATION ADAPTER
        (Pi first; others later)
                 │
        ┌────────┴─────────┐
        ▼                  ▼
HOST TOOLS / MODEL     DURABLE RUN FILES
```

The Rust core owns workflow semantics. It is deterministic and has no filesystem, network, clock, model-provider, or editor dependency. Hosts supply identity and timestamps, persist returned runs, and map their tools to canonical capabilities.

The initial WebAssembly boundary is a small versioned JSON ABI. This avoids coupling the core to one JavaScript binding generator and lets a later editor or CI integration load the same state machine.

## Sources of truth

| Concern | Canonical source |
|---|---|
| Durable principles | `CONSTITUTION.md` |
| AI authority | `docs/acceptable-ai-use.md` |
| Human-readable Product Change flow | `docs/workflows/product-change.md` |
| Executable phases, artifacts, gates, transitions, and capabilities | `spec/workflows/product-change-v0.1.json` |
| Shared AI behavior | `policy/common.md` |
| Phase AI behavior | `policy/product-change/*.md` |
| State transition enforcement | `crates/ahead-core` |
| Host mapping, storage, and UI | `integrations/pi` |

Generated integration instructions are build artifacts. They include the workflow version and a source hash and must not be edited directly.

## State and evidence

A run is an append-only event log. Events record an actor kind and identity, host-supplied timestamp, sequence, and one action:

- start the run;
- record an artifact;
- accept a human gate;
- advance or return between phases;
- close the run.

State is derived by replay. The core rejects invalid history rather than trusting a cached phase field. A return transition creates a new visit to the target phase. Earlier evidence remains in history, but only evidence recorded during the current visit satisfies its gate.

Pi stores state under the work's Git root:

```text
.ahead/
├── current.json
└── runs/
    └── <run-id>/
        ├── run.json
        └── artifacts/
            └── <sequence>-<phase>-<kind>.md
```

These are intended to be inspectable, diffable repository artifacts. A team can decide which records belong in Git, while CI and GitHub enforcement are later adapters over the same run contract.

## Enforced boundaries in v0.1

- Only a human actor can start a run, accept a gate, transition a phase, return work, or close a run.
- Artifact definitions state whether a human, AI, or either may record them.
- Human-first option and plan artifacts unlock AI assistance in those phases.
- Required current-visit artifacts must exist before gate acceptance.
- Advancement requires the current human gate.
- Independent human review must be recorded by an identity other than the latest changeset implementer, and that reviewer must accept the review gate.
- The Product Change flow distinguishes implementation, deployment, observation, audit, and human outcome.
- Model-invoked host tools require an explicit adapter mapping to an allowed canonical capability.

Instructions explain these boundaries to the model. The Rust core enforces the transition, actor, artifact, identity, and capability decisions even if instructions are ignored.

## Trust boundaries and limits

The Pi adapter is an engineering workflow control, not a security sandbox.

- Local human identity comes from `AHEAD_HUMAN_IDENTITY`, then Git email/name, then the local user. It is self-attested. GitHub review identity and protected-branch rules will provide a stronger boundary later.
- Pi's direct `!` shell is a human action and is not intercepted. Model-invoked `bash` is intercepted.
- Unknown model tools are denied until the adapter classifies them. This prevents a newly installed effectful tool from silently acquiring authority.
- Artifact and run writes are atomic, but v0.1 has no multi-process lock. One writer should operate a run at a time.
- Workflow files can prove that a named action was recorded, not that a person genuinely understood it. Human review and organizational accountability remain necessary.
- No GitHub checks, PR gates, migration engine, signature scheme, or backwards-compatible workflow upgrade exists yet.

## Reuse path

The reusable boundary is the engine API, not a CLI. Pi is the first adapter. A VS Code extension, GitHub check, or future WASM-capable editor can reuse the same compiled core and canonical fragments while providing its own UI, storage transport, identity strength, and tool-capability map.

Dogfooding should test whether phase visits, artifacts, gates, returns, capability vocabulary, and identity rules generalize before the remaining five workflow profiles are encoded.
