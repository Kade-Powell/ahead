# Decision 0001: Collaboration Engine & DeltaDB Dependency Decision

- Status: Accepted
- Date: 2026-09-17
- Deciders: AHEAD Core Maintainers
- Context: Section 3.5 & Milestone 0 exit requirement of [ahead-editor-mvp.md](../ahead-editor-mvp.md)

## Context and Problem Statement

AHEAD requires multi-user collaborative editing, code-anchored conversations, and durable workflow state.
Zed's Delta announcement describes synchronizing conversations and worktrees alongside Git using DeltaDB. We investigated whether DeltaDB is currently available as a reusable, embeddable open-source dependency for the AHEAD editor.

## Evaluation & Findings

1. **Public Availability**: While Zed announced Delta in public beta (September 2026), DeltaDB is not released as an independent, embeddable Rust crate, standalone service distribution, or documented SDK for third-party editors.
2. **Licensing & Distribution**: No standalone Apache-2.0/MIT distribution or embedding contract exists for external editor runtimes.
3. **Architecture Match**: Even if embedded, DeltaDB focuses on replicated worktrees and Git-aware sync, whereas AHEAD's MVP requires a single execution host topology with authoritative workflow gates, human-led permissions, and anchored inline threads.

## Decision

We formally select the **Yrs + SQLite Event Store** fallback architecture specified in Section 3.5 and 7.1:

1. **Live Text & Relative Positions**: Use **Yrs** (the Rust CRDT implementation of the Yjs ecosystem) for live buffer synchronization across connected clients and stable sticky indexes (`StickyIndex`) for code anchors through edits.
2. **Workflow Authority & Session State**: Use a durable **SQLite** event store owned by the execution host (for solo/private sessions) and team service (for shared sessions) to sequence workflow phase transitions, role grants, proposals, checkpoints, and immutable artifacts.
3. **No Lock-in**: If DeltaDB or a comparable system is released as an open-source embeddable library with acceptable licensing and embedding contracts in the future, the session host abstraction layer will allow evaluating it on the exact same collaboration acceptance tests.

## Consequences

- **Positive**:
  - Full control over memory footprint, storage format, and zero external binary dependencies.
  - Mature CRDT behavior for real-time text co-editing via Yrs.
  - Transactional SQLite safety for workflow revisions, approvals, and outbox operations.
- **Negative**:
  - AHEAD must manage buffer materialization and transaction barriers to the filesystem worktree.
