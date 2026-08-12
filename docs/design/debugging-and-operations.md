# Debugging and Operational Investigation

Status: design discussion, not an approved workflow specification

The minimal [corrective-debugging](../workflows/corrective-debugging.md) and [operational-stabilization](../workflows/operational-stabilization.md) profiles translate this discussion into pilotable flows. This document retains the reasoning and unresolved questions behind them.

## Human ownership

Debugging is human-owned. AI may help collect and organize evidence, suggest hypotheses and tests, identify contradictions, explain systems, and challenge conclusions. The human chooses what to investigate, performs or authorizes tests, interprets the evidence, selects interventions, and accepts the conclusion or remaining uncertainty.

There is no “AI investigation” phase. Investigation is a human engineering activity in which AI may participate.

## Shared reasoning loop

```text
Observation
→ Characterize
→ Build or update the mental model
→ Generate hypotheses
→ Human selects a discriminating test
→ Predict expected results
→ Run the test
→ Record evidence
→ Update or refute hypotheses
→ Repeat
```

The process distinguishes:

- **Fact:** directly observed and linked to evidence.
- **Inference:** an interpretation derived from facts.
- **Hypothesis:** a falsifiable proposed explanation.
- **Test:** an experiment or observation capable of changing confidence in a hypothesis.
- **Result:** what the test actually produced.
- **Conclusion:** a human-accepted explanation with confidence, limits, and remaining uncertainty.

Predictions should be recorded before a test when practical. This reduces hindsight interpretation of ambiguous results.

## Bug debugging

A bug is a defect where observed software behavior conflicts with intended behavior.

```text
Observe → Reproduce or establish
→ Characterize
→ Human-led investigation loop
→ Human accepts diagnosis or uncertainty
→ Choose fix → Plan → Implement → Validate locally
→ AI review → Independent human review
→ Human-authorized deploy or release when applicable
→ Verify the original failure and observe the outcome
→ Audit assumptions → Human outcome
```

Reproduction is valuable but not universally required. A failure may be intermittent, historical, production-only, environment-specific, or already mitigated.

## Operational investigation

An operational issue is undesirable system behavior that may not be a software defect. Examples include reconciliation storms, configuration drift, capacity exhaustion, cloud-provider behavior, dependency failures, identity or certificate failures, resource contention, bad rollout sequencing, and emergent controller interactions.

```text
Observed condition
→ Desired state versus actual state
→ Impact and scope
→ Timeline
→ System and control-loop model
→ Recent changes and external events
→ Evidence/hypothesis/test loop
→ Intervention decision
→ Verify convergence and user-visible behavior
→ Monitor recurrence
→ Corrective actions
```

The initial cause classification must remain tentative. Labeling an issue a code bug, configuration error, or provider failure before collecting evidence can bias the investigation.

## Incident mode

“Incident” describes impact and urgency rather than cause. An incident may be caused by a bug, configuration, capacity, provider behavior, a security event, data, or an interaction that remains partially unexplained.

Incident mode adds parallel concerns:

```text
Response:       assess impact → contain → recover → monitor
Investigation:  evidence → model → hypotheses → tests → conclusion
Coordination:   ownership → decisions → communications → timeline
```

Mitigation and recovery must not be blocked on completing a full diagnosis. Risky interventions still require explicit human authorization. After recovery, unresolved causal analysis and prevention work may continue as a linked bug, operational investigation, security issue, or technical-debt item.

## Current design direction

- Treat bug debugging and operational investigation as separate work types.
- Treat incident mode as an overlay that may apply to several work types.
- Keep the evidence and hypothesis loop flexible rather than gating every iteration.
- Reserve hard gates for human accountability, risky tests, consequential interventions, accepted conclusions, implementation plans, reviews, and verified outcomes.
- Distinguish hypothesis testing, fix validation, and post-deployment outcome verification.
- Allow causal conclusions to include a failure mechanism, trigger, enabling conditions, and detection or containment gaps instead of insisting on one root cause.

## Open questions

- What minimum evidence is needed before a human may accept a diagnosis?
- When may a team remediate while explicitly accepting that the cause is unknown?
- Which tests require approval based on environment, reversibility, or blast radius?
- How should AHEAD represent multiple interacting causes and confidence changes?
- When does an operational anomaly become incident mode?
- Which incident records must be produced during response, and which may be reconstructed afterward?
- How should follow-up work remain linked without keeping the incident itself permanently open?

## Evidence basis

The current reasoning loop is supported by direct empirical software-engineering research, though the exact AHEAD recording requirements are not yet validated:

- [Li and Coblenz, *A Grounded Theory of Debugging in Professional Software Engineering Practice*](https://arxiv.org/abs/2602.11435) observed professional developers and describes debugging as iterative mental-model construction that guides information gathering.
- [Alaboudi and LaToza, *Using Hypotheses as a Debugging Aid*](https://doi.org/10.1109/VL/HCC50065.2020.9127273) found that early correct hypotheses predicted success and that supplying potential hypotheses helped more than supplying fault locations in their controlled experiment.
- [Sillito and Kutomi, *Failures and Fixes*](https://doi.org/10.1109/ICSME46990.2020.00027) analyzed 30 incidents and identified distinct investigative and mitigative strategies.
- [Ghosh et al., *How to Fight Production Incidents?*](https://doi.org/10.1145/3542929.3563482) studied hundreds of high-severity cloud incidents, including non-code causes, across detection, diagnosis, and mitigation.

These studies support the shape of the process. They do not prove that requiring engineers to record every fact, hypothesis, or test improves results. AHEAD must test the minimum useful structure and remove requirements that interrupt investigation without improving reasoning, handoff, or learning.
