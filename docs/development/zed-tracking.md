# Zed feature and strategy tracking for the GPUI port

- Status: Tracking (source of feature ideas + implementation strategies)
- Date: 2026-09-17
- Context: GPUI + gpui-kit selected (Decision 0003). Lapce is the legacy
  base; Zed is the forward reference for editor behavior, collaboration,
  and GPUI patterns. VSCode remains a secondary reference only for
  interaction ideas, never for extension-compat promises.

## What to track from Zed (upstream: `zed-industries/zed`, Apache-2.0)

1. **GPUI patterns**: entity/context lifecycles, focus handling, overlay
   and panel composition, text rendering and input handling. Directly
   applicable — same framework family as our port target.
2. **Multi-buffer editing**: excerpts, project panels, symbol outline,
   diagnostics list, inline completions presentation.
3. **Collaboration**: channels, shared buffers, presence, following,
   version-vector or CRDT buffer sync, conflict presentation.
4. **Agent panel UX**: thread list, streaming deltas, tool-call cards,
   approval prompts, checkpoint/rewind affordances.
5. **Search and Champagne**: project search, symbol search, command
   palette ranking, keymap ergonomics.
6. **Terminal and tasks**: task definitions, terminal integration,
   output linking back to buffers.
7. **Accessibility**: focus order, live regions, screen-reader labels on
   editor chrome — feeds GPUI acceptance gate 4.

## How to track

- Pin a Zed upstream commit per review cycle (record SHA + date here).
- For each adopted feature: note the Zed file(s) studied, what was
  ported vs reimplemented, and the license/NOTICE carry-forward.
- Prefer porting interaction design and data-flow shapes; do not copy
  large subsystems verbatim — our session host, policy, and viewmodel
  are the authorities Zed equivalents don't have.
- Keep the comparison honest: Zed's team size and maturity exceed ours;
  adopt ideas at our pace, gated by the GPUI acceptance gates.

## Review log

- 2026-09-17: tracking file created. No Zed checkout pinned yet; first
  pin happens when GPUI shell work starts (port phase 4).
