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
| Search and replace | In Text mode, search for the literal `console.log(` and confirm it finds the JavaScript and TypeScript call sites without a regex error. Switch to Regex mode and confirm the unmatched `(` is reported as an invalid regex; switch back to Text and confirm literal results return. Then find `AHEAD_SEARCH_NEEDLE`, try Text, Word and Regex (exactly one mode selected; Case remains independent), include/exclude patterns, keyboard result paging, and opening a full match range. Replace matches in an unsaved editor buffer. | Parentheses are literal in Text mode and parsed only in Regex mode; switching modes clears stale results/errors. Results select the exact range; filtering and paging are correct; replacement is one undoable unsaved edit, opens affected tabs, and does not write the files to disk |
| Command palette | Press Cmd/Ctrl+Shift+P or F1 from the editor and focused terminal; move past eight results, type `git`, use arrows and Enter to open Source Control; reopen and press Escape | Filtered commands, visible keyboard selection, execution, focus restoration and dismissal |
| Language/LSP | With Rust Analyzer, complete `math::dou` to `math::double` in an unsaved edit; break/fix a call; test hover, signature help (Cmd/Ctrl+Shift+Space or Editor menu), definition/back, references, rename, Format Document (Shift+Alt+F or Editor menu), and code actions (Cmd+. / Ctrl+. or Editor menu); undo formatting and the code action; exercise a server-originated `workspace/applyEdit`; switch files during a pending reply. In TypeScript, remove the `calculate` import and test its quick fix. Open JSON for syntax highlighting. | Real suggestions and correct insertion; active signature parameter and overload switching; diagnostics clear; formatting and workspace/code-action edits are each one undoable change; advertised server commands run; verify the `workspace/applyEdit` response reports the actual applied result; resource create/rename/delete operations remain unsupported and fail closed; stale replies are ignored |
| Completion popup and details | In `src/main.rs`, trigger completion on a line with room above the caret and type a common prefix that returns many matches. Move through and accept one suggestion. Then request signature help inside a function call and move between overloads/parameters. | The popup appears above the caret when there is room, shows no more than five ranked suggestions, and displays useful type/signature details beside each label. Signature help shows the active parameter and its type/documentation without obscuring the current line. |
| Git and editor gutter | Commit the fixture baseline, edit and delete separate lines, then inspect Source Control and the editor gutter. With and without an active AHEAD session, check the labeled Add Comment action in the file toolbar and its prerequisite tooltip. Select a code range, start a session, then click the action and save; clear the selection and confirm breakpoint clicking remains distinct from the change rail. Hover the breakpoint and change marker; click the change marker, copy its diff preview, and stage/unstage/commit. Inspect active-line blame, then disable and re-enable its setting. | The primary Add Comment button stays visible in the file toolbar at narrow widths; its tooltip explains the selection/session prerequisite and saved comments use the selected range. Breakpoint tooltip and hit area remain distinct from the change rail; each hunk renders as one marker with an in-place old/new preview; copied diff matches the visible hunk; blame shows author/time by default and follows the setting; Git history matches the commit |
| Terminal/tasks | In **Tasks**, click `verify-python`; confirm it opens a new terminal titled `verify-python` and runs `just verify-python`. Then run `just verify`; create a second terminal, rename its tab from the terminal context menu, save with Enter, and cancel a second rename with Esc. Select/copy shell output, paste a command back into the PTY with the platform shortcut and context menu, and use **Paste Text**. Copy `data/path with apostrophe's name.txt` from the file manager and paste it into the shell. Check Zed's platform shortcuts: macOS Cmd+C/V and Ctrl+Cmd+V; Linux Ctrl+Shift+C/V and Ctrl/Shift+Insert; Windows Ctrl+Shift+C/V, Ctrl/Shift+Insert and Ctrl+V. Confirm Ctrl+C reaches the shell on Linux and when Windows has no selection; test image paste in a clipboard-aware TUI and bracketed paste in an application that enables it. | The Just recipe opens an independent terminal under its own name and runs successfully. Tabs stay independent; rename applies only to the selected tab, Escape leaves its title unchanged, and an empty name restores the shell title. Command executes and output can be copied; the pasted path is one shell-safe argument; Paste Text inserts clipboard text; shortcuts match the host OS; interrupt, image and bracketed-paste behavior are distinct and correct. |
| Agent chat | Select a code range and send a turn; clear selection and send without implicit file context; then explicitly use `@currentFile`. Type `/`, filter to the project skill, select it, and ask for its verification token. | Only the selection or explicit file reference is included; the palette filters skills as typed and inserts the selected skill; the activated skill returns `AHEAD_EDITOR_SMOKE_SKILL_LOADED`; no tool is duplicated as a slash command |
| Instructions, skills and FIM | Inspect root, `src/`, `docs/` and unrelated `data/AGENTS.md`; open `src/main.rs` and `docs/reference.md`, then make a FIM request from `src/`. Type `/` and select `.agents/skills/ahead-smoke-skill/SKILL.md`. | The FIM request includes root+`src/` instructions for the active file and root+`docs/` instructions for the open buffer, but excludes `data/AGENTS.md`; chat uses the applicable hierarchy, and the standard project skill filters, inserts and activates |
| Managed MCP | Explicitly approve the local `mcp_echo.py` server, ask the managed agent to call `echo`, then list resources/templates and read `memo://ahead-smoke`; deny one list and retry | The default fixture requires MCP 2026-07-28 discovery, per-request metadata, and modern result/cache fields. One approval per operation; denied list never contacts the server; exact echo arguments, listed resource/template and `MCP_RESOURCE_SENTINEL` content |
| Managed MCP form | Start the local mock below, enable `mcp_echo.py` with `--form`, approve its exact declaration in Settings, and ask the agent to call `ask_form`. Enter count `2`, skip color, and enter nickname `Nora`. | The card shows all three fields; the server trace contains `{"action":"accept","content":{"count":2,"nickname":"Nora"}}`; the managed turn completes. Also exercise Decline and Cancel separately. |
| Managed MCP URL | Start the local mock in `url` mode, enable `mcp_echo.py` with `--url`, approve its exact declaration in Settings, and ask the agent to call `ask_url`. Review the destination, then try Open, Decline and Cancel in separate turns. | The card names the server and shows the full `https://example.test/connect?state=abc` destination. Only Open attempts browser navigation; the server trace records the selected action without chat-collected credentials. |
| Models and provider visibility | Start two local catalogs with `node mcp_form_model.mjs /tmp/ahead-model-a.port /tmp/ahead-model-a.trace catalog-a` and `node mcp_form_model.mjs /tmp/ahead-model-b.port /tmp/ahead-model-b.trace catalog-b`. Add two OpenAI-compatible connections in Settings using `http://127.0.0.1:PORT/v1` and `shared-model`; open Model visibility, expand and collapse each provider, scroll through its 121 models with the pointer inside and outside the list, and hide `shared-model` under only one provider. Load the agent panel and verify one `model_catalog_request` per configured provider in each trace. Open the chat picker twice, search by provider and model name, select a filtered result, and rerender the open picker; these UI actions must not start duplicate discovery. Reload Settings and verify one fresh request per provider, then stop both mock servers. | The agent panel and Settings reload discover current catalogs; opening or rerendering the picker does not duplicate requests. Both sections start collapsed. Each list is virtualized with a visible scrollbar/position, and `shared-model` is hidden only for the selected provider. Scrolling over the model list moves only its rows and scrollbar while the provider heading and Settings page stay fixed, including at the list's top and bottom; scrolling outside the list moves the page. The other provider remains available. The chat picker opens with its search field focused, filters as the user types by model or provider, and lets the user select a result. Picker rows lead with model names and show provider identity as muted secondary text. No external credentials are needed. |
| Combined model and effort selector | Restart the two local catalogs from the preceding row if they are no longer running. In the managed chat, select `Shared Model` from catalog-a, move the discrete effort slider from Medium to High, and reset to the model default. Then select the identically named model from catalog-b; stop both catalogs after this row. | One compact selector contains effort, searchable model/provider selection, and the gpui-kit slider. Catalog-a shows Low/Medium/High with Medium as default; reset returns to Medium. Catalog-b's same model ID has no inherited effort choices from catalog-a. |
| Zed language extensions | In the development preview, search the live catalog, install a compatible language extension, open a matching file, then restart its server and test completion/diagnostics. Keep the test to reviewed packages and disposable copies. | The package installs and activates without restarting AHEAD; the server reaches Ready and provides the expected language feature. This preview catalog is not production registry acceptance; record that gate separately in `TODO.md`. |
| External ACP agents | When the corresponding CLI and login are available, install/launch only Pi, Codex or Claude Code; inspect each agent's model and boolean options, send a disposable turn, cancel it, and resume the same session. | Only the curated agents are offered; configured options reach the agent, streaming/cancellation/resume work, and external-process effects are not presented as AHEAD-enforced. Record provider/auth-dependent gaps in `TODO.md`. |
| FIM and managed agent edit | With a configured disposable test provider, request a small edit in a nested file and accept a completion at the caret. Inspect the agent-authored range in the gutter, then stage and commit it. | FIM includes live buffer and applicable session/instruction context; stale suggestions are discarded; accepted text lands at the caret; the managed edit has CodeAnchor attribution through the commit. No production credentials or real project files are used. |
| Advanced integrations | Voice and debugger are separate milestones; exercise only with the required device/server and adapter | Record results separately; these are not core broad-use release gates |

