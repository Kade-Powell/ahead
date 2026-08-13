# Operational Stabilization Workflow

Audience: AHEAD practitioners

Status: pilot v0.1

## Outcome

Use this flow when a live system is outside an acceptable operating state and the immediate outcome is restoration, stabilization, or demonstrated convergence. It covers code and non-code causes such as reconciliation storms, configuration drift, capacity exhaustion, provider behavior, dependency failure, and emergent controller interaction.

Incident mode is an urgency and coordination overlay. It does not assert a cause.

## Lifecycle

```text
             LIVE SYSTEM OUTSIDE
           ACCEPTABLE OPERATING STATE
                       │
                       ▼
┌──────────────────────────────────────────────┐
│ 1. ASSESS                                    │
│                                              │
│ HUMAN                                        │
│ • Assess impact, urgency, and scope          │
│ • Define desired versus actual state         │
│ • Assign accountable ownership               │
│                                              │
│ AI — ASSIST                                  │
│ • Correlate authorized signals               │
│ • Identify missing information               │
└──────────────────────┬───────────────────────┘
                       ↓
                ┌───────────────┐
                │ HUMAN SELECTS │
                │ RESPONSE MODE │
                │ Normal /      │
                │ incident /    │
                │ emergency     │
                └───────┬───────┘
                        ↓
        ┌──────── PARALLEL — NO JOIN BARRIER ────────┐
        │                                            │
        ▼                                            ▼
┌──────────────────────┐          ┌──────────────────────┐
│ 2A. RESPOND:         │          │ 2B. RESPOND:         │
│ INVESTIGATE          │          │ STABILIZE            │
│                      │          │                      │
│ HUMAN LEADS          │          │ HUMAN LEADS          │
│ • Model system       │          │ • Set priorities     │
│ • Select tests       │          │ • Select candidate   │
│ • Interpret evidence│          │   intervention       │
│                      │          │ • Assess risk        │
│ AI — ASSIST          │          │                      │
│ • Organize telemetry│          │ AI — ASSIST          │
│ • Suggest hypotheses│          │ • Compare actions    │
│ • Find conflicts     │          │ • Find side effects  │
└──────────┬───────────┘          └──────────┬───────────┘
           │ CONTINUES                         │
           │ WHILE NEEDED                      ▼
           │                      ┌─────────────────────────┐
           │                      │ 2C. HUMAN ACTION GATE   │
           │ MAY INFORM           │                         │
           ├─ - - - - - - - - - ►│ • Authorize actor/scope │
           │   NO BARRIER         │ • Blast radius          │
           │                      │ • Rollback/containment  │
           │                      │ • Accepted uncertainty  │
           │                      └────────────┬────────────┘
           │                                   ↓
┌──────────────────────────────────────────────┐
│ 3. EXECUTE AND OBSERVE                       │
│                                              │
│ HUMAN / PREAUTHORIZED AUTOMATION             │
│ • Performs the operational action            │
│                                              │
│ AI — ASSIST                                  │
│ • Interprets authorized observations         │
│ • Does not execute the intervention          │
│ • Workflow phase grants no production access │
└──────────────────────┬───────────────────────┘
                       ↓
                ┌───────────────┐
                │ CONVERGING?   │
                └───────┬───────┘
                   NO ↙   ↘ YES
          ↺ INVESTIGATE     │
            / STABILIZE     ▼
┌──────────────────────────────────────────────┐
│ 4. VERIFY RECOVERY                           │
│                                              │
│ HUMAN                                        │
│ • Verify convergence and user-visible state │
│                                              │
│ AI — ASSIST                                  │
│ • Analyze authorized recovery indicators     │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 5. MONITOR RECURRENCE                        │
│ HUMAN selects duration and signals           │
│ AI — ASSIST                                  │
│ • Summarize authorized telemetry             │
└──────────────────────┬───────────────────────┘
                       ↓
                ┌───────────────┐
                │ STABLE?       │
                └───────┬───────┘
                   NO ↙   ↘ YES
          ↺ INVESTIGATE     │
            / STABILIZE     ▼
┌──────────────────────────────────────────────┐
│ 6. HUMAN OUTCOME                             │
│ • Accept closure and remaining risk          │
│ • Link corrective, investigation, decision, │
│   or improvement follow-up runs              │
└──────────────────────────────────────────────┘
```

