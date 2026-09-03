# Internal Improvement Workflow

Audience: AHEAD practitioners

Status: pilot v0.1

## Outcome

Use this flow when externally required behavior should remain invariant while an internal quality improves: maintainability, comprehensibility, performance, reliability margin, operability, testability, cost, or preventive risk reduction.

Use product change when externally meaningful behavior is intentionally changing, or corrective debugging when an observed defect is the reason for the work.

## Lifecycle

```text
┌──────────────────────────────────────────────┐
│ 1. DEFINE INVARIANTS                         │
│                                              │
│ HUMAN                                        │
│ • Define behavior that must not change       │
│ • Define scope and non-goals                 │
│                                              │
│ AI — ASSIST                                  │
│ • Identify overlooked contracts/consumers    │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 2. BASELINE                                  │
│                                              │
│ HUMAN accepts method, evidence, uncertainty  │
│ AI — ASSIST challenges noise / confounders   │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 3. TARGET                                    │
│                                              │
│ HUMAN defines quality, threshold, guardrails │
│ AI — ASSIST exposes gaming / shifted cost    │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 4. OPTIONS                                   │
│                                              │
│ HUMAN — FIRST PASS                           │
│ • Propose at least one improvement approach  │
│                                              │
│ AI — ASSIST                                  │
│ • Expand alternatives • expose coupling      │
│ • Challenge complexity and assumptions       │
│                                              │
│ HUMAN                                        │
│ • Evaluate and refine options                │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 5. HUMAN DECISION GATE                       │
│ • Select approach and accept tradeoffs       │
│ • Record rationale                           │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 6. PLAN                                      │
│                                              │
│ HUMAN — FIRST PASS                           │
│ • Sequence change, checks, and rollback      │
│                                              │
│ AI — ASSIST                                  │
│ • Find affected boundaries and missing tests │
│                                              │
│ HUMAN finalizes and approves                 │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 7. IMPLEMENT                                 │
│ ENGINEER owns and understands the change     │
│ AI — ASSIST with bounded contributions       │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 8. AI REVIEW                                 │
│ • Hidden behavior changes • complexity       │
│ • Tests • coupling • plan compliance         │
└──────────────────────┬───────────────────────┘
                       ↓
          HUMAN DISPOSITIONS EACH MATERIAL
          AI FINDING WITH RATIONALE / EVIDENCE
                       ↓
┌──────────────────────────────────────────────┐
│ 9. INDEPENDENT HUMAN REVIEW                  │
│ • Reviewer is not the implementer            │
│ • Final judgment on preservation/improvement │
└──────────────────────┬───────────────────────┘
                       ↓
       10. HUMAN AUTHORIZES DEPLOY / RELEASE
                    WHEN APPLICABLE
                       ↓
┌──────────────────────────────────────────────┐
│ 11. VERIFY                                   │
│                                              │
│ HUMAN                                        │
│ • Verify invariants                          │
│ • Compare before/after target evidence       │
│                                              │
│ AI — ASSIST                                  │
│ • Analyze measurements / suggest checks      │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 12. AI AUDIT                                 │
│ • Compare invariants, target, plan, reviews, │
│   deployment, and observed evidence          │
└──────────────────────┬───────────────────────┘
                       ↓
          HUMAN DISPOSITIONS AUDIT FINDINGS
                       ↓
                ┌───────────────┐
                │ INVARIANTS    │
                │ PRESERVED?    │
                └───────┬───────┘
                   NO ↙   ↘ YES
                     │     │
                     │     ▼
                     │  ┌────────────────┐
                     │  │ TARGET QUALITY │
                     │  │ IMPROVED?      │
                     │  └───────┬────────┘
                     │     NO ↙   ↘ YES
                     │       │      └────► HUMAN OUTCOME GATE (13)
                     │       ▼
                     │  HUMAN CHOOSES
                     │  ├─ Rework ─────────────↺ IMPLEMENT
                     │  ├─ Roll back ─────────► HUMAN OUTCOME
                     │  └─ Accept partial/no improvement
                     │     with rationale ─────► HUMAN OUTCOME
                     ▼
              HUMAN CHOOSES
              ├─ Rework ──────────────────────↺ IMPLEMENT
              ├─ Roll back ──────────────────► HUMAN OUTCOME
              └─ Reclassify ─────────────────► NEW PRODUCT CHANGE RUN
                                                + HUMAN OUTCOME

Review changes requested ─────────────↺ IMPLEMENT
```

Preservation and improvement are separate judgments. A failed invariant cannot be accepted as an internal improvement: the team must roll back, rework, or deliberately reclassify the work as a product change. If invariants hold but the target quality does not improve, a human may stop and record a partial or unsuccessful result rather than manufacture success.

