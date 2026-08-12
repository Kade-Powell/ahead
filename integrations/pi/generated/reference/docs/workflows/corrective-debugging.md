# Corrective Debugging Workflow

Status: pilot v0.1

## Outcome

Use this flow when observed behavior conflicts with intended behavior and the dominant work is causal diagnosis plus correction. It ends with a verified correction or an explicit human acceptance of remaining uncertainty.

Use operational stabilization instead when a live system is unhealthy and restoration or convergence is the immediate objective.

## Lifecycle

```text
                  BUG REPORTED
                       │
                       ▼
              ┌─────────────────┐
              │ 1. CHARACTERIZE │
              │                 │
              │ HUMAN           │
              │ • What happens? │
              │ • Expected?     │
              │ • When / where? │
              │ • Impact/scope? │
              │ • Reproducible? │
              └───────┬─────────┘
                      │
                      ▼
              ┌─────────────────┐
              │ 2. HUMAN MODEL  │
              │                 │
              │ What do we      │
              │ currently think │
              │ is happening?   │
              └───────┬─────────┘
                      │
                      ▼
              ┌─────────────────┐
              │ 3A. AI ASSISTS  │
              │ EVIDENCE REVIEW │
              │                 │
              │ • Code          │
              │ • Logs/history  │
              │ • Dependencies  │
              │ • Runtime data  │
              │ • Contradictions│
              └───────┬─────────┘
                      │
                      ▼
              ┌─────────────────┐
              │ 3B. HYPOTHESES  │
              │                 │
              │ HUMAN LEADS     │
              │ AI ASSISTS      │
              │ • H1 / H2 / H3  │
              │ • Evidence FOR  │
              │ • Evidence      │
              │   AGAINST       │
              └───────┬─────────┘
                      │
                      ▼
              ┌─────────────────┐
              │ 3C. HUMAN       │
              │ SELECTS TEST    │
              │                 │
              │ • Prediction    │
              │ • Safety/scope  │
              └───────┬─────────┘
                      │
                      ▼
              ┌─────────────────┐
              │ 3D. TEST        │
              │ HYPOTHESIS      │
              │                 │
              │ HUMAN/ENGINEER  │
              │ Run / inspect / │
              │ instrument      │
              │ AI — ASSIST     │
              └───────┬─────────┘
                      │
                      ▼
                ┌──────────────┐
                │ HUMAN READY  │
                │ TO CHOOSE?   │
                └──────┬───────┘
                   NO ↙  ↘ YES
                     │    │
                     │    ▼
        ↺ HUMAN MODEL     HUMAN ACCEPTS DIAGNOSIS
                          OR UNKNOWN CAUSE / RISK (4)
                                  │
                                  ▼
                         HUMAN FIX APPROACH (5)
                                  │
                                  ▼
                         HUMAN FIRST-PASS PLAN (6)
                                  │
                                  ▼
                       ENGINEER IMPLEMENTS (7)
                                  │
                                  ▼
                              AI REVIEW (8)
                                  │
                                  ▼
                 HUMAN DISPOSITIONS MATERIAL FINDINGS
                                  │
                                  ▼
                    INDEPENDENT HUMAN REVIEW (9)
                                  │
                                  ▼
                  HUMAN AUTHORIZES DEPLOY / RELEASE (10)
                           WHEN APPLICABLE
                                  │
                                  ▼
          HUMAN VERIFIES CORRECTION OF ORIGINAL FAILURE (11)
                                  │
                                  ▼
                 HUMAN OBSERVES DEPLOYED OUTCOME
                                  │
                                  ▼
                              AI AUDIT (12)
                                  │
                                  ▼
                    HUMAN DISPOSITIONS AUDIT FINDINGS
                                  │
                                  ▼
                         HUMAN OUTCOME GATE (13)

Not corrected ───────────────────↺ HUMAN MODEL
```

Reproduction is useful but not a universal gate. Historical, intermittent, production-only, or already mitigated failures may proceed when the limitation is recorded.

“Ready to choose” means either the evidence sufficiently supports a human-accepted diagnosis or the accountable human explicitly accepts that the cause remains unknown and records the risk of proceeding. Unsupported hypotheses alone do not satisfy the gate.

Executable phase 3, `investigate`, contains the evidence review, hypothesis, human test-selection, and test loop shown as 3A–3D. The human conclusion is a separate gate so a plausible hypothesis cannot silently become a diagnosis.

