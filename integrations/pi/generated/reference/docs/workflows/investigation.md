# Investigation Workflow

Status: pilot v0.1

## Outcome

Use this flow when the primary deliverable is knowledge or reduced uncertainty and no intervention has yet been selected. It may answer feasibility, causal, capacity, vendor, architectural, or technical questions.

An investigation may end with no action, insufficient evidence, several viable interventions, or work routed to another flow.

## Lifecycle

```text
┌──────────────────────────────────────────────┐
│ 1. FRAME QUESTION                            │
│                                              │
│ HUMAN                                        │
│ • State the question and why it matters      │
│ • Record initial understanding               │
│ • Name audience or dependent decision        │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 2. BOUND INVESTIGATION                       │
│                                              │
│ HUMAN                                        │
│ • Define scope, exclusions, confidence need  │
│ • Set time/evidence budget and stopping rule │
│                                              │
│ AI — ASSIST                                  │
│ • Challenge whether it is answerable         │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 3. GATHER EVIDENCE                           │
│                                              │
│ AI — ASSIST                                  │
│ • Locate, summarize, compare, and cite       │
│ • Expose conflicting or missing evidence     │
│                                              │
│ HUMAN                                        │
│ • Evaluate source quality and relevance      │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 4. HUMAN MODEL                               │
│ • Build/update the explanation of the domain │
│ • Record assumptions and unknowns            │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 5. EXPLORATION PATHS                         │
│                                              │
│ HUMAN                                        │
│ • Contribute/select what is worth exploring  │
│                                              │
│ AI — ASSIST                                  │
│ • Generate hypotheses and alternatives       │
│ • Suggest analyses, tests, or prototypes     │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 6. HUMAN SELECTS AND AUTHORIZES              │
│ • Test / analysis / disposable prototype     │
│ • Prediction, safety, scope, and disposal    │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 7. EXECUTE EXPLORATION                       │
│                                              │
│ HUMAN / ENGINEER OWNS THE EXPERIMENT         │
│ AI — ASSIST                                  │
│ • May fully generate an isolated throwaway   │
│   prototype under prototype policy           │
└──────────────────────┬───────────────────────┘
                       ↓
                ┌───────────────┐
                │ STOPPING      │
                │ CONDITION     │
                │ REACHED?      │
                └───────┬───────┘
                   NO ↙   ↘ YES
           ↺ MODEL / PATHS  │
                            ▼
┌──────────────────────────────────────────────┐
│ 8. SYNTHESIZE                                │
│                                              │
│ HUMAN                                        │
│ • Interpret evidence, contradictions, limits │
│                                              │
│ AI — ASSIST                                  │
│ • Organize findings and challenge overclaims │
└──────────────────────┬───────────────────────┘
                       ↓
┌──────────────────────────────────────────────┐
│ 9. HUMAN CONCLUSION / OUTCOME GATE           │
│ • State conclusion, confidence, and unknowns │
│ • Recommend no action or link another flow   │
└──────────────────────────────────────────────┘
```

Full vibe coding is permitted for an isolated, explicitly disposable prototype when its purpose is learning. The learning is retained; prototype code is discarded or deliberately re-enters the appropriate production workflow.

Stopping conditions may be satisfied by enough evidence for a conclusion, exhaustion of the agreed time or evidence budget, discovery that the question is not currently answerable, or another human-approved boundary. Therefore “insufficient evidence” can advance to synthesis and an honest conclusion rather than forcing an endless loop.

## Minimal phases

| Phase | Human owns | AI may | Minimum record | Advance when |
|---|---|---|---|---|
| Frame | Question, decision relevance, audience, scope, and initial understanding | Clarify ambiguity and identify adjacent questions | Primary question and why it matters | Human accepts the question |
| Bound | Confidence needed, exclusions, time or evidence budget, and stopping conditions | Challenge whether the question is answerable | Scope, limits, and stopping rule | Investigation can proceed without unbounded research |
| Gather | Source evaluation and evidence access | Locate, summarize, compare, cite, and expose conflicts | Evidence links, source quality, and observations | Relevant available evidence is assembled |
| Model and explore | Mental model, path selection, test authorization, and interpretation | Generate hypotheses, analysis approaches, or prototype ideas | Model, assumptions, hypotheses, tests, and results | Evidence is sufficient for synthesis or budget is reached |
| Synthesize | Meaning, confidence, contradictory evidence, and limitations | Organize findings and challenge overclaiming | Findings, confidence, limits, and unknowns | Human can state a bounded conclusion |
| Conclude and route | Accepted conclusion, recommendation, and next workflow | Suggest follow-up questions or routes | Conclusion and linked decision/change/debug/improvement runs | Human accepts closure or explicitly extends the investigation |

## Knowledge chain

```text
HUMAN QUESTION / RELEVANCE
           │
           ▼
HUMAN SCOPE / STOPPING RULE
           │
           ▼
AI-ASSISTED EVIDENCE GATHERING
HUMAN SOURCE EVALUATION
           │
           ▼
HUMAN MODEL / ASSUMPTIONS
           │
           ▼
AI-EXPANDED EXPLORATION PATHS
HUMAN-SELECTED TEST / ANALYSIS / PROTOTYPE
           │
           ▼
RESULTS / CONTRADICTIONS
           │
           ▼
HUMAN CONCLUSION / CONFIDENCE / LIMITS
           │
           ▼
HUMAN RECOMMENDATION / LINKED RUN
```

## Non-waivable pilot rules

- The question is human-owned; AI does not quietly redefine it.
- Facts, retrieved claims, inference, and AI synthesis remain distinguishable.
- Important citations are followed to authoritative sources.
- A prototype demonstrates only what it actually tested.
- “Insufficient evidence” is a valid result.

## Pilot questions

- Were the stopping conditions usable or did research expand indefinitely?
- Did the conclusion state confidence and contradictory evidence honestly?
- Did a disposable prototype answer the learning question without leaking into production?
- Was the next workflow clear when the investigation ended?
