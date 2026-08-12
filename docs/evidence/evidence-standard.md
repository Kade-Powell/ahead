# AHEAD Evidence Standard

Status: proposed

## Purpose

AHEAD should be supported by the best available evidence without claiming more certainty than software-engineering research can provide. A credible methodology must distinguish evidence for a descriptive claim from evidence that a particular prescribed workflow improves outcomes.

No paper should be cited merely because its title resembles an AHEAD principle. For every material process rule, AHEAD records what was studied, how it was studied, what outcome was observed, how directly it applies, and what remains unknown.

## Evidence classes

### E1 — Direct empirical software-engineering evidence

Experiments, field experiments, observational studies, repository studies, or qualitative studies of professional software work that directly examine the behavior or outcome in question.

Examples: programmers debugging real code, developers using AI coding tools, or production responders handling incidents.

### E2 — Adjacent empirical evidence

Empirical research on a relevant cognitive, organizational, safety, or creative process outside the precise software-engineering context.

Adjacent evidence can motivate an AHEAD hypothesis. It does not prove that the same intervention will work in software engineering.

### E3 — Consensus standard or authoritative guidance

Standards and institutional guidance such as ISO/IEC/IEEE life-cycle standards or NIST incident-response publications. These establish vocabulary, expected controls, and professional consensus. They are not necessarily controlled evidence that one process outperforms another.

### E4 — Documented practitioner evidence

Published operating practices, case reports, experience reports, or durable methods from organizations and practitioners. This evidence can reveal feasible practices and important failure modes, but is vulnerable to selection bias and local context.

Toyota's human-centered automation philosophy belongs here.

### E5 — AHEAD design hypothesis

A plausible process rule derived from principles, indirect evidence, or design judgment but not directly validated. It must be labeled as a hypothesis and paired with an evaluation plan before being promoted as evidence-backed.

The requirement that a human always produce the first option and first-pass plan is currently E5, with partial E2 support related to AI anchoring and diversity. It is a normative AHEAD commitment, but its exact implementation still needs evaluation.

## Required evidence record

Each material process claim should record:

```yaml
claim: Human-originated options should precede AI-generated alternatives.
status: design-hypothesis
evidence_class: [E2, E5]
sources:
  - study: Generative AI enhances individual creativity but reduces collective diversity
    population: 293 short-story writers
    result: AI ideas improved evaluated individual output, but outputs became more similar across participants
applicability: Indirect evidence of anchoring and convergence; not a software-design study.
counterevidence: AI ideas improved several individual quality measures.
decision: Pilot human-first option capture, then measure option diversity and decision quality.
owner: methodology maintainers
review_date: 2027-01-31
```

At minimum, a record includes:

- precise claim;
- evidence class;
- source and study design;
- population, tasks, and environment;
- observed outcome;
- applicability to AHEAD;
- limitations and counterevidence;
- resulting process decision;
- confidence and review date;
- proposed validation measure for an AHEAD hypothesis.

## Source policy

- Prefer peer-reviewed primary research for scientific claims.
- Prefer the original standard or issuing institution for normative guidance.
- Use systematic reviews to understand a field, then cite primary studies for specific claims when practical.
- Use vendor or practitioner reports as E4 and disclose the source's interest and context.
- Do not convert correlation into causation.
- Do not generalize from students to professionals, small exercises to mature systems, or one organization to all engineering without stating the limitation.
- Preserve null, mixed, and contrary results.
- Re-check AI evidence frequently because tools, adoption skill, and work patterns change quickly.

## From evidence to process

Evidence rarely supplies a complete workflow. AHEAD uses this chain:

```text
Observed evidence
→ bounded claim
→ applicability analysis
→ proposed process mechanism
→ pilot
→ measured outcome
→ retain, revise, or remove
```

A process rule should become mandatory only when:

1. it protects a constitutional value or meaningful risk;
2. its mechanism is explicit;
3. the burden is proportionate to that risk;
4. there is direct evidence, strong professional consensus, or favorable AHEAD pilot data;
5. exceptions can be handled safely and visibly;
6. the team can measure whether the rule is producing its intended result rather than paperwork.

## Evaluation outcomes

AHEAD should evaluate more than delivery speed:

- correctness and escaped defects;
- decision quality and reversals;
- option and hypothesis diversity;
- time to understanding, diagnosis, mitigation, and recovery;
- operator and reviewer ability to explain the system and change;
- traceability from intent through outcome;
- review findings and their novelty;
- recurrence and corrective-action completion;
- workflow burden and bypass rate;
- perceived speed compared with measured speed;
- learning and retention over time.

The methodology should publish negative findings. If a gate adds delay without improving understanding, decision quality, safety, or learning, it should be changed or removed.
