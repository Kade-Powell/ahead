# Zed feature and strategy tracking for the GPUI port

- Status: Tracking (source import, feature adoption and implementation strategy)
- Date: 2026-09-21
- Context: GPUI + gpui-kit selected (Decision 0003). Lapce is the legacy
  base; Zed is the forward reference for editor behavior, collaboration,
  and GPUI patterns. VSCode remains a secondary reference only for
  interaction ideas, never for extension-compat promises.

## What to track from Zed (upstream: `zed-industries/zed`, mixed licenses)

1. **GPUI patterns**: entity/context lifecycles, focus handling, overlay
   and panel composition, text rendering and input handling. Directly
   applicable — same framework family as our port target.
2. **Multi-buffer editing**: excerpts, project panels, tabs/splits,
   selections, undo/redo, IME, symbol outline, diagnostics list, inline
   completions presentation. Direct source reuse is preferred.
3. **Collaboration**: channels, shared buffers, presence, following,
   version-vector or CRDT buffer sync, conflict presentation. Reuse the
   mature transport/rendering patterns where compatible; AHEAD owns session
   policy, identity, anchors and workflow events.
4. **Agent panel UX**: thread list, streaming deltas, tool-call cards,
   checkpoint/rewind affordances and terminal context. AHEAD's panel and
   policy workflow remain different from Zed's.
5. **Search and Champagne**: project search, symbol search, command
   palette ranking, keymap ergonomics.
6. **Terminal and tasks**: named terminal tabs, resizing, task definitions,
   terminal integration, output linking back to buffers and terminal-to-agent
   context. Tasks are driven by repository `just` recipes and commands.
7. **Accessibility**: focus order, live regions, screen-reader labels on
   editor chrome — feeds GPUI acceptance gate 4.

## How to track

- Pin a Zed upstream commit per review cycle (record SHA + date here).
- For each adopted feature: note the Zed file(s) imported, what was pruned or
  adapted, the tests carried forward, and the license/NOTICE carry-forward.
- Directly port the selected editor, search, source-control, terminal/task
  and debugger implementations where that reduces risk. Do not import Zed's
  cloud services, telemetry, agent policy, session model or unrelated product
  workflows; AHEAD's session host, policy and viewmodel remain authoritative.
- Keep the comparison honest: Zed's team size and maturity exceed ours;
  adopt ideas at our pace, gated by the GPUI acceptance gates.

## Review log

- 2026-10-02: compared pinned Zed's UTF-16 open test in
  `../zed/crates/editor/src/editor_tests.rs` (around line 42230) with
  `ahead-app/src/code_panel.rs::read_editor_file`. Zed auto-detects a
  BOM-marked UTF-16 file; AHEAD safely declines to edit it. In a rebuilt,
  uniquely named native AHEAD app against only a disposable project, opening
  a SQLite database and a UTF-16 file rendered the unavailable-text card;
  a chmod-denied file rendered `Permission denied (os error 13)`. The UTF-16
  fixture retained SHA-256
  `3966f6d6e1c4b36d2ec8cd12afd2139b79b5ad247eb75153226000b27073c4be`.
  A UTF-8 file and a zero-byte file both accepted edits and saved on disk.
  With five files open, the active empty file's path appeared in the editor
  header but its tab was outside the visible strip; tab-overflow visibility
  remains a UI gap. After reopening the same test app, an immutable SQLite
  copy retained SHA-256
  `33bac128232d266bdf2eb10403b3aa855d088150665a917de0e84f79ba59b11a`
  after an attempted edit and Cmd+S on its unavailable-text card. The
  reused-preview transition is still unverified.
  Zed-compatible, lossless UTF-16 editing remains a separate decision; no
  Zed source was copied.

- 2026-10-02: compared the current Extensions panel with Zed's pinned
  `crates/extension_host/src/extension_host.rs` gallery query and
  `crates/extensions_ui/src/extensions_ui.rs`. The rebuilt, isolated native
  app displayed 338 live Zed catalog candidates; `html` narrowed them to
  HTML and RsHtml, with Install controls. Clicking HTML Install rendered an
  installed/reloading notice. This is a working browse/install surface, not a
  vetted AHEAD registry or language-server-startup proof. Shell DNS could not
  resolve `api.zed.dev`, although the unsandboxed app fetched the catalog and
  package. The test exposed an isolation leak: `AHEAD_HOME` affected the
  managed agent but not `Directory::plugins_directory`, so HTML was installed
  into the normal debug-profile plugin directory. That newly created package
  was moved intact to a disposable recovery folder; the pre-existing icon
  theme remained untouched. `ahead-core/src/directory.rs` now honors an
  absolute `AHEAD_DATA_HOME` for app data/config/plugins/cache/logs. Its
  focused test passes. A rebuilt native install under that override placed the
  HTML package in the disposable app-data root while the normal debug profile
  remained untouched. The row lingered in Installing after the package reached
  disk; refreshing the gallery showed Installed. After restarting that same
  isolated app with Zed's already-cached `vscode-html-language-server` on PATH,
  opening a disposable `.html` file yielded a Ready HTML server in AHEAD's
  Language Servers panel. The editor footer still said `text`: gpui-kit's
  highlighter registry lacks HTML even though the proxy classified it for LSP.
  In a second native run, the HTML file was open before install. The server
  became Ready within seconds of clicking Install without an app restart, but
  the gallery row remained Installing for over 20 seconds. The proxy callback
  had fired because it dispatched the restart; the panel's foreground result
  wait was the remaining suspect. It now uses the app's async-channel wait
  pattern. In a rebuilt disposable native retest, the row changed to Installed
  without Refresh and the already-open file's HTML server became Ready without
  restarting the app. The
  production path still needs AHEAD-published, reviewed packages and hashes
  sourced from Zed's public `extensions.toml`; no Zed source was copied.

- 2026-10-02: the rebuilt AHEAD binary passed `tests/dap-shutdown-smoke.mjs`
  (running, initializing and full-stderr adapters) and
  `tests/editor-recovery-smoke.mjs` (SIGKILL persistence, stale-snapshot
  fencing, owner-only Turso database permissions) in disposable workspaces
  `ahead-dap-shutdown-ThVFd0` and `ahead-recovery-smoke-Hcf5aY`.
  The same binary passed `tests/agent-shutdown-smoke.mjs`: explicit shutdown
  and stdin EOF both persisted the cancelled partial turn across Turso reopen
  in `ahead-agent-shutdown-4d6YT8` using a loopback fake model.
  The current checkout also passed the focused managed-agent parallel stdio
  MCP regression with two independently approved tool calls, MCP resources,
  and an auto-approved follow-up; it remains a test, not a rendered chat pass.
  These are process and harness checks, not native debugger, recovery or chat
  UI proof.

- 2026-10-02: the opt-in
  `installed_zed_html_extension_starts_a_real_server` test copied the
  already-installed HTML API-0.7 package into a temporary extension root,
  detected `index.html` through its language config, opened the document,
  let AHEAD's Wasm host resolve its command, and observed a successful LSP
  handshake from the installed HTML server. The test passed with the server
  on PATH. This verifies the extension-to-proxy startup seam without
  changing Zed's installation. A further completion probe found a concrete
  incompatibility: `vscode-html-language-server` omits `completionProvider`
  unless the client advertises snippet support. AHEAD currently advertises
  false and rejects snippet edits, so its checked completion request returned
  no items; a diagnostic unchecked request returned HTML tag suggestions.
  Zed advertises true in `../zed/crates/lsp/src/lsp.rs` and parses snippets
  through its GPL-3.0-or-later `crates/snippet`, which is not copied into
  AHEAD. Snippet insertion, live settings changes and the native editor remain
  open gates. The complete proxy library suite then passed (191 tests, one
  opt-in real-extension test ignored by default); the headless app library
  suite also passed (172 tests). Neither is a native-window acceptance pass.

- 2026-10-02: compared `../zed/crates/lsp/src/lsp.rs::LanguageServer::new`
  with AHEAD's `ahead-proxy/src/plugin/lsp.rs::LspClient::new`. Zed spawns
  the selected server directly; AHEAD was running `chmod +x` on every
  absolute server path, including user-owned binaries. AHEAD no longer
  changes executable permissions. A focused non-executable-file regression
  passes, and the rebuilt root binary again passed the full disposable
  Rust/TypeScript/JavaScript/Python LSP smoke (`ahead-lsp-smoke-pgQWKZ`).
  This does not prove real extension startup or rendered editor controls.

- 2026-10-02: rebuilt `ahead --bin ahead` and ran the unchanged
  `tests/lsp-smoke.mjs` in a disposable project using already-installed Zed
  Vtsls, BasedPyright and standalone Rust Analyzer binaries. TypeScript,
  JavaScript, Python and Rust passed unsaved diagnostics, completion,
  definition and repair. TypeScript also passed auto-import resolve; restart
  replaced server instances, replayed unsaved buffers, rejected stale
  completion items and ended with an empty server process group. The successful
  copy was `ahead-lsp-smoke-Nwx1YV`. A restricted first run could not spawn
  Vtsls, and an unrestricted run found a rustup shim without a Rust Analyzer
  component; these were environment prerequisites, not counted product
  failures. The same rebuilt binary passed all four
  `tests/lsp-shutdown-smoke.mjs` cases (shutdown, stdin EOF, delayed
  initialization and unresponsive server) in disposable
  `ahead-lsp-shutdown-eGWJfk`. This proves the rebuilt proxy/server path,
  not rendered editor controls or a first-run language-server installer. No
  Zed source was copied.

- 2026-10-01: compared ACP package launch with Zed's
  `crates/project/src/agent_server_store.rs::LocalRegistryNpxAgent` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. Zed installs a registry
  npx package and resolves its executable before launching Node. AHEAD keeps
  its existing atomic, provenance-checked generations because multiple agent
  threads can resolve and launch concurrently. Under the install lock, AHEAD
  now removes abandoned staging directories and reclaims only generations
  published by this app process after their resolved configs and ACP children
  release shared file leases. Prior-process and pre-lease generations remain
  untouched to avoid deleting a package used by a child orphaned by an app
  crash. Eleven adapter tests and the disposable offline real-npm/Node smoke
  test passed; installed-agent UI and cross-restart reclamation remain in
  `TODO.md`. No Zed source was copied.

- 2026-10-02: checked the live ACP Registry index against Zed's pinned
  `crates/project/src/agent_registry_store.rs`. AHEAD's three curated entries
  (Pi, Codex, Claude Code) currently use npx distributions; binary-only agents
  remain outside its picker. The existing cache reader and background fetch
  were unbounded, and a fixed `.tmp` cache filename could follow a symlink.
  AHEAD now caps both reads at 4 MiB, rejects malformed replacement indexes
  before publishing, rejects symlinked/non-regular cache leaves on Unix, and
  publishes through a unique temporary file. A disposable regression verifies
  the bound and preserves an outside sentinel; the full AHEAD agent library
  suite passes with loopback enabled (134 passed, two ignored). This is cache
  hardening, not binary archive support or a native ACP install proof. No Zed
  source was copied.

- 2026-10-01: Zed's `crates/agent_ui/src/agent_panel.rs::load_agent_thread`
  unarchives a thread when activating it (commit
  `418f89714891f9d8105a3e92e60b9a7a5084d232`). AHEAD's legacy import
  instead preserves the source archive flag. Its Turso wrapper had rejected
  writes to such a resumed thread, although the retained recorder accepts
  them. A focused regression failed before the shared append fix and passes
  after it. In a disposable native app with a synthetic local provider, an
  archived `.jsonl.zst` source imported with three records; after a new turn,
  restart and another turn, Turso held 24 replay records and the provider
  confirmed the previous new prompt was in context. A separate malformed
  rollout displayed a parse error and kept the draft without a provider call
  or new runtime-thread row. These are native UI/model-request observations,
  not authenticated-provider proof. `TODO.md` retains the persistence-error
  surfacing and broader release gates. No Zed source was copied.

- 2026-10-02: compared Zed's
  `crates/agent_ui/src/agent_panel.rs::{new_thread_with_workspace,activate_new_thread}`
  at `418f89714891f9d8105a3e92e60b9a7a5084d232` with AHEAD's
  `ahead-app/src/threads_panel.rs::open_new_thread`. Zed activates an
  ephemeral draft; AHEAD uses a Describe → Review → Start wizard. A rebuilt
  native AHEAD window in a disposable project completed that wizard with a
  loopback provider selected and restored the resulting thread after restart.
  The mock received no model request: the chat composer was not present in
  macOS's accessibility tree, and attempted coordinate input did not reliably
  focus it. This verifies creation and restore, not a managed model turn or
  Zed-equivalent composer accessibility. No Zed source was copied.

- 2026-10-02: compared Zed's
  `crates/agent_ui/src/agent_panel.rs::focus` and active-thread focus handling
  at `418f89714891f9d8105a3e92e60b9a7a5084d232` with AHEAD's Agent
  shortcut and status-bar button. Both AHEAD entry points now focus the chat
  composer when a thread exists, while keeping the panel focusable before
  thread creation. The focused GPUI test passed. In the disposable native app,
  the shortcut accepted keyboard text, Enter sent an authorized request to a
  loopback fake provider, the streamed reply rendered, and both messages
  restored after restart. A later isolated window exposed the composer as a
  labeled AX text entry after it was clicked, although AX still reported the
  window as focused; screen-reader operation remains unverified. The
  model's CodeModeOnly metadata omitted top-level tools because AHEAD does not
  bundle the required host; this remains a release gate in `TODO.md`. A second
  native loopback turn with direct-tool `gpt-5.5` showed `file_search` and
  `read_editor_buffer` calls in one model response, both tool results in the
  next request, rendered tool activity, and a final answer. The buffer read
  correctly reported that the requested file was not open. In a fresh,
  uniquely identified test bundle, AHEAD created a thread, opened a Python
  file, and kept a typed marker unsaved. The next native `read_editor_buffer`
  call returned that marker to the model, the activity row completed, and the
  disk file remained unchanged. No Zed source was copied.

- 2026-10-02: Zed renders a completed context-compaction entry in
  `crates/agent_ui/src/conversation_view/thread_view.rs::render_context_compaction`
  at `418f89714891f9d8105a3e92e60b9a7a5084d232`. In AHEAD's isolated
  native app, `/compact` reached the loopback model with the retained runtime's
  checkpoint prompt, but the completed chat turn showed "No response received
  from the agent." The managed controller now persists and streams the concise
  completion notice "Context compacted." after a successful explicit compact.
  A rebuilt app rendered the notice, and it restored after process restart.
  This proves the synthetic-provider UI path, not an authenticated provider or
  failed/cancelled compaction path. No Zed source was copied.

- 2026-10-02: Compared Zed's repository event subscriptions in
  `crates/git_ui/src/git_panel.rs` and Git commit path in
  `crates/git/src/repository.rs` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. In an isolated AHEAD app
  and disposable Git project, a synthetic `gpt-5.5` Responses turn issued
  `apply_patch`; AHEAD applied it and showed a violet CodeAnchor rail. Source
  Control initially stayed at zero changes because it only refreshed at
  construction or after stage/commit. It now refreshes on activation and has
  a manual refresh control; the rebuilt app showed the changed Python file,
  staged it and completed a native commit. Git recorded `ahead` as author,
  the test human as committer and an `Ahead-Session` trailer. The rail cleared
  after commit. Explorer still needed manual refresh to clear its stale `M`
  badge, so switching back to Explorer now refreshes it. A rebuilt-app pass
  added and removed a disposable external edit: Source Control and Explorer
  both showed the change on activation and cleared it on the next activation.
  Event-driven status updates,
  authenticated models, multi-session trailers and hooks/signing remain open.
  No Zed source was copied.

- 2026-10-01: compared Zed's diff-hunk gutter and expansion in
  `crates/editor/src/element.rs`, `crates/editor/src/git.rs` and
  `crates/buffer_diff/src/buffer_diff.rs` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. AHEAD's single
  continuous Git rail now has its own click target; the breakpoint target
  previews a red dot on hover and retains its F9 tooltip. Clicking a Git
  marker opens a read-only, hunk-anchored before/after card using old lines
  from the proxy's live-buffer Git patch, without changing breakpoint state.
  The focused GPUI interaction and proxy payload tests passed offline. This
  does not yet reproduce Zed's true inline expansion, which inserts deleted
  rows into the editor display map; the gpui-kit editor lacks that block
  mechanism. `TODO.md` tracks the remaining editor work. No Zed source was
  copied. A root binary rebuild passed and the continuous rail was visible
  in the disposable project. In an isolated native copy, clicking the Git
  marker opened the hunk card and did not set a breakpoint. The hover tooltip
  and narrow-gutter layout are still tracked in `TODO.md`.

- 2026-10-01: compared Zed's background diff update and snapshot replacement
  in `crates/buffer_diff/src/buffer_diff.rs` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. AHEAD had cleared
  `CodePanel::git_state` on every edit, leaving the gutter blank until the
  debounced proxy reply. It now retains the current markers during refresh,
  replaces them on success, and clears them on a current-request error or
  file close/switch. No Zed source was copied. The focused GPUI test checks
  marker retention and stale-reply rejection. In `just dev` against the
  disposable Python fixture, markers remained visible after typing and Undo;
  the edit was not saved.

- 2026-10-02: revisited Zed's versioned background diff update in
  `crates/buffer_diff/src/buffer_diff.rs::update_diff`. AHEAD retained markers
  across edits, but its 150 ms timer still sent Git metadata requests during
  sustained typing because it started at the first edit. `CodePanel` now
  cancels the pending task when the buffer revision or repository generation
  changes and waits 150 ms after the latest change. The new GPUI scheduler
  regression failed before the fix and passes after it; the existing
  stale-reply test and all 177 app library tests pass. This reduces redundant
  proxy diffs and whole-file blame requests, but native visual stability is
  unverified while the Mac is locked. No Zed source was copied.

- 2026-10-02: checked the commit handoff against Zed's Git diff/blame split in
  `crates/editor/src/git.rs` and `crates/git/src/blame.rs`. AHEAD's pre-commit
  CodeAnchor policy is its own layer: the GitCommit route used to choose author
  `ahead` for any selected path with an agent anchor, even after a human had
  replaced the anchored quote. Its cleanup already required the quote and
  SHA-256 to match file content. The author decision now uses that same
  predicate; a regression failed before the fix and passes after it. The full
  proxy library suite passes (195 passed, one ignored). The synthetic test
  proves commit metadata and anchor retention, not the rendered user-driven
  gutter-to-commit journey. No Zed source was copied.

- 2026-10-02: checked Zed's staged-commit flow in
  `crates/git_ui/src/git_panel.rs::commit_changes` before fixing AHEAD's
  separate CodeAnchor attribution. The dispatcher used the first matching
  anchor's session ID, losing other source sessions. A real-repository
  regression with two sessions and repeated anchors failed before the fix;
  AHEAD now writes sorted, distinct `Ahead-Session` trailers while retaining
  the human committer and staged-content attribution predicate. The focused
  proxy test passes; a rendered user-driven commit remains unverified.

- 2026-10-02: compared Zed's Git-command commit in
  `crates/git/src/repository.rs::commit` with AHEAD's `git2` helper. AHEAD now
  treats only an unborn HEAD as a root commit; an invalid HEAD object reports
  an explicit error instead of being treated as parentless. A disposable-repo
  regression failed before the fix and passes after it. Zed's path also runs
  normal Git hooks and signing; AHEAD still bypasses both, so release parity
  remains open in `TODO.md`. No Zed source was copied.