## Minimal phases

| Phase | Human owns | AI may | Minimum record | Advance when |
|---|---|---|---|---|
| Report and characterize | Intended behavior, observed behavior, impact, scope, and evidence quality | Organize evidence and identify missing characterization | Failure statement and evidence links | The failure is bounded enough to investigate |
| Mental model | Current explanation of relevant components, state, and interactions | Explain unfamiliar mechanisms and challenge omissions | Model, assumptions, and unknowns | The model can generate testable hypotheses |
| Investigate and test | Hypothesis selection, test authorization, prediction, and interpretation | Generate alternatives, evidence for/against, and discriminating tests | Facts, inferences, hypotheses, predictions, tests, results, confidence | Evidence is ready for a human conclusion |
| Conclude diagnosis | Supported diagnosis or explicit acceptance of unknown cause, confidence, and risk | Challenge the conclusion against evidence and counterevidence | Diagnosis or accepted uncertainty | Human accepts the diagnosis or remaining uncertainty |
| Choose correction | Desired correction and tradeoffs | Compare fix approaches and recurrence risks | Selected correction and rationale | Human approves the correction |
| Plan | First-pass correction and verification plan | Find missing cases, risks, regression tests, and rollout concerns | Plan and rollback or containment needs | Human approves the plan |
| Implement | Code and engineering changes | Bounded implementation and debugging assistance | Linked changeset and regression evidence | Change is ready for review |
| AI review | Validate and disposition every material AI finding | Review the exact snapshot for correction, tests, risks, and plan alignment without modifying it | Snapshot-bound AI findings; separate human disposition | Every material finding is fixed, invalid, accepted risk, or follow-up with rationale |
| Human review | Independent final engineering judgment by someone other than the implementer | Answer targeted questions and retrieve evidence | Current independent human review | Independent human reviewer accepts the current change |
| Deploy or release | Authorization and rollout decision | Analyze readiness evidence within policy | Version, environment, actor, time, and result | The intended correction reaches the target environment or deployment is explicitly not applicable |
| Verify and observe | Original failure, regression protection, and deployed behavior when applicable | Suggest checks and analyze authorized evidence | Pre-change comparison, fix validation, deployment evidence, and observed outcome | The original failure and user-visible outcome are evaluated |
| AI audit | Disposition of findings and required response | Compare the result with the failure, diagnosis or accepted uncertainty, correction, plan, reviews, and observed behavior | AI audit findings; separate human disposition | The human disposer accepts the audit gate or reopens work |
| Outcome | Acceptance, rollback, continued investigation, follow-up, or abandonment | Summarize learning | Result, causal confidence, uncertainty, and follow-ups | Human accepts closure or reopens/routes work |

## Evidence chain

```text
OBSERVED FAILURE
       │
       ▼
FACTS / EVIDENCE ──► HUMAN MENTAL MODEL
                            │
                            ▼
            HUMAN-LED / AI-EXPANDED HYPOTHESES
                            │
                            ▼
                 PREDICTION / TEST / RESULT
                            │
                            └────────────↺ MODEL
                            │
                            ▼
                HUMAN CONCLUSION / CONFIDENCE
                            │
                            ▼
             HUMAN CORRECTION DECISION / PLAN
                            │
                            ▼
                  ENGINEER CHANGE
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
                HUMAN-AUTHORIZED DEPLOYMENT
                            │
                            ▼
              CORRECTION VERIFICATION EVIDENCE
                            │
                            ▼
                DEPLOYED OUTCOME EVIDENCE
                            │
                            ▼
               AI AUDIT / HUMAN DISPOSITION
                            │
                            ▼
                    HUMAN OUTCOME
```

## Non-waivable pilot rules

- Facts, inferences, and hypotheses remain distinguishable.
- AI hypotheses are candidates, not diagnoses.
- The human chooses or authorizes tests and interprets their results.
- A plausible cause is not treated as proven.
- Fix validation and post-deployment outcome verification are distinct when deployment applies.
- AI review and implementer self-review do not satisfy independent human review.
- Implementation completion is not deployment, recovery, or outcome verification.

## Pilot questions

- What was the minimum useful investigation record?
- Did recording predictions before tests reduce hindsight interpretation?
- When was correction justified without a conclusive cause?
- Did AI broaden hypotheses or anchor the investigator?
