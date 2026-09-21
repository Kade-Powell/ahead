# Decision 0006: Dependency replacement and Zed source-reuse correction

- Status: Accepted, with clean-room obligation (maintainer decision 2026-09-21:
  GPL Zed source may be imported and adapted now, and must be clean-room
  reimplemented later — tracked in TODO.md)
- Date: 2026-09-21
- Deciders: AHEAD maintainers
- Related: [0004 rename plan](0004-ahead-rename-plan.md),
  [0005 Zed source reuse](0005-zed-editor-source-reuse.md),
  [Zed tracking](../zed-tracking.md)
- Context: Lapce upstream is dying. Every Lapce-org dependency must be
  replaced or removed. Replacements must be actively developed and widely
  used (GitHub stars / crates.io adoption checked 2026-09-21). Where we
  build on top of a crate, Zed's source is the reference implementation.

## Crate verdicts (checked 2026-09-21)

| Crate | Health | Verdict |
|---|---|---|
| `ropey` (`cessen/ropey`, ~1.7k★, 11.8M downloads) | 2.0.0-beta.1 (2025-08) current pre-release; 1.6.1 stable | **Adopt 2.0.0-beta.1** per maintainer direction (contribute back, prep for major). No delta/interval APIs in any version — AHEAD builds its own delta layer. Byte-index migration cost accepted. |
| `sys-locale` (`1Password/sys-locale`, ~140★, 33M downloads) | 0.3.2, quiet but ubiquitous | **Adopted** — replaces `locale_config` git dep (1 call site, `terminal.rs`). |
| `wasmtime-wasi-http` (`bytecodealliance/wasmtime`, ~18.7k★, daily) | 48.x current; **14.x dead** | Adopt 48.x, but only together with the extension-host decision: our `wasmtime 14` host code serves volt plugins, which are being removed. |
| `lsp-types` (`gluon-lang/lsp-types`, ~404★, 36M downloads) | 0.97.0 current; **no `Debug` on `MessageType`** in any 0.95–0.97 | Adopt upstream **0.97.0**; add a local newtype/wrapper where `Debug` is needed. Unblocked by psp-types removal (psp-types needs fork-only `Url`). |
| `gpui-editor` (`iamnbutler/gpui-editor`, ~14★, prototype) | 0.2.0, 36 downloads | Reference only, never a dependency. Already listed as such in `AGENTS.md`. |
| Zed `rope`/`text` (monorepo ~90.7k★) | Unpublished (`publish=false`), **GPL-3.0** | **Cannot depend.** See license blocker below. |

## Zed source study (what to follow where we build)

- **Extension host**: Zed runs extensions as WASM components (`wasmtime 48`, WIT API, `zed_extension_api 0.7.0` on crates.io, Apache-2.0). Engine-level reuse of our wasmtime host is possible; the guest ABI, capability model and `Worktree/Project` delegates are Zed-specific and must be rewritten. Volt ABI is incompatible — no migration path, only replacement.
- **LSP**: Zed's `lsp` crate (framing, capabilities, server lifecycle) is reusable in shape, but `didChange` sync is driven by `project` (buffer versions, root URIs, adapters). Reuse requires the sync layer to be reimplemented against our buffer store.
- **Search/git UI**: inseparable from Zed's `Project`/`Worktree`/`Workspace`. Reference UX only; vendor nothing.
- **Low-level `git` crate**: liftable in principle, but also GPL — same blocker.

## License position (maintainer decision 2026-09-21)

Decision 0005 states Zed source is Apache-2.0. That is wrong for the
editor surfaces: `crates/rope` carries `LICENSE-GPL`
(verified via GitHub API), and `rope`/`text`/`lsp`/`project`/`editor`/
`search`/`git`/`terminal`/`dap` are `GPL-3.0-or-later` and unpublished.
Only `zed_extension_api` (and the `gpui` family) are Apache-2.0.

**Maintainer decision: import GPL Zed source anyway and clean-room it
later, after making the changes we need.** Rationale: speed of the MVP
port outweighs license purity today; the clean-room rewrite is an
explicit, tracked obligation, not a vague intention. Until the rewrite
lands, distributed builds carry GPL obligations (preserve per-file
copyright notices, ship license texts, disclose source).

**Consequence:**
- Imports proceed: pin the checkout, record SHA + paths + carried tests
  + notices per 0005/`zed-tracking.md` (TODO pin item UNBLOCKED).
- TODO.md carries a standing clean-room item: reimplement each imported
  GPL area from its documented behavior (not its text) and delete the
  vendored source, one area per commit with tests proving parity.
- Layering: rope layer = `ropey 2.0.0-beta.1` + AHEAD delta layer (no GPL
  there); editor behavior (buffers/tabs/selections/undo/IME, search, git
  UI, DAP surface) = ported Zed patterns on top, clean-roomed later.

## Accepted sequence

1. Extract shared RPC plumbing from `psp.rs`; rewire `LspClient`/`DapClient`
   startup to `dispatch.rs` (they currently ride the volt abstraction).
2. Delete the volt stack (`catalog.rs`, `wasi.rs`, `wasi/`, volt RPC
   surface, registry URL, fixtures) + `psp-types`, `wasi-experimental-http`
   (14.x), `wasmtime 14` deps if unused elsewhere. TODO.md entry for the
   Zed-model extension host.
3. Drop the `lsp-types` fork; adopt upstream 0.97.0 + local `Debug` shim.
4. Migrate `lapce-xi-rope` → `ropey 2.0.0-beta.1` with an AHEAD delta layer
   (apply/invert + serde for RPC), removing `floem-editor-core` in the same
   pass (its `RopeText` adapters die with xi-rope).
5. Decide the extension host (Zed WIT on wasmtime 48 vs deferred) before
   touching wasmtime versions.

## Priority (confirmed 2026-09-21)

Ecosystem crates first; Zed source second; bespoke AHEAD code last. When
rearchitecture needs implementation patterns, follow Zed's source at the
pinned checkout (`~/dev/zed`) and use its code directly where no
ecosystem crate covers the need — adapting names and boundaries to AHEAD
(clean-room obligation applies). Do not build bespoke what a healthy
crate already does; do not vendor Zed where a healthy crate exists.