- 2026-10-02: compared Zed's acknowledged commit lifecycle in
  `crates/git_ui/src/git_panel.rs` (`commit_changes`, staged-change check,
  pending task, draft retention and error toast) at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. AHEAD's Source Control
  button had no handler even though the proxy had a commit notification.
  The panel now offers `Commit Staged`, sends an acknowledged proxy request,
  disables duplicate submission and shows the returned error while preserving
  the message. The proxy attributes the actual staged index snapshot, not
  unstaged working-tree content; its regression covers agent edits, a human
  rewrite, staged agent content followed by a human rewrite, and an empty
  stage. A focused headless panel test covers missing message/service, and a
  proxy-client test covers the request/result bridge. Native
  click-through remains unverified while macOS is locked. No Zed source was
  copied.
  The commit status now sits above the action so long Git errors cannot push
  the button out of a narrow Source Control panel; its 260-pixel GPUI layout
  regression includes a long failure message. `Stage All` also moved off the
  GPUI thread to an acknowledged proxy request, preserving Git's error text
  for the panel; the disposable-repo dispatcher regression exercises it.

- 2026-10-01: compared Source Control rows with Zed
  `crates/git_ui/src/git_panel.rs` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. Zed lets long paths
  shrink and truncates directory text while keeping row controls fixed.
  AHEAD now start-ellipsizes the path in `ahead-app/src/workspace_panels.rs`
  and reserves width for Git status and selection. No Zed source was copied.
  A focused 260-pixel GPUI layout test confirms both controls remain inside
  the row; formatting and whitespace checks pass.
  A disposable-project native build opened Source Control, but the macOS
  screenshot feed stayed on an older Explorer frame; narrow-window visual
  confirmation is still pending.

- 2026-10-01: compared Zed's
  `crates/command_palette/src/command_palette.rs` modal, available-action
  catalog, fuzzy matching and focus restoration at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. AHEAD now opens a
  gpui-kit dialog from Cmd/Ctrl+Shift+P or F1, filters its implemented shell
  commands as the user types, executes through the same handler as shortcuts,
  and returns focus before dispatch. No Zed source was copied. AHEAD's
  unregistered editor-specific actions are not yet in the catalog. Focused
  headless palette tests passed (3), and the shell shortcut regression passed
  (1). In the disposable native workspace, Cmd+Shift+P opened the palette
  before a file was focused; Escape dismissed it; F1 reopened it; `git` filtered
  to Source Control, and Enter opened that panel. The first native pass exposed
  clipped result rows and missing startup focus. Explicit list height and
  initial shell focus corrected those in the next build. The final compact-
  height adjustment was rechecked in the running macOS app with three visible
  filtered rows and no clipped result. A window-scoped keystroke interceptor
  routes shell shortcuts even while the integrated PTY has focus; F1 from that
  PTY opened the palette in the rebuilt app. Zed's pinned
  `crates/picker/src/picker.rs::scroll_to_item_index` keeps the selected row in
  view. AHEAD now uses the existing GPUI scroll handle for both list scrolling
  and keyboard selection; after nine Down presses, the highlighted Stop
  Debugging row was visible in the native palette. The focused test covers
  scrolling and returning to the first result after filtering. After the
  change, all 152 `ahead-app` library tests, workspace formatting and
  `git diff --check` passed offline.


- 2026-10-01: tightened debugger request submission in
  `ahead-proxy/src/plugin/{dap,catalog}.rs`. Rechecked Zed's
  `crates/dap/src/client.rs::request` and
  `crates/project/src/debugger/session.rs::handle_run_in_terminal_request`
  at `418f89714891f9d8105a3e92e60b9a7a5084d232` (SHA verified).
  AHEAD now uses its existing callback path for Continue/Pause instead of
  spawning a thread to wait for each reply. Launch and async controls retain
  the adapter generation captured before submission. Queued terminal requests
  are rejected after Stop, and an active terminal wait observes cancellation.
  No dependency or compatibility layer was added.

  Three regressions exercise retired-generation submission, post-Stop
  terminal forwarding and async control errors. Formatting and whitespace
  checks pass, but these latest edits are not compiler- or runtime-verified.
  Before those edits, a test-only stripped link using Cargo's documented
  `rustc --lib --profile test -- -C strip=symbols` path still failed with
  `errno=28`. Dependencies were reused, but the test executable was not
  produced. About 232 MiB remained afterward. No further link was attempted;
  no cache or user data was deleted.

  Tracing the terminal path found a separate feature gap: the proxy advertises
  `runInTerminal`, but the editor ignores that core notification and
  `RunDebugConfig.debug_command` is skipped on the wire. Zed emits a typed
  terminal request and awaits a per-request response. AHEAD still needs that
  handoff into its native terminal owner, including argv/cwd/env, error replies
  and cleanup of late/cancelled terminals. The current shutdown smoke never
  requests a terminal and cannot verify this workflow. The missing handoff
  and pending-control admission remain in `TODO.md`.

- 2026-10-01: added typed debugger lifecycle/error updates in
  `ahead-rpc/src/{core,dap_types}.rs`, `ahead-proxy/src/plugin/{dap,catalog}.rs`
  and `ahead-app/src/{proxy_client,debug_bar}.rs`. Rechecked Zed's
  `crates/project/src/debugger/session.rs` shutdown path at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`: cancel boot work, request
  terminate or disconnect with explicit debuggee-termination intent, then
  kill the adapter after the request settles. This is AHEAD-owned code using
  that pattern; no Zed source was imported.

  The editor now distinguishes Starting, Running, Stopped, Stopping, ended
  and failed sessions. It waits for backend cleanup before enabling another
  launch and displays startup/launch/control failures. Stop during Initialize
  releases the pending request by reaping the adapter; running-session Stop
  no longer waits behind stack or terminal requests in the event worker.
  Successful terminate and disconnect both reap the adapter. Disconnect asks
  to terminate the debuggee, but that request is not proof of target cleanup.
  Process generations and state revisions fence delayed control replies and
  stack data. The old continued-only notification was removed, not retained
  as a compatibility alias.

  All 147 app library tests pass with 16 threads; the final rerun, including
  clearing old command errors on proxy EOF, took 7.01 seconds. A cached
  sweep passes the four debugger tests with four threads and GPUI seeds 20–39
  in 0.42 seconds. The new lifecycle test covers error text, retry, stale-session
  notifications and waiting for cleanup. Four new proxy regressions compile
  but could not execute: both test-executable links failed with `errno=28`,
  the second with about 650 MiB free. The failed linker removed its incomplete
  proxy test executable; no cache or user data was deliberately deleted.
  A subsequent offline proxy type-check began rebuilding uncached native
  dependencies and was interrupted with about 398 MiB remaining. Its process
  group exited and no AHEAD build subprocess was left running; this check
  did not complete.
  Previous proxy-suite results do not validate these changes. The root app
  remains stale, the shared watcher stopped and the Mac locked. Full proxy
  tests, a rebuilt-root replay, native debugging and real target/descendant
  cleanup remain open in `TODO.md`.

- 2026-10-01: fixed debugger session/thread routing in
  `ahead-rpc/src/dap_types.rs` and
  `ahead-app/src/{proxy_client,debug_bar}.rs`. Rechecked Zed's
  `crates/project/src/debugger/session.rs` and the selected-thread guards in
  `crates/debugger_ui/src/session/running.rs` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232` (checkout SHA verified).
  This is AHEAD-owned code using those patterns, not imported Zed source.

  `RunDebugConfig.dap_id` had been skipped by serialization, causing the proxy
  to allocate a different ID on decode. It now survives both start and
  terminal-request round trips. Each editor launch gets a fresh ID. Continue,
  step, stop and breakpoint changes target that session; stepping requires an
  actual stopped thread instead of inventing thread zero. Old-session events
  cannot resume, revive or replace breakpoints in the current session.
  The bar subscribes to existing metadata notifications and disables invalid
  controls. Proxy EOF clears debug state and blocks launches until app restart.

  A new launch disconnects the previous handle. The catalog keeps ownership
  while disconnect/cleanup runs, then prunes completed handles on the next
  launch. Review caught and removed an intermediate implementation that would
  have removed the handle before cleanup, leaving catalog shutdown unable to
  wait for it. No new background poller or dependency was added.

  Three app regressions cover serialized identity, two launches, stale events,
  missing thread IDs, keyboard routing and view notifications. Restoring the
  old codec fails with `DapId(2)` received instead of `DapId(1)`. Removing the
  continued-event fence lets an old session clear the new stopped state and
  fails the same regression. Both negative controls were restored. All 146
  app tests pass with 16 test threads in 6.21 seconds. A cached run passes all
  three debugger tests with four threads and GPUI seeds 20–39 in 0.42 seconds.
  All 172 proxy tests pass with four threads in 13.59 seconds, including the
  new catalog-retirement test and existing real-process DAP cleanup tests.
  The catalog test checks ownership with an unstarted handle and a deliberately
  missing adapter; it does not establish a successful debugger launch.
  Formatting and whitespace checks pass. The proxy linker still reports the
  existing macOS compact-unwind section-size warning.

  These are protocol and GPUI tests, not a native debugger session. Typed
  adapter startup/launch/termination errors, acknowledged start/stop state,
  full thread/stack/variable UI and the rebuilt-root smoke remain open in
  `TODO.md`. The Mac is locked, the root executable is stale and the shared dev
  watcher remains stopped after the earlier disk-full link failure. No user
  data or build cache was deleted.

- 2026-10-01: moved the remaining chat model-picker configuration reads and
  timestamp checks off the foreground thread in
  `ahead-app/src/session_panel.rs`. Rechecked Zed's
  `crates/agent_ui/src/model_selector.rs::ModelPickerDelegate::new` and
  `crates/settings/src/settings_file.rs::watch_config_file` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. AHEAD keeps the existing workspace
  watcher, one active background load and one coalesced reload request; no new
  watcher, timer, dependency or compatibility path was added.

  The managed selector and Send control wait for the initial load. Enter
  retains an unsent draft, and skill discovery waits instead of using a
  placeholder model. Reload completion resolves the current provider/model
  identity, preserving a choice made after the read started, and leaves
  conversation status and external ACP/session state alone. Removed choices
  fall back to the first remaining model. A Settings-triggered forced read
  survives coalescing with watcher events, including unchanged timestamps.

  Three new GPUI regressions pass with 10 or 20 scheduler seeds. Temporary
  negative controls each fail at seed 0: foreground IO reads `before-load`
  instead of `after-construction`; losing the forced reload leaves `initial`
  instead of `latest`; capturing selection when loading starts replaces the
  later `second` provider with `first`. All three controls were restored.
  The full editor suite passes all 143 tests with 16 test threads in 6.08
  seconds. A cached run also passes all five model-picker tests with eight
  threads and GPUI seeds 20–39 in 0.70 seconds. Formatting and whitespace
  checks pass.

  These are offline disposable-project tests, not authenticated model turns
  or a native loading/ACP-switching observation. Native checks remain in
  `TODO.md`: the Mac is still locked, available disk space is about 214 MiB,
  and the root executable is stale after the earlier disk-full link failure.
  The shared dev watcher remains stopped; no cache or user data was deleted.

- 2026-10-01: moved Settings bootstrap and reload IO off the UI thread in
  `ahead-app/src/settings_panel.rs`. Rechecked Zed's background
  `crates/settings/src/settings_file.rs::watch_config_file` and
  `SettingsStore::watch_settings_files` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. AHEAD reuses its proxy's existing
  workspace-change subscription instead of adding another watcher or polling
  timer. Rendering no longer reads Settings files.

  Provider controls stay disabled during initial loading, and loading values
  cannot trigger a save. Subsequent reloads coalesce behind active loads/saves;
  failed drafts defer reload until a successful retry. Workspace and save
  generation checks reject stale results. Saving the private layer updates
  only that layer's cached timestamp, so a simultaneous shared/local change
  still gets read and can report an invalid configuration.

  Four new GPUI regressions cover bootstrap contention, watcher notifications
  and removal, reload/edit ordering, and shared changes during a save. The
  initial contention fixture accidentally used `add_window_view`, which drains
  background tasks before returning. Inspection of gpui-pre's test helper
  identified that fixture error; installing the view into an empty window
  tests foreground progress before releasing the lock. With that correction,
  synchronous-loading negative control fails in 5.07 seconds. Removing only
  the generation check fails at seed 0 with `older-external-model` replacing
  `newer-edit`. Both controls were restored. All 140 editor tests pass with
  16 test threads; the reload/edit regression includes 20 scheduler seeds.
  A separate cached run passes all 20 Settings-matching tests with eight test
  threads and GPUI seeds 20–39 in 7.91 seconds.

  This proves the tested Settings paths, not all startup IO. The separate
  chat model picker still reads configuration synchronously, including through
  Settings' completion callback; that follow-up is now explicit in `TODO.md`.
  Native loading presentation and app close/quit remain unverified while the
  Mac is locked and the root executable cannot be rebuilt for lack of disk
  space. The shared dev watcher is still stopped; no cache cleanup was done.

- 2026-10-01: completed the remaining advisory-lock lifetime audit in
  `ahead-agent/src/runtime_support.rs`, `adapters.rs` and
  `ahead-proxy/src/ahead/recovery.rs`. Rechecked Zed's
  `crates/project/src/agent_server_store.rs::LocalRegistryNpxAgent` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232` for the install-before-launch flow.
  AHEAD's locks remain its own coordination mechanism; no dependency, path
  format, storage schema or compatibility path was added.

  MCP approval, ACP installation/registry refresh and default-option writes
  now explicitly unlock at operation exit. Recovery holds its owner lease
  until the owner is dropped and releases temporary claim leases after each
  claim, including error returns. Only successful acquisitions create a guard.
  Lock files stay in place, and live editor recovery rows remain exclusive.

  Duplicate-descriptor regressions pass with the guards. Removing explicit
  unlock made both agent lock tests fail and made both recovery ownership and
  error-release tests fail; the latter could no longer reclaim the abandoned
  row. All negative controls were restored. The full agent suite passes 108
  tests with two opt-in tests skipped, and the full proxy suite passes 171.
  Five concurrent-scheduling replays also pass all four matching agent tests
  and all three recovery tests each. The opt-in offline npm test separately
  passes: both locally packed package generations install and launch using
  isolated configuration/cache, without downloads. This is not authenticated
  Pi verification. Formatting and whitespace checks pass.

  Both package test targets rebuilt within the available disk space. The
  macOS linker still warns that `__eh_frame` exceeds the compact-unwind offset
  limit. These tests do not prove native app quit/crash behavior or
  Linux/Windows lock semantics. Those gates remain in `TODO.md`, along with
  the stale root executable, stopped dev watcher and blocked native UI pass.

- 2026-10-01: moved Settings autosave's read/merge/write operation off the UI
  thread. Rechecked the serialized update queue and atomic writes in Zed's
  `crates/settings/src/settings_store.rs` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. AHEAD uses one worker per Settings
  panel and one pending slot for the latest edit, without a new dependency.
  Each write reads the current file under the existing lock and preserves
  unrelated MCP settings. Superseded completions cannot change the status or
  reload the model picker; pending and failed drafts survive rendering.

  Window close and normal app quit now wait for accepted Settings saves.
  Failures, a five-second wait timeout and newer edits keep the window open.
  Settings generations join the existing buffer snapshot checks before and
  after recovery acknowledgements, including multi-window quit. A timeout
  does not cancel an accepted write. Forced process termination is not covered.

  Five new GPUI tests cover lock contention and coalescing, a concurrent MCP
  write, malformed-file failure and retry, close timeout, and Settings edits
  after an earlier window was prepared. Temporarily restoring synchronous IO
  made the contention test fail with `UI blocked on the lock` in 5.08 seconds;
  ignoring Settings generations made the multi-window test fail in 0.24
  seconds. Both negative controls were restored before the full suite.
  All 136 editor tests pass with 16 test threads, followed by five more
  concurrent full-suite passes (2.28–4.27 seconds each). Formatting and
  whitespace checks pass.

  These are offline tests in disposable projects, not a fresh native-app
  observation. The Mac remains locked, the root executable is stale and the
  shared dev watcher is stopped after the disk-full link failure. Initial
  Settings/bootstrap and external-reload reads still run on the UI thread;
  their remaining work and native/cross-platform checks stay in `TODO.md`.

- 2026-10-01: resolved the two reproduced editor-suite failures without
  increasing deadlines or adding retries. Rechecked Zed's
  `crates/terminal/src/terminal.rs::release_pty_resources`,
  `crates/terminal/src/pty_info.rs`, and queued settings writes in
  `crates/settings/src/settings_store.rs` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. No Zed source or dependency was
  added.

  Terminal sampling showed four cleanup workers in Alacritty's `Child::wait`,
  after their IO joins and termination signals. The sample is retained at
  `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/.tmp38B8TG/terminal-shutdown.sample.txt`.
  Flushing one stalled PTY released its pending shutdown receipt within
  100 ms. A deterministic regression now queues shell output only after the
  reader stops; it failed on the old path in 3.10 seconds. AHEAD now flushes
  the macOS queues after termination, before reaping. Normal shell-exit
  output remains in the terminal snapshot. The three PTY tests pass, including
  receipt completion and the resistant shell/foreground job.

  Settings' close-only lock release failed with a retained duplicate handle,
  matching the shared-lock behavior documented by the local macOS `flock(2)`
  manual and `fs4`. A small scope guard now explicitly unlocks before closing,
  including on early returns. The regression still rejects a second writer
  during the critical section, then acquires immediately while the inherited
  handle remains open. This covers the fork-inheritance mechanism without
  claiming to have captured every earlier flake's exact interleaving.

  The full editor suite passes 131 tests with 16 test threads. Ten further
  concurrent replays also pass all 131 tests each (2.15–3.95 seconds per run).
  Formatting and whitespace checks pass. Native UI,
  Linux/Windows and a rebuilt root executable remain unverified. Settings
  still performs its file operation on the UI thread; that and the other
  advisory-lock owners are tracked separately in `TODO.md`.

- 2026-10-01: investigated the Settings lock flake with the cached editor test
  executable. Thirty isolated lock runs and nine concurrent full-suite runs
  did not reproduce it; locking behavior and test deadlines are unchanged.
  Rechecked Zed's queued settings updates and atomic replacement in
  `crates/settings/src/settings_store.rs` and `crates/fs/src/fs.rs` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`.

  The replay exposed a separate terminal-close failure: two 16-thread runs
  passed 129/130 tests; seven full-suite runs passed all 130. Added failure-only
  PID diagnostics to the resistant-shell fixture and rebuilt the editor test
  target. On the second failure, `ps` showed the shell (`?Es`) but no foreground
  job. Both exact-PID follow-up checks found no remaining process after fixture
  cleanup. This does not identify the cause. Zed's
  `crates/terminal/src/pty_info.rs` kills the foreground group separately from
  the shell; AHEAD's join/termination/reap ordering needs further inspection.
  The two focused terminal tests pass. The concurrent failure and next checks
  are in `TODO.md`; no terminal runtime behavior was changed in this pass.

