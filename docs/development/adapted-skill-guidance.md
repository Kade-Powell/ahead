# Adapted Skill Guidance

Audience: AHEAD framework and tooling maintainers

Status: approved first adaptation set, 2026-08-12

AHEAD reviewed the [Matt Pocock skills collection](https://www.skills.sh/mattpocock/skills) at revision `885e2ca4d842d139e9aef4e48d366c63cb1b8013`, including the specifically discussed [grill-me](https://www.skills.sh/mattpocock/skills/grill-me), [prototype](https://www.skills.sh/mattpocock/skills/prototype), and [ask-matt](https://www.skills.sh/mattpocock/skills/ask-matt) skills. It also reviewed [Ponytail](https://github.com/DietrichGebert/ponytail/tree/2ed6c52c9d7e5e56942508591085fd45dea277d3/skills/ponytail). The useful ideas were treated as design input, not installed wholesale or made authoritative.

## Adapted into AHEAD now

- Guided questioning became a dependency-frontier method: AI discovers facts, humans answer consequential judgment questions, and rounds scale with risk.
- Research became provenance inside existing workflow artifacts: primary sources, direct observations, contradictions, applicability, and uncertainty.
- Planning became human-first vertical decomposition with dependencies, acceptance criteria, rollout, recovery, and expand-migrate-verify-contract stages for broad changes.
- Debugging gained tighter safe feedback loops, minimized reproductions, ranked falsifiable hypotheses, one-variable probes, tagged instrumentation, regression evidence, and an explicit no-safe-reproduction path.
- Prototyping became an Investigation technique with an explicit learning question, deliberately disposable code, visible outcomes, and mandatory human disposition. Prototype code cannot be promoted directly.
- Review became an exact-changeset workbench with structured AI findings, a separate implementing-human disposition, and later independent human judgment.
- Instruction design became progressive disclosure: binding profile, applicable phase and method fragments, then on-demand framework references.

The binding adaptations live in AHEAD's own workflow specs, `policy/methods`, and host-neutral contracts. They require only artifacts already justified by an AHEAD phase; no source skill's private artifact system or issue format was imported.

## Bundled AHEAD-owned skills

AHEAD also maintains `research`, `to-tickets`, and `diagnosing-bugs` as thin Agent Skill interfaces over those contracts. Supported host packages copy them from the repository-root `skills/` directory and expose them through the host's normal progressive-disclosure mechanism.

These are AHEAD-owned adaptations, not vendored upstream skills. Their pinned source revision, adapted hashes, and MIT notices live under `skills/`. The active workflow spec and generated phase policy remain authoritative: a bundled skill cannot create or accept a gate, advance a phase, authorize implementation, or weaken human ownership. Projects provide their own domain and tracker configuration instead of copying these generic skills into each repository.

`diagnosing-bugs` deliberately uses AHEAD ordering. The human characterizes the failure and records a falsifiable initial model before AI expands the hypothesis set; reproduction and minimization then precede AI-generated hypotheses and probes. This differs from the upstream skill's absolute reproduction-before-any-theory gate while retaining its useful tight-loop discipline.

## Deliberately not adopted

- Persona emulation such as “ask Matt” is not framework authority or a substitute for project evidence.
- No skill may skip the human's initial model, decision, plan, understanding, tests, or review.
- A quick, lazy, or persistent implementation is not production-ready merely because it runs.
- AHEAD does not silently install third-party skills, create extra artifacts, post GitHub comments, create issues, push code, or mark a pull request ready.
- Issue intake, domain or architecture specialization, formal understanding handoff, and wayfinding remain deferred until their AHEAD-native process is separately approved.

The recommendation catalog records external code AHEAD suggests installing and remains strictly opt-in. Method overlays record binding guidance AHEAD owns. Bundled skills are optional host-facing views of that owned guidance, not another authority layer. Keeping all three categories explicit lets projects use other editors or models without depending on a particular skill package or confusing reviewed third-party code with AHEAD-owned policy.
