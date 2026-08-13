# AHEAD Pilot Workflows

Audience: AHEAD practitioners

Status: executable dogfood v0.1

## Purpose

These six minimal workflow profiles are meant to be used on real engineering work through the Pi adapter or a manual record. Their canonical executable contracts live in `spec/workflows`, while these documents explain the same boundaries and show the human/AI rhythm. Dogfooding should reveal which phases, gates, and records improve reasoning and which create process burden.

The profiles are:

1. [Product change](product-change.md)
2. [Corrective debugging](corrective-debugging.md)
3. [Operational stabilization](operational-stabilization.md)
4. [Decision](decision.md)
5. [Investigation](investigation.md)
6. [Internal improvement](internal-improvement.md)

Choose the flow by its dominant outcome, not by the issue label. Incident, emergency, security, regulatory, and other concerns are overlays or modifiers.

## Rule hierarchy

The pilot applies rules in this order:

1. [AHEAD Constitution](../../../CONSTITUTION.md)
2. [Acceptable AI Use](../acceptable-ai-use.md)
3. The selected workflow profile and applicable overlays
4. Organization and repository guidance, which may narrow but not broaden AI authority

Phase permission is not a waiver of higher-level policy. When rules appear to conflict, apply the more protective rule and record the ambiguity for correction.

## Shared pilot contract

Every pilot run has:

- one accountable human owner;
- a stated outcome or question;
- a selected workflow and relevant modifiers;
- a human-originated initial understanding before AI expansion;
- links to material evidence rather than unsupported summaries;
- visible facts, inferences, unknowns, decisions, and accepted risks;
- recorded AI contributions when they materially influence the work;
- human authorization for consequential actions;
- independent human review where a lasting change is produced;
- outcome evidence and a human closure decision.

Phases may loop or reopen. A workflow is not invalid merely because learning changes an earlier decision or plan. The record should make the change visible.

## Common human gates

| Gate | Required when | Minimum evidence |
|---|---|---|
| Framing accepted | Every run | Human-owned outcome, question, failure, or invariant |
| Decision accepted | A course or intervention is selected | Chosen option, rationale, tradeoffs, unknowns, and accountable human |
| Plan accepted | Before a lasting implementation | Human first-pass plan plus accepted AI challenges or additions |
| Action authorized | Before a consequential, risky, destructive, or production action | Actor, purpose, scope, blast radius, rollback or containment, and authorization |
| Independent human review accepted | Before accepting a lasting engineering change | Review of the current changeset and material evidence by a person other than the implementer; both implementer and reviewer understand their responsibilities |
| Outcome accepted | Before closure | Verification against the original outcome plus remaining uncertainty and follow-ups |

Emergency policy may defer nonessential documentation and, where explicitly allowed, independent review needed to restore service. It does not remove authorization or accountability, and any deferred review gate remains open until a named human completes it after stabilization. Deferred reasoning is reconstructed after stabilization.

## Minimal run record

For the pilot, keep one Markdown file per run. A team can place it in `.ahead/runs/<id>.md`, an issue, or another durable system as long as links and revision history remain available.

```yaml
id: AHEAD-YYYY-NNNN
title: Short description
workflow: product-change | corrective-debugging | operational-stabilization | decision | investigation | internal-improvement
owner: human identity
status: active | blocked | complete | abandoned
modifiers:
  urgency: normal | expedited | incident | emergency
  assurance: standard | security | regulated | safety-critical
  environment: local | test | staging | production | external
links: []
work_item: optional provider-neutral URL
```

The body records only the sections required by the selected flow. Evidence may remain in its native system and be linked rather than copied.

## Shared human–AI rhythm

Diagram language is normative for the pilot:

- `HUMAN` or `ENGINEER` means the person owns the reasoning, decision, action, or artifact.
- `AI — ASSIST` means AI may research, organize, propose, explain, generate bounded material, or challenge; it does not own or approve the phase.
- `AI REVIEW` produces snapshot-bound hypotheses. The implementing human separately dispositions every material finding, and neither action satisfies the independent human-review gate.
- Every decision, test selection, risk acceptance, consequential-action authorization, final review, and outcome gate is human.
- All diagram permissions remain bounded by the rule hierarchy above. Humans lead and remain accountable; AI assists.

```text
┌──────────────────────────────────────────────┐
│ HUMAN FRAMES THE WORK                        │
│ • Outcome, question, failure, or invariant   │
│ • Initial understanding                      │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ AI — ASSISTS, AMPLIFIES, AND CHALLENGES      │
│ • Research • alternatives • hypotheses       │
│ • Gaps • contradictions • risks              │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ HUMAN UNDERSTANDS AND DECIDES                │
│ • Evaluate evidence • accept tradeoffs       │
│ • Resolve or accept important unknowns       │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ HUMAN PLANS OR AUTHORIZES ACTION             │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ ENGINEER ACTS WITH BOUNDED AI ASSISTANCE     │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ AI REVIEW / ANALYSIS                         │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ HUMAN REVIEWS AND ACCEPTS THE OUTCOME        │
└──────────────────────┬───────────────────────┘
                       ↓
              RECORD EVIDENCE AND LEARNING

Questions remain ───────────────↺ AI assistance
Outcome not accepted ───────────↺ Work / investigation
```

## Pilot feedback

For each completed run, record:

- which phase or gate prevented a mistake or improved understanding;
- which required record was unused or burdensome;
- where the team could not agree on routing or completion;
- where AI helped, anchored, distracted, or weakened learning;
- what was reconstructed after the fact;
- what the eventual engine should enforce, warn about, or leave to judgment.

These profiles are AHEAD design hypotheses. Using them is how AHEAD will learn whether the six-flow taxonomy and its gates deserve stronger enforcement.