- 2026-10-01: bounded the editor MCP bridge's stdin and local socket frame
  reads in `ahead-agent/src/acp_client.rs`. Rechecked newline framing in Zed's
  `crates/context_server/src/listener.rs::handle_io` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. AHEAD keeps its existing 1 MiB
  request limit, but now applies it during the read using `Read::take`, not
  after an unbounded allocation. No Zed source or dependency was added.

  Oversized socket input is rejected before newline or EOF. Oversized or
  invalid UTF-8 stdin input stops the reader, so trailing bytes cannot become
  another command. Both new regressions failed with the old reader restored;
  the fixed full agent suite passes 107 tests, with two opt-in tests ignored.
  Boundary-sized frames, UTF-8, fragmented requests and pending-call
  cancellation pass. Queue capacity, socket worker admission and absolute
  frame deadlines remain open in `TODO.md`. This is library/process evidence;
  app linking, native UI and authenticated adapter checks remain pending.

- 2026-10-01: added bounded DAP request completion and direct-child exit
  detection in `ahead-proxy/src/plugin/dap.rs`. Rechecked Zed's
  `crates/dap/src/client.rs::request` and `transport.rs::PendingRequests` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`: they settle requests when the
  transport closes but do not set a general DAP request deadline. AHEAD keeps
  its existing 30-second sync limit and now applies it to async requests too;
  disconnect/terminate retain their five-second limit. The process monitor
  follows AHEAD's existing LSP implementation. No dependency or Zed source
  was added.

  Expiry removes pending callbacks before invoking them, reports the command
  through the editor's supported `ShowMessage` channel, and leaves the adapter
  running so a later request can succeed. Late responses cannot claim another
  request. A per-launch monitor also notices direct-child exit even if a
  descendant keeps stdout open. It retires when that launch is replaced or
  stopped; it does not kill escaped descendants.

  All six debugger process tests pass, including callback reentry, retry after
  timeout, duplicate/late replies, sync timeout removal, and exit detection
  before pipe EOF. The full offline proxy suite passes 170 tests in 25.83
  seconds. Formatting and whitespace checks pass. The editor's status-message
  consumer was inspected, but the new message has not been observed in the
  native app. Startup/launch errors still use an ignored log notification and
  disconnected debug-bar state remains incomplete; both are in `TODO.md`.
  Root app linking and native verification remain blocked by disk space and
  the locked desktop. The shared AHEAD watcher is stopped.

- 2026-10-01: traced the intermittent proxy attribution-test timeout to its
  local mock server. A bounded diagnostic run failed on its third iteration
  after two of four model requests, with the attributed file already created.
  The sample at
  `/private/tmp/ahead-rustc-emit.iespdC/native-startup-target-3.sample.txt`
  captured this test, not its successor: the mock server thread had exited
  while the model client was waiting. A small TCP probe reproduced macOS
  inheriting nonblocking mode from the listener. A read returned `WouldBlock`
  immediately despite its read timeout; switching the accepted socket to
  blocking mode received the delayed byte. The fixture now sets that mode
  explicitly. No agent timeout, retry policy or sandbox was changed.

  The same pattern existed in the production ACP editor MCP bridge. Its worker
  now switches accepted sockets to blocking mode before reading a request.
  A regression splits a request across two writes and forces nonblocking mode
  on every platform; the worker waits for the second fragment, then returns
  the expected credential rejection without invoking editor tools. Zed's
  `crates/context_server/src/listener.rs` uses async socket reads, so its
  transport does not require this blocking-worker adaptation. Reference SHA:
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. No source or dependencies imported.

  Offline validation passed: 105 agent tests (two opt-in tests ignored), all
  168 proxy tests in 24.41 seconds, and ten isolated attribution-test runs in
  0.85–1.05 seconds each. Formatting and whitespace checks pass. The separate
  native parent/child shutdown fixture already has a blocking listener; its
  earlier startup failure remains unexplained despite passing this run.
  Root app linking is still disk-blocked, the dev watcher remains stopped,
  and these results do not establish native or authenticated ACP behavior.

- 2026-10-01: revisited the native parent/child shutdown fixture without
  changing its timeout. Its sandboxed run stopped at the local mock server's
  loopback bind with `PermissionDenied`, not at shutdown. The same compiled
  test passed with loopback access; 21 full `ahead-agent --lib` runs then passed
  (124 tests, 2 opt-in tests ignored each time), ten at four test threads and
  ten at sixteen. The earlier intermittent child-start failure did not
  reproduce, so the cause remains open rather than being treated as fixed.
  The current dirty root app also passed
  `cargo check --locked --offline -p ahead -j 1`; this is compile evidence,
  not a rendered app or authenticated-provider run.

- 2026-10-01: followed up on debugger ownership using Zed's
  `crates/dap/src/transport.rs::StdioTransport`, its pending-request cleanup,
  and `crates/dap/src/client.rs::kill` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. AHEAD now owns each adapter child,
  drains stderr in bounded chunks, and kills/reaps before reporting shutdown
  completion. Registration precedes initialization so catalog shutdown can
  reach a stalled startup. Each launch has its own writer channel and
  generation; old reader failures and queued events cannot stop a replacement.
  Disconnects fail pending requests outside their locks. Removed the synthetic
  initialized event on EOF and the unused delayed-registration notifications.
  No Zed source or dependencies were imported.

  The four focused process tests pass, including a reentrant failure callback,
  initialization interrupted by catalog shutdown, complete stderr writes and
  replacement isolation. The first full proxy run passed 167 of 168 tests;
  the known native child-agent case timed out at 0/4 mock model requests. One
  diagnostic replay passed all 168 in 27.27 seconds. The stack report at
  `/private/tmp/ahead-rustc-emit.iespdC/native-startup.sample.txt` caught the next
  test after the flaky case had finished. That pass and sample do not resolve
  the startup failure. Rust formatting, script syntax and whitespace checks pass.

  Disk pressure still blocks the app link. A proxy-only `cargo check` began
  compiling a different dependency set and was stopped before exhausting disk.
  A temporary compiler capture then allowed a production metadata-only check
  against Cargo's cached app dependencies; it passed. The combined test build
  linked the proxy executable but ran out of space linking the separate agent
  test executable. Running only `ahead-proxy --lib` then built and exercised the
  corrected tests without rebuilding those dependencies. These checks do not
  replace the root app build or the standalone DAP smoke replay.

  The previous root executable/dev bundle remain intact and the shared watcher
  is stopped. Native debugger interaction, graceful debuggee termination,
  escaped descendants, terminal handoff, startup/restart feedback and
  cross-platform cleanup remain in `TODO.md`. Adapter reaping alone does not
  establish complete debugger cleanup.

- 2026-10-01: reviewed Zed's
  `crates/terminal/src/terminal.rs::release_pty_resources` and
  `crates/terminal/src/pty_info.rs` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`, plus the pinned Alacritty PTY
  destructor. AHEAD discarded the IO thread handle and relied on SIGHUP.
  A regression reproduced both a shell and foreground job surviving terminal
  close when they ignored HUP/TERM. AHEAD now retains the handle, stops IO and
  terminates its owned Unix shell/foreground groups before reaping on a worker
  thread. The existing workspace `libc` dependency supplies process signals;
  no Zed source or new dependency version was imported.

  Tab close and authorized workspace shutdown explicitly stop terminals even
  when a panel entity is retained. Cancel and failed recovery leave them alive.
  The quit hook waits briefly for outstanding cleanup, including closed tabs.
  Natural shell exit preserves final output, rejects further input and stops
  the panel's polling task. All 130 editor tests passed, including real PTYs
  and GPUI close/recovery routing. The old process-leak case fails before the
  fix; cleanup in that failed test targets only its two fixture processes.

  Native close/quit is still unverified while the desktop is locked. Detached
  jobs, blocked IO threads and Linux/Windows remain open. A separate debugger
  audit against Zed's `crates/dap/src/transport.rs` found discarded adapter
  handles, unread stderr and missing catalog-shutdown cleanup in AHEAD. Those
  gaps are tracked in `TODO.md`; terminal evidence does not cover DAP sessions.

  The root app's Rust compilation completed, but linking failed with
  `errno=28 (No space left on device)` with about 620 MiB remaining. The old
  root executable and stable dev bundle are intact; neither includes this
  terminal fix. The shared watcher remains stopped rather than repeatedly
  attempting a disk-full build. No caches or project data were deleted.

  Added `tests/dap-shutdown-smoke.mjs` without changing the debugger client.
  Against the last working executable, `ahead-dap-shutdown-I0SOYF` reproduces
  running and initializing adapters surviving proxy exit, plus `EPIPE` on
  adapter stderr before launch. Only its isolated test process groups were
  terminated afterward. Early fixture runs lacked a proxy-ready barrier and
  mishandled stderr write errors; those startup failures are not cleanup
  evidence. The corrected regression, formatting and whitespace checks are
  saved; the debugger fix and a rebuilt-app replay remain open.

- 2026-09-30: reviewed Zed's `crates/agent/src/thread.rs::cancel` and
  `crates/agent_servers/src/acp.rs::AcpConnection::drop` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. Zed cancels child turns and flushes
  pending messages; its ACP connection owns direct-child cleanup. AHEAD now
  connects proxy shutdown to its existing session controller and reuses the
  retained Codex thread manager's concurrent, bounded shutdown-and-wait method.
  No Zed source, dependencies or compatibility paths were added.

  Active turns are marked cancelled before runtimes stop. Workers persist final
  message status before the controller's wait finishes, preserving partial text
  and retry requests. New sessions and turns are rejected during shutdown.
  Native creation/MCP-refresh admission is fenced before the thread snapshot;
  pending editor and human-input requests are released. ACP requests settle
  before the process is killed, and shutdown can interrupt initialization or
  session creation without waiting behind their request timeout.

  The full agent suite passed 104 tests with two opt-in tests ignored. The
  new checks cover native parent/child termination, editor waiters, silent ACP
  requests, session startup and partial-message cancellation. The app build
  passed in 51.25 seconds. `tests/agent-shutdown-smoke.mjs` then passed explicit
  shutdown and stdin EOF through real proxies and Turso reopen in
  `ahead-agent-shutdown-XLHeoY`. The previous dev binary fails the same test:
  reopen reports `failed` instead of `cancelled`. The initial script attempt
  used an invalid work-kind value and timed out before starting a turn; it
  provides no shutdown evidence. All four LSP shutdown regressions also passed
  in `ahead-lsp-shutdown-BDk3YX` after the agent cleanup change.

  An initial native shutdown test hit the known child-startup timeout before
  shutdown began. Added request/event diagnostics; a later process sample at
  `/private/tmp/ahead-agent-shutdown.PmoVjm/startup.sample.txt` captured waiting
  threads but that run progressed. Its separate expected-error assertion was
  corrected: an explicitly stopped runtime may close before delivering the
  normal interrupt event. The tests still require failed prompts and completed
  parent/child termination. Later passes do not resolve the startup flake.

  Native window close/quit remains unverified while the desktop is locked.
  Authenticated agents, pending MCP calls, slow process creation, blocked
  storage, escaped adapter descendants and terminal/debugger cleanup remain
  open in `TODO.md`.

- 2026-09-30: reviewed Zed's
  `crates/project/src/lsp_store.rs::shutdown_language_servers_on_quit`,
  its `on_app_quit` subscription and `crates/lsp/src/lsp.rs::shutdown` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. AHEAD's proxy previously queued
  catalog shutdown and exited before language servers completed the handshake.
  Closing a window also left its proxy running. No Zed source was copied.

  Authorized close/quit now releases buffers and stops that window's proxy
  after recovery confirmation. Cancel, changed buffers and failed recovery
  leave it running. A background reaper handles window-close child cleanup;
  it does not block GPUI's short quit-hook deadline. App-side stdin EOF follows
  the same proxy cleanup path. The catalog starts all server shutdowns before
  waiting on their shared deadline, and the dispatcher waits for catalog exit.
  The dispatcher disconnects its RPC handler without queueing a second shutdown.

  All 15 GPUI lifecycle tests and 21 plugin tests passed. The app build passed.
  `tests/lsp-shutdown-smoke.mjs` exercises real proxy subprocesses and disposable
  fake servers: explicit shutdown, stdin EOF, EOF while initialization is
  delayed and a server that ignores shutdown. All four cases passed in
  `ahead-lsp-shutdown-rAgijd`; each verifies the server has exited and the
  unsaved buffer was not written. The previous dev-bundle binary fails the
  same test because it exits before the shutdown/exit handshake. An earlier
  fixture attempt timed out before initialization and was not counted as
  shutdown evidence.

  The installed Rust Analyzer/Vtsls/BasedPyright smoke passed completion,
  definitions, diagnostics, close/reopen and restart again in
  `ahead-lsp-smoke-mWhEQm`. It now requests graceful proxy shutdown and verifies
  the test's process group is empty instead of relying on teardown to kill it.
  Forced cleanup remains only for a failed test. Targeted formatting, script
  syntax and whitespace checks passed.
  The full serial suites then passed all 164 proxy tests and all 127 editor
  tests. The native child-agent case passed in this run; its earlier
  intermittent startup timeout remains unresolved.

  The macOS `just dev` bundle now uses `/bin/cp -c` for the debug executable.
  The installed macOS manual documents this as a clonefile copy with ordinary
  copy fallback on unsupported filesystems. The recipe dry-run passed, the
  restored dev command finished its no-op build in 7.70 seconds, and `cmp`
  confirmed identical source/bundle executables. Available disk space rose
  from 336 MiB to 958 MiB across replacement of the old full copy. No cache,
  source or project data was deleted.

  Native Cmd+Q/window-close behavior remains unverified while the desktop is
  locked. Blocking extension acquisition, escaped descendants, Linux/Windows,
  and terminal/debugger/active-agent shutdown remain separate gates in
  `TODO.md`; LSP evidence does not establish those behaviors.

- 2026-09-30: reviewed `crates/lsp/src/lsp.rs` and the explicit build-script
  inputs in `crates/zed/build.rs` and `crates/ztracing/build.rs` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. AHEAD now sends LSP `shutdown`,
  waits for its reply and sends `exit` before closing the writer. One
  five-second deadline covers that exchange and process exit, with direct-child
  termination as the fallback. Workspace restart stops servers concurrently.
  Normal sent requests, including initialization, use Zed's 120-second fallback
  timeout. Expiry cancels the matching request, fails its callback outside the
  pending-request lock and ignores a late reply; other requests can continue.
  The existing process monitor owns the timeout check. No new dependencies or
  Zed source were imported.

  The focused plugin suite passed all 21 tests, including cooperative and
  unresponsive real child processes, ordered shutdown frames and timeout/retry
  behavior. The app build passed. The installed Vtsls/BasedPyright smoke passed
  in `ahead-lsp-smoke-AK2R5g`, including unsaved replay after restart and healthy
  replacement-server status. The combined serial suite passed all 127 editor
  tests and 163 of 164 proxy tests; the tracked native child-agent test again
  stalled at 0/4 model requests with no file or activity. Three package-directory
  replays then passed all 164 proxy tests each. A four-second stack-sampling
  trigger never fired, so there is still no stalled stack or identified cause.
  An earlier diagnostic replay used the wrong working directory and failed
  only the relative-path CLI test; it is not counted as a suite pass.

  Extended `tests/lsp-smoke.mjs` to installed Rust Analyzer. Its first run
  received the correct unsaved type error but the test expected the wrong
  diagnostic code. After matching the observed `E0308`, the full four-language
  probe passed in `ahead-lsp-smoke-kmQVFF`. Rust completion, cross-file
  definition, unsaved diagnostics/repair, close/reopen and restart now run
  alongside the TS/JS/Python checks. Restart preserved unsaved Rust errors,
  created a new server instance, recovered completion/definition and left the
  source file unchanged on disk. No servers or packages were downloaded.

  Cargo fingerprint logs identified whole-package scans in the two linker-only
  build scripts. Adding `rerun-if-changed=build.rs` to both stopped unrelated
  documentation edits from relinking AHEAD. The initial root-doc rebuild took
  13.96 seconds; after the fix, no-op and root/app documentation probes took
  4.39–4.76 seconds without recompiling or relinking. The temporary app Markdown
  probe was removed. Source and embedded-asset tracking are unchanged.

  All 15 RPC tests, targeted Rust formatting, JavaScript syntax and whitespace
  checks passed after the final edits.
  Restored the same approved `just dev` command in
  `/private/tmp/ahead-recovery-dev-1uccxv`. Cargo finished without recompilation
  in 8.91 seconds; process inspection confirmed one watcher, the stable
  `Ahead.app` and its proxy. The shared dev loop remains running.

  Native restart/timeout feedback, descendant cleanup, restart during edits or
  closure, and app/proxy quit cleanup remain unverified. The desktop stays
  locked; no computer-use retry was made. These results are process/protocol
  evidence, not a native UI or provider-backed acceptance pass.

- 2026-09-30: reviewed Zed's
  `crates/project/src/lsp_store.rs::restart_all_language_servers`,
  `restart_language_servers_for_buffers` and
  `crates/lsp/src/lsp.rs::shutdown` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. AHEAD reuses its current
  dispatcher/catalog/client boundary: restart stops the old handlers, clears
  their diagnostics and reopens the proxy's unsaved snapshots. Shared TS/JS
  languages still use one Vtsls instance. No source or dependency was added
  from Zed.

  The Language Servers panel now exposes Restart and shows live failure
  details. Status changes wake the view; losing the proxy marks its server
  entries disconnected. A process monitor detects child exit independently
  of stdout EOF. Shutdown fails pending and later requests, including queued
  requests during initialization. Callbacks run outside the pending-request
  lock, and completion failure delivers an empty list. Retired process IDs
  cannot publish diagnostics or status over their replacements; old
  completion items cannot resolve against a replacement server. Writer errors
  also stop the handler, and shutdown explicitly releases the writer loop.

  Validation: focused transport/catalog tests passed, including a real
  subprocess whose descendant keeps stdout open after the server exits.
  The combined serial offline suite initially passed all 127 editor tests
  and 160 of 161 proxy tests; the previously tracked native child-agent test
  timed out before its first model request. After rebuilding the app, the
  repeat passed all 127 editor, 161 proxy and 15 RPC tests. That retry does
  not resolve the intermittent startup failure; it remains in `TODO.md`.
  The rebuilt real Vtsls/BasedPyright probe passed in disposable
  `ahead-lsp-smoke-ajULXA`, including restart with unsaved errors, subsequent
  completion/definition/repair, shared TS/JS identity, retired-diagnostic
  clearing and rejection of an old auto-import completion. Source files
  were not saved. The final writer-cleanup build passed, and the full real
  language-server smoke passed again in `ahead-lsp-smoke-9C83jg`. Targeted
  rustfmt, JavaScript syntax and whitespace checks passed. One final link
  failed for lack of disk space; removing this run's obsolete 464 MB focused
  proxy-test executable allowed the retry to finish in 15.52 seconds. No
  source, database or project data was removed.
  The sandboxed `just dev` launch built successfully but aborted with a
  `com.apple.hiservices-xpcservice` connection error (signal 6). The same
  command with approved native-app access and `RUST_BACKTRACE=full` then
  rebuilt in 24.32 seconds and launched successfully. Process inspection
  confirmed one watcher, the same stable `Ahead.app` and one proxy in
  `/private/tmp/ahead-recovery-dev-1uccxv`. This is process evidence only;
  no locked-desktop computer-use retry was made.

  Native restart/error rendering remains unverified while the desktop is
  locked. The current stop path force-terminates the direct child after
  failing requests; Zed-style bounded graceful shutdown, alive-but-hung
  servers, inherited-pipe/descendant cleanup, restart during edits/closure
  and a real Rust restart pass remain tracked in `TODO.md`.

