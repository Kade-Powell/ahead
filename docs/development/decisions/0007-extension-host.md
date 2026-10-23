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
  Zed-style user-granted extension capabilities. The
  `granted_extension_capabilities` setting is read only from user-level
  `~/.ahead/settings.toml` and defaults to Zed's grants when absent. A process
  command must be both requested by the extension manifest and granted by the
  user; `download_file` and `npm:install` require a matching user grant.
  Workspace manifests cannot grant themselves host capabilities. If user grant
  settings are invalid, the extension host fails closed and reports the issue
  in Language Servers rather than silently enabling extensions. The Wasmtime
  engine is shared process-wide,
  while each extension host retains its own guest sessions, following Zed's
  `extension_host/src/wasm_host.rs` engine lifecycle. Extension updates carry
  their ID through the restart notification and evict only that package's
  cached guests; manual language-server restarts clear all guests. On macOS,
  AHEAD selects
  Wasmtime's Unix-signal trap handling instead of its default Mach ports:
  AHEAD launches forked agent/server processes, and the pinned Wasmtime 48
  docs warn that Mach ports do not work well across `fork()`. A serial
  unrestricted proxy suite previously aborted in Wasmtime's Mach handler;
  with signal mode it passes 212 tests (1 ignored).
- **Built-in adapters (current implementation, 2026-10-07):** Rust Analyzer
  follows Zed's worktree-aware resolution: use `rustup which rust-analyzer` for
  a workspace toolchain override when available, validate the executable, then
  try AHEAD's `PATH`, its managed cache, and finally the latest official
  GitHub release. AHEAD validates the installed binary and checks the release
  digest when GitHub provides one. Vtsls and BasedPyright use a runnable binary
  on `PATH` or an AHEAD-owned npm cache; a cache miss triggers a background
  install with lifecycle scripts disabled. Vtsls shares one process across
  TypeScript, TSX, JavaScript and JSX. Project trust gates startup. Native
  acquisition, restart and recovery checks remain tracked in `TODO.md`; unit
  and proxy tests alone do not prove the full user experience.
- **File icon themes, DAP extensions, snippets, color themes and MCP:** out of
  scope. The Extensions tab installs language-server packages; file and folder
  glyphs remain AHEAD's built-in icons.

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

The Extensions gallery queries Zed's live catalog and versioned package
download endpoint in both development and release builds, per the product
decision recorded on 2026-10-14. It filters to language-server packages whose
manifest and WASM API versions AHEAD can parse, then labels them as compatibility
candidates. The UI states that AHEAD is independent and not endorsed by Zed;
API-version compatibility does not guarantee that an extension or its server
will work in AHEAD. The app does not clone and execute source while browsing or
installing.