For local development, start the editor from the AHEAD repository root with
`just dev /absolute/path/to/disposable-copy`. The copied project contains its
own `justfile`; run `just verify` from that project directory. When starting
from the AHEAD repository root, first change the terminal's working directory
to the disposable copy with `cd /absolute/path/to/disposable-copy`, then run
`just verify`. This runs the offline Rust, TypeScript/JavaScript and Python
tests. Its Rust lane invokes Cargo, so stop the AHEAD dev watcher with Ctrl-C
before verification and restart `just dev` on the same project afterward.
The checks require Cargo, Node 24+ with npm, and Python 3.9+; no npm or pip
package installation is needed. The Node tests execute TypeScript but do not
replace a type-checker or an LSP check.

`mcp_echo.py` is an offline stdio server for the Managed MCP row. Its default
mode requires MCP `2026-07-28`; `--form` and `--url` intentionally return the
standard unknown-method error for discovery and exercise legacy fallback.
Declare it
under `.ahead/config.toml` as a `python3 -u` server using the script's absolute
path, then explicitly enable and approve that declaration in private
`.ahead/settings.toml`. The library test uses a disposable copy and mock model;
`node tests/agent-mcp-smoke.mjs target/debug/ahead` also exercises modern
discovery, legacy initialization fallback, per-operation approvals,
tool/resource/template calls and returned content through the current proxy
binary. Neither check proves the rendered in-app approval or resource UI; run
the native row in a disposable project for that.