- 2026-09-30: reviewed `crates/db/src/db.rs::open_db` and `open_main_db`
  at `418f89714891f9d8105a3e92e60b9a7a5084d232`, plus the locally pinned
  Turso/libSQL 0.6 client and its bundled Unix VFS. AHEAD keeps its existing
  no-migration/no-reset/no-fallback storage contract. No Zed source was copied.

  `SessionStore::open` now creates Unix database files with mode 0600 before
  initialization. It checks the owner and parent write permissions and rejects
  linked/special database or sidecar paths. Supported stores and existing
  journal/WAL/SHM files are tightened after validation; contents are retained.
  Permission changes use the parent directory descriptor, not raw database
  handles whose close could release libSQL's POSIX locks. The existing
  `rustix` dependency and `ahead-core::secure_fs` directory helper are reused.

  Existing stores get an immutable, read-only header check through Turso
  before a writable open. The hard fork never changes schema markers in
  place, so unsupported files can be rejected before journal recovery changes
  them. Supported stores still run normal crash recovery. The subprocess test
  leaves a real hot rollback journal: the current schema recovers its original
  row, while the old schema's database and journal stay byte-for-byte unchanged.
  Filename spaces and `#` exercise URI escaping. Other tests cover private
  sidecar creation/reopen, symlinks, hard links, directories, FIFOs, shared-write
  directories, orphaned sidecars and unchanged rejected-file modes.

  Validation: all 126 editor, 158 proxy and 15 RPC tests passed in the serial
  offline suite with local mock listeners enabled. The rebuilt real proxy
  passed `tests/editor-recovery-smoke.mjs` in `ahead-recovery-smoke-XfRyIY`,
  including 0600 creation/reopen, SIGKILL recovery, live-owner isolation and
  persistent discard. The same `just dev /private/tmp/ahead-recovery-dev-1uccxv`
  loop rebuilt in 12.71 seconds. Its existing database was 0644 before launch
  and 0600 afterward; one watcher, the stable bundle and one proxy were
  confirmed running. This is process/filesystem evidence, not a rendered UI
  journey. Windows ACL enforcement, inherited ACLs, Linux execution and
  simultaneous first opens in separate processes remain in `TODO.md`.

- 2026-09-30: reviewed Zed's `crates/editor/src/persistence.rs`,
  `crates/editor/src/items.rs` serialization/deserialization and
  `restore_serialized_buffer_contents` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. Zed restores dirty text over a
  loaded file buffer and retains its saved baseline for conflict detection.
  AHEAD follows that behavior using the existing Turso session database,
  with a saved-content digest rather than Zed's mtime. No Zed source or
  SQLite migration layer was copied. Schema 2 adds `editor_recoveries`;
  schema 1 is rejected unchanged, with no compatibility path.

  The editor coalesces dirty-buffer snapshots through the existing proxy
  RPC. Buffer UUIDs and increasing revisions prevent late writes from
  resurrecting cleared text. Proxy-owned OS leases prevent another live
  editor from claiming those snapshots; startup can claim them after the
  owner exits. Restore leaves source files untouched, preserves current
  dirty tabs and prompts before saving over a changed baseline. File >
  Recover Unsaved Changes exposes retained recoveries that could not open.
  Confirmed close/quit waits for backup-clear acknowledgements and checks
  buffer generations both before clearing and afterward. Failure or new
  typing keeps the buffer open and queues a newer snapshot. Opening a file
  after the last tab closes now gets its proxy from the session, not the
  released editor view.

  Validation: the serial offline suite passed all 126 editor, 154 proxy and
  15 RPC tests after allowing loopback sockets for the existing mock-provider
  tests (the sandboxed run rejected their listener binds). All fifteen
  full-shell lifecycle tests also passed a 20-seed sweep, including six
  recovery tests and the strengthened cross-window quit test. The rebuilt root app passed
  `node tests/editor-recovery-smoke.mjs target/debug/ahead` in disposable
  `ahead-recovery-smoke-Dyv8oL`: acknowledged Unicode text survived SIGKILL,
  a concurrent live proxy could not claim/read/overwrite it, disk text stayed
  unchanged and acknowledged discard survived another restart. This is real
  proxy/storage evidence, not a rendered native editor observation.
  The real Vtsls/BasedPyright smoke also passed completion, definition,
  diagnostics/repair, TypeScript auto-import resolve and TS/Python close/reopen
  in `ahead-lsp-smoke-1RdMtp`.

  Native recovery/menu/overwrite verification remains pending desktop unlock.
  Only acknowledged snapshots are durable: periodic capture runs every
  250 ms, and GPUI limits its best-effort shutdown observer to 200 ms.
  Untitled buffers, full workspace layout/caret restore, general disk-change
  handling, running-job exit policy, large-buffer profiling and safe cleanup
  of old lease files remain in `TODO.md`. Individual backups are capped at
  32 MiB. Existing databases and previous disposable fixtures were preserved.
  The shared `just dev /private/tmp/ahead-recovery-dev-1uccxv` loop rebuilt
  in 19.89 seconds and is running the same stable `Ahead.app` bundle with one
  proxy. The fresh fixture's Rust, TS/JS and Python checks passed through
  `just verify` in the terminal, not the editor UI. Startup created ignored
  session/settings/lease files; Git exposed only `.ahead/.gitignore` from that
  directory. A permission check found `session.db` mode 0644 (settings 0600,
  lease directory 0700). Database permission hardening is now a priority
  release gate in `TODO.md`; ignored does not mean owner-only.

- 2026-09-30: reviewed Zed's `crates/workspace/src/workspace.rs`
  `prepare_to_close`/`prepare_windows_to_quit`, `crates/zed/src/zed.rs::quit`
  and `crates/project_panel/src/project_panel.rs` deletion prompts at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. AHEAD now prepares every buffer
  before closing a window or executing its explicit Quit action, then checks
  every approved snapshot again before releasing buffers. Cancel, failed or
  timed-out saves, new tabs and edits during another window's prompt prevent
  closure. Quit requests are deduplicated. The full-shell test exposed a
  GPUI action-dispatch trap: reading windows synchronously from the action
  callback missed the currently borrowed window. Enumeration now runs inside
  the asynchronous task, as in Zed's Quit flow. Close Window is deferred for
  the same reason. The native window callback and Linux title-bar button use
  the same preparation path; native menu/key bindings expose Quit and Close
  Window. Save acknowledgements use an async receiver and GPUI timer rather
  than occupying a worker while waiting. Recent-file persistence queues its
  RPC without blocking a worker on the response.

  Explorer and tab menus now share Move to Trash, replacing direct permanent
  deletion. The prompt identifies the target and warns about affected unsaved
  buffers; changed decisions are rejected. The existing proxy Trash route
  runs on a worker, rejects the workspace root/outside paths and resolves
  symlink parents without following the selected symlink itself. Failure,
  timeout or edits made while the RPC is pending leave buffers open. Late
  replies cannot close retained buffers. There is no permanent-delete
  fallback. No dependency, backward-compatibility path or copied Zed source
  was added.

  Native prompt/menu/Trash verification remains pending while the desktop is
  locked. GPUI's test platform makes `quit()` a no-op: tests verify prepared
  state and buffer release, not OS termination. Its `on_app_quit` callback
  cannot veto shutdown. Dock/system termination, crash recovery, dev-watcher
  restarts and running-job exit policy remain release gates in `TODO.md`;
  AHEAD does not yet have Zed's dirty-buffer hot-exit serialization. Trash
  tests mock successful effects; the proxy test validates path boundaries
  without deleting files.

  Validation: all 119 editor tests passed in the serial offline combined
  editor/proxy/RPC run. Eight full-shell lifecycle tests also passed a separate
  `ITERATIONS=20` sweep, including action dispatch, cross-window edits, delayed
  acknowledgements and timeouts. The proxy suite passed 149 of 150 tests;
  the existing native child-edit/attribution test timed out before its second
  mock model request, then passed alone in 0.88 seconds. This intermittent
  failure is recorded in `TODO.md`, not treated as fixed by the isolated pass.
  The combined run stopped before RPC tests; whitespace and targeted rustfmt
  checks passed.
  All 28 store tests subsequently passed together. After adding a ninth
  lifecycle regression for opening a new tab during close preparation, the
  final combined suite passed all 120 editor, 150 proxy and 15 RPC tests.
  The earlier child-startup timeout remains unresolved; no timeout increase
  or automatic retry was added.
  The same `just dev /private/tmp/ahead-schema-smoke.oKBqbO/fresh` loop rebuilt
  in 30.62 seconds. Process inspection confirmed one AHEAD watcher, the stable
  app bundle and its proxy. The rebuilt real-server smoke passed TypeScript,
  JavaScript and Python completion/definition/diagnostic error and repair,
  TypeScript auto-import resolution and TS/Python close/reopen in disposable
  `ahead-lsp-smoke-XFe6Nk`. These are proxy/server observations, not rendered
  native close or Trash verification.

- 2026-09-30: reviewed Zed `crates/project/src/git_store.rs::recalculate_diffs`
  and `crates/git/src/blame.rs::Blame::for_path` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`: diff/blame are asynchronous and
  use the live buffer. AHEAD now sends a debounced `GitFileState` request
  through the existing proxy RPC and computes it with the already-linked
  Git library on a worker. The gutter compares the live snapshot with HEAD,
  preserving staged changes, and blame uses that same pinned commit plus the
  snapshot. Repaint and caret movement no longer execute Git subprocesses.
  Per-buffer requests are coalesced, only the current file/revision/repository
  generation can apply a reply, and closing a buffer drops its receiver.
  Repository notifications invalidate metadata even if the file-status list
  is unchanged; ignored build output does not trigger these refreshes. Watches
  include Git metadata outside the workspace for linked worktrees and opened
  subdirectories. The branch tooltip reads the live cache rather than a
  startup-only value or a synchronous subprocess. No new dependency, legacy
  compatibility path or copied Zed source was introduced. Native gutter and
  latency verification remain pending while the desktop is locked. Large-file
  and many-tab profiling also remain open: whole-file blame is bounded to one
  in-flight request per buffer but is not yet backed by a shared HEAD cache.
  Validation: the serial offline combined suite passed 111 editor, 149 proxy
  and 15 RPC tests. Git regressions cover unsaved/staged/clean/deleted/new/
  ignored/non-repository files, background RPC delivery, same-status external
  amend, linked-worktree/subdirectory metadata roots, view wakeups, transport
  errors, request coalescing, caret cache reuse and rejection of stale or
  closed-buffer replies. The GPUI scheduling regression ran ten seeds in the
  full suite. Fixtures use temporary local repositories and test identities;
  no real project commits or provider calls were used.
  A separate `ITERATIONS=20` sweep of the GPUI scheduling regression also
  passed. Targeted rustfmt and whitespace checks passed.
  The same `just dev /private/tmp/ahead-schema-smoke.oKBqbO/fresh` flow
  rebuilt in 53.61 seconds; process inspection confirmed one AHEAD watcher,
  the stable app bundle and its proxy. The rebuilt real Vtsls/BasedPyright
  smoke passed again in disposable `ahead-lsp-smoke-yiOM3h`: TypeScript,
  JavaScript and Python completion, definition, error/repair, auto-import
  resolve, shared TS/JS server identity, and TS/Python close/reopen. These
  are proxy/server observations, not a rendered native gutter check.

- 2026-09-30: reviewed `crates/workspace/src/pane.rs` save-on-close and
  `crates/project/src/lsp_store.rs::unregister_buffer_from_language_servers`.
  AHEAD's tab-close actions now prompt for Save, Discard Changes or Cancel,
  await successful saves, preserve buffers after failure, and reject decisions
  made before newer edits. Multi-tab actions capture their targets instead of
  recomputing them after each asynchronous reply. Confirmed close and preview
  replacement release the proxy snapshot, file watch, cached diagnostics and
  pending diagnostic generation, then send LSP `didClose`. Closing a tab does
  not write its discarded content to disk. Window quit and Delete File were
  still open here; see the subsequent close/Trash entry above.
  The lifecycle test also exposed a blocking attribution RPC on the UI thread
  during file open. Anchor refresh is now asynchronous, wakes the gutter on
  completion, and ignores older replies or replies for a closed buffer.
  The full serial offline combined suite passed 109 editor, 145 proxy and
  15 RPC tests; the subsequently added nonblocking/stale-anchor regression
  passed separately. Native prompts and close/reopen are not yet verified
  while the desktop is locked. These are AHEAD implementations using existing
  GPUI prompts, editor APIs and RPC routes; no dependency or Zed source copy
  was added.
  `just dev /private/tmp/ahead-schema-smoke.oKBqbO/fresh` rebuilt in 45.16
  seconds and relaunched the stable `target/debug/macos/Ahead.app` bundle;
  process inspection confirmed one AHEAD watcher, its app and its proxy.
  The rebuilt real-server smoke check passed in the disposable
  `ahead-lsp-smoke-eIDvKH` copy: TypeScript and Python close cleared unsaved
  diagnostics, reopen accepted fresh content and repair cleared new errors.
  TypeScript/JavaScript completion replies carried the same server identity.
  Auto-import resolve and all three languages' completion/definition/diagnostic
  checks also passed. This verifies the proxy/server path, not rendered prompts.

- 2026-09-30: reviewed `crates/project/src/lsp_store.rs` completion resolve
  and additional-edit handling. AHEAD's menu now keeps each original LSP item
  and server identity, resolves on acceptance, validates UTF-16 edit ranges,
  rejects overlapping edits and combines the symbol/import changes into one
  undoable replacement. Buffer, caret, file and request changes invalidate
  pending results. Servers without the optional resolve capability return
  their original item; transport errors remain visible failures. Snippets are
  still not advertised. Five editor/client completion tests and four matching
  proxy tests passed, followed by the full serial combined suite: 106 app,
  143 proxy and 15 RPC tests, all passing, including the previously timed-out
  native-child edit case. The real installed Vtsls probe resolves an auto-import
  and clears the undefined-symbol error after applying it in a disposable
  fixture. That protocol check does not prove the native menu/Undo journey.

- 2026-09-30: reviewed the pinned `crates/editor/src/{navigation,completions}.rs`
  and `crates/project/src/lsp_store.rs` for multi-language editing. AHEAD now
  updates the editor language on preview reuse, reports the actual language
  and caret in the footer, filters completions before the visible limit, and
  routes F12 results through the existing file/range opener. A LocationLink
  selects its UTF-16 target selection rather than the whole declaration.
  Standard LSP initialization now reports ready/error states without requiring
  Rust Analyzer's experimental notification. All 103 editor tests and 15 RPC
  tests passed. The combined proxy run passed 141/142; its native-child edit
  test timed out after 20 seconds, then passed in isolation in 1.62 seconds.
  The suite timeout is not treated as a clean full-suite pass.
  In the disposable native project, Vtsls completion inserted `doubled`, but
  diagnostics referred to nonexistent `douconst`/`doubledconsole` text.
  The proxy was sending range-less replacement snapshots to an incremental
  server; cached Vtsls used the new line count when replacing its old document.
  AHEAD now gives complex incremental changes the explicit old-buffer range.
  The opt-in `tests/lsp-smoke.mjs` reproduces the failure with the old binary.
  The rebuilt binary passes real Vtsls/BasedPyright completion, cross-file
  definition, error/repair and initialization-status checks for TypeScript,
  JavaScript and Python; TypeScript resolve also supplies a valid auto-import.
  These runs use installed servers and new fixture copies, with no package
  downloads or model requests. The desktop locked before the rebuilt native
  pass. TypeScript/JavaScript/Python syntax parsers are not cached and their
  download needs approval; selecting a language name does not prove grammar
  support. Extension execution, native navigation and remaining lifecycle
  gates stay in `TODO.md`. No Zed implementation was copied in this slice.

  2026-10-02 follow-up: compared Zed's
  `crates/grammars/src/{javascript,typescript,python}/config.toml` with
  gpui-kit 0.6.6's linked Tree-sitter features. AHEAD now enables its
  JavaScript, TypeScript, TSX and Python parsers in the normal build. The
  existing file-switch regression verifies actual error-free parse trees and
  multiple highlight spans for JavaScript/JSX, TypeScript/TSX, Python, Rust
  and JSON; all 172 app library tests and the `ahead` binary check pass.
  This uses gpui-kit's parsers, not copied Zed grammar code. Rendered color
  verification and arbitrary installed-extension grammar loading remain open.

- 2026-09-30: reviewed `crates/agent_servers/src/acp.rs::new_session` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. Zed awaits the session-creation
  request's result. AHEAD's local `StartWork` and adapter-install-selection
  requests now wait for their actual reply or a disconnected transport;
  the ordinary 30-second read deadline no longer makes these writes appear
  retryable while they are still running. EOF, write failure or a malformed
  reply closes the shared handler, fails its pending requests once and rejects
  further requests. The UI tells the user to restart AHEAD and restore saved
  sessions, since an in-flight write may already have completed. This adds no
  automatic retry, request journal, migration, schema change or dependency.
  Four RPC and two client regressions cover late replies, read deadlines,
  pending-request release, reentrant callbacks, rejected retries, EOF and
  malformed replies. The serial offline combined library run passed all
  253 tests (100 app, 138 proxy, 15 RPC); localhost mock-provider fixtures
  required permission to bind their test listeners. `just dev` rebuilt the
  app in 30.06 seconds. In the temporary bundle on
  `/private/tmp/ahead-schema-smoke.oKBqbO/fresh`, a suspended proxy held the
  creation response past 33 seconds. Repeated Start clicks and closing/reopening
  the form kept creation pending. Resuming the proxy added one inactive thread
  without replacing the new form or active chat. Terminating that test proxy
  showed the restart status; creation and retry immediately rendered the
  connection error. Restart restored the three saved threads with no thread
  from the rejected request. No provider turn or real adapter install was run.
  Atomic session-and-harness creation remains in `TODO.md`.

- 2026-09-30: reviewed `crates/agent_ui/src/conversation_view.rs`
  (`LoadingView`, `ServerState::Loading` and session-load completion) at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. Zed ties loading to explicit
  state and an owned task. AHEAD now guards creation and adapter changes with
  pending state and the creation form's generation. Closing a form does not
  undo a durable creation request: a late success adds an inactive thread,
  retaining the original agent name without replacing the newer form or chat.
  A reopened external picker waits for an older install to finish before
  requesting its catalogue.
  Six new GPUI tests exercise duplicate requests, retry after failure, late
  success/failure, preserved selection/errors and deferred catalogue loading
  at the request-completion boundary. The serial offline `ahead-app --lib`
  suite passed all 98 tests. `just dev` rebuilt the app in 39.39 seconds.
  In the temporary bundle on `/private/tmp/ahead-schema-smoke.oKBqbO/fresh`,
  suspending only the test proxy held a creation reply while Start was clicked
  twice. The controls showed Starting and were disabled. After closing and
  reopening the form, resuming the proxy added exactly one inactive thread;
  the new form and existing active chat stayed in place.
  No real adapter install or provider turn was run. No compatibility path,
  dependency or copied Zed code was added. The follow-up entry above removes
  the misleading transport timeout for these writes and tests disconnect
  recovery.

- 2026-09-30: reviewed
  `crates/agent_ui/src/conversation_view/thread_view.rs::{handle_thread_error,render_thread_error,clear_thread_error}`
  and `render_any_thread_error` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. Zed stores errors separately
  from normal status text and renders a local error callout. AHEAD now uses
  one optional error with the existing gpui-kit `Alert` component in the
  thread list and both creation forms. Removed the unrendered success-status
  messages and duplicate catalogue-error field; no compatibility alias,
  dependency or copied Zed code was added.
  Three new GPUI tests cover visible failures for unavailable-host creation
  and adapter installation, and a rejected history load. They also check
  preserved input and clearing on navigation or successful reload. The full
  serial offline `ahead-app --lib` suite passed all 92 tests.
  `just dev` rebuilt the app; the same debug binary in the temporary bundle
  opened `/private/tmp/ahead-schema-smoke.oKBqbO/unsupported`. The full storage
  failure rendered in the thread list, the creation wizard and the external
  agent catalogue. Back cleared the wizard error without losing its
  description. The invalid database hash stayed unchanged. No provider turn
  or real adapter installation was attempted. The subsequent review above
  covers pending-request lifecycle and duplicate-submit protection.

- 2026-09-30: compared `crates/agent/src/db.rs::ThreadsDatabase::new` and
  `connect` at `418f89714891f9d8105a3e92e60b9a7a5084d232`. Zed returns
  database errors and reserves in-memory storage for explicit stateless/test
  mode; its historical column upgrades are not part of AHEAD's hard fork.
  Removed AHEAD's task-objective, anchor, archive-time and session-mode
  migrations, the obsolete mode column/write, and old-index cleanup.
  New Turso databases now receive the full schema, application ID
  `0x41484544` and schema version `1` in one transaction. Current stores
  reopen without schema writes; unsupported stores fail unchanged.
  Workspace storage errors disable agent-session requests and retain the
  cause in the RPC response. The proxy stays connected for editor work,
  with no in-memory fallback or panic when opening the workspace store fails.
  The two new failure regressions first failed against the old code.
  After the fix, the combined serial library suite passed 100 agent and
  138 proxy tests (two opt-in agent tests ignored). Tests cover byte-identical
  rejected files, unchanged current-schema reopen, removal of obsolete
  columns, replacement of a previous host on failure, and continued editor
  saves. Source and tests are 168 lines smaller; no dependency or Zed code
  was added. `just dev` built and launched the disposable `fresh` fixture
  under `/private/tmp/ahead-schema-smoke.oKBqbO`; a temporary macOS bundle
  exposed the same debug binary to native UI automation. First startup
  created `.ahead` and the correctly marked database. A local session created
  through the new-thread wizard survived a process restart and rendered
  "Durable session restored". The separate invalid-database fixture rendered
  the storage-disabled status while Explorer and `main.rs` still worked.
  Starting a session did not attach one, and the invalid file's SHA-256 stayed
  unchanged. That pass exposed an unrendered wizard error, fixed and checked
  in the follow-up entry above. The ineffective automated Quit shortcut
  remains in `TODO.md`. No provider turn was sent and no real workspace database was
  opened or changed. Workspace formatting and whitespace checks passed.

- 2026-10-01: clarified managed restore after the explicit one-time legacy
  import exception. Turso remains the only live thread store: only when a
  requested thread is absent may AHEAD import one uniquely located, regular
  legacy rollout from its configured runtime home. The importer checks the
  thread metadata and canonical workspace, bounds plain and compressed input,
  commits the header and ordered replay rows atomically, and leaves source
  files untouched. It rejects malformed, symlinked, out-of-root,
  cross-workspace and active/archive-ambiguous sources; it adds no schema
  migration or live JSONL fallback. Focused plain/compressed, archive/history,
  rejection and rollback tests are present but remain unrun until the shared
  Cargo loop is available. See `TODO.md` and
  `ahead-agent-standards.md#filesystem-placement` for the current contract.

