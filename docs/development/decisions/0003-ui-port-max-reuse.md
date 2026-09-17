# Decision 0003: UI framework selection and migration plan

- Status: Accepted — GPUI + gpui-kit selected; migration is spike-gated
- Date: 2026-09-17
- Deciders: AHEAD maintainers
- Context: [0002](0002-ui-framework-port.md) evaluated GPUI and Tauri;
  user direction is to choose one new UI framework rather than leave the
  repository with contradictory plans. The comparison covers GPUI + gpui-kit
  and Tauri with Leptos, Dioxus, Vue, or Solid. Iced is not viable because it
  has no Lapce-grade editor control. The primary agent is built into the
  session host; ACP is side-tasks-only.

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

## Decision: GPUI + gpui-kit

Choose **GPUI + gpui-kit** for the production UI port. Keep Floem running
until the acceptance gates below pass. If GPUI fails a hard gate, the
contingency is **Tauri 2 + Vue 3 + TypeScript**, not an open-ended choice
among four frontend frameworks.

## Comparison

| Criterion | GPUI + gpui-kit | Tauri + Vue 3 | Tauri + Leptos/Dioxus/Solid |
|---|---|---|---|
| Editor fit | Native Rust editor, dock, overlays, IME and rendering in one process | Web editor in a system WebView; strong widget ecosystem, but a new editor boundary | Same WebView boundary; no material advantage over Vue for this product |
| Reuse | Keeps `lapce-core`, proxy, RPC, Rust position/selection logic and viewmodel types together | Keeps backend crates, but replaces the UI/editor layer and adds Rust↔TypeScript IPC | Keeps backend crates, but adds a Rust webview UI layer and IPC |
| AHEAD interaction model | Best fit for caret-safe pointers, anchored ranges and precise presentation | Easier semantic HTML and screen-reader behavior | Better than native GPUI only if the chosen web components are used well |
| Performance/control | GPU/native event loop; no high-frequency editor IPC | Excellent for ordinary panels; editor and overlay traffic cross the WebView boundary | Same Tauri/WebView constraints |
| Accessibility risk | Must prove platform accessibility before migration | Lowest risk due to HTML accessibility tooling and browser testing | Lower than GPUI, but smaller ecosystem and more custom integration |
| Delivery risk | Large Rust view rewrite; GPUI/gpui-kit API and platform maturity risk | Faster UI iteration, but new TS models, IPC, editor integration and WebView variance | Framework choice adds risk without removing the Tauri boundary |
| Extension/policy boundary | `gpui-shell` supports capability-granted scripts in-process | Requires a separate capability-safe plugin/WebView design | Same requirement |

