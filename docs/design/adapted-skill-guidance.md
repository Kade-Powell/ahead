# Adapted Skill Guidance

Status: approved first adaptation set, 2026-08-12

AHEAD reviewed the [Matt Pocock skills collection](https://www.skills.sh/mattpocock/skills), including the specifically discussed [grill-me](https://www.skills.sh/mattpocock/skills/grill-me), [prototype](https://www.skills.sh/mattpocock/skills/prototype), and [ask-matt](https://www.skills.sh/mattpocock/skills/ask-matt) skills. It also reviewed [Ponytail](https://github.com/DietrichGebert/ponytail/tree/2ed6c52c9d7e5e56942508591085fd45dea277d3/skills/ponytail). The useful ideas were treated as design input, not installed wholesale or made authoritative.

## Adapted into AHEAD now

- Guided questioning became a dependency-frontier method: AI discovers facts, humans answer consequential judgment questions, and rounds scale with risk.
- Research became provenance inside existing workflow artifacts: primary sources, direct observations, contradictions, applicability, and uncertainty.
- Planning became human-first vertical decomposition with dependencies, acceptance criteria, rollout, recovery, and expand-migrate-verify-contract stages for broad changes.
- Debugging gained tighter safe feedback loops, minimized reproductions, ranked falsifiable hypotheses, one-variable probes, tagged instrumentation, regression evidence, and an explicit no-safe-reproduction path.
- Prototyping became an Investigation technique with an explicit learning question, deliberately disposable code, visible outcomes, and mandatory human disposition. Prototype code cannot be promoted directly.
- Review became an exact-changeset workbench with structured AI findings, a separate implementing-human disposition, and later independent human judgment.
- Instruction design became progressive disclosure: binding profile, applicable phase and method fragments, then on-demand framework references.

These adaptations live in AHEAD's own workflow specs, `policy/methods`, and host-neutral contracts. They require only artifacts already justified by an AHEAD phase; no source skill's private artifact system or issue format was imported.

## Deliberately not adopted

- Persona emulation such as “ask Matt” is not framework authority or a substitute for project evidence.
- No skill may skip the human's initial model, decision, plan, understanding, tests, or review.
- A quick, lazy, or persistent implementation is not production-ready merely because it runs.
- AHEAD does not silently install third-party skills, create extra artifacts, post GitHub comments, create issues, push code, or mark a pull request ready.
- Issue intake, domain or architecture specialization, formal understanding handoff, and wayfinding remain deferred until their AHEAD-native process is separately approved.

The recommendation catalog records external code AHEAD suggests installing. The method overlays record guidance AHEAD owns. Keeping those separate allows a project to use any editor or model without depending on a particular skill package.