- 2026-09-30: reviewed `crates/agent/src/thread_store.rs::load_thread`
  and `crates/agent/src/agent.rs::create_subagent_thread` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. Zed loads the selected thread
  from its editor database and creates children through its native agent.
  AHEAD keeps that ownership boundary with Turso and the retained runtime.
  Per the hard-fork decision, removed AHEAD's old JSONL search/import fallback,
  its transactional import API, runtime-home/provider arguments used only
  by that importer, and obsolete migration tests. Missing database threads
  now fail without reading or changing old rollout files. No migration or
  user-data deletion was added.
  The new file-backed regression first failed against the old implementation
  because a missing thread was silently imported. Current-child restore
  coverage now creates a child through the native manager, reopens it, and
  checks its parent link and root-only tool exclusion.
  The source/test diff is 1,040 lines smaller. No Zed code or dependency was
  added. Old-schema upgrades and remaining Guardian compatibility markers
  still need pruning, as recorded in `TODO.md`.
  The serial locked/offline agent/proxy library suite passed: 100 agent
  tests and 138 proxy tests. Authenticated Pi and the opt-in Node/npm smoke
  remained ignored; the latter was run successfully in the earlier adapter
  pass. No new live GPUI/provider result is claimed.
  Separately, the native-child stall did not recur in ten isolated probes,
  one full proxy suite, or ten suite-order probes with stack capture armed.
  Those suite-order probes excluded the separate long spawn/resume test.
  No stack was captured and the timeout cause remains open; neither the
  test deadline nor sandbox was weakened.

- 2026-09-30: reviewed `LocalRegistryNpxAgent` and versioned archive caches in
  `crates/project/src/agent_server_store.rs` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. AHEAD's npm installer now stages
  a fresh package, validates its executable and provenance, and atomically
  publishes a manifest pointing to an immutable generation. Cached launches
  can still resolve the previous package while an updater holds the writer
  lock. This adapts Zed's distribution/cache separation; its npm implementation
  does not itself provide this generation-publication flow. No Zed code or
  dependency was added. The three new offline regressions cover failed updates
  with concurrent launches, simultaneous installers and invalid executables.
  The real offline npm/Node smoke test also passed: it packs/installs two local
  tarballs and launches both generations after the update, with relative
  module loading. Its npm config/cache are temporary, lifecycle scripts are
  disabled, and no registry or provider is contacted. The combined suite
  initially passed 104 agent and 141 proxy tests. The final repeat passed all
  104 agent tests but only 140 proxy tests: the already-tracked native-child
  timeout recurred before its first child request (1/4 model requests, no
  created file). The isolated repeat passed in 1.11s; that is not a fix.
  Authenticated Pi remains ignored; the offline npm smoke is also ignored by
  default but was run explicitly and passed. Formatting and whitespace checks
  passed.
  Registry downloads, rendered installation and safe old-generation cleanup
  remain in `TODO.md`; no user caches or data were deleted. The Node/npm smoke
  is opt-in so the default Rust suite does not require those tools:

  ```sh
  rtk proxy cargo test --locked --offline -p ahead-agent -p ahead-proxy --lib npx_generations_launch_locally_packed_packages_offline -j1 -- --ignored --test-threads=1
  ```
- 2026-09-30: checked generic MCP authorization and cancellation in
  `crates/agent/src/tools/context_server_registry.rs` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. Deleted the `ahead-mcp-state`
  Apps policy crate, account-based Apps tool filtering, hosted widget resource
  reader/provenance tracking, and its compaction-checkpoint fields. Removed
  the Apps feature flag, `connectors` alias and Apps path-override compatibility
  parser/schema. Ordinary MCP tool approval, resource reads, exact-client
  bindings and Turso replay remain. The combined AHEAD-owned run passed 101
  agent and 140 proxy tests; authenticated Pi remains ignored. Workspace and
  scoped runtime formatting plus whitespace checks passed. Copied core/MCP
  library tests were not run. No Zed code was copied, no user data was deleted,
  and no live GPUI or authenticated provider/OAuth journey was tested.
  Hosted tool formatting, upload/event handling and unused typed Apps
  configuration still need pruning; they are listed in `TODO.md`.
- 2026-09-30: reviewed per-server MCP tool ownership and refresh in
  `crates/agent/src/tools/context_server_registry.rs` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. Removed the Apps-only startup
  cache, disk persistence, cached server metadata and reconnect/backoff state.
  MCP runtime publication and prewarming no longer depend on model-account
  changes. Removed the unused MCP Apps/base-URL/home fields and direct
  model-auth dependency, plus orphaned hosted trusted-access fixtures.
  The ordinary per-server cache, exact-client tool binding, cancellation and
  server OAuth recovery remain. Copied cache lifecycle fixtures now use
  ordinary servers; their disabled library test target was not run.
  The combined AHEAD-owned run passed 101 agent and 140 proxy tests;
  authenticated Pi remains ignored. Workspace formatting, scoped runtime
  formatting and whitespace checks passed. No Zed code was copied, no user
  data was deleted, and no live GPUI or OAuth login was tested. The remaining
  Apps formatting, approval and hosted-event code is tracked in `TODO.md`;
  backward compatibility is not a requirement for that pruning.
- 2026-09-30: reviewed server-owned MCP authentication in
  `crates/project/src/context_server_store.rs` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. Removed the ChatGPT MCP auth
  selector, model-account routing, transport identity credentials and the
  lower-level HTTP auth-provider hook. Removed their copied fixtures and
  unused direct API/model-provider dependencies. Explicit server bearer/header
  credentials and stored OAuth remain; AHEAD's workspace HTTP declarations
  are still rejected until the auth UI exists. The new AHEAD-owned projection
  test covers those explicit credential fields without making a connection.
  The combined run passed 101 agent tests and 140 proxy tests; authenticated Pi
  remains ignored. Formatting and whitespace checks passed. No Zed code was
  copied, no user data was deleted, and no live GPUI or OAuth login was tested.
  Apps startup caching and account-based runtime refresh checks remain in
  `TODO.md`.
- 2026-09-30: reviewed MCP catalog and elicitation ownership against
  `crates/agent/src/tools/context_server_registry.rs` and
  `crates/agent/src/agent.rs` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. Deleted the hosted Apps auth
  parser, ChatGPT sign-in request/result rewriting and forced-refresh chain.
  Removed its catalog override, refresh lock, fetch-source selector, feature
  flag, unused aliases and copied fixtures. Server instructions still feed
  normal tool descriptions during initialization; only their unused retained
  copy was deleted. The AHEAD-owned config test uses the public MCP manager to
  verify the existing empty elicitation capability, with no URL advertisement.
  The combined run passed 100 agent tests and 140 proxy tests; authenticated Pi
  remains ignored. Formatting and whitespace checks passed. Corrected the
  standards and backlog: AHEAD currently declines server elicitation requests;
  Zed's form handoff is the reference for implementing the missing UI. Ordinary
  MCP tool-call approval through chat remains separate. No Zed code was copied,
  and no live GPUI or authenticated provider checks were run for this change.
- 2026-09-30: compared tool selection and fresh context construction with
  `crates/agent/src/thread.rs` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. Removed hosted Apps instruction
  generation, its world-state section, config/profile/default keys and model
  flag, plus unused app metadata helpers and copied tests. The user confirmed
  that backward compatibility is not required: old Apps tag handling and the
  product-SKU compatibility test/denylist entry are removed too. The native
  turn regression checks that requests and persisted context contain no Apps
  instructions, while project skill loading, streaming and restore still work.
  The combined run passed 100 agent tests and 140 proxy tests; authenticated Pi
  remains ignored. Formatting and whitespace checks passed. These are local
  mock-provider tests, not authenticated GPUI verification. No Zed code was
  copied. Apps cache/authentication/approval pruning remains in `TODO.md`.