Installed language grammars now have a separate production highlighting path.
The proxy loads a matching extension's bounded `grammars/*.wasm` and
`highlights.scm`, parses the live buffer off the UI thread, and returns plain
capture names and UTF-8 byte ranges. A bounded cache reuses the loaded grammar
and query while checking safe file-size/mtime stamps for asset changes; the
parser/query cache is keyed by extension and asset digest. The app rejects
stale path/generation/buffer-length responses before applying captures through
GPUI's active syntax theme and `TextDecorationCollection`. This follows Zed's
[`LanguageRegistry::get_or_load_grammar`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/language/src/language_registry.rs)
and `with_parser` lifetime pattern without passing Tree-sitter types across the
GPUI boundary. `injections.scm` now supports static or captured language names,
`injection.content`, optional `injection.host`, combined ranges and nested
injections using grammars from installed extensions. Parsing is bounded by
source/injected-byte, range, capture and nesting limits. `locals.scm` remains
unsupported, and rendered native verification is still open. Injection lookup
and grouping follow pinned Zed's
[`get_injections`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/language/src/syntax_map.rs#L1667)
and `language_for_name_or_extension` behavior; AHEAD intentionally omits Zed's
incremental changed-range queue because this path reparses one bounded snapshot.
GPUI Kit pins Tree-sitter 0.26.13, whose WASM feature brings Wasmtime C API 36;
the extension guest host remains on Wasmtime 48. This is a separate grammar
engine with a measurable first-build cost, not a second GPUI parser type.

The optional AHEAD additions feed remains available for packages reviewed,
built and published by AHEAD. Zed's public `extensions.toml`, `.gitmodules` and
submodule gitlinks identify source repositories and pinned revisions, not
installable archives. The publisher reviews each source/license and requested
host capability, builds and tests at the pinned revision, then publishes
metadata with source revision, package hash and AHEAD-owned download URL. A
live Zed result takes precedence over an AHEAD feed entry with the same ID.
AHEAD entries supplement missing Zed results and remain available when its
catalog cannot be fetched. This feed is currently empty.

The client reads `extensions/registry.json` locally in debug builds and from
AHEAD's public `main` branch in release builds, and queries Zed's live catalog
in both. AHEAD feed entries require a SHA-256 and an asset URL under this
repository's GitHub Releases, named `<id>-<version>.tar.gz`. Zed rows win over
AHEAD rows with the same ID; AHEAD rows supplement missing Zed results and
remain as fallback entries when Zed is unavailable. The checked-in feed is
empty. A manual-only
publisher builds from a pinned source checkout and CLI revision; individual
packages may pin separate repositories and commits, including submodules from
Zed's public extension catalog. The publisher checks repository identity,
clean state, revision, license and package metadata, then publishes a versioned,
hashed AHEAD GitHub Release asset and updates the feed. Offline tests cover
both the monorepo source and a pinned catalog-submodule source, but the workflow
has not been run, no AHEAD package has been published, and the rendered native
install journey remains unverified. See
[`extensions/README.md`](../../../extensions/README.md) for the feed contract.

Current endpoint/terms review (rechecked 2026-10-14): AHEAD calls the public
`https://api.zed.dev/extensions` catalog and versioned package-download route.
The pinned Zed source at
[`extension_host.rs`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/extension_host/src/extension_host.rs)
also shows that Zed rebuilds its local installed-extension index from installed
directories; that code does not establish third-party access rights for
AHEAD's client. Zed's [publishing
guide](https://zed.dev/docs/extensions/publishing/publishing-guide) describes
review and packaging through `zed-industries/extensions`, and its [license
requirements](https://zed.dev/docs/extensions/publishing/license-requirements)
specify which extension binaries Zed distributes; that license requirement
covers extension code, not external tools an extension downloads or uses.
AHEAD's registry review must assess those language-server binaries and download
terms separately. The [Terms of Service](https://zed.dev/terms), last updated
2026-03-02, require access through Zed-designated mechanisms (section 2.2) and
prohibit non-public APIs (section 2.4), but do not name the extension catalog
or package routes. Their availability to third-party clients is therefore
unconfirmed; this is not a legal determination. The user's release direction
is to use these live routes with a compatibility/non-endorsement disclaimer.
That disclaimer does not grant service permission. Confirm endpoint-specific
authorization before broad commercial distribution; retain the AHEAD-reviewed
feed as the fallback if Zed restricts direct access.

Publisher reference, rechecked 2026-10-08: Zed's public
[`extensions` repository](https://github.com/zed-industries/extensions) records
source repos and pinned commits in `extensions.toml`, `.gitmodules` and
submodule gitlinks. Its [CI workflow](https://github.com/zed-industries/extensions/blob/main/.github/workflows/ci.yml)
pins the `zed-extension` CLI, builds and validates changed packages, and
publishes them from `main` to Zed's object store; the packaging flow is visible
in [`package-extensions.js`](https://github.com/zed-industries/extensions/blob/main/src/package-extensions.js).
The source repository's Releases page has no package assets. Its public index
alone cannot provide installable packages. AHEAD's direct live Zed API path is
the release default by product decision; the optional reviewed AHEAD feed can
supplement it or serve as a fallback if the endpoint is unavailable or
restricted. Do not clone and execute source while users browse or install.

The existing install RPC carries the selected package version and an optional
archive SHA-256. The host compares the manifest version and, when supplied,
the digest before replacing a package. Zed's live catalog does not supply
a digest; the AHEAD publisher must include one for every production package.

The extension host is part of normal product builds and the `just dev` watch
set. There is no disabled-host build path or Rust-only probe environment
variable. Extension-defined filename and first-line rules participate in
language detection before built-in suffix rules.
