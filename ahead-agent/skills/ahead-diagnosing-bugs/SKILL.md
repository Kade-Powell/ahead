---
name: ahead-diagnosing-bugs
description: Diagnose a confirmed hard bug, intermittent failure, regression, or performance problem through a red reproduction loop, minimization, falsifiable hypotheses, selected experiments, and regression evidence. Use inside an assistance task when the cause is unknown.
---

# AHEAD diagnosing bugs

Read [../references/human-led-policy.md](../references/human-led-policy.md)
before proceeding.

This is a heavy assistance workflow. Do not activate it for a quick factual
question, a known small fix, a codebase audit or an unconfirmed report that
first needs triage.

## Procedure

1. Capture expected versus observed behavior, scope, impact, environment and
   the human's current model.
2. Build one named red loop: a test, command, fixture, request, browser probe,
   replay or other deterministic check that fails for the reported symptom.
   Do not form a confident theory before this signal exists.
3. Minimize the loop and improve a flaky reproduction rate when necessary.
4. Present three to five ranked hypotheses. Each must include a prediction
   that could disprove it. The human chooses which experiment to run.
5. Prepare the selected breakpoint, fixture, logpoint, instrumentation or
   approved check. The host and human control whether it runs.
6. Record actual observations against the target revision and environment.
   Update the model before changing code.
7. Once the cause is supported, let the human choose the correction and
   regression expectation. Apply only explicitly bounded supporting edits.
8. Rerun the original red loop and relevant checks, then remove temporary
   instrumentation and preserve the evidence.

If no tight red loop can be built, stop and state what is missing: environment
access, a captured artifact, an observable seam, or permission to add temporary
instrumentation. Do not invent a mock reproduction merely to continue.

Redact tokens, credentials, cookies, personal data and unrelated payloads before
putting logs, traces, HAR files, dumps or command output in chat, artifacts,
issues or shared checkpoints.

If the human asks to learn while debugging, attach `ahead-teaching` as a linked
arc. That arc may explain the evidence; it cannot authorize experiments or
change the parent assistance task's policy.