## Minimal phases

| Phase | Human owns | AI may | Minimum record | Advance when |
|---|---|---|---|---|
| Invariants | Behavior that must remain unchanged, scope, and non-goals | Identify overlooked contracts and consumers | Explicit invariants and exclusions | Human accepts the preservation contract |
| Baseline | Current quality evidence and measurement validity | Gather metrics and identify measurement gaps | Tests, measurements, structural evidence, or observations | Baseline is credible enough for comparison |
| Target | Quality to improve, success threshold, and tradeoffs | Suggest measures and unintended effects | Target quality and acceptance signal | Improvement can be evaluated |
| Options | Initial approach and evaluation of alternatives | After the human first pass, expand alternatives, expose coupling, and challenge abstraction | Human option, AI challenge when used, and evaluated options | Viable approaches and tradeoffs are understood |
| Decision | Selection, risk, confidence, and reversibility | Check the option against baseline, target, and invariants | Decision and rationale | Human approves the approach |
| Plan | First-pass sequence, verification, rollback, and the durable references the run will touch | Find affected boundaries, migration needs, missing tests, and propose reference updates | Plan, invariant checks, and planned reference updates (decision records, design docs, runbooks, or explicit none) | Human approves the final plan |
| Implement | Engineering change, scope control, and the reference edits that record the change | Bounded refactoring, explanation, tests, mechanical assistance, and proposed reference-update drafts | Linked changeset, deviations, and actual reference updates | Change is ready for review |
| AI review | Validate and disposition every material AI finding | Review against the full open run — decision, plan, plan-deviations, reference-updates, changeset, tests, and evidence — without modifying the change | Review inputs enumeration, snapshot-bound AI findings, and separate human disposition | Every material finding is fixed, invalid, accepted risk, or follow-up with rationale |
| Human review | Independent final judgment by someone other than the implementer about simplicity, coupling, risk, and preservation, with the full open run available | Answer targeted questions and retrieve evidence from the live run | Current independent human review | Independent human reviewer accepts the current change |
| Deploy or release | Authorization and rollout decision | Analyze readiness evidence within policy | Version, environment, actor, time, and result | The intended version reaches the target environment or deployment is explicitly not applicable |
| Verify | Invariant preservation and target-quality comparison | Analyze measurements and suggest negative checks | Before/after and deployed-outcome evidence when applicable, plus invariant results | Invariants are evaluated and target-quality results are measured; an invariant failure routes to rollback, rework, or product-change reclassification |
| AI audit | Disposition of findings and required response | Compare invariants, target, plan, reviews, deployment, and observed evidence; identify divergence or weak proof | AI audit findings; separate human disposition | The human disposer accepts the audit gate or reopens work |
| Outcome | Acceptance, rollback, partial result, or new work | Summarize learning | Outcome, remaining debt, and follow-ups | Human accepts closure |

## Preservation and improvement chain

```text
HUMAN-OWNED INVARIANTS / NON-GOALS
               │
               ▼
HUMAN-ACCEPTED BASELINE / TARGET
               │
               ▼
HUMAN OPTION + AI-EXPANDED ALTERNATIVES
               │
               ▼
HUMAN DECISION / FIRST-PASS PLAN
               │
               ▼
ENGINEER-OWNED CHANGESET
               │
               ▼
SNAPSHOT-BOUND AI REVIEW
               │
               ▼
IMPLEMENTING-HUMAN DISPOSITION
               │
               ▼
INDEPENDENT HUMAN REVIEW
               │
               ▼
HUMAN-AUTHORIZED DEPLOYMENT WHEN APPLICABLE
               │
               ▼
HUMAN-ACCEPTED INVARIANT VERIFICATION
AND BEFORE / AFTER COMPARISON
               │
               ▼
AI AUDIT / HUMAN DISPOSITION
               │
               ▼
HUMAN OUTCOME / REMAINING DEBT
```

## Non-waivable pilot rules

- Invariants are stated before the change.
- Passing existing tests alone does not prove preservation when those tests do not cover the contract.
- AI may propose a refactor; humans decide whether it reduces conceptual complexity.
- File movement or abstraction count is not itself an improvement.
- The outcome compares before and after evidence against the declared target.
- An invariant failure cannot close as a successful internal improvement.
- AI review and implementer self-review do not satisfy independent human review.
- Implementation completion is not deployment or outcome verification.

## Pilot questions

- Were the invariants specific enough to catch unintended behavior changes?
- Did the selected measure reflect the quality people actually wanted?
- Did the work reduce conceptual complexity or merely rearrange it?
- When did an internal improvement reveal that a product change or bug flow was needed?
