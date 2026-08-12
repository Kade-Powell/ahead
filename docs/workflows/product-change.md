# Product Change Workflow

Status: pilot v0.1

## Outcome

Use this flow when the dominant outcome is new, changed, adapted, migrated, or retired externally meaningful behavior. It ends with verified intended behavior and an accountable human outcome decision.

Do not use it when the main task is explaining an observed failure, restoring a live system, making a decision without implementation, producing knowledge, or improving internals while preserving behavior.

## Lifecycle

```text
┌──────────────────────────────────────────────┐
│ 1. DEFINE PROBLEM                            │
│                                              │
│ HUMAN                                        │
│ • Define desired outcome and users           │
│ • Define constraints and scope               │
│ • Define success signals                     │
│ • Define initial questions                   │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 2. RESEARCH                                  │
│                                              │
│ AI — ASSIST                                  │
│ • Search authorized sources                  │
│ • Compile and cite evidence                  │
│ • Answer known questions                     │
│ • Identify contradictions and gaps           │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 3. RESEARCH REVIEW                           │
│                                              │
│ HUMAN                                        │
│ • Read and understand the research           │
│ • Ask follow-up questions                    │
│ • Challenge findings                         │
│                                              │
│ AI — ASSIST                                  │
│ • Identify questions humans missed           │
│ • Research unanswered questions              │
└──────────────────────┬───────────────────────┘
                       ↓
                ┌───────────────┐
                │ IMPORTANT     │
                │ UNKNOWNS      │
                │ DISPOSED?     │
                └───────┬───────┘
                   NO ↙   ↘ YES
                      │     │
             ↺ RESEARCH     ↓
┌──────────────────────────────────────────────┐
│ 4. OPTIONS                                   │
│                                              │
│ HUMAN — FIRST PASS                           │
│ • Propose at least one viable approach       │
│ • Explain initial reasoning                  │
│                                              │
│ AI — ASSIST                                  │
│ • Research and challenge the human option    │
│ • Identify additional approaches             │
│ • Compare tradeoffs and risks                │
│                                              │
│ HUMAN                                        │
│ • Evaluate, refine, add, or remove options   │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 5. DECISION                                  │
│                                              │
│ HUMAN GATE                                   │
│ • Select approach                            │
│ • Accept tradeoffs and unknowns              │
│ • Record rationale and reversibility         │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 6. PLAN                                      │
│                                              │
│ HUMAN — FIRST PASS                           │
│ • Define implementation steps and systems    │
│ • Define testing, rollout, and recovery       │
│                                              │
│ AI — OPTIONAL ASSISTANCE                     │
│ • Find gaps, dependencies, risks, edge cases │
│ • Challenge assumptions                      │
│                                              │
│ HUMAN                                        │
│ • Finalize and approve the plan              │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 7. IMPLEMENT                                 │
│                                              │
│ ENGINEER                                     │
│ • Own and understand the implementation      │
│                                              │
│ AI — ASSIST                                  │
│ • Bounded code, tests, and explanation       │
│ • Debugging and refactoring suggestions      │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 8. AI REVIEW                                 │
│ • Correctness • security • tests             │
│ • Architecture • plan compliance             │
│ • Maintainability                            │
└──────────────────────┬───────────────────────┘
                       ↓
          HUMAN DISPOSITIONS EACH MATERIAL
          AI FINDING WITH RATIONALE / EVIDENCE
                       ↓
┌──────────────────────────────────────────────┐
│ 9. INDEPENDENT HUMAN REVIEW                  │
│ • Final engineering judgment by a reviewer   │
│   other than the implementer                 │
│ • Approve or request changes                 │
└──────────────────────┬───────────────────────┘
                       ↓
       10. HUMAN AUTHORIZES DEPLOY / RELEASE
                       ↓
           11. HUMAN VERIFIES / OBSERVES
                       ↓
                  12. AI AUDIT
                       ↓
          HUMAN DISPOSITIONS AUDIT FINDINGS
                       ↓
┌──────────────────────────────────────────────┐
│ 13. HUMAN OUTCOME                            │
│ • Accept, roll back, follow up, or abandon   │
│ • Record learning                            │
└──────────────────────────────────────────────┘

Review changes requested ─────────↺ IMPLEMENT
Outcome not demonstrated ─────────↺ PLAN / IMPLEMENT
```

