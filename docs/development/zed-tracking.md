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
  in the disposable project, but the native click result was inconclusive:
  multiple AHEAD windows shared one app identity and two `just dev` bundle
  launches aborted after a macOS LaunchServices error. The isolated native
  click/hover check is tracked in `TODO.md`.

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
  AHEAD's remaining parity gap is reevaluating that guest callback and sending
  `workspace/didChangeConfiguration` after settings changes, rather than keeping
  only the startup snapshot. The regression and implementation gate are tracked
  in `TODO.md`.
- 2026-09-23: search reference review at the pinned checkout. Zed's
  `crates/project/src/{search,project_search}.rs` separates path candidates,
  match detection and open-buffer range search, with bounded workers and
  ordered streaming results. `crates/search/src/project_search.rs` owns the
  query UI; `crates/agent/src/tools/grep_tool.rs` calls the same project search
  and offers an include glob and 20-result pages. `crates/file_finder/` uses a
  separate ranked path picker. AHEAD's shared `ahead-core::search` matcher is
  independent code, not an imported Zed file. It now accepts compiled
  include/exclude globs for panel content search and an include glob for
  native-agent search. The panel caches a proxy-owned path snapshot across
  queries, and simultaneous cold callers share its initial walk, but AHEAD
  still scans candidate file contents serially and lacks Zed's incremental
  worktree index, candidate pipeline and full query controls. AHEAD's `Cmd+P`
  modal now consumes its shared path ranker and generation-tagged proxy
  snapshot; rendered picker behavior remains unverified. See the search item
  in `TODO.md`;
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