Evidence used for this comparison: [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui)
is still pre-1.0 and actively developed; [gpui-kit](https://github.com/longbridge/gpui-kit)
provides the editor, dock, LSP-oriented code-editor surface, and accessibility
metadata we need to validate; [Tauri's architecture](https://v2.tauri.app/concept/architecture/)
puts the UI in HTML rendered by a system WebView with Rust/JavaScript message
passing; and [Vue's accessibility guidance](https://vuejs.org/guide/best-practices/accessibility)
provides the conventional semantic HTML/ARIA path for the contingency shell.

### Why GPUI wins

GPUI reuses more of this fork than the Tauri alternatives for AHEAD's actual
workload:

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

Tauri + Vue is the fallback if GPUI fails accessibility or platform maturity.
Vue wins that fallback slot because it gives us the most conventional
component, testing and accessibility workflow; Leptos and Dioxus preserve
Rust syntax but still render through a WebView, while Solid's fine-grained
reactivity does not compensate for the same boundary or its smaller desktop
ecosystem.

## Migration strategy: strangler, not big-bang

1. Extract framework-agnostic `ahead-viewmodel` crate (zero Floem/GPUI
   deps): wizard state machine, session error/busy, voice intent +
   generation, chat/proposal/tracker pure logic, Codex turn-shape
   assembly. Unit-tested. Both shells depend on it.
2. Keep the Floem app working as a thin adapter over the viewmodel
   (daily driver never breaks; computer-use walkthroughs continue).
3. Spike the GPUI shell against the same viewmodel crate.
4. Promote to full port file-by-file: signal-only state, chrome/panels,
   AHEAD overlays, then editor integration last.
5. Remove Floem only after the GPUI shell covers the MVP acceptance matrix.

## Hard acceptance gates

The port does not become the default shell until all of these work in the
same pinned dependency set:

1. Open, edit, undo, IME-compose, and save a real file through the existing
   proxy/document path without losing unsaved changes.
2. Complete one LSP request and render diagnostics, inline completion, and a
   non-caret-stealing AHEAD pointer/range overlay.
3. Run the Start Work, agent panel, voice state, collaboration, and review
   flows through `ahead-viewmodel` and the existing session host.
4. Pass keyboard-only traversal and screen-reader narration on macOS for the
   MVP shell. Windows and Linux accessibility checks are release gates for
   those platforms.
5. Demonstrate stable frame/input behavior with a large file and a streamed
   voice/agent update occurring while editing.

If any gate fails, stop the GPUI migration and build the contingency shell as
Tauri + Vue 3 + TypeScript. Do not carry both production shells indefinitely;
Floem is the temporary safety shell, not a third long-term product.

## Consequences

- Positive: every line moved into the viewmodel is written once and
  reused by both shells; backend (proxy/RPC/core, session host,
  built-in agent loop, ACP side tasks) untouched — all framework-agnostic.
- Negative: GPUI port is still a ~37k LOC rewrite at the view layer;
  "reuse" means logic + types + tests, not drop-in. gpui-kit youth risk
  carried; a11y must be proven in the spike.
- Neutral: `third-party/codex` fork (rust-v0.152.0) and protocol
  snapshot are UI-independent and unaffected.

## Post-MVP: clippy lint set (adopted from cola-v2, deferred until MVP)

Source: `../cola-v2/Cargo.toml` `[workspace.lints]` + `clippy.toml`.
Adopt at MVP, not before — no lint churn while the vertical slice lands.

- Baseline: clippy `all` at deny; rustc future-incompatible deny;
  `allow_attributes`/`disallowed-types`/`disallowed-methods`/`wildcard_imports`
  deny (reviewable suppressions, enforced boundaries).
- Production: `expect_used`/`unwrap_used`/`panic`/`todo`/`unimplemented`/
  `unreachable`/`missing_panics_doc` deny (propagate failures); tests get
  cola-v2's narrow `clippy.toml` exceptions (`allow-expect-in-tests`,
  `allow-panic-in-tests`, `allow-unwrap-in-tests`).
- Idiom: flat control flow (`collapsible_else_if`, `manual_let_else`,
  `redundant_else`, `match_bool`, `if_not_else`, `semicolon-if-nothing-returned`,
  …), explicit allocation/cast costs (`cast_*`, `implicit_clone`,
  `redundant_clone`, `inefficient_to_string`, …), no dropped work
  (`let_underscore_must_use`, `unused_async`, `large_futures`,
  `large_stack_arrays`), narrow APIs (`unused_self`, `unnecessary_wraps`,
  `ref_option`, …), no debug output (`dbg_macro`, `print_stdout/stderr`).
- Exceptions kept (not uniformly clearer): `default_trait_access`,
  `doc_lazy_continuation`, `match_same_arms`, `must_use_candidate`,
  `single_match_else` allow; `cognitive_complexity`/`as_conversions`/
  `get_unwrap` deliberately excluded.
- Ahead-specific additions: deny `todo!` outside tests now (already the
  delivery bar), plus a `disallowed-methods` entry for direct shell spawn
  in managed-session paths once the ahead-agent boundary stabilizes.
