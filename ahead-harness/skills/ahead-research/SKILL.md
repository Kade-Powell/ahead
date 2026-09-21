---
name: ahead-research
description: Answer a bounded external or repository question by following primary sources, recording citations and leaving a reviewable AHEAD research artifact. Use when a decision depends on facts outside the current conversation.
---

# AHEAD research

Read [../references/human-led-policy.md](../references/human-led-policy.md)
before proceeding.

Research is delegated fact-finding, not delegated decision-making. The human
still decides what the evidence means and whether to adopt it.

## Procedure

1. State one narrow, answerable question, the date/version boundary and why it
   matters to the current task.
2. Prefer primary sources: official documentation, specifications, source code,
   first-party APIs, release notes and the actual repository. Label community
   or secondary material as context rather than authoritative fact.
3. Follow each material claim to the source that owns it. Record URL/path,
   revision or retrieval date and the exact scope of the claim.
4. Stop when the question is answered or the evidence is contradictory. Do not
   expand into an unbounded survey.
5. Write one cited Markdown research artifact in the current AHEAD artifact
   convention. Mark stale or time-sensitive findings explicitly.
6. Return the artifact path and the unresolved decision points; do not silently
   edit code, update a tracker or turn findings into an ADR.

Do not recursively delegate research. Do not treat five uncited summaries as
stronger evidence than one inspected primary source. Do not load an old
research artifact as current without checking its source dates and revisions.

Teaching tasks may use the resulting sources to build a lesson. Design and
debugging tasks may attach the artifact as evidence, but neither inherits a
decision from it automatically.
