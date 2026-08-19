---
name: diagnosing-bugs
description: Run a human-led, evidence-first diagnosis for a specific hard bug, intermittent failure, or performance regression. Use when the user requests disciplined diagnosis, not for a quick explanation or a general codebase audit.
license: MIT; see LICENSE.ahead and LICENSE.mattpocock
---

# AHEAD Diagnosing Bugs

Diagnose one characterized failure through a safe feedback loop, falsifiable hypotheses,
and controlled probes. This adaptation intentionally follows AHEAD ordering: the human's
failure characterization and initial model come before AI-generated hypotheses.

## Authority and safety

- When an AHEAD run is active, read its authoritative context first. Use the Corrective
  Debugging workflow when diagnosis and correction are the dominant outcome, and never
  jump ahead of its human-owned gates.
- Before broad AI diagnosis, a human characterizes observed versus expected behavior,
  scope, impact, reproduction knowledge, and boundaries, then states a current
  falsifiable model of what may be happening and why. “Unknown after these observations”
  is honest uncertainty, but AI must not invent and attribute a model to the human.
- The human selects which hypotheses to test, accepts the diagnosis or uncertainty,
  chooses the correction, supplies the first-pass plan, and owns the first implementation.
- Use only authorized environments, systems, data, and tools. A necessary stabilization
  response may proceed through the Operational Stabilization workflow without proven root
  cause; diagnosis must not delay urgent human-authorized recovery.
- Commands, logs, captures, traces, dumps, and screenshots may contain credentials,
  cookies, tokens, personal data, or customer data. Minimize collection and redact
  sensitive content as `<REDACTED>` before showing or persisting it. If redaction removes
  the required signal, say so and request an authorized handling path.

## 1. Build the tightest safe feedback loop

After the human model exists, construct the smallest safe pass/fail signal that reaches
the reported symptom. Prefer, in order appropriate to the system:

1. a focused failing test at the real behavior seam;
2. a local HTTP or CLI invocation with fixture input and an exact assertion;
3. a headless UI script that checks DOM, console, and network behavior;
4. replay of an authorized, redacted request, payload, trace, or event log;
5. a disposable harness around the smallest real subsystem;
6. a property, fuzz, stress, differential, or automated bisection loop; or
7. a structured human-observation loop when automation is impossible.

Tighten the loop until it is as fast, deterministic, and specific as practical. For an
intermittent failure, increase and measure the reproduction rate through bounded looping,
controlled concurrency, seeded inputs, or timing probes rather than pretending the bug is
deterministic.

Record the exact command or observation procedure, the redacted result, and why it detects
the user's actual symptom rather than a nearby failure.

### No safe reproduction path

If no safe loop can be built, list what was attempted and request one of:

- authorized access to an environment that exhibits the failure;
- a minimized, redacted capture or diagnostic artifact; or
- approval for temporary, bounded instrumentation.

Use captured observations and authorized production instrumentation when reproduction is
unsafe. Record the limitation and remaining uncertainty. Do not claim a root cause merely
because a clean reproduction is unavailable.

## 2. Reproduce and minimize

Confirm that the signal exposes the characterized failure across enough runs to support
investigation. Capture the exact error, incorrect output, state transition, timing, or
other observable symptom.

Reduce inputs, callers, configuration, data, and steps one variable at a time, rerunning
the loop after each reduction. Preserve every condition shown to be load-bearing. Do not
simplify away a causal concurrency, authorization, data-shape, version, or environment
boundary merely to obtain a convenient test.

## 3. Expand the human model into ranked hypotheses

Using the human's initial model and reproduction evidence, propose a small ranked set of
falsifiable hypotheses. For each one state:

- evidence for and against it;
- the observation it predicts;
- the smallest safe probe that can falsify it; and
- the confidence and important alternatives.

Present the ranked set to the human. The human chooses what to test or explicitly permits
the proposed order. Do not anchor on one plausible explanation or treat an AI ranking as
a diagnosis.

## 4. Probe one explanatory variable at a time

Map every probe to one prediction. Prefer direct inspection, debugger or REPL state, and
narrow boundary instrumentation over broad logging. For performance regressions, establish
a baseline and use profiling, query plans, tracing, differential measurement, or bisection
before proposing optimization.

Tag temporary instrumentation with a unique marker such as `[AHEAD-DEBUG-a4f2]`. Keep its
output distinguishable from product behavior and record probe, result, and hypothesis
impact in the active investigation ledger or conversation.

Return to characterization or the human model when evidence contradicts the current
framing. Contradictions are progress, not permission to silently rewrite prior evidence.

## 5. Conclude before correcting

State the supported diagnosis—or the explicit unknown cause—with evidence, confidence,
residual risks, and competing explanations. The accountable human accepts that conclusion
before choosing a correction.

Do not move directly from an interesting probe to a code change. The human selects the
correction and verification strategy and supplies the first-pass plan. AI may challenge
the plan and perform explicitly requested bounded assistance within the active phase.

## 6. Verify and clean up

Verification must include:

- rerunning the original, unminimized failure signal;
- a regression test at the real behavior seam when such a seam exists;
- relevant broader checks for side effects and failure paths;
- removal of every tagged instrumentation point and disposable artifact; and
- documented limitations when no truthful regression-test seam exists.

If no correct test seam exists, report that architectural limitation rather than adding a
shallow test that cannot catch the bug. The human records the final outcome, rollback,
follow-up work, and accepted uncertainty through the applicable workflow.