Investigation and stabilization proceed independently: there is no join gate. Investigation may inform an intervention and continue after it, but service restoration does not wait for a complete causal explanation when a human authorizes a proportionate intervention.

The executable `respond` phase contains both parallel tracks and the human action gate shown as 2A–2C. Combining their evidence in one phase visit preserves the no-join rule: a bounded intervention may be authorized while causal investigation remains incomplete.

## Minimal phases

| Phase | Human owns | AI may | Minimum record | Advance when |
|---|---|---|---|---|
| Assess | Response-mode declaration, impact, scope, urgency, desired state, ownership, and recovery signals | Correlate authorized signals and expose missing information after the human assessment begins | Assessment and response-mode record | Response mode, ownership, boundaries, and recovery criteria are clear |
| Respond | System model, investigation direction, stabilization priorities, and selected intervention | Organize telemetry, generate hypotheses, compare interventions, and expose contradictions or side effects | Human model, optional investigation ledger, and bounded intervention with rollback and stop conditions | Human authorizes the consequential action; proven root cause is not required |
| Execute and observe | Action execution, stop or rollback decisions, and interpretation | Analyze observations; AI has no execute capability in this phase | Actor, command or change, time, result, and new evidence | Result is known and desired state is approached |
| Verify recovery | Recovery criteria and user-visible validation | Analyze convergence and recurrence indicators | Health, convergence, and external behavior evidence | Recovery is demonstrated, not merely assumed |
| Monitor | Duration and signals sufficient to detect recurrence | Summarize authorized telemetry | Monitoring window and result | Human accepts stability or reopens response |
| Outcome and follow-up | Closure, remaining risk, causal confidence, and routed work | Summarize timeline and proposed follow-ups | Outcome, unknowns, deferred records, linked runs | Human accepts closure and follow-up disposition |

## Parallel evidence and action chains

```text
OBSERVED OPERATING CONDITION
              │
              ▼
HUMAN-OWNED IMPACT / SCOPE / DESIRED STATE
              │
       ┌──────┴────────┐
       ▼               ▼
EVIDENCE / MODEL   STABILIZATION OPTIONS
       │ CONTINUES         │
       ▼                   ▼
HUMAN-SELECTED      HUMAN AUTHORIZATION
HYPOTHESES / TESTS         │
       │ MAY INFORM        ▼
       ├─ - - - - - ► ACTION / OBSERVED RESULT
       │                   │
       └──── updates ◄─────┘
                           │
                           ▼
            HUMAN-ACCEPTED RECOVERY / CONVERGENCE
                           │
                           ▼
REMAINING RISK / LINKED FOLLOW-UP RUNS

The investigation chain has no join barrier. AI assists
evidence gathering, option generation, hypothesis
generation, and analysis; it owns no gate.
```

## Incident and emergency overlay

During incident or emergency mode, maintain at minimum:

- current owner and impact;
- timestamped consequential actions and decisions;
- actor, rationale, scope, blast radius, rollback, and result;
- current recovery criteria and communications owner;
- deferred records that must be reconstructed after stabilization.

## Non-waivable pilot rules

- Workflow permission is not production authorization.
- A live intervention requires an accountable human unless existing automation was previously authorized for that action.
- Any lasting engineering change produced during stabilization still requires a human first-pass plan and independent human review, either in this record or a linked change run; emergency policy may defer but not erase those gates.
- Kubernetes readiness, controller status, or a successful command is not automatically user-visible recovery.
- The conclusion may include mechanism, trigger, enabling conditions, and detection or containment gaps rather than one root cause.
- Closure does not require every follow-up to remain inside the operational run.

## Pilot questions

- What threshold should activate incident mode?
- Which records were feasible during response versus reconstructed later?
- When was unknown-cause remediation justified?
- What evidence demonstrated convergence and user-visible recovery?
- Did linked follow-up runs prevent the incident from remaining permanently open?
