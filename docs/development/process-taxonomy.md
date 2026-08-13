# AHEAD Process Taxonomy

Audience: AHEAD framework and tooling maintainers

Status: proposed design
Last reviewed: 2026-08-12

## Why classify by outcome

AHEAD should not create a workflow for every issue label. “Security,” “performance,” “data,” “incident,” and “technical debt” often describe risk, domain, urgency, or cause—not the kind of reasoning needed to complete the work.

The primary workflow should be selected by the **dominant outcome** the human is trying to produce. Variants and overlays then adapt that workflow to context.

This gives AHEAD six proposed process families. All six now have minimal [pilot workflow profiles](../guide/workflows/README.md) for use and evaluation; that does not yet validate the taxonomy or justify automated enforcement.

## The six process families

| Process family | Dominant question | Terminal outcome | Examples | Status |
|---|---|---|---|---|
| 1. [Product change](../guide/workflows/product-change.md) | What behavior or capability should exist, and how should we deliver it? | Verified intended behavior | Feature, API change, integration, migration, dependency adaptation, decommission | Pilot v0.1 |
| 2. [Corrective debugging](../guide/workflows/corrective-debugging.md) | Why does observed behavior differ from intended behavior, and how should we correct it? | Verified correction or explicitly accepted uncertainty | Deterministic bug, flaky failure, regression, incorrect data processing | Pilot v0.1 |
| 3. [Operational stabilization](../guide/workflows/operational-stabilization.md) | Why is a live system outside an acceptable operating state, and how do we restore and stabilize it? | Demonstrated recovery/convergence and follow-up disposition | Reconciliation storm, capacity exhaustion, configuration drift, dependency outage | Pilot v0.1 |
| 4. [Decision](../guide/workflows/decision.md) | Which course should humans choose, given goals, evidence, constraints, and tradeoffs? | Approved decision and rationale | Architecture decision, buy/build, technology selection, policy or platform choice | Pilot v0.1 |
| 5. [Investigation](../guide/workflows/investigation.md) | What is true, feasible, or likely when no intervention has yet been selected? | Bounded conclusion, confidence, evidence, and remaining unknowns | Technical spike, feasibility study, causal follow-up, capacity study, vendor evaluation | Pilot v0.1 |
| 6. [Internal improvement](../guide/workflows/internal-improvement.md) | How can we improve system qualities while preserving an explicit behavioral contract? | Verified invariants plus improved target qualities | Refactor, preventive maintenance, maintainability debt, performance optimization without semantic change | Pilot v0.1 |

Six is a working taxonomy, not a sacred number. The threshold for adding a seventh family is deliberately high.

## Selection test

```text
Is the primary outcome new or changed externally meaningful behavior?
  → Product change

Is an observed behavior wrong and the main work is causal diagnosis plus correction?
  → Corrective debugging

Is a live system unhealthy, unstable, or failing to converge, with restoration as the immediate outcome?
  → Operational stabilization

Is the deliverable an accountable choice among alternatives?
  → Decision

Is the deliverable knowledge or reduced uncertainty, without a predetermined change?
  → Investigation

Must behavior remain invariant while internal qualities improve?
  → Internal improvement
```

A large effort may link several runs. An architecture decision can lead to a product change. An incident can create an operational investigation, a corrective bug, and an internal-improvement follow-up. A technical spike can end in a decision without pretending that knowledge production and option selection are the same activity.

## Why the additional three differ

### Decision

A feature includes decisions, but some engineering work ends with a decision rather than code. Its quality depends on framing, option coverage, evidence, tradeoffs, consequences, reversibility, and accountable approval. Forcing it through implementation and deployment creates meaningless states.

### Investigation

An investigation begins with a question, not an assumed defect or desired change. It may conclude that no action is needed, evidence is insufficient, a vendor owns the behavior, or several interventions remain viable. Its terminal quality is epistemic: evidence, confidence, limitations, and unknowns.

### Internal improvement