- 2026-09-30: followed up on the MCP registry review at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`
  (`crates/agent/src/tools/context_server_registry.rs`). Removed unused
  standalone connector discovery and its process-global app-list cache.
  The legacy instruction metadata helper remains private to the core; normal
  MCP tool refresh and approval behavior are unchanged. Removed the retired
  Apps product-SKU config fields and added an AHEAD regression for reading old
  config without retaining that key. Removed obsolete discovery/cache tests;
  the metadata-conversion test remains. The combined AHEAD agent/proxy run
  passed 101 agent tests and 140 proxy tests; authenticated Pi remains ignored.
  Formatting and whitespace checks passed. The copied core/MCP test binaries
  were not run. Documented the combined command to avoid rebuilding the runtime
  for separate test feature sets. No Zed code was copied. Remaining Apps
  instruction, tool-cache, authentication and approval code stays in `TODO.md`.
- 2026-09-30: rechecked MCP registration against
  `crates/agent/src/tools/context_server_registry.rs` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. Removed automatic `codex_apps`
  registration, the hosted-endpoint factory and MCP-only originator plumbing.
  The generic MCP catalog still applies environment authority and approval
  policy. Removed five factory-specific copied tests; generic permission and
  credential-isolation tests now use ordinary HTTP-server fixtures. The two
  AHEAD config regressions failed before the change because the runtime added
  a hosted server despite having no corresponding declaration. Both now pass;
  the full AHEAD agent suite passed 100 tests, with authenticated Pi ignored.
  All 140 proxy tests passed, including native child edits and Turso resume.
  Formatting passed. The copied core/MCP test binaries were not run;
  AHEAD-owned tests cover the config-to-runtime boundary. No Zed code was copied.
  Remaining Apps cache/auth/approval pruning and rendered MCP verification
  stay in `TODO.md`.
- 2026-09-30: reviewed MCP ownership against
  `crates/agent/src/tools/context_server_registry.rs` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. Removed the native runtime's
  write-only connector-selection cache, app-ID collection from user/skill
  input, and the last core-plugin mention module. Generic MCP readiness uses
  the existing shared mention parser; tool registration and approvals are
  unchanged. Skill selection no longer considers dormant connector names,
  but still excludes explicit MCP/legacy app links from skill matching.
  No Zed code was copied. All 44 retained skills tests and 100 AHEAD agent
  tests passed; the authenticated Pi test remains ignored. The native
  streaming regression now asserts that a project slash skill's body reaches
  the actual local-mock model request. The proxy suite passed all 140 on a
  repeat run after one child-edit notification timeout; the unresolved timing
  failure is tracked separately in `TODO.md`. Authenticated/rendered agent journeys
  and the remaining `codex_apps` transport/approval closure stay in `TODO.md`.
- 2026-09-30: followed Zed's `test-support` feature/dev-dependency pattern
  (`crates/agent/Cargo.toml`, `crates/project/Cargo.toml`) for AHEAD's native
  runtime tests. The proxy tests now select the built AHEAD sandbox helper
  instead of reentering their Rust test executable. This does not change the
  default app dependency graph or bypass sandboxing. The full proxy library
  suite passed all 140 tests on macOS with local mock-provider sockets,
  including child edit attribution and paginated Turso child resume.
- 2026-09-30: compared save notifications and diagnostic merging with
  `crates/project/src/lsp_store.rs` at
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. AHEAD now sends `didSave`
  after the proxy saves the editor's captured buffer. A real Rust Analyzer
  probe exposed a compiler error being overwritten by an empty pull reply.
  The proxy now retains diagnostics per server and per pushed/pulled source,
  following Zed's `DiagnosticSourceKind` behavior. The focused regression
  verifies that clearing one source leaves the others intact. This is an
  AHEAD implementation; no Zed source was copied. Native tracing also found
  that an absent static save capability prevented Rust Analyzer's dynamic
  `didSave` registration from being used. That fallback is now covered by a
  focused regression; boolean save support omits text unless requested,
  matching Zed's `include_text` helper. In the rebuilt native app, saving an
  unresolved function produced E0425 in Problems; fixing and saving it cleared
  the error while Problems stayed open. The trace confirmed both `didSave`
  notifications and compiler replies. Remaining LSP lifecycle work is in
  `TODO.md`.
- 2026-09-30: reviewed the pinned `crates/languages/src/{rust,typescript,python}.rs`,
  `crates/language_extension/src/extension_lsp_adapter.rs`, and
  `crates/project/src/lsp_store.rs` for broad-use language support. AHEAD's
  existing Wasmtime extension host is now part of normal builds, not a disabled
  optional path. Rust Analyzer, Vtsls (shared by TypeScript/JavaScript), and
  BasedPyright use the same native LSP client as installed language extensions;
  matching extensions take precedence, and failures remain visible. Commands
  currently require PATH installation; automatic acquisition and restart remain
  in `TODO.md`. Extension filename/first-line detection is reachable for unknown
  suffixes, and selectors accept manifest language names and protocol IDs.
  Guest stdin/stdout are isolated from the proxy's framed RPC transport.
  All 6 extension-host and 140 proxy library tests passed. The disposable
  multi-language fixture passed Rust, Node and Python tests, TypeScript checking
  and BasedPyright checking. These checks do not prove native language-server
  interactions, current-API extension execution or syntax grammar integration;
  those remain explicit native-test gates. No Zed implementation was copied
  for these changes.
- 2026-09-17: tracking file created. No Zed checkout pinned yet.
- 2026-09-21: pinned `418f89714891f9d8105a3e92e60b9a7a5084d232`
  (main, 2026-09-21, "Prevent multibuffer items from switching acti...").
  Checkout lives at `~/dev/zed` (shallow, outside this repo — do not
  vendor it here; record imported paths per item below).
- 2026-09-21: direct source reuse accepted for the editor, search, source
  control, terminal/task and debugger surfaces. Pin the checkout before the
  first import; preserve Apache-2.0 notices and record every imported path.
- 2026-09-21: license correction — Zed editor crates (`rope`, `text`, `lsp`,
  `project`, `editor`, `search`, `git`, `terminal`, `dap`) are GPL-3.0
  (`LICENSE-GPL` verified in `crates/rope`), not Apache-2.0 as decision 0005
  assumed. All GPL imports paused per decision 0006; pin deferred to legal
  sign-off. Only `zed_extension_api` (+ `gpui` family) confirmed Apache-2.0.
  Crate replacement verdicts recorded in decision 0006.
- 2026-09-21: maintainer decision — import GPL Zed source now, clean-room
  later (decision 0006). Pin + import unblocked; clean-room rewrite is a
  standing TODO obligation. Until the rewrite, preserve per-file copyright
  notices and ship license texts with distributed builds.
- 2026-09-21: FIM behavior port (independent implementation, no verbatim
  copy — see decision 0006 clean-room record): prompt-format inference,
  FIM prompt tags, stop tokens, completion cleaning, cursor-excerpt
  bounding, Ollama `/api/generate` routing, ghost interpolation on
  typing. Sources: `crates/edit_prediction/src/{fim,ollama,
  open_ai_compatible,cursor_excerpt}.rs` and `interpolate_edits` from
  `crates/edit_prediction_types/.../edit_prediction_types.rs`. Tests
  carried (adapted): format-inference family cases. AHEAD additions:
  session comment header, unknown-model fallback path, generation-guarded
  ghost flow with request-offset tracking.
- 2026-10-02: AHEAD's FIM host-to-provider regressions now cover the
  standard root-to-target `AGENTS.md` hierarchy for both the active file and
  a relevant open buffer in another subtree, plus editor-only prediction
  before any session exists. Both inspect actual loopback provider requests;
  native typing, insertion and undo remain separate acceptance checks.
- 2026-09-22: `ahead-extension-host/wit/since_v0.8.0/` records the Apache-2.0
  Zed extension WIT contract at pinned commit
  `418f89714891f9d8105a3e92e60b9a7a5084d232`. The AHEAD host is independent
  code; Zed's GPL extension-host implementation was not copied. See the crate
  `NOTICE` and the clean-room requirements document for the supported
  language-server boundary.
- 2026-09-26: the WIT guest's language-server workspace configuration now
  reaches the LSP host, which advertises and answers `workspace/configuration`
  from its startup snapshot. The section lookup follows Zed's
  `crates/project/src/lsp_store.rs` request handler. Scope behavior and refresh
  parity were still under review; see the 2026-09-28 clarification below and
  the remaining refresh gate in `TODO.md`.
- 2026-09-28: reviewed the pinned extension path more closely. Zed's
  `crates/language_extension/src/extension_lsp_adapter.rs::workspace_configuration`
  ignores `requested_uri`, and the v0.8.0 guest callback receives a worktree,
  not a scope URI. The per-scope dispatch in `crates/project/src/lsp_store.rs`
  is generic adapter behavior; extension-provided settings are scope-invariant.
  At that point AHEAD still needed to reevaluate that guest callback and send
  `workspace/didChangeConfiguration` after settings changes. The live guest
  verification gate remains in `TODO.md`.
- 2026-10-01: rechecked Zed's
  `crates/extension_host/src/wasm_host/wit/since_v0_8_0.rs::get_settings`
  and `crates/project/src/lsp_store.rs::on_settings_changed`. AHEAD's
  `ahead-extension-host/src/host.rs::get_settings` had returned `{}` for every
  request. It now reads the requested server's `lsp` category from bounded,
  symlink-safe user and project layers, using only ignored local config for
  executable binary overrides. Following Zed's
  `maintain_workspace_config`/`refresh_workspace_configurations` path, AHEAD's
  workspace watcher now triggers an off-thread extension callback; the catalog
  ignores stale results and updates the LSP host before sending
  `workspace/didChangeConfiguration`. Focused extension-host tests and all 188
  proxy library tests pass with local loopback permitted. At this stage a real
  guest and rendered app remained unverified; the later installed-package
  probe below closes the former, not the latter. User-global live watching
  remains open in `TODO.md`.
- 2026-10-01: a real-Wasm smoke in `ahead-extension-host/src/host.rs` copies an
  installed Zed extension into a disposable project before calling the AHEAD
  host. Locally installed Proto and HTML extensions declare API 0.7.0; both
  initially failed under AHEAD's 0.8.0-only binding (different GitHub asset
  and DAP TCP-host shapes). Following Zed's
  `crates/extension_host/src/wasm_host/wit.rs`, AHEAD now dispatches API 0.6/0.7
  through the Apache-2.0 `since_v0.6.0` WIT, copied exactly from pinned Zed,
  and retains `since_v0.8.0` for API 0.8. The AHEAD host implementation is
  independent; no Zed GPL host source was copied. Both installed-package
  probes now pass offline after packaging each installed manifest/Wasm pair
  and exercising AHEAD's installer, including changed workspace settings. Missing or
  unsupported API versions fail at install/discovery. Proto's actual command
  and initialization-options callbacks also pass with a disposable local
  binary override, without fetching a GitHub release. Following Zed's
  `crates/extension_host/src/wasm_host.rs` serialized extension-call loop,
  AHEAD now retains one Wasm guest/store per extension and worktree and
  serializes calls with a per-guest async lock. The installed-package probes
  exercise separate Tokio runtimes and assert a single instantiation across
  command and multiple configuration calls. Hot extension upgrades still
  need targeted invalidation or proxy restart. Language-server startup and
  live refresh in a native window remain unverified; keep the smoke on
  disposable copies, not the user's Zed installation.
- 2026-10-01: Zed's `crates/extension_host/src/headless_host.rs` loads each
  extension independently and retains failures for reporting. AHEAD's
  discovery had aborted the entire installed-adapter list on one malformed
  manifest or unsupported API version. It now returns valid adapters alongside
  per-package errors; the proxy reports each error through the existing
  server-status route. Language detection also keeps valid definitions when
  an unrelated package is malformed, without silently swallowing the error
  if no definition matches. Regressions cover both cases, and the real
  Proto/HTML offline probes still pass. Rendered
  failure visibility remains to be checked in the native app.
- 2026-09-23: search reference review at the pinned checkout. Zed's
  `crates/project/src/{search,project_search}.rs` separates path candidates,
  match detection and open-buffer range search, with bounded workers and
  ordered streaming results. `crates/search/src/project_search.rs` owns the
  query UI; `crates/agent/src/tools/grep_tool.rs` calls the same project search
  and offers an include glob and 20-result pages using an offset. AHEAD's
  managed `file_search` instead exposes only a revision-bound cursor so a
  changed worktree or unsaved buffer cannot silently shift later pages.
  `crates/file_finder/` uses a
  separate ranked path picker. AHEAD's shared `ahead-core::search` matcher is
  independent code, not an imported Zed file. It now accepts compiled
  include/exclude globs for panel content search and an include glob for
  native-agent search. The panel caches a proxy-owned path snapshot across
  queries, and simultaneous cold callers share its initial walk, but AHEAD
  still scans candidate file contents serially and lacks Zed's incremental
  worktree index, candidate pipeline and full query controls. AHEAD's `Cmd+P`
  modal now consumes its shared path ranker and generation-tagged proxy
  snapshot. A disposable native-window pass confirmed `Cmd+P`, filename
  filtering and Enter opening a selected file; restart persistence and
  large-worktree responsiveness remain unverified. See the search item in
  `TODO.md`;
  do not describe the current matcher as Zed-equivalent.
- 2026-09-29: pinned Zed `crates/project/src/project_search.rs` reads matches
  through its worktree/file abstraction. AHEAD's shared matcher now requires
  the selected workspace for disk search, uses no-follow handle-relative reads
  on Unix, and never reopens a path for encoding fallback. Panel, proxy, and
  native agent use this same boundary; buffer-only proxy search cannot open
  disk. The Windows path-based fallback and live GPUI behavior remain open.
- 2026-09-24: adopted the pinned
  `crates/search/src/project_search.rs` and `crates/util/src/paths.rs`
  path-filter behavior as an AHEAD implementation: comma-separated patterns
  with brace-glob commas preserved, literal directory-prefix matching and
  exclusion precedence. The panel and native agent still share
  `ahead-core::search`; Zed's worktree snapshot and concurrent candidate
  pipeline remain open. No Zed source file was imported for this filter.
- 2026-09-24: added an AHEAD-owned lazy file snapshot for proxy
  `GlobalSearch` and the managed agent, invalidated by the existing filesystem
  watcher on file-set and ignore-rule changes. The panel receives a
  generation-tagged copy by RPC, reuses it across queries, and refreshes
  matches after external content changes without rebuilding the path list.
  The proxy now uses that snapshot's visible entries to suppress search
  invalidation from ignored build-output events after the first walk.
  This follows
  the path-candidate lifetime in
  Zed's pinned `crates/project/src/project_search.rs` but does not copy its
  worktree code. Explorer, ordered concurrent candidate scanning and Quick
  Open remain open, as does live watcher/performance verification.
- 2026-09-24: exposed the shared matcher's case, whole-word and regex modes
  in the gpui-kit Search panel, following the controls in the pinned
  `crates/search/src/project_search.rs` without importing Zed's view code.
  Aligned the regex matcher's line-anchor and CRLF behavior with Zed's
  `crates/project/src/search.rs` so `^`/`$` act on lines, not whole files.
- 2026-09-24: followed pinned
  `crates/agent_ui/src/thread_metadata_store.rs` for metadata-first sidebar
  ordering by recent activity. AHEAD derives `updated_at` from indexed Turso
  conversation-message timestamps and keeps Codex replay items separate;
  no Zed source was imported for this change.
- 2026-09-24: made the AHEAD file-index cold walk cooperative for cancelled
  managed-agent searches and superseded proxy searches. A cancelled builder
  wakes waiting searches, which can build the current snapshot. This follows
  Zed's cancellable project-search candidate pipeline. The panel's
  `WorkspaceFiles` RPC now carries a request id and can cancel a superseded,
  closed or timed-out search while it awaits the shared path snapshot. AHEAD still has
  serial content scanning and needs live responsiveness verification.
- 2026-09-24: compared Zed's integrated `crates/agent/src/thread.rs` model
  request path with retained Codex `core/src/client.rs` and `codex-api`.
  AHEAD removed the unused Guardian/classifier inference routes and
  `free_guardian` switch; managed requests now have one `/responses` route.
- 2026-10-01: Zed's pinned
  `crates/agent/src/agent.rs::NativeThreadEnvironment::create_subagent_thread`
  creates a child from its parent and depth, without a reviewer-labeled source.
  AHEAD removed its copied Guardian label checks and instead rejects all
  unsupported feature-thread sources at creation and restore; arbitrary child
  labels remain non-root metadata. The focused AHEAD-owned create/restore test
  passes, but a running-app permission check remains open.
- 2026-09-24: external ACP adapter review against pinned
  `crates/project/src/agent_server_store.rs` and
  `crates/paths/src/paths.rs`. AHEAD now puts the registry and Npx packages
  under one user-local `external_agents` directory, while launching each
  agent in its selected project. The adapter ID is sanitized to one path
  component (with an extra `.`/`..` guard), and cross-platform file locks
  serialize installs across agent threads and processes. This is an AHEAD
  implementation of the observed layout, not an imported Zed file; no Zed
  tests or license-bearing source files were copied in this change.
- 2026-09-24: Zed's `crates/agent_servers/src/acp.rs` treats agent-advertised
  `configOptions` as the external session's model selector and applies choices
  through `session/set_config_option`. AHEAD now applies an explicit model
  choice on ACP session creation and reload through that standard method,
  failing when the adapter does not offer it. The external conversation now
  prepares its ACP session on attach, renders advertised select options in
  order, applies choices through the standard method, and replaces the state
  from full responses or agent updates. This follows Zed's session-config
  state boundary without importing its GPUI selector. Live UI verification
  and Pi provider access remain open in `TODO.md`.
- 2026-10-02: extended the disposable Pi ACP smoke test through Zed's
  `session/set_config_option` and `session/load` paths. The real installed
  adapter advertised an alternate model, accepted it offline, and returned an
  updated option. A fresh adapter process reopened the session after AHEAD
  re-resolved the install and saved model default; the selected model was
  advertised again. This verifies the ACP client exchange and empty-session
  restore, not provider inference, conversation replay or GPUI rendering.
- 2026-10-02: Zed's ACP `open_or_create_session` keeps the load RPC in an
  asynchronous task. AHEAD's proxy already routes `AgentSessionPrepare` to a
  background worker; a focused proxy regression now holds the session host
  busy and confirms an editor RPC still returns before preparation can finish.
  The running GPUI startup and option-update path remains unverified.
- 2026-09-25: followed Zed's pinned
  `crates/project/src/project_search.rs` and
  `crates/language/src/file_content.rs` decode boundary for shared content
  search. AHEAD validates ordinary UTF-8 while streaming so malformed bytes
  are detected even when the query has no match on that line; BOM-marked and
  detected legacy text then use the same decoded-text matching path. Results
  report byte columns in decoded UTF-8, as the editor expects. The complete
  `ahead-core` suite passes (27 tests). This is AHEAD-owned code, not copied
  Zed code; whole-file fallback memory use and binary detection beyond NUL
  bytes remain open.

## Archived agent audit recovery (2026-09-30)

The archived chats' worktree directories are gone, but their saved Git objects
remain under `refs/codex/snapshots/`. These commits match the edited files and
reported work in the named chats. Both other registered worktree branches have
no commits ahead of `main`; the existing secondary checkout is clean.

| Archived chat | Matching saved commit | Scoped comparison |
| --- | --- | --- |
| Audit agent chat and ACP | `a3d2d3544a588bdd998ee58ad99cd17d74067248` | Current code already has policy-hash checks, serialized turn startup, matching-turn cleanup, UI event guards, explicit resume failure, load/resume capability selection, durable presentation state, `/compact` and command metadata. Checkpoint export/restore was still missing that presentation state; adapted that fix to the current DTO. |
| Audit native agent architecture | `1b43c1b8629bb0ed6392b49f67618d46fb0c5f99` | The duplicate deterministic loop and standalone tool binaries are gone; `SOURCE_BASELINE.toml` records the fork. Did not import the interrupted broad crate/module rename. Remaining runtime pruning is tracked in `TODO.md`; private identifier spelling alone does not reduce the compiled dependency graph. |
| Audit docs tests and build speed | `3202d4702db1e75220e471edf73b816576ea4f92` | The build guide, package-qualified dev command, explicit workspace checks, per-process job-limit warning and disposable Pi fixture are present. Its separate-proxy packaging fix is superseded by the current `ahead --proxy` executable mode. |

Compared the chat snapshot's scoped source changes against the current files,
not just Git history: much of the current runtime extraction is untracked.
No complete snapshot was cherry-picked, no archived chat was restarted, and
no saved snapshot was deleted.

The old mandatory edit-scope gate conflicts with the current managed policy:
Assist permits workspace-bounded writes unless an explicit scope narrows them;
Learn remains read-only. The snapshot's blanket ACP permission rejection also
does not enforce external agents' own effects. Current unknown/read-only ACP
sessions decline client permission requests; Assist remains explicitly
unmanaged. Neither obsolete change was imported. Raw reasoning and pending
input are excluded from AHEAD's durable presentation state and checkpoint.

For checkpoint recovery, reviewed Zed's `crates/agent/src/db.rs::DbThread` and
`crates/agent/src/thread_store.rs` at
`418f89714891f9d8105a3e92e60b9a7a5084d232`. AHEAD reuses its own
`AgentRuntimeState` rather than adding another serialization model. The export
now requires that current presentation field; no older-bundle fallback or
conversion layer was added. Runtime bindings, credentials and model replay
history are not exported. The restored checkpoint is a readable session, not
proof that the original model conversation can resume on another machine.

`ahead::host::tests::export_restore_preserves_agent_presentation_state` failed
before the fix: the fresh Turso store returned `None` for the exported state.
It now verifies JSON export/import, file-backed database reopen, all five
presentation fields, and the absence of a transferred harness binding. The
combined run passed 101 agent tests and 141 proxy tests, including the existing
empty-state checkpoint round trip:

```sh
rtk proxy cargo test --locked --offline -p ahead-agent -p ahead-proxy --lib -j1 -- --test-threads=1
```

Authenticated Pi remains ignored by user request. Workspace formatting and
whitespace checks passed. These are local tests in disposable stores, not a
rendered checkpoint-import journey or an authenticated model turn.

The recovered audit also called for atomic adapter installation. The subsequent
fix stages packages and publishes an immutable generation through an atomic
manifest replacement, with concurrent-launch/failure regressions and a real
offline npm/Node smoke test. Registry downloads and rendered installation,
safe cache cleanup and authenticated Pi remain in `TODO.md`.

## Debugger rebuild and smoke (2026-10-01)

After the user ran `cargo clean`, the focused offline DAP suite passed all 13
tests. The first full proxy suite had 175 passing tests and four native-agent
tests unable to start because `target/debug/ahead` (their sandbox helper) had
been removed by the clean. The documented single `just dev` loop rebuilt the
root executable and launched the disposable multi-language project at
`/private/tmp/ahead-recovery-dev-1uccxv`; the native editor opened its Rust
file. After stopping that watcher, the full proxy suite passed 179/179 tests.
The rebuilt executable also passed `tests/dap-shutdown-smoke.mjs` in running,
initializing and full-stderr modes, with each disposable mock adapter reaped.

Zed's pinned `crates/project/src/debugger/session.rs` uses a per-request
`RunInTerminal` response channel, and `crates/debugger_ui/src/session/running.rs`
spawns a real terminal and returns its PID (see its successful and error
reverse-request tests). AHEAD now carries the DAP argv/cwd/env and
session/generation/request identity across RPC, opens the existing native PTY
terminal, replies with its shell PID or error, and closes it through the app on
session retirement. The skipped `RunDebugConfig.debug_command` and proxy-owned
terminal-close path were removed. A new proxy regression rejects a stale reply
and verifies that the matching PID reaches DAP; an app regression covers shell
quoting and environment-key validation; a second app regression verifies
request delivery, reply identity and retirement notification. After this
change, the full proxy suite passed 180/180 and the app suite passed 149/149
offline.

For native verification, launched `just dev` in the disposable project with a
local mock DAP adapter at `/private/tmp/ahead-recovery-dev-1uccxv/dap-adapter.mjs`
(not added to the product repository). Pressing F5 with `src/main.rs` open
spawned a visible native terminal that printed `AHEAD_DAP_TERMINAL_OK`; the
adapter log recorded a successful `runInTerminal` response with
`shellProcessId: 10383`. Shift+F5 sent `disconnect`, removed the debug
terminal tab, and a process check found PID 10383 gone. This is a mock-adapter
journey, not proof of real LLDB/Node/Python debugging, concurrent terminal
requests, cancelled-spawn races, cross-platform behavior or the full debugger
surface. Project-scoped configuration and these checks remain in `TODO.md`.
Opening a Rust file did not verify LSP behavior.

The final root-app rebuild replayed the same journey with a new shell PID
25439; it also exited after Shift+F5. The test-mode `just dev` watcher was
stopped afterward. `cargo fmt --all --check` and `git diff --check` passed.
Targeted Clippy did not reach a clean result: `-D warnings` failed on two
retained path-URI suggestions, and the repository-policy run stopped on 29
existing `ahead-agent` errors. This is tracked separately in `TODO.md`, not
counted as a passed lint gate.

On 2026-10-02, rechecked Zed's
`crates/project/src/debugger/session.rs::{continue_execution,step_over,on_step_response}`:
Zed marks the thread as moving before the adapter responds and restores its
stopped state on error. AHEAD's session state also owns the stopped frame, so
it now admits only one Step/Continue request until a reply or new state event
instead of clearing that frame optimistically. The new real-process admission
regression and all 15 proxy DAP tests pass. The rebuilt root executable also
passed `tests/dap-shutdown-smoke.mjs` in running, initializing and full-stderr
modes. Rapid native shortcut presses and rendered error/retry remain unverified
while macOS is locked.

## Targeted lint and rollout-import verification (2026-10-01)

After the retained-feature cleanup, the focused agent/app/proxy Clippy gate
passed with zero errors and 80 warnings. Seven AHEAD metadata `clone_on_copy`
errors were removed; the DAP and LSP timeout paths now use Rust 1.87-compatible
map operations while preserving callback-after-unlock behavior. All six focused
proxy timeout tests pass. The proxy test target also needed the existing
workspace `zstd` dependency declared for its compressed-rollout fixture.

Four focused proxy legacy-import tests pass for plain/compressed source,
unsafe-source rejection, and atomic/idempotent Turso import. The agent's
bounded-reader test also passes. These are local tests, not a native app
restart/import/restore observation. The remaining Clippy warnings, actual
Rust 1.87 compilation, and native journey remain in `TODO.md`.

The complete app library suite passed 161 tests. The complete proxy suite
passed 185 tests with local-loopback permission for its disposable model/FIM
fixtures; in the restricted sandbox, 178 passed and seven could not bind
loopback sockets. The full agent suite passed 110 tests with two opt-in tests
ignored. A native managed-turn regression originally expected a `tools` array
for bundled `gpt-5.6-sol`; the actual request omitted it because that model's
retained metadata requires CodeModeOnly and AHEAD has no bundled Code Mode host.
The regression now checks the optional wire shape and excludes unavailable
`exec`/`wait` tools. This is fail-closed, not a claim that the model can use
tools in AHEAD. Zed's pinned `crates/agent/src/thread.rs` (around line 4139)
collects the turn's available tools into each direct `LanguageModelRequest`;
AHEAD must either ship the retained host or make a deliberate model-mode
decision before offering equivalent functionality for CodeModeOnly models.

## Native Code Mode warning and restore check (2026-10-01)

In a disposable copy of `tests/fixtures/editor-smoke/`, `just dev` bootstrapped
`.ahead/` and opened a new managed session. Settings autosaved a credential-free
OpenAI-compatible connection to unavailable loopback `127.0.0.1:9` with model
`gpt-5.6-sol`. Sending a read-only prompt showed the missing Code Mode host
warning in the panel's warning area, distinct from the transient Thinking and
network-retry content. Cancelling the turn kept the warning visible. After
stopping and restarting the native app with the same isolated user profile and
workspace, the cancelled turn and warning both restored. The `just dev` watcher
was stopped afterward.

No model response, tool call, host worker or authenticated provider was
exercised: the loopback endpoint was intentionally unavailable. The host
bundle and provider-backed safety check remain open in `TODO.md`.

## Hosted Apps file-upload removal (2026-10-01)

Compared the retained MCP call path with Zed's direct JSON argument handoff in
`../zed/crates/agent/src/tools/context_server_registry.rs`. AHEAD no longer
registers `codex_apps`; the only consumer of OpenAI file-argument rewriting
was guarded for that removed server. Deleted the core rewrite, upload-only
client pool and API upload module, then removed `codex-mcp`'s hosted-only
`fileParams` schema masking and unused `ToolInfo` field. No Zed code was copied
and no user data was changed.

The complete AHEAD agent library suite passed (114 active, 2 opt-in tests
ignored). After dropping the upload module's unused direct `uuid` dependency
and Tokio `fs` feature, the locked production `ahead-agent` check and app
binary build passed again; the suite was not rerun for that manifest-only trim.
The workspace formatting check passed after a formatting-only assertion change
in `ahead-app/src/code_panel.rs`.
The copied
`codex-core --all-targets` test target still cannot compile because inherited
test dependencies are absent; this is not evidence of an MCP regression or a
replacement for live verification. The rendered stdio MCP tool-call and human
approval journey remain open in `TODO.md`; no authenticated provider was used.

## Managed stdio MCP approval roundtrip (2026-10-01)

Compared Zed's third-party MCP authorization and JSON argument handoff in
`../zed/crates/agent/src/tools/context_server_registry.rs` with AHEAD's
managed turn path. An AHEAD-owned offline test now starts a disposable stdio
MCP server and loopback model, discovers the tool through deferred search,
and verifies the server receives exact JSON arguments only after human
approval. It first exposed a hang: `NativeClient` answered
`Op::UserInputAnswer` with the tool-call ID while the retained session stored
the pending answer under the turn ID. Answering by turn ID unblocked one
call, but the extended test reproduced a second failure with two parallel
read-only calls in one turn: their pending answers overwrote each other. The
runtime now keys pending input by call ID, matching the UI and model call,
so each approval resolves only its own tool.

`cargo test --locked -p ahead-agent --lib` passed: 115 tests, two opt-in tests ignored.
Workspace formatting also passed. This is not a rendered GPUI or authenticated
provider result; those approval journeys remain open in `TODO.md`.

The retained runtime's stable tool-call elicitation feature is now enabled
for AHEAD-managed sessions. `NativeClient` maps only its identifiable,
empty-schema MCP tool approval forms into the existing chat question card;
the card includes the JSON arguments. An explicit Allow or Cancel resolves
that exact call. Other server elicitations still decline, and AHEAD continues
to advertise no general form or URL elicitation capability. The disposable
parallel stdio roundtrip passes for two independently approved calls and a
denied resource operation; a unit test checks the form discriminator. This
follows Zed's `../zed/crates/agent/src/thread.rs` per-tool human authorization
path while retaining the pinned runtime's request and response semantics.
It is not a rendered UI result: the desktop was locked during this pass.

The app-side review queue now follows Zed's
`../zed/crates/agent_ui/src/conversation_view.rs::Conversation` pattern:
retain every pending request per session in arrival order and retire only the
answered ID. Previously `ProxyClient` kept one request per session, so two
parallel MCP calls could overwrite the first card even though the managed
runtime held both approvals. The chat shows the next request and a pending
count. `parallel_agent_input_requests_remain_queued_until_each_is_answered`
now passes through the actual answer RPC, proving the second request remains
after the first succeeds; the native card sequence is still unverified while
macOS is locked.
`rendered_parallel_mcp_approval_shows_first_request` additionally draws the
headless GPUI chat, clicks Allow on the first card and verifies the second
request has not been presented simultaneously. It does not replace the native
window check or exercise an authenticated provider.

The managed event pump now follows Zed's
`../zed/crates/agent/src/thread.rs::run_authorization_loop` pattern of
keeping authorization waits in owned tasks while the main event stream
continues. Previously `NativeClient::consume_turn` awaited each human answer
inline, so the app could not receive the second parallel MCP approval until
the first was answered. The disposable two-call test now requires both
requests before either answer: it timed out before the fix and passes
afterward. Each turn also retires only its own unanswered host requests on
exit, so an aborted parent/child does not leave a stale answer route or remove
the other turn's pending requests. Its focused regression and all 124 active
`ahead-agent` library tests pass (two opt-in tests ignored). This is offline
host behavior, not a rendered GPUI or authenticated provider result. The
locked offline `ahead` binary check passes.

The host now assigns a unique AHEAD request ID to each pending approval,
including child-thread requests displayed in the parent work session. This
prevents separate runtime threads with the same model call ID from addressing
one another's pending answer; the retained runtime still receives its
original call and question IDs. The parallel roundtrip asserts distinct host
request IDs and still passes. A concurrent parent/child native review remains
unverified.

Per-turn cleanup now emits a cancellation notification for each unanswered
request that exits with that turn, following Zed's task-owned authorization
lifetime in `../zed/crates/agent/src/thread.rs::run_authorization_loop`.
The app removes only that card from its active-turn queue; a headless GPUI
regression shows the next card and clears the previous choice. All 124 agent
and 167 app library tests pass. This has not been checked in a native window.

## MCP client identity and hosted-Apps startup trim (2026-10-01)

Zed's `../zed/crates/context_server/src/context_server.rs` sends its editor
identity in MCP `clientInfo`, and its
`../zed/crates/agent/src/tools/context_server_registry.rs` uses the listed
tool name and JSON arguments directly. AHEAD now sends `AHEAD` as its MCP
client name/title and uses an `ahead-mcp-client` HTTP user agent. Removed the
unreachable hosted-Apps callable-name/namespace normalization module and
branch from `codex-mcp` startup; normal MCP discovery still strips untrusted
connector metadata and preserves the server namespace. AHEAD cannot register
`codex_apps` as a workspace server, so this does not retain an old-server
compatibility path. The disposable stdio MCP regression checks the AHEAD
client name and two independently approved tool calls; it passes. Remaining
hosted-Apps paths elsewhere in the retained runtime are tracked in `TODO.md`.
The locked offline AHEAD agent library suite passed 115 tests with two opt-in
tests ignored; `cargo check --locked --offline -p ahead --bin ahead -j 1` and
workspace formatting passed. The Mac was locked during this pass, so no
rendered app behavior was verified.

## Hosted MCP event and catalog branch removal (2026-10-01)

Checked Zed's per-server context store and tool registry before removing the
remaining host-owned Apps branch from `codex-mcp`. The hosted
`events/list`/`events/stream` resource adapter had no AHEAD caller: its only
core wrapper was uncalled, and its extension resource-client field was only
written. Deleted those paths and the watcher that kept a hosted event server
alive across runtime publication. AHEAD's normal MCP resource list/read entry
points still use `McpRuntime` directly. Removed the hosted catalog policy
exemption, larger item limit and cache bypass; all configured servers now
follow the ordinary per-environment policy and tool-cache path. No Zed code
was copied.

The locked offline AHEAD agent suite passed 115 active tests (two opt-in tests
ignored), including the disposable two-call stdio MCP approval roundtrip.
`cargo check --locked --offline -p ahead --bin ahead -j 1` passed. Rendered
resource/approval behavior remains unverified while the Mac is locked.

## Lower-level hosted MCP stream removal (2026-10-01)

Traced the remaining Plugin Runtime `events/stream` client against Zed's
`crates/agent/src/tools/context_server_registry.rs`: Zed issues ordinary
per-server `CallTool` requests, authorizes each call, and cancels through the
tool event stream. No AHEAD production caller remained for the hosted event
subscription. Deleted that request API, its notification-capture transport,
the HTTP event-specific cancellation and header timeout, and the copied
event-only tests. Standard streamable-HTTP SSE processing and `CallTool`
handling were left intact. No Zed code was copied.

`cargo check --locked --offline -p ahead --bin ahead -j 1` passed and
`cargo test --locked --offline -p ahead-agent --lib` passed 115 active tests
(two opt-in tests ignored). The copied `codex-rmcp-client` SSE-discovery test
target does not compile because its retired `pretty_assertions` and
`wiremock` dev-dependencies are absent, so that target gives no transport
proof. HTTP MCP remains gated on AHEAD-owned server-auth UI and a live
rendered journey.

## MCP tool-list connector parser removal (2026-10-01)

Compared the remaining tool-list handoff with Zed's
`crates/agent/src/tools/context_server_registry.rs`: Zed registers the
ordinary server-listed tool identity and authorizes the call per tool. AHEAD's
`codex-mcp` already discarded connector fields from ordinary server tools,
but `codex-rmcp-client` still ran a duplicate `tools/list` wrapper to parse
those untrusted fields. Removed the wrapper and reused its existing SDK-backed
`list_tools` result, retaining the ordinary metadata-stripping step. No Zed
code was copied. `cargo check --locked --offline -p ahead --bin ahead -j 1`
and `cargo test --locked --offline -p ahead-agent --lib` passed (115 active,
two opt-in ignored). This is source/test evidence, not a rendered approval
journey.

## Hosted MCP approval-template removal (2026-10-01)

Zed's `crates/agent/src/tools/context_server_registry.rs` authorizes the
ordinary server/tool identity per call. The retained Codex core instead had
a bundled consequential-question template table keyed exclusively by
`codex_apps` connector IDs. AHEAD rejects that hosted server, and its
ordinary tool registry never supplies those IDs, so the table and renderer
were unreachable. Deleted the module and asset; kept the generic approval
question and JSON argument display. Adjusted the copied approval test to
describe ordinary server metadata rather than a hosted template. No Zed
code was copied.

The locked offline app check and AHEAD agent suite passed (115 active tests,
two opt-in tests ignored), including its independently approved two-call
stdio MCP roundtrip. The copied `codex-core` test target remains unbuildable
from missing historical test-only dependencies; this does not establish a
rendered approval journey. Remaining hosted metadata/DTO branches stay in
`TODO.md`.

## Hosted MCP request metadata removal (2026-10-01)

Compared the retained core's `tools/call` envelope with Zed's direct
per-server call in `crates/agent/src/tools/context_server_registry.rs`.
Removed the unreachable `_codex_apps` request metadata payload and the
hosted connector-ID exception in session approval keys. AHEAD still sends
the ordinary call ID, turn metadata and eligible confirmation policies,
and still requires the generic per-server approval path. Deleted the copied
tests that asserted a hosted-only envelope; no Zed code was copied. The
locked offline app check and AHEAD agent suite passed (115 active tests,
two opt-in tests ignored), including the parallel stdio MCP approval
roundtrip. Native rendered verification remains open.

## MCP event identity pruning (2026-10-01)

Kept Zed's ordinary server/tool authorization model as the reference and
traced every `McpToolCallItem`/legacy-event constructor in AHEAD. Deleted
hosted connector, link, app-action and plugin identity fields from those
DTOs and projections. Kept tool `readOnlyHint` and MCP Apps
`_meta.ui.resourceUri`/`_meta["ui/resourceUri"]` because the
[MCP Apps extension](https://apps.extensions.modelcontextprotocol.io/api/documents/overview.html)
defines the tool-UI link; removed the proprietary
`openai/outputTemplate` fallback. AHEAD currently forwards the URI as
metadata but does not render an MCP App, so no host support is claimed. No
Zed code was copied.

The locked offline app check passed. The copied `codex-protocol --lib` test
target cannot compile without retired `pretty_assertions`/`tempfile`
test-only dependencies; an AHEAD-owned managed-agent regression now checks
begin/end event identity, UI URI, read-only hint and error propagation.
The full AHEAD agent suite passed 116 active tests with two opt-in tests
ignored. Native rendered verification remains open.

## MCP approval and tool identity pruning (2026-10-01)

Zed's `ContextServerTool::run` in
`crates/agent/src/tools/context_server_registry.rs` authorizes a tool with
`mcp:<server_id>:<tool_name>` before calling that server. AHEAD now removes
hosted connector IDs/names/descriptions, account email and plugin ID from its
managed MCP approval pipeline, lower tool catalogue, request metadata and
metrics. Ordinary approval still names the server and tool, shows original
arguments and optional tool title/description/annotations, and can remember
an automatic-mode choice for the session. The retained model-facing tool
name, schema, result and error semantics are unchanged for configured MCP
servers. No Zed code was copied. A native rendered approval journey remains
unverified; the disposable loopback MCP fixture covers the managed path. The
locked offline app and formatting checks passed. The AHEAD agent suite passed
(116 active tests, two ignored), then its focused approval roundtrip passed
again after the header copy changed. The first sandboxed suite run could not
bind local sockets; the approved loopback rerun passed.

## MCP resource gate cleanup (2026-10-01)

Zed keeps context servers under their configured server IDs in
`crates/project/src/context_server_store.rs` and loads their tools and prompts
through `crates/agent/src/tools/context_server_registry.rs`; there is no
hosted-Apps resource access mode to mirror. AHEAD removed its inert
`codex_apps`-only resource filter and the `orchestrator.mcp.enabled` config
flag. Explicit resource reads and aggregate resource/template listings now
use the same registered-server runtime path. The workspace-level
`codex_apps` name reservation and executor-local HTTP MCP discovery branch
were removed after the caller audit; the former hosted name is now only an
ordinary server ID, consistent with Zed's ID-keyed server registry. No Zed
code was copied. Native
rendered verification remains open. A disposable managed stdio roundtrip now
lists and reads a resource through the agent loop and passes; it is a
legacy-version fixture, not a 2026-07-28 resource conformance test.

The executor-local HTTP MCP discovery projection and its unused exec-server
helper are removed. Managed AHEAD sessions still load the reviewed workspace
stdio declarations; generic environment-config reading remains. Validation
ran on Darwin arm64 (`aarch64-apple-darwin`): the 117-test AHEAD agent suite,
the `codex-exec-server` and `codex-core` library checks, and the `ahead` binary
check passed. No remote executor was selected by the native client, and no
Docker/Wine or remote-host lane ran, so these results do not claim remote
execution coverage.

The model-callable resource handlers now pass through the retained per-server
MCP review path before dispatch, following Zed's authorize-before-request
ordering in `crates/agent/src/tools/context_server_registry.rs`. Aggregate
listings review each configured server first; they no longer identify
themselves as a `codex` server in turn events. The disposable mock-model test
proves a denied list sends no request and then exercises approved resource,
template and read requests. Resource approval is intentionally independent of
server-owned MCP tool-name allowlists to avoid granting unrelated operations
through a name collision. AHEAD now lets the human switch a reviewed,
workspace-local server between per-call review and auto-approval of permitted
tools and resources. The choice is tied to the current declaration fingerprint;
named tool `deny` and `confirm` entries still override it. This follows Zed's
per-server authorization order and tool-policy precedence in
`crates/agent/src/tools/context_server_registry.rs` and
`crates/agent/src/tool_permissions.rs`; no Zed code was copied. The AHEAD
agent suite passes 117 active tests (two opt-in tests ignored). The disposable
mock-model test now also switches the reviewed server to auto-approval for a
second managed session and checks that a tool call and resource listing reach
the stdio server without prompting. Native approval UI remains unverified.

## Copied Apps configuration removal (2026-10-01)

Compared Zed's project-owned `crates/project/src/context_server_store.rs` and
`crates/agent/src/tools/context_server_registry.rs` with AHEAD's retained
configuration. The Codex Apps `[apps]` config types, generated-schema field,
managed requirement merge branch, and their copied fixtures had no production
consumer, so they were removed. Ordinary `mcp_servers`, per-server approval
overrides, and the managed MCP effect boundary remain. No Zed code was copied.
The AHEAD agent path and config test target compile; a native rendered MCP
roundtrip and a current-version resource conformance test remain open.

## ACP editor bridge backpressure (2026-10-01)

Zed's `crates/agent_servers/src/acp.rs::mcp_servers_for_project` remains the
reference for passing project-owned MCP servers to external ACP sessions.
AHEAD's stdio editor bridge now uses a 32-slot bounded standard-library event
channel instead of an unbounded one; the reader blocks when the queue fills.
No Zed code was copied. The local bridge now caps accepted socket workers at
20, leaving four slots beyond the pending tool-call quota for cancellation and
releasing a slot when a worker exits or fails to start. The backpressure,
worker-slot and AHEAD agent library tests pass (121 active tests, two opt-in
tests ignored) with loopback access. AHEAD's local bridge now adds total socket
read deadlines and a 2 MiB response limit, with slow-drip and oversized-reply
regressions. Zed does not use this AHEAD-specific editor socket bridge; its
project-owned MCP server handoff remains the architectural reference. Native
ACP, overload and cancellation races at the new limits remain open.

## Display-bound windows and combinable search options (2026-10-01)

Zed's `crates/workspace/src/workspace.rs` leaves `window_bounds` unset for a
new workspace and lets GPUI choose display-aware defaults. AHEAD's fixed
1600×1000 opening window put the new-thread wizard's final control outside the
clickable region on the disposable native smoke display. AHEAD now uses
`TitleBar::window_options()` without overriding the bounds. Rebuilt native
verification of the new size and wizard completion is still pending.

Zed's `crates/search/src/search.rs::SearchOptions::build_query` treats case,
whole-word and regex as independent options; regex selects the query parser,
while the other two refine its matching. AHEAD's panel toggles and shared
matcher already use the same combinable semantics, so no exclusivity change
is needed. The native disposable project confirmed a live-buffer Git rail
click opens the hunk review card without turning that line into a breakpoint;
the card still overlays code instead of expanding editor rows as Zed does.

## Terminal reference-test recording (2026-10-01)

Zed's `crates/terminal/src/alacritty.rs` passes `false` for Alacritty's
`EventLoop::new` reference-test flag. AHEAD had passed `true`, which makes
Alacritty write PTY output to `./alacritty.recording` relative to the app
process, including during ordinary editor use. AHEAD now passes `false` too;
no Zed code was copied. All 167 app library tests pass after the change, and
a real PTY test run with a disposable working directory created no recording.
The pre-existing modified repository recording is preserved for review.

## Explorer ignore-state fallback (2026-10-01)

Zed's `crates/project_panel/src/project_panel.rs` reads each worktree entry's
`is_ignored` state. AHEAD still shells out to `git check-ignore` for its
Explorer tree; outside a Git repository, its already-handled empty-result
fallback printed repeated Git errors to the app's stderr. AHEAD now discards
that expected subprocess stderr. The 167-test app suite passes without the
earlier `fatal: not a git repository` noise. This is an AHEAD-only fix, not a
port of Zed's worktree cache; shared ignore-state integration remains part of
the broader search/Explorer parity work.

## Managed privacy and slash-skill cleanup (2026-10-01)

The retained agent runtime's config default still selected Statsig for metrics,
even though AHEAD does not initialize its OTEL provider in the production path.
The hard fork now defaults all exporters to `none`, and the native managed
config overrides an explicit runtime-home request for log, trace, metrics, or
prompt logging. A focused regression proves the retained loader reads those
requests before checking AHEAD's override, and the full AHEAD agent suite
passes (125 active tests, two opt-in tests ignored). The obsolete `/:name`
skill alias was also removed; source-qualified collision names follow the
Zed-inspired skill selection documented in `ahead-agent-standards.md`. These are
AHEAD-only cleanups, not Zed code ports. Native-turn privacy verification is
still pending because the Mac is locked.

## Extension-gallery source and install boundary (2026-10-01)

Compared pinned Zed
`crates/extension_host/src/extension_host.rs::{fetch_extensions,install_latest_extension}`,
`crates/extensions_ui/src/extensions_ui.rs::fetch_extensions`, and
`crates/cloud_api_types/src/extension.rs` with AHEAD's URL/ID install form in
`ahead-app/src/settings_panel.rs`. Zed's gallery requests its service's
`/extensions` list with `max_schema_version=1` and a `provides` filter; the
client installs a versioned archive through a download endpoint. A live
read-only probe of the public endpoint returned language-server metadata with
ID, version, Wasm API version, repository, and capabilities. AHEAD currently
accepts only Zed API 0.6/0.7/0.8 language-server Wasm packages, so a gallery
must not imply that all Zed marketplace categories work here.

The [public source index](https://github.com/zed-industries/extensions/blob/main/extensions.toml)
contains submodule paths and versions, not ready-to-install archives; Zed's
[publishing guide](https://github.com/zed-industries/zed/blob/main/docs/src/extensions/publishing/publishing-guide.md)
describes packaging after merge. Zed's published archives are served by its
own API and object store. Direct third-party use of that API is not documented
as supported; [Zed's terms](https://zed.dev/terms) restrict seeking access to
non-public APIs. At that point no production registry integration was
added pending the source/permission choice. The planned picker, offline
installed state, compatibility filter and disposable-package proof remain in
`TODO.md`; no Zed code was copied.

On 2026-10-02, AHEAD has a searchable gallery preview with Wasm API-version
filtering, offline installed-package visibility, and Install/Update/Retry states.
The live metadata response contained 412 language-server entries, of which 307
matched AHEAD's accepted Wasm API versions; this does not prove their servers
start in AHEAD, and the UI now calls them candidates rather than compatible.
It follows the pinned Zed gallery's list-then-versioned-download flow. A
disposable loopback test installed a real HTML 0.7 extension archive, and a
headless GPUI test checks catalog selection and the install request. This does
not resolve distribution: the current gallery and download URLs still target
Zed's service. The [public index](https://github.com/zed-industries/extensions/blob/main/extensions.toml)
is a source list, not a package feed, while [Zed's extension overview](https://zed.dev/blog/zed-decoded-extensions)
describes its own CI packaging and service upload. Before shipping live
installation, obtain explicit service permission or publish an AHEAD-owned
package feed from reviewed public extension sources; keep the compatible Wasm
ABI, package checks and installed-package UI independent of the feed choice.
No Zed source was copied for the gallery.

On 2026-10-02, AHEAD added All/Installed/Not Installed filters to the existing
Settings gallery, following the filter behavior in pinned Zed
`crates/extensions_ui/src/extensions_ui.rs`. At that point this was a UI
increment, not a dedicated Extensions page or a cleared distribution source. The proposed
AHEAD-owned feed would build versioned packages from reviewed public extension
repositories; Zed's public `extensions.toml` is only a source index.
Zed's pinned `ExtensionsPage` keeps search, installed-state filters, remote
results, and fetch errors in a dedicated workspace view; AHEAD now has a
separate Extensions page in Settings navigation; the next pass moved it to a
standalone center tab opened from the Command Palette and focused its search.
The gallery's catalog/install state, rendering and regressions now belong to
`ahead-app/src/extensions_panel.rs`, leaving Settings without a second gallery.
Like Zed's `ExtensionsPage::new`, AHEAD now begins the catalog fetch on first
open, while retaining locally installed entries if it cannot reach the feed;
manual Refresh still forces a later request. The focused headless GPUI
open/close test checks the loading transition and passes.
Search now follows pinned Zed `ExtensionsPage::fetch_extensions_debounced`:
nonempty queries wait 250 ms before sending `filter` to the catalog API, while
clearing the query reloads immediately. A request revision discards stale
responses, and the returned server matches are not re-filtered by a local
substring check; matching offline-installed packages remain visible. The
catalog URL encoding test and all 174 app library tests pass. The earlier
focused GPUI open/close and focus test and app binary check passed, while a
native navigation check remains blocked by the locked Mac. Its pinned
`ExtensionStore` fetches metadata and
downloads a selected version separately. AHEAD's install screen should keep
that separation while using a feed it is authorized to distribute from.
Zed's `ExtensionStore::upgrade_extensions` compares parsed semantic versions;
AHEAD now does the same before presenting or sending an Update request. A stale
catalog cannot offer a downgrade, and the row shows the installed version
when it is newer than the catalog. The catalog parser rejects non-semantic
remote versions. This is AHEAD code informed by Zed, not a source copy.

## Retained terminal grant prune (2026-10-02)

The retained runtime's terminal launch had a second, internal permission
profile for a copied plugin-metrics write grant. AHEAD's sole production
caller always passed `None`; only a copied test exercised the grant. The
internal profile, its approval text, and the copied fixture are removed.
Normal `additional_permissions` still flow through the same launch and
stdin-review policy; `merge_permission_profiles(grants, None)` was exactly a
clone of `grants`. The locked offline app check, repository format check, and
131 AHEAD agent library tests pass (two ignored) with loopback access. The
copied core test target remains outside this validation.

## Agent composer Enter handling (2026-10-01)

Zed's pinned `assets/keymaps/default-macos.json` binds `enter` to
`agent::Chat` only in its thread editor when modifier-to-send is off; the
editor's newline action remains separate. AHEAD's textarea instead emitted
`PressEnter` and let the plain key event continue, so an unsent draft gained a
newline even after the send handler ran. `ahead-app/src/session_panel.rs` now
consumes plain Enter in its focused-composer interceptor while leaving
Shift+Enter with the textarea. One rendered composer test and four adjacent
slash-palette tests pass in the locked offline app suite. A native-app keyboard
pass is still pending; no Zed code was copied.

## Managed provider-layer safety (2026-10-02)

Zed's `../zed/crates/agent/src/thread.rs::set_model` changes the selected
model on an existing thread. AHEAD's retained managed runtime fixes the
provider at thread spawn, so AHEAD instead replaces an idle harness when its
connection changes and reloads the same durable Turso thread before the next
turn. A two-endpoint local Responses regression checks the new endpoint and
credential, model-visible prior reply, and persisted conversation. Active
managed turns block the swap; the native Settings-to-next-turn journey remains
to be verified.

Compared pinned Zed `crates/settings/src/settings_store.rs`, which keeps user
and local settings layers separate and tracks file errors, with AHEAD's
provider-layer readers. AHEAD's managed runtime was the outlier: it followed
paths directly and silently skipped malformed layers. It now uses the same
bounded `ahead_core::config::read_ahead_config` path (no-follow on Unix) as the editor
and proxy, and errors propagate instead of selecting an unintended fallback.
That shared reader rejects credential-named fields anywhere in tracked
`.ahead/config.toml`, plus URL userinfo and credential-named query parameters;
ignored settings layers still accept credentials. The
managed agent and FIM now inherit a private key only when the endpoint stays
exactly the same; switching a tracked layer to another endpoint drops the key.
The 35-test core suite, focused managed-layer/precedence tests, both endpoint
switch regressions and two Settings/model-picker tests pass offline.
Other disguised secret values and native-app error presentation remain open
checks; no Zed code was copied.

Zed's `crates/agent/src/agent.rs::run_skills_scan` treats an absent global
skills directory as a normal first-run state and retries discovery later.
In a disposable AHEAD project, an absent `AHEAD_USER_HOME` instead caused
the shared native config build to fail before the project skill catalog
loaded. `runtime_config_from_sources` now omits only that absent optional
user settings root; other filesystem errors and dangling links still fail
closed. Its regression and the full 145-test active agent library suite pass
offline. A rebuilt native first-open check remains pending because macOS is
locked; this is source/test evidence, not a rendered-app claim.

The next AHEAD language-extension first-open gate is catalog responsiveness.
`PluginCatalog::handle_did_open_text_document` calls `ensure_lsp_server`
synchronously; an installed extension's command resolution uses a current-
thread runtime. Before the child-wait change, it could reach a blocking npm
`.output()` in `ahead-extension-host/src/host.rs`. Zed instead awaits
`crates/extension_host/src/wasm_host/wit/since_v0_8_0.rs` through its async
`crates/node_runtime/src/node_runtime.rs::npm_install_packages` path. AHEAD
now bounds its permitted process and npm children with Tokio async output,
a 120-second deadline and `kill_on_drop`; a hung-child regression and all 21
active extension-host library tests pass. It still needs an off-catalog,
cancellable acquisition with stale-result rejection and unsaved-document
replay; the current synchronous route has not been shown responsive in a
native first-open run.

Normal managed-MCP declaration and opt-in reads now use that same shared
reader instead of a second path/size/symlink implementation. The Unix
approval transaction still uses its directory-handle reader to keep the
reviewed declaration and private settings under one lock. That reader now
calls the shared tracked-credential validator too; a focused test verifies
both listing and direct approval reject a tracked secret. Four focused
workspace-MCP tests and the approval/private-settings regression pass; the
separate running-app MCP approval journey is still unverified. Managed
provider settings now enter the retained config builder as in-memory session
overrides, while its runtime-home user config layer is ignored; AHEAD no
longer writes provider credentials to `.ahead/runtime/config.toml`. A focused
regression proves an invalid stale runtime config cannot override the AHEAD
model, endpoint or key, and all 131 active agent library tests pass with
loopback access. The managed AHEAD host now explicitly skips retained managed
file/MDM config, system and managed requirements, and user/project execution-
policy rules, rather than letting those layers outrank its in-memory settings.
The retained config suite passes 247 tests, including an invalid managed-file
and macOS-preferences skip regression; AHEAD's own permission profile tests
remain green. Unix default runtime-home creation still rejects symlinked
parents, but the retained runtime addresses other state there by pathname;
that residual race remains in `TODO.md`. The locked/offline `ahead --bin ahead` check and repository
format check pass after this change. The Mac remained locked for the native
GPUI pass.

- 2026-10-02: compared the pinned Zed extension-host catalog/download flow in
  `crates/extension_host/src/extension_host.rs` and gallery in
  `crates/extensions_ui/src/extensions_ui.rs` with AHEAD's single Extensions
  tab. AHEAD now forwards the selected package version and an optional archive
  SHA-256 through its install RPC; the host rejects a mismatched manifest
  version or digest before replacing an installed package. The extension-host
  suite passed 17 tests (one opt-in test ignored), all seven filtered app
  extension tests passed, the proxy checked, and the RPC suite passed 15 tests.
  In a disposable native project, Cmd+Shift+X opened the tab, Load catalog
  displayed 308 live candidates, and searching `HTML` narrowed the list to
  two. No package was installed; a real catalog-selected install and an
  AHEAD-owned publisher/feed remain open. `just dev` built the bundle but its
  first launch exited with signal 6 after a macOS connection error; opening
  the rebuilt bundle by path worked. No Zed source was copied.
  A later elevated `just dev` run in another disposable project captured the
  Extensions tab auto-populated with live Zed candidates after launch. It did
  not install a package; the native UI tool listed AHEAD Dev but could not
  attach to its window.

## MCP form defaults and titled choices (2026-10-02)

Zed initializes typed field state from schema defaults in
`../zed/crates/agent_ui/src/conversation_view/elicitation.rs::ElicitationFormState::new`.
AHEAD still renders generic chat questions, but now carries only defaults that
pass its existing form response validator into that card and preselects them
when a new request arrives. Invalid or unrepresentable defaults stay blank;
an edited answer is not overwritten by a refresh of the same request. The
focused form and headless GPUI card tests pass. Zed's `single_select_options`
and `multi_select_options` keep a choice's submitted value separate from its
display title and description. AHEAD's question DTO now does the same for
titled MCP choices; the panel submits the value, and an opaque omission value
keeps optional `Skip` distinct from a literal answer of `Skip`. Pure form,
stdio MCP and headless GPUI regressions pass. A native `just dev` check in a
disposable project did not reach a window because the watcher kept restarting
during the build. At that point, dedicated typed controls and URL
consent/navigation remained open in `TODO.md`; no Zed code was copied.

## MCP URL elicitation review (2026-10-02)

Zed's URL card in
`../zed/crates/agent_ui/src/conversation_view/elicitation.rs` shows the
requesting server, destination host, full URL, and separate Open, Decline and
Cancel controls. AHEAD now projects retained managed-runtime URL elicitation
events into a similarly explicit chat card. It accepts only bounded HTTPS
URLs without embedded credentials, validates the pending request again before
opening, and sends a URL decision without collecting website secrets in chat.
The retained MCP client already has form/URL capability and 2026 MRTR tests;
the AHEAD managed config now advertises both modes. Focused URL validation,
managed-answer and headless GPUI click tests pass. A disposable stdio server
roundtrip also passed for both form and URL modes: the URL trace received
`accept` with empty content. The rebuilt native browser journey remains open
in `TODO.md`; an elevated disposable `just dev` run initialized the project,
but the UI tool could not attach to its window, so no URL-card click was
observed. The headless test and local mock are not live UI evidence. No
Zed code was copied.

## Built-in language-server acquisition boundary (2026-10-02)

Zed's `crates/project/src/lsp_store.rs::get_language_server_binary` can wait
for worktree trust before resolving and starting a local language-server
binary. Its BasedPyright and Vtsls adapters check existing binaries and use
cached npm packages before downloading. AHEAD still launches its built-in
Rust Analyzer, BasedPyright, and Vtsls commands from PATH, and it has no
worktree-trust gate. When the BasedPyright or Vtsls executable is absent from
PATH, AHEAD can now start a complete package from its own
`cache/language-servers/` directory using system Node. It does not execute an
opened project's `node_modules`. Managed acquisition still needs to populate
that cache and restart through the dispatcher so current unsaved buffer
snapshots are replayed. Project-local package execution needs a separate
trust decision and UI. The exact executable-launch failure is visible in the
Language Servers panel; package acquisition remains open in `TODO.md`. No Zed
source was copied. The cached BasedPyright package was copied into an isolated
disposable AHEAD profile and the freshly rebuilt native app showed its server
as Ready. On a cache miss, AHEAD now runs npm on a background thread with
install scripts disabled, checks the staged entrypoint, publishes the package
under its own cache, and asks the dispatcher to replay the current open-buffer
snapshots. A fake-npm install/failure test passes. The clean-profile native
test used an offline fake npm to copy local Zed packages into the staging
directory: both BasedPyright and Vtsls reached Ready after automatic restarts.
That proved AHEAD's orchestration without a real registry download or
package-integrity policy.
A second fresh native profile on 2026-10-02 used separate empty user/global
npm config files and the public registry. It downloaded BasedPyright 1.40.1
and Vtsls 0.3.0 into AHEAD's cache; both reached Ready after automatic
restart. Switching from Python to JavaScript left only Vtsls in the panel.
Updates, a bundled Node/npm runtime, and package-integrity policy remain open.
Following Zed's installed-version check, AHEAD now reads a bounded package
manifest and requires the expected name, a nonempty version, and a regular
entrypoint before using or publishing a cached built-in server. A per-package
file lock serializes publication across editor windows; a concurrent fake-npm
test passes. A malformed staged install preserves the previous valid cache in
the focused regression. This rejects malformed caches, not a package that
starts and then fails at runtime. The rebuilt disposable macOS app reused both
real public-npm packages: BasedPyright and Vtsls rendered Ready after opening
Python and JavaScript respectively (the latter via Quick Open).
Zed runs npm through its async Node runtime. AHEAD's retained system-npm path
now polls its child with a 120-second total deadline and signals cancellation
on proxy shutdown, waiting briefly for the installer to stop. A fake-npm
shutdown regression verifies that an active child exits before publication;
the rebuilt disposable macOS app also quit with fake npm active, leaving no
child process or cache entry. Windows still needs a live pass.
The first run also exposed stale health rows after restart. Restart now clears
old server statuses before replay, while the toolbar and panel distinguish a
server still starting from one that failed. Focused proxy and app regressions
pass. In the rebuilt disposable app, switching from Python to JavaScript left
only Vtsls Ready in the panel and the toolbar also reported ready.
Zed's `LanguageServerStatus` belongs to a real server id; AHEAD now likewise
routes installed-extension discovery failures separately from language-server
health. The panel shows them as an unboxed "Extension issues" list, and each
discovery pass replaces the list so resolved failures clear. Focused proxy and
app regressions passed. In a rebuilt macOS app with isolated data and project,
one malformed extension appeared below a real BasedPyright Ready card; hiding
that package and choosing Restart removed the warning while BasedPyright
remained Ready. The older "Managed by ahead-proxy" row is absent from current
source and did not recur. Git history traced it to a hard-coded workspace/proxy
note in the previous `LanguageServersPanel`, rendered with the same card
background as real servers despite carrying no server-health state. Its removal
needs no special-case status filter.

Zed's extension `worktree.which` calls use executable names; AHEAD's host now
rejects path-shaped names and uses the shared executable-aware lookup rather
than treating every regular PATH file as launchable. Zed's pinned
`project/src/lsp_store.rs` waits for a trusted worktree before resolving and
starting binaries, and `workspace/src/security_modal.rs` offers an explicit
Restricted Mode choice. AHEAD now keeps a user-level per-workspace trust list
outside the project; the Language Servers panel grants and revokes it. The
catalog checks before invoking an extension or starting an npm install, and
`LspClient::process` checks again before spawn. Two core store tests and a
proxy untrusted-launch regression pass; proxy and app library checks pass.
This is source/protocol evidence, not a rebuilt native trust journey. AHEAD's
restricted coverage is still narrower than Zed's: project settings, MCP
startup, other project-driven processes, and a first-open prompt remain open.
Windows ACL/reparse safety and native Linux behavior are also unverified.
