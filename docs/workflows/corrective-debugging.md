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
              │ 3. AI ASSISTS   │
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
              │ 4. HYPOTHESES   │
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
              │ 5. HUMAN SELECTS│
              │ WHAT TO TEST    │
              │                 │
              │ • Prediction    │
              │ • Safety/scope  │
              └───────┬─────────┘
                      │
                      ▼
              ┌─────────────────┐
              │ 6. TEST         │
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
                │ SUPPORTED    │
                │ ENOUGH?      │
                └──────┬───────┘
                   NO ↙  ↘ YES
                     │    │
                     │    ▼
        ↺ HUMAN MODEL     HUMAN DIAGNOSIS
                          OR ACCEPTED UNCERTAINTY
                                  │
                                  ▼
                         HUMAN FIX APPROACH
                                  │
                                  ▼
                         HUMAN FIRST-PASS PLAN
                                  │
                                  ▼
                       ENGINEER IMPLEMENTS
                                  │
                                  ▼
                              AI REVIEW
                                  │
                                  ▼
                            HUMAN REVIEW
                                  │
                                  ▼
                HUMAN VERIFIES ORIGINAL FAILURE
                                  │
                                  ▼
                         HUMAN OUTCOME GATE

Not corrected ───────────────────↺ HUMAN MODEL
```

Reproduction is useful but not a universal gate. Historical, intermittent, production-only, or already mitigated failures may proceed when the limitation is recorded.

## Minimal phases

| Phase | Human owns | AI may | Minimum record | Advance when |
|---|---|---|---|---|
| Report and characterize | Intended behavior, observed behavior, impact, scope, and evidence quality | Organize evidence and identify missing characterization | Failure statement and evidence links | The failure is bounded enough to investigate |
| Mental model | Current explanation of relevant components, state, and interactions | Explain unfamiliar mechanisms and challenge omissions | Model, assumptions, and unknowns | The model can generate testable hypotheses |
| Hypothesize and test | Hypothesis selection, test authorization, prediction, and interpretation | Generate alternatives, evidence for/against, and discriminating tests | Facts, inferences, hypotheses, predictions, tests, results, confidence | Human accepts a diagnosis or explicitly accepts uncertainty |
| Choose correction | Desired correction and tradeoffs | Compare fix approaches and recurrence risks | Selected correction and rationale | Human approves the correction |
| Plan | First-pass correction and verification plan | Find missing cases, risks, regression tests, and rollout concerns | Plan and rollback or containment needs | Human approves the plan |
| Implement | Code and engineering changes | Bounded implementation and debugging assistance | Linked changeset and regression evidence | Change is ready for review |
| AI review | Disposition of valid findings | Review correction, tests, risks, and plan alignment | AI findings and dispositions | Blocking findings are resolved or rejected with rationale |
| Human review | Final engineering judgment | Answer targeted questions and retrieve evidence | Current human review | Human accepts the current change |
| Verify outcome | Original failure, regression protection, deployed behavior when applicable | Suggest checks and analyze authorized evidence | Pre-change comparison, fix validation, and observed outcome | Human accepts correction or reopens investigation |

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
         ENGINEER CHANGE / AI REVIEW / HUMAN REVIEW
                            │
                            ▼
           HUMAN-ACCEPTED FAILURE VERIFICATION
                            │
                            ▼
             HUMAN-ACCEPTED DEPLOYED OUTCOME
```

## Non-waivable pilot rules

- Facts, inferences, and hypotheses remain distinguishable.
- AI hypotheses are candidates, not diagnoses.
- The human chooses or authorizes tests and interprets their results.
- A plausible cause is not treated as proven.
- Fix validation and post-deployment outcome verification are distinct when deployment applies.

## Pilot questions

- What was the minimum useful investigation record?
- Did recording predictions before tests reduce hindsight interpretation?
- When was correction justified without a conclusive cause?
- Did AI broaden hypotheses or anchor the investigator?
