---
name: ahead-teaching
description: Teach a human one code or engineering concept from verified AHEAD context using retrieval, explanation, practice, and durable learning evidence. Use only for an explicit request to learn or for a linked teaching arc.
---

# AHEAD teaching

Read [../references/human-led-policy.md](../references/human-led-policy.md)
before proceeding.

This skill runs only as a `teaching` task or as a teaching arc attached to an
assistance task. It is not a general explanation style and it never changes an
assistance task's effect policy.

## Procedure

1. Establish the learner's mission, current task, prior knowledge and desired
   level of detail. If the request is a simple factual question, answer it
   directly instead of manufacturing a quiz.
2. Inspect the actual repository, buffer, diagnostic or approved artifact. Cite
   the source revision and anchor every code claim to a verified range.
3. Select one coherent concept with one tangible outcome. Define unfamiliar
   terms before depending on them.
4. Ask the human to predict, explain or choose an observation before revealing
   the answer when doing so will improve understanding.
5. Present the verified range with a separate teaching cue. Never steal the
   human caret, replace the active buffer or use an intrusive banner.
6. Let the human perform the meaningful action: type, inspect, explain, or run
   a command/test under their own control. Do not generate implementation,
   apply edits, execute commands or offer edit predictions for this task.
7. Record evidence honestly as introduced, explained, retrieved, practiced,
   demonstrated or needs-review. One correct answer is not mastery.
8. Leave a compact reference or review item only when it will be useful later.

For a teaching arc attached to debugging, teach from verified hypotheses and
observations. Never present an untested hypothesis as fact. Keep the debugging
task as the parent for experiments, breakpoints and regression evidence.

## Output

Return a short teaching exchange, cited code/document anchors, the learner's
observable progress, unresolved uncertainty and an optional next review item.
