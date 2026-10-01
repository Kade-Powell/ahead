# Decision 0001: Collaboration Engine & DeltaDB Dependency Decision

- Status: Partially superseded (session persistence is Turso/libSQL)
- Date: 2026-09-17
- Deciders: AHEAD Core Maintainers
- Context: Section 3.5 & Milestone 0 exit requirement of [ahead-editor-mvp.md](../ahead-editor-mvp.md)

2026-09-24 update: The local SQLite event-store choice below was replaced by
Turso/libSQL at `.ahead/session.db`; the copied SQLx/SQLite state crate and
runtime dependency path have been removed. This decision retains the Yrs
collaboration direction and records the DeltaDB evaluation. See the [workflow
atlas storage decision](../ahead-workflows.md#9-storage-and-artifact-convention)
for the current persistence contract.

## Context and Problem Statement

AHEAD requires multi-user collaborative editing, code-anchored conversations, and durable workflow state.
Zed's Delta announcement describes synchronizing conversations and worktrees alongside Git using DeltaDB. We investigated whether DeltaDB is currently available as a reusable, embeddable open-source dependency for the AHEAD editor.

## Evaluation & Findings

1. **Public Availability**: While Zed announced Delta in public beta (September 2026), DeltaDB is not released as an independent, embeddable Rust crate, standalone service distribution, or documented SDK for third-party editors.
2. **Licensing & Distribution**: No standalone Apache-2.0/MIT distribution or embedding contract exists for external editor runtimes.
3. **Architecture Match**: Even if embedded, DeltaDB focuses on replicated worktrees and Git-aware sync, whereas AHEAD's MVP requires a single execution host topology with authoritative workflow gates, human-led permissions, and anchored inline threads.

## Decision

We retain **Yrs** for collaborative live text and relative positions. AHEAD's
local durable session and workflow state uses **Turso/libSQL** in
`.ahead/session.db`; it does not use a second SQLx/SQLite store.

If DeltaDB or a comparable system becomes available as an open-source embeddable
library with acceptable licensing and embedding contracts, AHEAD can evaluate it
against the collaboration acceptance tests without changing the current storage
decision.

## Consequences

- **Positive**:
  - Mature CRDT behavior for real-time text co-editing via Yrs.
  - One Turso/libSQL database for local managed-session and workflow state.
- **Negative**:
  - AHEAD must manage buffer materialization and transaction barriers to the filesystem worktree.
