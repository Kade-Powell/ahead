---
name: ahead-prototype
description: Answer a bounded feasibility question with the smallest safe experiment in an isolated or explicitly disposable scope. Use when the human needs evidence that an approach works in this codebase before designing production behavior.
---

# AHEAD prototype

Read [../references/human-led-policy.md](../references/human-led-policy.md)
before proceeding.

State the question, success signal, constraints, disposable scope and cleanup
plan before experimenting. Prefer a tiny fixture, test harness or temporary
workspace over production edits. The human chooses whether the experiment is
worth running and what result would change the design.

Record the exact revision, environment, procedure, result and limitations.
Separate “this experiment worked” from “the production design is accepted.”
Remove disposable artifacts or clearly mark them as temporary. Do not turn a
prototype into production code or a business decision without a new human-led
design step.
