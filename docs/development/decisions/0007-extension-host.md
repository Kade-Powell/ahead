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
  declared process capabilities. The Wasmtime engine is shared process-wide,
  while each extension host retains its own guest sessions, following Zed's
  `extension_host/src/wasm_host.rs` engine lifecycle. On macOS, AHEAD selects
  Wasmtime's Unix-signal trap handling instead of its default Mach ports:
  AHEAD launches forked agent/server processes, and the pinned Wasmtime 48
  docs warn that Mach ports do not work well across `fork()`. A serial
  unrestricted proxy suite previously aborted in Wasmtime's Mach handler;
  with signal mode it passes 212 tests (1 ignored).
- **Built-in adapters:** Rust Analyzer, Vtsls and BasedPyright currently resolve
  their executables from the app's `PATH`. Vtsls shares one process across
  TypeScript, TSX, JavaScript and JSX. Missing executables are reported through
  language-server status. Automatic acquisition and process restart are still
  release gaps in `TODO.md`; the adapter table alone does not prove usable
  language support.
- **File icon themes (added 2026-10-02):** AHEAD accepts icon-only Zed
  extension packages without a Wasm component. The Extensions tab installs
  them through the same bounded package path, and the explorer reads their
  `icon_themes/*.json` file and referenced SVGs for file stems, suffixes and
  folder names. Zed's built-in stem and suffix associations apply first, then
  the extension's JSON mappings override them. The built-in map in
  `ahead-extension-host/src/default_icon_associations.json` tracks the pinned
  [Zed associations](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/theme/src/icon_theme.rs).
  The selected icon theme is stored in the user config directory; Material Icon
  Theme is the default when installed. Invalid mappings fall
  back to the built-in file and folder icons.
- **DAP extensions, snippets, color themes and MCP:** out of scope for this path.

## Priority reminder (from 0006)

Ecosystem crates first; Zed source second. `LspClient` already speaks the
protocol. The Apache WIT contract is retained as a versioned compatibility
boundary; AHEAD's host implementation is independent code and keeps the
required provenance in `ahead-extension-host/NOTICE`.

## User flow

The Extensions center tab lists compatible installed language and file icon
extensions from local manifests even while offline. It can load live catalog
results for both kinds on request, filter language extensions to the Zed Wasm
API versions AHEAD currently hosts, and search, install, update or use a
selected package. Install rows show busy and retry states. The editor
sends the selected package's pinned download URL and ID through the existing
proxy installer, which validates the
manifest, language-server declaration and Wasm component before replacing an
installed package. Downloads and archive extraction are bounded by byte and
entry limits. Production downloads require credential-free HTTPS, including
every redirect; unit tests alone allow loopback HTTP for disposable package
smokes. A root-wide file lock serializes installs and discovery across
proxy processes; an interrupted update's valid backup is restored on discovery
or the next install attempt. Malformed backups are reported without hiding
healthy extensions. Discovery ignores hidden staging directories and rejects
a directory whose name disagrees with its manifest ID.
Hot-path file language lookup does not wait for the install lock; the
post-install restart reclassifies open buffers from the completed package.
After an install, the proxy client requests a language-server restart even if
the Extensions tab has closed; the proxy reclassifies open buffers and rescans installed
extensions before reopening documents. Opening a matching file then discovers
the package and starts its language server;
`[language-servers.*]` manual command settings are not supported. Existing
packages remain visible and installed when the catalog is offline; online
versions merge with local install state. Catalog discovery is
not an extension-compatibility guarantee; a real language-server session must
still be verified for each adapter.

The disposable HTML smoke builds Zed's `extensions/html` guest for API 0.7,
adds the `[lib].version` field that Zed's
[`extension_builder.rs`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/extension/src/extension_builder.rs)
adds after compilation, then serves the package from loopback. AHEAD downloads,
installs, discovers and invokes its settings callback. This proves the package
path offline, not a native Settings click or a live language-server session.

The current preview catalog endpoint is operated by Zed, not AHEAD. Production
installation needs an AHEAD-owned live registry sourced from reviewed
[public extension repositories](https://github.com/zed-industries/extensions),
with independently built, hashed, versioned packages; alternatively, direct
third-party use of Zed's service needs explicit permission. The public
`extensions.toml`, `.gitmodules` and the submodule gitlinks identify source
repositories and pinned revisions, not installable archives. A publisher
should sync that public index, review each source/license and host capability,
build and test a package at
the pinned revision, then publish metadata containing the source revision,
package hash and AHEAD-owned download URL. The app refreshes this published
feed; it must not clone and execute arbitrary extension source just to browse
or install. This keeps the gallery live without baking a list into the editor
or depending on Zed's package service. Do not treat a successful HTTP query
or offline package test as permission or production UI proof. Track the
publisher and native install verification in `TODO.md`.

The existing install RPC carries the selected package version and an optional
archive SHA-256. The host compares the manifest version and, when supplied,
the digest before replacing a package. Zed's preview catalog does not supply
a digest; the AHEAD publisher must include one for every production package.

The extension host is part of normal product builds and the `just dev` watch
set. There is no disabled-host build path or Rust-only probe environment
variable. Extension-defined filename and first-line rules participate in
language detection before built-in suffix rules.
