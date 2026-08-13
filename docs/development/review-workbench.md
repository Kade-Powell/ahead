# Review Workbench

Audience: AHEAD framework and tooling maintainers

Status: Pi host v0.1; editor-neutral contract v0.1

## Purpose

The review workbench makes the handoff from implementation to independent human review explicit and inspectable:

```text
HUMAN IMPLEMENTS AND SELF-CHECKS
              │
              ▼
CAPTURE EXACT CHANGESET FINGERPRINT
              │
              ▼
AI REVIEWS WITHOUT MODIFYING
              │
              ▼
HUMAN DISPOSITIONS EACH MATERIAL FINDING
              │
              ▼
INDEPENDENT HUMAN REVIEWS CURRENT SNAPSHOT
```

The adapter computes a fingerprint from the selected base and merge base, HEAD, engineering working-tree status, tracked diff, and hashes of untracked files. `.ahead/**` records are excluded so recording the review does not invalidate the engineering snapshot. AI and human review artifacts must carry `AHEAD-Review-Snapshot: <fingerprint>`. If the engineering changeset changes, the fingerprint changes and the review must be repeated.

## Portable contract

`integrations/pi/src/review.ts` defines host-neutral snapshot and source-location data plus a small `ReviewHost` boundary. Core workflow semantics require the AI findings, implementing-human disposition, and independent-human review; they do not require a terminal, VS Code, GitHub, or a particular comment API.

Pi is the first host. `/ahead-review` can show the snapshot and diff in the terminal, open a changed path in detected VS Code or `AHEAD_EDITOR=vscode`, request the AI review, and open the correct human record. A future VS Code adapter can map the same locations to native diffs and comments. A GitHub adapter can publish selected findings and verify protected-branch identities without changing the workflow contract.

## Finding and disposition shape

AI findings use stable `AR-001` identifiers and include severity, category, precise location, evidence, impact, and a falsifiable explanation. They are hypotheses. The implementing human separately marks every material finding `fixed`, `invalid`, `accepted-risk`, or `follow-up` and records rationale and evidence. An independent human then reviews the current snapshot and makes the final engineering judgment.

The workbench does not post comments, push branches, mark a pull request ready, or approve a pull request automatically. Those are explicit future host effects governed by human authorization and stronger remote identity.
