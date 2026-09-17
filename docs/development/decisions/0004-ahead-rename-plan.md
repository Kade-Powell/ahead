# Ahead terminology rename plan (Lapce → Ahead)

- Status: Planned (execute during the GPUI port, not before)
- Date: 2026-09-17
- Context: We depart from Lapce permanently; the tree must read as Ahead.
  Doing the rename now would churn the working Floem shell mid-slice, so
  the rename rides the GPUI port file-by-file.

## Scope (measured 2026-09-17)

- 4 crate dirs (`lapce-app`, `lapce-core`, `lapce-proxy`, `lapce-rpc`);
  ~130 `.rs`/`.toml` files mention `lapce` (case-insensitive).
- 61 files use `Lapce*` symbols. Top renames by occurrence:
  `LapceColor` (486) → `AheadColor`, `LapceIcons` (183) → `AheadIcons`,
  `LapceLanguage` (161) → `AheadLanguage`, `LapceConfig` (142) →
  `AheadConfig`, `LapceWorkspace` (95) → `AheadWorkspace`,
  `LapceWorkbenchCommand` (87) → `AheadWorkbenchCommand`,
  `LapceCommand` (65) → `AheadCommand`, `LapceWorkspaceType` (41) →
  `AheadWorkspaceType`, `LapceDb` (33) → `AheadDb`,
  `LapceBreakpoint` (11) → `AheadBreakpoint`.
- Package/binary names: root `lapce` package, `lapce`/`lapce-proxy` bins,
  `lapce-app`/`lapce-proxy`/`lapce-rpc`/`lapce-core` crates, bin paths
  `lapce-app/src/bin/lapce.rs`, `lapce-proxy/src/bin/lapce-proxy.rs`.

## Execution order (with the port, file-by-file)

Measured cross-crate surface (2026-09-17): 91 files reference
`lapce_*` crates in `use` paths; heaviest are `ahead/view.rs` (40),
`ahead/state.rs` (29), `editor.rs` (24), `window_tab.rs` (23). A crate
rename rewrites every one of those lines, so crates move LAST, symbols
first. Revised order:

1. Symbols via `lsp rename`, one family per commit (mechanical review):
   `LapceColor` → `AheadColor`, `LapceIcons` → `AheadIcons`,
   `LapceLanguage` → `AheadLanguage`, `LapceConfig` → `AheadConfig`,
   `LapceWorkspace(+Type)` → `AheadWorkspace(+Type)`,
   `LapceWorkbenchCommand` → `AheadWorkbenchCommand`, `LapceCommand` →
   `AheadCommand`, `LapceDb` → `AheadDb`, `LapceBreakpoint` →
   `AheadBreakpoint`. Gate: workspace check + suites green per family.
2. Bins: add `ahead`/`ahead-proxy` alongside `lapce`/`lapce-proxy`,
   verify walkthrough on new names, then remove old names + walkthrough
   again.
3. Crates + dirs (`lapce-app` → `ahead-app`, etc.) with `lsp rename_file`
   so the 91 `use` paths rewrite together. Gate: workspace check +
   suites green, walkthrough on renamed tree.
4. Strings/docs: user-visible "Lapce" → "Ahead", settings keys, schema
  titles, README/docs. Keep upstream attribution ("forked from Lapce",
  license/NOTICE) — rename is identity, not erasure.
5. Paths that must NOT change without migration: `~/.ahead` data dir
   (already Ahead), `.ahead/session.db` schema, volt/plugin install dirs
   (rename only with a migration shim), wire protocol names if external
   clients exist (version-gate instead).
## Gates

- `cargo check --workspace` + full test suite green after every step.
- `grep -ri lapce --include='*.rs' --include='*.toml'` limited to
  attribution comments, upstream URLs, and the pinned `lapce/floem` +
  `lapce/lsp-types` dependency lines (those stay until the GPUI port
  drops Floem).
- Computer-use walkthrough after bins rename (launch, open, Start Work).
