# Zed feature and strategy tracking for the GPUI port

- Status: Tracking (source import, feature adoption and implementation strategy)
- Date: 2026-09-21
- Context: GPUI + gpui-kit selected (Decision 0003). Lapce is the legacy
  base; Zed is the forward reference for editor behavior, collaboration,
  and GPUI patterns. VSCode remains a secondary reference only for
  interaction ideas, never for extension-compat promises.

## What to track from Zed (upstream: `zed-industries/zed`, Apache-2.0)

1. **GPUI patterns**: entity/context lifecycles, focus handling, overlay
   and panel composition, text rendering and input handling. Directly
   applicable — same framework family as our port target.
2. **Multi-buffer editing**: excerpts, project panels, tabs/splits,
   selections, undo/redo, IME, symbol outline, diagnostics list, inline
   completions presentation. Direct source reuse is preferred.
3. **Collaboration**: channels, shared buffers, presence, following,
   version-vector or CRDT buffer sync, conflict presentation. Reuse the
   mature transport/rendering patterns where compatible; AHEAD owns session
   policy, identity, anchors and workflow events.
4. **Agent panel UX**: thread list, streaming deltas, tool-call cards,
   checkpoint/rewind affordances and terminal context. AHEAD's panel and
   policy workflow remain different from Zed's.
5. **Search and Champagne**: project search, symbol search, command
   palette ranking, keymap ergonomics.
6. **Terminal and tasks**: named terminal tabs, resizing, task definitions,
   terminal integration, output linking back to buffers and terminal-to-agent
   context. Tasks are driven by repository `just` recipes and commands.
7. **Accessibility**: focus order, live regions, screen-reader labels on
   editor chrome — feeds GPUI acceptance gate 4.

## How to track

- Pin a Zed upstream commit per review cycle (record SHA + date here).
- For each adopted feature: note the Zed file(s) imported, what was pruned or
  adapted, the tests carried forward, and the license/NOTICE carry-forward.
- Directly port the selected editor, search, source-control, terminal/task
  and debugger implementations where that reduces risk. Do not import Zed's
  cloud services, telemetry, agent policy, session model or unrelated product
  workflows; AHEAD's session host, policy and viewmodel remain authoritative.
- Keep the comparison honest: Zed's team size and maturity exceed ours;
  adopt ideas at our pace, gated by the GPUI acceptance gates.

## Review log

- 2026-09-17: tracking file created. No Zed checkout pinned yet.
- 2026-09-21: direct source reuse accepted for the editor, search, source
  control, terminal/task and debugger surfaces. Pin the checkout before the
  first import; preserve Apache-2.0 notices and record every imported path.
- 2026-09-21: license correction — Zed editor crates (`rope`, `text`, `lsp`,
  `project`, `editor`, `search`, `git`, `terminal`, `dap`) are GPL-3.0
  (`LICENSE-GPL` verified in `crates/rope`), not Apache-2.0 as decision 0005
  assumed. All GPL imports paused per decision 0006; pin deferred to legal
  sign-off. Only `zed_extension_api` (+ `gpui` family) confirmed Apache-2.0.
  Crate replacement verdicts recorded in decision 0006.
- 2026-09-21: maintainer decision — import GPL Zed source now, clean-room
  later (decision 0006). Pin + import unblocked; clean-room rewrite is a
  standing TODO obligation. Until the rewrite, preserve per-file copyright
  notices and ship license texts with distributed builds.
