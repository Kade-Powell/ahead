# Decision 0007: Extension host model and direct server startup

- Status: Accepted
- Date: 2026-09-22
- Deciders: AHEAD maintainers
- Related: [0005 Zed source reuse](0005-zed-editor-source-reuse.md),
  [0006 dependency replacement](0006-dependency-replacement-and-zed-reuse-correction.md)

## Decision

AHEAD supports **Zed extensions, not Lapce volts** (volt stack deleted).
Language servers share the native `LspClient` for transport and document
handling. Installed Zed language extensions resolve commands through AHEAD's
Wasmtime 48 host. Rust, TypeScript/JavaScript and Python also have built-in
adapters, following Zed's `crates/languages/src/lib.rs` at
`418f89714891f9d8105a3e92e60b9a7a5084d232`. This broad-use requirement was
confirmed on 2026-09-30. An installed matching extension takes precedence;
an extension failure is reported rather than silently switching adapters.
Users do not configure separate arbitrary language-server commands.

- **DAP (this decision, implemented now):** `DapStart` starts the configured
  adapter from `debug-adapter`/`debug-adapter-args`. For backwards-compatible
  direct adapter configs, it falls back to the `RunDebugConfig`'s own
  `program`/`args`; `cwd` falls back to the workspace. The debug target stays
  in `program` when an explicit adapter is configured, so the adapter and
  launch target are not confused.
- **LSP (this decision, implemented now):** AHEAD discovers installed
  `extension.toml` + `extension.wasm` packages, matches the manifest's
  language-server entries to the opened language, calls the guest's
  `language-server-command` export and starts one native server for its languages
  on first `did_open`. Extension-provided environment variables are passed
  through. The host mediates worktree access, downloads, executable bits and
  declared process capabilities.
- **Built-in adapters:** Rust Analyzer, Vtsls and BasedPyright currently resolve
  their executables from the app's `PATH`. Vtsls shares one process across
  TypeScript, TSX, JavaScript and JSX. Missing executables are reported through
  language-server status. Automatic acquisition and process restart are still
  release gaps in `TODO.md`; the adapter table alone does not prove usable
  language support.
- **DAP, snippets, themes, icon themes and MCP:** out of scope for this path.

## Priority reminder (from 0006)

Ecosystem crates first; Zed source second. `LspClient` already speaks the
protocol. The Apache WIT contract is retained as a versioned compatibility
boundary; AHEAD's host implementation is independent code and keeps the
required provenance in `ahead-extension-host/NOTICE`.

## User flow

Settings provides the extension install/update path. It sends a package URL and
extension ID through the proxy, which installs the package into AHEAD's managed
plugin directory. Opening a matching file then discovers the package and starts
its language server; `[language-servers.*]` manual command settings are not
supported.

The extension host is part of normal product builds and the `just dev` watch
set. There is no disabled-host build path or Rust-only probe environment
variable. Extension-defined filename and first-line rules participate in
language detection before built-in suffix rules.
