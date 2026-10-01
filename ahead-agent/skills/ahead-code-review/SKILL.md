---
name: ahead-code-review
description: Perform an automated end-of-session review of a frozen AHEAD change snapshot against repository standards and the originating task or design. Use at session review, explicit review requests, or before a human decides to publish.
---

# AHEAD code review

Read [../references/human-led-policy.md](../references/human-led-policy.md)
before proceeding.

Automated review is allowed and expected at the end of a session. It is an
analysis step, not an autonomous editing or publication step.

## Review contract

1. Freeze or identify the exact review snapshot: base revision, current code,
   unsaved buffers and relevant untracked files. If the fixed point is missing,
   ask for it rather than guessing.
2. Load the originating work item, accepted design/plan, AHEAD rules and
   applicable ADRs. If no specification exists, report that the Spec axis is
   unavailable; do not invent one.
3. Review two independent axes:
   - **Standards:** documented repository rules, security/accessibility policy,
     error handling, ownership and known invariants.
   - **Spec:** whether the change satisfies the human-owned outcome, examples,
     decisions and acceptance evidence.
4. Anchor every finding to the snapshot, source rule or task artifact. Label
   severity and distinguish confirmed evidence from a concern or hypothesis.
5. Check for missing tests, stale documentation, scope creep, attribution loss,
   dirty-buffer mismatch, unauthorized effects and unverified runtime claims.
6. Produce findings and recommended dispositions. Do not edit code, auto-apply
   fixes, close findings, publish a PR or update a tracker unless the human
   separately requests and authorizes that action.

The review should be independent of the authoring turn when practical. A review
finding is not proof until the human or a follow-up check verifies its cited
location and impact. Persist the review against the immutable snapshot so later
code changes make its status stale rather than silently preserving approval.

At session completion, offer the review automatically. The human remains the
only authority who accepts findings, chooses fixes, declares remaining risk or
publishes external results.

## Runtime review addenda

When the snapshot touches `ahead-agent/` or `ahead-agent/runtime/source/`, also
check:

- model-visible tool names, descriptions, schemas, result/error shapes,
  instruction layering, streaming, compaction, cancellation and tool ordering;
- `ahead-rpc` DTOs, provider/config loading, persisted sessions and resume/
  cancellation behavior for compatibility breaks;
- bounded context injection: incremental history, no item over 10,000 tokens,
  and manual review for a new item that can exceed 1,000 tokens;
- integration coverage for changed agent behavior, with skipped,
  credentialed and sandbox-dependent tests reported separately;
- whether the diff is under 800 lines (under 500 for complex logic), or has a
  concrete staged split when it is larger.

Use the scoped `.agents/skills/ahead-agent-path-types` and
`.agents/skills/ahead-agent-remote-tests` procedures when those surfaces are
involved.
