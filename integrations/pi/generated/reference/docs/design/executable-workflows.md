# Executable AHEAD Workflows

Status: six-flow executable dogfood v0.1

## Purpose

The executable layer makes AHEAD workflow state durable and makes selected human/AI boundaries enforceable across integrations. It does not turn judgment into a checklist or make workflow artifacts proof of understanding.

The executable layer implements all six pilot workflows. They share a versioned event model and enforcement engine while retaining workflow-specific phases, artifacts, gates, returns, AI capabilities, and generated instructions.

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
| Human-readable flows | `docs/workflows/*.md` |
| Current executable phases, artifacts, gates, transitions, and capabilities | `spec/workflows/*.json` |
| Published historical workflow definitions retained for replay | `spec/workflows/legacy/*.json` |
| Compact binding agent profile and shared AI behavior | `policy/common.md` |
| Phase AI behavior | `policy/<workflow>/*.md` with shared engineering-tail fragments in `policy/shared/*.md` |
| Reusable phase practices | `policy/methods/*.md`, selected by `policy/methods/index.json` |
| Reviewed optional skills | `recommendations/skills-v0.1.json` |
| State transition enforcement | `crates/ahead-core` |
| Host mapping, storage, and UI | `integrations/pi` |

Generated integration instructions are build artifacts. They include the compact agent profile, active phase policy, enforced contract, workflow version, and a source hash and must not be edited directly.

The engine retains published historical workflow definitions for replay while new runs use the current definition. Product Change `0.1.0` remains embedded for existing runs; the review and audit disposition contract is Product Change `0.2.0`. A run is always replayed against the version recorded when it started rather than silently reinterpreted under the newest workflow.

The Pi package also copies the canonical Constitution and `docs/**/*.md` into a generated reference catalog. These full documents are not injected into every prompt. The adapter recommends references applicable to the active phase, lets humans read them through `/ahead-guide`, and lets AI retrieve a specific source through `ahead_get_reference`. This keeps the binding prompt small while making the framework, rationale, evidence, and original page-level provenance available on demand.

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
- Workflow-specific human-first artifacts unlock AI assistance only after the human's initial model, option, plan, baseline, or other required reasoning exists.
- Required current-visit artifacts must exist before gate acceptance.
- Advancement requires the current human gate.
- Independent human review must be recorded by an identity other than the latest changeset implementer, and that reviewer must accept the review gate.
- Lasting-change flows distinguish implementation, AI review, independent human review, deployment, observation, audit, and human outcome.
- AI review is bound to a fingerprint of the exact current engineering changeset. The implementing human records a separate disposition for every material AI finding before independent human review.
- AI-audit findings and their human disposition are separate required records, and the human disposer personally accepts the audit gate.
- Operational Stabilization permits investigation and recovery work to proceed without proven root cause, but never grants AI the `execute` capability for the intervention or its execution phase.
- Decision and Investigation close with human-owned records and do not silently authorize downstream implementation.
- Model-invoked host tools require an explicit adapter mapping to an allowed canonical capability.

Instructions explain these boundaries to the model. The Rust core enforces the transition, actor, artifact, identity, and capability decisions even if instructions are ignored.

During implementation, instructions explicitly permit questions, explanation, debugging help, and bounded suggestions while keeping the engineer first. If the engineer has not supplied a current model, attempted approach, or intended behavior, AI asks for it before proposing a solution. A request for help does not authorize AI to take over the implementation.

## Trust boundaries and limits

The Pi adapter is an engineering workflow control, not a security sandbox.

- Local human identity comes from `AHEAD_HUMAN_IDENTITY`, then Git email/name, then the local user. It is self-attested. GitHub review identity and protected-branch rules will provide a stronger boundary later.
- Pi's direct `!` shell is a human action and is not intercepted. Model-invoked `bash` is intercepted.
- Unknown model tools are denied until the adapter classifies them. This prevents a newly installed effectful tool from silently acquiring authority.
- Artifact and run writes are atomic, but v0.1 has no multi-process lock. One writer should operate a run at a time.
- Workflow files can prove that a named action was recorded, not that a person genuinely understood it. Human review and organizational accountability remain necessary.
- No GitHub checks, PR gates, migration engine, signature scheme, or backwards-compatible workflow upgrade exists yet.
- Local review fingerprints detect changes but are not signatures. A future GitHub adapter must add remote identity and protected-branch evidence rather than treating the local record as cryptographic proof.

## Reuse path

The reusable boundary is the engine API, not a CLI. Pi is the first adapter. A VS Code extension, GitHub check, or future WASM-capable editor can reuse the same compiled core and canonical fragments while providing its own UI, storage transport, identity strength, and tool-capability map.

Review presentation follows the same rule. The core requires snapshot-bound findings, dispositions, and review gates; a host maps portable paths and locations to a terminal viewer, VS Code diff/comment UI, or GitHub review API. See `docs/design/review-workbench.md`.

Dogfooding should test whether the six encoded flows route real work correctly, whether their records and gates earn their cost, and whether phase visits, returns, capability vocabulary, and identity rules generalize across integrations.
