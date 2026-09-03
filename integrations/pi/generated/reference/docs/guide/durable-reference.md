# Durable References

Audience: AHEAD practitioners

Status: pilot v0.1

## Purpose

AHEAD's workflows produce two kinds of records. **Run artifacts** are scoped to a single run and describe what happened: framing, research, decisions, plans, changesets, review findings, outcomes. **Durable references** outlive a run and describe how the system *ought to be* going forward.

Durable references are where a project's decisions and design accrete. Without them, every run has to reconstruct context from code and memory. With them, each run inherits what earlier runs concluded and updates the record when it changes.

AHEAD standardizes three durable reference classes and explicitly does not standardize the rest. For the rest, use [Diátaxis](https://diataxis.fr/).

## The three durable classes

| Class | Also called | Diátaxis mode | Shape draws from |
|---|---|---|---|
| `decision-record` | ADR (Architecture Decision Record) | explanation with reference lookup | [MADR](https://adr.github.io/madr/) |
| `design-doc` | design document, architecture document | reference plus explanation, layered | [arc42](https://arc42.org/), [C4](https://c4model.com/) |
| `runbook` | ops runbook (Google SRE: playbook) | how-to | [Google SRE Book](https://sre.google/sre-book/) |

The three names are what practitioners and coding agents already grep for. AHEAD uses the same strings in workflow specifications (`spec/workflows/*.json` artifact `kind` values) so the guide and the machine contract match.

## Decision records

### What one is

A `decision-record` captures a consequential choice made by an accountable human, its context, the drivers that shaped it, the alternatives considered, the choice, and its expected consequences. It is the durable form of a Decision workflow's publish artifact.

Not every Decision workflow run produces one. Small, reversible decisions may live only in the run record. A decision-record is warranted when the choice will bind future work, when it forecloses alternatives, or when a future engineer or reviewer must be able to understand why the current shape exists.

### When to create or update

- Create at the `publish` phase of a Decision workflow.
- Create when a Product Change or Internal Improvement run makes a decision durable enough to bind future work; log the creation in that run's `reference-updates` artifact.
- Update the `status` and `superseded_by` fields when a later decision-record replaces this one. The prior record is not deleted.
- Update the `revisit` date when a scheduled review of the decision occurs.

Decision records are otherwise immutable. A changed conclusion is a new decision-record that supersedes the prior one.

### Shape

Follow [MADR](https://adr.github.io/madr/). Concretely:

- Filename `NNNN-title-with-dashes.md` where `NNNN` is a zero-padded consecutive number.
- Store in `docs/adr/` or `docs/architecture/adr/`. Pick one location per project and hold to it.
- YAML front matter:
  ```yaml
  ---
  type: decision-record
  status: proposed | accepted | superseded | deprecated
  date: YYYY-MM-DD
  deciders: [names or roles]
  consulted: []
  informed: []
  supersedes: [NNNN, ...]        # optional
  superseded_by: NNNN            # optional
  revisit: YYYY-MM-DD            # optional
  ---
  ```
- First heading `# ADR NNNN: <Short title>`.
- Required sections: `Context and Problem Statement`, `Decision Drivers`, `Considered Options`, `Decision Outcome`, `Consequences`.
- Optional sections: `Confirmation`, per-option `Pros and Cons`, `More Information`.

Keep the whole record to a small number of screens. Long context, exhaustive alternatives, or deep technical detail belongs in a linked design-doc or research artifact.

### Human-only and AI-assistable sections

AHEAD is human-led. AI may scaffold and question everywhere. The following table records which sections humans must own outright versus where AI may draft, propose, or maintain material for the human to accept.

| Section | Human owns | AI may |
|---|---|---|
| Context and Problem Statement | final wording | propose questions to sharpen framing |
| Decision Drivers | final list | propose candidate drivers for approval |
| Considered Options | first-pass options | expand alternatives, add missing options |
| Pros and Cons per option | curates | draft skeletons, cite evidence |
| **Decision Outcome** | **writes** | summarize *after* the human has written it |
| Consequences | owns | propose overlooked consequences |
| Confirmation | owns | propose validation checks |
| Front matter (`status`, `supersedes`, `superseded_by`, `revisit`) | approves each transition | maintain on approved state changes |

An AI that produces a Decision Outcome for the human to rubber-stamp violates the human ownership boundary. An AI that proposes a Decision Outcome after the human has written one, framed as a summary the human then accepts or discards, is assistance.

## Design docs

### What one is

A `design-doc` describes a capability, subsystem, or cross-cutting concept as it is intended to be — its contract, its shape, and the reasoning that produced that shape. It is a living document. Runs that alter its subject update it.

A design-doc is not a decision. It records the design that decisions have produced. Where a decision is contained, a design-doc is layered:

- **Reference layer** — components, contracts, interfaces, invariants, sequence, deployment. What the thing is. Kept austere and scannable.
- **Explanation layer** — why this shape, what tradeoffs, what constraints hold, what the design is *not*. Kept adjacent to but distinct from the reference layer.

Both layers are required. A design-doc that is only "what" rots into a stale catalog. A design-doc that is only "why" duplicates the decision records.

### When to create or update

- Create at the `plan` phase of a Product Change or Internal Improvement run when the capability has no design-doc yet.
- Update at `implement` when a run changes what the design-doc describes. The update lands in that run's `reference-updates` artifact.
- Update the reference layer when code changes shift the contract, module structure, or diagram-worthy topology.
- Update the explanation layer when a new decision changes the design's rationale — link the new decision-record.
- Update `last-updated` metadata on every change.

### Shape

AHEAD does not prescribe a rigid template. Draw on [arc42](https://arc42.org/) for section coverage and [C4](https://c4model.com/) for layered detail. A useful minimum:

- Filename descriptive: `docs/architecture/<capability>.md` or `docs/design/<capability>.md`. Pick one root per project.
- YAML front matter:
  ```yaml
  ---
  type: design-doc
  status: draft | current | deprecated
  last-updated: YYYY-MM-DD
  supersedes: <path>              # optional
  related-decisions: [NNNN, ...]  # ADR numbers
  related-runbooks: [<path>, ...] # optional
  ---
  ```
- First heading `# Design: <Capability>` or `# <Capability> — Design`.
- Reference section: overview, contract, components, interfaces, data, sequence and runtime behavior, deployment, invariants. Kept factual.
- Explanation section: rationale, tradeoffs, constraints, alternatives that were considered and rejected (link the decision-record), residual risks.
- Cross-reference table: linked decision-records and linked runbooks.

If a design-doc grows beyond what one screen can hold at each layer, split by C4 layer or by subsystem rather than mixing modes.

### Human-only and AI-assistable sections

| Section | Human owns | AI may |
|---|---|---|
| Overview of the capability | edits | draft from code inspection |
| Contract and API surface | approves | generate and update from code |
| Component and module reference | approves | generate and maintain |
| Diagrams derived from code | approves | generate and update |
| **Rationale — why this shape** | **writes** | pose sharpening questions |
| Tradeoffs and constraints | owns | pose questions, cite evidence |
| Cross-reference table | approves | maintain |
| Change log and `last-updated` metadata | approves | maintain |

AI edits to a design-doc are only made inside a run. Every AI-proposed change lands in that run's `reference-updates` artifact and is accepted or edited by the human before it is applied. AHEAD does not permit background AI edits to durable references.

## Runbooks

### What one is

A `runbook` is a procedure a human executes when a specific operational condition occurs — an alert, a symptom, a scheduled event, a deployment step. Written to be usable under pressure.

A runbook is a how-to guide, not a reference. Contextual explanation belongs in a linked design-doc; per-alert action steps belong here. Google SRE calls the same artifact a "playbook"; the artifact is the same shape.

Not every operational note is a runbook. Environment variable listings, resource limits, and alert catalogs are reference material and belong in a design-doc chapter.

### When to create or update

- Create when a Product Change introduces a new alert, deployment step, or operational condition that a human must handle.
- Update at the `implement` phase of any run that alters operational behavior. Log in `reference-updates`.
- Update after an Operational Stabilization run that revealed a diagnosis or mitigation not previously documented.
- Update after any postmortem that identifies missing or incorrect procedure. Link the postmortem.

### Shape

Draw on [Google SRE Book](https://sre.google/sre-book/) chapters on on-call and effective troubleshooting. A useful minimum:

- Filename `docs/runbooks/runbook-<symptom>.md` or `docs/operations/runbooks/runbook-<symptom>.md`.
- YAML front matter:
  ```yaml
  ---
  type: runbook
  status: draft | current | deprecated
  severity: [P1|P2|P3|P4]
  alert: <alert identifier or none>
  owning-team: <team>
  last-updated: YYYY-MM-DD
  related-designs: [<path>, ...]
  ---
  ```
- First heading `# Runbook: <Alert or symptom>`.
- Required sections: `Symptom`, `Severity and Impact`, `Diagnosis`, `Mitigation`, `Escalation`, `Rollback`, `Related`.

Lead with the symptom. Every diagnosis and mitigation step is executable — a command, a query, a specific console page, a specific decision. Prose belongs in the linked design-doc, not here.

### Human-only and AI-assistable sections

| Section | Human owns | AI may |
|---|---|---|
| Symptom and alert description | approves | draft from monitoring configuration |
| **Severity and Impact** | **writes** (operator judgment) | ask clarifying questions |
| Diagnosis steps | validates each step | propose steps from code and prior incidents |
| **Mitigation steps** | **explicitly approves each** | propose candidates |
| Escalation contacts | approves | maintain from team directory |
| Rollback procedure | owns | propose from the deployment path |
| Related design-docs and decisions | approves | maintain link table |
| Postmortem links | approves | maintain |

Mitigation is high-consequence. AI may propose mitigation steps; the human on the run must explicitly approve each. AI-only mitigation is out of bounds regardless of how obvious the mitigation appears.

## How runs interact with durable references

### The `reference-updates` artifact

Product Change, Internal Improvement, and Operational Stabilization workflows require a `reference-updates` artifact during both `plan` and `implement` phases. It names:

- every decision-record the run creates or updates (including status/supersession changes);
- every design-doc the run creates or updates;
- every runbook the run creates or updates; and
- an explicit "none" when the run intentionally updates no durable reference.

The `plan` phase records the *intended* updates. The `implement` phase records the *actual* updates. Deviations between the two are recorded in `plan-deviations` like any other plan deviation.

An AI review that sees a code change without a corresponding `reference-updates` entry flags it. This is the primary mechanism AHEAD uses to keep durable references from drifting behind behavior.

### Co-authoring during implement

An AHEAD run's session stays live from framing through outcome. During `implement`, a practitioner may ask the AI to:

- check whether an in-progress change is covered by the plan and flag gaps;
- draft the reference-layer sections of an affected design-doc based on the code change;
- draft diagnosis and mitigation candidates for a runbook affected by the change;
- log a plan-deviation as it occurs;
- answer questions with the full run context available (decision, plan, evidence, prior updates).

The human accepts, edits, or discards each AI-proposed update. The human-only sections named in the tables above are not drafted by AI on the human's behalf.

### Review with full run context

The run remains open through review. The AI review phase reads the entire live run — decision-record, plan, plan-deviations, all `reference-updates`, changeset, tests, and evidence — and reports findings against that whole context, not the changeset alone. Reviewer findings remain snapshot-bound to the exact changeset fingerprint.

The independent human reviewer opens the same live run. The reviewer may query the run interactively ("why did we deviate at step 4?", "which design-doc changed here?") and receive answers from the record. The independent-review requirement is unchanged: reviewer must not be the implementer.

The run closes only at the outcome gate, and only when the accountable human explicitly closes it. There is no automatic close after time or after review.

## What AHEAD does not standardize

Adopt [Diátaxis](https://diataxis.fr/) for these. Diátaxis distinguishes four modes — tutorial, how-to guide, reference, and explanation — by user need. Use its vocabulary to shape and separate:

- **Tutorials** — first-time learning experiences. Owned by the project.
- **How-to guides** not tied to an operational alert — for example, "how to add a new API endpoint" or "how to run the integration tests." Owned by the project.
- **User-facing product documentation** — everything the end user reads about the product. Owned by the project.
- **Contributor references** — repository layout, build system, lint matrix, local development setup. Owned by the project.
- **Coding-agent instructions** — project-specific AGENTS-file conventions and per-repo skill collections. Owned by the project. AHEAD provides only [recommended optional skills](recommended-skills.md).

AHEAD does not disclaim these because they are unimportant. It disclaims them because their shape is well established in the wider industry, and every project should adopt existing practice rather than have AHEAD reinvent it. A project may choose to keep decision-records, design-docs, and runbooks alongside its Diátaxis-shaped documentation. AHEAD only constrains the three durable classes.

## Diátaxis anti-patterns — the soft nudge

Diátaxis identifies four common failure modes when documentation mixes modes. AHEAD's durable classes inherit them as review heuristics:

1. **Reference contaminated with narrative explanation** — the design-doc reference layer becomes hard to scan. Split into reference and explanation sections.
2. **How-to padded with tutorial-style context** — the runbook becomes too slow to execute at 3am. Move context into a linked design-doc.
3. **Explanation written as procedure** — a design-doc's rationale reads as steps and rots when the steps change. Rewrite as argument, not sequence.
4. **Tutorial that assumes prior knowledge** — not applicable to AHEAD's three classes but a common trap in the surrounding project docs.

These are conversational heuristics for review, not enforced fields. A reviewer who sees one of them says so; the implementer decides how to address it.

## Style baseline

For prose in decision-records, design-docs, and runbooks, adopt the [Google developer documentation style guide](https://developers.google.com/style) as the default. It is freely available, comprehensive, and consistent with the terse-and-scannable tone AHEAD's classes require. A project may layer its own conventions on top; the Google guide sets the floor.

## Rule of thumb

If a run's outcome will bind future work, it produces or updates a durable reference. If a run's outcome only affects itself, its record is enough. When in doubt, ask whether a future engineer, reviewer, or on-call responder would be worse off without the record; if yes, write the durable reference.