For a provider-free native form run, start `node mcp_form_model.mjs
/absolute/disposable/path/model.port /absolute/disposable/path/model.trace`
from the copied project. Use the written port for a private mock connection at
`http://127.0.0.1:PORT/v1` with model `gpt-5.5`; the mock answers only on
loopback, answers the endpoint check and model discovery request, and returns
the `ask_form` tool call. To verify a plain managed chat turn without enabling
the MCP fixture, pass `text` as the final argument; the assistant replies with
`AHEAD_TEXT_SMOKE_OK`. Declare `mcp_echo.py` with an
absolute trace path and `--form` in `.ahead/config.toml`, then use Settings >
MCP Servers > Refresh > Approve & Enable. Do this after opening the copied
project so the editor creates `.ahead/` itself. Stop the mock and `just dev`
when the run finishes. This tests the native UI, not a real provider.

For the URL row, start the same mock with `url` as the final argument and
declare the stdio server with `--url` instead of `--form`. The destination is
the reserved `.test` domain; it need not load a website to verify the editor's
review, browser-open attempt and server response.

Language support uses installed Zed language extensions and built-in adapters
for Rust Analyzer, Vtsls (TypeScript/JavaScript), and BasedPyright plus Ruff (Python), as
in Zed. Rust Analyzer resolution follows the workspace toolchain override first
(`rustup which rust-analyzer`, when a `rust-toolchain.toml` or `rust-toolchain`
file is present), then a runnable app-PATH binary, then AHEAD's managed cache.
On a cache miss, AHEAD downloads the latest official release into its cache;
the first acquisition needs network access. A rustup shim can exist without
the selected component, so AHEAD validates it before falling through. With
Node/npm available, AHEAD can install Vtsls and BasedPyright into its own cache
when missing from `PATH`.
Python's Ruff formatter/linter runs alongside its analyzer. The fixture enables
BasedPyright's `reportUnusedImport` warning so the smoke can verify that actions
from two servers coexist for the same import; this is not an AHEAD default.
AHEAD prefers
`.venv/bin/ruff` (`.venv/Scripts/ruff.exe` on Windows), then PATH and its cache,
and downloads an official Ruff release into the cache when none is runnable.
Use Language Servers > Restart after a server exits or after installing an
extension. Verify that unsaved text, diagnostics and completion survive that
restart. Stop requests use a five-second graceful shutdown deadline before
force termination. Package updates and remaining native lifecycle checks are
tracked in `TODO.md`.