Refactoring and preventive work are judged differently from feature work. They begin by specifying invariants and target qualities. Success means that required behavior was preserved while maintainability, performance, safety, comprehensibility, or another quality improved. Treating this as a feature encourages invented product outcomes; treating it as a bug assumes a failure that may not exist.

## Overlays, not primary processes

### Incident mode

Incident mode represents urgency, impact, coordination, containment, communication, and recovery. It can overlay corrective debugging, operational stabilization, a security event, or a data issue. It relaxes nonessential documentation during response but strengthens action authorization and decision logging.

### Security

Security adds confidentiality, evidence preservation, threat modeling, restricted AI access, disclosure, and security approval. A vulnerability may use corrective debugging; proactive hardening may use internal improvement; an active compromise may use incident-mode operational stabilization; a threat assessment may use investigation.

### Safety, regulatory, and compliance

These overlays strengthen traceability, independence, evidence retention, required reviewers, and non-waivable gates. They do not change whether the underlying work is a change, correction, operation, decision, investigation, or improvement.

### Emergency

Emergency handling changes sequencing and permits explicitly governed deferrals. It does not erase human accountability or evidence requirements; it moves some reconstruction and learning after stabilization.

## Labels and modifiers

Context belongs in typed modifiers rather than new workflow definitions:

```yaml
process: operational-stabilization
urgency: incident
domain: infrastructure
assurance: standard
failure_character: intermittent
environment: production
data_classification: internal
```

Useful modifiers may include:

- urgency: normal, expedited, incident, emergency;
- assurance: standard, security, safety-critical, regulated;
- environment: local, test, staging, production, external provider;
- failure character: deterministic, intermittent, performance, data, distributed, unknown;
- change character: additive, adaptive, migration, retirement;
- reversibility and blast radius;
- evidence sensitivity.

## Where common work maps

| Work label | Primary process or routing rule |
|---|---|
| Feature | Product change |
| Bug | Corrective debugging |
| Production reconciliation storm | Operational stabilization; add incident mode when impact/urgency warrants it |
| Architecture decision | Decision; link resulting implementation separately |
| Technical debt | Internal improvement when preserving behavior; product change when behavior changes; corrective debugging when it represents a known defect |
| Refactor | Internal improvement |
| Performance regression | Corrective debugging |
| Proactive performance optimization | Internal improvement or product change, depending on whether performance is a new product outcome |
| Security vulnerability | Corrective debugging plus security overlay |
| Active security compromise | Operational stabilization plus incident and security overlays |
| Security hardening | Internal improvement or product change plus security overlay |
| Research spike | Investigation |
| Compliance audit | Investigation plus compliance overlay; corrective or improvement runs handle findings |
| Dependency or platform upgrade | Product change with adaptive-change modifier |
| Service retirement | Product change with retirement and risk modifiers |

## Test for adding another family

A new primary process family should be added only when all of these are true:

1. It has a distinct terminal outcome.
2. It has a distinct central reasoning loop.
3. It requires materially different human decisions or gates.
4. It cannot be represented clearly as a variant, overlay, modifier, or linked combination of existing families.
5. Evidence or repeated practice shows that using an existing family creates confusion, unsafe behavior, or process theater.
6. The additional cognitive and tooling cost is justified.

## Evidence basis and limits

[ISO/IEC/IEEE 12207:2026](https://standards.ieee.org/ieee/12207/11416/) covers development, operation, maintenance, support, and retirement and allows processes to operate concurrently, iteratively, and recursively. [ISO/IEC/IEEE 14764:2022](https://www.iso.org/standard/80710.html) separately establishes software-maintenance types. These standards support broad coverage and composition, but they do not validate this six-family taxonomy.

Empirical debugging research supports a mental-model and hypothesis-testing process distinct from planned change. Empirical production-incident research distinguishes code and non-code causes and separates detection, investigation, and mitigation. Those findings support keeping corrective debugging and operational stabilization separate.

The proposed six-family classification itself remains an AHEAD design hypothesis. The pilot profiles should be tested against a diverse sample of real engineering work by asking whether teams can route work consistently, whether important states or gates differ, which records improve reasoning or handoff, and whether any family is rarely used or routinely misclassified.
