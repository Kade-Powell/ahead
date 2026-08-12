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
- [Debugging and operational investigation](docs/design/debugging-and-operations.md) captures the current design discussion; it is not yet a final workflow specification.

## Status

AHEAD is in the methodology-design stage. The current priority is to understand the human engineering processes worth supporting before designing the workflow engine, editor extension, or CI gates.

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