From the AHEAD repository root, an opt-in networked preview smoke installs the
current compatible HTML extension directly from Zed's live catalog, verifies
its real language server and `<di` completion, then exercises TypeScript,
JavaScript and Python through AHEAD's proxy:

```sh
AHEAD_LSP_INSTALL_MISSING=1 AHEAD_LSP_LANGUAGES=typescript,python \
  AHEAD_LSP_INSTALL_ZED_HTML=1 node tests/lsp-smoke.mjs target/debug/ahead
```

All app data, extension packages and npm caches stay in a sibling OS-temp
profile outside the generated project, so they do not pollute its file tree. A
successful run removes the profile and retains the project for inspection; a
failed run reports and retains both. This is a
development-preview acceptance check, not production registry authorization
or native Extensions-tab verification.

To rerun the language matrix without network access after a successful server
acquisition, pass `AHEAD_LSP_CACHE_SEED=/path/to/cache/language-servers` and
`AHEAD_LSP_INSTALL_MISSING=1`. The smoke copies that cache into its fresh
temporary data home; the source cache is left untouched.

Repeat the Language/LSP row in each language before marking it verified:

| Language | Files and completion | Diagnostic check |
| --- | --- | --- |
| Rust | `src/main.rs`, `src/math.rs`; `math::dou` | Pass a string to `double`, then undo |
| TypeScript | `typescript/src/main.ts`, `math.ts`; `result.dou`; in a new unsaved interface, type `extends Cal` and expect `Calculation` | Call `calculate("21")`, then undo |
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

Build AHEAD first and put installed `rust-analyzer`, `vtsls`,
`basedpyright-langserver` and `ruff` executables on `PATH`; by default the smoke runs all
three language groups. Set `AHEAD_LSP_LANGUAGES=rust` or a comma-separated
subset to run only locally available servers (`typescript` also checks
JavaScript). For a versioned Rust Analyzer binary in Zed's cache,
`AHEAD_LSP_RUST_ANALYZER_PATH` may name its absolute path; the smoke aliases it
as `rust-analyzer` only inside its disposable copy, avoiding a missing-component
rustup shim earlier on `PATH`. The check creates its own disposable copy, starts
the real proxy and language servers, then checks completion, TypeScript
auto-import resolve, shared TypeScript/JavaScript server identity, cross-file
definitions, Rust hover/references (including the declaration)/rename,
diagnostics, unsaved-buffer repair, Rust/TypeScript/Python close/reopen and
workspace language-server restart. Python additionally checks applied,
idempotent Ruff formatting, combined type/lint diagnostics and analyzer hover.
It collects code actions from both servers, resolves Ruff's unused-import fix
through its owning server when needed, and applies it to the unsaved buffer.
The smoke also creates, edits, renames and deletes `pyproject.toml`/`ruff.toml`
and checks the resulting formatting without restarting Ruff. These checks
repeat after restart without saving source files. It then changes Python server
order and hover/formatting routes in `.ahead/config.toml`, disables/re-enables
Ruff and checks errors for a disabled feature target. Native settings-file
events must restart the servers while retaining unsaved text and leaving source
files unchanged. Restart replays unsaved snapshots, clears
retired diagnostics and rejects completion items from the replaced server. It
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
crash. For first-run and storage checks, use a fresh disposable fixture copy.
Pre-production uses one initial Turso schema; legacy database compatibility
and schema-version checks are intentionally out of scope.
