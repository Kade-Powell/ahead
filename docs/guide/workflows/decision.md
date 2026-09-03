# Decision Workflow

Audience: AHEAD practitioners

Status: pilot v0.1

## Outcome

Use this flow when the deliverable is an accountable human choice among alternatives: architecture, buy versus build, technology selection, platform direction, policy, or another consequential course.

Implementation is optional and normally belongs in a linked product-change or internal-improvement run.

## Lifecycle

```text
┌──────────────────────────────────────────────┐
│ 1. FRAME DECISION                            │
│                                              │
│ HUMAN                                        │
│ • State the choice to be made                │
│ • Name accountable decider/stakeholders      │
│ • Define scope and deadline                  │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 2. DEFINE CRITERIA                           │
│                                              │
│ HUMAN                                        │
│ • Goals • constraints • criteria             │
│                                              │
│ AI — ASSIST                                  │
│ • Expose hidden criteria and tensions        │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 3. RESEARCH AND QUESTIONS                    │
│                                              │
│ AI — ASSIST                                  │
│ • Gather cited evidence                      │
│ • Find contradictions and missing questions  │
│                                              │
│ HUMAN                                        │
│ • Read, evaluate, and accept unknowns        │
└──────────────────────┬───────────────────────┘
                       ↓
                ┌───────────────┐
                │ EVIDENCE      │
                │ REVIEWED AND  │
                │ UNCERTAINTY   │
                │ DISPOSED?     │
                └───────┬───────┘
                   NO ↙   ↘ YES
              ↺ RESEARCH    │
                            ▼
┌──────────────────────────────────────────────┐
│ 4. HUMAN INITIAL OPTION                      │
│ • Propose at least one viable course         │
│ • Explain the initial reasoning              │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 5. EXPAND AND CHALLENGE OPTIONS              │
│                                              │
│ AI — ASSIST                                  │
│ • Add alternatives • compare • challenge     │
│ • Surface consequences and risks             │
│ • Analyze reversibility                      │
│                                              │
│ HUMAN                                        │
│ • Evaluate and refine the option set         │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 6. HUMAN DECISION GATE                       │
│ • Select the course                          │
│ • Accept tradeoffs and uncertainty           │
│ • Record rationale and dissent               │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 7. VALIDATE AND PUBLISH                      │
│                                              │
│ HUMAN                                        │
│ • Validate consequences                      │
│ • Set reversibility and review trigger       │
│ • Link resulting work                        │
│                                              │
│ AI — ASSIST                                  │
│ • Challenge consistency and missing effects  │
└──────────────────────┬───────────────────────┘
                       ↓
                 HUMAN OUTCOME

Reframe or seek evidence ─────────↺ FRAME / RESEARCH
```

“Disposed” means the uncertainty is answered or the accountable human explicitly accepts it. AI does not introduce solution alternatives during research; option expansion begins only after the human initial option is recorded.

## Minimal phases

| Phase | Human owns | AI may | Minimum record | Advance when |
|---|---|---|---|---|
| Frame | Decision to be made, accountable decider, scope, deadline, and stakeholders | Clarify ambiguity and identify missing stakeholders | Decision statement and owner | Human accepts the frame |
| Criteria | Goals, constraints, evaluation criteria, and relative importance | Challenge hidden criteria and identify tensions | Criteria and non-negotiable constraints | Criteria are sufficient for comparison |
| Research | Evaluation of evidence and disposition of uncertainty | Gather cited evidence and find contradictions or missing questions; do not introduce solution alternatives yet | Sources, findings, assumptions, unknowns, and limits | Material evidence is reviewed and uncertainty is answered or explicitly accepted |
| Options | At least one human-originated option | Expand, combine, challenge, and propose alternatives | Options with provenance | Plausible option space is understood |
| Compare | Interpretation of tradeoffs | Structure comparison and sensitivity analysis | Benefits, costs, risks, consequences, reversibility | Decision is ready for accountable judgment |
| Decide | Selection, rationale, accepted tradeoffs, and dissent | Test rationale for inconsistency or missing consequence | Decision and rejected alternatives | Accountable human approves a specific revision |
| Publish and revisit | Consequence check, communication, review trigger, revisit date, linked work, and a durable decision record written in the project's ADR convention | Improve clarity, traceability, and proposed validation checks | MADR-shaped decision record with status, consequences, reversibility, review trigger, revisit date, and links | Decision record is written, communicated, and accepted as current |

## Decision evidence chain

```text
HUMAN DECISION FRAME / CRITERIA
              │
              ▼
AI-ASSISTED EVIDENCE / HUMAN-ACCEPTED UNKNOWNS
              │
              ▼
HUMAN INITIAL OPTION
              │
              ▼
AI-EXPANDED / HUMAN-EVALUATED OPTION SET
              │
              ▼
TRADEOFFS / CONSEQUENCES
              │
              ▼
HUMAN DECISION / RATIONALE / DISSENT
              │
              ▼
HUMAN-OWNED REVERSIBILITY / REVIEW TRIGGER
              │
              ▼
LINKED IMPLEMENTATION OR INVESTIGATION
```

## Non-waivable pilot rules

- The accountable human defines the decision and evaluation criteria.
- A human contributes an initial option before AI expands the option set.
- AI does not select, approve, or manufacture consensus.
- Unknowns and dissent are not erased by polished rationale.
- The decision records when and why it should be revisited.

## Pilot questions

- Did human-first option generation preserve useful diversity?
- Were the criteria defined before a preferred answer emerged?
- Which evidence actually changed the decision?
- Did the review trigger cause a stale decision to be revisited?
