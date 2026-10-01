# Decision 0005: Zed source reuse and editor parity scope

- Status: Accepted
- Date: 2026-09-21
- Deciders: AHEAD maintainers
- Related: [0003 UI framework selection](0003-ui-port-max-reuse.md),
  [AHEAD editor MVP](../ahead-editor-mvp.md), [Zed tracking](../zed-tracking.md)

## Decision

AHEAD will use a pinned Zed source checkout as the implementation source for
mature editor surfaces. We will port and prune the relevant Rust/GPUI code and
tests directly instead of reimplementing equivalent editor behavior from
scratch.

This is source reuse, not a dependency on the Zed application. We do not adopt
Zed's cloud services, telemetry, product identity, agent policy, session model
or unrelated product workflows. Every imported area must record its Zed
revision, source paths, Apache-2.0 notices, carried tests and AHEAD changes.

## Adopt now

### Core editor

Use Zed's mature editor behavior as the baseline for:

- multi-file buffers, tabs and splits;
- cursor, selection, multi-cursor, undo/redo and IME behavior;
- editor input, scrolling, folding, syntax/highlighting integration and
  unsaved-buffer handling;
- external file changes and document/model synchronization.

The AHEAD shell, session host and viewmodel remain the integration boundary.

### Language intelligence

Add the normal LSP editing actions:

- rename;
- find references;
- document/workspace formatting;
- code actions;
- completion, hover, definition and diagnostics integration.

Symbol outline is explicitly deferred until the core editor and LSP actions
are working.

### Search

Port Zed's project search panel and result presentation, including project-wide
search, replace, regular expressions, case/word filters, result navigation and
opening results in buffers. Search must operate on the AHEAD workspace and
preserve unsaved-buffer rules.

### Source control

Port Zed's source-control/review surface for the initial Git workflow:

- changed-file list and per-file diff;
- stage/unstage and discard with data-loss safeguards;
- commit, branch and status operations;
- merge-conflict presentation and resolution entry points.

AHEAD attribution, anchors and session trailers remain authoritative and must
be integrated rather than replaced by Zed's review metadata.

### Terminal and tasks

Port the mature terminal structure with:

- multiple PTY terminals;
- tabs with user-visible names;
- resize/reflow and terminal lifecycle handling;
- terminal output/context selection that can be attached to an agent thread.

AHEAD tasks use repository `justfile` recipes and direct `just` commands. We
do not create a second task-definition system unless `just` cannot express a
required workflow.

### Debugging

Port Zed's DAP implementation and UI at high fidelity, including breakpoints,
debug sessions, threads, stack frames, scopes, variables, stepping, continue,
stop, source mapping and verified binding state. Keep AHEAD's guided-debugging
ownership, session identity and agent attribution at the boundary.

Debugging is a must-have, not a later enhancement. The port must carry the
relevant upstream tests and add AHEAD-specific tests for session/policy
integration.

### Agent panel

The AHEAD agent panel remains a distinct product surface, but it must reach a
polished baseline using the good parts of Zed's UX:

- streamed text with stable incremental rendering;
- clear thread header and model/provider state;
- good Markdown/code rendering;
- stop/cancel and follow-tail behavior;
- context attachments, including selected terminal output;
- durable reopen/resume without losing session context.

The panel does not copy Zed's agent policy or approval semantics. AHEAD's
managed effect boundary, teaching/assistance task rules and attribution remain native.

### Collaboration and voice

AHEAD will support voice chat with the agent during coding work, including
full-duplex input/output, interruption and independent work cancellation. Zed's
collaboration and voice implementation is a source reference where useful.

Adding other human users to the same agent thread is a nice-to-have after the
single-human-plus-agent journey is reliable. The shared thread must retain
participant identity, session permissions and durable message attribution.

## Defer

- remote development, including future Kubernetes-backed environments;
- broad visual customization and user profiles;
- arbitrary VS Code extension hosting, Marketplace compatibility, webviews and
  notebooks;
- symbol outline until the core editor/LSP/search baseline is complete.

The editor still uses GPUI/gpui-kit theme tokens everywhere so later
customization does not require a visual rewrite. Initial keybindings should
follow familiar VS Code shortcuts wherever they do not conflict with GPUI or
AHEAD actions.

## Acceptance boundary

The first usable editor must complete this loop on a real workspace:

```text
open multiple files -> edit/undo/IME -> search/replace -> LSP action
-> terminal/task -> debug/test -> review/commit -> agent context handoff
```

A compile check or imported source is not sufficient. The port needs rendered
macOS validation, real file edits, unsaved-buffer protection, debugger
interaction, terminal resizing, agent streaming and accessibility checks.
