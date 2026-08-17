# TigerStyle Practitioner Source

Audience: AHEAD evidence readers and framework maintainers

Status: reviewed practitioner source

## Source and provenance

[TigerStyle](https://tigerstyle.dev/) is TigerBeetle's published software-engineering methodology for advancing safety, performance, and developer experience in its distributed financial database. TigerBeetle is both the author of the source and the organization reporting the practice. The source describes a coherent operating philosophy and concrete coding rules; it is not an independent evaluation of their outcomes.

Under the [AHEAD Evidence Standard](../evidence-standard.md), TigerStyle is **E4 — documented practitioner evidence**. It can demonstrate a feasible practice, supply design heuristics, and identify failure modes. It does not by itself establish that a rule causes better outcomes or generalizes beyond its environment.

## Context

TigerStyle is shaped by foundational infrastructure with demanding correctness, durability, predictability, and performance requirements. Its concrete prescriptions also reflect Zig, explicit memory management, distributed state-machine design, simulation testing, and tight control over the software stack.

That context makes the source especially relevant to infrastructure, embedded, distributed, real-time, storage, networking, and other safety- or performance-sensitive systems. It also limits direct transfer to managed runtimes, user-interface code, ordinary business applications, scripts, prototypes, and systems with different fault models or economics.

## Themes adapted by AHEAD

AHEAD draws the following bounded practitioner lessons from TigerStyle:

- consider relevant quality attributes while shaping a design rather than relying on final inspection;
- make material limits, resource budgets, fault models, and invariants explicit;
- use proportionate defenses in depth, including executable checks, without treating automation as a substitute for understanding;
- examine both expected and invalid states and distinguish programmer errors from expected operating errors;
- treat interface quality, failure semantics, defaults, state locality, and semantic distance as system concerns;
- make rough performance estimates across network, storage, memory, and compute, considering latency and bandwidth, then validate them through measurement;
- evaluate dependencies and tools by their lifecycle, operational, performance, comprehension, and supply-chain costs;
- optimize names, rationale, documentation, and design for the people who will repeatedly read, review, operate, diagnose, and change the system;
- address material design risks while context is fresh and keep accepted engineering debt visible and owned.

These themes inform the [AHEAD Engineering Practice](../../guide/engineering-practice.md). The Constitution adopts only the durable, technology-neutral principle that relevant quality attributes should be designed, bounded, and verified.

## Contextual techniques, not universal AHEAD rules

AHEAD does not generalize TigerStyle's concrete requirements to all software. Examples that remain contextual include:

- prohibiting recursion;
- allocating all memory at startup and prohibiting later allocation;
- preferring fixed-width integers over architecture-specific types in all cases;
- requiring a fixed assertion density;
- imposing exact function- or line-length limits;
- scheduling reactions to external events at fixed intervals;
- separating control and data planes through batching;
- avoiding serialization, copying, or movable data structures;
- requiring cache-line-aligned layouts;
- prohibiting third-party dependencies;
- standardizing all tooling on one language;
- mandating TigerBeetle's naming and formatting conventions.

These techniques may be excellent choices when justified by a system's risk, workload, language, hardware, fault model, and operating environment. AHEAD requires the reasoning and evidence to remain visible; it does not prescribe the answer in advance.

## Important limitations and tensions

- TigerStyle orders safety, performance, and developer experience for TigerBeetle. AHEAD treats quality priorities as contextual and human-owned.
- TigerStyle advocates “zero technical debt” and doing work right the first time. AHEAD instead requires material debt to be explicit, owned, contained, and revisited; it preserves experimentation and revisable decisions.
- TigerStyle often treats process termination as the correct response to a violated program invariant. AHEAD requires failure behavior to follow the system's fault model and blast radius.
- Early performance sketches can expose architectural constraints but do not replace representative measurement.
- Assertions, fuzzing, simulation, types, and tests can reveal defects but do not prove correctness or replace a maintained human mental model.
- The source reports TigerBeetle's own practice and has not been evaluated here through controlled comparison or independent replication.

## Resulting AHEAD decision

AHEAD cites TigerStyle as an E4 practitioner source, adapts selected technology-neutral themes, and labels concrete systems techniques as contextual. Future claims that a particular TigerStyle-derived rule should become mandatory require direct evidence, strong professional consensus, or favorable AHEAD pilot results under the evidence standard.