An important unknown is disposed only when it is answered or an accountable human explicitly accepts the uncertainty and its consequences. Merely recording or deferring it does not satisfy the gate.

## Minimal phases

| Phase | Human owns | AI may | Minimum record | Advance when |
|---|---|---|---|---|
| Define | Desired outcome, users, scope, constraints, initial questions | Clarify ambiguity and expose assumptions | Problem and success signals | Human accepts the framing |
| Research | Reading and evaluating material | Gather internal/external evidence, cite, compare, identify gaps | Evidence links, findings, contradictions | Material evidence is available |
| Questions | Understanding and disposition of unknowns | Find missing questions and research answers | Answered, accepted, blocked, or deferred unknowns | Important unknowns are answered or explicitly accepted |
| Options | At least one initial approach and evaluation criteria | Expand, compare, challenge, add alternatives | Options and tradeoffs | Viable options are understood |
| Decide | Selection and consequences | Check rationale and surface risks | Decision, rationale, tradeoffs, reversibility | Accountable human approves the decision |
| Plan | First-pass implementation plan | Identify gaps, dependencies, tests, risks, and rollback needs | Sequenced plan and deviations policy | Human approves the final plan |
| Implement | Code and engineering changes | Bounded generation, explanation, tests, debugging, and refactoring assistance | Linked changeset and plan deviations | Work is ready for independent review and checks pass |
| AI review | Validate and disposition every material AI finding | Review the exact snapshot for behavior, security, tests, architecture, and plan alignment without modifying it | Snapshot-bound AI findings; separate human disposition | Every material finding is fixed, invalid, accepted risk, or follow-up with rationale |
| Human review | Independent final engineering judgment by someone other than the implementer | Answer targeted questions and retrieve evidence | Current independent human review | Independent human reviewer accepts the current change |
| Deploy or release | Authorization and rollout decision | Analyze readiness evidence within policy | Version, environment, actor, time, and result | The intended version reaches the target environment or deployment is explicitly not applicable |
| Verify and observe | Evaluation against intended behavior | Suggest checks and analyze authorized observations | Test, deployment, and user-visible evidence | Intended outcome is demonstrated or failure is recorded |
| AI audit | Disposition of findings and any required response | Compare the result with the problem, decision, plan, reviews, and observed behavior; identify divergence, weak evidence, and missed learning | AI audit findings; separate human disposition | The human disposer accepts the audit gate or reopens work |
| Outcome | Acceptance, rollback, follow-up, or abandonment | Summarize learning | Result, uncertainty, follow-ups | Human accepts closure or routes more work |

## Artifact and evidence chain

```text
PROBLEM / SUCCESS SIGNALS
            │
            ▼
RESEARCH / QUESTIONS
            │
            ▼
HUMAN OPTION / ALTERNATIVES
            │
            ▼
HUMAN DECISION / RATIONALE
            │
            ▼
HUMAN FIRST-PASS / FINAL PLAN
            │
            ▼
CHANGESET / TESTS
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
VERIFICATION / OBSERVATIONS
            │
            ▼
AI AUDIT / HUMAN DISPOSITION
            │
            ▼
HUMAN OUTCOME / LEARNING
```

## Non-waivable pilot rules

- A human defines or affirms the problem and outcome.
- A human contributes an option before AI expands the option set.
- A human authors the first-pass plan.
- AI review and implementer self-review do not satisfy independent human review.
- Implementation completion is not deployment or outcome verification.

## Pilot questions

- Did separate research and question review improve the decision?
- Did requiring a human option and first-pass plan improve understanding or only add ceremony?
- Which changes were material enough to reopen the decision or plan?
- What outcome evidence was available only after deployment?
