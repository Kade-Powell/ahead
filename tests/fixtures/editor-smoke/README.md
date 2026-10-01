# AHEAD editor smoke project

Copy this fixture to a disposable directory, initialize Git there, and open
that directory with AHEAD. Do not commit generated `.ahead/` state to this
template. It deliberately has no `.ahead/` so each run exercises editor
project setup and the default-deny private-state boundary.

Manual smoke targets:

| Area | Exercise in AHEAD | Evidence to record |
| --- | --- | --- |
| Bootstrap | Open the copy in AHEAD, complete first-time Settings, inspect `.ahead/` | Settings, config, ignore boundary |
| Editor | Open `src/main.rs`, edit, undo, save, reopen | Dirty state, persisted text |
| Recovery | Make unsaved edits in TS/Python; restart the dev loop; reopen the same copy. Repeat with a changed or missing disk file and a second window. Try File > Recover Unsaved Changes. | Last acknowledged text restored as dirty; no source-file write on restore; changed-baseline overwrite prompt; current dirty tabs and live windows preserved |
| Search | Find `AHEAD_SEARCH_NEEDLE`; search an unsaved edit; Quick Open `src/math.rs:1` | Correct path/range and buffer contents |
| Command palette | Press Cmd/Ctrl+Shift+P or F1 from the editor and focused terminal; move past eight results, type `git`, use arrows and Enter to open Source Control; reopen and press Escape | Filtered commands, visible keyboard selection, execution, focus restoration and dismissal |
| Language/LSP | With a Rust language server, complete `math::dou` to `math::double` in an unsaved edit; break/fix a call; test hover, signature help, definition/back, references, rename, formatting and code actions; switch files during a pending reply. Open JSON for syntax highlighting. | Real suggestions and correct insertion; diagnostics clear; edits and stale replies behave correctly |
| Git | Review changed file, diff, stage/unstage and commit in the copy | Correct attribution and history |
| Terminal/tasks | Run `just verify` and interact with the PTY | Keystrokes, output, exit status |
| Agent chat | Select a code range, send a turn; clear selection, then use `@currentFile` | Only selected text or explicit full file is sent |
| Instructions/skills | Inspect root and nested `AGENTS.md`; type `/smoke` in chat | Applicable hierarchy and filtered skill |
| External ACP/MCP, models, FIM, voice, debugger | Exercise when adapters, providers, devices and debug tools are available | Record untested gates in `TODO.md` |

For local development use `just dev /absolute/path/to/disposable-copy` from
the AHEAD repository root. Keep one shared Cargo build loop active.
`just verify` runs the offline Rust, TypeScript/JavaScript and Python tests.
It requires Cargo, Node 24+ with npm, and Python 3.9+; no npm or pip package
installation is needed. The Node tests execute TypeScript but do not replace
a type-checker or an LSP check.

Language support uses installed Zed language extensions and built-in adapters
for Rust Analyzer, Vtsls (TypeScript/JavaScript), and BasedPyright (Python), as
in Zed. The built-in executables currently need to be on the app's `PATH`:
`rust-analyzer`, `vtsls`, and `basedpyright-langserver`. A rustup shim alone
does not guarantee Rust Analyzer is installed. Use Language Servers > Restart
after a server exits or after installation. Verify that unsaved text, diagnostics
and completion survive that restart. Stop requests use a five-second graceful
shutdown deadline before force termination. Automatic acquisition and the
remaining native lifecycle checks are tracked in `TODO.md`.

Repeat the Language/LSP row in each language before marking it verified:

| Language | Files and completion | Diagnostic check |
| --- | --- | --- |
| Rust | `src/main.rs`, `src/math.rs`; `math::dou` | Pass a string to `double`, then undo |
| TypeScript | `typescript/src/main.ts`, `math.ts`; `result.dou` | Call `calculate("21")`, then undo |
| JavaScript | `javascript/src/main.mjs`, `math.mjs`; `tri` | Call `triple("14")`, then undo |
| Python | `python/main.py`, `arithmetic.py`; `dou` | Call `double("21")`, then undo |

Use each cross-file function for definition, references and rename. Search
the language-specific `AHEAD_*_NEEDLE` markers and an unsaved Unicode edit.
Opening two TypeScript/JavaScript files should share one Vtsls process.
Do not treat passing fixture tests as evidence that these native UI paths work.

The opt-in proxy check runs from the AHEAD repository root:

```sh
node tests/lsp-smoke.mjs target/debug/ahead
```

Build AHEAD first and put installed `rust-analyzer`, `vtsls` and `basedpyright-langserver`
executables on `PATH`. The check creates its own disposable copy, starts the
real proxy and language servers, then checks completion, TypeScript auto-import
resolve, shared TypeScript/JavaScript server identity, cross-file definitions,
diagnostics, unsaved-buffer repair, Rust/TypeScript/Python close/reopen and workspace
language-server restart. Restart replays unsaved snapshots, clears retired
diagnostics and rejects completion items from the replaced server. It
checks graceful proxy exit and an empty process group, then prints the retained
copy's path. On failure it terminates only its own process group. It does
not install packages, call model providers, or replace native UI testing.

`node tests/lsp-shutdown-smoke.mjs target/debug/ahead` uses disposable fake
language servers instead of installed ones. On Unix it checks explicit
shutdown, stdin disconnect, disconnect during initialization and an
unresponsive server. Each case must stop its server without saving the buffer.

`node tests/editor-recovery-smoke.mjs target/debug/ahead` separately checks
recovery ownership, abrupt proxy termination, reopen and explicit discard in
a fresh temporary project. It needs no language servers. The editor captures
periodically, so do not expect the last unacknowledged keystroke to survive a
crash. Keep old-schema fixtures intact and use a fresh copy if startup rejects
their database; there is no automatic migration.
