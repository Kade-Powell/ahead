# AHEAD

**Assisted Human Engineering and Development**

> Human-led engineering, amplified by AI.

AHEAD is a methodology for using AI in software engineering without transferring understanding, judgment, or accountability away from humans.

Its core loop is:

> Human thinks first → AI amplifies and challenges → Human decides.

AHEAD is being designed for several kinds of engineering work, including features, bugs, operational issues, incidents, refactoring, architecture decisions, technical debt, security issues, and investigations. These work types share principles, but they do not have to share one rigid workflow.

## Start here

- [Why AHEAD](docs/rationale.md) explains the reasoning behind the methodology.
- [Constitution](CONSTITUTION.md) records its non-negotiable principles.
- [Acceptable AI use](docs/acceptable-ai-use.md) defines appropriate, conditional, and prohibited uses of AI in engineering work.
- [Engineering practice](docs/engineering-practice.md) distills the general habits AHEAD expects engineers to cultivate.
- [Pragmatic Programmer page index](docs/references/pragmatic-programmer-page-index.md) preserves the edition-specific page references behind the submitted practice notes.
- [Submitted engineering notes](docs/references/submitted-engineering-notes.md) retain the additional tips, full checklists, and practices behind the distilled guide.
- [Pilot workflows](docs/workflows/README.md) provide minimal, diagrammed flows for product change, corrective debugging, operational stabilization, decision, investigation, and internal improvement.
- [Process taxonomy](docs/design/process-taxonomy.md) defines the six proposed workflow families and the test for adding another.
- [Debugging and operational investigation](docs/design/debugging-and-operations.md) captures the current design discussion; it is not yet a final workflow specification.
- [Evidence standard](docs/evidence/evidence-standard.md) defines what AHEAD means by evidence-backed.
- [Research map](docs/evidence/research-map.md) connects current design claims to empirical studies, standards, limitations, and open hypotheses.

## Status

AHEAD is in the methodology-design stage. Six minimal workflow profiles are ready for manual pilot use. The current priority is to apply them to real work and refine their phases, gates, and records before designing the workflow engine, editor extension, or CI enforcement.

No complete AHEAD workflow has yet been experimentally validated. The methodology distinguishes direct empirical support, adjacent evidence, standards, established practice, and AHEAD design hypotheses rather than presenting them as equally certain.

## What AHEAD is not

AHEAD is not:

```text
Prompt → AI designs → AI codes → human reviews AI output
```

It is:

```text
Human defines → AI researches → Human understands
→ AI expands and challenges → Human decides → Human plans
→ Engineer implements with AI assistance
→ AI reviews → Human reviews → Team learns
```
