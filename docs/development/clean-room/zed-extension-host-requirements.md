# Zed language-extension host requirements

Status: requirements-only reference for an independent AHEAD implementation.

This document describes observable behavior and acceptance criteria. A
clean-room implementation must be developed from this document, public
protocol specifications, and black-box tests without loading imported source
files. Imported source, provenance, notices and license texts are kept outside
the clean-room context.

## User experience

Built-in and extension-provided language adapters share one native LSP client.
Rust, TypeScript/JavaScript and Python should work without writing custom server
commands. Extensions add languages or replace a built-in adapter; a selected
extension's failure must be visible instead of silently falling back.

For extension-provided languages:

1. The user installs a language extension through Settings.
2. AHEAD pins and stores the extension package.
3. Opening a matching file selects the extension language definition.
4. AHEAD resolves the language server, installing it into the extension's
   private work directory when necessary.
5. AHEAD starts the server through the native LSP client and reports clear
   installing, ready, stopped and failure states.

Users do not configure language-server commands separately.

## Package and manifest behavior

- An extension is a directory containing `extension.toml`.
- The manifest identifies the extension, version, schema version and source
  repository.
- Language definitions live under `languages/<id>/config.toml`.
- A language definition provides a display name, grammar name, file suffixes,
  comment syntax and optional first-line matching.
- Grammar entries identify a source repository and immutable revision.
- Language-server entries identify a server name and the language names it
  serves.
- Language-server entries may map editor language names to protocol
  `languageId` values.
- Snippets, themes, icon themes, context/MCP servers and unrelated extension
  features are ignored or reported unsupported.

## Extension execution boundary

- Procedural extension code runs as a WebAssembly component using the pinned
  Zed extension WIT contract.
- The host supplies only the capabilities required for language-server
  resolution: extension work-directory access, worktree inspection, binary
  lookup, shell environment lookup, controlled downloads, executable-file
  preparation, settings reads and installation-status reporting.
- Every capability is default-deny and scoped to the extension and workspace.
- Downloads are restricted to the extension work directory and use bounded
  size, timeout and archive validation rules.
- Process execution is mediated by AHEAD. The extension returns a command;
  the AHEAD LSP boundary starts it.
- Unsupported extension capabilities return explicit unsupported errors.
- Extension failures never crash the editor or silently select another server.

## LSP bridge

The resolver returns:

- executable path or command name;
- ordered arguments;
- environment additions;
- working directory;
- optional initialization options;
- optional workspace configuration;
- installation status and a human-readable error.

The bridge maps the result to the existing AHEAD LSP client. It starts at
most one server instance per resolved server/workspace combination, sends the
initial open document, and routes subsequent changes, saves, requests,
notifications and diagnostics through the existing proxy boundary.

## Storage and lifecycle

- Installed package contents are immutable at a pinned revision.
- Extension work data and downloaded servers are separate from the package.
- A successful server resolution is cached by extension revision, platform,
  architecture and resolver inputs.
- A failed resolution is visible and retryable.
- Updates are explicit and replace the package atomically after validation.
- Uninstalling an extension stops its servers and removes only its package and
  private work data.

## Acceptance tests

- Install a declarative language extension and match a real file by suffix.
- Load its language configuration and queries.
- Run a procedural extension whose server is already on `PATH`.
- Run a procedural extension that downloads and caches its server.
- Verify the returned command starts through AHEAD's LSP client.
- Verify diagnostics, completion and document changes round-trip.
- Reject path traversal, archive escape, oversized downloads and unsupported
  capabilities.
- Show actionable errors for missing binaries, failed downloads and resolver
  failures.
- Verify no manual language-server command configuration is consulted.
