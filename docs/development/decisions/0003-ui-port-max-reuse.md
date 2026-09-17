# Decision 0003: UI port target maximizing reuse of the Lapce fork

- Status: Accepted
- Date: 2026-09-17
- Deciders: AHEAD maintainers
- Context: [0002](0002-ui-framework-port.md) (stayed on Floem for reuse);
  user direction now: fork the tree and port into a new UI framework that
  reuses the MOST existing code. Iced eliminated (no Lapce-grade editor).
  Race: full GPUI vs Tauri + Leptos. Primary agent is built into the
  session host; ACP is side-tasks-only (already implemented).

## Coupling measurement (this repo, 2026-09-17)

| Crate | LOC | Floem | Reusable anywhere |
|---|---|---|---|
| lapce-proxy | ~13.4k | zero | fully |
| lapce-rpc | ~4.3k | zero | fully |
| lapce-core | ~5.2k | zero | fully |
| lapce-app floem-free (31 files) | ~3.8k | none | as-is |
| lapce-app signal-only (~29 files) | ~10.4k | reactive signals + view scaffolding only | mechanical port |
| lapce-app deep editor-coupled (13 files) | ~16.9k | `views::editor`, custom `EditorView` paint, IME, gutter, diff | rewrite |
| lapce-app custom `View` impls (31 files) | — | `PaintCx`/`ComputeLayoutCx`, panels, palette, settings | rewrite |

Floem pinned at `e0dd862`. Backend total portable in every option: ~22.9k LOC.

## Verdict: full GPUI (all-Rust), not Tauri + Leptos

GPUI reuses more of this fork than Tauri + Leptos:

1. **No language boundary.** GPUI keeps buffers, rope/position math
   (`lapce-core` syntax, `rope_text_pos`, cursor, selection), keypress
   system, panel/dock data, and RPC types as shared Rust. Tauri splits
   the app into Rust backend ↔ webview and forces TS mirrors plus an IPC
   command layer for every view update — new code, not reuse.
2. **Editor logic survives as logic.** GPUI's editor model
   (`InputBaseState<EditorMode>`) shares no types with
   `floem::views::editor`, so the 13 deep files are still rewritten —
   but the port is Rust-to-Rust: offset/line-col math, selection
   semantics, diagnostics handling, and completion plumbing move over as
   logic with types intact. Tauri discards that code for Monaco: faster
   to a usable editor, but strictly less reuse (deletion is not reuse).
3. **Signal-only layer maps mechanically.** ~10.4k LOC of reactive
   state (session, auth/collab errors, voice intent, proposal gating)
   ports to GPUI entities/context with the same ownership shape. The
   Leptos signal model is equally close conceptually, but every ported
   line then also pays the IPC tax in (1).
4. **Extension story stays in-process.** `gpui-shell` (JS plugins under
   host-granted capabilities, default-deny) answers §15 without a VSIX
   host, Node runtime, or Marketplace legality work. Tauri's npm
   ecosystem would require a custom DOM plugin sandbox built around the
   very policy boundary AHEAD must own.

Tauri + Dioxus remains the fallback if the GPUI spike fails on screen
reader/keyboard support or Linux/Windows maturity — fastest exit, not
the primary.

## Strategy: strangler, not big-bang

1. Extract framework-agnostic `ahead-viewmodel` crate (zero Floem/GPUI
   deps): wizard state machine, session error/busy, voice intent +
   generation, chat/proposal/tracker pure logic, Codex turn-shape
   assembly. Unit-tested. Both shells depend on it.
2. Keep the Floem app working as a thin adapter over the viewmodel
   (daily driver never breaks; computer-use walkthroughs continue).
3. Spike the GPUI shell against the same viewmodel crate; promote to
   full port file-by-file (signal-only first, then panels, editor last).

## Consequences

- Positive: every line moved into the viewmodel is written once and
  reused by both shells; backend (proxy/RPC/core, session host,
  built-in agent loop, ACP side tasks) untouched — all framework-agnostic.
- Negative: GPUI port is still a ~37k LOC rewrite at the view layer;
  "reuse" means logic + types + tests, not drop-in. gpui-kit youth risk
  carried; a11y must be proven in the spike.
- Neutral: `third-party/codex` fork (rust-v0.152.0) and protocol
  snapshot are UI-independent and unaffected.
