# Work items: side-panel checklist with close-out docs and context retention

- Status: Design (implement during GPUI port, phase 4 of Decision 0003)
- Date: 2026-09-17
- Context: AHEAD work items must behave like a living todo list: visible
  in the side panel, dynamically addable, and each one accumulating the
  documentation needed to close its issue and to restore context later.
  Planning conversation (including summaries) is retained, not discarded.

## Data model (new tables + DTOs, mirrors existing store conventions)

- `work_items`: `id`, `session_id`, `title`, `status`
  (`open` | `in_progress` | `done` | `dropped`), `position` (manual
  order), `created_by`, `created_at`, `closed_at`.
- `work_item_events`: append-only per-item log (`note`, `decision`,
  `artifact_link`, `status_change`), each with `actor_id`, timestamp,
  and optional `anchor_id` / `artifact_id` refs.
- `work_item_closeout`: one row per closed item — `summary_markdown`
  (what was done, evidence, files changed, verification), `issue_ref`,
  `follow_ups` (JSON), `conversation_summary_id`.
- `conversation_summaries`: `id`, `session_id`, `phase`, `summary_markdown`,
  `message_id_range`, `created_at`. Written at phase transitions and on
  demand; never deletes the underlying messages.
- DTOs in `lapce-rpc/src/ahead.rs`: `WorkItem`, `WorkItemEvent`,
  `WorkItemCloseout`, `ConversationSummary`; RPC:
  `WorkItemCreate | WorkItemUpdate | WorkItemNote | WorkItemClose |
  WorkItemList | ConversationSummarize`. Reuse the revision-lock +
  `session_events` envelope pattern from workflow transitions.

## Side-panel behavior

- Checklist section in the session panel: open items with status pills,
  inline add box (Enter creates), drag or keyboard reorder, one-tap
  status cycling, per-item expander showing its event log + close-out.
- Items created from chat ("add X as a work item"), from plans, or by
  hand. Closing an item requires (or explicitly skips with reason) the
  close-out summary; the summary renders as the issue-closing comment
  draft via the existing tracker outbox flow.
- Conversation summary card per phase: retained alongside messages,
  linked from work items that referenced it. Part of session export and
  resume (a resumed session shows prior summaries first).

## Threads Sidebar & Multi-Thread Collaboration (Overall Repo Goal)

- **Threads Sidebar Component** (`ahead-app/src/threads_panel.rs`): Dedicated rightmost sidebar/dock panel displaying all active conversation threads grouped by workspace.
- **Dual Thread Taxonomy**:
  1. **AHEAD Work-Item Threads (Special / Collaborative)**:
     - Each thread represents an active or running AHEAD work item.
     - Fully collaborative: all invited workspace teammates can view, chat, and participate in the same thread.
     - Carries live phase state, invariant checklists, code proposals, and review attestations.
  2. **Delegated Task Threads (One-off / Subagent)**:
     - Represents one-off delegated tasks spawned to other harnesses (e.g. background ACP subagents, external analyzers, or test runners).
     - Returns attributed findings and review diffs without mutating shared collaborative session state.
- **Interaction**: Top search input ("Search threads..."), "+ AHEAD Thread" and "+ Task" triggers, active thread selection switching the central Agent Pairing Feed.

## Retention and export

- Nothing is deleted on close: messages, summaries, item events, and
  close-outs persist in the session store and ride the existing export.
- Future context: reopening a session (or its issue) surfaces open items,
  recent summaries, and close-outs of done items in that order.

## Gates

- Unit: item CRUD + ordering + close-out-required validation; summary
  written at phase advance without losing messages.
- E2E: create → progress → close with summary → export → resume on a
  fresh checkout shows the same items, summaries, and close-out docs.
