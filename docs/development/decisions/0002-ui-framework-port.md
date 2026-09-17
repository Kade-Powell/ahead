# Decision 0002: Port UI from Floem to GPUI + gpui-kit

- Status: Accepted (spike-gated; Floem tree kept intact until spike passes)
- Date: 2026-09-17
- Deciders: AHEAD maintainers
- Context: Lapce-fork UI is built on Floem (pinned git rev `e0dd862`); AHEAD must own its UI future. Related: [0001](0001-deltadb-collaboration-fallback.md) (Yrs + SQLite fallback), spec §3.2/§15 (extension scope), §14 (a11y).

## Measured coupling (this repo, 2026-09-17)

- `lapce-app`: 102 files, ~51k LOC; **71 files import floem**.
- Editor is deeply coupled: `floem::views::editor` (`Editor`, `Document`, `RopeText`, `PaintCx`, `ScreenLines`, IME, gutter, diff) with a custom `EditorView` paint layer.
- `lapce-proxy`, `lapce-rpc`, `lapce-core` do **not** depend on Floem. Only `lapce-app` is rewritten; proxy (session host), RPC DTOs, and core buffers/syntax survive.

## Options evaluated

| | GPUI + gpui-kit | Tauri v2 + Dioxus/Leptos/Vue | Iced |
|---|---|---|---|
| Editor | Ships `Editor` control: Tree-sitter highlighting, gutter, folding, decorations, multi-cursor + column select, built-in + programmatic search, completion/hover hooks | Monaco/CodeMirror on day one (best editor instantly) | No Lapce-grade widget; third-party `iced-code-editor` covers basics only — hand-build multi-cursor/diff/LSP/IME |
| Extension answer | `gpui-shell`: JS plugins in-process, host-granted capabilities, default-deny, +13.5 MiB, no WebView/DOM — resolves §15 without VSIX host | npm ecosystem; plugin sandbox must be custom-built around the DOM | No story |
| Language | All Rust | Rust + JS split; webview memory; two runtimes | All Rust |
| A11y | Weakest point — must be a spike gate | Best (native platform a11y via webview) | Mediocre |
| Maturity risk | gpui-kit young (0.6.1, 2026-09-09); upstream Zed reportedly slowed GPUI dev — pin rev, keep Floem branch | Tauri v2 stable + mobile, mature | Mature framework, immature editor story |

Pins (2026-09-17): `gpui-kit` **0.6.1**, Apache-2.0, `github.com/longbridge/gpui-kit`, `https://gpui-kit.com`.

## Decision

**Primary: GPUI + gpui-kit.** Single-language port, real editor control, dock/command-palette/status-bar chrome, and a capability-granted JS plugin path that fits the AHEAD policy boundary (host grants one capability at a time; script describes, Rust renders).

**Fallback: Tauri + Dioxus** (fastest Rust-rendered Tauri frontend) or **Tauri + Vue** (if npm ecosystem outweighs binary size). Trigger: spike fails on a11y or platform maturity.

**Rejected: Iced.** Editor gap is the dominant risk and nothing else compensates.

## Spike (small, on a branch — not started)

1. gpui-kit `Editor` opens a real file; edit + save through existing proxy buffer path.
2. Tree-sitter highlight on; one LSP completion round-trip.
3. Dock with two panels; keyboard-only traversal + screen-reader narration check.
4. One `gpui-shell` JS panel under a test capability grant.
5. Record revs, frame/perf notes, a11y verdict in this file's appendix.

## Consequences

- Positive: UI future owned; extension story without VSCode-compat burden; proxy-side work (session host, Codex-runtime fork, ACP) continues unchanged — all framework-agnostic.
- Negative: full `lapce-app` rewrite (~51k LOC); gpui-kit youth risk carried until spike passes; a11y must be proven, not assumed.
- Neutral: Linux/Windows trail macOS — same as current position.

## Amendment 2026-09-17: two-horse verdict (GPUI vs Tauri/Leptos)

Iced eliminated (no Lapce-grade editor widget; hand-building
multi-cursor/diff/LSP/IME is the dominant risk). Remaining race decided
for **full GPUI port**, against Tauri + Leptos/Dioxus/Vue:

- A Tauri port puts the editor in JS (Monaco). Every AHEAD differentiator —
  teaching range vs caret vs agent pointer, anchored inline threads,
  caret-safe presentation cues, sticky anchors — becomes cross-boundary IPC
  between the Rust session host and a webview. That is split-brain
  architecture for the exact features that must be pixel-precise.
- GPUI keeps buffers, policy, presentation, and GPU rendering in one Rust
  process at 120 fps. `gpui-shell` answers the extension question (JS
  plugins, host-granted capabilities, default-deny) with no VSIX host,
  no Node runtime, no Marketplace legality problem.
- Tauri + Dioxus stays the fallback if the spike fails on a11y or platform
  maturity. It is the fastest Tauri-shaped exit, not the primary.

## Spike evidence so far (`../ahead-spike-gpui/spike`, branch `spike/gpui-kit`)

- Resolved pins: `gpui-kit` 0.6.1, `gpui-base` 0.6.1, `gpui-component`
  0.6.1, `gpui-pre` 0.3.5. `cargo check` passes (~38 s first build).
- API confirmed in vendored source: `Editor::new(&Entity<EditorState>)`,
  `EditorState::new(window, cx).language(..).default_value(..)`,
  per-language `tree-sitter-*` features, `dock/` with panel registry
  (`dock.rs`, `panel.rs`, `tab_panel.rs`, `tiles.rs`).
- Still open (gates the full port): real window opening a real file,
  edit+save through the proxy buffer path, one LSP completion round-trip,
  keyboard-only traversal + screen-reader narration, one `gpui-shell` JS
  panel under a test capability grant.

## Amendment 2026-09-17 (superseding): port REJECTED on reuse grounds

The user reframed the objective: port to whatever reuses the MOST existing
fork code. Measured in this repo that day:

- `lapce-proxy` (~13k LOC), `lapce-rpc` (~4k), `lapce-core` (~5k): zero
  Floem. Portable anywhere, including staying put.
- `lapce-app` (~51k): 31 files Floem-free; 71 Floem-coupled, but only 13
  touch `views::editor` deeply (editor widget, doc, main_split, terminal
  view, completion, hover). The rest use Floem for reactive signals and
  view scaffolding — mechanical rewrites, not redesigns.
- GPUI/Tauri/Iced all rewrite the 71 coupled files against new APIs; the
  spike confirmed GPUI's editor model (`InputBaseState<EditorMode>`,
  entity/context) shares nothing with `floem::views::editor`. Reuse there
  is concepts, not code.

Verdict: **stay on Floem**, pinned at `e0dd862`. The vertical-slice work
(merged to main) lands directly with no translation layer. Extension
answer stays: native volts + standalone LSP now, Zed-style declarative
layer as first extensibility epic, no VSIX host. Spike branch
`spike/gpui-kit` + `../ahead-spike-gpui` kept as evidence (window opened,
file loaded/highlighted, save wired, screenshot-verified).
