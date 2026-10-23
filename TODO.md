# TODO

Incomplete work for AHEAD: unfinished features, stubs, mocks, and known gaps.
This is the single source of truth. When you finish an item and commit it,
delete it here in the same commit. Do not add a "done" section.

Anything knowingly shipped as a stub or mock must have an entry here in the
commit that introduces it. This workflow lasts until the editor is usable end
to end; after that we dogfood AHEAD to develop AHEAD and the backlog moves into
the app itself.

## Usable-editor goal (active)

Goal: AHEAD is broadly usable as an editor and integrated managed agent, with
`../zed` as the behavior reference. Acceptance is evidence-based: workspace
compile, maintained suites, disposable Rust/TypeScript/Python projects,
managed-agent smokes, then native rendered behavior.

- [x] 2026-10-23 `cargo check --locked --offline -p codex-core -p ahead-agent`:
      0 errors.
- [x] 2026-10-23 `cargo build -p ahead -p ahead-code-mode-host --bins` (cold
      10m link); `target/debug/ahead` and the Code Mode host are present.
- [x] 2026-10-23 `cargo test --locked --offline -p ahead-agent --lib`:
      182 passed, 4 ignored. Fixed four test-compile breaks: the cfg
      `(not(unix), test)` private-config writer was missing `Write`, a redundant
      `Read` import tripped `-D redundant-imports`, the curated ACP test read a
      removed `entry.npx` field instead of `RegistryIndex.agents[].distribution`,
      and it named `codex_rmcp_client` for `McpProtocolMode` (now `codex_mcp`,
      added as a dev-dependency).
- [x] 2026-10-23 offline smokes against that binary: editor recovery, managed
      Code Mode, memory, shutdown (explicit, stdin EOF, SIGKILL), MCP,
      workspace search scale (10k files), DAP shutdown, LSP shutdown.
- [x] 2026-10-23 Rust LSP with rust-analyzer 1.98.1: formatting, completion,
      definition, semantic tokens, hover, signature help, references,
      cross-file rename, unsaved diagnostics/repair, restart recovery and
      graceful shutdown. `tests/lsp-smoke.mjs` now sends the
      `SignatureHelpContext` the protocol requires; without it the proxy drops
      the notification and the client waits forever.
- [x] 2026-10-23 TypeScript, JavaScript and Python LSP with
      `AHEAD_LSP_INSTALL_MISSING=1`: managed acquisition installed Vtsls,
      Basedpyright and Ruff into a disposable home, and the full matrix passed,
      including restart recovery and graceful shutdown.
- [x] 2026-10-23 `agent-file-attribution-smoke`: shell-write denial, a Code Mode
      cell edit and a standard function-schema `apply_patch` each produce Turso
      CodeAnchors and staged commits. The smoke runs the Code Mode turn on the
      catalog model and the direct turn on a provider-model connection because
      AHEAD selects the tool mode from model metadata.

Open: native rendered acceptance (macOS HIServices abort before render), the
`ahead-app`/`ahead-proxy` suites after the latest edits, and clippy.

## Zed source reuse and editor parity

- [ ] Finish unsupported-file tab verification (`ahead-app/src/code_panel.rs`).
      In the 2026-10-02 rebuilt, uniquely identified native app, disposable
      SQLite and UTF-16 files showed the unavailable-text explanation; a
      chmod-denied file showed the distinct permission error. The UTF-16 bytes
      remained unchanged after opening. A UTF-8 control and an empty file
      both accepted edits and saved to disk. An immutable SQLite copy retained
      its SHA-256 after an attempted edit and Cmd+S on the unavailable-text
      card. Pinned Zed preserves a buffer's encoding and BOM through save;
      AHEAD now follows that behavior for strict BOM-marked UTF-16LE/BE using
      shared decode/encode logic and proxy-buffer encoding metadata. Malformed
      UTF-16 stays rejected. Core codec, proxy byte-round-trip, editor file-open
      and managed-agent source regressions pass on 2026-10-03. Verify opening
      UTF-16 through a reused preview tab in the native app; the headless GPUI
      file-open test already replaces UTF-8 with UTF-16 in the same preview
      panel and verifies editable content reaches the proxy snapshot. Keep
      binary data safe. On 2026-10-07, reran
      `code_panel::tests::file_open_recovery_preserves_missing_text_without_unlocking_binary_files`
      directly from the app test binary; it passed, and the binary was newer
      than `ahead-app/src/code_panel.rs`. Later that day, the source-current app
      test binary (19:36, newer than `code_panel.rs` at 18:14) passed all 54
      `code_panel::tests`, including file-open recovery, diff preview, gutter,
      blame, completion, navigation and formatting regressions. No Cargo build
      was started. Native rendered verification remains open while the desktop
      is locked. On 2026-10-09, the source-current `ahead_app` test runner passed
      all 61 `code_panel::tests`. A fresh `just dev` build then aborted before
      drawing with macOS `hiservices-xpcservice` connection errors; `open -a`
      also reported `kLSNoExecutableErr` although `Contents/MacOS/ahead` exists.
      Native checks remain pending host LaunchServices recovery. Rechecked on
      2026-10-12: the existing test runner (newer than `code_panel.rs`) passed
      the focused labeled Add Comment GPUI test (1/1). After `cargo clean`,
      `just dev` completed the clean build, then
      the app again aborted with `hiservices-xpcservice` connection errors
      (signal 6). At that earlier CUA check the Mac was locked, so rendered
      verification was unavailable.
      On 2026-10-13, widened the inline Add Comment target from 18px to 22px
      within the existing 26px gutter; the source-current focused GPUI action
      test passed. Native appearance and pointer behavior remain unverified.
      Follow-up at 11:11 ET on 2026-10-13: changed the inline action's icon
      foreground from the dark editor background to the bright sidebar text
      color for contrast against the magenta button. The source-current GPUI
      interaction test passed (1/1), and the integrated app rebuilt in 11.41s.
      GUI startup again aborted before drawing with the HIServices XPC signal 6;
      button contrast still needs a visible-window check.
      Follow-up at 13:46 ET: increased the inline Add Comment button to 24px
      with an 18px icon, within the unchanged 26px gutter. The focused GPUI
      interaction test passed (1/1), and the integrated app rebuilt; native
      startup again aborted in HIServices before rendering. The watcher was
      restarted on the disposable multi-language project and remains active.
      A source-current rerun then caught that the 24px inline control exceeded
      the gutter's measured bounds. Keep the 20px hit target and 26px gutter,
      but enlarge the icon from 16px to 18px; the focused GPUI interaction and
      gutter-boundary test now passes (1/1). The integrated app rebuilds, but
      native appearance remains unverified because macOS aborts before render.
      The 2026-10-13 source-current app test binary (17:49, newer than
      `code_panel.rs` at 17:48) also passed all 61 `code_panel::tests`,
      including the inline comment action and gutter-boundary regression.
      Rechecked on 2026-10-14 with app test binary
      `ahead_app-355518c791e0c7cf` (18:09, newer than `code_panel.rs` at
      17:28): the no-session Add Comment gate, completion-row signature/type
      detail, and signature-help popup with parameter documentation each pass
      (1/1). These headless checks do not verify native appearance; macOS still
      aborts before AHEAD renders.
      At 20:49 ET on 2026-10-14, reran the no-session comment gate, completion
      signature/detail row, and above-caret signature-help tests from
      `ahead_app-0e8e9c4151d65c97` (00:09 UTC, newer than `code_panel.rs` at
      00:08 UTC); all three passed (1/1 each). Rendered macOS acceptance remains
      pending.
      Follow-up at 23:38 EDT: switching the editor's active comment session
      now invalidates pending UI state and advances a request generation, so a
      late save/resolve cannot affect a later return to the same session ID.
      Starting a new comment clears any hidden draft from the prior session.
      The source-current serial app suite passed all 333 tests on 2026-10-15,
      including the session-switch reset/generation and draft-isolation
      assertions in `code_comment_actions_require_a_session_and_fit_the_gutter`.
      Native rendered behavior remains open.
      The fresh macOS report at
      `~/Library/Logs/DiagnosticReports/ahead-2026-10-13-111131.ips` locates
      the abort in AppKit/LaunchServices app registration while GPUI creates
      `NSApplication` (`_RegisterApplication` → `NSApplication` init →
      `MacPlatform::ensure_system_power_observers` → `App::new_app`), before
      AHEAD reaches rendering. No AHEAD-owned frame or safe app-side workaround
      is evident; resolve host LaunchServices startup before claiming rendered
      acceptance.
      Restarted `just dev` on 2026-10-13 with a fresh copy of the disposable
      multi-language fixture; the integrated app rebuilt in 9.63s, then aborted
      before drawing with the same `hiservices-xpcservice` connection failure
      (signal 6). The watcher remains active; rendered acceptance is still open.
      Follow-up: the app inventory reports bundle `io.ahead.dev` running, but
      repeated attempts to attach to its window time out; no dev terminal is
      attached to this task. The documented per-repository `just dev` lock path
      was absent at that earlier check, so there was no lock evidence of an
      active watcher then. Disk space has since recovered; see the latest
      follow-up below.
      Resumed on 2026-10-12 with a source-current app test runner and 45 GiB
      free. The focused Add Comment and model-list wheel-isolation GPUI tests
      both passed (1/1 each); an earlier Add Comment attempt had stopped before
      assertions when its temp project hit `ENOSPC`. `just dev` completed the
      cold integrated build in 10m06s, but launch again aborted with
      `hiservices-xpcservice` connection errors (signal 6). Its warm build was
      2.66s. Renaming the unused `SessionPanel::render` parameter removed its
      compiler warning; the watcher rebuilt that change in 10.73s without an
      AHEAD-owned unused-variable warning. The linker compact-unwind warning
      remains. No new AHEAD crash report was written; unified-log inspection is
      denied in this sandbox. The app still does not render. The shared
      `just dev` loop (exec session 97433) remained active at that point and held
      the build lock despite repeated app-child signal-6 exits. The existing app
      test runner passed 61 `code_panel::tests`, 54 `session_panel::tests`,
      29 `settings_panel::tests`, and the narrow-width dirty-tab regression
      (1/1). After removing legacy
      rollout import, locked/offline checks of the `ahead-agent` and `ahead-proxy`
      libraries and test targets passed with zero errors. The Turso replay
      round-trip and managed child-restore-after-restart tests passed (1/1
      each); the synthetic-provider resume test could not bind its local socket
      (`Operation not permitted`). `cargo machete`, formatting and whitespace
      checks pass. These headless checks do not replace native rendered
      verification. On 2026-10-12, stopped old watcher 97433 and restarted
      `just dev` against the disposable
      `/private/tmp/ahead-broad-smoke.im0WJM/project` with isolated
      `AHEAD_DATA_HOME=/private/tmp/ahead-dev-data.mdBJi2`. After the one-off
      fixture Cargo test, the current watcher is 88391; its integrated build
      passed in 12.53s, then the app child again aborted with a
      `hiservices-xpcservice` connection error (signal 6). CUA lists
      `io.ahead.dev` as running, but window attachment times out. Keep this
      isolated watcher; native UI acceptance remains blocked on host
      app/LaunchServices recovery.
      Latest restart on 2026-10-12: stopped session 99362 after its app child
      exited, then restarted the same `just dev` loop (session 11569) against
      the same disposable project and isolated data home. It waited for the
      shared Cargo build lock, then the integrated build passed in 30.21s,
      including the ACP install button's agent-specific accessibility labels.
      GUI startup again aborted with the HIServices XPC error (signal 6). The
      latest available macOS crash report (2026-10-08) shows SIGABRT during
      `NSApplication` registration through GPUI's
      `MacPlatform::ensure_system_power_observers`, before AHEAD begins
      rendering; it predates this restart and no new report was written. The
      watcher remains live; no additional Cargo process was started. Native
      rendered acceptance remains open.
      Latest follow-up on 2026-10-12: the rebuilt app test runner passed the
      integrated empty-agent-panel interaction test, clicking new AHEAD,
      external agent and shared-session join actions. The same source-current
      app runner passed the session panel (54), settings (29), editor (61),
      terminal (10), terminal panel (14), workspace panels (15), and extension
      gallery (10) test groups. These cover the reported model search/provider
      visibility, command palette, terminal copy/paste, completion placement,
      gutter interactions, and search-mode behavior. In Turso storage,
      `init_schema` now reruns one transactional set of idempotent initial DDL
      statements (`CREATE ... IF NOT EXISTS`) without inspecting schema tables;
      all seven focused `session_storage_` tests pass, including preservation
      of preexisting rows and creation of initial AHEAD tables. No versioning,
      migration, validation or compatibility path was added. The latest
      `just dev` rebuild completed in 2.24s, then the app again aborted before
      rendering on the HIServices XPC error. The follow-up LSP watcher tests
      pass when filesystem events are injected into the handler, but a direct
      raw `notify` probe received no OS callbacks in this test process. Native
      watcher delivery and rendered UI acceptance remain open. The restarted
      dev loop is still watching the same disposable project for source changes.
      Rechecked on 2026-10-13 with source-current test binary
      `ahead_app-e1173e43572818d7` (built 17:49, newer than the tested app
      sources): 194 tests passed across the editor (61), session/chat (54),
      settings (29), terminal emulator (10), terminal panel (14), workspace
      panels (15), and extension gallery (11). These are headless GPUI/unit
      checks; this host still cannot render the native app while locked.
      On 2026-10-13, strengthened the empty-session button regression to mount
      `AgentWorkspacePanel` in the actual `Shell` right-side `DockArea`; the
      headless GPUI test clicks New AHEAD, External agent and shared-session
      join/validation through that composition and passes (1/1, 322 filtered).
      Restarted the shared `just dev` loop on the same disposable project and
      isolated Turso home as session 28171; the integrated build passes in
      2.80s. The app then aborted before rendering with the HIServices XPC
      connection-invalid error (signal 6, exit 134). CUA listed AHEAD as
      running but could not attach to its window (-10005); the `just dev`
      watcher remains live on session 28171 after the app child exited, so it
      is still watching for edits. Native rendered/button input acceptance
      remains open.
      Rechecked on 2026-10-14 with `ahead_app-e1173e43572818d7`, built at
      10:39 and newer than `session_panel.rs` and `app_lifecycle_tests.rs`: all
      54 session-panel tests passed, and the integrated Shell test
      `empty_agent_panel_routes_ahead_and_external_start_actions` passed. It
      mounts the actual right dock at 640px, checks the buttons remain ordered
      and distinct, then clicks AHEAD, external-agent and shared-session
      actions. CUA currently reports Ahead is not running while shared watcher
      7363 remains live; rendered native acceptance is still open.
      On 2026-10-13, stopped session 28171 for the isolated ACP session/model
      option probe, then restarted `just dev` on the same disposable project
      with fresh `AHEAD_DATA_HOME=/private/tmp/ahead-dev-data.20261014.TgYMfc`.
      The integrated build passed in 16.73s; macOS again aborted the app before
      rendering at HIServices XPC initialization (signal 6). Watcher session
      66359 remains live on the disposable project. Native UI acceptance is
      still unavailable on this host.
      After adding the synthetic Codex ACP auth handshake probe, stopped
      session 66359 for the focused test and restarted the same project/data
      home as watcher session 76859. Its integrated build passed in 16.75s;
      macOS again aborted before rendering with the HIServices XPC signal 6.
      Session 76859 remains live, watching the disposable project.
      On 2026-10-13, stopped 76859 for the focused native filesystem-watcher
      tests; both the filtered watcher event and workspace `.ahead/config.toml`
      to catalog-refresh route passed (1/1 each) with local filesystem-event
      access. Restarted the same `just dev` project and Turso home as session
      76271; its integrated build passed in 9.48s, then macOS again aborted
      before rendering at HIServices initialization (signal 6). Watcher 76271
      remains live on the disposable project.
      Rechecked the terminal clipboard-to-PTY path on 2026-10-13 with the
      source-matched app runner built at 12:57 (newer than
      `terminal_panel.rs` at 2026-10-12 21:12): all 21 terminal-panel tests
      passed, including keyboard and context-menu paste/copy, exactly-once
      shell execution, shell-safe path paste, OSC 52 clipboard output, Ctrl-C,
      mouse reporting, and Zed shortcut behavior. This remains headless GPUI
      evidence; native OS pasteboard and rendered menu behavior are unverified.
      Recheck on 2026-10-14 exposed flaky PTY startup in this test group: GPUI's
      virtual timer could exhaust a polling loop before the native shell child
      was scheduled. A test-only wait now advances the GPUI timer and gives the
      OS process 10ms of real scheduling time. The rebuilt clipboard group
      passed 7/7 both serially and with the default parallel test runner; this
      changes test synchronization only, not production terminal behavior.
      On 2026-10-12, `tests/editor-recovery-smoke.mjs` passed against the
      shared loop's `target/debug/ahead`: an acknowledged Unicode unsaved buffer
      recovered after proxy SIGKILL without touching the disk file; a live
      owner could not be stolen; an explicit clear fenced stale revisions; and
      the isolated Turso database remained owner-only after reopen. No model,
      language server, provider credentials or real project was used. This is
      headless recovery-protocol evidence, not rendered recovery UI; the host
      Mac remains locked and AHEAD still aborts before rendering. The same
      current binary also passed `tests/dap-shutdown-smoke.mjs` in running,
      initializing and stderr-backpressure modes; each disposable adapter was
      reaped before proxy exit.
      Latest shared-loop restart on 2026-10-12 used the explicit disposable
      project path `/private/tmp/ahead-broad-use.0XxogT/project`; the integrated
      build passed in 6.16s after focused checks, then macOS again aborted the
      app child before rendering with the HIServices XPC error (signal 6) at
      18:17 ET. The same `just dev` watcher remains live on that fixture (exec
      session 85599); CUA confirms the Mac is locked. The previous watcher was
      stopped before the focused checks. The repository itself was not opened
      as a workspace. After the latest focused core-search run, restarted the
      same disposable-project loop as exec session 47087; the app rebuilt in
      3.46s, then macOS again aborted GUI startup with the HIServices XPC error
      (signal 6). Stopped that watcher to rebuild the current app test runner
      and run `workspace_panels::tests`; then restarted the same project and
      isolated data home as exec session 41521. Its integrated build passed in
      13.06s, but the app child again aborted before rendering with the
      HIServices XPC error (signal 6). The watcher remains live for edits; CUA
      again reports the Mac locked, so native rendering is unavailable.
      Latest restart on 2026-10-12: rebuilt and ran the source-current
      `ahead-agent --lib` suite; 164 passed, 4 opt-in integration tests ignored.
      Restarted `just dev` on the same disposable project and isolated Turso
      home; the integrated build passed in 9.69s, then macOS aborted GUI startup
      before rendering with the same HIServices XPC signal 6. The AHEAD child
      exited, but the `just dev` watcher session 20920 remained alive. After the
      focused test released the shared Cargo lock, that watcher rebuilt the
      current app in 3m17s (including lock wait); GUI startup hit the same
      HIServices failure and the watcher stayed alive. CUA confirms the Mac is
      locked. Pre-production storage remains one transactional initial schema
      with idempotent DDL; schema checks and any migration/version framework
      are deferred until production readiness.
      Stopped watcher 20920 for the focused Pi ACP test build, then restarted
      the same `just dev` loop against `/private/tmp/ahead-broad-use.0XxogT/project`
      with `AHEAD_DATA_HOME=/private/tmp/ahead-dev-data.mdBJi2`. Its current
      watcher is session 10912; the integrated build passed in 3.51s, then the
      app child again aborted before rendering with the HIServices XPC signal 6.
      The watcher remains live for subsequent edits.
      Latest Pi prerequisite follow-up: stopped watcher 10912 for the focused
      catalog/GPUI test pair (2/2 passed), then restarted the same project and
      data home as watcher session 82228. The app rebuilt in 3.08s; macOS again
      aborted before rendering with the HIServices XPC signal 6. Session 82228
      remains live; native rendering remains unavailable while the Mac is locked.
      Latest recheck on 2026-10-12: the source-current focused PTY Ctrl-C test
      and full serial app suite (320/320, excluding the two sandbox-blocked
      collaboration tests) pass after extending the external-process startup
      deadline and adding terminal-output diagnostics. Scoped rustfmt and diff
      checks pass. Restarted the shared `just dev` loop as session 91368 against
      `/private/tmp/ahead-broad-smoke.im0WJM/project` with isolated
      `AHEAD_DATA_HOME=/private/tmp/ahead-dev-data.mdBJi2`; its integrated build
      passed in 4.09s, but macOS again aborted the app before rendering with the
      HIServices XPC error (signal 6). The watcher remains live; CUA confirms
      the Mac is locked.
      On 2026-10-13, restarted the same shared loop as session 26174 against
      `/private/tmp/ahead-dev-smoke.XPWP4f/project` and its isolated sibling
      data home. The integrated build passed in 2.84s, then the app again
      aborted before rendering with the HIServices XPC connection error
      (signal 6). CUA lists `io.ahead.dev` as running, but attaching to its
      window timed out. The watcher remains live; native Add Comment rendering
      is still unverified. Retried with the documented native-app access path:
      the integrated build passed in 5.64s, the proxy entered its mainloop, and
      no HIServices abort was reported. CUA still lists the app as running but
      cannot attach to its window; watcher session 21454 remains live, so
      rendered verification is pending.
      On 2026-10-13, outlined the inline Add Comment control in the gutter with
      the comment accent while retaining its 22px hit target. The active shared
      `just dev` loop rebuilt the integrated app successfully in 14.36s; macOS
      then aborted the app before rendering with the HIServices XPC error
      (signal 6). CUA still times out attaching to AHEAD Dev, so the visual
      change is compile-checked but not yet verified on screen.
      The source-current `empty_agent_panel_routes_ahead_and_external_start_actions`
      GPUI test now resizes the agent panel to a 396px chat column, verifies the
      AHEAD, external-agent, and join buttons remain in bounds and vertically
      separated, and exercises their action transitions; it passed 1/1.
      Restarted the shared loop on the same disposable project with isolated
      AHEAD data at `/private/tmp/ahead-dev-smoke.7T4vXa/data`; the integrated
      build passed in 9.26s, then the app again aborted before rendering with
      the HIServices XPC error (signal 6). Watcher session 34299 remains live.
      On 2026-10-13, rebuilt the current app test binary; the focused
      `code_comment_action_is_visible_in_the_gutter_and_toolbar` and
      `file_open_recovery_preserves_missing_text_without_unlocking_binary_files`
      GPUI regressions each passed (1/1). Native contrast and unsupported-file
      rendering remain unverified while the Mac is locked.
      Restarted the shared `just dev` loop against the existing disposable
      multi-language fixture at `/private/tmp/ahead-dev.fHD6JH/project` with
      isolated `AHEAD_DATA_HOME=/private/tmp/ahead-dev.fHD6JH/data`. The
      integrated build passed in 2.61s; macOS then aborted the app before
      rendering with the HIServices XPC signal 6. The watcher remains live on
      the disposable fixture; rendered UI acceptance is still pending.
      After stopping the shared loop for focused tests, restarted it on the same
      disposable project `/private/tmp/ahead-dev.fHD6JH/project` with isolated
      Turso home `/private/tmp/ahead-dev-data.20261014.TgYMfc` as session 85646.
      The integrated build passed in 1.58s; macOS again aborted the app child
      before rendering with the HIServices XPC signal 6. The watcher remains
      live; native UI acceptance is unavailable while the Mac is locked.
      After adding the shared-pool concurrency regression, stopped session
      85646 for focused `ahead-core` tests (41/41 search tests passed), then
      restarted `just dev` with the same disposable project and Turso home as
      session 58936. The integrated build passed in 2.08s; the app again aborted
      before rendering with the HIServices XPC signal 6. Watcher 58936 remains
      live; native UI acceptance is still blocked on host startup/unlock.
      Then stopped session 58936 for the focused concurrent managed
      `file_search` regression (1/1 passed), and restarted the same loop as
      session 22039. Its integrated build passed in 11.47s; macOS again aborted
      before rendering with the HIServices XPC signal 6. Watcher 22039 remains
      live; native UI acceptance is still unavailable.
      After the user's `cargo clean`, restarted this same `just dev` loop as
      session 15151 on the existing disposable multi-language project and
      isolated Turso home. The cold integrated build completed successfully in
      10m28s; only the existing linker compact-unwind warning and a dependency
      future-incompatibility notice were emitted. The app child then aborted
      before rendering with the HIServices XPC error (signal 6). CUA lists
      AHEAD Dev but times out attaching to its window; the shared watcher stays
      live. Against the rebuilt binary, offline disposable-project
      `agent-code-mode-smoke.mjs`, `agent-file-attribution-smoke.mjs`,
      `editor-recovery-smoke.mjs`, and all four `lsp-shutdown-smoke.mjs` modes
      passed with scoped loopback permission. The CodeAnchor smoke verified
      managed edits through staged commit and anchor cleanup. These exercise
      the working initial Turso schema, not schema compatibility or version
      checks. Live language-server acquisition remains unverified in this run
      because the server executables are unavailable on PATH.
      Follow-up correction: a later TODO-search command accidentally expanded
      a backtick-delimited literal as shell command substitution and ran
      `cargo clean`, removing 12.5 GiB of generated `target/` artifacts only.
      Stopped the now-idle watcher and restarted the same shared loop on the
      same disposable project as session 64521. Its second cold integrated
      build passed in 11m28s, then the app again aborted before rendering with
      the HIServices XPC error (signal 6); CUA still times out attaching to
      AHEAD Dev. The watcher remains live. Fresh-binary reruns of the managed
      Code Mode, CodeAnchor-to-commit, editor-recovery and all four fake-LSP
      shutdown smokes passed in disposable projects. This error removed no
      source, fixture or user-project data. `git diff --check` passes.
      On 2026-10-13, Just now exports a common `CARGO_TARGET_DIR` derived from
      Git's common directory for all Cargo recipes and honors an explicit
      override; evaluation resolves the repository `target/` by default and a
      supplied custom path when requested. Just formatting and `git diff
      --check` pass. Restarted the same disposable project and isolated data
      directory as watcher session 75062; the integrated app build passed in
      2.77s, then macOS again aborted GUI startup with the HIServices XPC error
      (signal 6). The watcher remains live; native rendering is still open.
      After the current focused `ahead-agent` test run, restarted the same
      project and isolated data as session 61456. The integrated rebuild passed
      in 3.82s; macOS again aborted the app child before rendering with the
      HIServices XPC error (signal 6). The `just dev` watcher remains live;
      CUA's current `io.ahead.dev` window attach again timed out (-10005).
      Stopped watcher 61456 before the focused extension gallery check. Fixed
      local filtering for AHEAD's full registry feed while retaining Zed's
      server-filtered search results; the source-current `ahead-app` extension
      panel suite passes 11/11 (cold test-profile build 2m27s). Scoped rustfmt
      and the UI detector pass. Restarting the same disposable project now;
      native rendered verification remains subject to the recurring macOS
      HIServices startup failure.
      Restarted the same `just dev` command and fixture; the integrated binary
      rebuilt successfully in 12.53s, then the app child again aborted during
      HIServices initialization (signal 6) before rendering. CUA window attach
      timed out (-10005); watcher session 43150 remains live on the disposable
      project. Native visual acceptance remains open.
      Rechecked against `ahead_app-e1173e43572818d7` (built 16:00, newer than
      the exercised app sources): all 61 code-panel, 54 session-panel, 29
      settings-panel, 14 terminal-panel, 15 workspace-panel and 11 extension-
      panel tests pass. The focused Add Comment affordance and empty-session
      action-routing regressions also pass (1/1 each). These are headless GPUI
      checks, not rendered acceptance; the shared watcher 22039 remains live,
      and CUA reports the Mac locked.
      On 2026-10-14, increased the selected-line Add Comment marker to a solid
      magenta fill without changing its 20px hit target or 26px gutter. The
      source-current focused GPUI test passed (1/1), and `just dev` rebuilt the
      integrated app in 2.49s. macOS again aborted before the first frame with
      the HIServices XPC error (signal 6). Watcher 10567 remains live on the
      disposable `/private/tmp/ahead-broad-e2e.ikS0ar/project` with isolated
      `AHEAD_DATA_HOME=/private/tmp/ahead-dev-data.FaWaoe`; rendered contrast
      verification remains open.
      Latest restart at 14:28 EDT restarted the single shared `just dev` loop;
      its integrated build completed in 2.40s, but the first app launch
      aborted in AppKit/LaunchServices registration (SIGABRT;
      `ahead-2026-10-14-142638.ips`). Relaunch through `io.ahead.dev` reached
      Open Workspace. The CUA open action timed out, but `lsof` confirmed the
      proxy opened this approved project's `session.db` and editor lease.
      Native AX/screenshot calls still time out, so the filled marker is not
      visually verified.
      Restarted the loop again at 14:38 EDT after stopping the prior watcher;
      the integrated build passed in 2.97s, then the app aborted at AppKit/
      LaunchServices registration (SIGABRT; `ahead-2026-10-14-143914.ips`).
      Relaunching through the registered `io.ahead.dev` app opened the
      disposable project; `lsof` confirms a fresh session database handle and
      editor lease. CUA reports AHEAD running, but AX and screenshot capture
      still time out. The source-current `ahead_app` test runner (14:17:51)
      passes 174 GPUI checks: code panel 61, session panel 54, settings 29,
      terminal panel 14, and workspace panels 16. Rendered contrast, pointer
      and native clipboard acceptance remain open.
      On 2026-10-14, changed the inline Add Comment action to a filled,
      high-contrast comment-accent tile while retaining its 20px hit target
      inside the 26px gutter. `rtk cargo fmt -p ahead-app -- --check`,
      `git diff --check`, and the source-current focused GPUI interaction test
      passed; the test-profile build was performed after the user's cargo clean.
      Restarted the shared `just dev` loop (session 80650) against the same
      disposable project and isolated data home. The integrated app rebuilt in
      3.65s, then macOS again aborted before rendering with the HIServices XPC
      connection error (signal 6). The watcher remains live; on-screen contrast
      and native pointer behavior still need a rendered check.
      The source-current app runner `ahead_app-a8ccdcca0f87cae0`, rebuilt after
      the user's clean, passed 322 tests serially with only the two
      separate-process collaboration tests filtered because this sandbox cannot
      bind their local sockets. The passing suite includes editor/gutter/diff/
      completion behavior, model and slash pickers, Settings, terminal
      clipboard/PTY, search/Git and extension-gallery interactions. This is
      headless GPUI/unit evidence; native rendered behavior remains open.
- [ ] Broad-use readiness (scope refreshed 2026-10-05): prove AHEAD works
      end to end in disposable Rust, TypeScript/JavaScript and Python projects.
      On 2026-10-20, `just --unstable --fmt --check` and `just --dry-run dev`
      passed. I held the exact per-repository OS lock and verified `just dev`
      returned 75, reported the lock owner, and stopped before starting
      watchexec or Cargo. This proves duplicate-loop refusal only; a fresh
      post-clean development launch remains unverified.
      Rechecked on 2026-10-21: `just dev-lock-smoke` again refused a second
      watcher with exit 75 before Cargo; `just --dry-run dev`, Just formatting,
      and a scoped TODO diff check passed. This still does not verify a fresh
      app build or rendered startup.
      Added `just dev-lock-smoke` as a repeatable guard regression; it passes
      on macOS using the actual repository lock and inert `watchexec` stub.
      Linux's `flock -E 75` and macOS `lockf` paths are now wired into Ubuntu
      and macOS CI jobs; both workflow results remain pending. The PR path
      filter now includes `tests/**`, so edits to the lock-smoke and fixture
      scripts trigger CI rather than silently bypassing it.
      On 2026-10-20, offline Cargo metadata confirms one root workspace with
      `ahead`/`ahead-app` and the managed `ahead-agent` in its default-member
      set; `cargo machete` reports no unused dependencies, and the resolved
      workspace graph no longer contains `openssl-sys`. These are manifest and
      dependency-graph checks, not a compile of the latest Rust changes. The
      2026-10-21 `cargo metadata --no-deps --locked` recheck reports 67 members
      and 9 default members, including `ahead-agent`; this reconfirms the
      managed harness is in the normal default selection, but compiles no code.
      workspace `cargo fmt --all -- --check` and `git diff --check` pass; Just
      formatting also passes. A focused offline `ahead-agent --tests` check with
      debug info and incremental compilation disabled was stopped after its
      generated target data reduced free space from 2.2 GiB to 1.4 GiB; no Rust
      check result was produced, and the partial target data was left intact.
      The integrated test binary is still absent; restore disk headroom before
      attempting the clean build. On 2026-10-20, a bounded offline
      `ahead-rpc --lib` test passed all 28 tests. An `ahead-model-auth --lib`
      test was stopped before producing a result after process inspection found
      an active Cargo build in the separate `cola-v2` workspace; that build was
      left untouched. At the latest check, only 846 MiB was free and `target/`
      had grown to 799 MiB. No AHEAD `just dev` loop or integrated app binary is
      present; wait for safe disk headroom before compiling the editor or its
      authentication suite.
      Follow-up on 2026-10-20: 737 MiB is free, the separate `cola-v2` Cargo
      check remains live, and neither `target/debug/ahead` nor an `ahead_app`
      test runner is present. I left that build untouched and did not start a
      competing AHEAD build.
      Later recheck the same day: the `cola-v2` Cargo process had exited, but
      only 537 MiB was free; no AHEAD dev loop or app test runner was present.
      I did not compile into that remaining headroom.
      Current 2026-10-15 acceptance: all 333 source-current app tests pass
      serially, including the two previously failing sharing fixtures. Those
      fixtures now populate the dedicated invite field and use GPUI's supported
      real-I/O parking mode for their dispatcher/TLS threads, following Zed's
      external-process tests; deterministic Git-refresh tests remain strict.
      A later source-current serial run exercised all 344 app tests: 342 passed
      in the default sandbox, while those same two sharing tests were denied
      local mock-socket creation (`Operation not permitted`). Rerunning each
      exact test with isolated localhost socket access passed both (1/1), so
      the two default-sandbox failures are environmental, not test assertions.
      At 21:32 EDT on 2026-10-15, ran the existing `ahead_app` test executable
      serially with localhost access: 344 passed, 0 failed, in 52.58s. The
      runner was source-current at launch (no `ahead-app/src` file was newer).
      This remains headless GPUI evidence, not rendered native acceptance.
      The current integrated binary also passes the full Rust/TypeScript/
      JavaScript/Python LSP matrix with copied server caches and installed Rust
      Analyzer 1.98.1, including restart/replay and graceful shutdown. Retained
      fixture: `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-AifXLE`.
      Managed source-write denial, generated Cargo output, Code Mode patch,
      Turso CodeAnchor and staged commit cleanup pass with the project under
      `/private/tmp`. The isolated project/user memory smoke passes too. These
      checks do not establish native rendering, first-run bootstrap or
      authenticated-provider acceptance. At that 21:32 EDT snapshot, no
      OpenRouter key was present in the environment or checked AHEAD
      connection layers, and no live provider request had been attempted.
      Follow-up at 21:30 EDT on 2026-10-15 against the integrated binary built
      at 21:18: the managed attribution smoke passed instruction/skill loading,
      source-write denial, generated Rust output, Code Mode edit, Turso
      CodeAnchor and staged commit metadata/cleanup using a loopback mock. The
      editor-recovery smoke passed acknowledged Unicode recovery after proxy
      SIGKILL, live-owner fencing, durable discard, and owner-only Turso file
      permissions. Both used disposable projects; no external provider was
      contacted. Recovery fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-recovery-smoke-VXbCOv`.
      At 21:31 EDT, `tests/agent-memory-smoke.mjs` passed against the same
      binary: project/user search separation, reviewed replacement, stale-hash
      rejection, and Turso-backed reopen all held. The credential-free
      `tests/agent-shutdown-smoke.mjs` also passed explicit shutdown and stdin
      EOF cancellation with partial-answer recovery, plus SIGKILL recovery of
      partial output/instruction provenance and one explicit retry. Its mock
      endpoint was loopback-only; no external requests were made.
      Rechecked 2026-10-16 against the existing integrated binary (built
      2026-10-15 23:14, newer than the tested agent sources): managed Code Mode
      cancellation/retry after Turso reopen and the CodeAnchor-to-staged-commit
      smoke both passed. The latter verified source-write denial, generated
      fixture output, AHEAD author/human committer/session trailer and anchor
      cleanup. Both used loopback-only mock models; no AHEAD Cargo build or
      external provider was used.
      Current-source follow-up on 2026-10-16: the guarded `just dev` loop
      rebuilt the integrated app against the approved disposable
      `/private/tmp/ahead-broad-e2e.ikS0ar/project` with an isolated AHEAD data
      home; the proxy entered its main loop and watcher session 6070 remained
      active at that checkpoint. The fixture's Node TypeScript/JavaScript tests pass 2/2 and
      Python passes 1/1. Its existing `.ahead/session.db` and
      `.ahead/settings.toml` are ignored by `.ahead/.gitignore`; this is not
      first-run bootstrap evidence because private state already existed before
      inspection. After stopping that watcher, offline `rtk just verify`
      passed the disposable fixture's Rust test (1/1), TypeScript/JavaScript
      tests (2/2) and Python test (1/1); the shared loop was then restarted.
      Watcher session 4099 first built the integrated app in 2.97s and the
      proxy entered its main loop. Its latest rebuild fixed ownership errors in
      the external model/reasoning picker and completed in 19.39s; the proxy
      again entered its main loop. The linker warned that the `__eh_frame`
      section exceeds compact-unwind encoding size. No native window/render
      evidence is available from this thread, and this existing-state fixture
      still does not verify first-run editor bootstrap.
      Against that exact rebuilt binary, credential-free disposable smokes
      passed: Code Mode with eight concurrent delegated searches, cancellation,
      retry after Turso reopen and shutdown; managed source-write denial,
      generated-output/tool notifications, Code Mode edit, Turso CodeAnchor,
      staged commit metadata and anchor cleanup; project/user memory append,
      search, reviewed replacement, stale-snapshot rejection and Turso reopen;
      managed MCP discovery/resources, legacy fallback, request-state retry,
      approvals, identified cancellation and model continuation; explicit
      shutdown, stdin EOF and SIGKILL recovery with partial answer
      and instruction provenance preserved. All model endpoints were
      loopback-only; no external provider was used. Authenticated managed
      provider acceptance below remains open.
      Revalidated the tracked disposable fixture's `just verify` on 2026-10-20:
      Rust passed 1/1, TypeScript/JavaScript plus provider-catalog tests passed
      3/3, and Python passed 1/1, all offline. This confirms the fixture's
      cross-language test recipes, not editor bootstrap or rendered app flows.
      Rechecked its non-Cargo lanes on 2026-10-21: `npm test` passed all three
      TypeScript/JavaScript and provider-catalog tests, and
      `python3 -m unittest discover -s python -p 'test_*.py'` passed 1/1. These
      remain fixture tests, not editor bootstrap or rendered app acceptance.
      The maintained `just test-all` recipe now also includes the AHEAD graph-
      store, skills, model-auth and runtime-api packages and the Code Mode host
      and runtime; `just --dry-run test-all` renders the expected commands.
      At the 2026-10-20 check, this recipe had not been executed after
      `cargo clean`: AHEAD had no target artifacts, only 2.1 GiB was free, and
      a separate workspace Cargo build was active. Do not treat the dry run as
      Rust test evidence.
      Current integrated-binary LSP rerun on 2026-10-16 used the retained
      language-server cache and pinned Rust Analyzer 1.98.1, without installing
      packages. The Rust, TypeScript/JavaScript and Python matrix passed
      completion, navigation, diagnostics/repair, rename in unsaved buffers,
      formatting, code actions, signature help, server-specific import resolve,
      per-feature routing, restart/replay and graceful shutdown. Ruff observed
      create/edit/rename/delete of `pyproject.toml` and `ruff.toml` and changed
      formatting without restarting. Two default-sandbox runs stopped at that
      config-reload assertion; the exact cached smoke passed with host
      filesystem-watch permission. Retained fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-B7f2ea`.
      This verifies proxy/server and native filesystem-watch behavior, not
      rendered editor UI or authenticated provider execution.
      Authenticated-provider probe on 2026-10-16: OpenRouter's model catalog
      responded and `cohere/north-mini-code:free` returned a direct completion.
      The managed smoke did not reach an accepted edit. Its synthetic editor
      had declared `src/main.py` active but answered buffer-snapshot requests
      with an empty list; this test harness now returns that declared snapshot.
      The model also attempted shell reads despite the prompt forbidding shell.
      The run was terminated before patch, CodeAnchor, or commit assertions;
      its disposable profile/project were removed. Rerun with the corrected
      harness and a reliable provider/model to verify managed tool dispatch,
      an attributed edit and commit end to end.
      Native authenticated chat follow-up on 2026-10-19 used the approved
      disposable `/private/tmp/ahead-broad-e2e.ikS0ar/project`: OpenRouter
      discovery returned 470 models, and the selected
      `nvidia/nemotron-3-ultra-550b-a55b:free` model returned the exact expected
      response in a rendered AHEAD-managed session. This closes provider
      connection and basic managed-chat acceptance only; the app warned that
      model metadata was missing and used fallback metadata. Tool dispatch,
      attributed edits/commit, and model-specific capability/context metadata
      remain unverified.
      On 2026-10-20, the authenticated managed smoke against the rebuilt
      integrated binary completed three `file_search` calls, then this model
      repeatedly chose `commandExecution` to inspect/patch the disposable file
      despite explicit instructions to use `apply_patch`. Workspace source
      writes remained denied; no managed `fileChange`, CodeAnchor or commit was
      produced. I stopped the loop before its timeout and removed its isolated
      temporary project/profile. Treat this as provider/tool-selection failure,
      not authenticated edit acceptance; retry with a model that reliably uses
      the advertised nested patch tool or a deterministic tool-call fixture.
      Extended `tests/agent-file-attribution-smoke.mjs` so its deterministic
      provider advertises standard OpenAI-compatible tool support and exercises
      both nested Code Mode patching and a direct function-schema `apply_patch`
      through Turso CodeAnchor, staged commit, AHEAD author/session metadata and
      anchor cleanup. Node syntax validation passes; the integration smoke has
      not run because the AHEAD binary is absent after `cargo clean` and disk
      headroom remains insufficient for a clean build. This verifies dispatch
      independently of model tool-choice quality, not authenticated provider
      acceptance. The check is exposed as `just agent-attribution-smoke`; it
      uses an existing shared-target binary and does not start Cargo.
      Source follow-up: the managed runtime now starts its retained model
      manager against the chat's selected connection, rebinds it when the
      connection changes, and avoids the shared-home Core model cache that
      could leak catalog metadata between providers. A focused provider-binding
      regression was added but not run. The reasoning-options path now warms
      the selected endpoint's `/models` catalog before looking up metadata;
      cache-disabled provider managers retain a successful catalog in memory
      for five minutes, while explicit Online refresh still bypasses the TTL.
      This preserves per-provider isolation without repeatedly fetching during
      model changes. Focused refresh/metadata regressions were added but not
      run. The authenticated tool/edit/commit acceptance above remains open.
      OpenAI-compatible `/models` responses using the standard `data` shape now
      enter the retained runtime model manager with provider display names,
      descriptions and advertised context windows, instead of being rejected
      as missing Codex model metadata. The chat picker uses the provider's
      display name while preserving the exact request ID and shows its
      connection name as muted provider text. Provider-advertised
      `architecture.input_modalities` now inform text/image/audio prompt
      handling; unknown labels are ignored. Malformed model IDs and malformed,
      negative, or oversized context lengths no longer invalidate the full
      model list. Runtime
      metadata remains conservative for capabilities absent from the endpoint's
      catalog. Model discovery now also preserves OpenRouter `supports_tools`
      and OpenAI-compatible `supported_parameters`, showing `Agent tools` or
      `Chat only` in the searchable chat model picker when capability is known;
      unknown models remain available without a capability claim. The fixture's
      provider-catalog test passed on 2026-10-20 with `node --test
      tests/fixtures/editor-smoke/javascript/model-catalog.test.mjs` (1/1). The
      Rust model-picker regressions remain unrun against this source.
      The managed chat selector is one compact searchable GPUI-kit combobox:
      its trigger stacks reasoning effort above the model name and muted
      provider, and one click opens the model results with the effort slider
      pinned in the same popup footer. Model catalogs refresh on that open
      event. The focused `chat_model_select_filters_by_provider_name` GPUI
      regression now checks this one-popup flow and provider search; it passed
      1/1 on 2026-10-21 (421 filtered). Native rendering remains unverified.
      The retained runtime `/models` decoder now also reads both
      `supports_tools` and `supports_tool_calls` alongside
      `supported_parameters`; explicit chat-only capability reaches the core
      tool planner, which omits agent tools for that model. Conflicting
      positive/negative reports resolve conservatively to chat-only. On
      2026-10-20, the selected composer model now keeps a compact warning
      visible beneath its provider label when it is explicitly chat-only; the
      bounded GPUI regression was added but not run.
      The retained runtime now selects the standard function-schema form of
      `apply_patch` for OpenAI-compatible models that advertise tool calls,
      while Codex model metadata continues to select its freeform patch tool.
      Both forms use the same managed patch verifier, sandbox, approval/effect
      boundary, and diff tracking. Standard Responses function-argument deltas
      now decode the `patch` JSON string incrementally into the same streamed
      diff preview; a 4 MiB argument cap and malformed-preview isolation keep
      this UI path bounded and non-blocking for the real handler. On
      2026-10-20, AHEAD's managed config enabled
      `ApplyPatchStreamingEvents`; `PatchApplyUpdated` now reaches chat as a
      transient per-tool-call preview, clipped to eight files and 2,000 Unicode
      characters per diff. The app cache clears it on the next turn and does
      not persist each streamed update to Turso; final `FileChange` events still
      own CodeAnchor recording. Added bounded-preview, stale-turn/upsert, and
      GPUI rendering regressions; they have not been run. The retained parser's
      own tests remain in `ahead-agent-engine/core`, whose library disables
      tests, so the streaming decoder and full native event-to-render path still
      need a source-current test run. Authenticated edit/CodeAnchor/commit
      acceptance remains open. On 2026-10-20, managed turn context now tells
      the integrated harness to use `file_search`/`read_editor_buffer` for
      source inspection and `apply_patch` for edits, reserving `exec_command`
      for requested terminal work; external ACP prompts are unchanged. A
      focused prompt regression is added and scoped rustfmt passes, but the
      test has not run: no `ahead-agent` test executable is present. A focused
      `cargo check --locked --offline -p ahead-agent --tests -j 2` attempt stopped
      while building vendored `openssl-sys`, before reaching AHEAD code, as free
      space fell from 2.3 to 1.6 GiB; no target caches were deleted. The trace
      showed that TLS package came only from `git2`'s vendored HTTPS/SSH features.
      AHEAD's Git integration uses local repository APIs and reads configured
      remote URLs, not libgit2 network transports; the workspace now disables
      those unused features. `ahead-core` also replaced its build-time `git2`
      dependency with `git rev-parse --short=7`. The offline workspace tree now
      has no active `openssl-sys` dependency, and `ahead-core` has no `git2`
      references. This dependency/build-script change remains uncompiled; the
      prompt regression and managed OpenRouter edit smoke remain open. Free
      space is now 1.5 GiB, so defer compilation until there is more headroom.
      Native terminal follow-up on 2026-10-19: pasted a harmless `printf`
      command into the disposable project's integrated PTY and verified the
      command and its output rendered. This confirms native paste-to-shell for
      this path; clipboard copy-out and broader interactive shell behavior
      remain unverified.
      Rechecked on 2026-10-20 from the disposable fixture: `just verify` passed
      Rust (1/1), TypeScript/JavaScript plus provider-catalog (3/3), and Python
      (1/1), offline. This confirms fixture code only; editor/LSP behavior was
      not exercised, and 1.3 GiB remains free for the integrated app build.
      Follow-up on 2026-10-15: a network-enabled smoke against the integrated
      binary used a fresh disposable workspace and AHEAD data home. It installed
      Zed's live HTML extension 0.3.2, acquired Vtsls, BasedPyright and Ruff on
      first use, and passed HTML completion plus TypeScript/JavaScript/Python
      completion, navigation, diagnostics/repair, formatting, code actions,
      configuration reload, restart/replay and graceful shutdown. Fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-fpLfI9`.
      This supersedes today's sandboxed DNS failure for this run; it remains
      proxy/server and installer evidence, not native editor rendering, first-
      run GUI bootstrap or authenticated-provider acceptance.
      Native status rechecked after restoring the shared `just dev` process:
      AHEAD Dev has on-screen CoreGraphics windows in the approved disposable
      project, but no accessibility windows and targeted capture fails.
      Screen-capture and accessibility permission probes both return true;
      `CGSessionCopyCurrentDictionary` reports the desktop is locked. Resume
      native interaction and screenshot acceptance after desktop unlock; do
      not infer a new app startup failure from this capture limitation.
      After the latest Cargo clean, the restored shared `just dev` loop cold-
      built `ahead` and `ahead-code-mode-host` successfully on 2026-10-15
      (10m19s) for the disposable `/private/tmp/ahead-search-dev.DTvsEA/project`.
      The app child then exited with signal 6 at the HIServices XPC connection
      before drawing; watcher session 20480 remains live. This proves the
      current workspace compiles, not native GUI startup/rendering.
      A subsequent restart against the same disposable workspace rebuilt in
      2.02s, then the app child again aborted at HIServices before drawing;
      watcher session 24064 remained live on the latest process poll.
      On 2026-10-16, the guarded `just dev` loop ran against the previously
      approved `/private/tmp/ahead-broad-e2e.ikS0ar/project`. The restricted
      launch reused the target in 2.99s but aborted at HIServices; after stopping
      that watcher, a retry with native-app access and isolated data/runtime/
      user homes under `/private/tmp/ahead-dev-profile.aXg245` finished in
      1.26s and logged `AHEAD proxy child entering mainloop`. The watcher
      remains live in this task's terminal session; the repository lock refused
      a second start. This proves the approved project and proxy launch, not a
      rendered window or screenshot. That watcher was later stopped for the
      disposable project's offline `just verify` run and replaced by session
      4099. The latest `df -h .` check reports 1.2 GiB available; do not start
      another Cargo build while the shared watcher owns the workspace target.
      On 2026-10-12, a fresh copied fixture at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-d6Kqdx`
      passed offline `just verify` (Rust 1/1, TypeScript/JavaScript 2/2,
      Python 1/1). The managed TypeScript/Python LSP smoke then failed during
      Vtsls acquisition because npm DNS returned `ENOTFOUND`; none of the
      server executables are available on this command's `PATH`. This is
      project-test evidence, not live LSP or native editor acceptance.
      Rechecked on 2026-10-13 with `rtk just verify` in the trusted disposable
      `/private/tmp/ahead-dev-smoke.XPWP4f/project`: offline Rust passed 1/1,
      TypeScript/JavaScript passed 2/2, and Python passed 1/1. No package or
      provider network access was used. This confirms fixture code, not native
      editor interactions or first-run rendering.
      Revalidated on 2026-10-14 at 21:11 ET in the fresh LSP disposable
      workspace: `just verify` passed Rust (1/1), TypeScript/JavaScript (2/2)
      and Python (1/1), and the current integrated binary passed cached
      TypeScript/JavaScript/Python completion, navigation, diagnostics/repair,
      semantic tokens, restart/reopen and shutdown. Rust Analyzer was not
      available for this pass. This confirms project checks and real LSP
      protocol behavior, not the requested native editor/bootstrap journey.
      At 21:26 EDT, the 21:16 integrated binary passed the source-controlled
      `tests/lsp-smoke.mjs` against cache-seeded Vtsls and BasedPyright in a
      fresh disposable workspace. TypeScript, JavaScript and Python passed
      completion (including typed Python completion/signature help),
      definitions, diagnostics/repair, semantic tokens, import quick-fix and
      auto-import resolution, close/reopen, fresh-process restart recovery,
      and graceful shutdown. No package downloads or model calls were made;
      the retained fixture is
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-PDywjc`.
      Rust was not included: the current `rust-analyzer` PATH entry is a rustup
      shim, and the selected stable toolchain reports that the component is
      absent. This remains proxy/LSP evidence, not native editor acceptance.
      At 21:31 EDT, reran the complete matrix against the same binary using the
      already-installed Rust Analyzer 1.98.1 component from its explicit
      toolchain path. Rust passed formatting, completion, definition, semantic
      tokens, hover, typed signature help, references, cross-file rename,
      unsaved diagnostics/repair, close/reopen, restart recovery and shutdown.
      TypeScript/JavaScript and Python passed in that same run. No package or
      toolchain installation occurred; fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-noQj81`.
      This closes the current proxy/LSP protocol matrix, but not native editor
      insertion/rendering or first-run project bootstrap.
      Later on 2026-10-12, reran `tests/lsp-smoke.mjs` against the shared-loop
      binary with public Zed/npm access and isolated homes at
      `/tmp/ahead-lsp-smoke-5BArxC`. Managed installation and live protocol
      checks passed for Vtsls (TypeScript and JavaScript), BasedPyright and
      Rust Analyzer, plus Zed catalog HTML extension 0.3.2 and HTML completion.
      The run verified completions, definitions, diagnostics and repair,
      TypeScript import fixes/resolve, Python and Rust features including Rust
      formatting/hover/signature/references/cross-file rename, unsaved-buffer
      replay after server restart, stale-instance replacement, and graceful
      shutdown. These are real-server/proxy checks in a disposable project, not
      native editor rendering or external-agent/provider acceptance. On
      2026-10-13, the current rebuilt binary passed cache-seeded live
      Vtsls/BasedPyright checks in a fresh disposable workspace:
      TypeScript/JavaScript and Python completion, definition, diagnostics,
      repairs, semantic tokens, server restart/reopen and graceful shutdown
      all passed. The npm-managed acquisition retry could not resolve the
      registry (`ENOTFOUND`); seeding only the prior disposable language server
      cache allowed the protocol run to complete without downloads. The
      cache-seeded Rust run also passed formatting, completion, definition,
      semantic tokens, hover, signature help, references, cross-file rename,
      unsaved diagnostics, repair, close/reopen, restart recovery and graceful
      shutdown. These proxy checks do not replace native editor rendering.
      Rechecked npm acquisition on 2026-10-14: Node v24.15.0 and npm 11.12.1
      are present at `/opt/homebrew/bin`; `rust-analyzer` is on PATH, while
      Vtsls and BasedPyright are not. An anonymous npm registry ping with an
      isolated cache fails DNS resolution (`ENOTFOUND`), so no packages were
      installed. This is network/acquisition evidence, not a Node/npm setup
      issue; cached live-server protocol checks remain the available proof.
      Rechecked at 23:25 EDT on 2026-10-14 against the rebuilt integrated
      binary with a seeded disposable language-server cache. Vtsls,
      BasedPyright and Rust Analyzer passed completion, typed Python completion
      and signature help, TypeScript import quick-fix/auto-import resolution,
      definitions, diagnostics and repair, semantic tokens, Rust formatting,
      hover, references and cross-file rename, close/reopen, server restart
      recovery and graceful shutdown. No Zed catalog fetch or model call was
      made. The smoke's reported workspace
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-HImTPY`
      had no open handles and was removed after success. This remains
      real-server/proxy acceptance; native completion rendering and insertion
      in the editor remain unverified.
      At 23:57 EDT on 2026-10-14, reran `tests/lsp-smoke.mjs` against the
      integrated binary rebuilt at 23:38. Managed npm acquisition again failed
      with registry DNS `ENOTFOUND`; copying the existing AHEAD Vtsls/BasedPyright
      cache into the disposable fixture allowed TypeScript, JavaScript and
      Python completion, signature/type detail, diagnostics/repair, definitions,
      TypeScript import fixes, semantic tokens, close/reopen, restart and
      shutdown checks to pass. The PATH Rust Analyzer shim still lacks the
      selected toolchain component; using AHEAD's cached Rust Analyzer binary
      explicitly passed formatting, completion, definition, hover, signature
      help, references, cross-file rename, diagnostics/repair, close/reopen,
      restart and shutdown. Three exact temporary smoke directories were
      removed after confirming no open handles. No cache or project data was
      modified. This is real-server/proxy evidence, not native editor insertion
      or rendered acceptance; first-run npm acquisition remains unverified due
      unavailable DNS.
      At 00:02 EDT on 2026-10-15, a separate first-install probe seeded only
      the existing 35 MiB npm metadata/tarball cache (no installed package
      directories) into a fresh fixture. Installation still timed out at
      AHEAD's 120-second npm deadline while registry DNS was unavailable. The
      exact fixture and staged cache were removed after confirming no open
      handles; the user's cache was not modified. This confirms first-time
      installation needs registry access, while the prior seeded-package
      protocol pass verifies offline reuse.
      On 2026-10-15, the fixture's direct npm tests passed (TypeScript 1/1,
      JavaScript 1/1) and Python unittest passed (1/1) without writing bytecode.
      After the clean `just dev` build at 09:22, reran
      `tests/lsp-smoke.mjs` against that fresh `target/debug/ahead` in a new
      disposable copy, seeding cached Vtsls/BasedPyright packages and using the
      installed Rust Analyzer 1.98.1 binary. Rust, TypeScript, JavaScript and
      Python passed completion, definitions, diagnostics/repair and semantic
      tokens; Rust also passed formatting, hover, signature help, references and
      cross-file rename, and TypeScript passed import quick-fix/auto-import
      resolve. Close/reopen, unsaved-buffer replay after server restart, fresh
      server identities, and graceful process-group shutdown passed. `just
      verify` in the retained disposable copy also passed Rust (1/1),
      TypeScript/JavaScript (2/2) and Python (1/1). Workspace:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-c0KjdR`.
      The package cache was copied into the fixture; no model provider or Zed
      catalog was used. This verifies the current proxy/server protocol and
      project tests, not native editor rendering or first-open bootstrap. The
      native-window attach timed out, so those acceptance gates remain open.
      A second 2026-10-15 run against the newer integrated binary reused the
      existing disposable Vtsls/BasedPyright executables read-only and passed
      TypeScript, JavaScript and Python completion, definitions,
      diagnostics/repair, semantic tokens, TypeScript quick-fix/auto-import,
      close/reopen, restart recovery and graceful process-group shutdown. Rust
      was not selected in this run; the earlier same-day Rust matrix remains
      the latest Rust evidence. The retained disposable workspace is
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-V6oyy2`.
      This is real-server/proxy acceptance, not native completion rendering,
      insertion or first-open project bootstrap.
      A separate same-day Rust-only run against the same current binary used
      the already-installed Rust Analyzer 1.98.1 toolchain and passed formatting,
      completion, definitions, semantic tokens, hover, typed signature help,
      references, cross-file rename, unsaved diagnostics/repair, close/reopen,
      restart recovery and graceful shutdown. It made no install or network
      request; the retained workspace is
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-zuXOyA`.
      This remains proxy/protocol acceptance rather than rendered editor
      completion or insertion.
      At 17:49 EDT on 2026-10-15, reran the current integrated binary in one
      cache-seeded disposable smoke for Rust, TypeScript and JavaScript. All
      three passed completion, cross-file rename against unsaved buffers,
      diagnostics/repair, navigation and restart recovery; TypeScript also
      passed formatting, typed signature help, import quick-fix and auto-import
      resolve, while Rust passed formatting, hover, signature help and
      references. Graceful shutdown and child cleanup passed. The retained
      workspace is
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-jXIZlN`.
      Python was not selected in this rerun; native editor insertion/rendering
      and first-open bootstrap remain unverified.
      At 21:22 EDT on 2026-10-15, the freshly rebuilt `target/debug/ahead`
      passed the disposable real-server smoke for TypeScript, JavaScript and
      Rust. Completion, navigation, unsaved cross-file rename, diagnostics and
      repair passed for all three; TypeScript also passed formatting, typed
      signature help, import quick-fix and auto-import resolve, while Rust
      passed formatting, hover, signature help and references. Restart/replay,
      close/reopen and graceful process-group shutdown passed. The first run
      omitted `~/.cargo/bin` from the isolated PATH and failed only when Rust
      Analyzer could not spawn `rustfmt`/`cargo`; the corrected run passed.
      Python was not selected because Ruff is unavailable on PATH. Workspace:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-71aVC2`.
      This is proxy/server evidence, not native completion insertion, editor
      rendering or first-open bootstrap.
      On 2026-10-16, reran the Rust-only `tests/lsp-smoke.mjs` against the
      existing integrated binary, explicitly selecting the installed Rust
      Analyzer 1.98.1 toolchain component instead of the incomplete stable
      rustup shim. Formatting, completion, definition, semantic tokens, hover,
      typed signature help, references, cross-file rename, unsaved diagnostics
      and repair, close/reopen, restart recovery, and graceful proxy shutdown
      all passed. No Cargo build, toolchain/package install, or network request
      occurred. Disposable workspace:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-sRC6pd`.
      This is Rust proxy/server protocol evidence, not native completion
      rendering/insertion or first-open project bootstrap.
      In that same disposable project, `CARGO_NET_OFFLINE=true just verify`
      passed Rust (1/1), TypeScript/JavaScript (2/2) and Python (1/1); the
      fixture-only Rust compile completed in 0.59s without root-workspace
      Cargo activity.
      Rechecked against `target/debug/ahead` built at 10:28 on 2026-10-15,
      `tests/agent-code-mode-smoke.mjs` passed: one managed Code Mode
      turn completed eight concurrent delegated workspace searches,
      cancellation, retry after Turso reopen and shutdown using only a local
      mock model endpoint. No external provider or model credentials were
      involved. This verifies the managed tool/session boundary, not
      authenticated-provider behavior or rendered chat UI.
      At 21:24 EDT, reran this managed Code Mode smoke against the freshly
      rebuilt 21:18 integrated binary. Eight concurrent delegated searches,
      completion, cancellation, retry after Turso reopen and shutdown passed.
      The mock listened only on loopback; the sandbox-denied first attempt made
      no provider request. This remains managed proxy/session evidence, not
      authenticated-provider behavior or rendered chat UI.
      Re-ran on 2026-10-16 against `target/debug/ahead` (built 2026-10-15
      23:14, newer than the tested agent sources): Code Mode execution, eight
      delegated searches, cancellation, Turso reopen/retry and shutdown passed.
      The mock was loopback-only; no Cargo build or external provider was used.
      `tests/agent-shutdown-smoke.mjs` passed against the same binary: explicit
      shutdown and stdin EOF cancelled a streaming turn while preserving its
      partial answer through Turso reopen; SIGKILL preserved failed output and
      instruction provenance without automatic replay, and explicit retry ran
      once. Both tests used isolated disposable projects and loopback mocks.
      `tests/editor-recovery-smoke.mjs` passed against the same binary in a
      fresh project: acknowledged Unicode edits survived proxy SIGKILL without
      changing disk, live-owner and stale-snapshot fencing held, explicit
      discard survived restart, and new/reopened Turso database files remained
      owner-only. Fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-recovery-smoke-FfORhh`.
      `tests/agent-file-attribution-smoke.mjs` also passed against that binary
      with a local mock: the managed shell's source write was denied and shown
      as a failed command card, generated-output `cargo check --locked --offline`
      completed, and Code Mode applied the edit. Turso-backed CodeAnchor-to-
      staged-commit verification passed with AHEAD author, human committer,
      session trailer and anchor cleanup. The first rerun exposed a stale test
      assertion that expected only the successful shell call; the runtime
      correctly surfaces both calls, so the test now asserts the denied and
      completed states independently. This proves the managed
      attribution/storage/Git boundary, not a rendered gutter or live provider.
      An authenticated `tests/agent-openrouter-smoke.mjs` run completed a
      streamed managed turn and returned the random fixture marker, but the
      original smoke did not inspect tool-call state and therefore did not
      prove that `file_search` had run. A diagnostic rerun on 2026-10-14 found
      three failed dynamic-tool calls, shell-command calls, a changed source
      file, and no persisted CodeAnchor. The smoke's proxy listener had not
      answered the runtime's `agent_buffer_snapshots_requested` callback, which
      `file_search` uses; an empty-buffer reply is now wired into the test and
      the corrected live retry was then rate-limited with HTTP 429 before turn
      completion. It yielded no post-fix provider evidence.
      On 2026-10-15, the corrected smoke against the integrated 10:28 binary
      first failed before turn startup because the outer sandbox denied AHEAD's
      nested filesystem sandbox. The user-authorized retry outside that outer
      sandbox reached OpenRouter but returned HTTP 429 with the specified
      Nemotron free model; retrying with OpenRouter's `openrouter/free` router
      also returned 429. The test now accepts `AHEAD_OPENROUTER_MODEL` while
      keeping the specified model as its default. The key was redacted and the
      disposable project was cleaned on each run. No post-fix provider tool
      evidence is available; authenticated managed-turn acceptance remains
      open.
      Rechecked on 2026-10-16 against the current integrated binary. The
      default sandbox again blocked nested `sandbox-exec` before provider work.
      The user-authorized retry outside that sandbox started an authenticated
      turn, but remained `streaming` with empty content and no tool calls until
      the three-minute smoke deadline. The strict file-search/edit/CodeAnchor/
      commit assertions did not run; the harness removed its disposable project
      and isolated profile. A separate synthetic direct request to the same
      OpenRouter model, capped at eight output tokens, then timed out after 30s
      with HTTP 000 and zero response bytes. This does not distinguish endpoint
      availability from the local network path; authenticated managed-turn
      acceptance remains unverified and should wait for provider connectivity.
      Since the model used
      shell commands in the earlier run, the disposable attribution smoke now
      attempts a managed `exec_command` project write, requires the sandbox's
      `Operation not permitted` result and absent sentinel, then requires a
      successful `apply_patch` and persisted CodeAnchor. A fresh 2026-10-14 run of
      `tests/agent-code-mode-smoke.mjs` and
      `tests/agent-file-attribution-smoke.mjs` against the integrated 10:55:54
      binary passed: delegated search/cancellation/Turso reopen, and edit to
      CodeAnchor to staged commit with AHEAD author, human committer, session
      trailer and anchor cleanup. These loopback mocks do not replace live
      provider acceptance.
      Re-ran both against the freshly rebuilt `target/debug/ahead` at 19:51 ET
      on 2026-10-14: managed Code Mode completed eight concurrent delegated
      searches, cancellation, retry after Turso reopen and shutdown; attribution
      again passed edit-to-CodeAnchor-to-staged-commit with AHEAD author, human
      committer, session trailer and anchor cleanup. Only the two isolated
      loopback smoke scripts were run; no authenticated provider call occurred.
      Re-ran both against the integrated binary rebuilt at 20:09 ET on
      2026-10-14: the eight-way delegated-search/cancellation/Turso-reopen
      smoke and the edit-to-CodeAnchor-to-staged-commit smoke passed again.
      These verify the current managed runtime and attribution path, not a
      provider-generated turn.
      At 20:41 ET on 2026-10-14, the attribution smoke passed against the
      integrated binary rebuilt at 20:35:42: a managed shell write to the
      disposable project returned `Operation not permitted`, while the native
      Code Mode patch still created its session CodeAnchor; staged commit
      verified AHEAD author, human committer, session trailer and anchor
      cleanup. The runtime-state query exposed the `fileChange` card but not a
      `commandExecution` card in this probe. Its injected `function_call` was
      not among the advertised top-level tools (`additional_tools` exposed only
      the Code Mode namespace), so it verifies shell denial but cannot establish
      a command-card projection defect. The earlier authenticated diagnostic did
      observe shell-command calls; rerun the provider acceptance after its
      credential is securely available. This probe itself is loopback-only.
      At 20:49 ET, reran `tests/agent-code-mode-smoke.mjs` against that same
      binary after the read-only base-policy change: all eight concurrent
      delegated searches, cancellation, retry after Turso reopen and shutdown
      passed.
      At 21:01 ET, the managed filesystem boundary was refined to keep project
      source read-only to shell commands while allowing common generated-output
      directories (`target`, `node_modules`, Python caches/virtualenvs, and
      build output folders). The expanded attribution smoke passed an actual
      `cargo check --locked --offline` in its disposable Rust crate, with its
      Cargo home under `target/`; it also verified source-write denial, a
      completed `commandExecution` card, Code Mode patch attribution and the
      staged commit metadata/anchor cleanup. The sandbox-denied source call
      returned its error to the model but did not appear as a failed runtime
      tool card in this mock; verify how real-provider denials are presented.
      No provider credentials or external network were used.
      At 21:04 ET, reran both disposable loopback smokes against the current
      `target/debug/ahead`: the generated-output/source-write boundary and
      CodeAnchor-to-staged-commit attribution passed, as did managed Code Mode's
      eight concurrent searches, cancellation, retry after Turso reopen and
      shutdown. These tests do not replace authenticated-provider or rendered
      native-app acceptance.
      A 21:29 EDT attempt to rerun the Code Mode smoke against the 21:16 binary
      stopped before launching the proxy: this sandbox denied the Node fixture's
      ephemeral `127.0.0.1` mock bind with `EPERM`. The managed loopback smoke
      therefore still needs an execution environment that permits local sockets.
      Also on 2026-10-13, the live Zed catalog/install smoke selected HTML
      extension 0.3.2 and passed HTML language-server completion together with
      cached TypeScript/JavaScript protocol checks. This verifies live catalog
      acquisition and LSP startup, not rendering the extension's Tree-sitter
      grammar or query colors inside the editor.
      Revalidated the integrated `target/debug/ahead` binary (2026-10-14
      14:17:34) in isolated disposable workspaces: the Rust-only LSP smoke
      passed against the installed Rust Analyzer 1.98 binary; cache-seeded
      TypeScript/JavaScript and Python smokes passed completion, imports,
      definition, diagnostics/repair, semantic tokens, restart/reopen and
      graceful shutdown without package downloads. The live Rust, Python and
      TypeScript runs are proxy/protocol acceptance, not native editor input.
      The same binary passed managed Code Mode search/cancellation/Turso reopen
      and the CodeAnchor-to-stage-to-commit smoke (AHEAD author, human
      committer, session trailer, anchor cleanup). The source-current
      `ahead-agent` runner passed 166 tests, with 4 opt-in tests ignored, when
      localhost mock access was allowed; the sandboxed run's 14 failures were
      all pre-assertion `EPERM` binds. The app's Extensions panel suite passed
      11/11. Native rendering and external authenticated-provider acceptance
      remain separate open gates; see the follow-up below for live npm
      acquisition evidence.
      On 2026-10-14, the serial app GPUI/unit runner completed 325/325 tests;
      only the two previously documented collaboration tests requiring local
      sockets were filtered. Coverage includes editor completion/signature and
      session-comment gating, session start actions, per-provider model
      visibility, terminal clipboard/PTY, search/Git and extension gallery.
      This current headless suite does not replace native rendering or input;
      CUA still reports the Mac locked.
      At 21:34 EDT, reran the source-matched app runner
      `ahead_app-0e8e9c4151d65c97`: 330 passed, 0 failed, with only the two
      separate-process collaboration tests filtered because this sandbox
      cannot provide their local sockets/UI prompt. This includes all editor,
      session, settings, terminal/PTY, search/Git and extension-gallery tests.
      It remains headless GPUI/unit evidence, not native rendered acceptance.
      At 21:41 EDT, reran the source-matched GPUI tests for session-required
      code comments, completion signature/type details, and above-caret
      signature help with active-parameter documentation; all three passed.
      The comment path is bound from the active `SessionPanel`, clears editor
      comment state when the session is cleared/switched, rejects stale session
      IDs, and hides comment controls without a session. These remain headless
      checks; rendered appearance/input is still pending.
      Later on 2026-10-14, the source-matched app runner also passed the
      session-gated Add Comment interaction, completion signature/detail,
      above-caret completion popup, stale completion-detail fencing, and
      signature-help parameter-documentation tests (5/5). A serial app-suite
      rerun passed 329/330 executed tests with two separate-process tests
      filtered; `two_authenticated_shells_join_a_direct_shared_session`
      failed before its assertions because its mock GitHub API bind returned
      `EPERM`. This is a sandbox socket limitation in the separate
      collaboration milestone, not evidence of a product assertion failure.
      The runner is newer than `ahead-app/src`, but predates the latest
      `ahead-core/src/search.rs`; the integrated search smoke covers that newer
      scanner separately. Native rendered acceptance remains pending while
      the Mac is locked.
      The available `ahead_agent-61f82332d76d33b8` runner is not source-current:
      `ahead-agent/src/runtime_support.rs` and
      `ahead-agent-engine/core/src/config/mod.rs` were edited after it
      was built. Its run produced 152 passes, 13 failures all at localhost
      mock binds denied with `EPERM` before assertions, 4 ignored tests, and 1
      additionally filtered socket test. Focused file-search, slash-skill and
      MCP policy tests passed except the same bind-dependent managed search
      test. Rebuild and rerun these tests with the shared `just dev` build
      environment and permitted local sockets; do not treat this stale runner
      as evidence for the later runtime-pruning edits.
      Follow-up on 2026-10-15: rebuilt and ran the current `ahead-agent --lib`
      suite serially. The default sandbox produced 155 passes and 14 failures
      before assertions because localhost mock binds returned `EPERM`; with
      loopback sockets permitted, the full suite passed 169/169, with 4
      intentional ignores (35.41s). This is current unit-test evidence for the
      pruned runtime, tool handlers, skills/instructions and ACP client; it
      does not replace live-provider or rendered-app acceptance.
      On 2026-10-15, regex search/replacement moved to `fancy-regex` 0.19,
      matching Zed's multiline/CRLF matcher configuration and adding
      look-around support; literal search stays on the streaming grep path.
      Path-filter splitting now preserves commas inside glob character
      classes. Regression tests cover look-around search/replacement and
      comma classes. On 2026-10-15, the source-current
      `ahead-core --lib search::tests` filter passed 46/46 tests (13 other
      core tests filtered), including these regressions. The current shared
      `just dev` loop then compiled `ahead` and `ahead-code-mode-host`
      successfully; the app child aborted at HIServices before drawing.
      Integrated runtime behavior and rendered search acceptance are still
      unverified.
      Follow-up on 2026-10-14: the anonymous public npm registry was reachable
      from the explicitly network-enabled disposable smoke, despite DNS being
      unavailable in the restricted shell. The managed installer acquired
      Vtsls and BasedPyright into the isolated test cache, and TypeScript,
      JavaScript and Python completion/navigation/diagnostic/recovery checks
      passed. A complete rerun also installed HTML 0.3.2 from the live Zed
      catalog and passed Rust Analyzer formatting, completion, navigation,
      diagnostics/repair, unsaved-buffer recovery, restart and graceful-shutdown
      checks. No global language-server install or workspace database/schema
      change was made. The watcher remains the single active `just dev` loop;
      native rendered verification is still open because macOS aborts before
      drawing and CUA cannot attach to its window.
      On 2026-10-12, the rebuilt `target/debug/ahead` also passed
      `tests/agent-code-mode-smoke.mjs` and
      `tests/agent-file-attribution-smoke.mjs` with isolated temporary homes and
      loopback-only mock providers. The first verified managed Code Mode,
      delegated search, cancellation, retry after Turso reopen and shutdown.
      The second verified scoped AGENTS.md/skill context, managed edit to
      persisted Turso CodeAnchor, staged commit, AHEAD author, human committer,
      session trailer and anchor cleanup. Both scripts now pass only an
      allow-listed child environment rather than inheriting shell credentials.
      A further current-binary `tests/editor-recovery-smoke.mjs` run passed
      acknowledged Unicode-buffer recovery after proxy SIGKILL, live-owner
      protection, explicit-discard fencing across restart, and owner-only
      storage permissions, using isolated temporary homes and no model or
      language server. Its disposable project remains at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-recovery-smoke-h0gyuX`.
      These remain headless integration evidence, not rendered app acceptance.
      Re-ran on 2026-10-13 against the 12:50 `target/debug/ahead` binary:
      acknowledged Unicode recovery after proxy SIGKILL, live-owner fencing,
      discard persistence/stale-write fencing across restart, and owner-only
      storage permissions on create/reopen all passed. The isolated project is
      retained at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-recovery-smoke-jQJ2F6`.
      This remains proxy/storage evidence, not native recovery UI acceptance.
      Fresh current-binary recheck on 2026-10-13 ran
      `tests/editor-recovery-smoke.mjs` against `target/debug/ahead` (15:46,
      newer than the exercised proxy/agent storage sources). A project with no
      preexisting `.ahead/` initialized its Turso `session.db`; recovery,
      live-owner protection, discard fencing after restart and owner-only
      database permissions all passed. The temporary Python project and data
      home are isolated at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-recovery-smoke-6mi5Sz`.
      The source-current GPUI runner at 16:00 also passed
      `first_settings_load_bootstraps_private_workspace_config_once` (1/1),
      proving first Settings load creates private workspace settings and
      surfaces setup-required state. These verify bootstrap/storage services,
      not the native first-run screen; the schema remains the initial Turso
      schema with no version or migration checks.
      At 21:27 EDT, the current integrated binary passed the same recovery
      smoke in a new disposable Python project: acknowledged Unicode text
      survived proxy SIGKILL, a live owner could not be stolen, explicit
      discard stayed cleared across restart, stale writes were fenced, and
      the newly created/reopened Turso database remained owner-only. Fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-recovery-smoke-mXQ02o`.
      This is current proxy/storage evidence, not the rendered recovery UI.
      Also on 2026-10-13, the same binary passed
      `tests/agent-shutdown-smoke.mjs` with a disposable loopback mock:
      explicit shutdown and stdin EOF persisted partial streamed output as
      cancelled through Turso reopen; SIGKILL preserved failed output and
      instruction provenance without automatic replay, and explicit retry
      started exactly once. No credentials or external network were used.
      Native chat rendering remains open.
      Rechecked at 23:42 EDT on 2026-10-14 against the integrated
      `target/debug/ahead` rebuilt at 23:38: explicit shutdown and stdin EOF
      each preserved a partial streamed answer as cancelled through Turso
      reopen; SIGKILL preserved failed output and instruction provenance
      without replay, and explicit retry started once. The disposable
      loopback-only mock and isolated profile were removed on success. This
      confirms the managed session/storage lifecycle, not native chat rendering
      or authenticated-provider acceptance.
      The integrated binary also passed the full managed acquisition smoke
      with `AHEAD_LSP_INSTALL_MISSING=1` and
      `AHEAD_LSP_INSTALL_ZED_HTML=1`: Vtsls served TypeScript and JavaScript,
      BasedPyright served Python, Rust Analyzer served Rust, and the live Zed
      HTML extension 0.3.2 installed and completed HTML. The run covered
      completion, definitions, diagnostics/repair, semantic tokens, TypeScript
      quick fixes/import resolve/auto-import, Rust formatting/hover/signature/
      references/cross-file rename, unsaved-buffer replay across server restart,
      close/reopen and graceful shutdown. The isolated project, server caches
      and extension install remain at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-f6JO00`.
      This proves the real-server/proxy acquisition path, not native editor
      rendering or the installer screens.
      Follow-up recheck against the newly rebuilt binary on 2026-10-13:
      Node 24.15.0 and npm 11.12.1 are available, but the live Zed catalog
      request failed DNS resolution for `api.zed.dev`, and the separate managed
      TypeScript/Python run failed when npm could not resolve its registry
      (`ENOTFOUND` installing Vtsls). Rust Analyzer on PATH is only a rustup
      shim; the stable toolchain has no component by that name. These retries
      produced no new live-server pass; the Zed/npm network and usable Rust
      Analyzer remain external prerequisites for a fresh all-language run.
      Disposable failed npm attempt retained at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-CJz5wZ`.
      Its copied fixture then passed documented offline `just verify` against
      the Rust (1/1), TypeScript/JavaScript (2/2) and Python (1/1) examples.
      This validates the language projects without proving editor bootstrap,
      live LSP acquisition or native rendering.
      Rechecked offline on 2026-10-14: Node/npm are available, but `vtsls` and
      `basedpyright-langserver` are not on PATH; `rust-analyzer` resolves only
      to a rustup shim and is absent from the installed stable/nightly
      components. The isolated Rust LSP smoke therefore fails preflight before
      creating its fixture. No network or package installation was attempted;
      this was an offline preflight only, not a new live-server result.
      Later on 2026-10-14, a separate scoped network-enabled smoke against the
      integrated binary built at 10:55:54 installed live Zed HTML extension
      0.3.2 and npm-managed Vtsls/BasedPyright into isolated temporary data.
      HTML completion and the TypeScript/JavaScript/Python completion,
      navigation, diagnostic/repair, TS import-action, restart/recovery and
      graceful-shutdown checks passed. Its retained project is
      `/private/tmp/ahead-lsp-smoke-7qTnQc`; this did not include Rust Analyzer
      or native editor rendering. Detailed acquisition evidence is in the
      language-server checklist below.
      Re-ran offline against the rebuilt 2026-10-14 app using the retained
      temporary Vtsls/BasedPyright package cache: TypeScript and JavaScript
      completion, definition, diagnostics/repair, import actions, restart and
      recovery, and Python completion/navigation/diagnostics/repair all passed;
      both servers shut down cleanly. The disposable smoke is retained at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-BsML4U`.
      Rust Analyzer was not included because it is unavailable locally.
      A scoped network-enabled Rust-only run then verified AHEAD's own managed
      Rust Analyzer acquisition without changing the global toolchain. The
      current integrated binary downloaded the official release into the
      smoke's temporary AHEAD cache; formatting, completion, definition,
      semantic tokens, hover, signature help, references, cross-file rename,
      unsaved diagnostics/repair, close/reopen, restart/recovery and graceful
      shutdown all passed. Its disposable workspace/cache remains at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-oCGJsY`.
      On the same rebuilt binary, the managed Code Mode search/cancellation/
      retry-after-Turso-reopen smoke and the AGENTS.md/skill-to-CodeAnchor-to-
      staged-commit attribution smoke also passed with isolated loopback mocks.
      The authenticated `tests/agent-openrouter-smoke.mjs` turn streamed and
      returned its random fixture marker, but the initial run did not record
      which tools produced that answer. A diagnostic rerun on 2026-10-14
      observed three failed dynamic-tool calls and multiple shell-command
      calls; the requested edit landed without a Turso CodeAnchor. The test
      had omitted the editor's buffer-snapshot response, required by
      `file_search`; it now answers with an empty snapshot and requires
      successful managed `file_search`, no shell commands, and a persisted
      CodeAnchor. The corrected retry was rate-limited by OpenRouter with HTTP
      429 before turn completion, so it yielded no tool-path evidence; the
      disposable profile was removed. Wait for provider availability and rerun
      before accepting provider/tool/attribution behavior.
      On 2026-10-13, reran `just verify` in the active disposable project
      `/private/tmp/ahead-dev.fHD6JH/project`, with Cargo output isolated under
      that project's own `target/`: Rust 1/1, TypeScript/JavaScript 2/2 and
      Python 1/1 passed offline. This is fixture-language evidence only; the
      native bootstrap and rendered feature rows remain open.
      The source-current `ahead-agent` test runner's `native_client::tests`
      filter passed 52/52 with scoped localhost-only permission for mock model
      endpoints. Coverage includes managed tool registration, search over
      unsaved buffers, durable Turso history, MCP approval/forms, slash skills,
      subagent lifecycle, cancellation and shutdown. No external model service
      or provider credentials were used.
      Fresh 2026-10-13 rerun: `ahead_agent-0e1e93ffd7e62eb3` (built 11:54,
      newer than `native_client.rs` at 2026-10-12 14:36) passed all 52
      `native_client::tests` serially with scoped localhost mocks. This includes
      live-buffer search, provider settings, MCP approvals/forms/HTTPS consent,
      skills, durable history, child cancellation and graceful shutdown. No
      external provider, registry, or model credentials were used.
      Fresh 2026-10-13 `ahead-agent --lib` verification used the current root
      sources: all 166 runnable tests passed serially; 4 package/provider tests
      remain intentionally ignored. The first test-profile compile after the
      app build took 7m57s; the complete rerun from the shared Cargo target
      finished in 1.74s. Initial sandbox failures were limited to 14 ephemeral
      localhost binds; rerunning with only loopback permission passed all tests.
      No external network or credentials were used.
      Rechecked against the freshly rebuilt `target/debug/ahead` on 2026-10-12:
      `tests/agent-code-mode-smoke.mjs` passed managed Code Mode search,
      cancellation, retry after Turso reopen and shutdown;
      `tests/agent-file-attribution-smoke.mjs` passed scoped AGENTS.md/skill
      loading, edit-to-CodeAnchor persistence, staged commit metadata and
      anchor cleanup. Both use isolated synthetic projects and an in-process
      loopback mock provider. The same binary passed the cache-seeded
      `tests/lsp-smoke.mjs` matrix for TypeScript, JavaScript, Python and Rust,
      including navigation, diagnostics/repair, TypeScript import actions,
      Rust formatting/cross-file rename, restart recovery and shutdown. Its
      retained disposable project is
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-zwCbO2`.
      These are headless proxy/service acceptance checks, not native rendered
      UI or authenticated-provider acceptance.
      Fresh recheck on 2026-10-13 against `target/debug/ahead` built at 17:49:
      both mock-provider smokes passed with isolated projects and profiles.
      Code Mode verified streamed execution, delegated `file_search`,
      cancellation, retry after Turso reopen and shutdown. File attribution
      verified ancestor-only AGENTS.md plus selected `.agents/skills/` context,
      edit-to-Turso-CodeAnchor persistence, staged commit attribution and
      cleanup, with AHEAD as author, the human as committer, and the session
      trailer intact. No external model provider was contacted.
      Re-ran `tests/agent-code-mode-smoke.mjs` against the current rebuilt
      `target/debug/ahead` on 2026-10-13 with scoped loopback permission. It
      passed Code Mode execution, delegated file search, cancellation, retry
      after Turso reopen and shutdown using an isolated workspace and mock
      provider. This verifies managed-agent behavior, not schema compatibility
      or native UI rendering.
      Against the 11:11 integrated binary, reran the managed Code Mode smoke
      (delegated search, cancellation, retry after Turso reopen, shutdown) and
      `tests/editor-recovery-smoke.mjs` (Unicode recovery after SIGKILL, live
      owner fencing, durable discard, and owner-only database permissions); both
      passed in disposable workspaces. This exercises the working initial
      schema and recovery path, not schema compatibility or native UI rendering.
      Rechecked on 2026-10-12 with Node 24.15.0 and npm 11.12.1 available on
      `PATH`: fresh Vtsls acquisition failed because npm DNS returned
      `ENOTFOUND`. Fresh disposable-project reruns seeded from the earlier
      verified language-server cache passed TypeScript, JavaScript and Python;
      the full Rust/TypeScript/JavaScript/Python matrix also passed against the
      current `target/debug/ahead`. Coverage includes completion, definitions,
      diagnostics/repair, semantic tokens, TypeScript import fixes/auto-import,
      Rust formatting/hover/references/cross-file rename, close/reopen, restart
      recovery and graceful shutdown. The latest full-matrix fixture is retained
      at `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-VqaVuu`;
      fresh npm acquisition remains dependent on registry DNS, and this is not
      native rendered UI evidence.
      Fresh 2026-10-13 run against `target/debug/ahead` (17:29): cold managed
      acquisition stopped at Vtsls because npm DNS returned `ENOTFOUND`, before
      completing the fixture. Reused the previously verified disposable cache
      from `ahead-lsp-smoke-T8mhdS` with `AHEAD_LSP_CACHE_SEED`; the full
      TypeScript/JavaScript/Python/Rust matrix passed completion, navigation,
      diagnostics/repair, semantic tokens, TypeScript import actions, Rust
      formatting/hover/signature/references/cross-file rename, restart recovery
      and graceful shutdown. The new fixture is
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-mi9Ybz`.
      This confirms the current cached server/proxy path, not fresh acquisition
      or native editor rendering.
      Rechecked 2026-10-14 against the current `target/debug/ahead`: with
      scoped public npm access and package lifecycle scripts disabled, fresh
      managed acquisition passed the Rust/TypeScript/JavaScript/Python smoke,
      including navigation, completion, semantic tokens, diagnostics/repair,
      Rust formatting/rename, restart recovery and graceful shutdown. A second
      isolated run fetched Zed's live catalog, installed HTML 0.3.2 and verified
      HTML completion plus TypeScript/JavaScript/Python behavior. Fixtures:
      `ahead-lsp-smoke-k0kZUx` and `ahead-lsp-smoke-UJ5Ols`. These prove the
      managed proxy/catalog/server path, not rendered editor behavior. The
      unsandboxed registry fetch was scoped to these disposable fixtures; the
      default sandboxed attempt still failed at npm DNS (`ENOTFOUND`).
      The 17:40 `ahead_agent` test runner is newer than `adapters.rs` and
      `acp_client.rs`: all 14 non-ignored adapter tests and all 43 ACP client
      tests pass, covering curated catalog/install state, per-agent config
      defaults, selected model/config option updates, cancellation, and the
      editor MCP bridge. The opt-in packed-npm launch test also passed in its
      explicit offline mode using a disposable local package. Five socket tests
      first hit sandbox `PermissionDenied`; the rerun with only ephemeral
      localhost permission passed. Four installed-agent/provider acceptance
      tests are ignored by default; the local packed-npm launch case was
      explicitly enabled and passed. The actual pinned Pi ACP package also
      passed offline-session/model-selection persistence across restore with
      `PI_OFFLINE=1` and a deliberately unreachable local model endpoint. A
      separately enabled initialize-only smoke installed the ACP wrappers in
      temporary caches and successfully initialized Pi, Codex, and Claude Code
      against isolated homes with no inherited credentials or model turns.
      On 2026-10-13, reran three ACP protocol-fixture tests from a runner newer
      than `acp_client.rs`: advertised model selection, applying saved agent
      defaults, and publishing live option updates all passed (3/3). This
      verifies generic option wiring, not authenticated Codex/Claude turns.
      No provider call was made in this initialization-only run; a later
      authenticated Pi turn is recorded below.
      Rechecked on 2026-10-13 with `ahead_agent-0e1e93ffd7e62eb3` (14:54,
      newer than `adapters.rs`): Pi offline model selection/resume and the
      curated Pi/Codex/Claude initialization test both passed (2/2). The
      initial sandboxed npm attempt failed with `ENOTFOUND`; scoped public npm
      access passed with lifecycle scripts disabled. Each CLI used an isolated
      temporary home, and no provider prompt, credential, or project data was
      used.
      Host prerequisite check on 2026-10-14: `pi --version` reports 0.87.1
      (above AHEAD's 0.81 minimum), `claude --version` reports Claude Code
      2.1.248, and `codex --version` reports codex-cli 0.152.0. These local
      version checks do not claim a live authenticated ACP turn.
      The source-current 11:00 `ahead_agent` runner (newer than
      `adapters.rs`) passed all 16 runnable curated-adapter tests; four stayed
      ignored by default. The ignored
      `npx_generations_launch_locally_packed_packages_offline` test was also
      explicitly run and passed (1/1) with Node/npm and a disposable local
      package, without registry or model-provider access. Reviewing Zed's
      `agent_registry_ui.rs` and `LocalRegistryNpxAgent::get_command` confirms
      the same lifecycle: registry install records settings, while npm package
      installation occurs at first launch. AHEAD's registry marker plus lazy
      package install matches that behavior. This does not verify a live
      registry refresh or provider-backed agent turn.
      The same source-current 11:00 `ahead_agent` runner passed the complete
      library suite on 2026-10-13: 166 passed, 4 provider/npm/installed-agent
      cases ignored. Its local mock-socket tests passed with ephemeral loopback
      permission; no external ACP registry or model provider was contacted.
      Fresh rebuilt rerun on 2026-10-13: the complete `ahead-agent --lib` suite
      again passed 166 tests with 4 ignored. The first sandboxed run had 14
      failures, all before assertions at local mock-server binds; enabling only
      local loopback access made the entire suite pass in 26.70s. No external
      ACP registry or model provider was contacted.
      Rechecked on 2026-10-14 with the shared dev watcher stopped: the complete
      source-current `ahead-agent --lib` suite passed serially (166 passed, 4
      ignored, 26.17s). Two child-start timing failures from the parallel run
      also passed individually and in this serial run; they appear load-sensitive.
      Restarted `just dev` on the disposable project; the integrated build passed
      in 2.39s, but native startup again aborted at macOS HIServices before
      creating a window. CUA confirms the Mac is locked, so rendered acceptance
      remains open.
      The source-current 11:10 `ahead_app` runner passed 193 focused GPUI tests
      serially: code panel (61), session panel (54), settings (29), terminal
      (10), terminal panel (14), workspace/search/Git (15), and extensions
      gallery (10). These cover editor, model-picker, terminal, search and
      install flows at the component/test level, not native rendered
      acceptance.
      On 2026-10-13, the 11:11 integrated `target/debug/ahead` binary passed
      `AHEAD_LSP_LANGUAGES=typescript,python AHEAD_LSP_INSTALL_MISSING=1
      AHEAD_LSP_INSTALL_ZED_HTML=1 rtk node tests/lsp-smoke.mjs`: AHEAD fetched
      the live Zed catalog, installed HTML 0.3.2 and completed `<di`, acquired
      Vtsls/BasedPyright in the fixture's isolated cache, then passed TypeScript,
      JavaScript and Python completion, navigation, diagnostics/repair, semantic
      tokens, import fixes, reopen/restart recovery and graceful shutdown. The
      disposable project is
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-aKhGzu`.
      This is live catalog and real-server proxy acceptance, not native gallery
      installation/rendering or authenticated agent-chat acceptance.
      The 10:32 source-current app test runner (newer than `code_panel.rs` at
      10:31) passed all 7 `lsp_snippet::tests` and all 11 completion-filtered
      GPUI tests, including snippet navigation/editing, Unicode edit ranges,
      auto-import Undo, and popup placement. A fresh Rust-only LSP smoke found
      the PATH `rust-analyzer` is a rustup shim without the installed component;
      AHEAD's managed acquisition then failed to resolve GitHub release metadata
      (`api.github.com`, DNS). The disposable fixture remains at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-d7VwSj`;
      current Rust real-server acceptance therefore still depends on an
      available Rust Analyzer binary or registry network.
      A current `ahead-app` test runner rebuilt after `workspace_panels.rs`
      changed: the focused GPUI workspace/search/Git group passed 15/15. Its
      full serial run passed 319/322. The only remaining failures were the two
      shared-session invite tests (invite missing from join input; multiplayer
      is a separate milestone) and one PTY Ctrl-C startup timing assertion. The
      terminal case passed six consecutive isolated reruns. These are headless
      app results, not native hit-testing or rendered acceptance.
      Rechecked with the app test runner rebuilt for the 2026-10-13 Add Comment
      affordance change: all 61 `code_panel::tests`, 54 `session_panel::tests`,
      and 29 `settings_panel::tests` passed serially. The same runner also
      passed 35 terminal/PTY, 15 workspace/search/Git and 11 Extensions tests,
      205 targeted tests total. This re-covers the comment action, completion
      ranking/placement, terminal clipboard/paste, session start controls,
      slash filtering, provider/model search, per-provider visibility, search
      modes, Git rows and gallery selection; it remains headless evidence, not
      native visual acceptance.
      On 2026-10-14, the integrated binary at 10:55 (newer than the exercised
      agent, proxy and storage sources) passed four disposable current-binary
      smokes with an allow-listed environment and no external provider calls:
      Code Mode execution/delegated `file_search`/cancel/retry after Turso
      reopen; scoped root/source `AGENTS.md` plus selected `.agents/skills/`
      context and edit-to-CodeAnchor-to-staged-commit attribution; explicit
      shutdown, stdin EOF and SIGKILL recovery of partial streamed turns with
      explicit-only retry; and Unicode editor recovery, live-owner fencing,
      durable discard/stale-write fencing and private initial-store permissions.
      The recovery fixture is retained at
      `/private/tmp/ahead-recovery-smoke-SMPWxz`. These prove proxy, managed
      agent, attribution and storage behavior, not the native rendered UI.
      The same integrated binary also passed the LSP shutdown smoke in all
      four modes (normal shutdown, stdin EOF, initializing and unresponsive
      server), preserving unsaved source and reaping each child before proxy
      exit; the DAP lifecycle smoke passed running, initializing and stderr-
      backpressure modes with adapter cleanup before proxy exit. These used
      disposable roots `/private/tmp/ahead-lsp-shutdown-BArSVv` and
      `/private/tmp/ahead-dap-shutdown-ufKUsc`, an allow-listed environment,
      and no external services.
      The refreshed 2026-10-14 `ahead-app` test runner (10:39, newer than the
      latest app sources exercised) completed 321/323 serial GPUI tests. The
      only failures were the two shared-session lifecycle tests: the test
      guest lacked the expected pasted invite JSON, and one host additionally
      observed its guest exit before revocation. The other editor, chat,
      settings/model-picker, search/Git, terminal and extension tests passed.
      Collaboration remains a separate milestone; this is not native render
      acceptance.
      On 2026-10-14, the previous dev-session handle was missing and this
      repository's `just dev` lock was absent. CUA lists AHEAD Dev as running,
      but attachment by bundle ID and bundle path times out. `lsof` shows
      multiple processes mapped to the dev bundle and at least one has a
      non-disposable home `.ahead/session.db` open. Do not attach to, kill, or
      launch another app copy until the user confirms an isolated native
      process; native visual acceptance remains open.
      Safe isolated restart after the source-current GPUI suite: `just dev`
      rebuilt the integrated app in 2.59s against the same disposable
      `/private/tmp/ahead-broad-e2e.ikS0ar/project` with
      `AHEAD_DATA_HOME=/private/tmp/ahead-broad-e2e.ikS0ar/data`. The app again
      aborted before rendering during macOS HIServices XPC initialization
      (signal 6); watcher session 7363 remains live. The temporary data path
      had not existed before restart and no app window became available.
      Follow-up on 2026-10-14: the shared watcher rebuilt current sources in
      11.19s and its app child again aborted before rendering with the same
      HIServices XPC signal 6; the watcher handle remains live. Against that
      rebuilt `target/debug/ahead`, the disposable managed Code Mode smoke
      passed delegated `file_search`, streaming, cancellation, retry after
      Turso reopen and shutdown. The opt-in live LSP acquisition smoke fetched
      the Zed catalog, installed HTML 0.3.2, and acquired Vtsls, BasedPyright
      and Rust Analyzer only inside its temporary cache. TypeScript,
      JavaScript, Python and Rust checks passed for completion/navigation,
      diagnostics/repair, semantic tokens, supported formatting/rename and
      restart/reopen recovery; all four fake-server shutdown modes also passed.
      The 240 MiB LSP fixture and the shutdown fixture were inspected and
      removed; no global server/toolchain install was made. These are current
      managed-runtime and real-server proxy results, not native UI rendering.
      No authenticated provider turn ran: `OPENROUTER_API_KEY` is not set in
      this process environment, so provider-backed tool and CodeAnchor
      attribution acceptance remains open. With the watcher stopped, the same
      disposable project's documented `just verify` passed offline using its
      own `target/`: Rust 1/1, TypeScript/JavaScript 2/2 and Python 1/1. The
      watcher was restarted on the same project and isolated data home as
      session 46782. Its integrated build passed in 2.64s, then the app child
      exited before drawing on the macOS HIServices XPC signal 6; the watcher
      remains live.
      Rechecked with the source-matched 20:23 app runner on 2026-10-12,
      skipping only those two collaboration tests: 319/320 passed; the sole
      failure again timed out after three seconds waiting for the PTY foreground
      process marker (`terminal_panel.rs:2975`). The exact test passed alone
      and in ten sequential GPUI seed runs. This points to a possible
      suite-load-sensitive test synchronization issue. Follow-up: increased
      the real PTY startup-marker deadline to ten seconds; timeout diagnostics
      now include terminal scrollback. The focused test passed from the new runner; the
      source-current serial app suite then passed 320/320 with only those two
      collaboration tests skipped. This changes test synchronization only.
      First close the core gates for the managed agent/provider/chat flow,
      apply-to-CodeAnchor-to-commit, Turso-backed history and memory, curated
      ACP agents (Pi when valid provider access is available), and pluggable
      LSP/extensions, alongside the editor, search, Git and terminal workflows.
      New managed-provider blocker (2026-10-14): the live OpenRouter model's
      synthetic edit run recorded three failed dynamic-tool calls and shell
      command calls; the requested source edit landed without a CodeAnchor.
      The smoke had omitted the `agent_buffer_snapshots_requested` response
      required by `file_search`; the test now supplies an empty editor snapshot
      and strictly verifies successful `file_search`, no shell commands, and
      the expected `fileChange` event. Keep agent-command file effects
      attributable as well. The corrected live retry hit OpenRouter HTTP 429
      before turn completion. After provider availability returns, a fresh run
      must persist one session-owned CodeAnchor and pass the staged-commit
      attribution checks before this provider path is accepted.
      Retried on 2026-10-14 at 14:22 EDT against the 14:17 integrated binary;
      OpenRouter again returned HTTP 429 after its retry limit, so the strict
      end-to-end acceptance assertions did not complete. This remains an
      external provider-availability blocker, not a passing managed turn.
      Rechecked on 2026-10-14 against `target/debug/ahead` (13:08, newer than
      `native_client.rs` at 13:06 and `session.rs` at 12:49): the local-fake
      `agent-code-mode-smoke.mjs` passed delegated search, streaming,
      cancellation, retry after Turso reopen and shutdown; the local-fake
      `agent-file-attribution-smoke.mjs` passed a managed Code Mode edit through
      persisted session CodeAnchor, staged Git commit, AHEAD author, human
      committer, session trailer and anchor cleanup. These verify the managed
      code path without an authenticated provider turn; the OpenRouter 429 and
      live-provider acceptance remain open.
      Zed reference checked for this boundary: `../zed/crates/agent/src/tools.rs`
      exposes dedicated edit/write/terminal tools, and
      `../zed/crates/agent_ui/src/agent_diff.rs` builds its diff from
      action-log changed buffers. Use those as implementation references; they
      do not establish that AHEAD's shell edits are currently attributed.
      On 2026-10-14, the authenticated OpenRouter smoke completed a real
      managed turn and `file_search`, but the model made the requested edit via
      two Core `commandExecution` calls despite instructions to use
      `apply_patch`. The disposable file changed, then the smoke correctly
      failed its no-shell assertion before checking anchors or committing; this
      proves tool-choice guidance is not enforcement, but does not establish
      whether those shell edits were attributed. Resolve shell-write
      attribution/enforcement and rerun the complete authenticated
      CodeAnchor-to-commit acceptance before calling this gate complete.
      Late on 2026-10-14, retried the strict smoke with the user-authorized
      free OpenRouter model against the current integrated binary. The normal
      command sandbox blocked nested macOS `sandbox-exec` with
      `sandbox_apply: Operation not permitted`; a harmless nested-sandbox probe
      reproduced the same host limitation. Under the approved escalated test
      process, AHEAD's own Seatbelt launch proceeded, but OpenRouter returned
      HTTP 429 after retries (request ID `a4ab6b5f7de93320-ATL`) before a
      completed agent turn. No tool/edit/CodeAnchor/commit assertion passed;
      the test's temporary project and credential settings were removed.
      A later offline mock rerun initially appeared to expose a shell-write
      regression, but its disposable workspace was under `/private/tmp`, which
      the macOS process compatibility policy explicitly allows. Re-running
      `tests/agent-file-attribution-smoke.mjs` with its workspace under the
      repository's ignored `target/` directory passed: source writes were denied,
      generated-output writes and `cargo check` succeeded, then Code Mode
      persisted a session CodeAnchor and the staged commit verified AHEAD author,
      human committer, session trailer and anchor cleanup. This is local mocked
      provider evidence only; the authenticated OpenRouter acceptance remains
      blocked by HTTP 429. Do not disable AHEAD's sandbox.
      On 2026-10-15, rerunning that attribution smoke under `/private/tmp`
      reproduced the source-write escape. The cause was confirmed in both
      macOS Seatbelt layers: process scratch defaults and the managed profile's
      `TMPDIR`/`/tmp` writable roots overlapped the disposable workspace. The
      policy now excludes the active workspace (including its canonical path)
      from those scratch roots while preserving separately authorized paths;
      a new Seatbelt regression requires `src` writes to fail and `target`
      writes to succeed. The integrated binary rebuilt at 09:41 and the full
      attribution smoke passed again with its workspace under `/private/tmp`,
      including Code Mode edits, Turso CodeAnchor, and staged commit metadata.
      On 2026-10-15, the focused scratch-exclusion regression and all 82
      `codex-sandboxing` tests passed with real Seatbelt available; the current
      integrated binary also passed the complete `/private/tmp` attribution
      smoke. A subsequent source-current agent test exposed a separate
      policy-layer gap: the root-wide read grant did not shadow a broader
      `TMPDIR` write grant when the workspace lived underneath it. The managed
      profile now also declares the workspace read-only, with deeper explicit
      generated-output grants. The current 167-passing agent suite checks both
      temp-directory spellings, canonical workspace aliases, sibling scratch
      writes and read-only mode; the 257-test proxy suite also passes. The two
      ACP presentation tests compile with their missing test-only action import
      restored. After rebuilding through the same `just dev` loop, the complete
      `/private/tmp` attribution smoke passed again with the new managed
      profile: source-write denial, generated Cargo output, Code Mode patch,
      Turso CodeAnchor and staged Git commit attribution/cleanup. Native
      rendering and authenticated-provider acceptance remain
      open. A supplemental `/tmp`-spelling probe also
      denied the unauthorized shell source write, but its subsequent bridged
      `tools.apply_patch` could not write through `/tmp`: this test task grants
      `/private/tmp`, not the symlink spelling, as a writable host root. That
      partial harness run is not full alias acceptance; the canonical-path
      run remains the end-to-end result.
      Keep advanced multi-file editing, PR review, guided debugging, voice and
      multiplayer as separate milestones instead of implicit release gates.
      Use `../zed` as the primary behavior reference and retain useful Codex
      runtime implementations; prune only proven-unused code. User reaffirmed
      2026-10-13: pre-production database work is to make one final initial
      Turso schema work; do not spend readiness effort on schema checks or
      compatibility. At the time, this superseded the earlier one-time legacy
      import exception: do not add schema-version markers, compatibility
      checks, migration machinery, compatibility aliases, legacy rollout
      imports or live JSONL fallback. Disposable pre-production data may be
      recreated when the initial schema changes; defer formal schema checks
      and version policy until production readiness. The active 2026-10-20
      scope later restored only the already-scoped safe one-time local rollout
      import: it runs only when a Turso thread header is missing and adds no
      schema migration, version checks, aliases or live file fallback.
      Keep technical docs and TODO current
      as work lands. Leave screenshot-backed user documentation until feature
      verification is complete.
      Fresh clean-build acceptance on 2026-10-14: after the user-requested
      `cargo clean`, the single shared `just dev` loop rebuilt `ahead` and
      `ahead-code-mode-host` from scratch in 12m22s. Compilation succeeded;
      the app child again aborted before its first frame with the macOS
      HIServices XPC connection-invalid error (signal 6). Against this fresh
      binary, the local-fake Code Mode and file-attribution-to-staged-commit
      smokes passed, as did acknowledged Unicode recovery, owner/discard
      fencing, and owner-only Turso database creation/reopen. The live Zed
      catalog installed HTML 0.3.2 into a disposable cache; Vtsls, BasedPyright
      and Rust Analyzer passed the TypeScript, JavaScript, Python and Rust LSP
      matrix for completion/navigation, diagnostics/repair and recovery,
      including HTML completion, TypeScript auto-import, and Rust formatting
      and cross-file rename. All checks are runtime/proxy evidence, not native
      rendering or authenticated-provider acceptance. The OpenRouter retry
      remains unverified; no provider request ran in this fresh pass.
      At 23:42 EDT, the integrated binary rebuilt at 23:38 also passed
      `tests/agent-code-mode-smoke.mjs`: a local mock completed Code Mode,
      eight concurrent delegated `file_search` calls returned independently,
      cancellation prevented late snapshot replies/model continuation, and an
      explicit retry after Turso reopen completed with its search and cell
      results intact. This revalidates the managed tool/session path on the
      latest binary without authenticating to an external provider.
      The fresh serial `ahead-app --lib` GPUI/unit run completed 321 tests;
      its two failures are collaboration-specific session-invite flows
      (`separate_ahead_processes_share_code_and_chat` and
      `two_authenticated_shells_join_a_direct_shared_session`), both failing
      to expose the invite JSON prompt. Collaboration is tracked separately and
      is not a core release gate; do not count these failures as verified join
      behavior.
      On 2026-10-15, a fresh clean build through the shared `just dev` loop
      completed in 10m46s; the app child again aborted before its first frame
      with the macOS HIServices XPC connection-invalid error. The rebuilt
      binary passed the live Zed-catalog/LSP smoke across HTML, TypeScript,
      JavaScript and Python, and the mocked-provider file-attribution smoke
      verified source-write denial, generated-output writes, Code Mode edits,
      Turso CodeAnchor persistence, and staged commit metadata/cleanup. A
      focused model-picker GPUI test also passed discovery-on-open, refresh,
      filtering and provider selection; the fresh-terminal Justfile recipe
      regression also passed, verifying distinct focused terminals, the exact
      selected Justfile, recipe output and task-based tab title. These are
      integration/unit results; native rendered UI and authenticated-provider
      CodeAnchor acceptance remain unverified.
      Workspace evidence (2026-10-07, rechecked after the runtime prune):
      `cargo metadata --locked --offline --no-deps --format-version=1` reports
      the repository root as the workspace with 68 members, including the
      retained runtime source and AHEAD-owned runtime seam crates; no nested
      `ahead-agent-engine/Cargo.toml` exists. Metadata alone does not
      compile the workspace. A locked/offline dependency-closure query reaches
      all 54 retained runtime-source members from at least one non-runtime
      workspace member; this finds no manifest-level orphan, but does not prove
      every internal feature is product-required. The storage-dependency scan
      finds only `ahead-proxy`'s `libsql` client; native replay and agent-graph
      persistence are adapters over that Turso store. This is source/manifest
      evidence, not fresh end-to-end storage verification.
      Rechecked on 2026-10-12 using locked/offline no-dependency Cargo metadata:
      the root still has 68 packages, and all 54 retained runtime-source
      packages are reachable from `ahead-agent` or the Code Mode host/runtime
      roots. `cargo machete` reports no unused declared dependencies. The
      current source scan still finds one agent/editor/proxy search engine in
      `ahead-core::search` and no retained Codex `GrepFiles` tool. This does not
      establish internal symbol-level dead-code reachability.
      Rechecked 2026-10-13 against the current dirty pruning tree: locked,
      offline Cargo metadata still reports one root workspace with 68 packages;
      `ahead-agent-engine` contains 53 package manifests and no nested
      workspace root, alongside the three AHEAD runtime seam crates. The
      integrated `just dev` build compiled this tree in 2.69s, and
      `cargo machete` found no unused declared dependencies. This verifies
      manifest/build integration, not that each retained runtime symbol is
      product-required; the native app still aborts before rendering.
      Rechecked 2026-10-14 against the current dirty manifests:
      `cargo machete` again found no unused declared dependencies. This does
      not prove internal symbol-level dead-code reachability.
      Rechecked 2026-10-15 after the Cargo layout move: the repository root
      now owns one 67-member workspace. All 52 packages under
      `ahead-agent-engine/` are in the production normal/build dependency
      closure of `ahead`; four AHEAD-owned support packages now sit at the
      workspace root. External ACP remains in `ahead-agent`. The focused
      locked/offline `ahead-agent` library check passes. The main `ahead`
      package also passes locked/offline Cargo check. A direct Code Mode check
      initially failed because it skipped `scripts/ensure-code-mode-v8.sh`
      and requested the sandboxed archive from the default denoland release,
      which returned HTTP 404. The repository's setup script downloads the
      pinned archive and bindings from OpenAI's release and verifies both
      SHA-256 checksums. With that setup sourced, the Code Mode host and
      runtime pass `cargo check --locked`. A rendered-app check remains open.
      The copied zsh-fork escalation path was disabled by default and required
      patched zsh and an execve wrapper that AHEAD does not package or inject.
      Removed its feature, shell-mode branches, helper-readable-root exception,
      approval DTOs, unused snapshot PATH replay, and the
      `codex-shell-escalation` Cargo package. The normal unified exec path and
      external ACP client remain. A locked/offline combined check of `ahead`,
      `ahead-code-mode-host`, and `ahead-code-mode-runtime` passes after this
      reduction. A locked/offline linked build of the editor and Code Mode host
      binaries also passes; the only linker diagnostic is about the large
      `__eh_frame` section's compact unwind encoding. Earlier notes below
      about retaining `shell_zsh_fork` and `codex-shell-escalation` describe
      the old tree.
      Removed copied vendor/config/documentation files only after confirming
      they are outside the Cargo and runtime dependency paths. The compiled
      closure does not establish which internal engine symbols could still be
      pruned safely.
      A follow-up `cargo shear --locked --offline` pass found an unused root
      `time` dependency declaration, now removed. The later zsh-fork reduction
      removed three more unused declarations: `socket2` at the root and
      `codex-utils-absolute-path` and `tracing` in `codex-tools`. The rerun has
      no dependency errors; its ten warnings concern doctest targets without
      doctests and retained upstream library tests disabled pending AHEAD-owned
      coverage.
      A focused `cargo clippy --locked --offline -p ahead-agent --lib` previously
      reached seven `ahead-rpc` `collapsible_if` errors under the current
      toolchain; those seven nested `if` blocks in `ahead-rpc/src/file.rs` and
      `ahead-rpc/src/ahead.rs` are now collapsed in source.
      A source scan found no include, reader or path reference for six copied
      Core prompt templates and two built-in agent role TOMLs. Their contents
      matched the original Git blobs, so the eight orphan assets were removed;
      Core's referenced `include_str!` assets remain.
      `cargo fmt --all --check` initially identified formatting-only drift in
      three AHEAD runtime-auth files; after formatting them, the workspace
      check passes. The copied engine formatter's unsupported
      `imports_granularity` setting was removed, so stable rustfmt no longer
      warns about it. The earlier shared `just dev` rebuild passed in 28.43s;
      the app again aborted before drawing at HIServices initialization.
      A 2026-10-09 source-only recheck across all workspace manifests still
      finds `libsql` as the only declared SQL client; the lockfile has no
      `rusqlite` or `sqlx` package. `NativeClient` injects `TursoThreadStore`
      into Codex Core, whose append path calls the AHEAD `HarnessStore` and
      persists replay rows through the libSQL/Turso store. Production managed
      history has no local rollout-file reader, importer or writer; thread
      restore uses Turso rows only. This confirms source wiring, not a fresh
      database integration run. Rechecked on 2026-10-21: workspace manifests
      declare `libsql` as the only SQL client, and `Cargo.lock` contains no
      `rusqlite`, `sqlx` or `libsqlite3-sys`; this is dependency evidence, not a
      runtime database acceptance test.
      Rechecked on 2026-10-12: startup runs one unversioned transactional
      initial-schema DDL using `CREATE ... IF NOT EXISTS`; there is no schema
      inspection, version marker, migration runner or compatibility path.
      Existing table definitions are not reconciled. The source change compiled
      in the shared integrated dev build. Its seven-test `session_storage_`
      filter passed, including create/reopen and preservation of existing rows
      while ensuring initial AHEAD tables. Recreate disposable pre-production
      data if the initial schema changes; no remote Turso Cloud request was made.
      Later on 2026-10-12, the existing `ahead_proxy-e859a7c5f85b903d` test
      runner (14:47, newer than `store.rs` at 14:29 and
      `ahead-agent/src/store.rs` at 14:28) passed the initial-schema
      create/reopen test, all 16 `memory_` tests, and the file-backed agent
      runtime-state reopen test (18/18). These direct tests used temporary
      embedded Turso/libSQL stores; no Cargo build or remote Turso Cloud
      request was involved.
      Fresh direct runs on 2026-10-09 used
      `ahead_proxy-028b8cd5478b5060` (11:44:07, newer than `host.rs` and
      `store.rs`): all 36 `ahead::host::tests` passed, covering durable memory,
      workspace instruction context and session lifecycle. The 41-test store
      filter initially passed 39; its two spawned-agent tests failed only
      because sandbox policy denied their local fake-model listeners. Rerunning
      those exact cases with loopback permission passed both, so all 41 store
      cases passed across the runs. This exercises disposable embedded
      libSQL/Turso storage and reopen behavior, not remote Turso Cloud.
      On 2026-10-09, reran the managed Code Mode and file-attribution smokes
      directly against `target/debug/ahead` (12:42) with isolated temporary
      homes and loopback-only fake model servers. Both passed: delegated search,
      continuation, cancellation of a pending editor snapshot, Turso reopen
      and retry. The latest file-attribution run invoked `/calendar` from the
      project's `.agents/skills/` and verified its `SKILL.md` body plus
      ancestor-only `AGENTS.md` context, managed patch, Turso CodeAnchor,
      `ahead` author/human committer, session trailer and anchor cleanup. The
      default sandbox denied the initial Code Mode listener; the
      local-only rerun passed without external requests or credentials. Both
      successful scripts removed their disposable projects. This remains
      mock-provider/headless evidence at that point, not rendered acceptance.
      Later on 2026-10-09, `tests/agent-openrouter-smoke.mjs` passed against the
      12:42 `target/debug/ahead` binary using the user-provided
      `nvidia/nemotron-3-ultra-550b-a55b:free` connection. The turn completed
      with an observed `streaming` conversation row; Code Mode `file_search`
      returned a random value that had not been placed in the prompt. The test
      used a fresh trusted workspace and isolated Turso, runtime and user homes;
      its API key lived only in a mode-0600 temporary settings file, was not
      inherited by the proxy process, and the temporary root was removed.
      Only synthetic test data was sent. This proves one authenticated managed
      Responses turn plus streaming and delegated tool execution, not native
      chat UI or real-provider cancellation/resume.
      On 2026-10-12, the 09:57 `just dev` binary also passed the disposable
      `tests/agent-code-mode-smoke.mjs` and
      `tests/agent-file-attribution-smoke.mjs` runs. The first verified a Code
      Mode cell, delegated search, cancellation, retry after Turso reopen and
      shutdown. The second verified managed edit -> Turso CodeAnchor -> staged
      Git commit, including AHEAD author, human committer, session trailer and
      anchor cleanup. Both used a loopback fake model and isolated temporary
      homes; they do not replace a current authenticated-provider turn or
      rendered native chat/editor acceptance.
      Revalidated on 2026-10-12 against `target/debug/ahead` (built
      2026-10-11 16:21) with the two same disposable fake-model smokes. Code
      Mode cell execution, delegated search, cancellation, retry after Turso
      reopen and shutdown passed. Managed edit-to-CodeAnchor-to-Git commit also
      passed with the AHEAD author, human committer, session trailer and anchor
      cleanup verified. These remain proxy/headless checks, not authenticated
      provider or rendered native chat/editor acceptance.
      On 2026-10-12, the freshly rebuilt 11:39 `target/debug/ahead` passed
      `tests/lsp-smoke.mjs` in a disposable fixture seeded from already-cached
      Rust Analyzer, Vtsls and BasedPyright packages; no servers were installed
      or downloaded. Rust, TypeScript, JavaScript and Python passed completion,
      navigation, semantic tokens, diagnostics/repair and close/reopen checks;
      Rust also passed hover, signature help, references, cross-file rename and
      formatting, and TypeScript passed quick-fix and auto-import resolution.
      Server restart/recovery and graceful proxy shutdown passed. This is
      source-current real-server proxy evidence, not native popup/insertion or
      Problems-view rendering; the retained fixture is
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-9xOH64`.
      Revalidated against the 15:54 `target/debug/ahead` binary on 2026-10-12
      using the already-installed Zed Vtsls, BasedPyright and Rust Analyzer
      executables, without modifying their cache or downloading servers. The
      complete Rust/TypeScript/JavaScript/Python matrix, including TypeScript
      import actions, Rust cross-file rename, unsaved diagnostics replay after
      server restart and graceful process-group shutdown, passed again. The
      disposable workspace is `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-bIiLKE`.
      This validates the real proxy/server protocol path; native completion
      popups, insertion and Problems-view rendering remain unverified.
      Rechecked on the freshly rebuilt 2026-10-12 `target/debug/ahead` after
      the current editor changes: the full Rust/TypeScript/JavaScript/Python
      smoke passed using `AHEAD_LSP_CACHE_SEED` from a retained disposable
      cache, including cross-file rename, import actions, unsaved diagnostics
      replay after server restart and process-group shutdown. The new fixture
      is `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-kZTrck`.
      No servers were downloaded in this run. A preceding cold managed install
      attempt failed on npm DNS `ENOTFOUND` for Vtsls, so fresh online server
      acquisition remains unverified; native popup/insertion rendering also
      remains unverified.
      Attempted the live Zed catalog and managed-install path in its explicit
      disposable mode on 2026-10-12; it stopped before creating the fixture
      because DNS resolution for `api.zed.dev` failed (`ENOTFOUND`). No server
      or extension package was downloaded. The live registry/install acceptance
      still needs a network-capable run.
      The 14:15 app test runner, newer than `extensions_panel.rs` (10:58),
      passed all 10 `extensions_panel::tests` without network access. This
      covers Zed catalog query construction and compatibility filtering,
      AHEAD-feed hash/host validation, version-pinned install dispatch and
      offline installed-extension state; it does not verify release-feed
      publication or a live package download.
      Rechecked during the 2026-10-12 goal continuation: the same source-current
      runner passed all 10 `extensions_panel::tests`, and
      `node --test scripts/package-ahead-extensions.test.mjs` passed 4/4
      publisher checks, including pinned repository/submodule validation and
      tamper rejection. `../zed` is clean at the exact revision in
      `extensions/sources.json`; its `target/debug` directory and the pinned
      `zed-extension` executable are absent. No package was built or published.
      The checked-in AHEAD feed is still empty, direct `api.zed.dev` access is
      still limited to the development preview pending endpoint-terms review,
      and live release-catalog installation remains open.
      Rechecked on 2026-10-12 using the same app runner: all 61
      `code_panel::tests`, 29 `settings_panel::tests`, 10 `terminal::tests`,
      14 `terminal_panel::tests` and 15 `workspace_panels::tests` passed
      serially without Cargo. The runner is newer than these sources but
      predates the current `session_panel.rs` and `threads_panel.rs`; this
      does not verify those newer chat/sidebar paths or native rendering.
      The 14:53 `ahead_agent` test runner (newer than all `ahead-agent/src`
      files) passed the full serial suite: 149 passed, 15 failed, and 3 were
      ignored. Fourteen failures stopped at mock loopback binds with `EPERM`
      before assertions; the parent/child shutdown case likewise failed while
      binding its mock model. The remaining session-startup test checked
      `anyhow::Error::to_string()` for an inner shutdown cause, but that returns
      only the outer context. Its assertion now checks and prints the full
      error chain. After pausing the watcher, the focused locked/offline Cargo
      `session::tests::shutdown_interrupts_session_startup_and_persists_active_turn_cancellation`
      rerun passed (1/1). Restarted the shared `just dev` loop as session 20106
      against `/private/tmp/ahead-broad-use.0XxogT/project` with the existing
      isolated data home. Its warm build passed in 3.36s; macOS GUI startup
      again aborted with the HIServices XPC error (signal 6). The watcher stays
      live; native rendering remains unverified. CUA still reports
      `io.ahead.dev` as running but attachment timed out, and no crash report
      newer than 2026-10-08 appeared. The fake-provider smoke was denied at
      loopback bind before creating a fixture. No external request was made.
      The source-current 319-test app runner was then run serially without Cargo:
      316 passed, two collaboration tests and one terminal Ctrl-C timing test
      failed. The terminal case passed when rerun alone. The two collaboration
      tests failed at their mock localhost binds in the initial run; with local
      loopback permission they advanced but still failed to read the session
      invite from the clipboard. Multiplayer remains a separate milestone, not
      a core broad-use release gate; no collaboration code was changed here.
      Rechecked on 2026-10-12 with the 10:59 app test runner, newer than every
      `ahead-app/src/*.rs` file: the serial full suite ran 322 tests, with 320
      passing and only the same two cross-process collaboration tests failing
      before their assertions because sandbox policy denied localhost binds.
      The full run passed all seven terminal clipboard/PTY integration tests,
      including copy, paste, shell-safe path paste, OSC 52 and Ctrl-C. Exact
      Ctrl-C reruns passed five times alone and in a subsequent clean group
      run. No Cargo build occurred; native UI and OS pasteboard behavior remain
      separate from this GPUI/headless evidence.
      Current 2026-10-12 rebuild: the focused `code_panel::tests` group passed
      61/61. The complete rebuilt app runner then passed 320/322 serial tests
      with scoped localhost access for mock services. The only failures were
      `app::lifecycle_tests::separate_ahead_processes_share_code_and_chat` and
      `app::lifecycle_tests::two_authenticated_shells_join_a_direct_shared_session`;
      both failed because the expected shared-session invite was absent from
      the join input, not because localhost binding was denied. This remains
      outside the core broad-use gate; no collaboration code was changed.
      The same fresh `target/debug/ahead` passed
      `tests/workspace-search-scale-smoke.mjs` against 10,000 disposable files:
      indexing 1,377 ms, cold no-hit search 884 ms, warm no-hit search 532 ms,
      and 282 ms to return the 5,000-match cap. Ripgrep measured 447/378 ms for
      cold/warm no-hit scans and 358 ms to count all 10,000 hot matches. These
      are machine/cache-sensitive proxy timings, not a UI benchmark; the
      exact-10,000/over-10,000 Replace All boundary and full core suite were
      subsequently rerun; see the current core-search result below.
      On 2026-10-12, the rebuilt `ahead-core` runner passed all 52 tests,
      including the exact-10,000/over-limit Replace All boundary. The first
      full run exposed a stale cancellation assertion capped at four paths;
      AHEAD's CPU-based worker window matches Zed's `(num_cpus - 1).max(1)`
      policy, so the shared worker-count helper now drives both implementation
      and assertion. The focused cancellation test and full suite then passed
      (1/1 and 52/52); search behavior is unchanged.
      A 2026-10-09 call-site audit finds the editor Search/Replace panel,
      managed `file_search`, and proxy search all use `ahead_core::search`;
      no retained Codex `GrepFiles`/`grep_files` implementation was found.
      This follows Zed's `project::search` ownership boundary without importing
      Zed's worktree internals. It is static architecture evidence: the
      10,000/10,001 Replace All regression exists but still needs a current
      test run, and rendered search behavior/performance remain open.
      Artifact recheck on 2026-10-09: the only app test runner is from 15:18,
      before `workspace_panels.rs` (19:08), `code_panel.rs` (20:51), and
      `ahead-core/src/search.rs` (21:08); no executable `ahead_core` test
      runner is present. CUA reports AHEAD stopped; `lockf`/`watchexec` remain
      live, with no Cargo or rustc process. Current-source tests and rendered
      acceptance remain pending; do not start a competing Cargo loop.
      Superseding recheck later on 2026-10-09: the app test runner
      `ahead_app-5b00065cae9bc61c` (12:20:14) is newer than `code_panel.rs`
      (11:37:41) and `session_panel.rs` (12:19:40); direct no-Cargo runs passed
      61 code-panel, 54 session-panel and 29 Settings tests. The existing
      runner also passed seven Extensions-panel and 15 workspace-panel tests;
      it is newer than those sources, including `workspace_panels.rs` (19:08).
      It also passed all 10 `terminal::tests` and 14
      `terminal_panel::tests`, newer than both terminal sources. Coverage
      includes PTY paste/bracketed-paste framing, selection copy, OSC 52,
      shell-safe paste and reaping shells/jobs on close.
      Coverage includes Zed-catalog search and pinned install requests, offline
      extension states, mutually exclusive search modes, Replace All preview,
      Git initialization, and narrow changed-file rows. These are headless
      GPUI checks only. On 2026-10-09, a locked/offline build of the current
      `ahead-core` test binary passed all 52 tests serially, including the
      exact-10,000 versus over-10,000 Replace All safety boundary. The current
      production binary separately passed the 10,000-file proxy search smoke.
      On 2026-10-12, the app runner `ahead_app-5b00065cae9bc61c` rebuilt at
      09:56 and verified newer than all exercised app sources. Direct serial
      runs passed 201 headless GPUI tests: 61 code-panel, 54 session-panel,
      29 Settings, 35 terminal/PTY, 15 workspace/search/Git and 7 Extensions
      tests. Coverage includes Add Comment visibility, gutters/diff previews,
      above-caret completion, model search and provider-scoped visibility,
      slash skills, terminal clipboard/paste and process cleanup, exclusive
      search modes, virtual result rows, Git initialization, and extension
      gallery filtering/install requests. These remain headless component
      evidence; native app startup still aborts before rendering.
      A fresh 11:53 app test runner was verified newer than the exercised
      app sources. Direct serial runs passed 183 tests across the code panel
      (61), session/chat (54), Settings (29), workspace/search/Git (15),
      terminal panel (14), and Extensions (10). This rechecks model-provider
      search and visibility, slash-skill filtering, editor gutter/diff/comment
      behavior, exclusive search modes, and terminal input/clipboard
      interactions. Native rendering
      and OS-level behavior remain unverified.
      Reused that 11:53 runner for five targeted session-pane GPUI interaction
      tests: empty-chat AHEAD/external/join routing, shell-level action routing,
      duplicate-submit/retry, failed-session input retention, and late-session
      completion. All five passed. These verify click routing and form state,
      not server-backed session startup or native hit testing.
      At 14:22 EDT, I stopped the old `just dev` session (33116) and restarted
      it as session 7336. The build completed in 14.29 seconds, but direct GUI
      startup again aborted with `com.apple.hiservices-xpcservice` connection
      errors (signal 6); `/usr/bin/open -n` returned `kLSNoExecutableErr` for
      the generated bundle. CUA launching that exact bundle reached AHEAD Dev's
      Open Workspace chooser, but selecting its disposable project timed out;
      CUA still reports AHEAD running but cannot attach. Session 7336 exited
      with status 134, so the dev watcher is stopped. Native rendered checks
      remain open; do not start another Cargo loop while the desktop process is
      still reported running.
      The shared `just dev` binary at 22:31 is newer
      than source files in the agent, app, core, proxy, RPC, viewmodel,
      extension-host and Code Mode crates.
      On 2026-10-08, the existing macOS watcher held the per-checkout lock but
      did not rebuild after source replacements or metadata touches; its app
      binary remained at 15:54. A fresh 250ms polling `watchexec` did detect a
      Rust source replacement. `Justfile` now enables 1s polling on macOS to
      preserve the shared loop without starting another Cargo watcher; verify
      that behavior after the current loop is restarted.
      Rechecked at 17:41 EDT: Activity Monitor confirms the live lock belongs
      to this checkout (`lockf` 9488, parent `just` 9487, process group `rtk`
      9486). AHEAD Dev remains running, but CUA cannot attach to inspect its
      project or dirty buffers. I left the loop untouched rather than risk
      losing unsaved test-project edits. The current source and Justfile are
      newer than the app binary (15:54); after the project can be inspected,
      save/resolve dirty buffers, restart this one loop, and verify polling,
      rebuild, and the filled Add Comment button in the disposable app.
      Latest continuation recheck at 18:48 EDT: `target/debug/ahead` remains
      at 15:54, `code_panel.rs` is 18:47, `session_panel.rs` is 18:41, and
      the app test binary is 15:18. CUA reports the Mac locked, this thread has
      no attached terminal, and the guarded `just dev` retry exits 75 because
      the checkout's OS lock is held. Preserve that loop; do not run a competing
      Cargo command or stop it before the disposable project's dirty buffers
      can be inspected.
      Latest continuation recheck on 2026-10-08: macOS is still locked, this
      task still has no attached app terminal, and process listing is denied.
      The guarded `just dev` retry again exits 75 on the same checkout lock;
      no executable `ahead_core` test runner is available, while the app binary
      remains at 15:54 and the search sources are newer. Preserve the existing
      loop and its disposable-project state; run the new 10,000/10,001 Replace
      All boundary regression after its owner/terminal can be inspected.
      Latest continuation: the guarded `just dev` retry still exits 75, and
      `lsof` confirms PID 9488 owns the checkout lock. The Mac remains locked
      and this task has no app terminal. A new clean copy of
      `tests/fixtures/editor-smoke/` at
      `/private/tmp/ahead-broad-smoke.im0WJM/project` has a committed Git
      baseline; its Node suite passes (2) and Python suite passes (1). These
      fixture-language tests do not verify AHEAD. `target/debug/ahead` remains
      older than the current proxy sources; do not run Cargo or stop the owner
      loop until its disposable buffers can be inspected.
      Process recheck: `lockf` PID 9488 and its `watchexec` PID 9498 are alive
      in this checkout, but no AHEAD `cargo` or `rustc` process is visible; the
      only `cargo` process is PID 9820 in a separate Cola worktree. The app
      binary is still 15:54 while `code_panel.rs` is 20:51 and the FIM host is
      20:48. The live lock therefore does not prove a rebuild is underway. The
      current watcher output/arguments remain inaccessible; do not stop or
      duplicate it while the Mac is locked.
      At 21:22 EDT, CUA still reports the Mac locked. The current source has
      the persistent labeled Add Comment toolbar action at
      `ahead-app/src/code_panel.rs:5052`; the user's latest screenshot again
      reports the old gutter-only plus icon is hard to see, matching the 15:54
      binary. Verify the labeled primary toolbar button in a fresh render before
      changing its styling or restoring a gutter action; do not duplicate the
      control. Rebuild and rendered verification remain pending. The
      same source also positions completions above the caret when space permits
      and caps the list at five; the screenshot's below-caret, longer list
      likewise predates those changes and needs a fresh rendered check.
      Latest process recheck at 21:15 EDT: the Mac remains locked and the
      watcher/lockf handles are live. Both AHEAD app processes map to the
      15:54 bundle; one holds `.ahead/session.db` and an editor lease in the
      disposable `ahead-comment-ui` project. No app terminal is attached here.
      Do not restart or claim a current render until that session can be
      inspected; the new search tests also remain uncompiled.
      Latest continuation at 09:07 EDT on 2026-10-09: CUA now inventories the
      desktop as unlocked and AHEAD Dev as running, but two attempts to attach
      to `io.ahead.dev` timed out. `lsof` still shows the same dev watcher and
      app processes, with PID 87295 holding the disposable project's session
      database and editor lease; the on-disk app bundle remains at 15:54 while
      `code_panel.rs` is newer. Do not restart that session. No Cargo/rustc
      process is open. The existing 2026-10-08 15:18 app test binary is newer
      than `settings_panel.rs` (2026-10-07 23:01); its serial Settings filter
      passed 29/29, including `model_list_scroll_does_not_scroll_settings_page`.
      This is headless Settings evidence only; fresh native render, model-picker
      transitions and the newer editor/search sources remain unverified.
      Recheck at 14:56 EDT: reused the live per-checkout `just dev` watcher
      (watchexec PID 32434); the guarded second `just dev` exited 75 rather
      than start another Cargo loop. A metadata-only touch to the already-dirty
      `ahead-app/src/lib.rs` triggered the existing watch build, and the app
      bundle was refreshed. In the disposable
      `/private/tmp/ahead-comment-ui.NnpclX/project`, opening `python/main.py`
      rendered the filled green `Add comment` action on the file-path toolbar;
      the accessibility tree exposes `Add comment to selected code`. This
      verifies visible placement and contrast in that editor layout, so do not
      add a duplicate gutter control. A later app relaunch restored the test
      workspace, but CUA attachment timed out again; proxy reconnection remains
      unverified. The screenshot-backed user guide is still the final milestone.
      The same 2026-10-09 manifest audit confirms 68 members in the root Cargo
      workspace, including all 54 retained runtime-source crates, with no
      nested `ahead-agent-engine/Cargo.toml`. The only declared database
      client is `libsql` in `ahead-proxy`; `SessionStore::open` uses Turso's
      local embedded builder. This confirms one workspace and one Turso/libSQL
      storage client, not Turso Cloud replication or a remote-service workflow.
      The existing extension-host runner (2026-10-08 16:04:37; newer than its
      manifest and every source file) passed 35 tests serially; its two
      localhost HTTP tests were filtered because the sandbox denies loopback,
      and two environment-dependent package tests remained ignored. No external
      requests or downloads occurred.
      Follow-up on 2026-10-09: directly reran the same source-current runner
      with local-only loopback permission; 37 passed and the same two real-
      package tests were ignored. This includes mock HTTP download/version
      paths, archive integrity/size limits, staged-install recovery, capability
      grants, Zed extension manifest/language discovery, and Node runtime
      verification. No external requests or downloads occurred.
      Latest 2026-10-09 `just dev` rebuild completed in 3.45s, then the AHEAD
      child aborted before drawing with the macOS HIServices connection error;
      CUA reports the app stopped. The same command-session handle remains live
      after the child exit, so retain it as the sole dev loop and do not start
      another Cargo command. From the source-current app runner built at 16:18,
      direct no-Cargo reruns passed the empty-state AHEAD/external/join button
      routing test and model-picker provider search/filter test. These are
      headless GPUI interaction checks, not native rendered acceptance.
      Later CUA launch of the built `io.ahead.dev` bundle reached its native
      Open Workspace chooser. Selecting the existing disposable
      `/private/tmp/ahead-comment-ui.NnpclX/project` timed out, and subsequent
      accessibility reads timed out; the global app inventory still reports
      AHEAD Dev running. The project load and rendered UI are therefore not
      verified. Keep the existing `just dev` session; do not start another
      Cargo loop.
No-Cargo panel rerun on 2026-10-08: the existing app test runner (15:18)
postdates `settings_panel.rs` (Oct 7 23:01), `extensions_panel.rs`
(Oct 7 05:07), and `terminal_panel.rs` (09:25). Serial runs passed all
29 Settings tests, 7 extension-gallery tests and 14 terminal-panel tests.
The runner predates the newer chat/editor/search sources; those remain
unverified by this run.
On 2026-10-09, reran the source-matched terminal filters from that runner:
all 14 `terminal_panel::tests` and all 7 `terminal_clipboard_tests` passed
serially without Cargo. Coverage includes Edit-menu clipboard actions,
shell-safe path paste, PTY interrupt and OSC 52. This is headless evidence;
native clipboard and rendered terminal behavior remain open.
On 2026-10-12, reran the clipboard suite from the 10:59 app test runner
(newer than `terminal_panel.rs`): all 7 `terminal_clipboard_tests` passed
serially, including shortcut and Edit-menu paste/copy, shell-safe file-path
paste, OSC 52 and Ctrl-C. One earlier grouped run had a transient shell-start
timeout in the Ctrl-C test; that test passed alone five consecutive times and
the full clipboard group passed on rerun. This remains GPUI/PTY evidence, not
native desktop clipboard or rendered terminal acceptance.
Revalidated after the 14:15 app-test build on 2026-10-12: all 14
`terminal_panel::tests`, 7 `terminal_clipboard_tests`, and 10
`terminal::tests` passed serially from that runner, newer than both terminal
sources. This includes editor/terminal clipboard round-trip, shell-safe path
paste, OSC 52, Ctrl-C, mouse reporting and PTY cleanup. No Cargo build was
started; native pasteboard and rendered UI remain unverified.
At 21:32 EDT on 2026-10-14, the 20:09 app runner (newer than
`terminal_panel.rs` at 10:37) passed all 14 `terminal_panel::tests` again.
This rechecks VTE cell output, printable/shortcut routing, mouse reporting,
scroll behavior and shell-safe path paste. It remains headless GPUI evidence;
native terminal focus, pasteboard and rendered behavior are open.
It also passed four source-matched app regressions: directory launch,
      stale session/turn rejection, VS Code shortcut routing, and the narrow-
      width inline dirty-tab marker. These are headless checks, not native
      layout or click-target verification.
      Latest editor screenshot recheck at 10:01 EDT on 2026-10-09: CUA reports
      AHEAD stopped. The source already has the labeled primary Add Comment
      action, above-caret completion placement, and a five-item fuzzy-ranked
      completion cap, but `target/debug/ahead` is still from 2026-10-08 15:54
      while `code_panel.rs` is from 2026-10-09 09:58. The locked shared watcher
      has no visible AHEAD Cargo build, and a separate Cargo process is active
      in the Cola worktree. Treat the screenshots as stale; keep fresh rendered
      editor verification open and do not start a competing Cargo loop.
      Rechecked at 23:38 EDT on 2026-10-14: the integrated binary was rebuilt
      from current `code_panel.rs`; production completion responses are fuzzy-
      ranked and capped to the best five before rendering, with keyboard
      selection using that same capped vector. The existing rank test asserts
      the five highest-ranked items. The popup's 320px value is only a maximum
      height, not a forced height. No additional row cap is needed for the
      request to reduce suggestions; the GPUI completion test runner is still
      older than this source, and native appearance remains unverified while
      macOS is locked.
      At 23:51 EDT on 2026-10-14, direct no-Cargo runs from the 20:09 app test
      runner passed 54 `session_panel::tests`, 29 `settings_panel::tests`, 12
      `extensions_panel::tests`, 14 `terminal_panel::tests`, 7 terminal
      clipboard tests, 10 `terminal::tests`, and 16 `workspace_panels::tests`.
      The runner is newer than those sources but older than `code_panel.rs`, so
      this does not refresh the latest editor/comment/completion tests. These
      are headless interaction checks; native rendering remains unverified.
      The same runner passed another 21 source-matched tests: Explorer (5),
      Quick Open (7), command palette (5), help (1), and shell environment (3).
      This remains test-binary evidence, not native UI acceptance.
      Continuation recheck on 2026-10-09: CUA still reports AHEAD stopped.
      `lsof` confirms the checkout's `lockf` PID 9488 and `watchexec` PID 9498
      remain alive, but neither the AHEAD executable nor `target/debug/.cargo-lock`
      is open. This thread has no attached app terminal, and the watcher's
      launch arguments/output are unavailable; its disposable project cannot
      be safely inferred. Do not start a competing `just dev` loop or claim a
      fresh render. Reattach to the existing loop or establish its workspace
      before resuming native acceptance.
      Contention-path verification: a second `just dev` attempt exited 75,
      reported lock holder PID 9488 through `lsof`, and instructed developers
      to reuse the existing terminal; it did not invoke a second watcher.
      Agent-worktree integration audit: all local branch tips are ancestors
      of `main`; the clean `feat/publish-pi` worktree commit and the
      `spike/gpui-kit` commit are already included. The latter checkout is
      missing and marked prunable, so any uncommitted files from that path
      cannot be inspected. No checkout changes were made.
      Revalidated on 2026-10-12: all five local branch tips remain contained
      in `main`; the `feat/publish-pi` worktree is clean, and the absent
      `spike/gpui-kit` checkout has no unique commits. Nothing remained to
      cherry-pick or merge from the registered agent worktrees.
      On 2026-10-08, the source-matched app test runner built at 05:18:52
      passed serial filtered suites for CodePanel (56), Extensions (7),
      Settings (29), Session (52), workspace panels (15), and terminal (32),
      with no Cargo build. This is headless GPUI/proxy evidence, not rendered
      desktop acceptance; CUA still reports the Mac locked. The terminal suite
      logged a sandbox `Operation not permitted` while stopping one process
      group, but all 32 tests completed successfully.
      Follow-up on 2026-10-08: Zed-style `ShowCompletions`/Ctrl+Space dispatch
      and the response-to-popup retry regression both pass. After matching
      Zed's PTY startup-marker pattern to avoid shell-echo false positives and
      startup races, the serial five-test clipboard/PTY group passes, as does
      the full 32-test terminal filter. Coverage includes paste-to-shell,
      exactly-once command execution, output copyback, shell-safe path paste,
      OSC 52, Ctrl+C and mouse reporting. The shared `just dev` loop rebuilt
      the current app successfully in 29.19 seconds; macOS then aborted before
      rendering with the locked desktop's `hiservices-xpcservice` connection
      error (signal 6). A follow-up `just dev` build at 06:35 finished in
      10.62 seconds with `ahead-app` and the root binary compiled, but native
      startup hit the same macOS error. The same `just dev`/watchexec handle
      was stopped before focused GPUI testing; the shared dev loop is now
      running again against the same disposable project, with one Cargo
      watcher. A fresh `just dev` build compiled successfully in 26.53 seconds
      and an incremental rebuild completed in 1.19 seconds, but both app
      launches aborted before rendering with the locked Mac's
      `hiservices-xpcservice` error. AHEAD now exposes Copy, Paste, and Select
      All in its native Edit menu using the same GPUI input actions as the
      focused terminal/editor. The new action-dispatch regression passes;
      source-current `terminal_clipboard_tests` passed 6/6 and the full
      terminal filter passed 33/33, covering shortcut and menu-action paste,
      shell execution, selected-output copy, shell-safe path paste, OSC 52,
      Ctrl+C and mouse reporting. Scoped rustfmt and `git diff --check` pass.
      Actual rendered menu/context-menu and native OS clipboard interaction
      remain unverified until an unlocked desktop session is available.
      Latest recheck at 08:04 on 2026-10-08: the current source-built
      `cargo test --locked --offline -p ahead-app --lib terminal_ --
      --test-threads=1` passed all 31 terminal tests. The same disposable
      `just dev` loop rebuilt `ahead-app` and `ahead` in 1.17 seconds, then
      macOS aborted the app before rendering with the same
      `hiservices-xpcservice` error; CUA confirmed the desktop is still locked.
      This leaves native OS pasteboard and rendered menu acceptance open; the
      passing suite verifies the GPUI/PTY clipboard round trip, not native UI.
      After the target cleanup, `just dev` rebuilt the workspace binaries from
      scratch in 8m21s, but startup again aborted before rendering with the
      `hiservices-xpcservice` connection error (signal 6). The source-current
      offline test runner now passes all 7 `terminal_clipboard_tests` and all
      31 terminal tests; the agent-auth exit regression passes a 20-seed sweep
      after waiting for a shell-start marker. Coverage includes shortcut and
      Edit-menu paste, exactly-once shell execution, selected-output copyback,
      and shell-safe path paste. Launching the built app bundle and opening the
      approved disposable project allowed rendered terminal verification:
      native paste inserted and ran `printf '%s\n' AHEAD_NATIVE_PASTE_OK`, and
      selected terminal output was copied with Cmd+C and pasted back at the
      prompt with Cmd+V. The `just dev` direct launch still aborts with the
      `hiservices-xpcservice` error; rendered Edit/context-menu acceptance
      remains open. Node and npm are already on the user's `PATH`, so no
      separate installation is needed.
      Follow-up on 2026-10-08: rerunning that same `just dev` loop with native
      app access rebuilt and opened the current AHEAD app on the approved
      disposable project. The rendered workspace, integrated PTY and Pi ACP
      side thread are visible. This supersedes the direct-launch blocker above:
      the restricted shell launch failed at macOS app registration, while the
      native-access retry succeeded. Rendered editor completion/navigation
      and terminal Edit/context-menu acceptance remained open at that check.
      Rechecked in the rendered AHEAD terminal on the same approved disposable
      project: pasted `node --version && npm --version && npm test` into the
      focused PTY and verified Node v24.15.0, npm 11.12.1, and both fixture
      tests passing. This confirms live clipboard-to-PTY paste and inherited
      Node/npm PATH; native Edit/context-menu acceptance remains separate.
      Native editor check in the same workspace: Cmd+P opened the TypeScript
      fixture, Ctrl+Space displayed a completion popup, and F12 navigated from
      `double(input)` to its definition. In Python, F12 navigated from
      `double(21)` in `main.py` to `arithmetic.py`. No fixture source was edited.
      Rechecked 2026-10-08: Node v24.15.0 and npm v11.12.1 are available.
      The opt-in managed LSP smoke (`AHEAD_LSP_INSTALL_MISSING=1`) reached the
      Vtsls npm install but failed with `ENOTFOUND`; the npm cache contains no
      Vtsls or BasedPyright package, so TypeScript/Python managed installation
      remains unverified until registry DNS is available. The `rust-analyzer`
      on `PATH` is a rustup shim whose default stable toolchain lacks that
      component. Using the installed Rust Analyzer 1.95 explicitly, the
      offline LSP smoke passed formatting, completion, definition, semantic
      tokens, hover, signature help, references, cross-file rename, unsaved
      diagnostics and repair, close/reopen, restart and graceful proxy exit.
      Recheck on 2026-10-08 with Node/npm present on `PATH` and the current
      `target/debug/ahead` binary reached AHEAD's managed Vtsls install and
      failed at npm registry resolution with `ENOTFOUND`; the isolated failed
      fixture was removed. This confirms the blocker is DNS, not executable
      discovery. The TypeScript/Python LSP matrix cannot complete until the
      registry is reachable.
      A network-enabled retry on the same day, still isolated to a disposable
      project and AHEAD data/user homes, installed the real Vtsls and
      BasedPyright packages through system Node/npm and passed the complete
      TypeScript/JavaScript/Python protocol smoke: completion, definition,
      diagnostics and repair, semantic tokens, TypeScript import quick-fix and
      auto-import resolve, close/reopen, server restart/recovery and graceful
      proxy shutdown. The fixture was removed afterward; no global packages,
      provider credentials or Cargo build were used. Native editor popup
      insertion and install-progress rendering remain separate acceptance.
      A second network-enabled retry hid Homebrew's Node/npm path from the AHEAD
      proxy. AHEAD fetched its checksum-verified Node LTS fallback, installed
      both language servers into the disposable AHEAD cache, and passed the
      same full TypeScript/JavaScript/Python matrix. This verifies the macOS
      clean-PATH runtime-download path; the fixture was removed afterward.
      Revalidated after the 2026-10-08 native-access `just dev` build using
      system Node v24.15.0/npm 11.12.1 and
      `AHEAD_LSP_INSTALL_MISSING=1 node tests/lsp-smoke.mjs target/debug/ahead`.
      AHEAD installed Vtsls, BasedPyright and pinned Rust Analyzer in the
      disposable workspace's isolated AHEAD cache; the binary is newer than
      the LSP host/runtime sources. TypeScript/JavaScript passed completion,
      definitions, diagnostics/repair, semantic tokens, import quick-fix and
      auto-import resolve; Python passed completion, definition,
      diagnostics/repair and semantic tokens; Rust passed formatting,
      completion, definition, semantic tokens, hover, signature help,
      references, cross-file rename and unsaved diagnostics/repair. All three
      recovered after process restart with unsaved buffers and shut down
      gracefully. No global packages or credentials were used. Workspace
      retained at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-6Zksmb`.
      This closes the managed install/proxy matrix for the current binary, not
      rendered language-server install progress or popup behavior; native
      interaction remains subject to the locked desktop.
      The 13:42 `target/debug/ahead` binary also passed
      `tests/lsp-shutdown-smoke.mjs` in shutdown, stdin-EOF, initializing and
      unresponsive-server modes. It verifies the proxy waits for the LSP
      shutdown/exit handshake or bounded deadline, reaps the child, and does not
      save unsaved editor buffers. Disposable workspaces remain at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-shutdown-X2PNtF`.
      An older 1.90 server failed because it requested Cargo's unsupported
      `--lockfile-path`; the 1.95 server passed against the current Cargo, so
      this was a mismatched test binary rather than an AHEAD launch defect.
      Model-selector recheck on 2026-10-08: the source-current focused GPUI
      test `chat_model_select_filters_by_provider_name` passed 1/1 and exercises
      opening the picker, typing a provider name, hiding non-matches and
      selecting the result. The pre-existing AHEAD Dev window still showed the
      full model menu without a visible search field, and typing did not filter;
      that window was not proven to be running the current binary. After closing
      it, a fresh `just dev` build succeeded but the app aborted during AppKit
      application registration (`NSApplication` / `_RegisterApplication`,
      signal 6), matching the existing `hiservices-xpcservice` launch failure.
      Rendered search-picker acceptance remains open until the current build can
      be launched in an unlocked desktop session.
      The source-current `empty_` GPUI hit-test group also passes all 6 tests,
      including the three vertically ordered session-start/join actions. Native
      pointer/rendered behavior remains unverified while macOS is locked.
      Re-ran `app::lifecycle_tests::empty_agent_panel_routes_ahead_and_external_start_actions`
      against the 11:20 app test binary (newer than `session_panel.rs` and
      `app_lifecycle_tests.rs`); it passed, covering integrated-panel clicks
      through both start flows and join validation without Cargo. This still
      does not prove the native pointer behavior in the user's screenshot.
      Rendered session-pane check on 2026-10-08: the threads `+` menu opened
      both the New AHEAD Thread flow and the External Agent chooser. Pi was
      installed through the curated in-app control, and a disposable-project
      side thread reached the rendered `External ACP agent ready` state. This
      verifies package install/launch and ACP initialization in the desktop.
      No model turn was sent: the selected Pi model uses a paid provider, and
      the prior free OpenRouter credential probe failed authentication. No
      file/tool request was issued to Pi. ACP response streaming, cancellation,
      and resume with a valid test provider remain open; Codex and Claude Code
      install/launch were unverified at that 2026-10-08 check.
      On 2026-10-12, the current `ahead-agent` test runner passed the ignored
      `pi_registry_adapter_round_trips_offline_session` test after seeding the
      temporary Pi home with two synthetic models on an unreachable localhost
      endpoint. The public `pi-acp` package installed into the temporary cache;
      Pi advertised both models, accepted a selection change, and restored it
      after session reload with `PI_OFFLINE=1`. No provider call, user Pi
      directory, or persistent cache was used. Pi therefore has install,
      initialization, option selection and resume coverage; one authenticated
      response is verified below, while provider-backed cancellation/resume
      remains open. A later 2026-10-12 run of the
      current `ahead-agent` test runner passed the ignored
      `curated_acp_agents_initialize_with_isolated_homes` test for Pi, Codex and
      Claude Code. It installed all three public ACP packages into disposable
      caches, launched each adapter with empty per-agent homes, and completed
      ACP initialization without credentials or prompts. Codex/Claude session
      creation, model-option wiring and authenticated turns remain unverified.
      Rechecked on 2026-10-13 with the source-current
      `ahead_agent-0e1e93ffd7e62eb3` runner: Pi's offline model selection/resume
      test and the three-agent initialization test both passed (2/2). The first
      sandboxed npm attempt failed with `ENOTFOUND`; the retry with scoped public
      npm access passed with lifecycle scripts disabled. All CLIs used isolated
      temporary homes; no model prompt, credential, or project data was used.
      Rechecked 2026-10-13 with
      `curated_acp_session_model_options_offline` (source-current test runner;
      public package install only, lifecycle scripts disabled): Pi's synthetic
      offline catalog and Claude Code each created an isolated ACP session,
      exposed model choices, accepted a selection change and published the new
      value. Codex initially returned `Authentication required` before
      `session/new`; the test then supplied a synthetic API key, called the
      advertised ACP `authenticate` method, and successfully created a session
      and changed its model option. This verifies the ACP authentication/config
      handshake for all three adapters, not a real provider credential, model
      turn or rendered chat picker. No prompt or usable credential was used;
      outbound child HTTP was directed to a dead localhost proxy.
      Authenticated provider follow-up on 2026-10-12: the current
      `target/debug/ahead` produced a streamed response matching a random
      disposable-workspace value. The test did not capture tool-call state, so
      this was not proof of Code Mode `file_search`. A 2026-10-14 diagnostic
      run found failed dynamic-tool calls, shell-command calls and an unanchored
      source edit. The credential remained isolated: silent input, mode-0600
      temporary settings, removal from the proxy environment and cleanup with
      the temporary profile.
      The ignored `pi_registry_adapter_completes_an_acp_turn` test now creates
      an isolated Pi home and test-only OpenRouter model entry; it does not
      touch the user's Pi settings. Its source-current runner passed the live
      ACP turn (1/1), confirming the provider-qualified model selection and a
      streamed `PONG` AgentDelta. The API key remained environment-only and the
      disposable workspace/home were removed. This verifies one authenticated
      Pi turn, not Pi cancellation/resume or Codex/Claude authenticated turns.
      The installed ACP bridge launched the separate Pi v0.87.1 CLI and
      announced v1.2.0 as an available update; the bridge package does not pin
      or install the Pi CLI.
      Native UI recheck on 2026-10-08: in the live disposable workspace, the
      threads `+` control opened its menu and choosing New AHEAD Thread opened
      the Describe/Review wizard. Closed before submission; no model request
      or new session was created.
      The full source-matched `ahead-app` GPUI runner completed 308 tests:
      306 passed; two collaboration subprocess tests could not bind their local
      mock services (`Operation not permitted` in this sandbox). The failures
      are `separate_ahead_processes_share_code_and_chat` and
      `two_authenticated_shells_join_a_direct_shared_session`; the rest of the
      app suite, including terminal, editor, model picker, settings, search and
      session controls, passed. Multiplayer remains a separate milestone.
      Latest rerun on 2026-10-08 used the 11:20 app test binary, newer than the
      app sources: 311 of 313 tests passed. The same two collaboration tests
      were denied local mock-listener binds by the sandbox (`Operation not
      permitted`); no other app tests failed. This confirms the same boundary,
      not a collaboration pass; multiplayer remains unverified here.
      In the explicitly approved disposable multi-language project,
      `just verify` passed offline: Rust (1), TypeScript/JavaScript (2), and
      Python (1), with no package installation.
      The 06:35 root binary also passed
      `tests/editor-recovery-smoke.mjs` in a fresh disposable workspace:
      acknowledged Unicode edits survived proxy SIGKILL, a second process
      could not steal a live recovery owner, explicit discard fenced stale
      writes across restart, and the local Turso database stayed mode `0600`.
      No model, language server, credential or real project was used.
      Re-ran this recovery smoke against the 2026-10-09 12:20 `target/debug/ahead`
      binary: all three checks passed again; its isolated fixture is
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-recovery-smoke-f4bK5S`.
      `tests/agent-code-mode-smoke.mjs` also passed against the same binary
      with a loopback-only mock model and isolated temp homes: delegated file
      search and Code Mode continuation returned the expected result, turn
      cancellation with a pending editor snapshot persisted through Turso
      reopen, explicit retry completed, and both proxy processes shut down.
      The sanitized environment contained no provider credentials or external
      network access; the smoke cleaned up its own fixtures on success.
      `tests/agent-file-attribution-smoke.mjs` passed against the same binary
      with a separate loopback-only model mock: the managed `apply_patch` edit
      saved once, persisted one Turso CodeAnchor with the current session,
      `actor_id = ahead`, path and edited range, and supplied only root plus
      ancestor `src/AGENTS.md` instructions (not sibling `data/AGENTS.md`).
      The disposable project and profile were removed after success.
      Fresh rerun on 2026-10-08 against the binary used by the shared
      `just dev` loop: both managed Code Mode and file-attribution smoke tests
      passed. They verified delegated file search plus cancellation/retry after
      Turso reopen, then an `apply_patch` write with one correctly scoped
      `actor_id = ahead` CodeAnchor and root/ancestor-only `AGENTS.md` context.
      Their mock servers bound only to loopback; the initial sandboxed bind was
      denied, and the local-only rerun passed without external network access.
      Latest rerun against `target/debug/ahead` (2026-10-08 10:22 build), with
      Node v24.15.0 and npm v11.12.1 available on `PATH` (the smoke scripts
      invoke Node; npm installation was not needed): both agent smoke tests
      passed again, as did `tests/editor-recovery-smoke.mjs` for SIGKILL
      recovery, single-owner fencing, discard/stale-write protection and
      owner-only DB permissions. Its fixture and isolated data home were removed.
      `tests/agent-shutdown-smoke.mjs` also passed against that executable:
      partial streamed output and cancelled status survive explicit shutdown
      and stdin EOF; after SIGKILL, failure state and instruction provenance
      survive Turso reopen without automatic replay, while explicit retry starts
      exactly once. This used a loopback mock model and disposable project only.
      Rechecked on 2026-10-08 against the 13:42 `target/debug/ahead` binary:
      explicit shutdown, stdin EOF and SIGKILL/retry scenarios all passed with
      the loopback mock. The new recovery fixture was retained at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-recovery-smoke-O4i4o2`.
      Re-ran on 2026-10-09 against the 12:20 root binary: partial streamed
      answers and cancelled turns survived explicit shutdown and stdin EOF;
      SIGKILL preserved failed state and instruction provenance without replay,
      and explicit retry started exactly once. The smoke used a loopback-only
      model and cleaned its disposable project on success.
      The source-matched proxy test runner at 04:14 also passed
      `dispatcher_commit_uses_anchor_attribution_and_clears_rows` after its
      03:45 source change, confirming AHEAD author, human committer, session
      trailers, verified staged content and matching-anchor cleanup.
      Its 17 serial memory/storage tests also passed: project/user-scope writes,
      Turso-backed memory search and index reopen/revision tracking, reviewed
      snapshot replacement, size limits, and symlink/path-safety checks.
      On 2026-10-09, the source-current proxy runner also passed six session
      storage tests and six memory tests, plus native-thread replay and
      runtime-state reopen against file-backed Turso. This verifies the local
      embedded schema and reopen behavior, not remote Turso Cloud.
      On 2026-10-08, the source-current app test binary (newer than
      `ahead-app/src/code_panel.rs`) passed all 59 `code_panel::tests`
      directly, without Cargo. This covers the completion cap/filter/acceptance,
      LSP navigation/actions/formatting, Git gutter and diff preview, comment
      anchor relocation, and blame regressions. These remain GPUI harness
      checks; later edits to `code_panel.rs` still need a source-matched rerun
      and fresh rendered app check. After feedback that the compact gutter
      action remained hard to see, the selected-range action moved to the file
      toolbar as a labeled Add comment button. Selecting code no longer hides
      its breakpoint control, and the gutter remains 26px wide. A new GPUI test
      asserts the labeled control has visible width and that both controls work;
      it has not run against this source yet. The latest source-only follow-up
      keeps the primary action enabled before its prerequisites, avoiding the
      low-contrast disabled state. Its dynamic tooltip explains that code must
      be selected or an AHEAD session started; invalid clicks do not create a
      draft. The GPUI regression now checks both incomplete states and
      successful activation. Scoped
      rustfmt, `git diff --check`, and the Impeccable detector pass, but the
      changed test has not been rebuilt or run. Rechecked on 2026-10-08:
      `code_panel.rs` is newer than `target/debug/ahead` (15:54) and the app test
      runner (15:18). CUA reports the Mac locked, this thread has no attached
      app terminal, and live PID 9488 (`lockf`) still holds the checkout lock.
      Leave it untouched; do not run Cargo or claim rendered acceptance.
      The cached 15:18 app runner was used for a narrower follow-up: the four
      `code_panel::tests::completion` cases passed, including the eight-item
      fuzzy-ranking cap and above-caret placement; the popup-above-caret render
      regression also passed (1). These tests are present in that runner but
      do not validate later `CodePanel` edits or native rendering.
      The latest user screenshot still shows the removed, gutter-only plus
      action. `code_panel.rs` is now 20:02, while `target/debug/ahead` remains
      15:54, consistent with the screenshot coming from a stale app build.
      Confirm the labeled toolbar button, click feedback, and editor layout in
      the unlocked disposable app. On 2026-10-08, the lock-protected `just dev`
      loop rebuilt the
      current app, proxy and extension host, then launch aborted with
      `Connection Invalid error for service com.apple.hiservices-xpcservice`
      and exit 134. A later CUA attempt found only the orphaned Open Workspace
      panel; macOS returned `procNotFound: no eligible process` when interacting
      with it. No fresh native screenshot was obtained. Retry rendered checks
      when the desktop app can stay running; do not count the GPUI tests as
      visual verification. On a later 2026-10-08 attempt, the existing AHEAD
      process opened a copied smoke project at
      `/private/tmp/ahead-comment-ui.NnpclX/project` and created its `.ahead/`
      state. The language-server trust sheet appeared; I attempted **Keep
      Blocked** and did not grant trust, but subsequent CUA calls timed out
      while the AHEAD process remained live. Activity Monitor showed no
      Rust Analyzer, Vtsls or BasedPyright process, so no project LSP was
      launched. Trust-sheet dismissal and rendered editor verification are
      still unconfirmed.
      Rechecked on 2026-10-09 after another screenshot showed the gutter-only
      plus: the source already has the labeled primary Add comment toolbar
      action, but `code_panel.rs` (20:51) is newer than both the app bundle
      (15:54) and GPUI test runner (15:18). The shared `watchexec` process is
      still active. A fresh CUA inventory now reports AHEAD stopped, and
      `lsof` no longer shows a process holding the disposable project's session
      database or editor lease. The existing `lockf`/`watchexec` pair still
      holds the checkout build lock, but this thread has no attached dev
      terminal; do not launch another Cargo loop. Verify the labeled action in
      a fresh rendered editor before changing its design further.
      Latest 2026-10-09 verification: `just dev` rebuilt the current app and
      the focused `code_comment_action_is_always_visible_outside_the_narrow_gutter`
      GPUI test passed (1/1). The rendered disposable editor shows the green,
      labeled Add comment toolbar button; the old gutter-only add control is
      absent. While bringing up the app, the broad-smoke project's trust prompt
      was mistakenly allowed, then immediately revoked from Language Servers;
      that panel now confirms servers are blocked. The app remains open with
      `python/main.py`; the dev watcher was stopped after trust cleanup.
      A subsequent no-send native chat check confirmed `/` lists workspace
      skills and that the green labeled Add comment action remains visible in
      the editor toolbar. Typing `/set` also exposed fuzzy description matches
      from unrelated skill metadata, so slash ranking now keeps description
      matches only when no command-name match exists. The rendered GPUI picker
      regression and all eight slash-ranking tests pass; `rustfmt --check`,
      `git diff --check` and the Impeccable detector pass. The updated native
      app rebuilt under `just dev`, but CUA could not reconnect after launch,
      so the post-fix `/set` render is still unverified. Model discovery showed
      the OpenAI-compatible endpoint unavailable; no model request was sent.
      The dev watcher is running against this disposable project and owns the
      checkout lock; do not start another Cargo loop.
      Tool reuse (user direction, 2026-10-05): use Zed as the interaction and
      behavior reference, and retained Codex implementations as the default
      implementation source. Adapt their handlers/contracts to AHEAD's editor,
      permission, attribution and Turso boundaries instead of rewriting the same
      capabilities. Do not prune a useful behavior just because its stock
      Codex-facing tool surface is disabled; prune product integrations only
      when AHEAD has no need for them. AHEAD currently follows Zed's single
      resumable `spawn_agent` shape (`label`, `message`, optional `session_id`)
      and reuses Codex Core dispatch, `ThreadManager`, `CodexThread` and turn
      execution. Zed's `SpawnAgentTool::run` awaits `subagent.send` and returns
      its session ID/output; its guidance recommends separate non-overlapping
      parallel delegations and session-ID follow-ups, not separate wait/message/
      list tools. AHEAD likewise persists child edges and parent cancellation
      interrupts the loaded descendant subtree. AHEAD's host dispatcher now
      uses Codex Core's in-flight `FuturesOrdered` pattern from
      `ahead-agent-engine/core/src/session/turn.rs::try_run_sampling_request`
      to poll dynamic tool requests concurrently while preserving response
      order. A focused Turso regression sends two
      `spawn_agent` calls and withholds either child response until both child
      turns start, while retaining durable-resume and paging checks.
      Reuse audit (2026-10-05): `NativeClient::run_subagent_turn` already uses
      Codex Core's `ThreadManager::spawn_subagent_session`,
      `resume_thread_by_id_with_tools`, `CodexThread::start_turn_if_idle` and
      shared turn consumer. Core owns thread creation, parent execution/MCP/
      history inheritance and the durable pre-create child edge; the AHEAD
      adapter owns the Zed-compatible `label`/`message`/`session_id` contract,
      AHEAD scope/settings, Turso-backed AHEAD session mapping and editor
      events. It should not be mistaken for a second agent loop. The removed
      Codex MultiAgentV1/V2 handlers also contained useful behaviors, so their
      removal does not establish that every behavior was unnecessary. Keep
      Zed's single awaited, resumable `spawn_agent`; do not restore separate
      send/wait/list commands without a demonstrated AHEAD workflow. The audit
      found and removed a duplicate Turso edge-open update on new children;
      only a resumed child needs reopening in the adapter.
      Still verify child editor-tool inheritance, permission enforcement,
      cancellation, cold resume and rendered behavior in the disposable
      project before treating this as complete.
      The child create/cold-resume regression now asserts its full AHEAD-owned
      dynamic tool surface (search, skill resource, editor buffer and
      presentation) while ensuring recursive `spawn_agent` stays unavailable.
      Host validation now counts Unicode characters like the tool schema's
      `maxLength` rather than UTF-8 bytes; a focused regression covers the
      multibyte boundary for both label and message. On 2026-10-07, the existing
      `ahead-agent` test binary (00:43) was newer than `native_client.rs`
      (2026-10-06 23:19); its full 54-test `native_client::tests` module passed
      serially. The default-sandbox attempt was denied at loopback mock-server
      binds; the same local-only tests passed with local-socket permission,
      without external providers or Cargo. This is unit evidence, not a fresh
      build or end-to-end acceptance. Earlier, after the user's `cargo clean`,
      the shared `just`/`watchexec` loop was observed but its output was not
      attached to this task. Current recheck (2026-10-07 12:14): `target/` has
      since been repopulated; `target/debug/ahead` is from 11:22 and predates
      current source, while the app test runner from 12:13:47 predates
      `settings_panel.rs` at 12:14:14. The Codex terminal is unattached, process
      inspection is denied, and CUA confirms the Mac is locked. These files do
      not prove a source-current build or rendered behavior. Reattach to/reuse
      the shared `just dev` loop after unlock, then run the Code Mode smoke and
      rendered checks; don't start a competing Cargo loop.
      Later on 2026-10-07, the parent-abort path was changed to await pending
      dynamic-tool futures so an active child's Turso edge can close before the
      parent prompt returns. The focused `native_agent_cancel_interrupts_an_active_child_before_the_parent`
      test rebuilt from current source and passed with local-loopback permission;
      its default-sandbox attempt failed before the test body at the mock-server
      bind. Recheck (2026-10-07): the source-current `ahead_agent` test runner
      built at 17:03:11 is newer than all `ahead-agent/src` Rust sources and
      retained runtime Rust sources (latest 09:38). Its full serial library run
      passed 164 tests, with 3 ignored and 0 failures, including the active-child
      cancellation regression. Local-loopback mock permission was used; this
      does not verify authenticated providers or rendered behavior. Most
      recently, `just dev` rebuilt current `ahead-app` and `ahead` sources at
      21:21, then macOS aborted app startup after
      `com.apple.hiservices-xpcservice` returned a connection-invalid error
      (signal 6). One watcher remains on the disposable E2E project.
      The source-current offline `ahead-app --lib terminal` run passes all 23
      matching tests, including GPUI shortcut-to-clipboard-to-PTY paste and
      terminal-output copyback; native clipboard/menu interaction and rendered
      behavior remain unverified until app launch works again.
      On 2026-10-08, the existing app test binary was newer than the terminal
      sources and all 31 terminal tests passed, including clipboard round-trip,
      PTY paste, OSC 52 and mouse reporting. After that run, terminal selection
      was updated to publish Linux/FreeBSD PRIMARY and middle-click to paste it
      when mouse reporting is inactive, following Zed's
      `crates/terminal/src/terminal.rs` behavior. A focused GPUI regression and
      guide entry were added. On 2026-10-08, a locked/offline source build
      passed the new external-path clipboard regression and all 19
      `terminal_panel::` plus 9 `terminal::` tests. The regression pastes a
      file path containing spaces, an apostrophe and shell metacharacters into
      `cat`, verifies the file content, and asserts that shell syntax from the
      name did not execute. Scoped rustfmt and `git diff --check` also pass.
      Native macOS clipboard/menu behavior and rendered interaction remain
      unverified while the desktop is locked. To avoid a competing build, the
      shared watcher was stopped for the focused tests and restarted against
      the same disposable project. At 05:19 ET it rebuilt both `ahead-app` and
      `ahead` successfully in 10.20 seconds; native startup then aborted with
      `hiservices-xpcservice` signal 6 because the Mac remains locked. The
      `just dev` watcher remained available on that disposable project.
      At 05:28 ET, a direct rerun of the source-matched app test binary passed
      all five `terminal_clipboard_tests`: clipboard-to-PTY paste, selection
      copyback, shell-safe file paths, OSC 52, and Ctrl+C interrupt. This is
      GPUI clipboard/PTy evidence, not native macOS pasteboard or rendered-menu
      acceptance. The wider `terminal` filter on that source-matched runner
      also passed all 32 terminal tests, including bracketed-paste framing,
      scrollback selection, mouse reports and PTY process cleanup.
      A later source-current rebuild exposed instability in the five-test
      `terminal_clipboard_tests` group: one serial run passed two and failed
      three PTY-startup/clipboard/mouse checks, while each failed case passed
      when rerun alone. Resolved on 2026-10-08 by waiting for a split shell
      readiness marker before PTY input and for command-output markers before
      clipboard assertions; the current serial group passes all five tests,
      and the wider terminal filter passes all 32. This remains GPUI/PTY
      evidence, not native macOS clipboard or rendered-menu acceptance.
      Initial checkout freshness check on 2026-10-08 found `terminal_panel.rs`
      (02:44) and `session_panel.rs` (02:17) newer than the cached app test
      runner (00:17:30) and binary (00:18:09). At 03:51, the lock-protected
      `just dev /private/tmp/ahead-model-scroll-e2e.0bpFnT/project` acquired
      the repository's shared watcher lock and rebuilt the current app,
      extension host and proxy; Cargo finished in 48.59 seconds and linked
      `target/debug/ahead` at 03:51:45. macOS then aborted app startup because
      the desktop is locked (`hiservices-xpcservice`, signal 6). The same
      `watchexec` session remains live and owns the lock; reuse it after unlock
      rather than starting another watcher. This is a source-current binary
      build, not a test-target rebuild or rendered acceptance. At that point,
      model-search tests still needed a source-matched run; later chat/model
      picker evidence is recorded below. Native macOS clipboard/menu acceptance
      remains open.
      Follow-up against that source: all 28 `settings_panel::tests` passed,
      including provider-scoped hidden models, collapsible provider catalogs,
      stale discovery rejection and OpenAI-compatible `/models` parsing; the
      GPUI chat-selector test also passed provider-name search and selection.
      The disposable managed-agent shutdown smoke passed explicit shutdown,
      stdin EOF and SIGKILL/reopen/explicit-retry paths with partial streamed
      history retained in Turso. Its model endpoint was loopback-only. The
      shared `just dev` loop rebuilt through 21:29, but macOS again aborted at
      app startup on the locked desktop's invalid `hiservices-xpcservice`
      connection. The cached app GPUI test runner passes all 18 terminal-panel
      tests (14 keymap/input/mouse/path-paste tests and 4 clipboard-related
      GPUI tests), including Cmd+V reaching a disposable shell PTY and Cmd+A/C
      copying its output back through GPUI's clipboard. Native macOS clipboard
      and right-click menu behavior still need an unlocked rendered session.
      These checks do not replace rendered picker or authenticated-provider
      acceptance.
      Additional source-newer direct run on 2026-10-08 used the app test runner
      built at 11:20 (newer than `terminal_panel.rs` at 09:25). It passed 14
      `terminal_panel::tests`, 10 `terminal::tests`, and all 7
      `terminal_clipboard_tests`, including Edit-menu copy/paste, clipboard/PTY
      round-trip, shell-safe path paste, OSC 52 and interrupt. This remains
      headless GPUI/PTY evidence; native macOS pasteboard and menu rendering are
      still unverified.
      At 21:56 ET on 2026-10-07, reran
      `tests/agent-code-mode-smoke.mjs` against the existing `target/debug/ahead`
      binary (21:14:57; no AHEAD Rust source was newer). With loopback-only
      permission, the disposable mock-provider run passed delegated `file_search`,
      Code Mode execution/continuation, cancellation, retry after Turso reopen,
      and shutdown. No Cargo build or external provider was used.
      Revalidated at 23:14 ET against the `target/debug/ahead` binary rebuilt
      by the 23:09 `just dev` loop: the same managed Code Mode smoke passed
      delegated `file_search`, execution/continuation, cancellation while an
      editor snapshot was pending, Turso reopen/retry, and shutdown. It used a
      temporary project and a `127.0.0.1` mock model only; no external provider
      was contacted.
      Recheck at 21:56 ET on 2026-10-07: the source-matched app test runner
      (21:13:30; newer than `session_panel.rs` at 21:13:07,
      `settings_panel.rs` at 17:15 and `terminal_panel.rs` at 21:02) passed
      the exact provider-name model-search, virtual-model-list scroll
      containment, and terminal clipboard-to-PTY round-trip GPUI tests. No
      Cargo build was started. The native rendered checks remain open.
      Rechecked from the newer app test runner built at 22:24 on 2026-10-07
      (newer than all five app modules): 54 `code_panel`, 28 `settings_panel`,
      50 `session_panel`, 14 `threads_panel`, 15 `workspace_panels`, and 29
      terminal-filtered tests passed serially; the specific
      `empty_agent_panel_routes_ahead_and_external_start_actions` GPUI
      interaction also passed. This covers the dirty-tab dot, gutter/diff
      preview, inline-blame setting, model search/visibility, slash skills,
      ACP model options, exclusive search modes, terminal clipboard and TUI
      mouse reports. The disposable-project `just dev` build at 22:31 compiled
      but macOS again aborted before rendering with the locked desktop's
      `com.apple.hiservices-xpcservice` error (signal 6); these tests do not
      replace native UI acceptance.
      The following serial, offline full `ahead-app --lib` run passed 288 of
      290 tests. The only failures were the two existing multiplayer guest
      invite/child-process cases documented below; a loopback-permitted
      lifecycle rerun confirmed the same failures after 25 other lifecycle
      tests passed. The complete run included the terminal clipboard and model
      picker tests; neither failed. Do not count the known multiplayer failures
      as evidence against the terminal or provider-search controls.
      Do not restore Codex MultiAgentV2 aliases solely for parity. If actual
      AHEAD workflows need background steering, adapt the existing Codex
      handler/control logic through AHEAD's Turso and editor-policy seams rather
      than reimplementing it.
      Current durable-session recheck on 2026-10-07 used `target/debug/ahead`
      built at 22:31 (newer than the touched agent, proxy and app sources):
      `tests/agent-code-mode-smoke.mjs` passed delegated `file_search`, Code
      Mode result/continuation, cancellation while an editor snapshot was
      pending, Turso reopen, explicit retry and shutdown. The companion
      `tests/agent-shutdown-smoke.mjs` passed explicit shutdown, stdin EOF and
      SIGKILL recovery with partial streaming/provenance retained and no
      automatic replay. Current proxy tests passed four memory-index safety,
      size and revision cases plus the file-backed index-reopen case; the app
      runner passed the `@memory` project-excerpt selection test. These used
      temporary homes and loopback-only mocks, no Cargo build or real provider.
      They establish persistence/protocol and GPUI-harness behavior, not a
      rendered native chat or authenticated-provider turn.
      Rechecked on 2026-10-08 against the source-current 00:18
      `target/debug/ahead` binary: `tests/agent-code-mode-smoke.mjs` passed
      delegated `file_search`, Code Mode continuation, cancellation while an
      editor snapshot was pending, Turso reopen/retry and shutdown;
      `tests/agent-shutdown-smoke.mjs` passed explicit shutdown, stdin EOF and
      SIGKILL recovery with partial output and instruction provenance retained,
      followed by an explicit retry. Both used disposable workspaces and only
      loopback mock models; no Cargo build or external provider was used. Native
      rendered chat and authenticated-provider acceptance remain open.
      Incremental editor/LSP synchronization now sends a minimal UTF-8-byte
      `AheadDelta` on normal edits while retaining full snapshots for open and
      reload. The proxy validates the delta base length and owns revisions.
      Focused app and proxy regressions passed, including stale-base rejection.
      A fresh `target/debug/ahead` built at 23:49:46 passed
      `tests/lsp-smoke.mjs` across TypeScript, JavaScript, Python and Rust using
      cached language servers and the incremental notification path. This is
      real-server protocol evidence; native completion-popup/editor-insertion
      acceptance remains open while the desktop is locked.
      Rechecked on 2026-10-08 with the updated smoke against
      `target/debug/ahead` (00:18:09), newer than the LSP dispatch, RPC and
      editor-sync sources used by proxy mode. TypeScript, JavaScript, Python
      and Rust passed completion/navigation, diagnostics and repair; TypeScript
      also passed semantic tokens, quick-fix and resolved auto-import, while
      Rust passed formatting, hover, signature help, references and cross-file
      rename. Close/reopen, unsaved-buffer restart recovery and graceful server
      shutdown passed. The run reused local Vtsls/BasedPyright caches and Zed's
      Rust Analyzer; managed installation was disabled. Disposable fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-BFHO4Y`.
      This remains proxy protocol evidence, not native editor insertion or
      rendered completion acceptance. The same binary also passed all four
      `tests/lsp-shutdown-smoke.mjs` fake-server modes (normal shutdown, stdin
      EOF, delayed initialization and unresponsive server), with source files
      unchanged. Disposable lifecycle fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-shutdown-aN9NXM`.
      On 2026-10-08, the existing `ahead_app` test runner built at 00:17:30
      (newer than `code_panel.rs` at 00:04:11) passed 29 settings, 7 extension,
      15 workspace-panel, 56 code-panel and 31 terminal tests serially. This
      includes the editor's LSP/gutter/diff GPUI regressions and the terminal's
      disposable-PTY clipboard round trip. No Cargo build was started.
      `session_panel.rs` changed later at 02:17:54, so its chat/session tests
      are not covered by this runner. These are GPUI test-harness checks, not
      native rendered acceptance.
      On 2026-10-08, source-current existing runners passed all 49 `ahead-core`
      tests (including 36 shared-search tests) and all 55
      `ahead-agent::native_client::tests`. Both runners were newer than every
      Rust source in their respective crates; they ran directly without Cargo.
      The agent suite used only ephemeral localhost model/MCP mocks and covers
      managed streaming/cancellation, MCP approval, live-buffer file search,
      slash skill/action routing, and durable history. These are harness
      regressions, not a real provider response or chat-palette rendering.
      Fresh disposable acceptance on 2026-10-08 against `target/debug/ahead`
      (04:15:54) passed the full real-server `tests/lsp-smoke.mjs` matrix for
      TypeScript, JavaScript, Python and Rust, including completion, navigation,
      diagnostics/repair, semantic tokens, Rust formatting/rename, restart with
      unsaved-buffer replay and graceful shutdown. It used the Zed-cached Vtsls,
      BasedPyright and Rust Analyzer; no packages or providers were contacted.
      The same binary passed `tests/agent-code-mode-smoke.mjs` and
      `tests/agent-shutdown-smoke.mjs`: delegated search and Code Mode
      continuation, cancellation, Turso reopen/retry, explicit shutdown, stdin
      EOF, and SIGKILL recovery with partial output and instruction provenance.
      Those agent smokes used only loopback mock models and disposable profiles.
      The app test runner built at 04:13:22, newer than `session_panel.rs` at
      02:17:54 and `settings_panel.rs` at 23:01:47, passed all 52 session-panel
      and 29 settings-panel tests, including provider/model picker search,
      provider-scoped hiding, virtual-list scroll containment, and slash-skill
      filtering/selection. These are proxy and GPUI-harness checks, not native
      rendered acceptance or authenticated-provider evidence. The 04:15 root
      binary predates the later extension-host callback change; these smokes do
      not verify that callback path.
      From the same 04:13:22 app runner, all 56 `code_panel::tests` and 15
      `workspace_panels::tests` also passed serially. Coverage includes
      CodeAnchor gutter ranges, Git diff-marker isolation and preview/copy,
      blame tracking, narrow Git staging rows, exclusive search modes and
      search-result pagination. Native rendering and clipboard/file-picker
      acceptance remain separate.
      On 2026-10-08, process inspection found two live `just`/`watchexec` pairs
      rooted in this checkout; their recipe arguments/output are not attached,
      so neither was stopped. The `just dev` recipe now uses a per-user,
      per-checkout OS lock (`lockf` on macOS, `flock` on Linux) to reject future
      duplicate starts without waiting behind or killing the existing watcher.
      `just --unstable --fmt --check` passed, and a direct macOS contention check
      confirmed a second process exits immediately with the lock-held error.
      The Linux branch and end-to-end `just dev` duplicate-start flow remain
      unverified; the guard does not retroactively serialize the two existing
      watchers.
      Reference: `../zed/crates/agent/src/tools/spawn_agent_tool.rs`.
      The agent's `file_search` adapter and the project-search panel both use
      `ahead-core::search`; keep that as the single search engine. Zed's agent
      `grep` calls the editor's `Project::search`, then enriches matches from
      parsed buffers with syntax-ancestor/symbol context and navigable resource
      locations. AHEAD's shared scanner returns bounded matching-line previews;
      managed `file_search` now adds best-effort LSP document-symbol names for
      up to 16 open buffers with a 300 ms budget. That is not equivalent to
      Zed's syntax-ancestor enrichment from parsed buffers. Source review of
      pinned gpui-kit 0.6.6 confirms its code-editor syntax tree is held behind
      a private `InputHighlighter` adapter; `EditorState` exposes fold
      candidates, not syntax nodes, and AHEAD `CodePanel` has no outline or
      syntax-context API. Close the gap through a bounded context response owned
      by the live editor buffer/parser; do not add a second filesystem scanner
      or a separate stale parse. Keep the LSP-symbol fallback until that seam
      exists, then verify Rust, TypeScript, Python and extension grammars plus
      buffer-edit races. Do not imply Codex provides an equivalent project-aware
      search implementation. On 2026-10-07, the source-current
      `ahead_core-5ac384a5835214dc` test binary (01:01:43, after
      `ahead-core/src/search.rs` at 2026-10-06 18:39) passed all 36
      `search::tests` serially without Cargo. This validates the shared
      scanner/index behavior, not live-buffer syntax context or native UI
      acceptance. The Codex runtime does provide
      reusable native `exec_command`/unified-exec and `apply_patch` handlers;
      retain their execution, patch parsing/verification and model-facing
      contracts, with AHEAD adapters only for editor policy, open-buffer state,
      CodeAnchor attribution and UI events. Retain Core's MCP dispatch,
      deferred `tool_search`, streaming, cancellation and compaction behavior as
      agent-runtime implementations too. Attribution gap found on 2026-10-05:
      `NativeClient` emits `HarnessEvent::FileChange` for successful
      `PatchApplyEnd`, while `ExecCommandEnd` currently emits command status
      only. File writes performed through shell commands therefore do not get
      CodeAnchors from this bridge. The managed developer prompt now directs
      deliberate project-file edits through the retained `apply_patch` tool and
      reserves `exec_command` for non-editing work; this is model guidance, not
      host enforcement. Source inspection confirms
      `ExecCommandEndEvent` has no file-delta field, and its `ParsedCommand`
      summaries only classify reads, listing, search, or unknown commands. The
      retained `ExecCommandHandler` also allows parallel tool calls, so a
      workspace-wide before/after diff cannot safely attribute changes to one
      call and may include concurrent human edits. Capture exact deltas at the
      executor process boundary, keyed by call, with workspace/path checks;
      validate local and remote executors separately. Do not infer writes from
      command text or a turn-wide Git diff. On 2026-10-06,
      `tests/agent-file-attribution-smoke.mjs`
      passed against the existing `target/debug/ahead` binary: a scripted local
      Responses provider called Code Mode `exec`, which invoked nested
      `tools.apply_patch`; the resulting CodeAnchor was returned from Turso with
      the expected session, actor, path and quote. This is headless synthetic-
      provider evidence, not proof that a real model chooses the tool or that
      the rendered gutter-to-commit path works. On 2026-10-07, the current
      `target/debug/ahead` rebuilt by `just dev` passed the same disposable
      attribution smoke with a loopback-only scripted Responses provider; the
      patch created exactly one Turso CodeAnchor with the expected session,
      AHEAD actor, `main.py` path and changed-line quote. Native gutter-to-commit
      behavior and real-model tool choice remain unverified. Rechecked on
      2026-10-08 against the source-current 00:18 `target/debug/ahead` binary;
      the same isolated smoke passed with the loopback-only scripted provider,
      confirming the AHEAD actor, edited path, changed-line quote and Turso
      persistence. This still does not verify the rendered gutter-to-commit
      path or real-model tool choice. Later on 2026-10-08, both the managed
      Code Mode cancellation/retry smoke and the apply_patch-to-Turso-anchor
      smoke passed again against the 04:16 `just dev` binary, using only a
      disposable project and local mock model. If supported MCP,
      shell or generator
      writes also need attribution, adapt Core's existing execution boundary to
      report exact file deltas and add a regression; keep `exec_command` and do
      not duplicate or replace its executor. `read_editor_buffer` and presentation
      tools are AHEAD editor capabilities, not replacements for Codex's disk
      tools. `file_search` is AHEAD-owned because Codex has no equivalent native
      project scanner; its scanner is shared with the project-search panel.
      Later on 2026-10-08, extended the disposable smoke through the real proxy
      GitStagePaths/GitCommit requests and reran it against `target/debug/ahead`:
      the AHEAD-authored edit was committed with author `ahead`, the configured
      human committer, and an `Ahead-Session` trailer; its Turso CodeAnchor was
      cleared after commit. This headless mock-backed check still does not verify
      the rendered gutter-to-commit interaction or real-model tool choice.
      Retained-runtime dependency prune (2026-10-06): package-wide searches
      confirmed three unused edges and removed `libc`/`windows-sys` from
      `codex-rmcp-client` plus the unused `tempfile` test dependency from
      `codex-mcp`. Earlier package-level `cargo-machete` findings (`cc`,
      `anyhow`, `git2`) were traced to build scripts; a fresh 2026-10-07
      workspace-wide `cargo machete` run reports no unused dependencies.
      Rechecked on 2026-10-08 after the voice transport changes: `cargo machete`
      found `ahead-rpc` was an unused direct dependency of `ahead-voice`; no
      source or build-script references existed. Removed that edge. Workspace
      `cargo machete` and locked offline Cargo metadata now pass; no build was
      started for this manifest-only cleanup.
      Both pinned-stable and nightly workspace `cargo fmt --all --check` runs,
      plus `git diff --check`, also pass; stable rustfmt warns that the configured
      `imports_granularity` option is nightly-only.
      Locked offline checks of both affected packages pass; no runtime behavior
      was removed. A 2026-10-07 full
      metadata/normal-edge audit found all 52 retained `codex-*` packages
      inside the current AHEAD dependency closure, with no simply unreachable
      Codex package left to delete. Nine are direct `ahead-agent` dependencies.
      A follow-up source audit found an unused `AuthRouteConfig` wrapper in
      `ahead-agent-engine/login/src/outbound_proxy.rs`. The current
      worktree removes it, its re-export, `Config::auth_route_config`,
      `resolve_bootstrap_auth_route_config`, and the obsolete login compatibility
      module/error alias. Workspace search finds no remaining references to
      those names. `HttpClientFactory` and `OutboundProxyPolicy` remain used by
      model endpoint routing and the shared auth HTTP client. Recheck
      (2026-10-07 22:55 ET): the source-current existing `ahead-agent` test
      runner passed 164 tests serially (3 ignored), including native tool, ACP,
      MCP, instruction loading, file-search, cancellation, Turso history and
      CodeAnchor regressions. The shared `just dev` binary compiles this pruned
      dependency graph, and its watcher remains live. No Cargo build or external
      provider was used in this recheck. This does not replace native app
      acceptance, which remains blocked by the locked desktop. Current disk
      headroom was 35 GiB at the earlier audit.
      `codex-exec-server` is also directly used by `ahead-proxy`. Reverse
      dependency traces show `codex-utils-audio` participates in Core history
      input handling, `codex-shell-escalation` in shell policy execution, and
      `codex-otel` in Core/exec-server instrumentation. Further reduction must
      remove a demonstrated unused module/feature edge without breaking Codex
      tool, history or policy behavior; crate count alone is not a pruning
      target. Current persistence inspection also confirms the desired Turso
      boundary: `ahead-proxy` is the only Cargo package declaring `libsql`;
      `NativeClient` injects AHEAD's `TursoThreadStore` and
      `TursoAgentGraphStore` into Core, and the thread adapter persists replay
      items through the AHEAD store while using Core's in-memory index. The
      retained rollout crate provides history types/policy/context scanning;
      managed Core has no rollout-file reader, importer or recorder. This
      source/dependency inspection does not replace Turso
      reopen and crash-recovery acceptance. Telemetry inspection confirms the
      `codex-analytics` crate is absent, and the root workspace disables
      `codex-otel` default features: neither the `ahead` app nor
      `ahead-code-mode-host` resolves `opentelemetry-otlp` in its normal Cargo
      tree. Session metadata and W3C trace-context helpers remain available;
      no Codex OTLP exporter is in the shipped application dependency graph.
      Expand `tests/fixtures/editor-smoke/` to Rust, TypeScript/JavaScript and
      Python projects. Prove pluggable language support through installed Zed
      language extensions: acquisition, language detection, server startup,
      completion, diagnostics, navigation, rename, formatting, configuration
      changes and restart. Test editing/save/undo, search/Quick Open, Git,
      terminal/tasks, settings/bootstrap, chat/context/skills/instructions,
      durable sessions and available debugger/agent features in disposable
      copies. Keep native observations separate from unit/protocol evidence
      in `docs/development/zed-tracking.md`; record prerequisites and remaining
      failures instead of claiming untested features work. Continue offline
      where provider credentials, adapters or devices are unavailable.
      On 2026-10-07, a fresh copy at
      `/private/tmp/ahead-editor-smoke.hmqu87/project` passed its offline
      `just verify`: one Rust, two TypeScript/JavaScript and one Python test.
      This verifies the disposable fixture only, not AHEAD workflows; that
      older temporary path is no longer present. The user-approved current
      copy at `/private/tmp/ahead-model-scroll-e2e.0bpFnT/project` already has
      `.ahead/`, so it is not first-open evidence. On 2026-10-07 23:07 ET its
      offline `just verify` again passed: one Rust, two TypeScript/JavaScript,
      and one Python test. The separate Settings loader regression now covers
      creation of `.ahead/settings.toml` in a fresh temporary workspace. Neither
      check proves native editor initialization or rendered setup. A shell-level
      GPUI regression now opens a fresh workspace and asserts the private
      settings file is created without generating project config; it has not
      been run because the post-clean Cargo build is waiting for more disk space.
      The 2026-10-05 LSP pass added staged F2/menu symbol rename and an
      all-target workspace-edit preflight. On 2026-10-06, the focused GPUI
      multi-file rename interaction regression passed after a clean rebuild;
      a serial full-suite run had 247 passes and three failures. One was a
      Settings lock-contention fixture that omitted the private endpoint; after
      specifying the matching default endpoint, its five GPUI seeds pass. The
      current serial app suite passes 248 tests with only the two failing
      shared-session integration tests skipped; those remain in the separate
      Multiplayer milestone below. The default-parallel run previously had one
      transient mock-model socket `WouldBlock`; that test passes alone and in
      the serial suite.
      On 2026-10-07, the current app test binary (newer than every file under
      `ahead-app/src`) ran all 285 tests serially: 283 passed. Only the two
      separate-process sharing tests failed. Their mock binds were denied in
      the default sandbox; exact reruns with local-only loopback access reached
      the join flow and exposed a missing invite in the headless clipboard
      fixture. This is not native clipboard evidence and remains in the
      separate Multiplayer milestone below; no other tests failed.
      Rechecked on 2026-10-08 with `ahead_app-601e2b645b3fbe4a`, newer than
      every `ahead-app/src` file: the serial suite ran 304 tests (302 passed,
      with the same two sharing tests failing). Exact localhost-enabled reruns
      reproduced the headless join fixture returning an empty clipboard invite
      in both cases. This remains collaboration-test/clipboard-fixture work,
      not evidence against the terminal clipboard path or native OS clipboard.
      The 2026-10-07 `ahead-proxy` test binary (05:10, newer than the current
      `ahead-proxy`, `ahead-core` and `ahead-rpc` sources) ran serially from its
      package root with local loopback enabled: 240 passed and one opt-in
      installed-extension test was ignored. Its earlier sandboxed workspace-root
      run had 229 passes and 11 environment failures; the package-root/local
      rerun cleared them all. This headless suite uses mock providers and does
      not verify an authenticated provider turn or native UI behavior.
      The cached `ahead-core` test harness (01:01, newer than all current
      `ahead-core/src` files) then passed all 49 tests, covering search,
      encodings, trust and secure file handling. This is also headless evidence.
      On 2026-10-06, direct execution of the current app test binary also
      passed focused LSP completion/auto-import, format undo, confirmed symbol
      rename, diagnostics notification, and version/read-only workspace-edit
      preflight regressions. These are headless GPUI/proxy checks; they do not
      replace fresh native interaction with installed language servers.
      Seven focused `ahead-agent` instruction tests also pass: target-specific
      `AGENTS.md` hierarchy for FIM, repository-root discovery, precedence
      ordering and replaced-parent-path rejection.
      The 2026-10-07 cached `ahead-agent` test binary passed 163 tests with 3
      intentionally ignored at the time it was run. A freshness recheck found
      that binary is from 00:43:40, newer than `instructions.rs` (2026-10-06
      09:34) but older than `native_client.rs` (2026-10-07 03:28), so it is not
      current-source evidence for the managed runtime. Re-running the seven
      `instructions::tests` from that binary on 2026-10-07 passed; those tests
      match the unchanged instruction source. The same binary passed all 25
      `runtime_support::tests`; its 00:43:40 build is newer than
      `runtime_support.rs` (2026-10-05 05:54). These focused module results are
      headless and source-matched; neither establishes current native-client
      behavior or a full current-source crate pass.
      On 2026-10-06, direct execution of the newer `ahead-agent` test binary
      passed all seven `instructions::tests`, four `runtime_support` workspace
      MCP policy tests, and three slash-skill invocation/qualification tests.
      Two managed stdio MCP form/URL end-to-end tests also passed with local-only
      permission after the default sandbox denied their loopback mock-model
      listener; no external provider or credentials were used. These are test
      binary results, not rendered approval UI or authenticated-provider proof.
      Verify multi-file rename in the disposable editor. Native verification
      on 2026-10-06 launched the app-only `target/debug/ahead` directly with
      isolated `AHEAD_DATA_HOME`, `AHEAD_USER_HOME` and `AHEAD_HOME` directories
      plus a fresh disposable project. The app created `.ahead/session.db`
      there with owner-only permissions; the three external temp homes remained
      empty. Settings later saved the provider configuration under the same
      project `.ahead/settings.toml` with mode 0600; the provider key was
      cleared from both saved fields after the endpoint check. Rust, Python
      and TypeScript files rendered, Cmd+P filtered/opened
      TypeScript, a Python edit showed the inline dirty dot and Undo cleared it,
      a breakpoint toggled on/off, and the PTY `pwd` returned the fixture path.
      The fixture was initialized as a local Git repo with one changed Python
      line, then an adjacent two-line change. Clicking the first opened an
      in-editor hunk with removed/added lines and a Copy change diff control.
      The adjacent two-line change rendered as one continuous gutter rail; its
      preview showed both removed/added lines, and Copy pasted the full unified
      hunk into a blank editor line. Both temporary pastes were undone and the
      source restored; the disposable Git worktree is clean.
      Three temporary long, nested untracked paths also rendered with ellipsis
      truncation in Source Control while status marks, per-file stage checkboxes,
      Stage All and Commit Staged stayed visible. The paths were deleted after
      inspection and the panel returned to Changes (0).
      That earlier run used the app-only binary; a later fresh disposable
      project was opened through the exact `just dev` loop, which built and
      launched the editor with the sibling Code Mode host. Opening the project
      created `.ahead/session.db`. Its integrated PTY found Homebrew `just`,
      Node, npm, Cargo and Python through the login-shell PATH, and `just verify`
      passed the fixture's Rust (1), TypeScript/JavaScript (2) and Python (1)
      tests from inside AHEAD. A TypeScript file opened through Cmd+P, accepted
      a temporary edit, and returned to its original buffer after Undo; the
      disposable source file was not saved. The LSP trust dialog was kept
      blocked, so no language server started or downloaded; repeat LSP checks
      only after an explicit workspace-trust approval and local server setup.
      Rechecked offline on 2026-10-07 against the binary rebuilt by the 19:37
      `just dev` loop. The standard servers are absent from the shell PATH, so
      the smoke used only Zed's already-cached Vtsls, BasedPyright and Rust
      Analyzer executables via explicit paths; no installs or downloads. The
      full `tests/lsp-smoke.mjs` run passed TypeScript auto-import resolution,
      JavaScript's shared server identity, Python completion/definition and
      diagnostic repair, Rust formatting/completion/definition/hover/signature
      help/references/cross-file rename, close/reopen, server restart with
      unsaved snapshots and graceful process-group shutdown. Its disposable
      fixture remains at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-VkXppM`.
      This is current-binary proxy evidence only; rendered/native editor
      acceptance remains open while macOS is locked.
      Revalidated on 2026-10-07 against `target/debug/ahead` rebuilt by the
      22:31 `just dev` loop: a combined run of
      `AHEAD_LSP_LANGUAGES=typescript,python,rust` passed in one disposable
      workspace using Zed-cached Vtsls, BasedPyright and Rust Analyzer. It
      covers completion, navigation, diagnostics/repair, TypeScript import
      resolution, formatting/rename where supported, restart recovery and
      graceful shutdown, with no downloads or model-provider calls. The
      combined workspace is
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-MwcTIE`.
      This still does not prove rendered editor insertion, diagnostics UI or
      native workspace-trust interaction.
      The PTY also read the user's zsh startup file and printed missing
      `zoxide`/`starship` warnings; `pwd` still worked. Keep shell environment
      isolation as a separate check. The GUI-launched shell initially could
      find Cargo and Python but not Homebrew `just`, Node or npm; `zsh -l -i`
      resolved all three. AHEAD now captures only the interactive login-shell
      `PATH` before launching the app, following Zed's
      `util::load_login_shell_environment` approach, while preserving the
      process environment. Its parser tests, full `ahead` binary target check,
      fresh `just dev` launch, in-app tool discovery and in-app `just verify`
      pass. Per-project shell environment/direnv capture remains separate.
      Current terminal work adds mouse drag/double-click/triple-click grid
      selection, selected-text clipboard copy, Select All over scrollback and
      visible output, platform copy/paste/Select All shortcuts,
      Alacritty bracketed-paste/newline handling, and a gpui-kit right-click
      Copy/Select All/Paste/Paste Text menu. Paste now follows Zed's terminal behavior for
      text, shell-quoted external file paths, and image clipboard entries
      forwarded as Ctrl+V. Clipboard handling now also follows Zed's OSC 52
      copy/read events. Keyboard routing follows Zed: the terminal consumes
      clipboard actions and terminal-specific keys, while printable/IME text
      and unrelated app shortcuts continue through GPUI. A separate Paste Text
      menu entry inserts clipboard text without path/image handling. The
      2026-10-07 offline focused terminal run passed all 16 tests, covering
      shell-path quoting, Ctrl+C/Select All routing, scrollback selection and
      extraction, printable-text routing, bracketed-paste/newline behavior,
      the OSC 52 PTY event, pasted command delivery to a disposable `/bin/sh`,
      and GPUI Copy/Paste/Select All action availability with pasted shell
      execution and copied output verified through the test clipboard.
      The GPUI PTY round-trip now simulates the actual platform shortcuts
      (Cmd+V, Cmd+A, Cmd+C on macOS) through the terminal key handler rather than
      dispatching clipboard actions directly. The source-matched focused test
      passed at 19:37 on 2026-10-07: the pasted command ran in the disposable
      `/bin/sh` PTY and the copied selection contained its output marker. This
      verifies the key-routing and clipboard paths in the GPUI test harness;
      native desktop clipboard/context-menu behavior still awaits an unlocked
      app session.
      Following the Zed keymap check, the current source now keeps Linux
      Ctrl+C/Ctrl+V available to the shell and uses Ctrl+Shift+C/V (plus
      Ctrl/Shift+Insert) for clipboard actions; Windows retains Ctrl+V and
      copies Ctrl+C only for a selection; macOS uses Cmd+C/V. Text-only paste
      follows Zed's Ctrl+Cmd+V (macOS) and Ctrl+Alt+V (Windows) bindings. The
      regression test now asserts the platform map and drives the GPUI
      clipboard round trip with the host's shortcut, sends a shell-input
      sentinel afterward, and verifies the pasted command ran exactly once.
      The select-all binding now also matches Zed: Cmd+A on macOS and
      Ctrl+Shift+A elsewhere. A regression asserts plain Ctrl+A remains
      available as the `\x01` PTY input used by shell/readline.
      The focused offline `ahead-app` terminal run passed 20 tests, including
      the GPUI clipboard round trip, exactly-once paste, Ctrl+A PTY input, Zed
      shell-edit shortcuts, and Shift/Cmd scrollback navigation. Scroll keys
      are ignored in alternate-screen applications. That compile caught a
      missing gpui-kit `FluentBuilder` import in terminal rename rendering; the
      import is fixed.
      Rechecked from the newer app test runner: the GPUI clipboard-to-shell
      round trip, live mouse-report test, Zed platform shortcut map, disposable
      `/bin/sh` paste test, and OSC 52 copy event all pass individually. The
      round trip verifies the pasted command runs once, copied terminal output
      returns through GPUI's clipboard, and the shell remains alive. This is
      harness evidence; native OS clipboard and context-menu rendering remain
      open until the desktop can launch AHEAD while unlocked.
      A fresh `just dev` at 21:03 compiled `ahead-app` and `ahead`, but native
      launch still aborts with signal 6 after macOS reports an invalid
      `com.apple.hiservices-xpcservice` connection. Native clipboard/menu
      behavior remains unverified while the Mac is locked; after unlock, repeat
      keyboard and context-menu copy/paste, Paste Text, file-path/image paste,
      selection highlighting and bracketed paste in an application that enables
      it. `resize_to_bounds`
      and `TerminalBackend::resize` already propagate panel bounds to the PTY;
      native resize/reflow remains unverified because app launch is blocked
      while macOS is locked. Multiple PTYs already appear as separate tabs in
      the bottom `DockLayout`; each title uses the shell's dynamic title plus
      its terminal ID. Zed supports custom tab names via
      `TerminalView::rename_terminal` and its tab context menu. AHEAD now has
      source for the matching context-menu/inline-title flow and a focused
      title-state regression, but neither has been compiled or rendered yet.
      Native resize/reflow and close/quit acceptance also remain open (tracked
      below). Debug terminal handoff remains a separate debugger milestone. Do
      not treat the passing headless clipboard tests as native desktop
      verification.
      Fresh source-current verification on 2026-10-07: the offline
      `ahead-app --lib terminal` run passed all 23 matching tests, and the
      focused GPUI `editor_clipboard_actions_round_trip_text_through_terminal`
      rerun passed. The GPUI test simulates the host's paste/copy shortcuts,
      runs a pasted command exactly once in an isolated `/bin/sh` PTY, selects
      and copies terminal output, and reads that output back from the test
      clipboard. This verifies the rendered GPUI action path through the PTY
      and clipboard test seam, not native desktop clipboard or context-menu
      interaction. One earlier process-group teardown logged sandbox
      `Operation not permitted`; a subsequent signal-0 check found no such
      process group. Native desktop clipboard/menu behavior still needs an
      unlocked app session.
      Rechecked at 23:27 ET on 2026-10-07 with the app test runner built at
      23:26:14 (newer than `terminal_panel.rs` at 23:25:34): all 30 `terminal`
      tests passed. In addition to the clipboard round trip through a
      disposable `/bin/sh` PTY and mouse reports reaching a TUI process, the
      new GPUI `control_c_interrupts_the_foreground_pty_process` test sends
      Ctrl-C through key dispatch, interrupts a real foreground `sleep`, and
      verifies the shell resumes with status 130. This follows Zed's explicit
      Linux `ctrl-c` -> `terminal::SendKeystroke` binding in
      `../zed/assets/keymaps/default-linux.json`. The shared `just dev` loop
      rebuilt the app in 9.91s at 23:26, but macOS again aborted before
      rendering because `hiservices-xpcservice` is unavailable while the
      desktop is locked; the watcher remains on the disposable project.
      Following Zed's
      `../zed/crates/terminal/src/mappings/mouse.rs` and
      `../zed/crates/terminal/src/terminal.rs`, the terminal now forwards
      click/release, drag/motion, wheel, and alternate-screen scroll input to
      PTY applications that request it; Shift keeps terminal selection and
      scrollback available. Native OS clipboard/context-menu acceptance and
      rendered resize/reflow remain unverified while the Mac is locked.
      Follow-up at 23:29 ET on 2026-10-07: the source-current app runner passes
      all 31 `terminal` tests, including a new GPUI regression that sends OSC 52
      from `/bin/sh` through AHEAD's terminal event pump and verifies the app
      clipboard receives the decoded text. This matches Zed's clipboard-event
      handling in `../zed/crates/terminal/src/terminal.rs`. The one-off test
      passed after pausing the shared `just dev` watcher. Restarting the same
      disposable-project dev command rebuilt the app in 10.52s, but launch
      again aborted before rendering on the locked desktop's
      `hiservices-xpcservice` error; the watcher is running again. Native OS
      clipboard and context-menu behavior still requires an unlocked desktop.
      Recheck on 2026-10-08: the source-matched `ahead-app` runner passed all
      31 terminal-filtered tests, including the GPUI platform-shortcut clipboard
      round trip into a disposable `/bin/sh` PTY, exactly-once paste, copied
      output readback, OSC 52, Ctrl-C, and interactive TUI mouse reports. The
      current `just dev` build compiled the updated app, but native launch again
      aborted before rendering because macOS reported an invalid
      `hiservices-xpcservice` connection. Native OS clipboard/context-menu
      acceptance remains open.
      Follow-up source-matched offline run on 2026-10-08:
      `cargo test --locked --offline -p ahead-app --lib
      terminal_clipboard_tests -- --nocapture` passed all four GPUI tests.
      They exercise platform clipboard shortcuts through the terminal view and
      disposable PTY, copied output readback, OSC 52 and foreground Ctrl+C.
      This is headless GPUI/PTY evidence; native OS clipboard and context-menu
      behavior still needs an unlocked desktop.
      A previously reported seed-9 failure in
      `control_c_interrupts_the_foreground_pty_process` did not reproduce in
      two 20-iteration runs today (seeds 9–28 and 0–19). The runner (15:18) is
      newer than `terminal_panel.rs` (09:25); no terminal code change was made.
      Monitor this regression if it recurs.
      A direct 20-seed GPUI sweep then reproduced a separate agent-terminal
      exit-status callback race at seed 7: the PTY output pump could observe its
      IO thread finished just after polling events and stop before the final
      queued exit event was drained. `start_output_pump` now performs one final
      poll after thread exit, and the callback regression sweeps 20 seeds.
      `cargo check --locked --offline -p ahead-app --tests -j1` passes for this
      change. The post-fix seed sweep still needs a linked test runner; disk has
      under 500 MiB free after the previous app-link failure. The native app
      still aborts before rendering with the locked desktop's XPC error.
      On 2026-10-06, the same disposable `just dev` session configured a
      keyless OpenAI-compatible loopback connection, created a managed AHEAD
      session, and rendered the exact `AHEAD_TEXT_SMOKE_OK` reply from model
      `gpt-5.5`; a minimized request-trace check confirmed the model and prompt.
      A follow-on native Settings check configured the user-supplied OpenRouter
      endpoint/model in that disposable workspace; Test Endpoint reached
      `/models` and reported HTTP 200. This verifies endpoint reachability only,
      not a generated model response. The test credential was blanked from both
      saved `api_key` fields afterward. A second managed chat turn remains
      unverified: the new-thread wizard accepted the smoke prompt, but its
      Review/Next action could not be activated through the current native
      computer-use bridge.
      The sessionless Agent panel had hidden its constructed start-session
      empty state behind a `chat_available` condition. It now renders that state
      and both start actions unconditionally when appropriate; its focused
      GPUI regression passes. The combined `AgentWorkspacePanel` also has a
      GPUI interaction regression that links the real session and threads
      panels, clicks New AHEAD session, and confirms the creation form appears;
      it passed on 2026-10-06. Its expanded regression now clicks through the
      External agent chooser, returns to the empty state, and confirms the Join
      shared session control remains reachable; the focused test passed on
      2026-10-06.
      The standalone session-panel regression covers Join validation. In the
      rebuilt disposable native workspace,
      New AHEAD session opened the setup wizard, External agent opened the
      supported-agent chooser, and Join shared session displayed validation
      for an empty invite. All three controls responded in that earlier run,
      but the user's latest screenshot reports that the session-pane buttons
      are unresponsive. Treat interaction as unresolved until all three are
      retested in a fresh native window after unlock; do not rely on the older
      successful run alone. Retry through the
      empty-state action now reports which setup was opened, and the AHEAD
      wizard focuses its goal field so the next action is immediately usable.
      This adds source feedback but does not replace the requested native
      interaction check. Follow-up source fix: `AgentWorkspacePanel` now scopes
      resizing to a GPUI drag that begins on its divider, following Zed's
      pane-group pattern in `../zed/crates/workspace/src/pane_group.rs`. The
      panel no longer attaches mouse-move/up handlers to the same container as
      the session controls, and dragging can continue outside the divider.
      Recheck the three actions in a fresh native window when available.
      ordinary `just dev` launch after resolving its macOS failure. A later
      fresh `just dev` app relaunch now aborts
      after compilation with `com.apple.hiservices-xpcservice` connection
      errors (signal 6); the same failure reproduced after the Git-panel
      rebuild. A signed, isolated app bundle still aborts when its executable
      is run directly; `/usr/bin/open -a` reports `kLSNoExecutableErr` on that
      same bundle, though a prior desktop-automation launch in a temporary
      isolated bundle succeeded. The session empty state now has a dedicated
      invite-JSON field grouped below the two
      session-start actions; chat text no longer triggers joining. Its focused
      GPUI regression checks empty input, ignores chat text, and verifies the
      explicit rejection guidance for a pasted URL. The separate section is titled “Join
      an existing session,” explains that users must paste the JSON invite
      copied from the host’s Share session action, and labels the field
      “Session invite” with the placeholder “Paste invite JSON here.” Pasting
      an `http(s)` URL now reports that session links are unsupported and asks
      for the copied JSON invite. The voice-and-sharing guide documents that
      payload. A clean `just dev` build on 2026-10-07 completed in 6m33s, then
      again aborted at direct GUI launch with the same XPC error. The 11:00
      macOS crash report shows `RegisterApplication` / `NSApplication`
      initialization before AHEAD startup. The generated bundle contains its
      executable, but `/usr/bin/open -n` still reports `kLSNoExecutableErr`
      after ad-hoc signing and successful `codesign --verify`; native rendering
      remains blocked in this Codex launch context. Do not treat it as an AHEAD
      startup regression without a fresh launch through the user's desktop
      workflow. A later isolated `just dev` build compiled; its constrained launch
      failed with the known macOS LaunchServices/XPC abort. The elevated
      `just dev` loop on 2026-10-06 compiled the current production path after
      `cargo clean`, and the focused `empty_chat_renders_session_start_actions`
      GPUI regression passed. Launch again aborted with the known macOS
      LaunchServices/XPC `Connection Invalid` error (signal 6). The visible
      Earlier on 2026-10-06, direct runs of the app test executable passed
      `empty_chat_renders_session_start_actions` and
      `empty_agent_panel_routes_ahead_and_external_start_actions`. Their
      synthetic GPUI clicks exercised New AHEAD session, External agent, and
      Join shared session validation, including return-to-empty-state routing.
      Rechecked on 2026-10-14 at a 520px right-dock width: the three controls'
      hit bounds stay on the chat side of the Threads split, and GPUI clicks
      still route to AHEAD creation, external-agent setup, and invite validation
      (1/1). This is headless geometry/input evidence; native pointer behavior
      remains unverified while macOS is locked. The integrated app rebuilt via
      the same `just dev` loop in 11.14s afterward; its app child again aborted
      at HIServices before drawing. Watcher session 3780 remains live on the
      same disposable project and data home.
      On 2026-10-07, the direct app test binary (00:51:24) was newer than
      `app.rs` (00:50:12) and `app_lifecycle_tests.rs` (Oct 6, 22:06). Direct
      runs of `empty_chat_renders_session_start_actions` and
      `empty_agent_panel_routes_ahead_and_external_start_actions` both passed.
      Later on 2026-10-07, a fresh locked/offline test build compiled the
      current app sources and both focused GPUI click/hit-test regressions
      passed again. This rebuild also compiled the current session-panel
      changes. At 05:07:54, reran both tests from the source-current app test
      executable (newer than `session_panel.rs` at 05:07:23); both still pass.
      These are current headless event/hit-test evidence only; native input and
      rendering still need a fresh-window check after macOS unlock. The latest
      screenshot does not visually match this source: it shows the old
      “Message the session” field inline with Join and the start actions in one
      horizontal row, while the current empty-state source labels a JSON invite
      field separately and stacks the start actions vertically. Treat this as
      a source/runtime mismatch until the current native window is inspected.
      Follow-up check on 2026-10-07: `session_panel.rs` is now timestamped
      08:22:15, later than the existing app test binary at 05:07:54. That
      executable no longer validates the current session-panel source. Scoped
      `rustfmt --check` on the current file passes; rebuild and rerun the
      synthetic clicks before treating the latest layout as tested. At 10:45,
      the direct app test executable (newer than `session_panel.rs` at 08:22:15,
      `app.rs` at 00:50:12 and `app_lifecycle_tests.rs` from Oct 6) passed
      `empty_agent_panel_routes_ahead_and_external_start_actions` again without
      Cargo. This still does not establish native pointer input or explain the
      screenshot/runtime mismatch.
      At 19:36 on 2026-10-07, `ahead_app-601e2b645b3fbe4a` (newer than
      `session_panel.rs` at 19:36:32 and `app.rs` at 16:21:58) passed both
      interaction tests again. This closes the headless test-binary staleness
      gap only; native hit targets and the screenshot/runtime mismatch remain
      open.
      Recheck: the same binary, timestamped 21:13:30 and newer than
      `session_panel.rs` at 21:13:07, again passed both
      `empty_chat_renders_session_start_actions` and
      `empty_agent_panel_routes_ahead_and_external_start_actions`. Its GPUI
      mouse hit tests cover New AHEAD session, External agent, and Join
      validation. Native pointer/rendered acceptance remains open; the Mac is
      locked in this session.
      Rechecked on 2026-10-08: `ahead_app-601e2b645b3fbe4a` is newer than
      `session_panel.rs` and `app_lifecycle_tests.rs`; both focused empty-state
      click tests pass again. This confirms the current GPUI handlers and hit
      tests only; native pointer input and the screenshot/runtime mismatch
      remain open while the Mac is locked.
      At 23:10 ET on 2026-10-06, `lsof` found three running AHEAD processes,
      all loading executables last modified at 18:44, before the later app and
      test source edits. Retest through the existing `just dev` loop after
      unlock; these running copies do not verify the newer source.
      Rechecked 2026-10-07: `target/debug/ahead` and the dev-bundle executable
      remain dated 2026-10-06 18:44:53, while `session_panel.rs` is dated
      2026-10-07 05:07:23. `lsof` confirms the existing `just`/`watchexec`
      pair is still in this checkout and three AHEAD processes still map to
      those older binaries; no Cargo process is visible. Do not start a second
      dev loop. The stale-binary explanation is plausible but the actual
      window cannot be inspected until macOS unlocks.
      Rechecked after the 2026-10-07 composer-copy edit: `target/debug/ahead`
      is still 2026-10-06 18:44:53, while `session_panel.rs` is 08:22:15 and
      `extensions_panel.rs` is 05:07:30. The shared watcher and a separate
      old app process are present, with no Cargo child. Only 370 MiB is free,
      and CUA still reports the Mac locked. Do not count the running process as
      current-source UI evidence; wait for unlock and adequate build space.
      gpui-kit's `TestWindowExt::click` dispatches mouse movement/down/up at a
      visible target's observed center, so the GPUI regression covers headless
      hit testing; it does not prove native OS event delivery or that the open
      window is running this checkout. The combined-panel test now asserts the
      three action bounds are vertically non-overlapping and clicks Join after
      returning from both start flows. The fresh native-window check remains
      open.
      AHEAD Dev window is still an existing active smoke conversation, not the
      sessionless state; its “Write a message to the agent” placeholder is the
      active-chat composer, while the empty-state source renders the separate
      invite-JSON field below the two start actions. Preserve the smoke session
      and keep rendered empty-state verification open. Rechecked on 2026-10-07:
      `git diff --check` passed; the existing watcher remained alive
      without a visible Cargo child, only 362 MiB was free, and the desktop was
      locked. Do not start another build; repeat rendered/click verification
      after unlock.
      Current-source recheck: `session_panel.rs` is dated 08:22:15, but the
      available `ahead_app` test executable is 05:07:54 and `target/debug/ahead`
      is still 2026-10-06 18:44:53. The earlier headless click tests therefore
      do not cover the latest source, despite the current source retaining
      explicit button handlers and named hit-test regressions. CUA still reports
      the Mac locked, no app terminal is attached, process inspection is denied,
      and the volume has 364 MiB free. Do not launch Cargo or treat the old
      window/binaries as current evidence; rebuild through the shared loop and
      repeat all three button interactions once the host is available.
      Latest recheck on 2026-10-07: disk availability is 592 MiB (the 364 MiB
      reading above was earlier). `ps` returned `Operation not permitted`, and
      this thread has no attached app terminal, so the current watcher/Cargo
      state is unknown. Do not assume that loop exited or start another one.
      On 2026-10-09, direct runs of
      `target/debug/deps/ahead_app-5b00065cae9bc61c` passed both
      `empty_chat_renders_session_start_actions` and
      `empty_agent_panel_routes_ahead_and_external_start_actions`. The runner
      timestamp (12:20:14) is newer than `session_panel.rs` (12:19:40); it
      verifies current GPUI button hit-tests and route transitions, not native
      OS input. AHEAD Dev is listed as running, but attaching to its window
      still times out, so the user's rendered unresponsive-buttons report
      remains open.
      Its Git panel previously showed a raw repository
      NotFound error with no initialization action. It now identifies the
      uninitialized workspace, offers the existing Git-init RPC, and disables
      stage/commit until initialization succeeds. Both focused Git-panel GPUI
      tests pass. Clicking Initialize Repository in the disposable fixture
      produced branch `master` and `Changes (26)`; `git rev-parse` confirmed
      the workspace root and the files remain unstaged. Workspace language
      servers stayed blocked. Commit Staged now uses its staged-file/message
      readiness predicate; the focused test passes and the rendered button
      stayed disabled both with no staged files and with an empty message, then
      enabled after staging files and entering a message. No commit was made;
      the fixture was returned to unstaged state. Repeat the live flow with the
      ordinary `just dev` launch after resolving its macOS LaunchServices
      failure.
      On 2026-10-03, a fresh disposable fixture copy passed `just verify`
      (Rust 1, TypeScript/JavaScript 2, Python 1) and received an initial Git
      commit without creating `.ahead/`. This validates the fixture, not editor
      bootstrap, LSP behavior or a rendered UI; those still need the native run.
      On 2026-10-06, the checked-in fixture's TypeScript/JavaScript tests
      (2) and Python test (1) passed directly without starting another Cargo
      loop. Rust fixture verification remains with the shared build loop.
      After rebuilding the debug sandbox helper, all 204 `ahead-app` library
      tests passed with isolated AHEAD data/runtime/user homes and loopback
      access. The managed-agent suite now passes 152 tests (3 ignored), and the
      proxy suite passed 225 tests (1 ignored) with temporary app/user homes
      and workspace-owned runtimes. This is automated coverage, not a rendered
      app pass. After restoring the pinned, hash-verified Code Mode V8 cache,
      the complete offline `just test-all` recipe passed on 2026-10-03,
      including Code Mode, retained patch and API error-context targets.
      A later 223-test `ahead-app --lib` serial run after the command-palette,
      staged-commit and narrow-tab changes passed 221 tests; the two failures
      were cross-process collaboration fixtures denied local mock-server binds
      (`PermissionDenied`). The new palette/task/commit, Git index-state and
      inline dirty-tab tests all pass individually. Those failures were a
      sandbox loopback restriction, not a product regression.
      On 2026-10-04, all 226 current `ahead-app --lib` tests passed serially
      with loopback enabled for local mock services. The host-model fixture
      answers the managed runtime's startup `GET /v1/models` before checking
      the streamed Responses request. This remains automated, not rendered,
      verification.
      Provider-backed ACP/native end-to-end tests remain opt-in and unrun.
      On 2026-10-06, direct execution of the newer `ahead-agent` test binary
      passed mocked ACP regressions for selecting an advertised model, applying
      and persisting saved model/config defaults, publishing live updates, and
      serializing concurrent option updates. This does not exercise installed
      Pi/Codex/Claude Code processes or a provider-backed turn; those remain
      open.
      On 2026-10-06, the ignored Pi offline-session acceptance passed against
      Pi CLI 0.87.1 and pinned `pi-acp@0.0.34`: the real bridge installed into
      the test's temporary AHEAD cache, initialized, changed its advertised
      model option, and restored the same session with the selection retained.
      It used `PI_OFFLINE=1` and a temporary Pi home; no model-provider request
      was made. This verifies Pi install/protocol/model-option/resume, not a
      provider-backed turn, Codex/Claude processes, or the native install UI.
      On 2026-10-07, direct adapter-layer runs from
      `ahead_agent-1dc9b367027b0bcd` (00:43, newer than `adapters.rs` at Oct 6
      21:17) passed all 14 runnable `adapters::tests` (3 ignored). The ignored
      local-package npm generation test also passed separately with system
      Node/npm and only a temporary package/cache. This verifies curated
      catalog selection, install safety, config defaults, and offline
      generation replacement; it does not verify the newer `acp_client.rs` or
      a live installed-agent turn. This continuation re-ran all 14 runnable
      adapter tests and the ignored local-package generation test successfully,
      without contacting the ACP registry or a model provider. A rerun of the
      ignored Pi offline-session test completed after 70 seconds but failed
      before ACP initialization:
      npm could not resolve `registry.npmjs.org` while installing `pi-acp`
      (`ENOTFOUND`). It used temporary adapter/cache and Pi-home paths with
      `PI_OFFLINE=1`, and made no model-provider request. This is a blocked
      package-fetch attempt, not evidence of an ACP protocol failure; rerun
      against current `acp_client.rs` once the package is locally available or
      registry DNS works.
      Rechecked on 2026-10-08 from the product proxy with Node v24.15.0 and npm
      11.12.1 available on PATH: normal initialization and
      `list_external_acp_adapters` returned the curated catalog. The real
      AHEAD-managed install then reached npm but again failed with
      `ENOTFOUND registry.npmjs.org` before Pi ACP startup. No provider
      credentials or inference request were used; this is registry DNS, not a
      missing Node/npm executable or proxy RPC framing issue.
      A later network-enabled retry in the same disposable product flow
      installed `pi-acp@0.0.34`; Pi ACP initialized, then correctly required
      authentication for `session/new`. With only local editor-MCP bridge
      access enabled, AHEAD surfaced the advertised `pi_terminal_login`
      method. `PI_OFFLINE=1` remained set and no login, model turn or provider
      request was attempted. This verifies current-package install, ACP
      initialize and auth-required handoff, but not model-option discovery,
      authenticated operation or rendered sign-in UI.
      Follow-up cache check: `npm cache ls pi-acp@0.0.34` lists registry and
      tarball cache keys, but `npm view pi-acp@0.0.34 dependencies --offline`
      still returns `ENOTCACHED`. `npx_install_command` explicitly directs each
      install to the AHEAD cache under the supplied storage root (temporary for
      this test), so the user cache cannot be reused as-is. Don't repeat the
      offline run without safely seeding the disposable AHEAD cache or restoring
      registry access.
      On 2026-10-07, the source-current `ahead-agent` test binary built at
      17:03:11 (newer than every `ahead-agent/src` and retained runtime Rust
      source) passed 164 tests serially, with 3 ignored. This includes
      `serializes_acp_config_updates_before_publishing_and_saving_defaults`
      and the active-child cancellation regression. Local loopback was enabled
      only for disposable mock services; the suite made no external provider
      calls. This supersedes the older 00:43 binary note above. The three
      ignored adapter tests still require an installed Pi/authenticated model
      or a disposable npm package; see their individual prerequisites.
      On 2026-10-06, the locked/offline serial `ahead-proxy --lib` suite passed
      238 tests with 1 ignored using local loopback. Codex Core's dynamic
      tool executor was serializing all AHEAD dynamic calls despite the host's
      ordered-concurrent dispatcher; only `file_search`, skill resource reads
      and `spawn_agent` now opt into local parallel execution. This hint is
      skipped from wire serialization/schema generation, and presentation tools
      remain serialized. The Turso regression holds child responses until both
      child turns start. Also corrected the model mock's `/v1/models` startup
      response and canonicalized the Rustup fixture path on macOS.
      On 2026-10-06, a fresh disposable Rust/TypeScript/JavaScript/Python
      fixture copy passed `just verify` (Rust 1, TS/JS 2, Python 1). This checks
      the sample projects, not editor bootstrap or rendered LSP behavior. The
      current locked/offline `ahead` binary then passed the recovery smoke
      (Unicode SIGKILL recovery, live-owner fencing, discard persistence,
      owner-only Turso file) and all four LSP shutdown modes using a fake local
      server. The fake now exits successfully for `--help`/`--version` probes;
      otherwise AHEAD correctly rejected it and tried its managed download.
      No provider request or download was made. Native GPUI verification is
      completed for the app-only launch with isolated profile roots; that
      binary was built without the Code Mode host, and workspace LSP trust was
      kept blocked. The exact `just dev` package set has now been rebuilt with
      the pinned V8 assets; its managed Code Mode smoke and streamed-turn
      recovery smoke pass below. The macOS lock still prevents rendered
      verification of that full app path.
      A 2026-10-06 rerun of `tests/agent-code-mode-smoke.mjs` against this
      app-only binary did not advertise Code Mode `exec` (only
      `request_user_input`); this binary was built without
      `ahead-code-mode-host`, so this is not a product regression. A fresh
      exact `just dev` build now includes `ahead-code-mode-host` and passes
      `tests/agent-code-mode-smoke.mjs` against the disposable loopback model;
      that passing current-source result is recorded under Harness integration.
      This is headless mock-provider evidence, not a rendered chat turn.
      `tests/agent-shutdown-smoke.mjs` also passes against the same binary for
      explicit shutdown, stdin EOF and SIGKILL: partial output and instruction
      provenance survive Turso reopen, and only an explicit retry starts a new
      turn. Its loopback model and workspace/profile are disposable.
      The 2026-10-06 locked/offline serial library checks for `ahead-core`,
      `ahead-extension-host`, `ahead-rpc`, `ahead-viewmodel`, `ahead-voice` and
      `codex-apply-patch` passed: 163 passed, 2 extension-host fixtures ignored
      because they require a real installed Zed extension package.
      On 2026-10-04, the exact `just dev` package set was rebuilt locked and
      offline, then `tests/agent-code-mode-smoke.mjs` passed against the fresh
      binary: delegated file search, model continuation, cancellation, retry
      after Turso reopen and clean shutdown. The smoke uses a disposable project
      and local mock provider, not authenticated credentials or the rendered
      GPUI app. Its mock now answers the managed runtime's `/v1/models` startup
      request before accepting Responses POSTs.
      The disposable editor-recovery smoke also passed against that binary:
      Unicode recovery after proxy SIGKILL, live-owner fencing, explicit
      discard across restart and owner-only database permissions. The native
      editor's rendered recovery prompt remains unverified.
      Revalidated local agent branches/worktrees on 2026-10-07: the clean
      `feat/publish-pi` worktree's `9418cd8` commit and the `spike/gpui-kit`
      worktree's `a5a43a7` commit are already ancestors of `main`; there are
      no commits unique to the local `ahead/floem-vertical-slice` or
      `feat/initial-pi-extension` branches. The `ahead-engine` worktree is
      clean. The missing `spike/gpui-kit` checkout is marked prunable and was
      left untouched, so there were no unique agent changes to integrate. The
      main checkout still has 951 dirty paths; they have been preserved.
      The first 2026-10-06 default-PATH attempt to repeat
      `tests/lsp-smoke.mjs` stopped before launch because `vtsls` and
      `basedpyright-langserver` were not on this shell's PATH; the offline
      smoke correctly refused its npm fallback. The already-installed servers
      were present in Zed's language cache and were supplied explicitly for the
      current-source rerun below; no install or download was needed.
      On 2026-10-04, `tests/lsp-smoke.mjs` gained an
      `AHEAD_LSP_LANGUAGES=rust` mode so available language servers can be
      exercised independently. The first Rust run exposed that `rust-analyzer`
      on PATH is only a rustup shim; the component is absent. The smoke now
      probes `rust-analyzer --version` and fails before creating a fixture when
      that happens. Using the already-cached Zed Vtsls, BasedPyright and
      versioned Rust Analyzer binaries with no download, the updated full
      smoke then passed TypeScript, JavaScript, Python and Rust, including
      completion, diagnostics, definitions, format/rename/navigation where
      supported, unsaved-buffer reopen, server restart and graceful cleanup.
      Rust-only, TypeScript/JavaScript-only and Python-only selections also
      passed independently.
      This is disposable proxy evidence, not native GPUI verification.
      On 2026-10-06, the exact shared `just dev` binary passed the full
      `tests/lsp-smoke.mjs` again using cached Zed Vtsls, BasedPyright and
      Rust Analyzer. Rust covered completion, definition, hover, signature
      help, references, cross-file rename edits, formatting, diagnostics/repair,
      close/reopen and restart; TypeScript covered completion, definition,
      diagnostics, quick-fix and resolved auto-import; JavaScript shared the
      Vtsls process; Python covered completion, definition, diagnostics/repair
      and close/reopen. All three servers restarted with unsaved snapshots and
      shut down cleanly. This remains protocol/proxy evidence, not rendered
      completion acceptance or native editor workspace-edit application. The
      successful fixture remains at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-z6YB2Q`
      for disposable inspection.
      On 2026-10-07, `tests/lsp-smoke.mjs` gained the explicit
      `AHEAD_LSP_INSTALL_MISSING=1` opt-in; its default still refuses absent
      servers before fixture creation. Separate Python-only and TypeScript-only
      opt-in runs used AHEAD's real npm acquisition path to install BasedPyright
      1.40.2 and Vtsls 0.3.0 into their fresh fixture `AHEAD_DATA_HOME` caches
      (npm lifecycle scripts disabled; data, runtime and user homes isolated).
      Python completion, definition, diagnostics/repair, close/reopen, restart
      and shutdown passed. TypeScript completion, definition, diagnostics,
      quick fix, resolved auto-import, close/reopen and restart passed; the same
      Vtsls instance served JavaScript completion, definition and diagnostics.
      A first Python attempt exposed a startup race; the smoke now waits for the
      server's Ready/error status before requesting completions. These are
      headless proxy results, not rendered GPUI completion. The passing fixtures
      remain at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-WuZfuT`
      and `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-IPyt3x`.
      A combined offline run then reused both exact AHEAD-installed package
      caches plus Zed's cached Rust Analyzer and passed TypeScript, JavaScript,
      Python and Rust in one fresh fixture. It covered server restart and
      graceful shutdown; Rust also passed formatting, navigation, hover,
      signature help, references and cross-file rename. `target/debug/ahead`
      (13:49) is newer than the LSP catalog, dispatcher, RPC and directory
      sources, so this is current proxy-source evidence. Fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-g84nPb`.
      Repeated the all-language smoke on 2026-10-06 with
      `CARGO_NET_OFFLINE=true`; it passed again without installing or
      downloading servers, and its fresh fixture is at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-PyEUFW`.
      On the next 2026-10-04 local recheck, `target/debug/ahead` was built
      at 01:55 while `ahead-proxy/src/plugin/catalog.rs` was modified at
      06:53, so that binary predates the current source. `vtsls` was absent
      from PATH and the Mac was locked. Do not treat a smoke against that
      binary as current-source evidence; rebuild and rerun with installed
      servers when build capacity is restored. The 2026-10-04 screenshot
      still showing the removed `Managed by ahead-proxy` card is consistent
      with a stale run: `target/debug/ahead` is built at 01:55, while the
      current panel source changed at 07:16 and the placeholder is absent from
      app/proxy/RPC code. A fresh app terminal is
      not attached, the Mac is currently locked, and only about 200 MiB is
      free, so native confirmation and rebuilding remain deferred.
      After the fresh 2026-10-04 package-qualified debug build, all four
      `tests/lsp-shutdown-smoke.mjs` modes passed against the current binary
      (shutdown, stdin EOF, initialization and unresponsive server), leaving
      unsaved source unchanged and reaping each language-server process.
      After the 2026-10-04 OTLP feature gate, the exact `just dev` binary set
      rebuilt locked/offline in 3m58 with the pinned, SHA-verified V8 cache
      supplied. The fresh binary passed the managed Code Mode and all four LSP
      shutdown smokes. This is a post-clean build duration, not a before/after
      benchmark; the rendered app remains unverified while the desktop is
      locked. The Windows sandbox's `windows` bindings dependency is now
      target-gated: Cargo's macOS graph omits `windows@0.58`, while the Windows
      graph retains it. The exact editor/host build passed again in 1m46 after
      that graph change; this incremental run is not directly comparable to the
      post-clean duration and does not establish a speedup.
      On 2026-10-07, `rtk node tests/lsp-shutdown-smoke.mjs
      target/debug/ahead` passed all four fake-server lifecycle cases again:
      shutdown, stdin EOF, initialization disconnect and unresponsive server.
      Reran after the 19:37 current-source `just dev` build: all four modes
      again passed, preserved the unsaved file and reaped each fake server.
      This was proxy lifecycle evidence; native editor shutdown remains a
      separate verification gate.
      The portable sandbox crate now keeps `dirs-next` and `tracing-appender`
      out of the macOS normal dependency graph while retaining them on Windows.
      Its 27 library tests pass offline, and the locked editor/host build passes
      in 2m28 after the manifest-triggered rebuild; this is not a comparable
      timing benchmark. An immediate repeat of the exact package build took
      4.13 seconds (3.98 seconds reported by Cargo), confirming the warmed
      dev-loop time without attributing that change to the graph trim.
      On 2026-10-05, the full locked/offline `ahead-agent --lib` suite passed
      156 tests (3 ignored). The stale remote-HTTP MCP credential projection
      fixture was replaced with a stdio environment projection test, matching
      AHEAD's retained MCP transport. The full `ahead-app --lib` suite also
      passed 228 tests after fixing a test-only `GitStatusFile` import exposed
      by the command-palette regressions.
      The 2026-10-05 Zed spawn-tool parity pass now treats omitted, null and
      blank `session_id` values as new-child requests and returns structured
      `session_id`/error output after a child has been created, preserving
      retry/resume. Its delegation guidance also follows Zed's advice to keep
      work self-contained, non-overlapping and follow-ups concise. The focused
      regressions and full locked/offline `ahead-agent --lib` suite pass (158
      passed, 3 ignored). The native app remains a separate rendered-verification
      gate.
      An earlier 2026-10-05 check found `target/debug/ahead` older than edited
      sources. The shared `just dev` loop later rebuilt it at 14:39:30, after
      `ahead-agent/src/native_client.rs` (13:36:56) and
      `ahead-proxy/src/plugin/catalog.rs` (14:38:57). The rebuilt binary passed
      `tests/agent-code-mode-smoke.mjs` with a disposable loopback model:
      delegated shared search, continuation, cancellation, retry after Turso
      reopen and clean shutdown. In the live GPUI app on the disposable Rust
      fixture, restarting language servers and pressing Ctrl+Space in
      `src/main.rs` produced a Rust completion popup. The integrated shell's
      `rust-analyzer` is a broken rustup shim, and the AHEAD cache has no
      managed Rust Analyzer binary; the popup proves completion worked but
      does not identify the server executable or verify the new acquisition
      fallback. A temporary Rust type error produced the expected inline
      `expected i32, found &'static str` diagnostic; the buffer was restored
      from its fixture text and `git status` confirmed no saved file change.
      The same disposable app run opened TypeScript and showed a
      Vtsls completion popup for the imported `doubled` member. Opening Python
      installed BasedPyright into AHEAD's cache; the server reached Ready and
      rendered an undefined-name diagnostic. Typing an attribute access in
      `main.py` opened semantic Python completions. F12 opened
      `arithmetic.py` at `double`; Shift+F12 rendered references spanning the
      Python source and test files. Temporary edits were undone and the
      fixture was confirmed clean. The newer app source now binds F2 and the
      Rename Symbol menu to `prepare_rename`/`rename`, presents an inline name
      field, and applies returned workspace edits through a version-, scope-,
      and read-only-aware preflight before updating editor buffers. Unit tests
      cover Unicode/default-behavior placeholders and stale-version/read-only
      rejection; a GPUI interaction regression covers prepare, inline
      confirmation and two-file workspace-edit handoff and passed its focused
      `ahead-app` run on 2026-10-06. This implementation is newer than the
      recorded native pass;
      verify a real cross-file Rust rename, unsaved open-buffer behavior,
      cancellation/stale replies, and save/undo in the disposable project before
      counting the LSP rename journey complete. On 2026-10-08, manual
      completion was wired through Zed's named `ShowCompletions` action and
      `ctrl-space` binding. Focused GPUI tests confirm one request with the
      active file/prefix/caret and that a returned completion opens the menu.
      Native keypress verification remains open because Zed notes macOS may
      reserve Ctrl+Space. Shift+Alt+F invoked Vtsls formatting in the
      editor and changed a temporary `value*2` to `value * 2`; the TypeScript
      file was restored and its on-disk Git status remained clean. Still trace
      the Rust executable provenance and verify Rust navigation plus Python
      formatting with an installed formatter. Use Zed's
      `crates/editor/src/editor.rs` rename flow as the behavior reference.
      The Code Mode smoke is proxy evidence, not
      authenticated provider or rendered-chat verification.
      On 2026-10-05, reran `tests/agent-code-mode-smoke.mjs` directly against
      the existing `target/debug/ahead`, which is newer than
      `ahead-agent/src/native_client.rs`. It passed delegated `file_search`
      within a Code Mode cell, continuation, cancellation while awaiting an
      editor snapshot, Turso reopen/restore, explicit retry and clean shutdown.
      The script used and removed its own temporary workspace/profile and a
      loopback mock provider; this remains proxy/mock evidence, not an
      authenticated-provider or rendered-GPUI pass. The latest
      `ahead-proxy/src/plugin/catalog.rs` source is newer than this binary and
      is not covered by the smoke.
      The 2026-10-04 `just test-all` run rebuilt the app binary in 2m07, then
      failed while linking the `ahead-proxy` library test with `No space left
      on device`; the host volume had about 250 MiB free and `target/debug`
      measured 119 GiB. No full-suite test result was produced. A Cargo clean
      dry run selected 132.6 GiB, so shared build artifacts were left untouched;
      rerun after disk space is safely recovered.
      The Search panel now makes Text, Word and Regex mutually exclusive
      modes, with Case as an independent modifier. A GPUI regression covers
      switching modes and retaining Case. In the rebuilt isolated native app,
      selecting Word then Regex left only the latest mode active; Case remained
      independently selectable. On 2026-10-07, the current `ahead-app` GPUI
      test binary passed both
      `workspace_panels::tests::search_modes_are_exclusive_and_case_is_independent`
      and `workspace_panels::tests::search_result_virtual_rows_group_paths_and_map_keyboard_selection`;
      native interaction evidence remains separate.
      The editor's custom gutter is now 26 px wide while preserving a 16 px
      breakpoint target, and Git hunk/blame refreshes coalesce for 300 ms after
      edits. Last-successful markers stay visible on refresh errors or unchanged
      responses. GPUI coverage now checks width, request coalescing, stale replies
      and marker retention; rerun it when disk space permits.
      On 2026-10-20, the editor gutter's Git, attribution and teaching-pointer
      indicators were consolidated into a 3 px shared rail, reducing the full
      gutter to 23 px while preserving the 20 px action column. The focused GPUI
      width/hit-target assertions were updated but not run; native visuals remain
      for user verification.
      After the user's cargo clean, the shared `just dev` app is running against
      the disposable multi-language fixture. Native Settings reached the
      loopback-only OpenAI-compatible mock (HTTP 200), discovered its `gpt-5.5`
      catalog entry, and a text-only managed turn completed with the expected
      `AHEAD_TEXT_SMOKE_OK` response. This verifies the rendered settings and
      managed request/response path, not real provider behavior, tool calls or
      MCP; no MCP server was approved or launched. The native Language Servers
      panel also reproduced Rust Analyzer startup failure: PATH resolves to a
      rustup shim whose component is absent, while the standalone Zed-cached
      binary runs. The source follow-up in `ahead-proxy/src/plugin/catalog.rs`
      now prefers the workspace toolchain when an override is present, probes
      candidates instead of trusting `which`, then uses AHEAD's cache or a
      managed GitHub-release install. Focused unit tests cover broken shims,
      rustup selection, asset naming and cancellation; verify the download/
      checksum path and rendered startup against the current binary. The earlier
      completion popup came from a binary older than this resolver and does not
      establish that the fallback ran. Use Zed's
      `RustLspAdapter::check_if_user_installed` as the behavior reference.
      On 2026-10-07, the existing proxy test executable (05:10, newer than
      `catalog.rs` at 05:05 and `lsp.rs`) passed four `rust_analyzer_` tests
      (broken-shim rejection, target asset selection, rustup override and
      cancellation), the relative-PATH-before-chdir resolver test, the
      language-server process-exit monitor test, and all three `builtin_npm`
      cache-publication/concurrency/shutdown-cancellation tests. These used
      temporary fixtures and fake npm processes without Cargo or network access.
      They do not reproduce or resolve the proxy EOF observed when opening
      Python in the native app; its live process/log trace remains unavailable
      while the Mac is locked.
      In the same native run, Vtsls installed into AHEAD's cache and reached
      Ready; an unsaved TypeScript edit produced its unused-binding diagnostic,
      and Undo cleared it. Opening `python/main.py` then changed the Rust and
      Vtsls cards to `AHEAD proxy connection closed`; the `just dev` session
      logged EOF while reading the proxy, and the in-app restart control could
      not recover it. Diagnose this process-level failure before counting
      Python LSP startup or retrying further native language-server checks.
      Subsequent 2026-10-03 removal of ChatGPT-only CyberAccessProgram
      request/context metadata and dead auth helpers has passed rustfmt and diff
      checks. A call-graph follow-up also removed a no-op auth-refresh API and
      an ignored Codex-home test constructor; external bearer refresh no longer
      carries the ignored `ExternalAuthRefreshContext` (including its unused
      ChatGPT account ID). The current `ahead-agent --lib` run compiles these
      cuts and passes all 152 active tests (3 ignored) with loopback-enabled
      local mocks. It also corrected AHEAD test servers to answer the standard
      `GET /v1/models` catalog request before the Responses POST.
      The latest auth pass removes ChatGPT OAuth/token variants and FedRAMP
      bearer-header plumbing, and removes account IDs/emails from session
      telemetry and network-policy audit metadata. Provider API keys and
      host-supplied headers remain; scoped rustfmt and the managed-agent test
      suite pass after these changes.
      The retained runtime credential enum is now named `AheadAuth`, not
      `CodexAuth`, across provider, session, and test call sites; credential
      variants and behavior are unchanged. API-key `Debug` now redacts the
      secret, with a focused regression added. A repository-wide stale-name
      search, formatting check and `git diff --check` pass; the auth regression
      and type-check still need the shared Cargo loop.
      A follow-on source-pruning pass removes Codex-backend-only model
      visibility/routing hints, request compression, and previous-model
      compaction retry. Static model catalogs no longer retain an unused auth
      manager; focused assertions cover API-supported model visibility and
      selected-model compaction. OpenAI model managers now leave auth ownership
      with their endpoint clients instead of retaining duplicate auth state;
      the request setup also no longer resolves a duplicate unused auth value.
      The model-manager suite passes all 47 tests. A sandboxed managed-agent
      run failed 13 tests at local listener binds; rerunning with loopback
      enabled passes all 154 tests (3 ignored). A scoped all-target check of
      `ahead-agent` and `ahead-app` completes with zero errors. Its AHEAD-owned
      `unused_mut` warning is now removed; only dependency future-incompatibility
      notices remain. Native UI verification remains blocked while the Mac is
      locked.
      Agent-worktree audit found the clean `feat/publish-pi` checkout and the
      `spike/gpui-kit` branch's committed history already contained in `main`;
      the GPUI worktree path itself is missing/prunable, so any uncommitted
      files from that checkout cannot be verified here.
      A host-level FIM provider regression now proves that root and target
      `AGENTS.md` instructions, persisted active-session context and relevant
      open-buffer excerpts reach the request while an unrelated sibling
      `AGENTS.md` does not. The focused test passed on 2026-10-03 with local
      loopback access; rendered typing/insertion/undo remain unverified.
      Rechecked on 2026-10-13 with source-matched proxy runner
      `ahead_proxy-72b012fe0cee57a8`: both the active-session outbound-request
      regression and editor-only FIM hierarchy regression passed against
      isolated loopback provider stubs. The request assertions cover root and
      target-specific `AGENTS.md`, applicable open-buffer paths, unsaved
      excerpts, durable objective/plan/decision context, and exclusion of an
      unrelated sibling instruction file. No external provider was contacted.
      The opt-in installed Zed HTML adapter smoke also proves startup settings,
      an embedded-CSS diagnostic, completion, document-formatting edits and
      paired-tag rename from the real server. A missing named `css` section
      must resolve to `{}`: the bundled server panics when AHEAD returns
      `null`. These checks are headless, not rendered-editor acceptance. The
      server also dynamically registers
      `workspace/didChangeWorkspaceFolders`. Defer folder deltas until AHEAD
      supports changing or adding workspace roots: each app/proxy currently
      owns one immutable root. If multi-root support lands, mirror Zed's
      `LanguageServer::set_workspace_folders` diff and test the installed adapter.
      A further inert-analytics cleanup removes compaction event construction,
      extra token-usage snapshots, deleted-token counts, and retained-image
      telemetry scans while preserving compaction hooks, results, history and
      request metadata. Hook completion also no longer builds a discarded
      analytics fact; synchronous lifecycle events and local hook metrics remain.
      Subagent-start analytics and its inherited client-metadata lookup are
      removed; subagent source, persistence and lifecycle events remain.
      Image-history preparation no longer accumulates analytics-only dimensions,
      detail, roles or call IDs; resize/detail policy and durable image
      processing are unchanged.
      A runtime audit confirms the managed-agent Cargo graph has no SQLite,
      rusqlite or SQLx dependency. AHEAD's live thread store writes to Turso;
      managed history is read only from Turso. The unused Codex-core
      public JSONL recorder/session-listing exports are removed. This pass also
      removed the model-visible Codex extension-tool adapter, its extension-only
      image-generation tool path, extension-tool discovery/collision tests,
      tool lifecycle/MCP-result callback plumbing, and unused shared-history
      snapshots. After `cargo clean`, `cargo check --offline -p ahead-agent
      --tests -j 1` passes. The AHEAD runtime API/skills tests (20), protocol
      sleep-item serialization test and three config-only MCP catalog
      integration tests pass. Keep the disabled copied Codex unit harness out
      of the product gate. A fresh root/`ahead-*` manifest and lockfile scan
      confirms `libsql` is the only database client; `rusqlite` and SQLx are
      absent from the workspace dependency graph.
      Copied test seams are now narrower: the unused `ahead-model-auth`
      `test_support` module was removed, and `codex-models-manager` plus
      `codex-utils-absolute-path` expose fixture helpers only through a
      non-default `test-support` feature enabled by dependent test targets.
      Re-run the affected package tests in the shared Cargo loop; this changes
      test-only visibility, not runtime behavior.
      Removed the disabled copied Core config-loader test suite (`test = false`)
      and moved the supported macOS managed-requirements cases into the active
      config-crate loader tests. The moved tests still need a focused macOS run.
      The managed runtime no longer has the Codex extension registry or its
      context, instruction, config, turn, item, token and lifecycle callbacks.
      Its former `codex-extension-api` crate is renamed `ahead-runtime-api` and
      contains only the host instruction loader plus ephemeral scoped state
      used by AHEAD skills, model selection and tool configuration.
      It is not a plugin API or persistence layer. The generic extension-owned
      turn-item envelope and extension-only ToolCall context are removed; sleep
      is now a native typed turn item. This follows Zed's direct
      integrated-agent shape in `../zed/crates/agent/src/agent.rs`. MCP
      registration now accepts configured servers only while retaining the
      environment-policy gate; compatibility/extension precedence is gone.
      On 2026-10-05, removed stale inline-test initializers for deleted
      `TurnContextItem.multi_agent_mode` and
      `SubAgentSource::ThreadSpawn.agent_role`. The locked/offline test target
      now compiles; `paginated_model_context_reads_reverse_pages_without_full_hydration`
      and `native_child_restore_hides_spawn_tool_after_restart` both pass.
      `cargo check -p ahead-agent --lib` also passes. On 2026-10-07, the
      current-source offline `ahead-agent --lib` suite passed all 163 active
      tests with three documented environment-dependent tests ignored. The
      sandbox initially denied the temporary loopback listeners used by 14
      tests; rerunning the same suite with local mock-socket access passed all
      166 non-ignored tests without outbound network access. Keep the disabled
      copied `codex-core` test harness out of the product gate.
      On 2026-10-07, the current-source offline `ahead-app --lib` suite passed
      256 tests. Only the two separate-process sharing tests were excluded;
      this sandbox denies their loopback socket binds. The passing run includes
      both LSP code-action continuation regressions and the provider-scoped
      model-visibility tests. The live two-process sharing checks remain open.
      A later serial run used the source-matched app test executable
      (2026-10-07 05:07:54, newer than `session_panel.rs` at 05:07:23 and
      `code_panel.rs` at 00:51:02) and skipped exactly those two
      separate-process sharing tests: 259 passed, 0 failed, 2 filtered. This
      includes the session-start actions, editor gutter/diff, provider model
      visibility, project search, terminal and settings regressions. It remains
      headless evidence; the desktop is locked and native interaction is open.
      On the same current source, offline library runs passed `ahead-core` 49,
      `ahead-rpc` 19, `ahead-viewmodel` 12, and `ahead-extension-host` 24 tests;
      two extension-host integration cases remain ignored pending their
      external package fixtures. The grammar-only extension test now uses an
      explicit Zed-style display name and homogeneous archive-path types. These
      package runs do not replace the workspace-wide `just test-all` gate.
      Reuse one `just dev` build loop, app identity and disposable workspace
      across checks where practical; restart only to load changes or test
      lifecycle behavior. Do not bypass required approvals, alter real project
      data, add product features outside this scope, or publish/release.
      Live multi-language follow-up (2026-10-08): the disposable fixture's
      `npm test` passes (2 tests), and the integrated terminal successfully runs
      Node and reports npm 11.12.1. `AHEAD_LSP_INSTALL_MISSING=1 node
      tests/lsp-smoke.mjs target/debug/ahead` acquired official TypeScript,
      Python and Rust language servers into isolated temporary caches and passed
      the completion/navigation/diagnostics/repair matrix, TypeScript quickfix,
      auto-import and semantic tokens, Rust formatting/rename/references, and
      restart/reopen checks. In the native app, creating a disposable AHEAD
      session and opening `python/main.py` worked; opening Language Servers
      coincided with `AHEAD proxy connection closed`, reproduced after a fresh
      `just dev` relaunch. The proxy child exited and the app observed EOF; a
      temporary shutdown/drop trace did not identify an explicit shutdown
      callsite. Investigate this proxy/LSP lifecycle before accepting editor-side
      language-server startup. The disconnect status now includes the proxy
      child's exit status when observable; a Unix regression was added but not
      run. Keep this item open until the test and native lifecycle cause are
      verified. No model-provider inference was attempted.
      Current 2026-10-12 continuation: the source-current app library binary
      ran all 322 tests serially: 320 passed; the only two failures are the
      already-separate multiplayer cross-process session tests below. In the
      default sandbox, both first failed to bind their synthetic 127.0.0.1
      servers. Focused loopback-enabled reruns reached the product assertions
      and reproduced the existing missing-invite/join failures. AHEAD's app
      chooser opened `/private/tmp/ahead-broad-use.0XxogT/project`; the folder
      is the disposable Rust/TypeScript/JavaScript/Python fixture. Opening it
      created `.ahead/settings.toml`, `session.db` and `editor-leases/`, but
      subsequent native accessibility and screenshot requests timed out, so
      this does not prove visible bootstrap completion. The existing shared
      `just dev` session remains live; a second start correctly refused its
      build lock. Do not launch another Cargo loop.
      Fresh 2026-10-13 acceptance against the current `target/debug/ahead`:
      `tests/agent-code-mode-smoke.mjs` passed managed Code Mode, delegated
      search, cancellation, retry after Turso reopen and shutdown;
      `tests/agent-file-attribution-smoke.mjs` passed the managed edit -> Turso
      CodeAnchor -> staged commit path with AHEAD author, human committer,
      session trailer and anchor cleanup. Both used isolated temporary data and
      loopback-only mock providers. `AHEAD_LSP_INSTALL_MISSING=1
      AHEAD_LSP_INSTALL_ZED_HTML=1 rtk node tests/lsp-smoke.mjs
      target/debug/ahead` freshly installed Vtsls and BasedPyright and installed
      HTML 0.3.2 from Zed's public catalog. The live-server matrix passed for
      TypeScript, JavaScript, Python and Rust: completion, navigation,
      diagnostics/repair, semantic tokens, code actions/import resolve,
      formatting, hover/signature/references/cross-file rename, restart/reopen
      recovery and graceful shutdown. Disposable fixture retained at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-m9SoXl`.
      Native follow-up: AHEAD Dev launched to its workspace chooser and the
      disposable `/private/tmp/ahead-dev-smoke.XPWP4f/project` was selected, but
      every subsequent accessibility/screenshot request timed out while the app
      remained in the running-app inventory. It is unknown whether workspace
      loading or a modal is blocking the window; no language-server trust was
      approved. Rendered acceptance remains open. On 2026-10-13, restarted the
      shared `just dev` loop (session 41219) against the same disposable project
      and isolated data home. The integrated build passed in 28.20s; macOS then
      aborted GUI startup before rendering with the HIServices XPC error (signal
      6). CUA app attachment timed out (`-10005`), so the Add Comment control
      could not be rechecked visually. From the source-current app test runner
      (built 09:16, newer than `code_panel.rs` at 09:13 and the session/settings
      panel sources), the Add Comment gutter/toolbar test, empty-chat
      start/join-actions test, and model-list scroll-isolation test passed (3/3).
      These confirm headless interaction and layout bounds, not rendered
      appearance. The watcher remains active.
      After the latest integrated rebuild (2026-10-13 11:42 EDT), reran
      `tests/agent-file-attribution-smoke.mjs` and
      `tests/agent-code-mode-smoke.mjs` against `target/debug/ahead`; both
      passed with isolated workspaces/data homes and loopback mock providers.
      This revalidates scoped AGENTS.md/skill context, managed Code Mode search,
      cancellation/retry across Turso reopen, CodeAnchor-to-commit attribution,
      and clean shutdown against the current binary. No external model provider
      was contacted. Its GUI child again aborted before rendering with the
      HIServices XPC signal 6.
      Repeated both isolated mock-provider smokes after the latest integrated
      rebuild: managed Code Mode delegated search, cancellation, retry after
      Turso reopen and shutdown passed; the `/calendar` turn loaded the selected
      project `.agents/skills/` body alongside only its applicable root/source
      `AGENTS.md`, applied the edit, persisted the CodeAnchor and verified the
      staged commit attribution/cleanup. No external provider was contacted.
      The same rebuilt binary passed a fresh managed LSP matrix with
      `AHEAD_LSP_INSTALL_MISSING=1` and `AHEAD_LSP_INSTALL_ZED_HTML=1`:
      AHEAD installed Vtsls, BasedPyright and the live Zed HTML 0.3.2 extension
      into disposable caches, then passed TypeScript/JavaScript, Python and Rust
      completion/navigation, diagnostics and repair, semantic tokens, import
      actions, Rust formatting/rename, restart recovery and graceful shutdown.
      Fixture: `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-T8mhdS`.
      This verifies the latest proxy/catalog/server boundary, not native editor
      completion rendering or installed-extension grammar colors.
      The current-source `ahead-app --lib` suite then passed 320 tests; its two
      multiplayer lifecycle tests still fail at the missing-invite assertion
      after a localhost-enabled rerun, so collaboration remains a separate
      milestone. Current-source `ahead-agent --lib adapters::tests` passed all
      16 runnable tests; four opt-in tests are ignored by default. Separately
      ran the current-source
      `curated_acp_agents_initialize_with_isolated_homes` test: Pi, Codex and
      Claude Code each initialized through their curated adapters using
      isolated homes and no model turns (1/1). The authenticated Pi turn remains
      unrun because `OPENROUTER_API_KEY` is unset in this execution environment;
      no credential was copied from chat into a shell command. Stopped
      watcher 43803 before Cargo checks, then restarted `just dev` as session
      7925 on the same disposable project and isolated Turso home. Its
      integrated build passed in 2.62s, then macOS again aborted the app child
      before rendering with the HIServices XPC signal 6. Watcher 7925 remains
      live; native UI acceptance is still open. On 2026-10-13, reran the
      fixture's documented offline `just verify` in place: Rust 1/1,
      TypeScript/JavaScript 2/2 and Python 1/1 pass under the installed Node
      24.15. A default-sandbox live-acquisition attempt stopped on DNS for
      `api.zed.dev`; the rerun with scoped network permission succeeded against
      the integrated binary rebuilt at 12:50 EDT. It installed the live Zed
      HTML extension 0.3.2, passed HTML completion, and acquired Vtsls,
      BasedPyright and Rust Analyzer into the scripted disposable cache. The
      matrix passed completion/navigation, diagnostics and repair, semantic
      tokens, TypeScript import actions, Rust formatting/hover/signature/
      references/cross-file rename, unsaved close/reopen, server restart and
      graceful proxy shutdown. Workspace/cache: `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-MLnteW`.
      This is real catalog/server acceptance, not native editor rendering or
      extension grammar colors. Against the same integrated binary,
      `tests/agent-code-mode-smoke.mjs`,
      `tests/agent-file-attribution-smoke.mjs` and
      `tests/editor-recovery-smoke.mjs` all passed with isolated temporary
      projects and profiles. They cover streaming/delegated search/cancel/retry,
      standards-based context through CodeAnchor-to-commit, and crash recovery,
      discard fencing and initial-schema owner-only storage.
      `tests/lsp-shutdown-smoke.mjs` also passed all four fake-server modes
      (shutdown, stdin EOF, initializing, unresponsive) against that binary;
      the source file remained unchanged. The test now gives the proxy and
      fake server isolated AHEAD homes, forwards an explicit environment
      allowlist, and checks that a synthetic secret sentinel is not inherited.
      Rechecked in this continuation against the 2026-10-13 12:50
      `target/debug/ahead`: `tests/agent-file-attribution-smoke.mjs` passed
      the managed scoped-instructions/skill edit, Turso CodeAnchor, staged Git
      commit, AHEAD author/human committer, session trailer, and cleanup path;
      `tests/agent-code-mode-smoke.mjs` passed delegated search, cancellation,
      retry after Turso reopen, and shutdown; `tests/editor-recovery-smoke.mjs`
      passed Unicode-buffer crash recovery, live-owner protection, discard
      fencing, and owner-only database permissions. All used disposable
      fixtures and mock/no providers. Recovery fixture retained at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-recovery-smoke-JvBc9W`.
      These are proxy/storage integration results, not rendered editor or
      authenticated-provider acceptance.
      On 2026-10-13, the rebuilt disposable-project binary passed all four
      fake-LSP shutdown modes (shutdown, stdin EOF, initialization and
      unresponsive server). The separate offline real-server smoke stopped at
      preflight because Vtsls, BasedPyright and a runnable Rust Analyzer are
      absent from this PATH; it installed nothing and made no network request.
      Earlier managed acquisition and real-server matrix runs remain the live
      LSP evidence; native editor and extension-installer rendering remain open.
      The source-newer 2026-10-13 app runner (13:45:54, newer than the
      session-panel/settings sources) passed all 54 `session_panel::tests` and
      all 29 `settings_panel::tests`. Coverage includes session start actions,
      provider/model search and visibility, model load/reload behavior, slash
      skills, provider-scoped collapsible model lists, virtual-list scroll
      isolation and endpoint response handling. These are headless GPUI tests,
      not native rendered acceptance. Afterward, the `target` test/app binaries
      were cleaned; the existing watcher session remained live but has no
      rebuilt app binary, so restart it before further executable checks.
      Later, the shared watcher rebuilt `target/debug/ahead` at 17:49. Against
      that binary, the managed Code Mode smoke, file-attribution-to-staged-commit
      smoke, editor-recovery smoke, all four LSP shutdown modes, and a cache-
      seeded real LSP matrix passed. The matrix covered Rust, TypeScript,
      JavaScript and Python completion/navigation, diagnostics/repair, semantic
      tokens, supported formatting/rename/import actions, restart and shutdown.
      The new LSP fixture is
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-OyDqHe`.
      A later ignored curated-ACP model-options retry stopped before agent
      launch when npm returned `ENOTFOUND` for `registry.npmjs.org/pi-acp`; no
      prompt or provider request ran. Earlier isolated ACP handshake evidence
      remains valid. The shared `just dev` watcher is active, but the desktop
      remains locked and native rendering is still open.
      On 2026-10-14, the empty-state New AHEAD session and External agent
      actions no longer inherit the busy/disabled state used for session
      sharing and joining, so an in-flight invite cannot strand the user in
      the empty panel. The GPUI regression clicks both actions with that state
      set; all 54 `session_panel::tests` passed. The integrated `just dev`
      build passed, but macOS again aborted before window creation with the
      HIServices XPC error; watcher session 51652 remains live and CUA
      attachment times out (-10005). Native rendered click acceptance remains
      open.
      Fresh continuation checks against the current integrated
      `target/debug/ahead` passed: the offline, cache-seeded real-server LSP
      matrix covered Rust, TypeScript, JavaScript and Python; the managed
      Code Mode smoke covered eight concurrent delegated `file_search` calls,
      streaming, cancellation, Turso reopen/retry and shutdown; the attribution
      smoke covered a managed edit,
      persisted session CodeAnchor, staged commit metadata and anchor cleanup;
      and the recovery smoke covered acknowledged Unicode recovery, live-owner
      fencing, durable discard and owner-only database permissions. The two
      agent smokes used isolated loopback mock providers; no authenticated
      provider or Cargo build was used. The dev watcher remains live, while
      native rendering remains unavailable due the HIServices startup abort.
      Rechecked at 23:13 ET on 2026-10-14 against the newly rebuilt binary:
      managed Code Mode, file-attribution-to-staged-commit, editor-recovery,
      and all four LSP shutdown modes passed. The two fake-provider smokes
      required sandbox permission for their disposable loopback listeners; no
      authenticated provider call or separate Cargo job was used. Native UI
      acceptance remains open because the Mac is locked.
      Fresh recheck on 2026-10-15 against the current shared-loop binary:
      `tests/agent-code-mode-smoke.mjs` passed streamed Code Mode execution,
      eight concurrent delegated searches, cancellation, retry after Turso
      reopen and shutdown; `tests/agent-file-attribution-smoke.mjs` passed
      source-write denial, managed editing, Turso CodeAnchor persistence,
      staged Git attribution and anchor cleanup; `tests/agent-memory-smoke.mjs`
      passed isolated user/project memory append, search, reviewed replacement,
      stale-snapshot rejection and Turso reopen. Loopback mock-provider smokes
      required scoped local-network permission; the memory smoke passed without
      it. No authenticated provider request was made because
      `OPENROUTER_API_KEY` is not configured in this process. Native rendering
      remains open.
      Follow-up against `target/debug/ahead` built at 17:35 on 2026-10-15:
      Code Mode and shutdown smokes passed again. They cover eight concurrent
      delegated searches, streaming cancellation, retry after Turso reopen,
      explicit shutdown, stdin EOF and SIGKILL recovery of partial output and
      instruction provenance; explicit retry starts once. The isolated memory
      smoke again passed project/user append, search, reviewed replacement,
      stale-snapshot rejection and Turso reopen. Mock-provider smokes used
      scoped loopback access; memory used no network. The attribution smoke was
      not rerun because it invokes Cargo inside its fixture while shared watcher
      8471 is live. No Cargo build or authenticated-provider request was made.
      Native rendering remains open.
      On 2026-10-20, the no-session chat now hides the generic “Start a
      session…” status row and shows it only for actionable feedback. The GPUI
      regression asserts the idle row is absent while unavailable-action and
      invalid-invite feedback remain visible. Formatting, whitespace and the UI
      detector pass; the GPUI test was not run because no app test binary is
      available after the clean and only 2.5 GiB is free. Native rendering
      remains open.
      On 2026-10-21, three guarded `just dev` attempts targeted a fresh
      Git-initialized copy of the smoke fixture with isolated AHEAD data/user
      homes. A separate Cola Cargo check reappeared after each preflight; I
      stopped only the AHEAD watcher to avoid parallel builds. The latest run
      compiled for about three minutes before stopping. Its partial root target
      is 3.8 GiB, 6.0 GiB is free, and no app binary or render was produced.
      The macOS desktop is unlocked. The shared-loop regression passed again:
      a duplicate watcher exits 75 before Cargo.
- [ ] Finish AHEAD-owned Clippy warning cleanup before calling the build
      production-ready. On 2026-10-01,
      `cargo clippy --locked --offline -p ahead-agent -p ahead-app -p ahead-proxy --lib -j 1`
      passed with zero errors and 80 warnings. Seven redundant metadata clones
      in `ahead-agent/src/native_client.rs` are removed. DAP/LSP timeout maps
      no longer use `HashMap::extract_if`, which exceeds the declared Rust
      1.87 MSRV; six focused proxy timeout tests pass. A later agent-only
      Clippy pass was stopped when free disk fell below 700 MiB. It identified
      and removed an obsolete MCP lint expectation and two mechanically
      equivalent retained-runtime warnings. A subsequent focused Clippy check
      of all three changed crates passed; the MCP crate now reports only its
      two unrelated existing warnings. The full AHEAD Clippy gate remains open,
      and free disk fell below 250 MiB before two stale rebuildable proxy test
      executables were removed to restore limited headroom. Verify with the
      declared workspace MSRV and reduce AHEAD-owned warnings. The broader
      `-D warnings` attempt previously stopped on retained upstream
      `codex-utils-path-uri` suggestions; decide a scoped policy for pinned
      upstream warnings without globally silencing new AHEAD diagnostics.
      A post-clean 2026-10-01 pass including `ahead-extension-host` now succeeds
      after reversing an MCP approval branch rejected by `clippy::if_not_else`:
      52 `ahead-app`, 12 `ahead-proxy`, zero `ahead-agent`/extension-host and
      12 retained-runtime warnings (76 total). The 125-test agent suite passes.
      A 2026-10-02 focused pass after mechanical AHEAD-only cleanup reports
      51 `ahead-app` and 2 `ahead-proxy` warnings. The proxy remainder is one
      complex callback type and one long dispatch signature; do not hide these
      with blanket lint allowances. Focused language-detection and Settings
      lock tests pass. A proxy-only Clippy pass took four minutes because
      changing the selected package set rebuilt shared dependency features;
      keep subsequent build selections stable. A subsequent stable package-set
      pass reduced `ahead-app` from 51 to 44 warnings with behavior-preserving
      source edits; `ahead-proxy` remains at 2. The full 172-test app library
      suite and repository rustfmt check pass. The remaining app warnings are
      mostly callback complexity and action-specific cleanup, not silently
      suppressed.
      A 2026-10-03 post-clean pass over `ahead-agent`, `ahead-app` and
      `ahead-proxy` now succeeds. ACP permission cancellation, MCP form and
      approval handling, attribution line conversion and mechanical UI/proxy
      cleanup removed all target-crate errors and non-structural warnings.
      Remaining AHEAD warnings: 26 app and eight proxy, all
      `type_complexity` or `too_many_arguments`; agent has zero. Do not add
      one-use wrapper types solely to suppress these. The full offline
      `just test-all` recipe passed after the changes (agent 149/3 ignored,
      app 204/0, proxy 219/1 ignored, plus host/runtime and retained targets).
      The earlier 1.87 note is stale; the workspace now declares Rust 1.90.0.
      Full workspace MSRV verification remains open.
      On 2026-10-04, the same scoped package set passed Clippy with zero
      errors and 51 warnings. This pass fixed the AHEAD-owned semicolon lint
      error, removed `search_one_path`'s excess argument by keeping its
      cancellation predicate at the worker boundary (all 33 focused
      `ahead-core` search tests pass), and removed two temporary allocations
      from code-action stale-buffer comparisons. Remaining warnings are mostly
      structural type/argument complexity plus retained-runtime suggestions;
      do not hide them with blanket allowances or one-use wrappers. The
      single-range snippet vector is intentional because tabstops can own
      multiple ranges. Full workspace Clippy and Rust 1.90 verification remain
      open. On 2026-10-05, removed the unread `agent_role` rollout projection
      and stale typed test fields; all 61 `codex-rollout-trace --lib` tests
      pass. On 2026-10-07, the shared `just dev` app build exposed an unused
      `BTreeMap` import and a library-only dead-code warning for a helper used
      only by editor tests; the import is removed and the helper is now
      `cfg(test)`. A focused `cargo check --offline --locked -p ahead-app
      --tests -j 1` also found an unused `AppContext` test import, which is
      removed. The repeated test-target check prints no AHEAD source warnings;
      full workspace Clippy remains open. On 2026-10-13, `ahead-core` directory
      creation and UTF-16 decoding Clippy warnings were fixed without changing
      the existing user/data-home overrides or encoding behavior. The all-target
      `cargo +1.90.0 clippy --locked --offline -p ahead-core` check is warning-
      free, and `cargo +1.90.0 test --locked --offline -p ahead-core` passes all
      53 tests. This verifies `ahead-core`, not the entire workspace.
      Rechecked on 2026-10-14: `cargo +1.90.0 clippy --locked --offline
      --workspace --all-targets -j1` rejects the locked Wasmtime 48.0.2 /
      Cranelift 0.135.2 dependency closure before compilation because those
      packages require Rust 1.95. `rust-toolchain.toml` selects stable (1.98.1
      here). The workspace minimum has since been raised to Rust 1.95 in line
      with the locked Wasmtime/Cranelift requirements; CI and
      `rust-toolchain.toml` use stable. The full dependency graph still needs
      validation under Rust 1.95. A stable all-workspace Clippy attempt was stopped
      before reaching AHEAD crates when free disk fell below 1.6 GiB; it
      surfaced existing retained `codex-utils-path-uri` and
      `codex-shell-command` warnings. A focused Rust 1.90 core retry was also
      stopped during a fresh vendored OpenSSL build at 1.1 GiB free. Neither
      attempt is a passing gate; do not repeat broad Clippy without adequate
      disk headroom. After updating the workspace declaration and build guide
      to 1.95, the existing shared `just dev` loop accepted the manifest and
      rebuilt `ahead`/`ahead-app` with stable in 14.73s. The app then hit the
      same pre-render HIServices XPC abort. This does not verify compilation or
      Clippy at the declared 1.95 minimum.
- [ ] Finish cross-platform storage privacy verification in
      `ahead-proxy/src/ahead/store.rs::SessionStore::open`. Unix creation and
      supported-store reopen now enforce mode 0600 for the database and
      sidecars; owner, regular-file, link and non-shared-write-directory checks
      reject unsafe paths. Focused tests cover journal/WAL/SHM modes, preserving
      existing rows when the initial schema is ensured, and actual hot-journal
      recovery. Native startup changed the existing disposable database from
      0644 to 0600; the real proxy recovery probe also
      passed new/reopened mode checks. Verify Linux behavior, Windows ACL
      enforcement and inherited ACLs.
      The Windows path now secures the database directory, main file and
      existing recovery sidecars with current-user-only DACLs before libSQL
      opens them; new sidecars inherit the protected directory ACL. It rejects
      reparse points and hard-linked files and does not reopen the live DB to
      apply ACLs. Windows regressions cover existing DB/WAL byte preservation,
      orphan-sidecar preservation, hard-link rejection, ACL tightening, secure
      first creation and inherited ACLs on new sidecars; the existing Windows
      CI `memory_` filter includes them. This host cannot compile or run that
      Windows-only code, so require the Windows CI result before relying on it.
      Four-process concurrent first-open/history/metadata writes now pass ten
      consecutive runs on macOS. Blocked-commit regressions cover history,
      metadata, titles, workflow, thread deletion, recent files, memory indexing
      and turn requests: rollback retains prior data and permits a retry on the
      same connection. Connections use a five-second busy timeout and reserve
      history ordinals under an immediate transaction, following Zed's bounded
      contention handling. The storage filter now runs explicitly in macOS and
      Windows CI too; those CI results remain unverified here. Verify that
      persistent contention reaches actionable native UI state. Keep production
      data intact and never silently replace an unavailable database with
      in-memory state. Disposable
      pre-production stores may be recreated when the initial schema changes;
      no schema-upgrade path is required yet.
- [ ] Fix cross-language editor highlighting in `ahead-app/src/code_panel.rs`:
      language selection now follows the file type on creation and preview
      reuse, with a passing GPUI switch regression and a dynamic status footer.
      The normal build now enables gpui-kit's JavaScript/JSX,
      TypeScript/TSX and Python Tree-sitter parsers alongside Rust and JSON.
      The focused editor regression parses valid examples for each file type
      without errors and verifies multiple highlight spans; all 172 app
      library tests and the `ahead` binary check pass locked/offline. On
      2026-10-09, the source-matched app runner (12:20, newer than
      `code_panel.rs` at 11:37) passed
      `preview_switches_update_the_editor_language` (1/1), covering preview
      reuse across Rust, TypeScript/TSX, JavaScript/JSX, Python, JSON and plain
      text. A rebuilt native AHEAD Dev render in the disposable
      `ahead-comment-ui` project showed Python syntax coloring in `main.py`.
      Native switching across all languages and extension-provided grammar
      rendering remain unverified. The gallery can install Zed packages with
      grammar assets; confirm the installed-package-to-rendered-editor path in
      a live disposable project.
      `ahead-extension-host` now retains each Zed language's `grammar` entry,
      accepts grammar-only extension packages, and exposes bounded WASM plus
      optional highlights/injections query assets with symlink checks.
      The earlier 01:01 host binary passed the grammar-asset and symlink tests,
      but later `host.rs` edits made those results stale. On 2026-10-07, the
      shared clean `just dev` build compiled the current host, then the focused
      manifest/config symlink regression passed under locked/offline Cargo; the
      complete current host suite passed with 26 passed, 2 ignored. Manifest
      and language-config reads now use the same bounded no-symlink asset
      reader as grammar/query assets. These tests verify package loading and
      path safety, not editor parsing. Rechecked on 2026-10-08 from the host
      runner built at 04:39:18 (newer than `host.rs` at 04:38:07): 29 tests
      passed and 2 were ignored in the full serial run. The two async HTTP tests
      were denied loopback binds by the default sandbox; rerunning those exact
      cases with local-loopback permission passed both. Thus all 31 active
      tests pass, with no external HTTP requests. At that point (2026-10-08), a
      repository search found `language_syntax_for_path` only in its export,
      definition and tests; the production proxy/editor route was added later
      (see 2026-10-14 below). The pinned
      GPUI Kit 0.6.6 already exposes
      `LanguageRegistry::register_parser_factory`, but that callback is
      synchronous and consumes tree-sitter 0.26 types. Its tree-sitter `wasm`
      feature pulls Wasmtime C API 36 while the extension host uses Wasmtime 48;
      the published gpui-component 0.7.1 still pins tree-sitter 0.26.13, so a
      GPUI Kit upgrade alone does not remove this seam.
      Zed's `LanguageRegistry::get_or_load_grammar` loads grammar bytes on its
      executor and feeds a shared parser pool using its tree-sitter
      0.27/Wasmtime 48 pairing (`../zed/crates/language/src/language_registry.rs`,
      `../zed/crates/language/src/language.rs`). Do not pass the incompatible
      Tree-sitter `Language` types across the GPUI boundary or do file/WASM work
      in its synchronous factory. The 2026-10-07 design exploration used Zed's
      `crates/language/src/syntax_map.rs` (`QueryCursor` over parsed syntax
      layers) and `crates/language/src/language.rs::build_highlight_map`
      (capture names to theme entries) as references, and proposed returning
      extension capture byte ranges for GPUI `TextDecorationCollection`. The
      production implementation and its separate-parser boundary are recorded
      under 2026-10-14 below. Historical dependency check
      (2026-10-07): Tree-sitter 0.27.0 source and crate archive are cached, but
      the optional `wasmtime-c-api-impl` 48.0.1 source/archive needed by its
      `wasm` feature is not; `Cargo.lock` currently resolves Tree-sitter
      0.26.13 only. The filesystem had 51 GiB free at the latest check, so disk
      is no longer the blocker. Resolve the exact 48.0.1 dependency through the
      shared build loop, then implement and test real installed-extension grammar/query
      parsing, stale-range handling, and rendered colors before closing this
      gap. Rechecked on 2026-10-08: the exact 48.0.1 C API archive is still
      absent. The GPUI-compatible Tree-sitter 0.26.13 WASM feature also requires
      `wasmtime-c-api-impl` 36.0.14; only 36.0.9 is cached, so neither version
      can be enabled offline. Keep the pinned Zed 0.27 parser path; do not
      silently substitute an incompatible GPUI parser type or add foreground
      WASM loading. Rechecked 2026-10-12: Tree-sitter 0.27.0 source and archive
      are cached, but `wasmtime-c-api-impl` 48.0.1 remains absent; the shell's
      `git ls-remote` probe could not resolve `github.com`. No Cargo build was
      started, and no untestable parser integration was added.
      Rechecked on 2026-10-13 after the latest extension-host edits: the full
      serial `ahead-extension-host --lib` suite passed 37 tests, with 2 ignored,
      when its in-process loopback tests were allowed. This exposed and fixed a
      test-only mock-server issue: it previously closed after one TCP read,
      before request headers were necessarily complete; it now reads the
      bounded header through its terminator. At that time (2026-10-13), the
      language grammar asset reader still had no production editor/proxy caller;
      the route was added the following day. The restarted shared `just dev` loop rebuilt the
      integrated app in 21.54s, but macOS again aborted before rendering with
      the HIServices XPC signal 6. After the scoped rustfmt check, the same
      watcher rebuilt the current source in 14.32s and hit the same pre-render
      macOS abort; watcher session 90764 remains live on the disposable
      `/private/tmp/ahead-dev-smoke.7T4vXa/project` with isolated data at
      `/private/tmp/ahead-dev-smoke.7T4vXa/data`.
      Rechecked the built-in highlighting boundary on 2026-10-13 using
      `ahead_app-e1173e43572818d7` (12:57, newer than `code_panel.rs` at 11:10):
      `preview_switches_update_the_editor_language` passed, parsing and checking
      multiple highlight captures for Rust, TypeScript/TSX, JavaScript/JSX,
      Python and JSON while switching previews; unknown extensions remain plain
      text. This does not exercise an installed Zed grammar; that integration
      remains open pending the compatible parser/WASM runtime dependency.
      Revalidated on 2026-10-14: installed extension grammars now have a
      production path. The proxy parses bounded source with the installed
      `highlights.scm` off the UI thread; a bounded cache reuses loaded grammar
      and query assets using safe size/mtime stamps, with the parser/query keyed
      by extension/grammar asset digest. It returns capture byte ranges,
      and the app validates UTF-8 boundaries plus request generation, path and
      buffer length before applying the active GPUI syntax theme. It follows
      Zed's `get_or_load_grammar`/`with_parser` lifetime pattern while using
      Tree-sitter 0.26.13 to match GPUI Kit; enabling its WASM support adds
      Wasmtime C API 36 alongside the extension host's Wasmtime 48. The first
      uncached test-profile build took 5m36; the shared incremental dev build
      took 16.98s. The real installed HTML extension capture test passed, as did
      app capture-range, proxy-route and mounted stale-response regressions
      (3 app tests). The host now applies bounded `injections.scm` queries,
      including combined and nested injection ranges; installed-HTML tests
      cover combined script blocks and injected captures. Zed's
      `../zed/crates/language_core/src/queries.rs::QueryFile` does not include
      `locals.scm`; AHEAD now likewise ignores that unrelated file rather than
      loading an unused query. The rebuilt app again aborted
      before drawing with the macOS HIServices XPC signal 6, so native rendered
      extension colors and plugin-installed language switching remain open. A
      mounted GPUI regression now rejects an old same-path syntax response after
      an edit, then applies the current response's capture range; that test also
      passes on the current app test runner. Revalidated on 2026-10-14: the
      current GPUI runner passed
      `extension_syntax_applies_only_the_current_proxy_response` (1/1), and the
      installed HTML extension host tests passed capture-range and combined
      injection coverage (2/2) against the cached Zed HTML package. These still
      do not prove rendered colors in the app. On 2026-10-15, the opt-in live
      HTML install smoke failed before catalog access because `api.zed.dev`
      could not resolve (`ENOTFOUND`); no package was installed by that attempt.
      Revalidated on 2026-10-15 against current source: the installed-HTML host
      tests passed (2/2) for real Tree-sitter captures and combined script
      injections, and the GPUI stale/current response regression passed (1/1),
      applying the current `tag` capture through the active syntax theme.
      Together these verify the installed grammar/query parser and editor
      decoration acceptance separately; the real package-to-proxy-to-rendered
      color path still needs a live desktop check. The restored shared
      `just dev` loop rebuilt the integrated app in 3.01s, but macOS again
      aborted the app child before rendering with
      `com.apple.hiservices-xpcservice` (signal 6). The same watcher remains
      active on the approved disposable project.
      On 2026-10-19, extension-catalog rediscovery now advances a syntax
      generation through the existing proxy metadata notification; open editor
      buffers re-request extension-provided syntax after install/restart rather
      than waiting for a file edit or reopen. The existing mounted GPUI syntax
      test now also delivers that notification and asserts the open buffer
      requests and applies the refreshed capture. This source-current extension
      refresh regression has not yet been run in test mode.
      The shared `just dev` rebuild compiled the integrated app in 8.99s and
      the proxy entered its main loop; no test-mode Cargo command was started.
- [ ] Verify Settings autosave and close/quit in the rebuilt native app.
      `ahead-app/src/settings_panel.rs::save_config` now serializes writes on a
      background worker with one coalesced pending edit. It holds the lock
      across read/merge/write, ignores stale completion and preserves failed
      drafts. Window close and quit wait for accepted saves and reject errors,
      timeouts or newer edits. GPUI regressions cover contention, concurrent
      MCP changes, retry and multi-window quit; they are not native proof.
      Constructor/bootstrap and external reload now use background tasks;
      provider controls stay disabled until initial loading finishes. The
      existing workspace watcher drives reloads, with no render-time reads.
      Verify that startup/loading presentation in the native app too. The lock
      explicitly unlocks on scope exit so a fork-inherited handle cannot delay
      release; verify Linux/Windows behavior.
- [ ] Verify model-picker loading in the rebuilt native app. Initial reads,
      Settings callbacks and watcher timestamp checks in
      `ahead-app/src/session_panel.rs` now run off the UI thread. GPUI tests
      cover unsent drafts during initial loading, selection changes during a
      reload, reordered/removed models, session status preservation and queued
      forced reads with unchanged timestamps. The model name is the selector's
      primary label; provider/server identity appears as muted secondary text
      in the option and beside the selected model. In a fresh native build on
      the disposable multi-language fixture, automatic discovery populated both
      Settings visibility switches and the chat picker from its configured
      OpenRouter endpoint; toggling one model off and on worked. The selected
      model and muted endpoint label were visible in the picker. On 2026-10-20,
      the single chat-picker trigger was tightened to place the active effort
      above the model name and muted provider, keeping the searchable model
      combobox and GPUI slider together in its popover. The source passes
      rustfmt and the design detector; this visual adjustment still needs a
      GPUI/native render check. A focused
      `chat_model_select_filters_by_provider_name` test build on 2026-10-20
      was stopped before reaching the test when free space fell to 2.2 GiB; no
      GPUI assertions ran. Native verification must still cover loading at
      narrow widths and managed/external transitions; no provider
      turn has been verified against these changes. On 2026-10-21, a retry
      began with 6.7 GiB free but rebuilt the retained engine/Wasmtime test
      dependency graph; a separate `cola-v2` Cargo test started concurrently.
      I stopped only the AHEAD compile at 3.7 GiB free to avoid exhausting the
      shared disk. It did not reach the GPUI test, and no caches were deleted.
      Settings
      "Test connection" and model discovery now share the OpenAI-compatible
      `/models` endpoint; the probe reports non-success HTTP status rather than
      treating a bare-base URL response as successful. The SettingsPanel scalar
      fallback now drops a private API key when a project-layer Base URL differs;
      its exact-match/mismatch regression passed. The model-picker load and
      hidden-model selection regressions also passed. Model visibility is
      grouped by provider, collapsed by default, and rendered with GPUI's
      virtual `uniform_list`; hidden models remain scoped to their provider.
      Provider collapse state and each provider's virtual-list scroll handle
      now key by stable provider ID, so reordering or refreshing settings does
      not transfer one server's UI state to another. A reorder regression was
      added but has not been run.
      On 2026-10-07, an offline Cargo run against current source passed all 24
      `settings_panel::tests` and all 7 `model_picker` tests, including the
      duplicate-provider identity regressions. This continuation re-ran the
      same 31 tests directly from the source-matched app test binary (05:07:54,
      newer than `settings_panel.rs` at 00:37:14 and `session_panel.rs` at
      05:07:23), with no Cargo build. Source uses GPUI `uniform_list`
      for provider/model rows, with provider sections
      collapsed initially and hide state persisted independently per provider.
      Rechecked on 2026-10-16 from the source-current app test runner: all
      eight `model_picker` tests pass serially, covering provider grouping,
      visibility, search, loading/reload and preservation of the selected model.
      This does not verify native picker focus or narrow-width rendering.
      On 2026-10-19, successful `/models` refreshes now replace that provider's
      discovered catalog, while failed endpoints retain their last-known models
      and explicitly configured models remain in the picker. Added a regression
      for replacement, offline retention and configured-model preservation; it
      has not been run in this source checkout.
      The discovery result now distinguishes the authoritative configured
      provider set from successful refreshes: removing a provider evicts its
      cached catalog, endpoint outages preserve models only for still-configured
      providers, newly hidden IDs are removed even during outages, and unreadable
      settings preserve the last-known cache. Config reload no longer clears
      that cache before reconciliation. Added session-panel regressions for
      provider removal, hidden IDs during outage, and settings-read failure;
      they have not been run.
      The 2026-10-16 source-current `session_panel::tests` suite also passed
      serially (60/60) after rebuilding the app test runner. This includes the
      rendered ACP model-option selection/update path. The restored shared
      `just dev` loop rebuilt the integrated binaries in 2.57s on the same
      disposable project/profile, but the app child again aborted before
      drawing with the macOS HIServices XPC error (signal 6); watcher session
      48426 remains live. Native picker focus, scrolling and narrow-width
      behavior remain unverified.
      On 2026-10-08, the source-current offline Cargo runs for
      `chat_model_select_filters_by_provider_name` and
      `model_selector_searches_model_and_provider_names` both passed. They
      confirm the focused picker search field filters by provider or model and
      allows selection. The same `just dev` run rebuilt `ahead-app` and the
      root binary, but macOS again aborted before rendering with an invalid
      `com.apple.hiservices-xpcservice` connection (signal 6); native search
      field appearance and focus remain unverified.
      Rechecked from the 04:13:22 app test runner (newer than the current
      `session_panel.rs` and `settings_panel.rs` sources): all 52
      `session_panel::tests` and 29 `settings_panel::tests` passed serially.
      This includes provider-scoped visibility, virtual-list wheel containment,
      search by provider/model name with selection, model discovery/reload
      fencing and slash-skill filtering/selection. Native scrolling, focus and
      narrow-width rendering remain open.
      On 2026-10-09, re-ran all 29 `settings_panel::tests` directly from the
      existing 2026-10-08 15:18 app test runner; 29/29 passed in 2.18s without
      Cargo. The runner is newer than `settings_panel.rs` (2026-10-07 23:01)
      but older than `session_panel.rs` (2026-10-08 19:52), so it does not
      verify the latest model-picker source or any native rendering.
      On 2026-10-08, the 15:18 app runner (newer than `session_panel.rs` at
      14:42) passed all 29 settings tests, but the serial session suite passed
      only 52/53; `chat_model_select_filters_by_provider_name` also failed
      when run alone, leaving OpenRouter selected after the test's manual
      mouse-down/up at the filtered Anthropic row. That test omitted pointer
      movement; gpui-kit `simulate_click` dispatches movement before down/up.
      Replaced its three manual click sequences with `simulate_click`. This
      corrected the test interaction path; a rebuilt rerun was still required
      at that point before claiming selection behavior passed.
      Follow-up on 2026-10-09: the direct app runner at 12:20:14, newer than
      `settings_panel.rs` (2026-10-07 23:01), `session_panel.rs` (12:19:40), and
      `code_panel.rs` (11:37:41), passed all 29 Settings, 54 session-panel,
      and 61 code-panel tests serially without Cargo. Coverage includes
      provider-scoped model visibility, nested-list wheel containment, model
      search/filter/selection, empty-session button hit tests, Add Comment
      visibility, gutter hit targets, inline blame, diff previews and completion
      placement/ranking. This is headless GPUI evidence; native rendering,
      focus and OS pointer behavior remain separate acceptance gates.
      These headless regressions do not verify native scrolling or narrow-width
      rendering. On 2026-10-06, eight focused provider/model tests passed
      directly (one each), covering provider grouping/filtering,
      per-connection persistence, catalog parsing, endpoint construction and
      status, and stale-result fencing. An earlier full GPUI rerun still hit
      macOS application-service errors; verify rendered scrolling, narrow
      widths, and the full settings interaction after unlocking the desktop.
      On 2026-10-07 at 10:45, the direct app test executable, newer than
      `settings_panel.rs` at 10:45:01, passed
      `model_list_scroll_does_not_scroll_settings_page` without Cargo. The GPUI
      regression confirms the virtual model list advances while the enclosing
      Settings page receives no wheel event; native nested-scroll behavior
      remains to be visually verified.
      After a later report that the page still moved while the pointer was over
      the list, the model viewport was changed to consume wheel input locally
      throughout the viewport, including at its edges. The regression now
      checks page position after an inner scroll and at both the top and bottom
      limits, including that the inner offset remains clamped at the bottom.
      The source-current headless gate passed at 19:36 on 2026-10-07 by running
      `model_list_scroll_does_not_scroll_settings_page` directly from
      `ahead_app-601e2b645b3fbe4a`, newer than `settings_panel.rs` at 17:15.
      It verifies inner-list scrolling at its limits and page scrolling outside
      the list. The same runner passed all 28 `settings_panel::tests`; the
      source-current model-picker search tests for model and provider names
      also passed. Native wheel/trackpad and scrollbar-drag behavior remain
      open.
      At the same time, all 25 `settings_panel::tests` and 7 `model_picker`
      tests passed directly from that executable (newer than
      `session_panel.rs` at 08:22:15), with no Cargo build. This includes
      provider-scoped hidden models, collapsed virtual-list behavior, settings
      reload races and the nested-scroll regression; rendered/native behavior
      remains a separate gate.
      The model-list viewport now forces GPUI Kit's `ScrollbarMode::Always` and
      reserves its 16px rail so it does not overlap model switches. On
      2026-10-07, the disposable `just dev` build completed in 2m16s with this
      production code; app launch then aborted with macOS
      `com.apple.hiservices-xpcservice` “Connection Invalid” (signal 6), so no
      rendered result is claimed. The retained fresh fixture copy is
      `/private/tmp/ahead-model-scroll.ozavFa/project`; it has no `.ahead/`
      because launch failed before project bootstrap. After the one unused
      import was removed, the current app test executable (11:08:33), newer
      than `settings_panel.rs` at 11:07:14, passed the scroll regression, all
      25 Settings tests and all 7 model-picker tests via focused offline Cargo
      runs. Native scrollbar
      visibility and drag behavior remain unverified.
      2026-10-07 follow-up: the nested-scroll test now mirrors the Settings
      page's GPUI virtual `List` parent. It proves wheel input over model rows
      advances the model list without moving the page list, while input outside
      the model area scrolls the page. Both production UI and test use the same
      `model_visibility_scroll_area` helper, and all 25 settings tests pass
      offline. `just dev` rebuilt the app successfully, but native launch
      aborted with macOS `com.apple.hiservices-xpcservice` “Connection Invalid”
      at 11:19:11 and again at 11:22:23 after the shared-helper change, so
      actual in-app scrolling remains unverified.
      Current 2026-10-07 recheck: `settings_panel.rs` is dated 00:37:14;
      direct runs from the 05:07:54 test binary passed
      `model_visibility_rows_are_provider_scoped_and_collapsed_by_default`,
      `parses_hidden_models_per_connection`, and
      `duplicate_provider_ids_use_the_last_profile_in_settings` (one each, no
      Cargo build).
      Latest nested-scroll repro: gpui-base 0.6.6's capture-phase
      nested `overflow_y_scroll` handlers bubble the same wheel event through
      each ancestor without stopping propagation, so both the model list and
      Settings page move. `ModelListScrollMask` now captures wheel input across
      the model viewport, applies it to the virtual list, and stops propagation
      even at the top and bottom limits. The GPUI regression checks inner-list
      movement, outside-page scrolling, and both edge limits. Changed source
      still needs a rebuilt focused test and native verification. Recheck at
      12:43 after the user's Cargo clean: `target/debug` and `target/debug/deps`
      exist, but neither the `ahead` executable nor an `ahead_app*` test runner
      is present. The Codex app terminal is unattached and process listing is
      denied, so don't start a competing build; rerun the focused test when the
      shared `just dev` build produces its runner.
      That binary predates `session_panel.rs` at 08:22:15, so its chat-picker
      regressions do not establish current-source behavior. Native scrolling
      and narrow-width rendering remain unverified; they need a rebuilt app
      launched on the disposable fixture.
      A duplicate-identity edge is covered by those tests: when one config source
      repeats a `provider_id`, Settings and chat both use its last profile,
      hidden models from the replaced profile do not leak into the picker, and an
      active connection name still resolves to that provider. No app terminal
      is attached, so rendered/native behavior remains open.
      The disposable editor fixture now has `catalog-a` and `catalog-b`
      loopback modes, each returning `shared-model` plus 120 distinct IDs; a
      direct local endpoint check verified both 121-item catalogs. This tests
      fixture responses only, not rendered scrolling or provider-scoped UI
      behavior; use the Models row in `tests/fixtures/editor-smoke/README.md`
      for the native acceptance steps.
      Current source-matched check: the direct app test runner dated 11:55:41,
      newer than `settings_panel.rs` at 11:51:11, passed all 25
      `settings_panel::tests`, including nested model/page wheel isolation, with
      no Cargo build. `session_panel.rs` is newer than that runner (11:56:16),
      so this does not verify the current model-picker source. Native scrolling,
      scrollbar interaction and narrow-width rendering remain open.
      2026-10-07 13:40 update: the focused test was rebuilt and passed against
      current source. It now sends wheel input over a model row, over its Switch,
      after the outer Settings list has moved, and at both inner-list limits;
      only input outside the 280px model viewport moves the page. The viewport
      clips its rows and puts the transparent wheel-capture layer above the
      scrollbar and controls. `rustfmt --check` and `git diff --check` pass.
      A fresh isolated `just dev` run on
      `/private/tmp/ahead-model-scroll-e2e.0bpFnT/project` compiled the editor
      and Code Mode host in 25 seconds, but launch aborted with macOS
      `com.apple.hiservices-xpcservice` “Connection Invalid” (signal 6), so
      rendered/native scroll, scrollbar drag and narrow-width checks remain
      open.
      Follow-up: removed the custom `ModelListScrollMask` and its manually
      translated hitbox/offset math. The viewport now lets GPUI's virtual list
      process wheel input and stops propagation in the normal bubble phase,
      matching Zed's nested `List` event ordering. Rebuilt
      `model_list_scroll_does_not_scroll_settings_page` against this source;
      it passes with wheel input over rows and switches, outside the viewport,
      after page scrolling, and at both list limits. `rustfmt --check` and
      `git diff --check` pass. Native scroll/drag verification remains open
      because the disposable `just dev` app launch aborts with the macOS XPC
      service error above.
      A fresh `just dev` run at 13:50 rebuilt the changed editor and Code Mode
      host successfully in 19.72 seconds, then reproduced the same
      `com.apple.hiservices-xpcservice` “Connection Invalid” launch abort. The
      watcher was stopped before the focused one-off check; native scrolling
      still needs a rendered app run after the macOS service issue is resolved.
      A later locked/offline rebuild of the GPUI regression also passed against
      the current session/voice source, confirming the earlier transient
      `LiveCallEvent::Capabilities` exhaustiveness error is no longer present.
      `git diff --check` passes. No app was launched by this check.
      The runtime report above describes the pre-fix behavior. In the rebuilt
      disposable `just dev` app on 2026-10-07, scrolling over the 100-model
      virtual list moved visible rows from `scroll-test-001` through
      `scroll-test-008` to `scroll-test-028` through `scroll-test-036`, while
      the provider heading and Settings page remained stationary. The focused
      GPUI regression also passes from the rebuilt test executable. Narrow-width
      layout and scrollbar dragging remain separate native checks.
      Follow-up to the user's clarification: wheel input anywhere over the
      model viewport must advance only the provider's virtual list, not the
      Settings page. The propagation stop is now also attached directly to the
      model `uniform_list`, after its own overflow handler, while the viewport
      guard remains for scrollbar/empty-space input. File-scoped rustfmt and
      `git diff --check` pass; both headless and rendered wheel-isolation checks
      now pass.
      Native retest on 2026-10-07 in the shared `just dev` app used the
      disposable 100-model fixture: wheel input over the model rows advanced
      from `scroll-test-001`–`008` to `056`–`063`; wheel input over a model
      switch then advanced to `084`–`091`. The provider heading, explanatory
      text and Settings page stayed stationary in both cases. Rendered
      scrollbar dragging and narrow-width layout remain open.
      An authenticated model
      turn remains unverified. One native attempt against
      `nvidia/nemotron-3-ultra-550b-a55b:free` was rejected with HTTP 401 because
      the disposable workspace credential had already been cleared; no key was
      re-entered. Zed's OpenRouter parser
      (`../zed/crates/open_router/src/open_router.rs`) retains `context_length`
      and other model capabilities from `/models`. AHEAD now carries
      `context_length` or generic `context_window` into the picker, deduplicates
      by ID while filling metadata from duplicate entries, and falls back to
      32,768 only when context metadata is absent. The focused regression covers
      both field names, string-only catalogs, deduplication, and missing/zero
      values; it passed in the current focused app test run. Per-model context
      is now carried in the turn request through Turso retry persistence and
      into the managed runtime's per-thread model settings; clearing a model
      override falls back to the runtime startup setting. Regressions cover
      override/reset behavior and persisted retry requests. The clean `just dev`
      rebuild compiled the production path, and the focused empty-chat GPUI test
      passed. On 2026-10-06, serial `cargo test -p ahead-app --lib model`
      passed all 17 matching tests, including the 20-seed reload-coalescing
      case, provider-scoped filtering, catalog metadata and invalid-layer repair.
      Those tests now check the settled catalog generation and distinguish a
      repaired TOML warning from independent endpoint-discovery warnings. On
      2026-10-06, the AHEAD-native
      `managed_context_window_override_resets_to_startup_value` regression
      passed against a loopback mock: a 120,000-token model override applied on
      one managed turn, and an explicit clear restored the startup value on the
      next. The file-backed Turso
      `instruction_sources_survive_turso_reopen_after_retry_state_is_removed`
      regression also passed and round-trips `model_context_window` in the
      persisted retry request after closing and reopening the database. These
      AHEAD-boundary checks avoid restoring the disabled copied Codex Core test
      harness. An authenticated model turn remains unverified; retry after the
      user restores a test credential.
      Fresh direct GPUI regression run on 2026-10-07 used
      `ahead_app-856717cf3484b321` (17:15:44; newer than the Settings, session,
      editor, and lifecycle test sources): all 28 Settings tests, five model
      picker reload tests, both provider-scoped configured-picker tests, the
      ACP model-option test, session empty-state controls/join validation, all
      53 code-panel tests, and the narrow dirty-tab marker test passed without
      Cargo. The latest model-scrollbar content-size change compiled in the
      shared `just dev` build at 17:18:55. It still needs a rendered thumb/drag
      check, and Python LSP needs a live reconnect check; the Mac locked before
      those checks, so unlock is requested.
      Rechecked on 2026-10-08 without Cargo: the 00:17:30 app test runner is
      newer than `settings_panel.rs` (2026-10-07 23:01) and passed all 29
      `settings_panel::tests`, including the nested-scroll GPUI regression,
      provider-scoped visibility, discovery fencing, and bootstrap/save cases.
      `session_panel.rs` (2026-10-08 02:17) is newer than that runner, so this
      does not validate the current chat model picker. Native wheel/trackpad,
      scrollbar drag, narrow-width layout, and authenticated-provider behavior
      remain open while the Mac is locked.
      Rendered check in the running `AHEAD Dev` app on 2026-10-08, using the
      approved disposable 100-model project: searching `scroll-test-001`
      reduced the chat picker to that exact model; hiding `scroll-test-056`
      removed it from chat and showed the saved per-model status, then restoring
      it made it available again. Scrolling over the virtual list moved rows
      `scroll-test-001`–`008` to `056`–`063` while the provider header and
      Settings page stayed fixed. The provider's loopback endpoint is offline,
      so live discovery and an authenticated model turn remain unverified;
      rendered narrow-width layout and scrollbar dragging are also open.
      Rechecked 2026-10-08 18:56 EDT from the existing `ahead_app` test runner
      built at 15:18 (newer than `settings_panel.rs` at 2026-10-07 23:01): all
      29 `settings_panel::tests` passed serially without Cargo. This confirms
      the headless provider/settings behaviors, not native rendering.
      Rechecked on 2026-10-14 from `ahead_app-e1173e43572818d7`, built after
      the current `session_panel.rs` and `settings_panel.rs`: all 54
      `session_panel::tests` and all 29 `settings_panel::tests` passed directly
      without Cargo. This revalidates picker filtering/selection, provider-
      scoped virtualized visibility, and wheel isolation; native narrow-width,
      focus, scrollbar-drag and authenticated-provider checks remain open.
      After the user's clean, the 15:44 source-current runner also passed all
      21 app tests matching `model` serially, including provider/model search,
      picker startup/reload and stale-result fencing, provider-scoped hidden
      models, and virtual-list scroll containment. This was run directly
      without Cargo while the shared dev watcher was idle after app startup.
      The disposable-project checklist now matches the implemented refresh
      contract: discovery runs when the agent panel loads and when Settings
      reloads; opening or rerendering the picker does not issue duplicate
      catalog requests. Per-open refresh would require an open-change hook not
      currently exposed by gpui-kit Select.
      Rechecked against the rebuilt 20:09 app test runner on 2026-10-14:
      all 16 tests matching `model_` passed serially, including provider-name
      search, hidden-model scoping, model catalog normalization, failed/stale
      discovery handling and virtual-list wheel isolation. This is headless
      evidence; native search focus, narrow layout and authenticated endpoint
      behavior remain open.
      Rechecked on 2026-10-19 in the already-running disposable `just dev`
      app: opened the chat model picker, entered `nemotron`, confirmed the
      visible list filtered to one result, then closed the picker without
      changing the selected model or sending a provider request. This verifies
      native search/filter interaction at the current window size; narrow-width
      layout, scroll behavior and a live provider turn remain open.
      The chat selector now keeps model search/selection and reasoning effort
      together in one compact searchable popover. Its gpui-kit slider shows the
      active effort and can reset to the provider/model default; the existing
      rendered selector test asserts the slider remains inside that popover.
      The shared `just dev` build and disposable live UI check verified search,
      effort changes and reset. Narrow-width layout and authenticated-provider
      behavior remain open. A unit regression for discrete effort ordering,
      default positioning and slider-bound clamping is added; it still needs a
      test-mode run after the shared dev loop is stopped.
      The earlier source-current rendered GPUI test
      `chat_model_select_filters_by_provider_name` passed (1 passed, 414
      filtered out), covering effort visibility, the slider inside the
      searchable popover, and provider search/selection. The latest trigger
      layout adjustment puts effort beside the model and the muted provider on
      a second row; its row-order assertion has not been rerun. The integrated
      `just dev` rebuild failed at final linking because the filesystem ran
      out of space, so native rendering remains open.
      On 2026-10-20, reshaped the managed picker to match the compact Codex
      control: one composer trigger shows model, muted provider and effort; its
      popover keeps the active effort, searchable model/provider row and
      gpui-kit slider together. The inner combobox is an unbordered inline row
      rather than a second boxed selector, matching the single-surface feel in
      the Codex reference. Catalog refresh still happens on opening the
      combined picker, and the provider-search GPUI regression covers the
      nested flow. The latest layout follows the Codex selector's hierarchy:
      active effort, searchable model/provider row, then the GPUI-kit slider,
      all in one popover. Its GPUI regression checks that order. Scoped rustfmt
      and `git diff --check`
      pass. The focused GPUI interaction test has not been rerun after this
      final layout adjustment; native rendering remains open. The first focused
      `cargo check -p ahead-app --tests --locked --offline -j 2` exposed
      selector API mismatches; after fixing them, the test-target check passed
      with zero errors. The focused GPUI interaction test has not completed: an
      initial attempt was stopped when `../cola-v2` started building; a retry
      was stopped before AHEAD's test binary linked after dependency builds
      reduced free disk from 3.2 GiB to 2.4 GiB. A later retry began after a
      6.7-GiB idle snapshot, but a new `../cola-v2` Cargo check appeared
      immediately and the AHEAD test was stopped. Rerun only in a coordinated
      build window with sufficient headroom. Narrow-width layout, native
      rendering and an authenticated provider turn remain unverified.
      Source-only follow-up on 2026-10-20: corrected the inner gpui-kit
      `Combobox` to `.appearance(false)`, so its selected-model row is actually
      unbordered inside the combined picker. Rust formatting and diff checks
      pass; the focused GPUI interaction test and rendered native verification
      remain open.
      Follow-up on 2026-10-21: retained gpui-kit's focus-ring behavior by
      restoring `appearance(true)` while overriding the idle trigger background
      and border to transparent. Rust/Just formatting, diff checks and the UI
      detector pass. The source-current focused GPUI test
      `chat_model_select_filters_by_provider_name` passes 1/1 (421 filtered),
      checking the effort-first combined trigger, provider search, Escape close,
      and the effort slider in the same popover. Native focus-ring appearance
      and rendered layout remain open.
- [ ] Verify advisory-lock release on Linux/Windows and in rebuilt native
      process-lifecycle checks. MCP approval and ACP cache/defaults/install
      guards in `ahead-agent/src/runtime_support.rs` and `adapters.rs` now
      explicitly unlock when their operation ends, including error returns.
      `ahead-proxy/src/ahead/recovery.rs` keeps the editor's lease for the
      owner's lifetime and unlocks temporary claim leases after each claim.
      Duplicate-descriptor regressions verify exclusion while held and release
      while an inherited handle remains open; live recovery rows stay protected.
      Recheck native concurrent process creation and abrupt proxy exit. Keep
      lock inodes in place; do not treat these fixes as old-lock-file cleanup.
- [ ] Investigate the separate parent/child shutdown startup failure in
      `ahead-agent/src/native_client.rs::tests::native_shutdown_waits_for_parent_and_child_termination`.
      An earlier run timed out before its child reached the mock model. Its
      shared cancellation fixture reports request count, prompt result and
      observed events. A focused run and 21 complete agent-library runs on
      2026-10-01 passed with loopback access (124 passed, 2 opt-in tests
      ignored per run; ten runs each at 4 and 16 test threads), but no cause
      has been established for this case. A sandboxed attempt failed earlier
      at its loopback bind with `PermissionDenied`, not at shutdown.
      Do not mask it with retries or a longer timeout. The proxy attribution
      fixture's similar failure was traced on 2026-10-01 to macOS inheriting
      nonblocking mode on accepted sockets; that fixture now explicitly uses
      blocking reads. This shutdown fixture already has a blocking listener,
      so do not assume the same diagnosis applies. See the Zed tracking log
      for the captured stack, socket probe and validation results. A
      2026-10-02 full parallel run again timed out before this child reached
      its mock model, alongside six other mock-turn timeouts; the focused
      test, serial 145-test suite and repeated parallel suite passed. The
      fixture now records accepted connections and bytes received as well as
      complete requests on failure. Capture its next failing trace before
      changing the timeout or shutdown policy.
- [ ] Use `tests/fixtures/editor-smoke/` as the reusable disposable-project
      template. Copy it outside this repository, initialize Git in the copy,
      then open it in the running editor without precreating `.ahead/`.
      The template now includes the specification-shaped project skill
      `.agents/skills/ahead-smoke-skill/SKILL.md` and a unique verification
      token for the Agent Chat smoke row; discovery and activation remain
      unverified in the native app.
      On 2026-10-20, the model catalogs gained a selector fixture: catalog-a's
      `shared-model` advertises Low/Medium/High with Medium as default, while
      catalog-b's same ID intentionally has no reasoning metadata. The README
      adds reset and provider-isolation steps. `javascript/model-catalog.test.mjs`
      now runs under `npm test`. The documented fixture `just verify` passes
      offline (2026-10-20): Rust 1/1, TypeScript/JavaScript 3/3 (including
      loopback catalog assertions), and Python 1/1. This validates fixture code
      and mock payloads only, not AHEAD bootstrap, rendered selection/slider
      behavior, editor interactions or LSP behavior.
      Follow-up without Cargo on 2026-10-20: Node 24.15.0 `npm test` passed
      3/3 and Python `unittest` passed 1/1. This was limited to fixture-language
      checks; the Rust test and editor acceptance were not rerun in that pass.
      Re-ran those offline paths on 2026-10-21: `npm test` passed 3/3,
      including provider-local reasoning metadata, and Python `unittest` passed
      1/1. These validate only the disposable fixture and mocks, not AHEAD's
      native setup or model selector; the Rust fixture remains unrun because a
      separate Cargo build is active and shared disk headroom is low.
      The fixture README also calls out above-caret completion placement, the
      five-result cap, and typed completion/signature details; those native
      popup interactions remain part of the unrun UI matrix.
      On 2026-10-16, the approved disposable copy at
      `/private/tmp/ahead-broad-e2e.ikS0ar/project` now has a synthetic Git
      baseline (`234a4ec`) from fixture files only; `target/` and `.ahead/`
      state were excluded. The private Turso database and settings remain
      ignored, and only generated `.ahead/.gitignore` is untracked. The current
      isolated `just dev` loop built this source and its proxy entered the
      main loop; the watcher remains live, but this thread has no native-window
      evidence. The fixture's offline `just verify` passed Rust 1/1,
      TypeScript/JavaScript 2/2 and Python 1/1. Restarted the guarded loop on
      the same project/profile; the integrated build passed in 2.97s and the
      proxy entered its main loop (watcher session 4099).
      On 2026-10-09, the fixture's Node/TypeScript tests passed 2/2 and its
      Python unittest passed 1/1 without Cargo. These validate fixture code
      only, not the new skill's discovery or activation in AHEAD.
      Re-ran the disposable copy at
      `/private/tmp/ahead-broad-use.0XxogT/project` on 2026-10-12 with
      `just verify`: Rust 1/1, TypeScript/JavaScript 2/2, and Python 1/1 passed
      offline. This is project validation only; it does not replace editor
      setup or rendered feature checks.
      A no-dependency Node assertion also checked its frontmatter delimiters,
      required fields, matching skill/directory name, size limits and token;
      the official `skills-ref` validator is not installed.
      Verify first-time Settings/bootstrap, the private-state ignore boundary,
      and every row of its README smoke matrix in the native UI. Record actual
      results separately from unit tests; provider-backed managed turns, Pi
      ACP, MCP, FIM, voice and debugger require their respective local
      adapters/devices/configuration and remain open until exercised. On
      2026-09-30 the native picker opened a copy at
      `/private/tmp/ahead-feature-smoke-yc9OxV`, created `.ahead/.gitignore`,
      private `settings.toml` and `session.db`, then restored the disposable
      session on reopen. Git exposed only `.ahead/.gitignore`; Settings' body
      became visible after removing nested scrolling, and `pwd` in the
      rebuilt integrated terminal returned that workspace. `just verify` in
      that terminal passed its offline Rust test. The rendered composer showed
      no implicit file chip, added one for a selection or `@currentFile`, and
      cleared it when the selection collapsed. The model-facing prompt builder
      now also excludes a merely focused file path and asks for a file/range
      when a supplied selection is stale; its focused regression passes. The
      composer and shared turn boundaries now validate UTF-16 ranges before
      scope checks and target-instruction loading, dropping stale implicit
      focus while retaining explicit attachments; regression coverage was
      added but not run.
      On 2026-10-02 a fresh Git-initialized copy at
      `/private/tmp/ahead-broad-smoke-8dh22Q/project` opened without
      `.ahead/`; AHEAD created its ignore file, private settings and Turso
      session database. Git showed only `.ahead/.gitignore` as untracked.
      In the rebuilt native app, Cmd+Shift+P and F1 from the focused PTY
      opened the command palette; Down moved beyond eight results, `git`
      selected Source Control, and Escape returned focus to the PTY (`pwd`
      printed the disposable project path). A new managed thread's `/smoke`
      picker showed the project `/smoke-skill` first. The first attempt failed
      because this test had set `AHEAD_USER_HOME` to a nonexistent directory;
      creating that isolated directory and reopening the picker succeeded.
      The optional provider-settings layer now skips a truly absent user home
      while keeping other filesystem errors and dangling links fail-closed;
      its focused regression and all 25 `runtime_support::tests` pass. Rebuild
      the native app to verify that first-open skill discovery works without
      precreating this override directory. The full `ahead-agent --lib` suite
      passes offline with local loopback access (145 passed, 2 ignored) both
      serially and on a repeated parallel run. An earlier parallel run timed
      out in seven mock-model tests; one affected test passed alone and the
      same complete parallel command then passed, so test contention remains
      worth watching rather than counting as a confirmed product failure.
      Enter-to-insert was not checked because the Mac locked during that UI
      action; the focused GPUI slash-skill Enter regression passes, but is not
      native evidence. On 2026-10-12, the authenticated
      `tests/agent-openrouter-smoke.mjs` run passed against the current debug
      binary using the supplied `nvidia/nemotron-3-ultra-550b-a55b:free` model:
      it observed streaming, a completed managed turn and the random token
      returned by read-only Code Mode `file_search`. The run used a disposable
      trusted project and isolated homes; the key was stored mode-0600 only in
      the temporary settings file, stripped from the proxy environment, and
      redacted from diagnostics. The temp profile was removed. This proves one
      authenticated provider/tool turn, not native chat rendering or real-model
      cancellation/resume. Keep the remaining smoke rows unverified.
      On 2026-10-06, the documented isolated `just dev` loop built and launched
      AHEAD against `/private/tmp/ahead-just-dev.53krav/project` with separate
      data, runtime and user homes. The project copy had no `.ahead/` before
      launch; AHEAD created `.ahead/.gitignore` and an owner-only `session.db`.
      Language Servers visibly reported Blocked and the isolated trust store
      recorded the project as blocked; no LSP was launched. Quick Open opened
      the TypeScript fixture with syntax highlighting. Typing showed the dirty
      tab dot inline, and Undo back to the saved snapshot cleared it. The
      integrated PTY's `pwd` returned the disposable project path. Separately,
      `tests/agent-code-mode-smoke.mjs target/debug/ahead` passed the managed
      Code Mode tool call, delegated workspace search, cancellation, retry after
      Turso reopen and shutdown using a local fake model; the sandboxed attempt
      denied loopback, while the permitted local-only rerun passed. These checks
      do not cover the remaining smoke matrix or an authenticated provider turn.
      On 2026-10-06, the editor-smoke fixture's TypeScript and JavaScript tests
      passed under Node 24.15.0 (2 tests), and its Python unittest passed (1
      test), without invoking Cargo. These exercise fixture logic only; they are
      not editor or LSP acceptance evidence. Re-ran both commands on 2026-10-07:
      TypeScript/JavaScript 2/2 and Python 1/1 passed.
      Also copied the fixture to `/private/tmp/ahead-acceptance-FQvExa/editor-smoke`,
      initialized Git, and ran its documented `just verify` recipe on 2026-10-07:
      offline Rust 1/1, TypeScript/JavaScript 2/2, and Python 1/1 passed. The
      test-only run left `.ahead/` absent; editor bootstrap and native smoke rows
      remain unverified.
      On 2026-10-08, expanded the reusable smoke matrix to cover exclusive
      search modes and replacement, gutter/diff preview/copy and optional blame,
      slash-skill and explicit file context, root/src/docs versus unrelated
      data AGENTS/FIM hierarchy, curated ACP
      installs, extension activation and CodeAnchor-to-commit. Added the
      root/src/docs and unrelated data instruction files, plus the
      special-character path used by its terminal clipboard check. A fresh
      Git-initialized copy at
      `/private/tmp/ahead-editor-smoke.pJnTUn/project` passed `just verify`
      offline (Rust 1/1, TypeScript/JavaScript 2/2, Python 1/1). This validates
      only the language fixture; no app bootstrap or native UI was run, and the
      remaining manual smoke rows are still unverified.
      Rechecked on 2026-10-12 in the current disposable project
      `/private/tmp/ahead-broad-smoke.im0WJM/project`: offline Rust 1/1,
      TypeScript/JavaScript 2/2 under Node 24.15.0, and Python 1/1 passed.
      This confirms the sample projects themselves, not editor bootstrap or
      rendered behavior.
- [ ] Verify the chat `/` picker in the rebuilt disposable app. It now groups
      AHEAD actions, agent-provided commands and managed project/user skills,
      fuzzy-filters while the unfinished command token changes, and inserts a
      selected skill or command without sending the turn; rendered GPUI tests
      cover those cases and hide managed skills for external ACP sessions.
      The current 10-test `ahead-app --lib slash` sweep passes (2026-10-03).
      On 2026-10-07, `target/debug/deps/ahead_app-bcbfa40f10585a2b` (05:07:54,
      newer than `session_panel.rs` at 05:07:23) passed all 10 slash-filtered
      tests directly, including skill-metadata filtering/insertion and
      mouse/keyboard selection. No Cargo build was started. This remains
      headless evidence;
      native skill invocation is separate from palette selection.
      The native fixture showed the project skill, but still needs a rebuilt
      check for filtering, Up/Down/Enter/Escape, and skill execution after
      selection. A 2026-10-05 isolated `just dev` build completed in 2m24, but
      the launched debug app could not be attached through CUA. LaunchServices
      returned `kLSNoExecutableErr` for the app bundle even after ad-hoc signing;
      the native picker behavior remains unverified.
      On 2026-10-05, the existing `ahead-app-7b8796618af5560b` test harness,
      newer than `session_panel.rs`, passed all eight focused `slash_palette`
      GPUI tests, including skill metadata/filter/selection and keyboard/mouse
      navigation. No Cargo build was started. Native skill selection and
      invocation remain unverified. On 2026-10-06, the isolated app-only native
      run typed `/skill`, displayed the project `/smoke-skill`, moved selection
      with Down, and Enter inserted `/smoke-skill ` into the composer. Escape
      closed the picker and kept the draft; the draft was cleared without
      sending and context stayed at 0.0k. Native filtering, Down/Enter selection
      and Escape are verified; Up-navigation and a provider-backed skill turn
      remain unverified. A source-current rerun at 19:36 on 2026-10-07 from
      `ahead_app-601e2b645b3fbe4a` passed all eight `slash_palette_` tests and
      `composer_slash_input_tracks_the_partial_command` (newer than
      `session_panel.rs` at 19:36:32); native filtering/selection remains a
      separate acceptance gate.
      On 2026-10-13, the source-current `ahead_app-e1173e43572818d7` test
      binary passed all 54 `session_panel::tests`, including all 10 slash
      palette/input tests and the model-search/selection cases. This is
      headless GPUI evidence; native skill activation/provider execution is
      still a separate gate.
- [ ] Visually confirm `ahead-app/src/workspace_panels.rs` Source Control file
      rows at narrow and resized sidebar widths in the disposable project:
      long paths should show a leading ellipsis while Git status and the
      per-file stage control remain visible. The path is now a keyboard-
      accessible button that opens the workspace-relative file; the focused
      path-opening and 260px GPUI layout tests pass on 2026-10-04. The row
      start-ellipsizes its single line before the fixed status/stage controls.
      Direct execution of the current 2026-10-07 app test binary passed
      `workspace_panels::tests::narrow_git_row_keeps_status_and_staging_visible`;
      it does not replace a native resize check.
      The 2026-10-01 native
      screenshot feed stayed on an older Explorer frame despite Source Control
      being present
      in the accessibility tree; a later isolated app showed one long-ish
      changed path, status and stage control at 480px, but the 260px
      rendered row is not yet confirmed. On 2026-10-06, three temporary long
      nested paths rendered with ellipses in the native Source Control pane;
      statuses, stage checkboxes, Stage All and Commit Staged remained visible.
      Source Control now refreshes when
      opened and via its refresh button, and Explorer refreshes when selected.
      `ahead-app/src/workspace_panels.rs` and `ahead-app/src/explorer_panel.rs`
      now background-refresh the active panel on the proxy's debounced Git
      generation notification, keeping selected Source Control rows intact.
      On 2026-10-06, direct execution of the current app test binary passed
      `proxy_client::tests::git_metadata_notifications_invalidate_identical_status_and_wake_views`
      and `explorer_panel::tests::git_badges_refresh_after_external_edit`.
      They cover notification fanout and Explorer badge refresh separately;
      no current test drives a Git-generation notification through the active-
      panel router into an already-open Source Control panel. Verify that path
      in a fresh native window after unlock.
      Explorer now also rebuilds its tree on proxy file-set generations, while
      preserving expanded folders and selection; a focused headless regression
      covers external create/remove and skips content-only notifications.
      Verify these live open-panel updates after an external edit, create,
      remove and commit once the desktop is unlocked.
- [ ] Expand the initial GPUI command palette in
      `ahead-app/src/command_palette.rs` beyond the shipped shell commands to
      editor/buffer, Git and task actions with availability-aware dispatch.
      Open File, Open Folder, Save File and Close Editor now route to the
      existing shell/menu and close-confirmation handlers; Save, Close and
      debugger actions are hidden when their editor or debug target is absent.
      A focused palette test covers available-action filtering and keyboard
      selection; the full 214-test app library suite passes serially with
      loopback access on 2026-10-03. Render the new entries in a disposable app once the desktop is
      unlocked. Stage All Changes now reuses the panel's acknowledged proxy
      request and opens Source Control for its result; verify it in the native
      app. Discovered `just` recipes now appear as dynamic Run actions through
      the existing task-terminal path. Commit Staged Changes is available only
      when Git is connected, the index contains staged changes, a commit message
      is present and no operation is pending. Porcelain-index parsing and
      focused palette filtering/Enter and commit-availability tests pass.
      Focused palette filtering/Enter and commit-availability tests pass; verify
      both actions in the rebuilt native app.
      Cmd/Ctrl+Shift+P and F1 now open a searchable dialog backed by the same
      shell command handler as keyboard shortcuts; a focused headless test
      covers filtering and Enter. Zed registers available actions dynamically;
      AHEAD now discovers workspace task commands on palette open. LSP
      navigation, rename, formatting, completion, signature-help and code-action
      callbacks are now searchable when a text editor has language services.
      Verify
      unavailable-action states and shortcut hints in the rebuilt native app.
      Just recipe launches from both the Tasks panel and Command Palette now
      open and focus a fresh terminal titled with the recipe name, preserving
      other shells and previous task runs. Both retain the declaring justfile
      and invoke `just --justfile <filename> <recipe>` from its directory.
      The palette now exposes the existing group-aware Close Other/Left/Right,
      Close All Editors, and Move Editor to Other Group actions; the split
      action is shown only when another file is open. Source added availability
      and fuzzy-filter regressions; they have not been run.
      On 2026-10-15 the source-current command-palette tests passed (5/5).
      `app::lifecycle_tests::just_tasks_open_fresh_named_terminals` also passes
      with real PTYs: repeated launches use independent processes, focus the
      open bottom dock, keep the task title, and execute a hidden justfile
      rather than the neighboring default in a path containing spaces and an
      apostrophe. Rendered native terminal-title acceptance remains open:
      the disposable AHEAD Dev window was not accessible through macOS.
      Follow-up on 2026-10-15: the real-PTY regression now explicitly checks
      each task terminal's displayed title. The first source-current rerun
      stopped during dependency compilation with `No space left on device`
      (116 MiB available); after headroom recovered to 15 GiB, the serial
      focused rerun passed (1/1). Shared watcher 24064 was stopped to avoid
      competing Cargo jobs, then `just dev` restarted against the same
      disposable project and isolated homes. The integrated app and Code Mode
      host rebuilt in 2m45s; macOS then aborted at HIServices XPC startup before
      rendering. Watcher 88190 remains live. Native rendered title acceptance
      remains open.
      Revalidated at 23:55 EDT on 2026-10-15 using the existing source-current
      `ahead_app` runner built at 21:15 (newer than the tested app sources):
      `session_panel::tests` 59/59, `settings_panel::tests` 29/29,
      `extensions_panel::tests` 12/12, `code_panel::tests` 70/70, and the real-
      PTY Just task terminal test 1/1. This is headless GPUI/PTY evidence; no
      Cargo build or rendered native acceptance occurred in this recheck.
      Re-ran `app::lifecycle_tests::just_tasks_open_fresh_named_terminals`
      directly from that source-current runner on 2026-10-16; it passed 1/1
      without invoking Cargo. The recipe row click is wired to this fresh,
      focused terminal path; native rendered title acceptance remains open.
      At 00:28 EDT on 2026-10-16, reran the same app runner directly:
      `session_panel::tests` 59/59, `settings_panel::tests` 29/29,
      `code_panel::tests` 70/70, and `terminal_panel::` 21/21 passed, including
      chat/model/skill controls, provider-scoped model visibility, editor/LSP
      and diff interactions, clipboard round-trips, and terminal input. These
      are headless GPUI/PTY results, not rendered-window or authenticated
      provider acceptance; no Cargo command was started.
      A disposable-project native pass confirmed Cmd+Shift+P before opening a
      file, F1 after Escape, `git` filtering and Enter opening Source Control;
      the compact-height layout and F1 from a focused PTY were rechecked in the
      rendered app. A scroll-handle regression and native pass confirm that
      keyboard selection remains visible past the first eight results.
      The legacy `defaults/keymaps-*.toml` files are not loaded by the GPUI
      shell.
- [ ] Prune only proven-unused Codex product integrations from the retained
      runtime. Preserve useful model/tool handlers and loop behavior; do not
      perform a blanket clean-room rewrite. Remove a package or feature only
      after tracing its call sites and dependency closure. `ahead-agent-engine`
      has no nested workspace manifest; the repository root is the sole Cargo
      workspace, but the retained subtree still contains dozens of crate
      manifests and Codex-named packages. Root workspace integration is not the
      same as pruning/rebranding that retained dependency closure.
      Rechecked 2026-10-20: locked offline Cargo metadata reports one root
      workspace with 67 members and 9 default members; all 52 packages under
      `ahead-agent-engine` occur in `ahead-agent`'s transitive dependency tree.
      `codex-core` receives only its empty default feature, and
      `cargo-machete` reports no unused direct dependencies. No package is
      removable on orphan/manifest evidence alone; continue tracing dead
      product-specific code paths inside required crates. A source trace also
      confirms `codex-otel` is live runtime plumbing: session, tool, MCP and
      exec paths use its trace-context and metrics interfaces. AHEAD's native
      client explicitly sets log (`exporter`), trace (`trace_exporter`) and
      metric (`metrics_exporter`) exporters to `None` and `log_user_prompt =
      false` before initializing the retained runtime, while ignoring
      runtime-home `config.toml`; the retained config defaults also disable all
      three exporters and prompt telemetry, and workspace `Cargo.toml` disables
      `codex-otel/otlp`. Do not remove the crate wholesale without replacing
      those internal interfaces and preserving the exporter-off invariant.
      On 2026-10-22, the telemetry call-graph audit confirmed
      `codex_core::otel_init::{build_provider, record_process_start}` and
      `codex_otel::record_process_start_once` (`PROCESS_START_METRIC`) had zero
      callers across the workspace after CLI/App Server removal; removed
      `ahead-agent-engine/core/src/otel_init.rs`,
      `ahead-agent-engine/otel/src/metrics/process.rs`, and the unused
      `GOAL_*_METRIC`/`HOOK_RUN_*_METRIC` constants, and expanded the
      featureless `OtelProvider::try_new` unit test in
      `ahead-agent-engine/otel/src/provider.rs` to assert all-`None` settings
      return `Ok(None)` and any non-`None` general, trace, or metrics exporter
      returns the disabled-exporter error. `rtk cargo test --locked --offline
      -p codex-otel --lib` passed 10/10. Also removed the zero-caller
      `detached_memory_responses_metadata` helper (`CodexResponsesRequestKind::Memory`),
      `windows_sandbox_read_grants` (`grant_read_root_non_elevated` and
      `run_setup_refresh_with_extra_read_roots`), the unused CLI Windows
      sandbox onboarding setup/config-writer path in
      `ahead-agent-engine/core/src/windows_sandbox.rs`, the zero-caller
      `check_execpolicy_for_warnings` helper, the unused `ahead-agent-engine/core/src/config/edit.rs`
      TOML mutation module (plus `codex-core`'s direct `toml_edit` dependency
      and `codex-utils-path`'s `resolve_symlink_write_paths`/`write_atomically`
      and normal `codex-utils-absolute-path`/`tempfile` dependencies), the
      unread `[desktop]`, `[goals]`, `[ghost_snapshot]` and `oss_provider`
      sections (`validate_oss_provider`, `resolve_oss_provider`), and the empty
      `ExtraConfig` thread-store placeholder (and fixed a stale `agent_role`
      test reference in `codex-thread-store`). `rtk cargo test --locked
      --offline -p codex-utils-path -p ahead-rpc -p codex-thread-store --lib`
      passed 50/50 across all three suites, and `rtk cargo test --locked
      --offline -p codex-config --lib -- strict_config` passed 9/9. Workspace
      `cargo fmt --all -- --check`, `git diff --check`, and `cargo machete`
      pass; defer broader Cargo compilation while `cola-v2`'s workspace
      `cargo sqlx prepare` check is active.
      The 2026-10-20 Codex plugin check found no marketplace integration to
      prune: `codex-core` does not declare a `plugins` module, its plugin
      source directory has no files, and there are no `plugins::` call sites
      in `codex-core`. Keep AHEAD's separately named `ahead-proxy/src/plugin`
      path, which owns live LSP/DAP editor integrations.
      Login-path trace on 2026-10-20: the integrated engine has no
      `codex-login` package or ChatGPT account/OAuth sign-in flow. Managed
      requests use `ahead-model-auth` to adapt AHEAD's selected provider
      credentials into the retained API's generic `AuthProvider` interface;
      command-backed bearer refresh remains a provider-configured mechanism.
      ACP sign-in requests are separate and belong to installed external agents,
      so preserve that path for Pi/Codex/Claude adapters. The surviving
      `codex_home` path name is still used for agent instructions, policy and
      runtime state; it is not evidence of a user login dependency.
      A 2026-10-04 `cargo machete` audit found one genuinely unused direct
      dependency (`base64` in `codex-rmcp-client`), which was removed from its
      manifest and lock entry. A follow-up audit reports only intentional
      dependencies that the scanner misses: `ahead-voice`'s `cc` and
      `ahead-core`'s `anyhow`/non-Linux `git2` are used by build scripts;
      `codex-core`'s `openssl-sys` enables vendored OpenSSL for musl targets;
      `codex-protocol`'s `icu_provider` unifies ICU features and `strum` is
      used by generated derives. No other unused direct dependency was
      reported. Locked offline dependency resolution passes for
      `codex-rmcp-client`. A separate 2026-10-04 Cargo graph audit found all
      60 packages under `ahead-agent-engine` in `ahead-agent`'s
      transitive dependency tree; no retained runtime crate is an orphan that
      can be deleted wholesale without first changing the integrated loop.
      Prune dead feature branches within required crates instead of flattening
      or renaming the directory tree. This does not materially shrink the
      transitive graph.
      AHEAD's persisted spawned-agent graph contract has now been moved out of
      `source/` and renamed `ahead-agent-graph-store`; it remains a small
      separate package to keep the dependency direction acyclic between the
      host and retained core. The ignored, deprecated Codex JS-REPL path/module
      settings were removed from runtime configuration rather than preserved
      as inert compatibility fields. The unused Chronicle sidecar, external
      agent-memory import and MCP Apps feature toggles are also gone; their
      feature flags had no implementation in the retained integrated runtime.
      The orphaned App Server media-notification and Bedrock TUI-setup flags
      were removed for the same reason. The retained thread manager no longer
      rewrites outbound request originators from Codex Work/ChatGPT service
      names; AHEAD uses its own originator while keeping persisted thread
      attribution and telemetry service labels separate. A server-reported
      model mismatch now reports the requested and reported model instead of
      falsely claiming the user's account was flagged for cyber activity.
      Removed the three old-transcript-only warning filters as well; obsolete
      Codex warning text is no longer specially scrubbed from loaded history.
      Usage-limit errors retain structured rate-limit/reset information but no
      longer render Codex plan upsells or the `x-codex-promo-message` header.
      Context-window and filesystem-policy warnings now use AHEAD's product
      language while retaining the original structured error and restriction.
      On 2026-10-20, made `codex-otel`'s package-level default feature set empty
      as well as disabling it in the workspace dependency. A workspace-wide
      build can therefore select the crate itself without implicitly enabling
      OTLP exporter dependencies; exporters remain an explicit opt-in feature.
      Updated the crate README. No Cargo build was run alongside the shared
      `just dev` loop. The shared watcher now includes every first-party root
      workspace crate; it previously omitted the agent graph store, skills,
      model-auth and runtime API packages, so edits there could leave the
      running app stale. The Codex-core realtime voice conversation transport
      is now opt-in (`codex-realtime-voice`) and excluded from AHEAD's managed
      runtime; AHEAD voice uses its dedicated `ahead-voice` crate. The voice
      transport, world-state section and voice-only context fragments are
      excluded from the default build; protocol start/audio/text/speech/list
      events return a clear unavailable error, while close remains a no-op.
      The API boundary now gates the realtime call/websocket modules and
      protocol exports behind the same opt-in feature; its voice-only
      `async-channel` edge and realtime integration test are also optional.
      The Responses websocket transport remains enabled. Formatting and
      call-site tracing pass. The shared `just dev` loop rebuilt the integrated
      app and launched it at 10:33 EDT on 2026-10-20 with the feature-off
      configuration, confirming the default managed build compiles. No
      voice-enabled build or test suite was run. Keep this pruning item open
      for further call-site/dependency review; do not remove voice code needed
      by the separate AHEAD voice path. A follow-up default-build cleanup now
      compiles retained realtime-only hooks only with that opt-in feature and
      keeps editor completion/snippet test helpers out of release code. The
      shared `just dev` rebuild completed without Codex-core or AHEAD-owned
      unused-import/dead-code warnings; the optional voice-enabled build remains
      unchecked.
      Until done, preserve per-file copyright notices and ship license texts
      with builds.
- [ ] Port Zed's mature multi-file editor behavior: buffers, tabs, splits,
      selections, multi-cursor, undo/redo, IME, external-change handling and
      document-model synchronization. Validate real unsaved files before
      replacing the current single-file `CodePanel`. Code tabs can now be moved
      between side-by-side editor groups while keeping one `CodePanel` per open
      buffer; dock reparenting now updates side membership, but duplicate views
      of one buffer and saving editor-group layouts remain unfinished. A
      file opened through a different canonical path spelling now reuses its
      existing `CodePanel`; LSP workspace edits and search replacement share the
      same file-identity check. A GPUI regression covers a `src/../src` alias
      and has not been run in this source-only pass.
      disposable native-window
      pass exposed a false modified marker after Undo restored saved content;
      `CodePanel` now compares the current rope with its saved snapshot, and
      the focused GPUI Undo regression passes. The rebuilt isolated app showed
      the marker appear on typing and clear on Undo in the fake `main.rs`.
      The modified-tab dot now stays inline beside a truncating filename; the
      520px GPUI regression asserts same-row placement and no overlap. The
      current 2026-10-07 app test binary passed
      `app::lifecycle_tests::dirty_file_tab_keeps_its_marker_inline_at_narrow_width`;
      verify filename truncation and close-control visibility in the rendered
      tab bar.
      2026-10-06: workspace notifications now carry changed paths through the
      proxy. A clean open buffer reloads external edits off the UI thread and
      rebases the selection through unchanged text; a dirty buffer is retained
      and saving asks before overwriting the disk version. Two focused GPUI
      GPUI regressions cover clean reload/caret rebasing, dirty-buffer
      retention, ignored-file notifications and deletion Cancel; proxy tests
      cover changed paths and subscriber isolation. In the rebuilt
      isolated native app, an external clean-buffer edit rendered immediately;
      a later disk edit left the dirty buffer untouched, Cmd+S showed the
      changed-file warning, and Cancel preserved both local text and the newer
      disk version. In a clean Python buffer, inserting a line before the caret
      on disk reloaded the text and moved the caret with its unchanged suffix.
      The separate proxy open-buffer event now reaches only matching editor
      panels, so a file excluded by workspace search can still reload without
      broadcasting unrelated Settings/model refreshes. Focused routing and
      GPUI tests confirm subscriber isolation and reading the latest disk text
      rather than stale event payload content. External NotFound is now
      distinguished from other read errors: the editor keeps local text, warns
      that Save recreates the deleted/renamed old path, and Cancel leaves it
      absent. A focused GPUI regression covers dirty-buffer deletion and Cancel.
      Zed updates the buffer's file handle when the worktree entry changes and
      reloads only a clean buffer whose new disk state is Present. AHEAD now
      carries ordered rename pairs through the proxy and adopts a renamed file
      (including open descendants of a renamed directory) after checking the
      destination remains inside the workspace; dirty text is retained and the
      LSP/recovery path is updated. A focused GPUI regression covers a dirty
      file rename. Run that regression and exercise rapid concurrent writes.
      Workspace trust stayed blocked
      and no provider was used. The full serial `ahead-app --lib` suite passed
      239 tests with local loopback; its restricted run's only two failures
      were mock-server bind denials.
      Profile same-length edits on large files.
- [ ] Verify the save/discard/cancel close flows in rendered GPUI:
      Close, Close Others, Close Left/Right and Close All must retain a buffer
      after Cancel, a failed save or an edit made while a decision/save is
      pending. Successful saves must finish before removal. Full-shell GPUI
      tests now cover window close, the explicit Quit action, cross-window
      edits during prompts, delayed/failed/timed-out saves and buffer release.
      Explorer and tab menus share a confirmed, workspace-bounded Move to
      Trash RPC; cancellation, failures, timeouts and newer edits retain open
      buffers. Verify the native menu/shortcuts, traffic-light close and
      Linux title-bar close after unlocking the desktop, plus actual system
      Trash/recovery on disposable files and directories. The tests mock the
      Trash operation and do not prove native integration. Verify preview
      replacement and confirmed close send `didClose`, remove the proxy's
      unsaved snapshot and ignore old
      diagnostic pulls, then reopen with current content. The rebuilt real
      Vtsls/BasedPyright proxy smoke passed close/reopen/error/repair on
      2026-09-30; the native shell/prompt journey still needs verification.
- [ ] Verify dirty-buffer recovery in the native app across OS termination
      and dev-watcher restarts. `ahead-app/src/{app,code_panel}.rs` now uses
      the initial Turso `editor_recoveries` table, with coalesced snapshots,
      revision-fenced saved/discarded rows, and OS owner leases. Restore leaves
      disk files and current dirty tabs unchanged. It warns before overwriting
      a changed saved baseline and supports File > Recover Unsaved Changes. Run
      `tests/editor-recovery-smoke.mjs` after building, then the rendered
      startup/overwrite/duplicate-path recovery journey in a disposable
      project after desktop unlock. The 2026-10-02 rebuilt-binary smoke passed
      crash persistence, stale-snapshot fencing and owner-only database
      permissions in `ahead-recovery-smoke-Hcf5aY`. It re-passed on 2026-10-03
      against the fresh 01:38 debug binary; the disposable fixture was removed.
      The post-clean debug binary passed it again on 2026-10-03.
      On 2026-10-05, reran it against the current `target/debug/ahead` binary:
      Unicode recovery after SIGKILL, live-owner fencing, explicit discard and
      stale-revision fencing across restart, and owner-only database
      permissions on create/reopen all passed. The script retained its
      disposable project for inspection; this was proxy, not GPUI, evidence.
      Re-ran on 2026-10-06 against the current shared-dev binary; the same
      assertions passed in `ahead-recovery-smoke-LAJVHw`, including SIGKILL,
      live-owner exclusion, persistent discard and 0600 create/reopen. The
      fixture remains under the system temporary directory for inspection.
      Repeated on 2026-10-07 against the 19:37 current-source `just dev`
      binary in `ahead-recovery-smoke-WHooJ6`; acknowledged Unicode recovery,
      live-owner fencing, explicit discard across restart and owner-only
      database permissions all passed. This remains proxy/storage evidence,
      not the native startup recovery UI journey.
      Re-ran against the 21:21 current `just dev` binary; the same SIGKILL,
      Unicode snapshot, owner-fencing, persistent-discard and 0600 database
      checks passed. The fixture remains at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-recovery-smoke-0n7XhB`.
      Native startup/restore and duplicate-path UI acceptance remain open.
      On 2026-10-12, the freshly rebuilt 11:39 `target/debug/ahead` passed
      `tests/editor-recovery-smoke.mjs` in disposable
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-recovery-smoke-rJW9su`:
      Unicode recovery after SIGKILL, live-owner fencing, persistent discard,
      stale-write fencing and owner-only permissions on create/reopen all
      passed. This exercises the working initial schema's recovery behavior,
      not schema-version compatibility or the native recovery UI.
      The last unacknowledged edits can be lost:
      periodic capture is 250 ms and GPUI's shutdown flush has a 200 ms budget.
      Test disk-full/disconnect behavior in the native UI, profile serialized
      DB writes with large/many buffers and define safe lease-file cleanup.
      Verify recovery path normalization and owner locking on Windows/Linux;
      current native process evidence is macOS-only.
      The 32 MiB per-buffer ceiling, untitled buffers, layout/caret/scroll
      restoration, plus external deletion/rename/race handling, remain open.
      Define and verify exit behavior for running agent turns and
      terminal/debugger jobs. Old databases stay intact; no migration.
- [ ] Verify nonblocking attribution refresh in the native editor's gutter.
      `ProxyClient::refresh_anchors` no longer blocks file open or save on its
      RPC reply; the client regression covers delayed/out-of-order results,
      close invalidation, view wakeups and visible failure. Exercise the
      apply-to-anchor-to-gutter-to-commit journey. Git gutter/blame now use a
      debounced background proxy request against the live buffer and HEAD,
      including staged changes; render/caret movement read a cache, stale or
      closed-buffer replies are discarded, and repository notifications
      invalidate metadata even when the branch/file-status summary is equal.
      Gutter row creation is now bounded to the editor's visible source-line
      range with one-line overscan, and Git refresh is scheduled from edit,
      file-open, and repository-generation events rather than as a render side
      effect. The typing-idle debounce is 600 ms so normal keystroke pauses do
      not repeatedly trigger full-file Git metadata refreshes; the previous
      successful markers stay rendered until the replacement state arrives.
      Diff/deletion marker row maps are now prepared when Git state changes and
      updated on buffer line-count changes; rendering only looks up visible
      rows. Same-line keystrokes no longer compare or rebuild hunk geometry in
      the render path, and diff payload changes that do not affect the gutter
      are ignored. A source regression covers stable geometry, baseline-payload
      changes, and line-count clipping; it has not been run. Live typing and
      caret flicker still need the user's rendered check.
      On 2026-10-09, the source-matched app test runner
      `ahead_app-5b00065cae9bc61c` (12:20:14; newer than `code_panel.rs` at
      11:37:41) passed the visible-row bound test and both Git-metadata tests
      for typing debounce, request coalescing, last-marker retention, and stale
      or closed-buffer reply rejection. This verifies state behavior in the
      GPUI harness; native typing/render flicker still needs a live-window
      check.
      Rechecked on 2026-10-09 from the 16:18 source-current app runner: all
      three `git_metadata` tests passed, including typing debounce, coalescing,
      and retaining the last successful markers while a refresh is pending;
      both `git_change_marker` tests also passed, covering diff preview and
      breakpoint isolation. Native flicker/rendering remains open.
      Folded-row mapping remains unverified: gpui-kit exposes a contiguous
      visible source-line range, while Zed maps gutter markers through display
      rows. Preserve folded alignment before treating this optimization as
      complete.
      The active-line author/time annotation defaults on and can be disabled
      in Editor settings (workspace-private `.ahead/settings.toml`). A live
      disposable GPUI pass on `blame-demo.rs` confirmed the Git icon, author,
      and relative time on the active line; disabling the setting hid it and
      restoring it brought it back. The config test covers the on-by-default
      behavior and persisted opt-out. Proxy regressions cover invalidation and
      blame refresh after an external HEAD amend, plus a committed nested file
      opened from both the repository root and a subdirectory in a linked
      worktree. A branch-checkout regression verifies the new branch summary
      and updated blame. Still verify this refresh in the native editor.
      Git change markers now appear as a continuous colored hunk rail; clicking
      a marker opens an in-editor old/new preview with Copy. The focused GPUI
      regression verifies modified and deleted hunks plus copied diff text.
      On 2026-10-07, direct execution of the current app test binary passed
      `git_change_marker_does_not_toggle_breakpoint`,
      `git_change_marker_previews_modified_and_deleted_hunks`,
      `git_metadata_waits_for_typing_to_settle`, and
      `inline_blame_tracks_line_end_and_relative_author_time`; rendered
      alignment remains unverified. On 2026-10-07, the same source-current
      app binary (05:07:54; newer than `code_panel.rs` at 00:51:02) also passed
      `git_hunk_preview_text_matches_the_visible_diff_rows`,
      `source_control_open_expands_first_git_hunk_after_refresh`,
      `formats_cached_git_blame_metadata` and
      `agent_anchor_gutter_rows_include_the_full_range_and_ignore_other_actors`.
      These remain headless checks; inline diff layout and gutter tooltip still
      need a fresh native render.
      Zed renders hunk controls through its editor diff rows, while AHEAD uses
      an anchored overlay; compare the native interaction and presentation
      before deciding whether deeper display-row integration is warranted.
      Profile large files and many open tabs: whole-file blame currently runs
      per coalesced revision; closing a view discards its result but does not
      interrupt an already-running libgit2 calculation. Add a shared HEAD
      baseline cache or cancellable worker if measurements require it.
- [ ] Make the editor's LSP path production-ready through the existing
      proxy/RPC boundary. In `tests/fixtures/editor-smoke/` with a real Rust
      language server, verify automatic and manual completion, filtering and
      acceptance at the correct unsaved-buffer range (separate from FIM);
      diagnostics appearing and clearing after a fix; hover, signature help,
      go-to-definition/back, references, cross-file rename, formatting and
      code actions. Implement missing routes, reject stale replies after edits
      or file switches, and verify server restart/recovery in the native app.
      `AheadDelta::contiguous_replacement` now converts selection replacements
      and multi-part edits into one precise changed range for incremental LSP
      sync; malformed deltas still fall back to a full snapshot. Buffer and
      language-server regressions cover a Unicode selection replacement and
      UTF-16 positions. On 2026-10-20, `rtk cargo test -p ahead-rpc --locked
      --offline` passed 28 tests with one ignored, covering the changed delta
      and RPC notification DTOs. This does not compile the managed-agent or
      editor event-to-render paths; those remain unverified in this source
      state.
      On 2026-10-20, the editor now measures caret bounds whenever completion,
      signature help, code actions, FIM ghost text or snippet choices are
      visible, positions them against the measured caret, and clamps each
      overlay's horizontal position and width to the editor viewport, following
      Zed's cursor-popover layout. Geometry tests cover the preferred widths;
      the signature-help GPUI test now checks the measured anchor even when
      completion is not open. Formatting and the design scan pass, but this
      source change has not been built or tested due to 1.5 GiB of available
      disk space. Native rendering remains unverified.
      On 2026-10-20, AHEAD now requests details for every available fuzzy-ranked
      completion row (up to five) as the menu opens, rather than resolving only
      the selected row, following Zed's visible-completion prefetch. The proxy
      already returns the original item when a server omits optional
      `completionProvider.resolveProvider`. The GPUI regression checks that a
      second visible item is prefetched and still fences stale replies. Scoped
      rustfmt and diff checks pass; the focused test has not run while a
      separate workspace's Cargo clippy is active.
      LSP definition and result-selection jumps now share per-window Go Back /
      Go Forward history, with title-bar controls, menu items, shortcuts and
      context-aware Command Palette entries; native interaction remains to be
      verified. On 2026-10-19, the shared `just dev` build compiled the
      integrated app after importing gpui-kit's `Disableable` trait for the
      navigation controls; no rendered navigation check was performed.
      On 2026-10-15, fixed optional rename preparation following Zed's
      `PrepareRename::response_without_request`: servers that advertise rename
      without preparation use the existing editor word-range prompt; an
      explicit `renameProvider: false` is rejected. A real child-process
      regression covers absent/false/true/options/prepare-false/prepare-true
      capabilities and actual rename forwarding. The full current proxy
      suite passes 259 tests with one opt-in test ignored. The expanded
      `tests/lsp-smoke.mjs` passed with cached Vtsls and BasedPyright against
      the rebuilt integrated binary: TypeScript, JavaScript and Python
      declaration renames update multiple unsaved buffers, retain definitions
      and typed diagnostics, and leave disk contents unchanged. TypeScript
      formatting is applied and checked for idempotence; TypeScript/JavaScript
      typed signatures, close/reopen, unsaved replay on restart and process
      cleanup also pass. Fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-adsDTy`.
      These are proxy/server checks, not native F2/undo/formatting acceptance.
      Python now starts Ruff alongside BasedPyright through the existing LSP
      lifecycle, following Zed's Python defaults. Multiple installed adapters
      for one language resolve independently, and stopping one server preserves
      the others. The expanded proxy suite passed 266 tests (one opt-in test
      ignored) with local networking/file-watcher access. A live test acquired
      Ruff 0.17.1 from its official release, verified the published checksum and
      observed combined analyzer/linter diagnostics. After the clean rebuild,
      the integrated Python smoke and combined TypeScript/JavaScript/Python
      smoke passed: Ruff formatting and idempotence (including a valid null
      result), combined diagnostics, typed analyzer hover, cross-file rename
      without saving buffers, close/reopen, fresh processes after restart and
      graceful shutdown. The combined fixture is
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-dmACY3`.
      Code actions now aggregate complementary servers through the existing
      proxy/RPC/client path, preserving each action's owner for resolve and
      command dispatch. Null/empty results and partial server failures do not
      hide healthy actions. Code-action requests expire before the editor's
      wait ends, without shortening other requests. The current proxy suite
      passes 270 tests (one opt-in ignored), the app suite passes all 341 tests,
      and the mounted owner/resolve/command/undo regression passes 20 seeds.
      The rebuilt all-language proxy smoke also passes with cached Rust
      Analyzer, Vtsls, BasedPyright and Ruff, including Python fixes from both
      servers before and after restart. Ruff supplies an inline edit in this
      probe; lazy resolve/command ownership is covered by the mounted test.
      Fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-NbF2BB`.
      Server ordering and per-method selection now use the first capable
      enabled server rather than racing overlapping analyzers. TOML language
      settings support explicit lists, exclusions and remaining-server expansion;
      live changes restart servers with unsaved replay. The real all-language
      smoke verifies hover/formatting routes, disabling/re-enabling Ruff and
      rejection of a disabled method target. Native code-action selection/undo
      still needs verification; configuration currently uses TOML, not a UI picker.
      Watched-file registrations now use the existing filtered file watcher,
      with per-server registration ownership and cleanup. The real Python
      smoke verifies `pyproject.toml`/`ruff.toml` creation, edits, rename and
      deletion without restarting Ruff, before and after an explicit restart.
      Canonical matching preserves the server's workspace URI spelling on
      macOS. Workspace settings refresh also handles both native and aliased
      event paths, including deleted settings files. The current proxy suite
      passes 285 tests (zero failures, one opt-in ignored); the rebuilt
      Rust/TypeScript/JavaScript/Python smoke passes in disposable fixture
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-HStpx4`.
      At 21:28 EDT on 2026-10-15, the Python-only matrix also passed against
      the integrated binary built at 21:18, using cached BasedPyright and Ruff.
      It covered completion/navigation, diagnostics and repair, both servers'
      code actions, formatting and typed hover, live config-file create/edit/
      rename/delete without restart, unsaved cross-file rename, close/reopen,
      server routing, restart/replay and graceful shutdown. A restricted run
      missed the newly created config event; the same run passed when native
      filesystem notifications were permitted, consistent with sandbox event
      suppression. No network or model calls were made. Retained fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-vYzueO`.
      Repeated at 23:17 EDT against the source-current `target/debug/ahead`
      with cached servers, isolated homes and Python-only coverage; config
      create/edit/rename/delete, restart/replay, routing and shutdown all
      passed outside the sandbox. The matching sandboxed run failed only at
      config creation, confirming this environment suppresses the OS watcher
      callbacks. Fixture: `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-4nJrz6`.
      At 23:20 EDT, the same source-current binary also passed the TypeScript/
      JavaScript smoke (typed completion, auto-import resolution, quick fixes,
      formatting, navigation, unsaved cross-file rename, restart and shutdown)
      and the Rust smoke (completion, definition, hover, signature help,
      references, rename, formatting, diagnostics/repair, restart and shutdown).
      Rust Analyzer 1.98.1 was invoked by its explicit installed toolchain path;
      no global toolchain change or package download occurred. Fixtures:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-QwBBA3` and
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-oXtImq`.
      These are real proxy/server checks; rendered editor acceptance remains
      open.
      A fresh `just dev` restart at 23:23 EDT rebuilt the integrated binary in
      1.24s, then its app child again aborted in HIServices before drawing
      (signal 6). The watcher session exited; rendered acceptance remains
      unavailable on this host.
      At 23:30 EDT, the user-authorized OpenRouter smoke used only synthetic
      data and a temporary mode-0600 profile. Its sandboxed run could not
      create AHEAD's inner filesystem sandbox; the unsandboxed retry remained
      streaming through the 180s deadline without an edit. A direct 16-token
      probe returned an HTTP 200 envelope containing Nvidia's 503
      `provider_overloaded` error. No credential or profile was retained.
      Authenticated managed-provider acceptance is still unverified; retry the
      exact configured model after the upstream service recovers.
      Keep symbol outline explicitly deferred. `CodePanel` now sends unsaved
      snapshots, requests completion after typing, uses UTF-16 positions and
      applies LSP `textEdit` ranges. A live disposable-project pass with the
      cached Rust Analyzer showed `math::dou` producing a `double` popup and
      Return inserting exactly `math::double(21)` into the unsaved editor;
      the focused GPUI Enter regression passes. The earlier server crash came
      from unwrapping absent JSON-RPC parameters on Rust Analyzer's
      `workspace/diagnostic/refresh` request. The parser now accepts
      parameterless requests and notifications (regression passes), and the
      proxy now answers that refresh request and pulls document diagnostics
      for open buffers, also pulling after unsaved edits. In the disposable
      native app, an unsaved `let = ;` produced three Rust Analyzer Problems
      entries and Undo cleared them. Diagnostic notifications wake the
      code and Problems views. On 2026-09-30, an already-open Problems view
      displayed a saved compiler error and cleared it after the correction,
      without switching tabs after either reply. Debounce the current per-edit pulls
      if their traffic becomes costly. Cmd+S now sends its captured snapshot
      through the proxy's buffer save path and emits LSP `didSave`; save
      acknowledgements preserve newer unsaved typing and ignore older replies.
      The shared save path uses a unique temporary file instead of a fixed
      `.bak`, preserving existing backups, symlinks and executable permissions.
      Focused proxy and GPUI save tests pass. A native pass in
      `/private/tmp/ahead-save-smoke-caPdPX` saved `math::missing_symbol(21)`,
      showed E0425, then restored `math::double(21)` and cleared Problems.
      Protocol tracing confirmed `didSave` and the compiler replies. That pass
      exposed two repaired bugs: an empty pull reply replaced pushed compiler
      errors, and the static capability check ignored dynamic save registration.
      Diagnostics now merge by server and pushed/pulled source; regressions
      cover independent clearing and dynamic save support. Dynamic `didSave`
      registrations now merge by registration ID, including overlapping
      `includeText` requests; absent/null selectors use the host's configured
      document selector and `unregisterCapability` removes only its matching
      ID/method. A focused regression covers overlaps, null options and
      targeted removal; run it in the shared Cargo loop and verify against a
      real server. Stopped-server diagnostics now clear
      by process instance; other servers' diagnostics remain, and late updates
      from retired processes are ignored. Verify this in the native Problems
      view after desktop unlock.
      An offline Rust Analyzer protocol probe on 2026-10-07 found that the
      initial `textDocument/diagnostic` pull can be cancelled during
      `workspace/diagnostic/refresh`; retrying it returns the expected
      unsaved-buffer diagnostic. `refresh_diagnostics` now retries
      `SERVER_CANCELLED` up to three times while the document generation is
      unchanged. Current-source regressions now drive one cancelled request to
      a successful full report and verify publication, and ensure a changed
      document generation prevents stale retry/publication. The focused retry
      run passed (2 tests), and the complete `document_diagnostic_tests` module
      passed 15 tests with its opt-in real-server test ignored. These use a
      fake RPC channel and do not require a provider or live language server.
      On 2026-10-07, the full `tests/lsp-smoke.mjs` matrix passed against the
      `target/debug/ahead` binary rebuilt by `just dev` at 21:21. It used
      already-installed Zed Vtsls, BasedPyright and Rust Analyzer packages via
      explicit local paths; no downloads, installs or model providers were
      used. Rust passed completion, definition, hover, signature help,
      references, cross-file rename, diagnostics/repair and formatting.
      TypeScript, JavaScript and Python passed completion, definitions,
      diagnostics/repair, TypeScript quick-fix/auto-import and shared TS/JS
      server identity. The run also passed close/reopen, restart with unsaved
      buffer replay and graceful process cleanup; its disposable project is at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-e6GUOA`.
      On 2026-10-09, an isolated rerun opted into AHEAD-managed server
      acquisition for Rust and installed the live Zed HTML extension from its
      catalog. HTML 0.3.2 completed successfully. Rust passed formatting,
      completion, definition, semantic tokens, hover, signature help,
      references, cross-file rename, diagnostics/repair, close/reopen, unsaved
      buffer replay after server restart and graceful shutdown. The rustup
      shim was not installed in the user's stable toolchain; AHEAD acquired
      what the disposable run needed under its isolated data home. No user
      package cache or credentials were used. Fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-fef6Qn`.
      This validates the Rust server and live Zed extension install paths, not
      TypeScript/JavaScript or Python on this machine.
      A second 2026-10-09 run enabled AHEAD-managed acquisition for the full
      Rust/TypeScript/JavaScript/Python matrix and installed HTML 0.3.2 from the
      live Zed catalog. All four language paths passed completion, navigation,
      diagnostics/repair and semantic-token checks; Rust also passed formatting,
      hover, signature help, references and cross-file rename. TypeScript passed
      quick-fix edits/command execution and resolved auto-import insertion;
      JavaScript shared Vtsls with TypeScript. Close/reopen, restart with
      unsaved-buffer recovery, and graceful shutdown passed. Servers and
      extension were stored only in the isolated fixture at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-5NAB8d`;
      no global package or user cache was changed. This is live proxy/server
      evidence, not native editor popup, trust UI, or Problems-view evidence.
      The same 2026-10-09 root binary passed `tests/lsp-shutdown-smoke.mjs` in
      all four cases: explicit shutdown, stdin EOF, EOF while initializing, and
      an unresponsive server. Each completed the proxy/server cleanup contract
      without saving the unsaved editor buffer. Disposable fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-shutdown-52z0rj`.
      This exercises proxy/server behavior only. Verify cancellation recovery
      against a live server and the native completion insertion/popup,
      navigation and Problems views before closing the editor gate.
      Later on 2026-10-09, reran the full managed-acquisition matrix against
      `target/debug/ahead` (15:00:37) with
      `AHEAD_LSP_INSTALL_MISSING=1`. TypeScript, JavaScript, Python and Rust
      passed completion/navigation/diagnostic repair and semantic tokens;
      TypeScript quick-fix/auto-import, Rust formatting/hover/signature
      help/references/cross-file rename, close/reopen, unsaved-buffer recovery
      after restart, and graceful proxy/server cleanup also passed. All server
      packages and homes were isolated in
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-jDE6ah`;
      no user project data or model credentials were sent. This is fresh
      managed real-server evidence, not native popup/navigation/Problems-view
      verification.
      Revalidated at 23:12 ET on 2026-10-07 against the current binary rebuilt
      by `just dev` at 23:09, using only Zed-cached Vtsls, BasedPyright and
      versioned Rust Analyzer executables. TypeScript, JavaScript, Python and
      Rust all passed their real-server completion/navigation/diagnostic flows;
      the run also passed restart with unsaved-buffer recovery and graceful
      shutdown. The separate current-binary `tests/lsp-shutdown-smoke.mjs`
      passed explicit shutdown, stdin EOF, initialization disconnect and
      unresponsive-server cleanup, with source files unchanged. No download,
      package installation or model-provider call occurred. These are
      disposable headless proxy/server checks; native rendered editor behavior
      remains open.
      Revalidated at 23:32 ET on 2026-10-07 against `target/debug/ahead`
      rebuilt by the shared `just dev` loop at 23:30:25, newer than the LSP
      proxy, catalog and editor sources. The smoke passed TypeScript,
      JavaScript, Python and Rust completion/navigation/diagnostic flows,
      TypeScript quick-fix and auto-import resolve, Rust formatting and
      cross-file rename, restart with unsaved-buffer replay, and graceful
      shutdown. It used existing AHEAD-cached npm servers and the locally
      cached Rust Analyzer, with no downloads or provider calls. Disposable
      fixture: `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-dbvif7`.
      These are current-binary proxy/server checks; native completion-popup,
      insertion and Problems-view acceptance remain open.
      On 2026-10-08, CodePanel began consuming the existing LSP semantic-token
      route through a debounced GPUI decoration collection with generation,
      path and UTF-8 length fences. Three focused unit/route tests pass. The
      rebuilt `target/debug/ahead` also passed semantic-token requests from
      Zed-cached Vtsls for TypeScript/JavaScript, BasedPyright and Rust Analyzer
      in the full disposable `tests/lsp-smoke.mjs` matrix; no servers were
      installed and no provider was contacted. The fixture is
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-5WDZCr`.
      This validates real-server protocol and byte ranges, not rendered token
      colors. Native editor behavior remains open while macOS aborts startup
      before rendering.
      On 2026-10-08, the full smoke passed again against the existing
      `target/debug/ahead` binary, with `AHEAD_LSP_INSTALL_MISSING=1` and a
      sanitized environment. AHEAD installed Vtsls and BasedPyright into the
      disposable data-home cache; AHEAD also acquired pinned Rust Analyzer
      2026-10-05 into its managed cache. TypeScript and JavaScript passed
      completion, definition,
      diagnostics/repair, semantic tokens and import quick-fix/resolve;
      Python passed completion, definition, diagnostics/repair, semantic
      tokens and close/reopen; Rust passed formatting, completion, definition,
      hover, signature help, references, cross-file rename and diagnostics.
      All three recovered after server restart with unsaved snapshots replayed,
      then shut down cleanly. The disposable workspace is
      `/tmp/ahead-lsp-smoke-BE8xsl`. This proves managed npm acquisition and
      proxy/server behavior, not the Zed extension-gallery install or native
      editor popup/insertion/Problems UI. The smoke now fails immediately when
      a selected language server reports `server_status.error` instead of
      waiting for the diagnostics timeout; `node --check` passes, and a
      controlled install-error run reported the server failure in 2.6 seconds.
      Rechecked at 02:03 ET on 2026-10-08: this shell's default PATH lacks
      Vtsls and BasedPyright, and its `rust-analyzer` entry is a rustup shim
      without the component, so the default smoke correctly stopped before
      launch. Using the existing Zed-cached Vtsls and BasedPyright binaries,
      the TypeScript/JavaScript/Python subset passed completion, navigation,
      diagnostics/repair, semantic tokens, reopen/restart recovery and graceful
      shutdown in a disposable workspace at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-2oHunV`.
      No download, install or provider call. This run used `target/debug/ahead`
      from 00:18; Rust Analyzer and native UI behavior were not covered.
      Repeated the full matrix after that test change using the retained
      Vtsls, BasedPyright and Rust Analyzer executables without downloads; all
      language checks, restart/replay and graceful shutdown passed again. The
      second disposable workspace is `/tmp/ahead-lsp-smoke-ZaaMBM`.
      Revalidated at 05:29 ET on 2026-10-08 against the `target/debug/ahead`
      binary rebuilt by `just dev` at 05:19:48. The complete Rust,
      TypeScript/JavaScript and Python matrix passed with the Zed-cached Vtsls,
      BasedPyright and Rust Analyzer; restart replayed unsaved buffers and all
      servers shut down cleanly. No install, download or provider call occurred.
      Disposable workspace: `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-0pi86T`.
      This confirms current-binary proxy/server behavior, not native completion,
      syntax-color or Problems-view rendering.
      Revalidated again against the 06:35 `target/debug/ahead` build in a
      sanitized environment using Zed-cached Vtsls, BasedPyright and the
      versioned Rust Analyzer. TypeScript/JavaScript, Python and Rust all passed
      completion, definitions, diagnostics and repair, semantic tokens, server
      restart with unsaved-buffer replay, and graceful shutdown; TypeScript also
      passed quick-fix and auto-import resolve, while Rust passed formatting,
      hover, signature help, references and cross-file rename. No provider
      credentials were inherited; no packages were installed or downloaded.
      Disposable workspace: `/private/tmp/ahead-lsp-smoke-QN2Cf6`. This remains
      proxy/server evidence, not native completion-popup, insertion, diagnostics
      or syntax-color rendering.
      The completion bridge now preserves LSP `null` separately from a genuine
      empty list; the editor retries one null response after 200 ms and cancels
      that retry when the buffer generation, file or caret changes. The focused
      GPUI regression passes; verify this recovery against live Rust,
      TypeScript and Python servers and the native popup. Tab also inserted the suggestion,
      Escape dismissed the popup, and Down preserved the editor caret in the
      rendered app. Accepting a suggestion no longer reopens the popup; the
      focused GPUI test covers Enter and one-edit suppression. Verify Up/Down
      selection with multiple suggestions. The Language Servers panel now has
      a workspace Restart action and live status/error updates. The proxy
      observes actual child exit, fails outstanding requests, waits for stop,
      and reopens the current unsaved snapshots with replacement servers.
      A subprocess regression covers a descendant holding stdout open after
      its server exits. Verify the rendered restart/error/recovery journey.
      Shutdown now follows Zed's five-second `shutdown`/`exit` deadline before
      forced termination. Sent requests, including initialization, have Zed's
      120-second fallback deadline; normal timeouts send cancellation and fail
      the request without closing the server. Tests cover cooperative and
      unresponsive real child processes, ordered exit/IO closure, timeout
      cancellation and late-response rejection. On 2026-10-07, direct execution
      of `ahead_proxy-7d82424f1299b774` (05:08:15, newer than
      `ahead-proxy/src/plugin/lsp.rs`) passed all seven
      `plugin::lsp::tests`, including workspace trust, executable resolution,
      child-exit detection, and cooperative/forced shutdown. This is current
      proxy unit evidence, not native editor verification. Verify delayed
      startup and timeout feedback in the native app; add a per-server timeout
      setting when the language settings owner/event path below is implemented.
      Test repeated restarts for inherited-pipe or
      descendant-process leaks, plus restart during edits and tab closure.
      Verify native Cmd+Q/window-close process cleanup after desktop unlock.
      Closing an authorized window now stops its proxy after recovery is
      confirmed. GPUI tests cover Cancel, failed/stale recovery and final
      buffer-close-before-shutdown ordering. The proxy now waits for catalog
      cleanup before exiting on shutdown or stdin EOF. The standalone
      `tests/lsp-shutdown-smoke.mjs` passes cooperative, delayed-initialization
      and unresponsive-server cases; it fails against the previous binary
      that exited before the LSP handshake. After the workspace-trust gate was
      added, the smoke now isolates `AHEAD_DATA_HOME` and grants only its
      disposable project language-server permission; all four shutdown modes
      pass again against the fresh 01:38 debug binary (2026-10-03), and the
      disposable fixture was removed. A 2026-10-14 native-window observation
      found the trust prompt only said “this workspace,” so its target could
      not be matched to the previously approved disposable project. The prompt
      now names the workspace folder and shows its full path beside the warning.
      The focused `workspace_trust_prompt_identifies_project_and_full_path`
      test passes; do not accept a trust prompt until its path identifies the
      intended disposable workspace. Native rendering of the updated prompt
      remains unverified. All four modes passed again after the
      post-clean rebuild and on 2026-10-04 against the current-source binary.
      On 2026-10-07, the current `target/debug/ahead` binary also passed
      `rtk node tests/lsp-shutdown-smoke.mjs target/debug/ahead` in all four
      modes: shutdown, stdin EOF, initialization disconnect and unresponsive
      server. Each process was reaped and its disposable source stayed
      unchanged; this run remains at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-shutdown-kXQlYk`.
      Fresh rerun on 2026-10-08 against `target/debug/ahead` (10:22 build)
      passed the same four modes; the language-server child was reaped in every
      case and the newly created disposable fixture was removed afterward.
      The
      installed-server smoke also verifies natural proxy exit and an empty
      test process group.
      Installed-extension command resolution now runs off the catalog thread
      and reports through a generation-fenced catalog notification. The catalog
      retains the current unsaved Rope snapshot and replays it to the newly
      started server, ignores a result after document close, and rechecks
      workspace trust before startup. Shutdown signals cancellation and waits
      up to one second for resolver workers; permitted npm/process children
      now use Tokio async pipes with a 120-second deadline; timeout and caller
      cancellation explicitly kill and reap the child before returning. Unix
      regressions assert both timeout and cancellation leave no child process.
      The focused unsaved-replay and late-close regressions pass; 17 catalog
      tests pass with one opt-in real-extension test excluded from the normal
      suite. That test also passed after this async change using a local Zed
      HTML extension, its cached server, and an isolated temporary AHEAD data
      home; it observed a successful LSP handshake. Still prove first-open
      responsiveness with an installed Zed extension in a native app, real
      child cancellation/reaping and descendant cleanup on shutdown, repeated
      restart, and Linux/Windows behavior. Discovery itself is still synchronous;
      avoid treating the worker move as complete catalog responsiveness.
      Verify active native/ACP cleanup in the unlocked app with real providers.
      The proxy now stops the session controller; native parent/child shutdown,
      pending editor requests, ACP startup interruption and persisted partial
      cancellation have focused tests. `tests/agent-shutdown-smoke.mjs` passes
      real proxy exit and Turso reopen on explicit shutdown and stdin EOF; the
      2026-10-02 rebuilt-binary rerun passed both cases in disposable
      `ahead-agent-shutdown-4d6YT8` using a loopback fake model. The
      2026-10-05 rerun also passed SIGKILL recovery: streamed partial text
      survived Turso reopen, was not replayed automatically, and an explicit
      retry issued exactly one Responses POST. Its fixture now handles `/v1/models`
      separately from model-turn request counts. These remain mock-proxy, not
      authenticated or rendered-app, results. The
      previous binary fails its cancelled-status assertion. Cover slow process
      creation, blocked storage, pending MCP calls and adapter descendants.
      Repeat terminal cleanup through native window close/quit after unlock;
      GPUI routing and real PTY tests now cover explicit tab close, retained
      panel references, Cancel, recovery failure, and HUP/TERM-resistant shell
      and foreground processes. Neither those tests nor agent/LSP tests prove
      debugger cleanup; see the DAP ownership gap below.
      The real Rust Analyzer/Vtsls/BasedPyright proxy probe now passes unsaved
      replay, diagnostic clear/repair, completion, definition and replacement
      identity checks; old Vtsls completion items are rejected after restart.
      Repeat those checks through the native editor after desktop unlock.
      On 2026-10-08, with Node v24.15.0 and npm 11.12.1 on PATH, the shared
      `just dev` loop rebuilt the current source after the Cargo.lock change in
      34.57 seconds. Launching through that shell aborted with macOS
      `hiservices-xpcservice` connection-invalid errors (signal 6); launching
      the freshly built app through LaunchServices succeeded in the already
      approved disposable project. The native Language Servers panel showed
      BasedPyright and Vtsls Ready. Python `pri` completion included `print`;
      TypeScript `result.` returned `doubled` and `input`; JavaScript `tri`
      returned the fixture's `triple`. All probe edits were undone and the
      editor buffers matched their original fixture text. This confirms live
      Python/TypeScript/JavaScript completion on the current build, not just
      server readiness. The shell-launched GUI abort remains a separate
      `just dev` issue; the native app was opened via LaunchServices for these
      checks.
      Follow Zed's versioned buffer lifecycle in
      `../zed/crates/project/src/lsp_store.rs`: replace per-keystroke full
      snapshots with small incremental edits; `didClose` is now wired from
      preview replacement and confirmed tab closure. Verify close/reopen under
      a real server and external file rename/deletion; match Zed's
      completion filtering and snippet behavior in
      `../zed/crates/editor/src/completions.rs`. Add first-run Rust Analyzer
      acquisition or an actionable missing-server UI; an override alone is
      only a development probe.
      F12 now emits a file/range open request through the existing shell path,
      rejects stale replies and uses UTF-16 `targetSelectionRange`. Verify the
      full rendered route, then add back navigation and a multiple-target picker.
      Completion filtering/ranking now honors `filterText`/`sortText` before
      the 12-item display limit. Auto-import acceptance now resolves the full
      item through its original server and applies its symbol/import edits as
      one Undo operation. Focused GPUI/client tests cover Unicode, invalid and
      overlapping ranges, stale results and transport failure; verify the
      rendered selection/resolve/Undo path with Vtsls. A clean-room LSP snippet
      parser now expands numbered stops, nested defaults, repeated references,
      choices and escapes;
      completion is one undoable edit, selects placeholders, tracks edits and
      supports Tab/Shift+Tab through the final caret. Snippet choice lists now
      retain every escaped option, render beside the active placeholder, and
      cycle with Up/Down while preserving the current completion edit's undo
      boundary. Common LSP snippet variables now resolve from the live buffer,
      selection, clipboard, file and workspace (`TM_SELECTED_TEXT`,
      `TM_CURRENT_LINE`,
      `TM_CURRENT_WORD`, cursor index/number, line index/number,
      filename/directory/filepath, relative filepath, workspace folder/name,
      clipboard, standard date/time, random values and UUID); unknown variables
      use their defaults or become editable placeholders.
      Variable regex transforms now support captures, look-around, bounded
      backtracking, global/case/multiline/dot-all options, and
      upcase/downcase/capitalize plus conditional formats.
      AHEAD advertises `snippetSupport: true`, allowing the installed Zed HTML
      server to expose its completion provider. `LINE_COMMENT`,
      `BLOCK_COMMENT_START` and `BLOCK_COMMENT_END` now cover built-in Rust,
      JavaScript/TypeScript, Python and HTML/XML file types. The extension host
      also reads Zed `line_comments` and `block_comment` config for matching
      languages, even when no grammar is installed; declared fields override
      filename fallbacks while missing fields retain them. Host, snippet and
      stale-response regressions were added but not run.
      Before calling this production-ready, implement synchronized editing of
      repeated tabstop ranges (gpui-kit currently exposes only one selection),
      then verify the real HTML completion, choice popup and Undo path in the
      native disposable project. Do not import Zed's
      GPL-3.0-or-later `crates/snippet`. Completion commands and large-span edit
      performance still need review. The focused GPUI test covers insertion,
      one-step Undo, choice cycling, placeholder selection/editing, Tab/Shift+Tab,
      final-caret exit and normal Tab indentation; the proxy capability
      regression passes.
      The full 2026-10-03 app-library suite passed 220 tests with loopback
      enabled; an earlier sandboxed run could not bind two collaboration mock
      servers (`PermissionDenied`). Standard initialization
      now supplies ready/error states for every server, with consistent names
      for experimental updates; retired-process status updates cannot overwrite
      the replacement server. Restart rendering and hung-server handling
      remain open as described above.
      On 2026-10-08, the existing app test runner (15:18, newer than
      `lsp_snippet.rs` at Oct 3 12:19) passed all 7 snippet parser/session
      tests without Cargo. Repeated-range synchronization, choice UI, variables
      and native completion insertion remain open.
      `tests/lsp-smoke.mjs` now runs real installed Vtsls and BasedPyright
      against a fresh disposable copy. On 2026-09-30 it passed TypeScript,
      JavaScript and Python completion, cross-file definition, diagnostics and
      repair, plus TypeScript auto-import resolve. It reproduces the old
      Vtsls `douconst` corruption when shortening an unsaved buffer; explicit
      old-buffer ranges for incremental sync fix it. This is proxy/runtime
      evidence, not native UI proof. The Mac locked during the rendered pass;
      resume native verification after the user unlocks it. A fresh 2026-10-02
      root-binary run of the unchanged smoke passed the full Rust, TypeScript,
      JavaScript and Python flow, plus restarted-server identity, unsaved replay,
      stale resolve rejection and graceful cleanup. It used already-installed
      Zed Vtsls, BasedPyright and standalone Rust Analyzer binaries on PATH;
      a rustup `rust-analyzer` shim alone failed because that toolchain had no
      component. The restricted run could not spawn a server, and the first
      unrestricted run lacked installed binaries on PATH; neither is counted
      as a product failure. After the 2026-10-03 cold rebuild, the same offline
      smoke passed using existing Zed Vtsls, BasedPyright and standalone Rust
      Analyzer binaries on PATH. Native editor interaction remains open.
      After the language-server trust gate was added, the smoke now uses an
      isolated `AHEAD_DATA_HOME` and grants only its disposable fixture
      permission. A 2026-10-03 rerun against the existing debug binary passed
      the full Rust/TypeScript/JavaScript/Python flow, restart, unsaved replay
      and shutdown with locally cached Zed server binaries; no download or
      real project was involved.
      An initial current-shell attempt resolved `~/.cargo/bin/rust-analyzer`, a
      rustup shim whose selected toolchain has no Rust Analyzer component; the
      versioned server already in Zed's cache was not named `rust-analyzer`.
      `tests/lsp-smoke.mjs` now accepts `AHEAD_LSP_RUST_ANALYZER_PATH` and
      aliases that executable only inside its disposable copy. With the cached
      Zed Rust Analyzer, Vtsls and BasedPyright, the rebuilt 2026-10-03 proxy
      passed the full three-language smoke, including Rust hover, references
      with the declaration, prepare/cross-file rename, diagnostics, restart,
      unsaved replay and shutdown. The reference request now includes the
      declaration, matching Zed's `crates/project/src/lsp_command.rs`. This is
      proxy evidence only. On 2026-10-06, the current `target/debug/ahead`
      passed the complete offline matrix again with Zed's cached Vtsls,
      BasedPyright and Rust Analyzer. TypeScript, JavaScript, Python and Rust
      completion/navigation/diagnostics, repairs, auto-import resolution, Rust
      formatting/signature help/references/rename, close/reopen, restart with
      unsaved-buffer replay, and process-group cleanup passed in disposable
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-bfuhrr`.
      Rendered editor interaction remains open. Format Document is now exposed by Shift+Alt+F and the
      Editor menu; a focused GPUI regression verifies it applies as one undoable
      edit, preserves the caret and rejects late replies after newer typing. The
      real Rust Analyzer smoke confirms formatting edits arrive for an unformatted
      buffer. The shortcut follows pinned Zed's VS Code-format mapping in
      `../zed/assets/keymaps/macos/vscode.json`. Verify the shortcut/menu and
      rendered result in the native editor after unlock. Manual signature help
      is wired to Cmd/Ctrl+Shift+Space and the Editor menu, with active-parameter
      highlighting, UTF-16 label-offset handling, overload cycling and Escape
      dismissal. Focused GPUI tests cover the routed response and popup; the
      disposable Rust Analyzer proxy smoke returns a real signature.
      The focused above-caret signature-help GPUI test passed on 2026-10-14
      (1/1), including parameter documentation. The same-day `just dev` rebuild
      completed in 2.24s, but the app again aborted at HIServices initialization
      before drawing; CUA reports that the Mac is locked, so rendered acceptance
      remains open.
      Code actions now use Zed's Cmd/Ctrl+. affordance and show a
      filtered-at-caret action menu. AHEAD applies unversioned active-file text
      edits as one undoable change; cross-file and versioned edits use the
      shared workspace-edit validation and application path, and an attached
      server command runs only after that path confirms success. Execute
      commands remain limited to commands from a server's static or dynamically registered
      `executeCommandProvider`; AHEAD advertises dynamic registration and
      unregisters commands by registration ID. The static/dynamic command
      behavior follows Zed's `../zed/crates/lsp/src/lsp.rs` and
      `../zed/crates/project/src/lsp_store/dynamic_registration.rs`. On
      2026-10-09, the dynamic command overlap/unregister-by-ID regression passed
      with `cargo test --locked --offline -p ahead-proxy --lib
      dynamic_execute_commands_are_allowlisted_and_unregistered_by_id`.
      Static-command and live extension-backed execution still need their own
      verification. Resource operations remain fail-closed.
      Server-initiated `workspace/applyEdit`
      now follows Zed's `ApplyWorkspaceEdit` request path into the app, where it
      uses AHEAD's shared workspace-edit validation/application path and returns
      the actual applied/failure result to the server. RPC relay and Core-RPC
      stdio request/response round-trip regressions are added but still need
      rebuilding and execution in the shared dev sequence. Focused GPUI tests
      cover request, popup, lazy resolve, active-file edit, command
      dispatch and undo, and the new cross-file regression checks command
      ordering. The
      real Vtsls smoke returns its `Add import` edit and executes its advertised
      command through the proxy. Verify the actual Rust Analyzer quick fix,
      shortcut/menu rendering and the remaining LSP journeys in the native
      editor. `just dev`
      rebuilt successfully, but its app launch aborted with the Mac locked; no
      native UI result is claimed.
      On 2026-10-07, four focused tests passed directly from the existing app
      test binary (00:51:24, newer than `ahead-app/src/app.rs` and
      `ahead-app/src/code_panel.rs`): workspace-edit preflight/version/read-only
      validation, grouping edits by file, cross-file code-action ordering, and
      complete rename edits. This is source-matched app-unit evidence only; the
      binary predates the current `proxy_client.rs`, and the live server's
      `workspace/applyEdit` round trip still needs the shared build and native
      disposable-app checks. On 2026-10-07, a focused locked/offline Cargo run
      against the current source passed all five `ahead-app --lib
      workspace_edit` tests; server round-trip and rendered acceptance remain
      open.
      Revalidated on 2026-10-08 against the root binary rebuilt by `just dev`
      at 00:22, using only Zed-cached Vtsls, BasedPyright and Rust Analyzer.
      Rust, TypeScript, JavaScript and Python passed completion, navigation,
      diagnostics/repair and semantic-token requests; Rust also passed hover,
      signature help, references, cross-file rename and formatting, and
      TypeScript passed quick-fix/auto-import resolve. Close/reopen, restart
      with unsaved-buffer replay and graceful process cleanup passed as well.
      The disposable workspace is
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-ErPwYz`.
      This remains proxy/server evidence; native popup insertion and rendered
      diagnostics are still unverified.
      Later on 2026-10-08, a fresh `just dev` build compiled the updated proxy
      and app before macOS again aborted launch on the locked desktop's
      `hiservices-xpcservice` error. The full disposable LSP matrix then passed
      against that binary using Zed's cached Vtsls 0.3.0, BasedPyright 1.40.0
      and Rust Analyzer: completion, navigation, diagnostics/repair, semantic
      tokens, TypeScript quick-fix/auto-import, Rust formatting/rename, and
      restart with unsaved-buffer replay and graceful shutdown. No provider or
      package download was used. This validates real server/proxy behavior,
      not native popup insertion or Problems rendering. A separate clean
      managed-acquisition retry hit npm `ENOTFOUND` in this environment; the
      installer now exposes only the allowlisted npm error code from a bounded
      stderr tail, never raw output that could contain registry credentials.
      The focused locked/offline proxy `npm` tests passed all four cases,
      covering safe error reporting, bounded capture, publication,
      concurrency and cancellation.
      Rendered LSP check on 2026-10-08 in the running disposable app: F12 from
      Rust `math::double` opened its `math.rs` definition; F12 from Python's
      `double` call opened `arithmetic.py`. Opening the JavaScript fixture also
      started `vtsls`, shown Ready alongside `rust-analyzer` and `basedpyright`.
      These confirm native cross-file navigation for Rust and Python and
      TypeScript/JavaScript server startup; diagnostics, rename and formatting
      still need their own rendered acceptance checks.
      Rechecked the real-server protocol path on 2026-10-08 against
      `target/debug/ahead`, newer than the proxy catalog, LSP bridge, app editor,
      RPC and smoke sources. The complete Rust, TypeScript/JavaScript and
      Python matrix passed using Zed's cached Vtsls, BasedPyright and Rust
      Analyzer: completion, navigation, diagnostics/repair, semantic tokens,
      TypeScript quick-fix/auto-import, Rust formatting/signature help/references/
      cross-file rename, unsaved-buffer restart recovery and graceful shutdown.
      The default shell PATH has no Vtsls/BasedPyright and only a Rust Analyzer
      rustup shim. The first managed-install attempt hit sandbox DNS
      `ENOTFOUND`; a network-enabled retry with
      `AHEAD_LSP_INSTALL_MISSING=1` acquired Vtsls, BasedPyright and Rust
      Analyzer into the fixture's own
      `ahead-data/cache/language-servers/` and passed the same full matrix.
      Nothing was installed globally and no model request occurred. Offline
      fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-STxIWJ`;
      failed install fixture retained at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-JHBazf`.
      Managed-install fixture retained at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-NwLMIo`.
      The Mac locked during follow-up UI input, so live completion-popup,
      acceptance and rendered diagnostics remain open.
      Follow-up on 2026-10-08 in the rebuilt AHEAD app on the approved
      disposable Rust/TypeScript/Python fixture: manual Ctrl+Space displayed
      real LSP completions, and screenshots confirmed each popup is positioned
      above the active caret without covering the line being edited. F12 also
      opened the TypeScript and Python definitions. No fixture files were
      changed. This supersedes the locked-desktop limitation for popup placement
      and these definition-navigation checks only; rendered diagnostics,
      rename, formatting and code-action flows remain open.
      Follow-up on 2026-10-08 lowered the ranked completion cap from 12 to 8,
      after fuzzy score, preselect and server sort ordering. The focused offline
      regression passed with 12 matching and 30 irrelevant items, asserting the
      top 8. A fresh native app launch rendered a two-result Python completion
      popup above the active line; this fixture did not produce more than eight
      live results. After the latest screenshot still showed too many rows, the
      current source caps the fuzzy-ranked menu at five, and its regression
      verifies that a weaker fuzzy match cannot displace a stronger result just
      because its server sort key is earlier. This five-item source change has
      now been verified on 2026-10-09 by the current app runner (12:20,
      newer than `code_panel.rs` at 11:37), which passed the five-item ranking
      and above-caret popup GPUI
      tests. The 12:42 root binary passed the full managed Rust/TypeScript/
      JavaScript/Python LSP matrix plus a live Zed HTML 0.3.2 install, including
      TypeScript quick-fix/auto-import, server restart with unsaved-buffer
      replay and graceful shutdown. Its disposable project and isolated package
      cache remain at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-mz6z4G`;
      no global caches changed. This is real-server/proxy and headless popup
      evidence; rendered five-item count and Problems view remain open.
      On 2026-10-12, the 09:57 `just dev` binary passed the complete
      `tests/lsp-smoke.mjs` Rust/TypeScript/JavaScript/Python matrix using a
      fresh isolated workspace. TypeScript auto-import/quick-fix and Python
      completion/diagnostics passed; Rust Analyzer 1.95.0 passed completion,
      formatting, navigation, hover, signature help, references and rename.
      All languages passed diagnostics repair, semantic tokens, close/reopen,
      server restart with unsaved-buffer replay and graceful shutdown. Vtsls
      and BasedPyright were copied from an existing disposable AHEAD cache;
      the smoke now supports this explicit `AHEAD_LSP_CACHE_SEED` for offline
      reruns and documents it in the fixture README. A fresh acquisition attempt
      without that seed hit npm DNS `ENOTFOUND`, so new package acquisition
      remains network-dependent. Fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-rWb3fl`.
      This is current real-server/proxy evidence; the native AHEAD window still
      aborts before drawing, so rendered editor/LSP behavior remains open.
      Revalidated on 2026-10-12 against `target/debug/ahead` (built
      2026-10-11 16:21) with the seeded disposable Vtsls, BasedPyright and
      Rust Analyzer cache. Rust, TypeScript, JavaScript and Python passed the
      real-server proxy matrix, including completion/navigation, diagnostics
      repair, semantic tokens, TypeScript quick-fix/auto-import, Rust formatting
      and cross-file rename, close/reopen, restart with unsaved-buffer replay,
      and graceful shutdown. Fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-cYIaFk`.
      This remains proxy/server acceptance; native editor popup, insertion,
      trust UI and Problems-view rendering are still open.
      Later on 2026-10-12, the current `target/debug/ahead` passed the full
      Rust/TypeScript/JavaScript/Python proxy matrix using a seeded disposable
      language-server cache. Rust passed formatting, completion, definition,
      hover, signature help, references, cross-file rename and diagnostic
      repair; TypeScript passed quick-fix and auto-import resolution; every
      language passed completion/navigation, semantic tokens, diagnostics
      repair, close/reopen, unsaved-buffer replay after server restart and
      graceful shutdown. The run was retained at
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-Yu8Mtc`.
      The current binary also passed all four `tests/lsp-shutdown-smoke.mjs`
      modes and `tests/editor-recovery-smoke.mjs`'s acknowledged-buffer,
      discard and owner-only database checks. These proxy/process checks do not
      replace native popup, trust or Problems-view acceptance.
      Rechecked after the 2026-10-12 build against the same isolated offline
      cache seed: Rust, TypeScript, JavaScript and Python again passed the full
      `tests/lsp-smoke.mjs` matrix, including completion, navigation,
      diagnostics/repair, semantic tokens, TypeScript quick-fix/auto-import,
      Rust formatting/signature help/references/cross-file rename, restart with
      unsaved-buffer replay and graceful shutdown. Fresh fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-SugOCX`.
      No server acquisition or network request was needed; the native AHEAD
      window remains unavailable for rendered acceptance.
      The same current `target/debug/ahead` also passed all four
      `tests/lsp-shutdown-smoke.mjs` modes and
      `tests/editor-recovery-smoke.mjs` against fresh disposable workspaces:
      shutdown waits for server cleanup, acknowledged Unicode buffers survive
      abrupt proxy exit, stale snapshots are fenced, and database permissions
      remain owner-only. These are protocol/process checks, not native UI proof.
      Rechecked against the latest `target/debug/ahead` on 2026-10-12 with the
      retained disposable Rust Analyzer/Vtsls/BasedPyright cache seed. Rust,
      TypeScript, JavaScript and Python again passed completion, navigation,
      diagnostics/repair, semantic tokens, TypeScript import actions, Rust
      formatting/hover/signature help/references/cross-file rename, close and
      reopen, restart recovery and graceful proxy shutdown. The fixture is
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-PD6sy6`.
      This remains real-server proxy evidence; native popup, insertion, trust
      and Problems-view rendering remain open.
      On 2026-10-14, completion rows now combine LSP detail and label details
      so signatures/types remain visible while typing; the popup is wider and
      capped to the five highest-ranked items. Signature help stays anchored
      above the caret, updates after a short debounce while inside a call, and
      shows active-parameter and signature documentation. Focused GPUI tests
      and the full `ahead-app` library suite passed (325 tests; two local-socket
      tests filtered in this environment). Native popup placement and legibility
      still need a rendered-window check; `just dev` builds, but macOS aborts
      before window creation with a HIServices XPC error.
      Revalidated on 2026-10-14: the completion-filtered suite passed 11 tests
      and the signature-help placement/documentation GPUI test passed (1/1).
      Also revalidated on 2026-10-14 against the current `target/debug/ahead`
      with a seeded disposable cache and `npm_config_offline=true`: Rust,
      TypeScript, JavaScript and Python passed the real-server matrix. Rust
      passed formatting, completion, definition, hover, signature help,
      references, cross-file rename, diagnostics/repair, close/reopen and
      unsaved-buffer recovery after restart; TypeScript/JavaScript passed
      completion, definition, diagnostics/repair, semantic tokens, quick-fix
      and auto-import; Python passed completion, definition, diagnostics/repair
      and semantic tokens. Disposable fixture
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-0q2Wf6`
      was removed after the run. Server acquisition was not exercised; native
      popup, insertion, trust and Problems-view acceptance remain open.
      Follow-up on 2026-10-14: Zed's pinned
      `crates/editor/src/code_context_menus.rs::resolve_visible_completions`
      resolves visible candidates as selection changes. AHEAD now resolves the
      selected completion when its initial `detail` is missing, updates the
      visible row from the response, extracts signature-like first lines from
      resolved documentation, and fences late replies to the current popup
      context. A cached real BasedPyright run confirmed that the initial
      completion is bare while `completion/resolve` returns
      `def double(value: int) -> int`; signature help returns
      `(value: int) -> int`. The disposable Python LSP smoke passed completion
      resolution, signature help, definition, diagnostics/repair, reopen and
      restart. Focused GPUI resolution/stale-response, row-detail, comment
      session-gate and popup-placement tests pass. The shared `just dev` build
      passed in 9.65s after this change, but macOS again aborted in HIServices
      before the first frame; rendered popup and comment appearance remain open.
      Follow-up on 2026-10-14: the initial LSP completion reply now uses an
      async channel rather than blocking a GPUI background worker on a
      crossbeam receive. The null-result retry test and all 12
      completion-filtered GPUI/ProxyClient tests pass; rustfmt and diff checks
      pass. Also reran the no-session comment-action gate (1/1) and all three
      signature-help GPUI tests. The 1.5-second completion and signature-help
      fallbacks now race their async-channel replies against GPUI background
      timers, removing timed-out requests without a sleeping OS thread per
      request; the hover path's no-op sleeper was removed as well. The Ctrl+Space
      interaction test now supplies its mocked null response and drains the
      GPUI task. The active shared `just dev` loop rebuilt the integrated app
      after these changes; scoped rustfmt and diff checks pass, but the test
      binary was not rebuilt. The Mac is locked, so post-change GPUI and rendered
      acceptance remain open.
      The rebuilt 20:09 `target/debug/ahead` then passed the disposable
      all-language LSP smoke with managed acquisition enabled: installed the
      live Zed HTML extension 0.3.2 and exercised HTML, Vtsls TypeScript and
      JavaScript, BasedPyright Python, and Rust Analyzer. Completion, navigation,
      diagnostics/repair, semantic tokens, TypeScript quick-fix/auto-import,
      Rust formatting/hover/signature/references/cross-file rename, document
      reopen, server restart recovery, and graceful shutdown passed. Fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-AfizGS`.
      This verifies live installer/proxy/server behavior without model calls;
      native editor popup, insertion and Problems rendering remain open.
      Revalidated at 21:11 ET on 2026-10-14 against that current 20:09 binary
      with a copied server cache and managed acquisition enabled: TypeScript,
      JavaScript and Python passed completion, definition, diagnostics/repair,
      semantic tokens, TypeScript quick-fix/auto-import, close/reopen, restart
      recovery and graceful shutdown. Rust Analyzer was unavailable, so Rust
      was not in this run. Disposable fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-oBD53A`.
      This is current-binary proxy/server evidence, not rendered editor UI.
      On 2026-10-15, reran the full Rust/TypeScript/JavaScript/Python
      `tests/lsp-smoke.mjs` matrix against `target/debug/ahead` built at 10:28,
      newer than the editor, extension catalog/host and LSP bridge sources. The
      run seeded the disposable AHEAD data home from the retained local server
      cache. All languages passed completion, definition, diagnostics/repair
      and semantic tokens; TypeScript also passed quick-fix/auto-import, Python
      close/reopen, and Rust formatting, hover, signature help, references,
      cross-file rename and unsaved diagnostics. Fresh-process restart with
      unsaved-buffer recovery and graceful shutdown passed. Fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-jnE6rM`.
      This is proxy/server integration evidence, not native popup/insertion or
      Problems-view acceptance. A subsequent run on the same binary enabled
      the user-approved live Zed catalog/package path: HTML extension 0.3.2
      installed and completed HTML LSP checks, and the Rust/TypeScript/
      JavaScript/Python matrix passed again. Fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-npaSzQ`.
      This verifies live catalog/install and proxy/server behavior, not the
      native extension gallery, completion popup or Problems-view rendering.
      `tests/lsp-shutdown-smoke.mjs` also passed against that binary in all four
      cases: explicit shutdown, stdin EOF, EOF during initialization, and an
      unresponsive server. Each case waited for child cleanup and confirmed the
      unsaved source remained unchanged. Fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-shutdown-AniENf`.
      Revalidated on 2026-10-16 against the current integrated binary: all
      four fake-server shutdown modes passed again, each child process was
      reaped, and the unsaved source remained unchanged. Disposable fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-shutdown-90bDQs`.
      Also rebuilt the app test runner from current sources at 09:45 on
      2026-10-16 and passed all 70 `code_panel::tests` serially. Coverage
      includes typed completion details, above-caret completion/signature
      placement, active-parameter documentation, stale reply fencing and
      completion acceptance. This remains headless GPUI evidence; the native
      popup/insertion path is still unverified.
      On 2026-10-19, signature-help requests began carrying LSP context from
      the editor through proxy/RPC to the language server: manual requests use
      `Invoked`, edit-triggered refreshes use `ContentChange`, and active
      signature state is retained for retriggers. The client advertises
      `contextSupport`; GPUI and proxy serialization/capability regressions
      cover the path. Scoped formatting and diff checks pass; Cargo tests were
      not run, so runtime behavior remains for the user to verify.
      On 2026-10-20, completion popup placement switched from a guessed
      character-width offset to rendered bounds at the actual cursor (not the
      selected range), and clamps width to the editor pane. The focused GPUI
      fixture now includes a tab, wide Unicode and a non-empty selection. Unit
      and GPUI assertions are added but not run: an unrelated Cargo build
      occupied the shared disk during this pass, leaving 852 MiB free after it
      exited, and no AHEAD test runner is present. The Mac is locked, so native
      rendered placement also remains unverified.
      Zed's `document_selector_matches` also checks URI schemes
      case-insensitively. AHEAD now retains the LSP selector scheme and applies
      the `file` constraint to both startup selectors and dynamic save
      registrations; non-file schemes no longer claim workspace-file events.
      A proxy regression covers case-insensitive file matching and rejects an
      `untitled` save registration. It has not been run in this checkout.
      On 2026-10-20, removed the completion-detail row's single-line truncation
      so long LSP signatures and argument/return types can wrap. The shared
      `just dev` loop rebuilt and relaunched the integrated app; the focused
      GPUI wrapping assertion and rendered popup still need verification.
- [ ] Finish Zed language-extension configuration lifecycle. AHEAD now
      advertises and answers `workspace/configuration` from the extension's
      startup snapshot. `ahead-extension-host/src/host.rs::get_settings`
      previously returned `{}` for every category. It now reads the requested
      `[lsp.<server-id>]` from bounded, symlink-safe workspace config layers:
      tracked `.ahead/config.toml` can supply `settings` and
      `initialization_options`, while only ignored `.ahead/config.local.toml`
      may select an executable `binary`. User-level
      `~/.ahead/settings.toml` defaults and workspace-private
      `.ahead/settings.toml` now layer around these files, merging individual
      nested settings while keeping executable overrides local-only. A focused safety/scope test is added;
      it passes with the eight-active-test extension-host library suite, and the
      locked offline `ahead` binary check passes. Workspace `.ahead` file
      changes now queue a debounced refresh. The catalog computes the guest's
      workspace configuration off-thread, ignores stale results, and updates
      the live `workspace/configuration` response before sending
      `workspace/didChangeConfiguration` after initialization. Focused proxy
      tests pass for the watcher event and protocol notification; the offline
      proxy library suite passes 225 tests with local loopback permitted.
      Installed Proto and HTML API-0.7 guests read changed settings in
      disposable offline host probes. The opt-in real Zed HTML server smoke now
      also verifies initial `workspace/configuration` and embedded CSS
      diagnostics. That exposed and fixed a compatibility edge: an
      unconfigured named section must be `{}`, since the HTML server passes
      its CSS section directly to the validator. User-global settings changes
      are now watched by a separate non-recursive watcher when `~/.ahead`
      exists at proxy initialization. Create/remove/modify events for the exact
      `settings.toml` path are debounced and refresh extension configuration
      without reindexing workspace files or recalculating Git diff. A focused
      regression covers that notification boundary. The home directory is now
      watched non-recursively for
      the exact `.ahead` entry; creation/removal dynamically attaches/detaches
      the filtered settings watcher. A source-matched deterministic regression
      injects `.ahead` and `settings.toml` events and verifies nested watcher
      registration plus configuration refresh. The raw native watcher emitted
      no callbacks in this test process, so OS event delivery still needs a
      live check. Follow-up on 2026-10-13: the sandboxed one-directory Node
      probe failed at watcher registration with `EMFILE`; the same disposable
      probe received a native event when run with filesystem-notification
      access. Added
      `ahead-proxy/src/watcher.rs::filtered_nonrecursive_watch_delivers_a_native_file_event`;
      the focused offline test passed (1/1) and verifies the OS event reaches
      AHEAD's filtered watch token/path route. Also added
      `ahead-proxy/src/dispatch.rs::workspace_settings_change_reaches_catalog_from_native_file_watcher`;
      it passed (1/1) and verifies an actual workspace `.ahead/config.toml`
      edit reaches `RefreshWorkspaceConfigurations`. Combined with the
      installed HTML-server refresh smoke, this covers both sides of the
      watcher-to-server update path, but not as one live end-to-end run.
      Workspace refresh now resolves the full extension command snapshot: a
      launch command, arguments, environment or initialization-options change
      restarts only that server, while ordinary `settings` changes still send
      `workspace/didChangeConfiguration` without a restart. The process-identity
      comparison regression passes for all four launch fields and confirms
      settings-only changes stay live. Extension `LanguageSettings` requests
      now read `tab_size`, `hard_tabs`, and `preferred_line_length` from the
      matching AHEAD `[languages.<name>]` layers, with Zed-compatible defaults
      and validation; focused host regressions cover case-insensitive language
      names, layer precedence and invalid values. The editor now uses the same
      shared resolver and applies `tab_size`/`hard_tabs` when attaching a
      workspace, opening a file, and receiving a workspace-settings change;
      invalid settings fall back to four-space indentation with a status error.
      The editor now displays a column ruler at the configured
      `preferred_line_length`, using rendered glyph bounds for its monospaced
      column width. Bounded soft-wrap at that column remains unsupported by the
      pinned GPUI Kit editor API. The extension host now returns an explicit
      unsupported-category error instead of silently returning `{}` for
      settings categories outside `language` and `lsp`. Other Zed settings
      categories and rendered acceptance remain open.
      The pinned Zed extension WIT's
      `language_server_additional_workspace_configuration` callback is now
      invoked on other installed adapters when resolving an extension server,
      both at startup and during configuration refresh. Results use Zed's
      recursive object/unique-array/scalar-overwrite merge semantics for both
      initialization options and workspace configuration; a focused host
      regression covers merging, an initially empty target, and explicit AHEAD
      initialization options overriding adapter-contributed values. This
      source change has not been built or run here.
      Rechecked on 2026-10-16 against the current integrated binary: the
      cache-seeded Rust/TypeScript/JavaScript/Python LSP smoke observed live
      create/edit/rename/delete of Ruff and pyproject configuration and changed
      formatting without restarting Ruff. This verifies the native filesystem
      watcher through to a running server, but not the separate extension-host
      `workspace/configuration` refresh path or rendered editor UI.
      Zed's `ExtensionLspAdapter::workspace_configuration`
      ignores `requested_uri`, and the Zed 0.8.0 guest callback accepts a
      worktree but no scope URI; Zed's per-scope lookup in
      `../zed/crates/project/src/lsp_store.rs` is generic adapter behavior.
      AHEAD's extension-provided response should therefore remain identical
      across scopes (covered by a two-scope regression). The host now retains
      one guest per extension/worktree and serializes command and refresh calls
      across proxy Tokio runtimes; the installed Proto and HTML smoke checks
      assert one instantiation after multiple calls. The opt-in real HTML-server
      smoke now flips embedded-CSS validation off and observes the diagnostic
      clear without a server restart, using the same catalog refresh event as
      the workspace settings watcher. An extension update now carries its ID
      through the restart notification and evicts only that package's cached
      Wasm guest sessions before rediscovery; a manual server restart evicts all
      guests. Restarts also advance the resolution generation and clear pending
      built-in/extension lookups. Fresh host and proxy regressions cover scoped
      cache invalidation and
      stale-resolution rejection. On 2026-10-07, a fresh locked/offline build
      compiled the host, proxy, app and RPC library test suites; all extension-
      filtered tests passed (34 passed, 2 ignored), and
      `restart_invalidates_pending_server_resolution` passed separately. The
      complete host suite passed (26 passed, 2 ignored) and the proxy catalog
      suite passed (22 passed, 1 ignored). The full proxy suite has not been
      rerun after the extension-ID notification change; its previous run had
      10 failures at sandbox-denied local socket binds. Validate the
      filesystem-watcher-to-editor path natively; test rapid edits, error
      recovery, and a slow callback
      while editor/LSP requests remain responsive. On 2026-10-07, the opt-in
      `installed_zed_html_extension_refreshes_settings_and_uses_a_real_server`
      test passed against a copied Zed-installed HTML guest and its cached HTML
      server using the proxy test binary newer than `catalog.rs`, isolated
      temporary AHEAD data/user homes and no download or Cargo build.
      On 2026-10-09, the full disposable LSP smoke also fetched the live Zed
      catalog, installed HTML extension 0.3.2 into its isolated AHEAD data home
      and verified HTML completion; no app-profile or global extension state
      was changed.
      Rechecked on 2026-10-08 with the existing host test executable
      (16:04:37, newer than `host.rs` at 16:04:24): 35 tests passed serially;
      two runnable loopback-listener tests and the opt-in real-Zed package test
      were filtered, and the material-theme package test remained ignored.
      A sandboxed attempt to run
      `extension_guest_capabilities_require_user_grants_and_download_async`
      could not bind `127.0.0.1:0`; this was an environment limitation, not a
      test result. On 2026-10-09, the existing source-matched host runner passed
      all active tests with scoped localhost access: 37 passed, 2
      package-dependent tests ignored, both serially and at eight test threads.
      A separate-process parallel invocation once reset the npm mock connection;
      rerunning that test alone and running the full suite in parallel both
      passed, so keep an eye on that isolated flake. These mocks verify host
      callbacks, not live registry access or installation from a real Zed
      catalog. The ignored `installed_zed_extension_reads_changed_workspace_settings`
      host test also passed against the locally installed Zed HTML package and
      its `vscode-html-language-server` ID, using an ephemeral loopback mock.
      It verifies copied-archive installation, digest/version checks,
      discovery, and settings reads; it does not contact the registry, launch
      the server, or verify the native gallery. The source-matched app runner
      (15:18, newer than `extensions_panel.rs` at 05:07) also passed all seven
      gallery tests again on 2026-10-09. These cover mocked catalog query and
      install/reload handoff, not live registry installation or its rendered
      native flow. No Cargo build or external request.
      Separate read-only checks of Zed's live public catalog returned 414
      language-server entries and 32 icon themes. The current compatibility
      scope is language-server packages only; file icon themes remain out of
      scope. Catalog counts verify metadata/query semantics only, not an AHEAD
      gallery install, package execution, or native UI.
      Rechecked on 2026-10-14 with the current `ahead_proxy-72b012fe0cee57a8`
      test binary (11:04, newer than the extension-host and proxy sources):
      the ignored real-server test passed against Zed's installed HTML guest
      and matching cached server, using isolated temporary AHEAD data/user
      homes and no download or Cargo build. It verified workspace settings
      refresh and embedded-CSS diagnostic clearing, HTML completion,
      formatting, and paired-tag rename. Gallery activation, native watcher
      delivery into the editor, and rendered completion insertion/Undo remain
      open.
      The source-current `ahead_extension_host-d3c0f9f2a9ecf8fa` runner
      (10:59, newer than `host.rs`) passed its full suite with loopback-only
      access: 35 passed, 1 ignored. Then the ignored real-package case passed
      against Zed's installed HTML guest with server ID
      `vscode-html-language-server`; its download mock used only 127.0.0.1 and
      its package/workspace were temporary. This separately verifies the
      installer-to-real-guest ABI and settings read, not gallery UI activation.
      The same day, the source-current `ahead_app-e1173e43572818d7` runner
      (10:39, newer than `extensions_panel.rs`) passed all 11 extension-gallery
      tests serially. They cover the install/reload handoff, Zed catalog search,
      compatible/installable filtering, offline installed packages, and partial
      catalog failures. These use fixture catalogs and do not prove a live
      gallery install or rendered native interaction.
      With scoped filesystem-notification access, the same 2026-10-14 proxy
      runner also passed `filtered_nonrecursive_watch_delivers_a_native_file_event`
      and `workspace_settings_change_reaches_catalog_from_native_file_watcher`
      (1/1 each). The default sandbox run timed out waiting for the OS event;
      the permission-scoped rerun observed it in temporary directories. This
      verifies native watcher-to-catalog notification, separately from the
      installed-server refresh smoke; native editor UI propagation is still
      unverified. Follow-up on 2026-10-15: the extension-host resolver now
      applies effective launch settings over the guest command like Zed.
      Initialization options recursively merge over extension defaults, and
      ignored local binary overrides can replace path, arguments and env with
      worktree-relative path resolution. The host suite passes (36 passed,
      3 ignored), the installed HTML host probe passes, and the real HTML server
      proxy smoke now verifies settings-only updates do not restart while an
      initialization-options change does; completion, formatting and paired
      rename pass after restart. The proxy smoke dispatches the refresh event
      directly. The editor now consumes settings-change notifications for
      `tab_size`/`hard_tabs`; rendered behavior remains open. The same day, the shared disposable `just dev`
      `/private/tmp/ahead-search-dev.DTvsEA/project` build rebuilt the app,
      proxy and extension host successfully in 24.78 seconds; macOS then aborted
      app startup before rendering with `com.apple.hiservices-xpcservice`
      (signal 6). The dev watcher remains active; native UI verification is
      still unavailable in this desktop session.
- [ ] Support the Zed extension ABI used by real installed language adapters
      before claiming extension installation works. The host now dispatches
      declared API 0.6/0.7 to Zed's pinned `since_v0.6.0` WIT and API 0.8
      to `since_v0.8.0`; missing/unknown versions are rejected at install and
      discovery. The ignored real-extension smoke packages installed Proto
      and HTML into disposable archives and passes each through AHEAD's
      installer, then reads changed workspace settings offline. The HTML
      0.7 guest built from `../zed/extensions/html` now also passes the
      disposable loopback download-to-install-to-discovery-to-settings smoke
      (2026-10-02); its source manifest needs `[lib].version = '0.7.0'`,
      which Zed's extension builder writes after compiling. Proto also
      resolves its real command and initialization options through a
      disposable local binary override, without network access. An opt-in
      proxy test now detects an HTML file, opens it through the real installed
      HTML guest and language server from a disposable extension copy, and
      observes LSP readiness. It now also requests completion after readiness
      and confirms the server returns the `html` tag for `<di`, returns an
      embedded CSS warning from project settings, clears it after live settings
      refresh without restarting, formats markup, and renames both paired tags;
      this verifies proxy-to-server behavior, not the editor's rendered
      insertion path or native filesystem watcher. On 2026-10-13, a fresh
      integrated-binary smoke also queried the live Zed catalog, installed its
      HTML 0.3.2 extension into an isolated AHEAD data home, and got `div`
      completion for `<di`; the same smoke acquired Vtsls and BasedPyright and
      passed TypeScript/JavaScript/Python LSP checks (see `docs/development/zed-tracking.md`).
      This closes live catalog-to-package-install-to-server coverage, not the
      native gallery install/activation UI. Still test gallery UI activation,
      watcher-to-editor refresh, completion resolve/insertion/Undo in the native
      app, and other real extension packages. The
      Offline recheck on 2026-10-13: the source-matched proxy test binary
      (17:08, newer than `catalog.rs` at 08:49) passed the ignored real-server
      smoke (1/1) against the locally installed Zed HTML guest and its matching
      cached server. Isolated temporary AHEAD data/user homes were used. This
      revalidated workspace settings refresh, embedded-CSS diagnostics and
      clearing, completion, formatting and paired-tag rename without registry
      access; native gallery activation and rendered editor insertion remain
      open. A separate live-catalog/npm-acquisition retry could not resolve
      `api.zed.dev` / npm from this environment (`ENOTFOUND`).
      versioned ABI alone does not prove every adapter is compatible. On
      2026-10-07, the source-matched `ahead-proxy` test binary (newer than
      `catalog.rs` and `lsp.rs`) passed all 248 active tests serially from the
      package root with local mock-loopback access; only this opt-in real-server
      test was ignored. An initial direct invocation from the workspace root
      failed only the relative-path fixture because it expects Cargo's package
      working directory; rerunning from `ahead-proxy/` passed that test and the
      full suite. Re-ran the
      ignored test against the locally installed Zed HTML extension and its
      matching server, under isolated temporary AHEAD data/user homes; it
      passed workspace settings refresh, embedded-CSS diagnostics/clearing,
      completion, formatting and paired-tag rename. This still does not test
      gallery installation, native filesystem-watcher propagation, or rendered
      completion insertion/Undo.
      Recheck at 22:01 ET on 2026-10-07: the existing proxy test binary
      (18:43:21; newer than `catalog.rs` at 18:43:02, `lsp.rs` and extension
      host sources) passed the ignored test again against Zed's locally
      installed HTML extension and matching cached language server. It used
      isolated temporary data/user homes, downloaded nothing, and ran without
      Cargo; the temporary homes were removed afterward.
      Revalidated on 2026-10-08 by directly running
      `ahead_proxy-f12d2fbf5fdfa3b0`, newer than the current catalog and
      extension-host sources. The ignored real-server test again passed
      settings refresh, embedded-CSS diagnostic/clear, completion, formatting
      and paired-tag rename against the installed Zed HTML guest and matching
      cached server. It used isolated homes under the OS temp directory and
      performed no Cargo build or download. Gallery installation, native
      watcher propagation, and rendered completion insertion/Undo remain open.
      The source-matched app test binary built at 15:18 (newer than
      `extensions_panel.rs` at 05:07) passed all seven `extensions_panel::tests`,
      including the Zed catalog query and install/reload handoff. This verifies
      panel/RPC behavior, not a live gallery install or rendered native flow.
      Capability-policy audit (2026-10-08) against Zed's `CapabilityGranter`,
      `ExtensionSettings` and `assets/settings/default.json` found that the
      current host had been deriving npm/download grants from the extension's
      own manifest. The host now reads Zed-style `granted_extension_capabilities`
      from user-level `~/.ahead/settings.toml`, defaults to Zed's grants, and
      requires process commands to be both requested by the extension and
      granted by the user. On 2026-10-08, the source-current locked/offline
      `ahead-extension-host` library suite passed 29 tests (2 ignored), including
      default parity, per-capability user grants, empty-grant denial, malformed
      grant rejection and loading grants from the AHEAD user settings file.
      `just dev` then rebuilt the root app and extension host; macOS aborted
      launch at `com.apple.hiservices-xpcservice` because the desktop is locked.
      The ignored real Zed HTML guest/server smoke passed again from the newer
      proxy test binary with isolated temporary data/user homes and no download.
      Extension-host initialization errors now
      join manifest-discovery failures in the Language Servers > Extension
      issues notification instead of being trace-only. Still verify the real
      Zed HTML guest can install its npm server from a clean isolated PATH and
      exercise empty grants through the guest capability callback. The real-
      server smoke uses a cached server on PATH and does not prove npm
      acquisition.
      Follow-up on 2026-10-08 moved npm version lookup and extension download
      callbacks to async HTTP. The source-matched extension-host test runner
      (16:04:37, newer than every `ahead-extension-host/src` file; latest
      `host.rs` at 16:04:24) passed all 37 active tests, with 2 ignored. Its
      first sandboxed run denied two localhost fixtures; rerunning the same
      suite with access limited to ephemeral `127.0.0.1` mock servers passed
      both. No external requests or credentials were used. The current root
      binary (15:54:28) predates this host change; rebuild through the shared
      `just dev` loop before native activation checks. The AHEAD app still
      times out on CUA attachment, so rendered verification is still open.
      Recheck at 17:41 EDT using the existing host runner (16:04:37, newer than
      `host.rs` at 16:04:24): with the two async-HTTP callback tests excluded,
      35 tests passed and 2 remained ignored. The existing app runner (15:18,
      newer than `extensions_panel.rs` at 05:07) also passed all 7 gallery
      panel tests for Zed catalog search, pinned install requests, server
      reload, offline installed packages and compatibility filtering. This is
      headless coverage, not live gallery installation or rendered activation.
      Rechecked on 2026-10-08 with the existing
      `target/debug/deps/ahead_proxy-028b8cd5478b5060` runner, newer than the
      proxy catalog/LSP and extension-host sources. Its serial proxy suite
      passed 253 tests with 1 ignored (the opt-in installed-Zed HTML server
      smoke); this covers current proxy/configuration and mock-service
      behavior, not native gallery install, watcher propagation or activation.
      Then ran that ignored real-server smoke against Zed's installed HTML
      extension and its cached `vscode-html-language-server`. The guest reached
      the server, refreshed embedded-CSS diagnostics from workspace settings,
      completed `<di`, formatted markup and renamed paired tags. AHEAD data/user
      homes and project were isolated under the OS temp directory; no download
      or provider call occurred. This does not verify gallery download/install
      or the native editor's completion insertion/Undo path. The socket-free
      `extension_process_callback_requires_manifest_and_user_grants` test now
      calls the generated `process:exec` guest callback and asserts both halves
      of Zed's grant rule. It passed against current source on 2026-10-08. That
      focused package run also exposed that the extension host's Tokio
      dependency omitted `macros` required by its own `tokio::select!` calls;
      the feature is now declared directly, so standalone crate tests do not
      depend on workspace feature unification. The sandboxed runner still
      denies the two localhost fixture binds. Rechecked 2026-10-08 18:57 EDT by
      directly running the source-matched host binary with local-only
      `127.0.0.1` access: 37 passed, 2 ignored; no Cargo or external requests.
      A later sandboxed rerun of that same binary (16:04:37, newer than the
      latest extension-host Rust source at 16:04:24) filtered only the two
      ephemeral-localhost HTTP tests; the other 35 active tests passed, with
      the same 2 ignored. No Cargo build or external request occurred.
      The 15:18 app runner (newer than `extensions_panel.rs` at 05:07) also
      passed all seven gallery tests again. These are headless checks; the
      current root app binary predates the host change, so native
      install/activation remains open.
      Revalidated on 2026-10-14 using current proxy sources and Zed's locally
      installed HTML extension plus its matching cached language server. The
      ignored smoke passed (1/1): extension guest startup, workspace settings
      refresh, embedded-CSS diagnostic and clearing, `<di` completion, markup
      formatting, and paired-tag rename. The server was added only to this
      test's PATH; AHEAD data/user homes were isolated under the OS temp root.
      No registry access, downloads, or provider calls. Rebuilt the integrated
      app through the same disposable `just dev` loop afterward (2.85s); macOS
      again aborted before rendering with HIServices XPC signal 6. Watcher
      session 41985 remains live. Native gallery activation and rendered
      completion insertion/Undo remain unverified.
      Also revalidated the integrated binary against locally installed real
      Vtsls, BasedPyright and Rust Analyzer servers using
      `tests/lsp-smoke.mjs` and a disposable multi-language workspace. The
      matrix passed TypeScript and JavaScript completion/definition/diagnostics/
      repair/semantic tokens, TypeScript import quick-fix and auto-import
      resolution, Python completion/definition/diagnostics/repair/semantic
      tokens, Rust formatting and the broader Rust completion/navigation/
      references/rename/diagnostics lifecycle, plus restart, recovery and
      graceful shutdown across all three languages. This exercises the live
      proxy/server path; native popup placement, acceptance rendering and Undo
      remain subject to the macOS app launch failure above. All server binaries
      were already installed locally; no downloads or provider calls occurred.
- [ ] Finish language-server acquisition and discovery in
      `ahead-proxy/src/plugin/catalog.rs`, `ahead-extension-host/src/host.rs`
      and `ahead-app/src/extensions_panel.rs`. Normal builds now include the
      extension host; compatible installed extensions can supply additional
      languages. Installed extensions are discovered at startup and after
      install/update restarts; opening an unsupported language no longer
      synchronously rescans the package directory or waits on the install lock.
      Direct filesystem changes still require Restart Language Servers.
      For Rust workspaces with `rust-toolchain.toml` or
      `rust-toolchain`, built-in Rust Analyzer now asks the selected rustup
      toolchain first and probes the binary before falling back to PATH, the
      AHEAD cache, and managed release acquisition, following Zed's
      `RustLspAdapter::rustup_rust_analyzer_for_worktree` and
      `check_if_user_installed`. A Unix fake-rustup regression covers both
      override-file names. The `rustup which` subprocess now uses the existing
      cancellable, three-second probe lifecycle instead of blocking in
      `Command::output`; a Unix regression exercises cancellation of a stalled
      lookup through the shared kill-and-wait helper. On 2026-10-06, all three
      focused tests passed by direct execution of the existing proxy test
      binary, which is newer than `catalog.rs`; no Cargo build was started.
      On 2026-10-05, the already-running AHEAD Dev app
      showed Python's BasedPyright as Ready and Problems (0) in the disposable
      `ahead-app-readiness` project. This confirms native server health/status,
      not the unverified Rust toolchain-selection path.
      BasedPyright and Vtsls also start from a complete AHEAD-owned cached npm
      package via system Node when their PATH commands are absent; focused
      cache-resolution coverage passes, and BasedPyright rendered Ready in a
      fresh disposable native app with an isolated profile and copied cache.
      The earlier screenshot-specific BasedPyright “No such file or directory”
      failure is resolved: AHEAD canonicalizes the executable before the child
      changes to the worktree, following Zed's resolved-command launch pattern;
      all seven LSP tests pass, and later disposable native runs showed
      BasedPyright Ready. Current-build startup and trusted-workspace behavior
      remain part of this broader verification gate.
      On a cache miss, npm-backed servers now stage an install under AHEAD's
      cache with lifecycle scripts disabled and restart through the dispatcher
      after publication; a fake-npm failure/recovery test passes. In a clean
      disposable app profile, an offline fake npm populated each package from
      Zed's local cache and both BasedPyright and Vtsls rendered Ready after
      automatic restart. A separate clean-profile native run with credential-free
      public npm configuration downloaded both real packages into AHEAD's cache;
      BasedPyright and Vtsls each rendered Ready after automatic restart.
      An AHEAD-owned on-demand Node LTS fallback now serves the built-in npm
      LSP installer and Zed extension host when system Node/npm are unavailable.
      It verifies the official archive checksum, bounds/stages extraction and
      reuses valid cache offline. Five focused runtime tests and the staged
      built-in npm install test pass offline. This environment uses Node
      24.15.0/npm 11.12.1 from PATH, confirming the preferred system-tool path,
      while a 2026-10-08 network-enabled disposable run hid that path from AHEAD,
      fetched the checksum-verified Node LTS runtime, installed Vtsls and
      BasedPyright, and passed the full TypeScript/JavaScript/Python LSP matrix.
      This verifies the clean-PATH runtime download on macOS. Still verify a
      cache-only fallback, Windows launch behavior, and native install progress.
      On 2026-10-07, direct execution of the existing proxy test binary passed
      all nine `plugin::catalog::tests` and seven `plugin::lsp::tests`. That
      binary is newer than `catalog.rs` and `lsp.rs`, but older than
      `ahead-extension-host/src/host.rs`; count this as built-in catalog/LSP
      evidence only, not fresh extension-host or native verification.
      On 2026-10-09, the rebuilt proxy runner passed all nine catalog tests and
      seven LSP tests. A source-current extension-host runner passed all three
      discovery/recovery tests, grammar-only Zed asset installation and
      install-lock serialization. This covers local manifests/cache behavior,
      not live registry downloads or native extension activation.
      Cache acceptance now validates a bounded, regular package manifest with
      the expected name, a nonempty version and a regular entrypoint;
      malformed metadata becomes a cache miss. A per-package file lock
      serializes publication across windows, and a concurrent fake-npm
      regression passes. A rebuilt disposable app reused both real cached
      packages: BasedPyright and Vtsls each rendered Ready after opening Python
      and JavaScript through the UI.
      A focused 2026-10-07 rerun against the 05:10 proxy test binary passed
      complete-cache validation, staged package publication, concurrent
      publication, and active-install shutdown (4 tests). This covers the
      acquisition/cache boundary, not a native editor install or restart.
      The first native run exposed stale status cards after an automatic
      restart. The RPC/app now clear old server statuses and classify Starting
      separately from Error; focused proxy/app status tests pass. A fresh
      disposable rebuild showed only Vtsls Ready after switching from Python
      to JavaScript, and the toolbar agreed with the panel.
      After a cached managed server reports Ready, AHEAD checks npm's configured
      `latest` tag asynchronously and compares it with the cached semver. A
      newer version installs through the existing staged cache publisher, then
      restarts language servers. The running server remains Ready during the
      background refresh; failed updates preserve the current cache and do not
      replace a healthy status with Starting/Error. Offline or failed lookups
      keep the current cache. The lookup is tied to an AHEAD-cached package
      launch, so PATH-selected binaries and installed extension servers do not
      trigger registry requests or cache restarts. The check uses an already
      available npm/Node runtime and does not fetch Node just to check versions.
      Parser, bounded-output and fake-npm regressions were added but not run in
      this pass.
      An explicit Repair cache action now targets only the failed
      AHEAD-managed BasedPyright/Vtsls npm package, leaves PATH-installed
      servers alone, rejects symlinked cache roots, and restarts open documents
      to trigger the normal staged installer. Source regressions cover package
      isolation, cache-root safety and repair routing; they have not been run
      for this change. Before release, verify Windows update/repair behavior and
      continue checking install diagnostics for credential leaks.
      The installer now stops after 120 seconds and shutdown cancels its npm
      child; a fake-npm shutdown regression passes. A rebuilt macOS disposable
      app quit while its fake npm was active: the child exited and no cache
      entry or staging directory remained. Verify this on Windows.
      The versioned ABI and HTML server-start probe do not yet prove full
      adapter operation. Discovery now keeps valid adapters when one
      installed package has a malformed manifest or unsupported ABI and
      reports that package through a separate extension-issues notification.
      Language detection likewise keeps valid definitions alongside malformed
      packages. The Language Servers panel now keeps real server health as
      cards and lists extension-discovery failures separately. A rebuilt
      disposable macOS app showed BasedPyright Ready beside a malformed
      extension warning; hiding the bad package and restarting cleared the
      warning while BasedPyright stayed Ready.
      The old "Managed by ahead-proxy" row was a hard-coded workspace/proxy
      note in the prior panel, not a server status; its identical card styling
      was misleading. It has been removed, with real server health cards left.
      Built-in Python and TypeScript/JavaScript npm setup progress and failures
      now appear as notices in the existing Extensions tab, with a per-server
      Repair cache action on failures. Verify the rendered notices and a real
      failed-install/repair flow in a disposable app. Use AHEAD-owned cache/storage for downloads;
      do not auto-execute a workspace's `node_modules` until AHEAD has a
      project-trust gate. Zed waits for worktree trust before starting local
      language-server binaries. AHEAD now stores a per-project decision outside
      the project in the user config directory; the Language Servers panel can
      grant or revoke it. `PluginCatalog::ensure_lsp_server` refuses to invoke
      an extension or start an npm install until trusted, and
      `LspClient::process` checks again before spawning. The focused core
      trust-store and proxy denied-launch regressions pass, and proxy/app
      library checks pass. The app GPUI test fixture now passes the workspace
      path to the panel; the locked offline app test-target check passes.
      The panel labels its control as language-server permission, since this
      is not yet a full workspace Restricted Mode. `HostWorktree::which`
      rejects path-shaped names and
      uses executable-aware lookup, but still exposes inherited PATH after
      trust. Verify a rebuilt native restricted→trusted→restricted journey in
      a disposable project, including cancellation and persisted reopen.
      A 2026-10-02 `just dev` rebuild for a disposable smoke project linked,
      but the app exited with signal 6 before showing a window. A direct
      `RUST_BACKTRACE=full` rerun produced only a macOS
      `com.apple.hiservices-xpcservice` connection-invalid error and no Rust
      stack; no trust-panel interaction was verified.
      The user-level decision now distinguishes undecided from explicitly
      blocked, and a deferred first-open prompt records Allow & Start or Keep
      Blocked for language servers only. Focused store tests and the app
      test-target check pass; verify the prompt, persistence and duplicate-
      window behavior in a running disposable app before claiming it works.
      The trust store now canonicalizes its config path after rejecting a
      symlinked config directory, so macOS's `/var` parent alias works without
      weakening the leaf symlink check. Three focused core trust tests and the
      opt-in real HTML-extension probe pass from an isolated `/var` data home.
      Add broader Restricted Mode coverage for project settings, MCP
      auto-start and other project-driven processes
      before claiming untrusted-project safety. Windows reparse/ACL handling
      and native Linux behavior remain unverified.
      Successful
      installs already request a dispatcher restart. A focused regression now
      proves the restart sends current unsaved buffer text and revision, while
      the saved file stays unchanged (2026-10-02); verify a real installed
      server receives that text after reload, without blocking the catalog
      thread or duplicating stale document text. Follow Zed's
      `crates/languages/src/{rust,vtsls,python}.rs` and extension store; avoid
      a second LSP transport or per-project command overrides. Verify clean
      first-run setup, cached offline startup, upgrades, TypeScript/JavaScript
      sharing, custom-language mappings and a real installed extension in
      disposable projects. On 2026-10-04, current-source proxy catalog tests
      passed 17 with 1 opt-in real HTML-server test ignored, including delayed
      resolution replaying the latest unsaved buffer and closed-document stale
      resolution rejection. The first test build after `cargo clean` took
      6m23s; the cached catalog rerun took 3.74s. This is proxy evidence, not
      rendered GPUI or authenticated-provider verification. Do not mark the
      normal-build wiring as live proof.
      The raw URL/ID form has been replaced by a live searchable
      gallery with candidate-version filtering, All/Installed/Not Installed
      filters, and Install/Update/Installed states. Extensions now opens as a
      standalone center tab from the Command Palette and focuses search, like
      Zed's `ExtensionsPage`. Cmd/Ctrl+Shift+X now opens that same tab directly.
      First open starts a live catalog refresh, as Zed's page does; installed
      extensions remain listed if the fetch fails or the machine is offline.
      Catalog/install state, rendering and regressions
      now live in `ahead-app/src/extensions_panel.rs`; Settings does not own a
      second gallery. Search now sends Zed's `filter` query after a 250 ms
      debounce (clearing the query reloads immediately), while a request
      revision ignores stale responses and server results are not hidden by
      AHEAD's local substring filter. The catalog URL/encoding regression and
      all 174 app library tests pass. The focused GPUI open/close
      and search-focus test, all 173 app library tests, and `cargo check -p ahead`
      pass. AHEAD's offline catalog
      parser test passes, and a read-only live metadata probe on 2026-10-02
      returned 412 language-server entries, 307 matching AHEAD's declared
      Wasm API versions. This is an ABI candidate
      count, not proof that every server starts in AHEAD. A later rebuilt
      native run in `/private/tmp/ahead-extension-gallery-A0S6kW` opened the
      Extensions center tab with Cmd+Shift+X, loaded 308 live candidates,
      and narrowed the visible list to two by searching `HTML`. It did not
      install a package. The `just dev` launch first exited with signal 6
      after a macOS connection error; opening the rebuilt bundle by path
      worked. The copied dev bundle now uses `io.ahead.dev` (`AHEAD Dev`)
      so native checks can distinguish it from an installed AHEAD app.
      A sandboxed direct launch still aborted in macOS application registration;
      both `open -W -n` and an elevated one-off direct launch kept test
      processes alive but exposed no window to the native UI tool. Its app
      inventory omitted windows for other apps too, so investigate the
      launch/tool boundary and rerun the
      gallery-to-install smoke before claiming native install success.
      A separate elevated `just dev` run on 2026-10-02 opened a disposable
      project and a targeted window capture showed the Extensions tab already
      populated with live Zed catalog rows on startup. The UI tool listed
      AHEAD Dev but could not attach to its window, so this did not test an
      Install click or installed-server startup.
      A GPUI test now covers gallery selection, its pinned URL request to the
      proxy, successful install state and restart notification. Zed's pinned
      `crates/extension_host/src/extension_host.rs` queries `/extensions` with
      `max_schema_version=1` and `provides=language-servers`, then downloads a
      selected version; `crates/extensions_ui/src/extensions_ui.rs` owns the
      search, loading and error states. AHEAD must filter to its supported
      0.6/0.7/0.8 Wasm APIs and packages with a language-server declaration,
      preserve installed packages when offline, and test registry data before
      production release. Package download/install has an offline real-HTML
      loopback smoke. A native catalog-selected HTML install reaches disk and,
      after restart with a cached server binary on PATH, reports its language
      server Ready in the disposable project.
      The installer now rejects packages without language-server declarations
      and bounds total unpacked bytes and archive entries; its focused host
      regressions pass. Its root-wide file lock serializes separate proxy
      processes, and a stranded valid `.previous` package is restored on
      discovery or retry. Malformed backups report an error without hiding
      healthy extensions; focused lock/recovery tests and the real-HTML
      loopback smoke pass. Discovery skips hidden staging directories and
      rejects manifest/directory ID mismatches in both server and language
      scans. File language lookup remains lock-free while packages unpack;
      a focused regression covers lookup under the install lock. After install,
      the proxy client requests a language-server restart
      independent of the Settings view; that path reclassifies open buffers and
      rescans installed extensions before reopening documents. A
      catalog-selected HTML package installs in a disposable app profile. With
      the `.html` file already open and the executable on PATH, its server
      became Ready in the Language Servers panel without restarting the app.
      Offline installed-package listing now reads compatible local manifests and
      merges installed versions with fetched entries; a disposable regression
      covers offline visibility and online update state. The gallery now shows
      Installing and Retry row states. The gallery compares semantic versions like Zed's
      updater and refuses a catalog-triggered downgrade, including a stale
      Retry action; the parser and GPUI install regressions pass. The maintained
      `just test-all` recipe passes after the guard on 2026-10-02.
      In the uniquely identified native app on 2026-10-02, the Extensions tab
      rendered the online Zed gallery with 338 candidates; search narrowed
      `html` to HTML and RsHtml. Clicking HTML Install completed and showed a
      language-server reload notice. The
      install exposed that `AHEAD_HOME` isolates only the managed agent, not
      app-level plugins: it wrote to the ordinary debug profile. The newly
      created HTML directory was moved intact to the disposable recovery
      folder; the pre-existing icon theme was untouched. `AHEAD_DATA_HOME`
      now redirects app config/data/plugins/cache/logs for native runs. A rebuilt
      app installed HTML under that disposable data home without touching the
      normal debug profile. Its row originally lingered in Installing after
      the package appeared on disk; Refresh resolved it to Installed. A second
      native run kept an HTML file open during installation: its server became
      Ready within seconds, but the row stayed on Installing for over 20
      seconds. The gallery now awaits the proxy result through the app's
      existing async-channel pattern. In a rebuilt native retest, the row
      changed to Installed without Refresh and the already-open file's HTML
      server became Ready without restarting the app.
      After restart, the gallery-installed HTML
      server reported Ready for a disposable `.html` file with Zed's cached
      binary on PATH. The editor still labeled that file `text` because
      gpui-kit's highlighter registry lacks HTML; add compatible extension
      grammar/highlighting support separately from LSP activation. The same
      panel reported BasedPyright missing in this isolated PATH. Shared LSP
      spawn errors now name the failed executable, and the panel wraps the
      full error; a disposable native pass confirmed both. The stale advice
      to install a matching extension in Settings was removed.
      Recheck: the same offline smoke also passed when pointed directly at the
      installed Rust Analyzer 1.95 binary. The default `stable` rustup shim
      reports that Rust Analyzer is absent, but this does not block the actual
      proxy smoke. Native editor insertion/trust UI and rendered verification
      remain open.
      On 2026-10-09, `AHEAD_LSP_INSTALL_MISSING=1 node tests/lsp-smoke.mjs
      target/debug/ahead` passed with managed acquisition into its isolated
      profile for Rust, TypeScript, JavaScript and Python. It covered
      completion, definitions, diagnostics/repair, semantic tokens, close and
      reopen, TypeScript import actions and completion resolve, shared TS/JS
      server identity, Rust formatting/hover/signature help/references/rename,
      server restart and graceful shutdown. The retained disposable project is
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-x0FyNM`.
      A second isolated run fetched the live Zed catalog/archive, installed HTML
      extension 0.3.2 and passed real HTML completion alongside the
      TypeScript/JavaScript/Python and restart/shutdown checks; its project is
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-VYp4L2`.
      Both use the real proxy, make no model-provider calls and shut down their
      language-server processes. The separately ignored
      `installed_zed_html_extension_refreshes_settings_and_uses_a_real_server`
      test also passed directly against the current proxy test runner with
      Zed's already-installed HTML extension and matching server, isolated
      homes and a credential-free environment. It covered settings refresh,
      diagnostics, completion, formatting and rename. The server's dynamic
      `workspace/didChangeWorkspaceFolders` registration remains unsupported;
      native Extensions-tab interaction, fresh rendered editor behavior and
      production authorization for the live Zed catalog remain open.
      Revalidated on 2026-10-14 against the integrated binary built at
      10:55:54, newer than the current extension-host, catalog, LSP and panel
      sources. In a credential-free disposable environment, managed npm
      acquisition plus the live Zed catalog installed HTML extension 0.3.2;
      HTML completion passed. Vtsls and BasedPyright acquisition then passed
      TypeScript, JavaScript and Python completion/navigation/diagnostics and
      repair; TypeScript import quick-fix, advertised command and auto-import
      resolve also passed. Closing/reopening documents, server restart/recovery,
      Ready status and graceful proxy/process-group shutdown all passed. This is
      real catalog/npm and proxy/server acceptance, not native panel rendering
      or Tree-sitter grammar activation. The inspectable disposable project is
      `/private/tmp/ahead-lsp-smoke-7qTnQc`.
      Fresh network-enabled retry on 2026-10-15 against the integrated binary
      passed the same managed path from an empty isolated AHEAD data home. The
      live catalog/package API installed HTML 0.3.2; AHEAD acquired Vtsls,
      BasedPyright and Ruff, then passed expanded TypeScript/JavaScript/Python
      checks, including cross-file unsaved-buffer rename, typed signatures,
      Python analyzer/Ruff code actions and formatting, live Ruff configuration
      reload, restart/replay and process-group cleanup. No credentials or model
      calls were used. Fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-fpLfI9`.
      Native Extensions-tab and editor-popup rendering remain unverified.
- [ ] Finish production hardening of the searchable Extensions tab. Per the
      user's 2026-10-14 decision, release builds should query Zed's live catalog
      and package API, with an explicit compatibility/non-endorsement disclaimer;
      AHEAD's reviewed, hashed feed remains supplementary/fallback. Filter to
      capabilities AHEAD can host and label API-version matches as candidates
      until startup is proven. The published [Zed Terms of Service](https://zed.dev/terms)
      require service access through Zed-designated mechanisms and restrict
      seeking non-public APIs, but do not establish whether these routes are
      authorized for third-party editors. A compatibility disclaimer does not
      grant permission; resolve endpoint-specific terms before broad commercial
      distribution. Preserve the HTTPS/artifact trust boundary and separately
      review extension licenses and external language-server binaries.
      The gallery now offers **Uninstall** for an installed language extension.
      Removal runs through the proxy/catalog boundary: active servers are stopped,
      the target and any validated interrupted-update backup are removed under
      the shared install lock, installed grammar-cache entries are evicted, and
      the remaining extensions are rediscovered before open buffers are replayed.
      Host regressions cover removal isolation, backup cleanup and a mismatched
      manifest ID; run them through the shared dev loop when convenient.
      Rechecked on 2026-10-16 from the current app test runner: all 12
      `extensions_panel::tests` pass, covering filtering, live-catalog
      precedence/fallback, install requests and language-server reload routing.
      These headless tests do not issue a live catalog request, publish a
      package or verify the native install journey.
      Rechecked against Zed's live Terms of Service on 2026-10-14 (page last
      updated 2026-03-02): sections 2.2 and 2.4 retain the designated-access
      mechanism and non-public-API restrictions, but do not name these extension
      catalog/download routes. Their use by AHEAD remains unconfirmed; the
      user's decision to use the API in releases is recorded in decision 0007,
      but does not resolve this service-terms risk.
      On 2026-10-14, corrected source precedence: live Zed results win duplicate
      IDs; hashed AHEAD entries supplement missing results and remain available
      when the live catalog is unavailable. The source-current Extensions panel
      suite passed 12/12, including duplicate-ID and fallback cases. This does
      not resolve the endpoint-terms question or verify a published release.
      At 21:23 EDT, the 20:09 app runner (newer than `extensions_panel.rs` at
      19:49) again passed all 12 `extensions_panel::tests`. The tested source
      includes the visible independence/non-endorsement and compatibility
      disclosure; native presentation remains unverified while the Mac is
      locked, and the endpoint-terms question remains open.
      Review extension-code licenses and separately assess external language
      servers and other downloaded tools: Zed's [license requirements](https://zed.dev/docs/extensions/publishing/license-requirements)
      explicitly exclude those tools from the extension-code license rule.
      The shared installer now requires credential-free HTTPS and refuses
      downgrade redirects in production; its disposable loopback package test
      remains test-only. The install request now carries an optional archive
      SHA-256 and selected package version; the host checks both before
      replacing an installed extension. The digest, version and RPC regressions
      pass. Zed's preview catalog does not supply the archive digest.
      On 2026-10-12, release builds began querying Zed's live catalog as well
      as the AHEAD-owned feed. Debug builds read the checkout copy of
      `extensions/registry.json`; release builds read its published URL. Zed
      rows are labeled as compatibility candidates with an explicit
      non-endorsement disclaimer. AHEAD entries require a valid SHA-256 and a
      version-named GitHub Release asset from this repository; they supplement
      missing Zed IDs, while the live Zed entry wins duplicates. The checked-in
      feed is empty, so it contributes no AHEAD-owned packages today, but
      release catalog discovery remains available from Zed's live API. Added a
      manual-only publisher at
      `scripts/package-ahead-extensions.mjs` and
      `.github/workflows/extensions-publish.yml`: it pins the source checkout
      and Zed CLI, and each package can independently pin a source repository,
      revision and selected submodule path. It rejects mismatched remotes,
      revisions and dirty source, validates package/license metadata, and
      publishes versioned archives plus their SHA-256 into the AHEAD feed.
      Four Node tests pass, including a pinned catalog-submodule fixture that
      rejects dirty nested source without changing the published feed; the
      workflow has not been run and no asset is
      published. Gallery rows now retain and show declared runtime
      dependencies and host capabilities. All ten current-source
      `extensions_panel::tests` passed, including parsing and summarizing the
      HTML extension's runtime and host requirements. This does not verify
      publication or the rendered install journey.
      Rechecked 2026-10-13 with Node 24.15.0:
      `node --test scripts/package-ahead-extensions.test.mjs` passes 4/4,
      including pinned submodule and tamper rejection. No package was built or
      published; the checked-in feed remains empty.
      Revalidated on 2026-10-15: the publisher suite still passes 4/4. This
      does not publish an asset or run the release workflow; no AHEAD package
      has been built or added to the feed.
      Re-ran `rtk node scripts/package-ahead-extensions.test.mjs` at 23:53 EDT
      on 2026-10-15 in the current checkout; all four tests pass again. This
      remains fixture-only evidence and does not build/publish a package or run
      the manual GitHub workflow.
      Revalidated on 2026-10-20 from the current checkout: all four publisher
      tests pass, including pinned-source, license/metadata and dirty-submodule
      rejection. No package was published and the release workflow was not run.
      Source recheck on 2026-10-15 supersedes the 2026-10-12 release-source
      description above: both debug and release builds query Zed's live
      catalog and the AHEAD feed. Zed entries win duplicate IDs; AHEAD entries
      supplement missing IDs, and remain available when Zed's catalog fails.
      The live-catalog compatibility/non-endorsement disclosure is present in
      the gallery and user guide. An empty AHEAD feed means no AHEAD-published
      additions yet, not that release discovery is empty. The endpoint-terms
      review and published AHEAD packages remain open.
      Restarted the shared `just dev` loop against
      `/private/tmp/ahead-broad-use.0XxogT/project` on 2026-10-12. The current
      source build completed in 15.94s, then macOS app launch aborted with
      `com.apple.hiservices-xpcservice` connection errors (signal 6). After
      the runtime metadata UI edit, the same loop rebuilt successfully again
      (26.66s) and hit the same native launch abort. A second `just dev` was
      correctly refused by the existing shared build lock; the original loop
      remains active, but native gallery behavior is still unverified.
      On 2026-10-12, a user-requested cold restart after `cargo clean` rebuilt
      `ahead` and the Code Mode host successfully in 10m25s, but the app exited
      at launch with `com.apple.hiservices-xpcservice` connection-invalid
      errors (signal 6). Relaunching against the disposable
      `/private/tmp/ahead-broad-use.0XxogT/project` reused the build in 15.67s
      and hit the same abort. Ad-hoc signing made `codesign --verify` pass but
      did not help: LaunchServices still returned `kLSNoExecutableErr`, and
      direct execution of the signed app again aborted with the same HIServices
      error. The prior 2026-10-08 crash report's main-thread stack ends in
      macOS `_RegisterApplication`/HIServices, not an AHEAD Rust panic. The
      desktop API also timed out on the app. The watcher was stopped after the
      child failure. Native rendered gallery/install verification remains open.
      On 2026-10-06, the existing app test binary passed four focused gallery
      regressions: Zed's filtered `/extensions` query, icon-only entries,
      installable API/digest filtering (including absent-digest acceptance and
      malformed-digest rejection), and offline installed-package merging.
      These are catalog/parser checks, not external download or native install
      verification. Two additional direct app-binary regressions passed for
      gallery package/version/digest request dispatch, language-server reload,
      and reload notification after a successful proxy install. They use mocked
      RPC and do not verify a live catalog download or native activation.
      On 2026-10-07, all seven `extensions_panel::tests` passed directly from
      the existing app test binary, which is newer than
      `ahead-app/src/extensions_panel.rs`; reran the same seven at 20:17 local
      without Cargo. This includes Zed query construction, offline installed
      entries, compatibility filtering, and mocked install/reload dispatch. It
      does not rebuild the newer
      `ahead-extension-host/src/host.rs` or prove current native activation; the
      Mac remains locked.
      Rechecked on 2026-10-07 without Cargo against source-newer runners:
      `ahead_proxy-f12d2fbf5fdfa3b0` (18:43:21, newer than catalog source at
      18:43:02) passed 25 catalog/LSP lifecycle tests (1 real-server test
      ignored until its extension and server are explicitly configured), and
      `ahead_extension_host-35cb811b7092e2ec` (13:03:57, newer than `host.rs`
      at 12:48:19) passed 26 tests (2 package-specific tests ignored). The proxy
      cases cover safe package-cache publication, cancellation, stale
      resolution, diagnostics, and server lifecycle; host cases cover
      HTTPS/digest/size checks, atomic install recovery, extension capabilities
      and settings.
      These remain test-binary evidence; the real installed-extension server
      fixture is still ignored without its explicit package and server setup,
      and native gallery activation remains unverified while the desktop is
      locked.
      Rechecked on 2026-10-08 at 15:18 EDT: all seven
      `extensions_panel::tests` passed directly from
      `target/debug/deps/ahead_app-5b00065cae9bc61c` (built 15:18, newer than
      `extensions_panel.rs` at 2026-10-07 05:07). This reconfirms the catalog
      query, compatible package filtering, offline entries, and mocked
      install/reload path. It does not validate the newer extension-host
      source, a live catalog or download, or native activation.
      On 2026-10-07, the ignored real-package test first exposed an incomplete
      archive fixture that omitted declared `grammars/proto.wasm`; the fixture
      now includes each grammar wasm named by the manifest. Re-ran it against
      the locally installed Zed Proto package with the source-current
      extension-host test binary. Its loopback-only download, manifest/hash/
      grammar validation, install/discovery, Wasm command and initialization
      callbacks, and repeated changed-settings reads all pass. No registry,
      provider, or Zed installation was modified. Native gallery activation and
      watcher-to-editor refresh remain open.
      Read-only live-catalog check on 2026-10-08 queried the exact Zed
      `/extensions` endpoint with AHEAD's `max_schema_version=1` and each
      `provides` filter. Both requests returned HTTP 200 well under AHEAD's
      4 MiB response cap: 414 language-server entries (309 match AHEAD's
      schema/Wasm API gates before the remaining parser checks) and 32 icon-
      theme entries. This verifies the live preview feed's current shape, not
      download/install compatibility or permission for production use.
      Added opt-in `AHEAD_LSP_INSTALL_ZED_HTML=1` coverage to
      `tests/lsp-smoke.mjs`. On 2026-10-08, the network-enabled run fetched the
      live HTML 0.3.2 entry, installed its package through AHEAD's proxy into
      the disposable profile, started `vscode-html-language-server`, and
      resolved `div` completion for `<di`; the HTML server stayed Ready after
      restart. The same run passed the TypeScript/JavaScript/Python matrix.
      Its temp fixture was removed; no global packages or Cargo build were
      used. This exercises the real catalog/download/host path, not the
      rendered Extensions tab or production authorization.
      Revalidated on 2026-10-12 against `target/debug/ahead` (built
      2026-10-11 16:21) with the isolated cache seed and live Zed preview. AHEAD
      fetched and installed HTML 0.3.2 in a fresh temp profile, started its real
      HTML language server, and returned `div` completion for `<di`. The same
      run passed TypeScript/JavaScript completion, quick-fix/auto-import,
      diagnostics, close/reopen, server restart and graceful shutdown. Fixture:
      `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-3G4GBJ`.
      This validates the direct Zed development-preview path only; it does not
      authorize production use or verify the rendered Extensions gallery.
      Keep explicit Install/Update/Retry and offline installed state. AHEAD's
      `Installing…` action state follows Zed's `ExtensionStatus::Installing`;
      verify in the native disposable project that a gallery install activates
      its language server without restarting the app. Do not mirror Zed-hosted
      binaries or label an API-version match as verified AHEAD compatibility
      before a real server-start check. Expand
      past language-server extensions only after the corresponding host path
      works.
      On 2026-10-13, partial catalog failures now report the number of
      available candidates instead of claiming the entire registry is
      unavailable; an empty failed catalog still reports the unavailable state.
      The source-current gallery suite passes 12/12. Zed's `ExtensionsPage`
      likewise distinguishes fetch failure from loaded results. The live
      Per the user's 2026-10-20 product decision, releases use the live Zed
      catalog with the existing compatibility and non-endorsement disclaimer;
      do not treat an API-version match as verified AHEAD compatibility. The
      AHEAD-owned registry remains empty, and rendered gallery installation
      still needs acceptance.
      The change rebuilt with the same `just dev` disposable-project loop in
      14.64s; macOS again aborted the app child at HIServices initialization
      (signal 6) before rendering. Watcher session 96223 remains live; this UI
      status change is covered by headless tests only.
- [ ] Port Zed's project search panel with search/replace, keyboard result
      navigation, match highlighting,
      include/exclude globs and paginated results. Clicked matches open at the
      start and select the full range, including cross-line matches; the Search
      tab remains beside code tabs so results can be revisited without rerunning
      the query. Verify range selection and tab navigation in rendered GPUI.
      On 2026-10-16, added a `Replace Match` action beside `Replace All`; it
      applies the selected result only and prefers dirty-buffer text. Focused
      core tests pass for an unsaved UTF-8 match and regex capture/escape
      expansion; the GPUI regression confirms the outgoing edit changes only
      that occurrence and leaves disk unchanged. The integrated app rebuilds
      through the guarded dev loop, but native search-panel rendering remains
      unverified.
      After `Replace Match` succeeds, the panel now reruns against the updated
      unsaved buffer and selects the next occurrence after the replaced span,
      wrapping to the first result when needed. Changing the query/options or
      manually navigating cancels that pending advance. A source regression
      covers a multiline replacement and wrap selection; it has not been run.
      Rechecked on 2026-10-16 with the source-current 09:45 app test runner:
      all three `workspace_panels::tests::search_replace` tests passed,
      including selected-match-only replacement, replace-all and collapsible
      controls. Native search-panel rendering remains open.
      On 2026-10-19, native Replace Match exposed a missing-window error when
      the target was not already open. The shell now applies the edit through
      the active window, and a GPUI regression verifies opening that target,
      changing only its unsaved editor buffer, and leaving disk unchanged. The
      focused shell and search-panel test runs pass (4 tests); the live app
      replaced one match and the reverse replacement restored the disposable
      fixture. Native search query/edit behavior is now verified for this path.
      Search results now have a visible selected row, cyclic Up/Down keyboard
      navigation, and Enter opens the selected full match range. The focused
      navigation regression passed with
      `rtk cargo test --locked --offline -p ahead-app --lib
      search_result_navigation_wraps -j 1`; rendered key propagation and list
      styling remain unverified. Search matches now include their exact
      UTF-8 byte range within the bounded preview; result rows render that
      range in the accent color and bold. Regressions cover clipped Unicode
      previews, multiline matches, and safe rendering of invalid ranges. The
      focused core suite passed (28 tests), the app preview test passed (1),
      and the managed-agent paging test passed (1). The GPUI range-selection
      regression now follows gpui-kit's end-caret selection behavior; all 73
      `ahead-app --lib` tests pass (2026-09-29). Verify the rendered result row.
      The existing `ahead_core` test binary passed all 36 `search::tests`
      directly on 2026-10-06 and again on 2026-10-07; the latter binary is newer
      than `ahead-core/src/search.rs`. Coverage includes path filtering,
      gitignore behavior, symlink boundaries, stale snapshots, Unicode/multiline
      ranges, cancellation, unsaved buffers, streaming, and global result
      limits. These runs did not start another Cargo loop and do not close
      rendered-panel checks.
      The shared matcher now returns every match up to each caller's limit and
      searches eligible dirty open `CodePanel` tabs and proxy-owned buffers.
      The panel now makes Text, Word and Regex mutually exclusive match modes;
      case sensitivity is a separate modifier. The three modes now render
      inside one bordered group with Match case separated as an independent
      option, making the selection model explicit. On 2026-10-20, the replace
      field, Replace Match and Replace All actions moved into one Zed-style
      toolbar row; a GPUI geometry regression now checks their alignment. It
      has not been run in this source-only pass because only 2.3 GiB is free
      and no shared dev loop is active. Its comma-separated
      include/exclude path-pattern fields and the native agent's include_pattern
      and exclude_pattern arguments both use the shared worktree path filter.
      A 2026-10-20 call-site audit confirms the editor streams through
      `ahead_core::search::search_paths_with_overrides_stream` and managed
      `file_search` calls the same engine's `search_paths_with_overrides` API.
      Zed's `ProjectSearch` likewise combines worktree paths and loaded buffers
      under one query/result handle. Retained Codex `ToolSearchHandler` only
      ranks deferred tool definitions; it is not a second workspace scanner.
      Agent pagination cursors now bind both path filters, and managed-search
      regressions cover excluded saved files, schema exposure and filter-change
      cursor invalidation (source added 2026-10-20; not run). The GPUI regression
      for exclusive Text/Word/Regex mode switching and independent case
      sensitivity passed again against current source on 2026-10-06. On
      2026-10-07, the
      source-current `ahead-app --lib search_` run passed all 15 matching search
      and GPUI panel tests; the source-current `ahead-core --lib search::tests`
      run passed all 36 scanner/replacement tests. This still does not verify
      native rendered panel behavior.
      On 2026-10-12, the 11:38 `target/debug/ahead` binary passed the
      10,000-file scale smoke with four search workers: no-hit scans took
      1.15s/680ms cold/warm, and returning the 5,000-result cap took 469ms.
      Zed's `project_search.rs` uses a shared background executor with one
      worker per available CPU minus one; on this 14-way host AHEAD's four
      worker cap was limiting. AHEAD now uses the same CPU-minus-one count
      (13 workers). After the change, the same smoke measured 636ms/532ms for
      no-hit scans and 239ms to return the capped 5,000 results. Its ripgrep
      comparison was 432ms/333ms for no-hit and 508ms to count all 10,000
      matches. These are single sequential runs with OS-cache variation, and
      the capped-result and full-count timings are not equivalent. The change
      improves the AHEAD scan in this fixture. At that time, AHEAD still
      created per-query OS threads instead of using a shared executor; the
      bounded process-wide pool and current profile are recorded below.
      Revalidated against the rebuilt 2026-10-12 dev-loop binary with the
      disposable 10,000-file smoke and optional ripgrep comparison: index build
      6.49s, cold/warm no-hit scans 830ms/441ms, and 327ms to return the capped
      5,000 results; ripgrep measured 410ms/429ms for no-hit and 496ms to count
      all 10,000 matches. This is another single sequential run, not a stable
      benchmark; it confirms the safety/result-limit path and leaves concurrent
      search contention profiling open.
      On 2026-10-13, the rebuilt dev binary passed the same disposable
      10,000-file smoke twice: no-ripgrep run indexed in 4.91s, with 515ms/374ms
      cold/warm no-hit scans and 278ms to return the 5,000-result cap; a
      separately OS-cache-warm run indexed in 272ms, with 375ms/376ms no-hit
      scans versus ripgrep at 400ms/401ms and 226ms to return the cap versus
      450ms for ripgrep to count all 10,000 matches. These are sequential
      single-host runs and the capped-result/full-count timings differ in work;
      they do not answer the concurrent-search contention question.
      Re-ran the current `just dev` binary with same-process ripgrep comparison:
      indexing 10,000 files took 5.08s, AHEAD no-hit scans took 468/490ms
      versus ripgrep's 403/369ms, and AHEAD returned the 5,000-match cap in
      278ms versus ripgrep's 473ms to count all 10,000. This is one sequential
      measurement with different capped/full-count work; keep cold indexing and
      no-hit latency as optimization candidates, not a claimed win over
      ripgrep.
      On 2026-10-13, a fresh proxy-only run against the rebuilt
      `target/debug/ahead` indexed the same 10,000-file fixture in 1.19s; cold
      and warm no-hit scans took 525ms/502ms, and returning the 5,000-result
      cap took 236ms. Ripgrep measured 357ms/348ms for no-hit scans and 447ms
      to count all 10,000 matches. This is one sequential host run; indexing
      and scan times vary with OS cache, and capped-result/full-count timings
      do different work. The temporary workspace/profile were removed. Native
      large-result rendering and concurrent-search contention remain open.
      A fresh 2026-10-13 run with the integrated binary indexed the same
      10,000-file fixture in 544ms; cold/warm no-hit scans took 391/380ms,
      and returning the capped 5,000 matches took 226ms. Ripgrep measured
      367/408ms for no-hit scans and 441ms to count all 10,000 matches. This
      is one cache-sensitive sequential sample, and capped results differ
      from a full count; it is not a general speedup claim. The smoke removed
      its temporary workspace and profile after success.
      The cached cold index now uses the pinned `ignore` crate's parallel
      walker, then sorts the completed path snapshot once; the existing lazy
      iterator remains unchanged for interactive/cancellable scans. Current
      `ahead-core` search tests pass (40/40), and the integrated app compiled
      through `just dev` before macOS again aborted at HIServices initialization
      prior to rendering. The synthetic 10,000-small-file profile measured
      the exact old serial index walk against the parallel cached index over
      five alternating samples: median 71ms serial versus 55ms parallel, with
      identical ordered file paths and visible-entry collection. These small
      files were OS-cache-warm, and this is one synthetic host run; it supports
      the direction but does not establish broad-workspace startup improvement.
      The earlier 56ms-versus-82ms comparison against lazy file listing was
      unequal work and is superseded. Re-run the broader disposable workspace
      smoke before claiming a general indexing win.
      A same-process `ahead-core/examples/search_profile.rs` profile then ran
      three full-tree no-hit batches at each concurrency level. Median batch
      times were 429ms for 1 scan, 841ms for 2, 1,394ms for 4 and 2,715ms for
      8; at that point each request fanned out to 13 scoped OS threads on this
      host.
      Throughput rose from about 2.3 to 3 scans/sec rather than collapsing, but
      concurrent scans increased batch latency and could create up to 104
      scanner threads. This warm-cache profile motivated the process-wide pool
      follow-up recorded at the end of this item; verify its trade-off under
      real editor/agent load.
      Rechecked on 2026-10-13 with the source-newer `ahead-core` runner and
      `target/debug/examples/search_profile` (both built at 12:49, newer than
      `ahead-core/src/search.rs` at 12:36): the shared-pool identity test passed.
      On 10,000 synthetic files, concurrent no-hit query batches at 1/2/4/8
      queries measured median 564/824/1,444/2,790ms. Throughput levels off near
      2.9 queries/sec as batches grow, while per-batch latency increases. This
      is a warm-cache synthetic profile, not real editor/agent load. Compared
      with Zed's shared background search pipeline in
      `../zed/crates/project/src/project_search.rs`, AHEAD now uses one bounded
      process-wide Rayon pool, keeps open-buffer overrides, and streams file
      results through its existing Search panel service. Native result-list
      rendering and actual multi-session contention remain open.
      Rechecked on 2026-10-14 by rebuilding and running the same profile
      offline: 10,000 files produced 10 expected hits; serial/parallel index
      medians were 78/64ms and the ordinary hit scan median was 699ms. Median
      concurrent no-hit batches were 911/1,397/1,557/3,139ms at 1/2/4/8
      simultaneous scans. This three-sample synthetic run again shows rising
      batch latency with concurrency, but no throughput collapse; it is not a
      representative editor/agent load result.
      The current rebuilt app binary also passed
      `tests/workspace-search-scale-smoke.mjs` with its ripgrep baseline: the
      10,000-file index took 573ms; AHEAD no-hit scans took 461/467ms versus
      ripgrep's 477/464ms, and AHEAD returned its 5,000-result cap in 262ms
      versus 557ms for ripgrep to count all 10,000 matches. This is one
      cache-sensitive run, and capped results versus a full count are different
      work; it is diagnostic, not a speedup claim.
      Rechecked 2026-10-08 18:56 EDT from the existing `ahead_app` test runner
      built at 15:18, newer than `workspace_panels.rs` (2026-10-07 18:13): all
      eight `workspace_panels::tests::search_` tests passed serially without
      Cargo. Native search-panel appearance and focus remain unverified.
      Rechecked on 2026-10-13 from the app test runner rebuilt for the narrow
      Agent-panel regression: all eight search-panel tests passed, covering
      mutually exclusive match modes, replace preview/application to unsaved
      buffers, keyboard result navigation, pagination and virtual rows. This
      remains headless GPUI evidence; rendered search focus/appearance is open.
      Rechecked the current binaries on 2026-10-13: all 40 `ahead-core`
      `search::tests` passed from the 12:49 runner, newer than
      `ahead-core/src/search.rs` at 12:36; all 15 app `search_` tests passed
      from the 12:57 runner, newer than the relevant editor/panel sources.
      These cover scanner ordering/cancellation/limits, Replace All safety,
      result ranges, panel mode exclusivity, navigation, pagination and virtual
      rows. This is source-matched headless coverage, not rendered acceptance.
      Added `concurrent_workspace_searches_complete_from_shared_pool` on
      2026-10-13: eight independent core searches over 256 files each all
      returned their full result sets, and all 41 core search tests passed.
      An attempted eight-way proxy `global_search` probe instead confirmed its
      process-wide latest-query cancellation, so that UI endpoint is not a valid
      harness for concurrent independent agent searches. Actual editor/agent
      contention profiling remains open.
      The current NativeClient regression also ran two `file_search` calls for
      separate ACP session IDs concurrently; each received an independent
      editor snapshot request and returned both matching files after scanning
      the 256-file fixture. The focused test passed (1/1). This covers the real
      managed tool boundary without a model provider, not provider-generated
      parallel calls or mixed editor/agent performance.
      Re-ran the 10,000-file smoke on 2026-10-14 against the current integrated
      binary with same-process ripgrep comparison: workspace index 4.888s,
      AHEAD no-hit scans 430/393ms and 254ms to return the 5,000-result cap;
      ripgrep no-hit scans 414/422ms and 451ms to count all 10,000 matches.
      This is one cache-sensitive sequential sample, and capped-result versus
      full-count timings do different work; it is not a broad speedup claim.
      The successful smoke removed its disposable workspace and profile.
      At 21:37 EDT, reran the same 10,000-file smoke against the 21:16 binary:
      index 709ms, AHEAD no-hit scans 494/503ms versus ripgrep 325/325ms, and
      367ms to return the 5,000-result cap versus 367ms for ripgrep to count
      all 10,000 matches. On this fixture AHEAD's no-hit path is about 1.5x
      slower; cap-versus-full-count timing is different work. Keep profiling
      and improving no-hit latency open, and do not claim overall parity from
      this single cache-sensitive run. The smoke removed its temporary files.
      At 22:00 EDT, reran the comparison against the same 21:16 integrated
      binary: index 885ms, AHEAD no-hit scans 447/547ms versus ripgrep
      326/341ms, and 333ms to return the 5,000-result cap versus 372ms for
      ripgrep to count all 10,000. The inconsistent cold/warm ordering and
      non-equivalent hit workloads confirm these are diagnostic samples only;
      no-hit latency still merits profiling before claiming parity. The smoke
      removed its disposable workspace and profile.
      Repeated with the smoke harness warming both searchers and alternating
      execution order over five paired samples: index 298ms, AHEAD
      no-hit 484ms versus ripgrep 342ms, and 250ms to return the 5,000-result
      cap versus 458ms for ripgrep to count all 10,000. The no-hit gap persists
      at about 1.4x in this fixture; cap-versus-full-count remains unequal
      work. This is stronger diagnostic evidence, not a general benchmark.
      The updated smoke removed its disposable workspace and profile.
      Rechecked on 2026-10-14 against the integrated binary built after the
      current `ahead-core/src/search.rs`: two fresh five-pair runs measured
      AHEAD/ripgrep median no-hit scans of 477/374ms and 450/415ms. The
      10,000-file index took 686ms and 267ms; returning the 5,000-match cap
      took 729ms and 758ms, a different workload from ripgrep's full count.
      These samples show a variable no-hit gap, not stable parity or a claimed
      win; keep profiling open. The successful smokes cleaned their fixtures.
      Enabled grep-regex's candidate-line prefilter for ordinary single-line
      literal queries, retaining multiline behavior for regexes and literal
      patterns containing CR/LF. The rebuilt integrated binary passed the
      10,000-file smoke plus a multiline-literal search check. Five alternating
      no-hit samples measured a 458ms AHEAD median versus ripgrep's 338ms;
      the prior paired sample was 484/342ms. This is directional, not a
      controlled before/after claim; the no-hit gap remains. Indexing took
      5.154s, and AHEAD's 730ms capped-5,000 result time is not comparable to
      ripgrep's 410ms full count. Keep profiling; do not claim broad parity.
      On 2026-10-14, the scanner stopped recomputing a validated workspace-
      relative path before securely opening each indexed file: it carries the
      relative path into the worker and reuses the existing `openat`/
      `O_NOFOLLOW` regular-file open boundary. The active `just dev` watcher
      rebuilt `target/debug/ahead` after this source change. The current binary
      passed the 10,000-file smoke in flat and 10-directory layouts, including
      rejection of a stale indexed path replaced with an outside symlink. With
      five alternating samples, flat-layout AHEAD/ripgrep no-hit medians were
      429/381ms; nested-layout medians were 473/342ms. Indexing took 362/264ms
      and capped-5,000 search took 609/736ms respectively; those capped-result
      timings are not comparable to ripgrep's full count. These cache-sensitive
      diagnostics do not show a stable performance win: path depth still
      increases AHEAD latency, and no-hit search remains slower than ripgrep.
      A native profiler and current Rust test runner were unavailable, so keep
      profiling and source-matched tests open; do not weaken secure file opens.
      Rechecked on 2026-10-15 against the integrated binary (09:51, newer than
      `ahead-core/src/search.rs` at 00:39): the 10-directory smoke measured
      AHEAD/ripgrep median no-hit scans at 776/504ms across five alternating
      samples. The index took 890ms in a separate run and 7.388s in the
      comparison run, confirming setup/index timing is highly variable; AHEAD's
      1.123s capped-5,000 search and ripgrep's 657ms full 10,000-match count
      remain unequal work. This isolates a repeatable-looking small-file
      no-hit gap in this synthetic fixture, not general editor performance.
      Profile the secure-open/reader path on a representative project before
      tuning concurrency; keep native result-list responsiveness open.
      Follow-up: streaming UTF-8 reads no longer repeat the file-header and
      encoding classifier for every chunk; they retain late NUL/control-byte
      rejection and use the existing UTF-8 validator for later invalid bytes.
      Added `utf8_validator_streams_large_multibyte_content`; this source change
      has not been built, benchmarked, or run through Cargo tests, so it is not
      yet a measured search-speed improvement.
      Rechecked on 2026-10-15 with the integrated 10:28 binary and five
      alternating AHEAD/ripgrep no-hit samples. For 10,000 files in ten
      directories, medians were 533/395ms; a flat 10,000-file directory
      measured 541/449ms. Index walks took 816ms and 759ms respectively.
      Returning AHEAD's capped 5,000 results took 898ms and 832ms, while
      ripgrep's full 10,000-match counts took 441ms and 562ms; those hit-path
      timings do different work. Both layouts rejected an indexed file replaced
      by an outside symlink. This is one host's synthetic diagnostic: the
      no-hit gap persists, but the layout comparison does not show a consistent
      path-depth penalty. Keep profiling open and preserve the secure-open
      boundary.
      The narrow 260px Source Control row regression also passes (1 test, 213
      filtered); verify rendered controls in the disposable project. The
      retained runtime has no separate grep/content
      search handler: its `tool_search` is deferred-tool metadata discovery,
      while managed worktree search is the single AHEAD `file_search` backed
      by this service. Managed search results now show worktree-relative paths
      accepted by `read_editor_buffer` and reject out-of-workspace result
      paths. The 2026-10-03 `file_search_pages_matches_without_silent_truncation`
      regression passes and verifies cursor paging, stale-page rejection and
      out-of-workspace path rejection.
      Zed's `../zed/crates/agent/src/tools/grep_tool.rs` enriches matches from
      its live `BufferSnapshot` using syntax ancestors and containing symbols.
      AHEAD now requests best-effort symbols through its existing
      `GetDocumentSymbols` RPC for up to 16 open buffers during managed
      `file_search`, with a 300 ms deadline and bounded per-buffer and total
      payloads. It labels matches with the available containing-symbol chain
      (for example, `module › function`), capped at eight levels per match, and
      keeps matching-line previews when symbols are unavailable. Ordinary
      `read_editor_buffer` and ACP requests remain content-only. This is not
      equivalent to Zed's in-process syntax-tree context; do not add a parser,
      add parsing/search to `ahead-core`, or create a second search
      implementation. Run the new normalization, bound, request-routing and
      match-format tests, then verify Rust, TypeScript and Python with a live
      LSP plus missing/slow-server fallback in the disposable project.
      On 2026-10-07, the cached `ahead_core` test binary (newer than
      `ahead-core/src/search.rs`) passed all 36 `search::tests`; no Cargo build
      was started. This validates the shared scanner, not the agent result
      formatter or native LSP-enriched output.
      On 2026-10-07, the native formatter was updated to preserve containing
      LSP parent names and cap ancestry depth; a regression now checks flat,
      nested and deeply nested output. Rechecked 2026-10-07 23:09 ET with a
      locked/offline Cargo build of the current source: all three
      `native_client::tests::file_search` regressions passed, covering paged
      cursor safety, query/buffer fingerprinting and flat/nested/deep symbol
      ancestry. This verifies formatter behavior, not Zed's in-process syntax
      tree context or live-LSP/native result presentation.
      On 2026-10-07, the current `ahead-rpc` test binary (05:08, newer than
      `ahead-rpc/src/ahead.rs`) passed all 20 tests, including bounded
      document-symbol context validation. This closes only the protocol check;
      the agent formatter and request-routing change still need a current build.
      The source-matched `ahead-app` test binary (05:07:54, newer than
      `proxy_client.rs` and `session_panel.rs`) also passes the existing LSP
      document-symbol route and partial-results-on-timeout regressions.
      `SessionPanel::set_buffers` already tracks the open saved `CodePanel` tabs;
      snapshot requests enumerate those eligible tabs, and the agent applies
      every returned buffer override to the shared workspace search. On
      2026-10-06, the current `ahead-agent` test binary passed the snapshot
      request/reply, multi-entry workspace/scope/private filtering,
      unsaved-buffer query-fingerprint, and mock-model `file_search` tests; the
      mock-model case found an unsaved marker in one supplied buffer. An earlier
      disposable GPUI smoke also exercised one-buffer
      `file_search` and `read_editor_buffer` calls. Add a two-unsaved-tabs
      regression before claiming multi-buffer search; this was added to
      `buffer_snapshot_request_round_trips_through_native_client` on 2026-10-06
      and checks that both unsaved texts replace their saved disk versions and
      both paths appear in one result. On 2026-10-08, the source-current offline
      Cargo run of `buffer_snapshot_request_round_trips_through_native_client`
      passed (1 test), including both unsaved-buffer overrides and the combined
      `file_search` result. This verifies managed tool request/response
      plumbing, not native editor event delivery. Untitled-buffer search
      remains unsupported because snapshots require workspace-relative paths.
      On 2026-10-08, the source-newer cached app and agent test binaries also
      passed the LSP symbol-context route/formatting checks:
      `document_symbol_context_flattens_nested_and_flat_lsp_results`,
      `document_symbols_use_the_existing_lsp_proxy_route`,
      `symbol_collection_keeps_ready_results_when_an_lsp_request_times_out`,
      and `file_search_results_add_open_buffer_symbol_hierarchy`. This verifies
      offline request routing, bounded result formatting, and timeout fallback.
      Current source also revalidates symbol results against the still-open tab,
      session, path, and exact captured text after the LSP wait; changed or
      closed buffers retain their text snapshot but omit potentially stale
      symbol context. A pure freshness regression was added, but the available
      test binary predates this edit, so compilation and rendered search behavior
      remain unverified.
      Follow-up on 2026-10-08: `ahead-rpc` now retries transient stdio
      `WouldBlock` without losing a partial line, covered by a passing focused
      regression test. In the disposable project, BasedPyright and `vtsls`
      both reached Ready and Ctrl+Space returned Python and TypeScript
      completions without disconnecting the proxy; `npm test` passed (2 tests)
      and `python3 -m pytest -q` passed (1 test). The TS fixture still reports
      missing Node type declarations for `node:assert/strict`. A pasted `echo
      AHEAD_PASTE_OK` executed in the integrated PTY, and copied terminal output
      was readable through `pbpaste`. Rust Analyzer subsequently reached Ready
      and returned Rust completions in the follow-up native run. Native LSP
      startup and completion are confirmed for Rust, TypeScript and Python
      across these runs; final feature screenshots remain deferred.
      Snapshot replies cap at 128 buffers and 8 MiB and fail the tool explicitly
      when exceeded; verify the UI error, cancellation and timeout paths under
      real turns. Proxy
      `GlobalSearch` and the managed agent now reuse a lazy file snapshot
      invalidated by file-set and ignore-rule watcher events. The panel
      receives generation-tagged copies over RPC and refreshes on the
      `WorkspaceFileChange` notification, including external content changes
      without rebuilding the path snapshot. Explorer now consumes the same
      generation signal, refreshes external create/delete changes, and preserves
      expansion and selection. On 2026-10-06, direct execution of the current
      app test binary (newer than the relevant sources) passed
      `nested_and_empty_directories_remain_expandable` and
      `external_file_set_changes_refresh_tree_without_resetting_state`. These
      headless regressions verify the tree behavior, not live watcher delivery;
      verify that in the disposable editor project.
      Benchmark large worktrees and verify create/delete/ignore-rule changes
      and panel refresh on both file-set and content changes under live watcher
      delivery. Compared Zed's worktree-snapshot worker pool in
      `../zed/crates/project/src/project_search.rs` and added bounded parallel
      scanning to `ahead-core/src/search.rs`: a scoped pool sized to available
      parallelism minus one (minimum one), an
      in-flight/reorder window bounded by worker count, stable input-order
      delivery, cancellation and the global result cap. The regression
      `parallel_stream_preserves_input_order_and_global_match_limit` passed
      against the current source on 2026-10-03 (1 test). Its first cold build
      caught an inferred unsized `Path` channel payload; explicit owned
      `PathBuf` channel types fixed it, and the `ahead` dev binary now builds.
      This is correctness evidence, not yet a measured speedup. A dependency-
      free baseline harness at `ahead-core/examples/search_profile.rs` creates
      10,000 disposable Rust files, indexes them through `workspace_files`,
      checks repeated literal-search results, and reports index plus scan
      timings. These runs predate the parallel scanner. Run it with
      `cargo run --locked --offline -p ahead-core --example search_profile -j1`;
      two runs completed on 2026-10-03 with 10,000 files and 10 matches each:
      index build 88-120 ms; sorted scan timings were 1,512/2,803/24,657 ms and
      1,574/2,826/27,384 ms. Each run has a large scan outlier; because timings
      are sorted, its chronological pass is unknown. Treat this shared-machine
      synthetic baseline as noisy, not a Zed comparison. The worker cap is
      deliberately lower than Zed's
      `max(num_cpus - 1, 1)` because agent threads may compete for CPU and disk.
      A post-parallel run on the same fixture indexed in 101 ms and searched in
      656/690/9,150 ms (median 690 ms), versus the pre-parallel 2,826 ms median.
      The profile now preserves pass order instead of sorting the printed
      samples. Two more runs indexed in 91/57 ms and scanned in
      7,790/668/650 ms and 10,768/658/634 ms. Both show a slow first pass and
      two ~0.65-second warm passes; this is consistent with cold filesystem
      state but does not isolate its cause. The earlier baseline was also noisy,
      so these numbers are not a stable speedup claim or a Zed comparison. A
      further run against the current-source example binary indexed 10,000
      files in 154 ms and scanned in 7,778/643/656 ms, reproducing that cold-
      then-warm pattern without making a controlled speedup claim.
      Input ordering,
      result-cap and first-match cancellation regressions now pass; cancellation
      consumes no more than the four scheduled paths. Rerun under controlled
      load before making performance claims. On 2026-10-06, direct execution
      of the existing `ahead_core` test binary, newer than
      `ahead-core/src/search.rs`, passed all 36 `search::tests`, covering
      encoding fallback, workspace safety, index refresh/coalescing, ordered
      parallel results, cancellation and match caps. This is correctness
      evidence, not a controlled performance comparison. On 2026-10-07, the
      test binary again postdated `ahead-core/src/search.rs`; all 36
      `search::tests` and 3 `replace_` tests passed directly without starting
      Cargo. This remains headless engine evidence. The panel
      streams matching files through a bounded channel and
      falls back to a direct walk only without a proxy. A 2026-09-30
      native-window search in the disposable project found `AHEAD smoke test`
      once in `src/main.rs`; clicking the row opened line 2
      with the match selected. It also found `UNSAVED_SEARCH_NEEDLE` once in an
      unsaved `CodePanel` buffer and returned to zero results after Undo; the
      disk file stayed unchanged. This small fixture does not prove live watcher
      delivery or large-worktree quality.
      The panel now follows Zed's 10,000-range search ceiling and renders
      results in 100-match increments, with keyboard navigation limited to
      visible rows and a fixed-footer Show more control. The focused 10-test
      `ahead-app --lib search_` run passes, including page growth/clamping and
      a GPUI click through the control. `just dev` built the app, but native
      launch aborted while macOS was locked; rendered verification and
      rendered result-row verification remain open.
      The Search panel has a collapsible Replace field and a dedicated action
      row for `Replace Match` and `Replace All`, kept below the field so both
      controls remain visible in narrower panels. The action row uses the
      available GPUI Kit replace icon, and a GPUI layout regression checks that
      both controls are visible and non-overlapping. Replacements reuse
      text/word/regex, case, and path filters; literal mode
      keeps `$` text literal, while regex mode expands named/numbered captures
      and Zed-style `\\n`, `\\t`, and `\\\\` escapes. The core replacement
      service reads dirty editor overrides, rejects out-of-workspace paths,
      honors cancellation, and never writes files. AHEAD preflights every
      current editor snapshot before applying one undoable whole-buffer edit per
      target; changed files remain unsaved and open as tabs for review. The
      focused `ahead-core --lib replace_` suite passes (3 tests), and
      `ahead-app --lib search_` passes (13 tests, including replace-row
      collapse/expand and a GPUI Replace All handler/disk-unchanged smoke).
      `just dev /private/tmp/ahead-search-replace.aa8MBO` built the app, but its
      native launch aborted with signal 6 while macOS was locked. Native visual
      verification remains open; this app still lacks Zed's project-wide buffer
      store, so large replacements currently create one editor tab per changed
      file. Replace All now shares the 10,000-range ceiling with project search
      and returns an error before applying anything when a workspace exceeds it;
      a cross-file regression was added but still needs a source-current run.
      Concurrent cold searches now share one index build. After the first
      snapshot, ignored build-output events no longer
      invalidate the path list or restart search. Before that first walk,
      repeated invalidations with no cached view or running walk now coalesce;
      the focused regression and all 42 `ahead-core` library tests pass on
      2026-10-03, including the bounded parallel-scanner ordering/result-cap
      regression. Git diff refresh can still run for ignored cold-index events.
      Index invalidation now also drops the
      visible-directory snapshot, so newly created nested paths fail open
      until the replacement walk. The focused
      `ahead-core --lib workspace_file_index_rebuilds_after_invalidation`
      regression passed on 2026-10-01 (1 passed, 33 filtered).
      AHEAD Quick Open now shows up to 20 session-local recent indexed files
      for an empty query, promotes the active file among fuzzy results, and
      parses `path:line`, `path:line:column` and `path:start-end` before opening
      the requested location. `:start-end` selects the complete inclusive line
      range, ending at the following line's start; goto columns use character
      positions while content-search selections retain UTF-8 byte offsets.
      All 7 `ahead-app --lib quick_open` tests re-passed on 2026-10-01 after
      the range and column-unit changes (157 filtered). The CodePanel suite also covers
      Unicode external columns and byte-range positions. The last 20 canonical,
      workspace-relative paths are now persisted in `.ahead/session.db`, with
      traversal and symlink escapes rejected; Quick Open merges restored recents
      behind current-session navigation. The two focused proxy
      `recent_workspace_files` tests passed on 2026-10-01 (185 filtered).
      The Quick Open suite includes a GPUI Enter-key confirmation and
      Escape dismissal. The 2026-09-30 disposable native-window pass found
      that the shell omitted the dialog layer; after adding it and retaining
      one picker entity across renders, `⌘P` and the toolbar picker displayed
      indexed files, filtered by name, and mouse selection opened `src/main.rs`
      in the editor. GPUI's dialog Confirm action intercepted Enter; Quick Open
      now handles that action. The final native-window pass confirmed Enter
      opened `src/AGENTS.md` as the active editor tab. Verify recent-file
      persistence across restart and large worktrees.
      The initial path-snapshot build now stops for superseded proxy searches,
      cancelled native tool calls, and cancelled panel `WorkspaceFiles` RPCs.
      Live watcher delivery and cancellation/timeout behavior still need a
      GPUI smoke test.
      Verify all three search callers against Zed's worktree/private-file
      filtering and non-UTF-8 decoding. Shared search now validates UTF-8 while
      streaming (including lines with no matches), then follows Zed's
      BOM/encoding-detection fallback. Tests cover UTF-16 BOM, Windows-1251,
      decoded UTF-8 byte columns, invalid-file partial-result recovery and
      cancellation. The complete 29-test `ahead-core` library suite passes with
      `rtk proxy cargo test --locked --offline -p ahead-core --lib -j 2`. An
      AHEAD-owned 1 KiB
      prefix classifier now recognizes plausible BOM-less UTF-16LE/BE text
      (including Cyrillic), known binary signatures, and control-heavy binary
      chunks without NUL; a regression covers both byte orders, Cyrillic, and
      no-NUL binary decoys, including a text prefix before the binary data. The
      classifier follows `../zed/crates/language/src/file_content.rs` at pinned
      Zed revision `418f897`; the UTF-16/binary regression passes in the full
      suite. Non-UTF-8 fallback still buffers the full file;
      binary formats without a known signature or control-heavy content can
      still evade detection. The
      shared matcher and RPC match DTO now carry multiline regex ranges as
      start/end line and byte-column coordinates, following Zed's
      `../zed/crates/project/src/search.rs` range model. The multiline range
      regression now passes in the full suite; verify the panel's range display,
      result navigation and full-range editor selection before claiming regex
      parity.
      Native
      `file_search` now has 20-match pages and Zed's default private-file
      filter plus optional include patterns over the shared
      worktree path view. Its opaque cursor binds the query, current open-buffer
      contents, workspace search revision and emitted-result prefix; content-
      only watcher events invalidate cursors without rebuilding the cached path
      list, while file-set and ignore-rule events invalidate both. Stale cursors
      fail with a restart instruction rather than silently shifting pages. The
      agent grep in Zed also starts a project search for each offset page; keep
      this behavior aligned unless multi-page benchmarks show a material cost.
      obsolete raw-offset argument has been removed from the AHEAD-owned tool
      schema and handler; pagination accepts the returned cursor only. The
      focused managed-tool and file-search tests pass (1 and 2 tests), and
      `cargo fmt --all -- --check` passes. The
      complete `ahead-core --lib` suite passes (31 tests), the complete
      `ahead-agent --lib` suite passes (88 passed, one provider test ignored),
      and `ahead-proxy --lib` checks with the watcher revision update. The
      agent suite's loopback-only mock tests required the local-test permission;
      no real provider was contacted. Remaining: configurable private-file
      rules and evaluating formats without recognized signatures beyond the
      bounded prefix heuristic. Search now recognizes common gzip, BZip2, XZ,
      Zstandard, LZ4, 7z and RAR signatures; a printable-only BZip2 file is
      covered by the binary-skip regression so it cannot leak text matches.
      Classifier/header and search-path regressions are added but remain
      uncompiled and unrun because the shared Cargo loop is still locked.
      The Search panel now drains up to 32 queued per-file result batches before
      updating and sorting its accumulated list, avoiding a full-list sort and
      repaint for every matching file. Verify responsiveness in the native
      large-project search flow. File and match rows are now flattened into
      stable row indexes and rendered through GPUI `uniform_list` at a fixed
      height; Up/Down scrolls the selected match into view. A regression checks
      file grouping and match-to-row mapping. Native 10,000-result
      UI responsiveness is not yet verified because the Mac remains locked.
      A one-off 2026-10-08 proxy scale smoke against the 13:42 binary scanned a
      disposable 10,000-file worktree with no hits in 876 ms, then returned the
      configured 5,000-match cap in 275 ms. This is a single headless proxy
      measurement, not a renderer or Criterion benchmark. AHEAD has no
      GPUI benchmark target or `bench-support` feature; `ahead-app` enables
      `test-support` in dev-dependencies, so a production benchmark needs an
      isolated package whose feature graph excludes it.
      The shared content scan now follows Zed's bounded worker pattern: each
      worker owns its `Searcher`, results are delivered in input order, and the
      in-flight/reorder window is capped at four files. A regression was added
      for deterministic order and the global match cap. The 2026-10-08
      `tests/workspace-search-scale-smoke.mjs` now exercises the cached
      `workspace_files` snapshot before content search, matching the Search
      panel's request order. Against the existing binary on 10,000 files, it
      measured 1,891 ms for the index walk, 1,729 ms for the first no-hit scan,
      1,773 ms for a repeated no-hit scan, and 923 ms to return 5,000 capped
      matches. A separate direct-cold run measured 3,669 ms end-to-end, close
      to the index-plus-scan total. This is headless proxy timing, not a renderer
      benchmark; the disposable workspace/profile were removed on success.
      Re-ran the same smoke against the current `target/debug/ahead` on
      2026-10-09: 10,000-file index 5,104 ms, first full no-hit scan 1,315 ms,
      warm no-hit scan 741 ms, and 388 ms to return the 5,000-match cap. The
      isolated workspace and profile were removed on success. These timings are
      machine/cache-sensitive and do not replace the still-pending current
      `ahead-core` test run for the 10,000/10,001 Replace All boundary or native
      large-result rendering/benchmark verification.
      Another isolated run on 2026-10-09 against `target/debug/ahead` (built
      12:42) measured a 476 ms index walk, 1,259 ms first no-hit scan, 689 ms
      warm no-hit scan, and 444 ms to return the 5,000-match cap. The large
      same-day swing in index time reinforces that these are cache-sensitive
      smoke timings, not a performance benchmark; the temporary project/profile
      were removed on success.
      Rechecked twice on 2026-10-09 against the 15:00 `target/debug/ahead`
      binary in a clean environment: the runs measured (index / cold no-hit /
      warm no-hit / 5,000 capped matches) 529 / 6,034 / 5,695 / 2,176 ms and
      433 / 5,442 / 1,908 / 937 ms. Both isolated projects and profiles were
      removed. The no-hit scans vary substantially even between same-binary
      runs; do not tune worker counts from this smoke alone. Profile secure
      per-file opens and contention before changing the bounded worker policy.
      The smoke now has an opt-in `AHEAD_SEARCH_SCALE_RIPGREP=1` baseline that
      runs `rg --count-matches --fixed-strings` against the same corpus and
      asserts its counts. Two same-binary comparisons measured AHEAD
      (cold/warm no-hit, 5,000 capped results) at 1,842/690/378 ms and
      1,316/732/410 ms; ripgrep (first/repeated no-hit, count all 10,000 hits)
      at 549/414/417 ms and 457/427/484 ms. In each run, AHEAD's cold scan
      preceded ripgrep, so ripgrep had a warmer filesystem cache; capped AHEAD
      result delivery is also not equivalent to ripgrep counting every hit.
      This provides a useful same-corpus reference, not an apples-to-apples
      benchmark. A third run measured AHEAD at 1,162/718/389 ms and ripgrep at
      357/426/444 ms in the same order. Alternate scan order and profile
      secure-open/protocol overhead before changing the worker policy.
      The smoke now passes only PATH/TMPDIR and isolated HOME/AHEAD data paths
      to the proxy child instead of copying the caller's environment. It still
      passed with synthetic OpenAI/OpenRouter/Anthropic key variables present
      in the runner, and removed the disposable project/profile on success; no
      provider request was made.
      On 2026-10-12, `Utf8ValidatingReader` now validates complete UTF-8 chunks
      directly instead of copying each file chunk through a temporary buffer;
      split, truncated and malformed UTF-8 remain covered. The source-current
      `ahead-core --lib search::tests` run passed 39 tests (13 filtered). A
      post-change 10,000-file smoke measured a 2,426 ms index walk, 1,163 ms
      cold and 722 ms warm no-hit scans, and 434 ms to return the 5,000-match
      cap; ripgrep measured 413/354 ms for its first/repeated no-hit scans and
      478 ms to count all 10,000 hits. These single-run timings are
      cache-sensitive and the capped AHEAD result is not equivalent to
      ripgrep's full count; treat them as diagnostics, not a speedup claim.
      One run timed out while building the workspace-file index, then the
      immediate retry passed. The restarted `just dev` binary built, but native
      launch still aborts on the locked Mac's HIServices connection error.
      The proxy now begins warming that same shared index in a cancellable
      background task during workspace initialization, following Zed's visible
      worktree initial-scan lifecycle. This shifts the cold index walk away
      from first use but adds startup filesystem I/O; verify the change in a
      rebuilt app and measure contention/startup cost. A core regression now
      cancels a cold walk mid-scan and verifies that a later caller can rebuild
      the index, and a proxy regression checks initialization starts the warmup
      and shutdown cancels it. Source-current index-filtered runs passed five
      `ahead-core` tests and twelve `ahead-proxy` tests, including these
      cancellation paths. `ProxyClient::new` sends one `Initialize` per proxy
      child; `FileWatcher` and `PluginCatalogRpcHandler` each consume a
      single-use receiver. Same-dispatcher workspace reinitialization is not a
      supported lifecycle; opening another workspace creates another proxy.
      Measuring warmup contention/startup cost remains open. A current-build
      2026-10-12 scale rerun with
      `AHEAD_SEARCH_SCALE_RIPGREP=1` passed on 10,000 files: index request
      795 ms, cold/warm no-hit scans 1,033/543 ms, and 378 ms to return the
      5,000-result cap; ripgrep measured 467/438 ms for its first/repeated
      no-hit scans and 475 ms to count all 10,000 hits. The smoke removed its
      disposable workspace/profile on success. Treat this as a diagnostic
      measurement; initialization warmup may overlap the index request.
      On 2026-10-12, another current-binary run measured a 6,073 ms index
      request, 852/582 ms first/warm no-hit scans, and 249 ms to return 5,000
      capped matches. A ripgrep comparison immediately after measured a 1,849
      ms index request, 1,097/584 ms AHEAD no-hit scans, and 332 ms for capped
      results; ripgrep took 580/917 ms for first/repeated no-hit scans and 418
      ms to count all 10,000 hits. Since ripgrep runs after AHEAD, the scan
      order warms its filesystem cache; index time also varied across runs.
      These diagnostics do not prove a speedup or regression. Both disposable
      projects and profiles were removed.
      Latest current-binary run on 2026-10-12 with optional ripgrep comparison:
      10,000-file index request 6,604ms; AHEAD cold/warm no-hit scans 714/496ms
      and 257ms to return 5,000 capped matches; ripgrep no-hit scans 328/386ms
      and 407ms to count all 10,000 matches. These are sequential, cache-sensitive
      diagnostics and the capped-result/full-count timings differ; no speedup
      claim follows from this run. The disposable workspace and profile were
      removed on success.
      Keep native large-result rendering verification open, and run a
      production-shaped renderer benchmark once AHEAD has an isolated
      benchmark feature graph.
      Rechecked 2026-10-13 against the rebuilt `target/debug/ahead` with the
      ripgrep baseline enabled: 10,000-file index request 4,782 ms; AHEAD
      cold/warm no-hit scans 451/423 ms and 271 ms to return 5,000 capped
      matches; ripgrep no-hit scans 362/365 ms and 340 ms to count all 10,000
      hits. The disposable workspace/profile were removed. This single
      cache-sensitive run is diagnostic only; capped results and full counts
      are not equivalent, and native large-result rendering remains open.
      Three repeated current-binary runs on 2026-10-13 measured workspace
      indexing at 273-671 ms, the first no-hit search at 484-523 ms, the
      repeated no-hit search at 505-549 ms, and 320-406 ms to return the
      5,000-result cap. Ripgrep measured 306-347/295-323 ms for first/repeated
      no-hit scans and 358-401 ms to count all 10,000 hot matches. Median AHEAD
      no-hit scans were about 1.6-1.7x slower in this synthetic fixture; the
      hot result comparison is not equivalent because AHEAD caps at 5,000.
      Treat these as diagnostics, not a user-visible benchmark. Profile the
      secure file-open and result-ordering path before considering changes;
      do not weaken workspace-path or symlink protections for speed.
      Search disk reads now require an explicit workspace scope. On Unix the
      matcher opens indexed files relative to a no-follow directory handle,
      reuses that same file for non-UTF-8 fallback, and rejects out-of-workspace
      buffer overrides; buffer-only searches cannot open disk paths. A
      disposable-project regression replaces both an indexed file and its
      parent directory with outside symlinks, and all 34 `ahead-core` tests pass
      (2026-09-29). The Windows fallback still uses a path-based reopen after
      canonicalization, so handle-relative protection there remains open.
      An `ahead-agent --lib` regression compiled the shared search API on
      2026-09-30. The disposable-project panel smoke above exercised this API;
      app/proxy focused checks and a real managed tool-call remain open.
      The `Cmd+P` GPUI modal now uses `ahead_core::search::rank_file_paths`
      with the generation-tagged workspace snapshot, current-directory
      affinity, match highlighting, Zed-style smart-case scoring and
      stale-rank cancellation. Verify rendered focus, arrow/Enter navigation,
      file opening and watcher refresh against Zed's
      `../zed/crates/file_finder`; measure responsiveness on a large worktree
      before choosing whether to parallelize. The focused path-highlight test
      and one-job app-library check pass, but do not claim the live picker works
      until that smoke test passes.
      Follow-up on 2026-10-13, using Zed's
      `../zed/crates/project/src/project_search.rs` shared-background-executor
      pattern: replaced per-query OS worker fanout with a lazily initialized
      process-wide Rayon pool sized to available CPUs minus one (13 threads on
      this host). Each search retains its bounded in-flight window, cancellation
      checks, input-order delivery and streamed result cap; a thread-local
      `grep_searcher::Searcher` is reused by each pool worker. The source-current
      `ahead-core --lib search::tests` suite passes 40/40, including a regression
      that asserts searches share one pool with the configured worker bound.
      The rebuilt same-process 10,000-file profile measured median batches for
      1/2/4/8 concurrent no-hit searches at 629/920/1,564/2,996 ms; a second
      warm run measured 666/957/1,837/3,331 ms. The preceding per-query-worker
      profile measured 589/856/1,642/2,770 ms. These synthetic runs show similar
      throughput with noisier/slower 8-way latency, while limiting search worker
      threads from as many as 104 to 13 per process. Verify mixed interactive
      panel plus concurrent managed-agent searches and separate proxy processes
      before closing scheduler tuning or responsiveness. After the fresh
      2026-10-13 rebuild, the 10,000-file disposable smoke measured 620ms index
      build, 442ms cold and 524ms warm no-hit scans, and 262ms to return the
      5,000-match cap. This run did not include a ripgrep baseline and is
      diagnostic only; it does not close concurrent-search responsiveness.
      Two current-binary 10,000-file runs on 2026-10-14 measured index builds
      of 5,015ms and 509ms, showing strong filesystem-cache sensitivity. AHEAD
      full no-hit scans were 502/529ms and 531/520ms; returning the capped 5,000
      matches took 335ms and 304ms. Sequential ripgrep comparisons measured
      407/317ms and 348/444ms for no-hit scans, then 374ms and 477ms to count
      all 10,000 hot matches. These are diagnostic proxy timings, not an
      apples-to-apples benchmark: the AHEAD result is capped and the first run
      also paid a cold index walk. Do not tune worker policy from these samples;
      use a production-shaped renderer benchmark for UI claims. Both temporary
      projects and profiles were removed on success.
      A further 2026-10-14 scale run against the current integrated binary
      indexed in 764ms; no-hit scans took 528/764ms, and the capped 5,000-match
      result took 492ms. Ripgrep took 457/734ms for no-hit scans and 406ms to
      count all 10,000 matches. This is another cache-sensitive diagnostic,
      not an apples-to-apples speed claim. An exploratory eight-request burst
      against the proxy `global_search` route returned `expired search job` for
      superseded requests. Source inspection shows that route deliberately
      cancels older scans via a process-global latest-job counter; the workspace
      panel uses per-panel generations and the agent `file_search` uses
      per-request cancellation, both over `ahead-core::search`. The proxy burst
      therefore does not measure their contention. Mixed live panel/agent
      searches and separate proxy-process contention remain open.
      On 2026-10-14, the current integrated-binary managed Code Mode smoke was
      strengthened to launch eight `file_search` calls concurrently through
      the real tool bridge. All eight produced independent buffer-snapshot
      requests and completed; the full delegated-search/cancel/Turso
      reopen/retry/shutdown smoke passed. This confirms concurrent agent search
      results survive the proxy's latest-query-wins `global_search` policy; it
      does not measure 10,000-file agent latency or mixed live panel/agent
      contention.
      Rechecked against the rebuilt 20:09 integrated binary on 2026-10-14:
      the 10,000-file disposable search index took 298ms, cold/warm no-hit
      scans 471/490ms, and returning the 5,000-match cap 348ms. Ripgrep took
      335/326ms for no-hit scans and 374ms to count all 10,000 matches. This is
      one cache-sensitive sample and compares capped results with a full count;
      it is diagnostic only, not a general speedup claim. The smoke removed its
      temporary workspace and profile.
      Rechecked at 23:19 ET on 2026-10-14 against the current integrated
      binary with alternating ripgrep baseline: 10,000-file indexing took
      348ms; median AHEAD no-hit searches took 454ms versus ripgrep's 324ms;
      AHEAD returned its capped 5,000 matches in 787ms while ripgrep counted
      all 10,000 hot matches in 410ms. The no-hit comparison is about 40%
      slower in this synthetic run; capped results and full counts are not
      comparable, and one run is not enough to retune workers. Profile the
      secure per-file open/result-ordering path before changing the policy.
      The disposable workspace and profile were removed after success.
      Rechecked on 2026-10-15 against the current integrated binary with the
      same disposable 10,000-file workspace and alternating ripgrep baseline:
      indexing took 5.387s; median no-hit scans were 718ms for AHEAD and 564ms
      for ripgrep; AHEAD returned its 5,000-result cap in 1.239s while ripgrep
      counted all 10,000 matches in 611ms. The stale-index symlink escape was
      rejected and the smoke cleaned its fixtures. This synthetic run confirms
      the remaining no-hit latency gap; capped-result and full-count timings
      differ in work, so it is diagnostic rather than a parity claim. Profile
      the secure per-file open and result path before changing either boundary.
      Rechecked later on 2026-10-15 against the 17:35 integrated binary with
      the alternating ripgrep baseline: 10,000 files indexed in 403ms, median
      no-hit scans were 503ms for AHEAD versus 353ms for ripgrep, and AHEAD
      returned its 5,000-result cap in 757ms while ripgrep counted all 10,000
      hot matches in 384ms. The stale-index symlink escape was rejected and
      fixtures were cleaned. This repeats the synthetic no-hit latency gap;
      capped output versus a full count is not an apples-to-apples comparison.
      Profile secure per-file opens and ordered result delivery before tuning.
      Two alternating rechecks later on 2026-10-15 measured median AHEAD versus
      ripgrep no-hit latency at 497/350ms and 506/389ms; indexing took 290/270ms
      and capped-result delivery took 743/847ms. Ripgrep counted all 10,000 hot
      matches in 442/408ms. Reusing the UTF-8 prefix buffer per search worker
      then measured no-hit latency at 485/332ms and 472/408ms; index builds
      varied from 268ms to 5.281s and capped delivery took 702/740ms. These
      cache-sensitive samples show no clear speedup; keep the allocation reuse
      only as a bounded per-worker improvement, not a parity claim. Capped
      output and full counts remain incomparable. The source-current
      `ahead-core --lib` suite passes 59/59; it exposed and now covers literal
      whole-word replacement using the look-around matcher without expanding
      `$` replacement text. The disposable projects and profiles were removed.
      The scale smoke now also supports 10 flat sibling directories to compare
      path depth at the same file count. Two flat runs measured AHEAD/ripgrep
      median no-hit scans at 426/448ms and 514/375ms; nested runs measured
      472/408ms and 487/390ms. The measurements are too noisy to attribute a
      consistent cost to the extra directory component, so keep the secure
      per-component open path and do not add a directory-handle cache without
      profiling evidence.
      A later recheck against the integrated binary measured a 312ms index
      build and median no-hit scans of 595ms for AHEAD versus 468ms for ripgrep
      across five alternating samples. AHEAD returned the capped 5,000 matches
      in 897ms; ripgrep counted all 10,000 hot matches in 426ms, which is
      different work. The stale-index symlink escape was rejected and the
      disposable fixture/profile were cleaned. This synthetic run confirms the
      no-hit gap but does not isolate secure-open cost from matching or ordered
      delivery. The `search_profile` example has no current built binary and
      less than 1 GiB is free, so no Cargo build or performance code change was
      started; profile those components separately before changing the security
      boundary or worker policy.
      On 2026-10-20, the rebuilt integrated proxy batches eight paths per
      Rayon task to reduce per-file task/channel overhead while retaining
      ordered delivery and bounded cancellation. The disposable 10,000-file
      smoke passed (index request 5.55s, median AHEAD/ripgrep no-hit 489/385ms,
      771ms for AHEAD's 5,000-result cap); the stale-index symlink escape was
      rejected and fixtures were removed. This single diagnostic has no
      same-binary pre-change baseline, so it does not establish a speedup; the
      no-hit gap and production profiling remain open. After making hit batches
      flush promptly, a later exact-binary run measured 5.80s indexing,
      652/430ms median no-hit scans, and 1.04s for the capped hit search. The
      same-day samples vary substantially, so no performance claim follows.
- [ ] Add an optional watch/restart action for parsed just recipes using
      watchexec's supervisor/process-group and clear-screen support; direct
      recipe runs should remain the default. The Tasks panel now has a
      separate Watch action that opens a fresh terminal titled `watch: <recipe>`
      and runs watchexec with restart, clear-screen and direct-argv mode; Run
      remains unchanged. A GPUI regression covers Watch dispatch and a command
      test covers shell-safe recipe/justfile names. Watching requires
      `watchexec` on PATH; native task restart behavior remains for user review.
- [ ] Port a Git source-control/review view with changed files, diffs,
      stage/unstage, commit, branches and conflicts; preserve AHEAD anchors,
      attribution and session trailers. `Commit Staged` now calls an acknowledged
      proxy request and keeps the message on error; `Stage All` now also uses
      an acknowledged background proxy request and shows Git's failure.
      Workspace status now comes from an asynchronous proxy request using
      Zed's porcelain-v1, NUL-delimited, no-renames Git query, scoped to the
      selected folder when it is nested in a larger repository. Per-file
      stage/unstage checkboxes now use an asynchronous proxy operation, show
      index state, and preserve worktree contents when unstaging. Proxy tests
      cover staged plus unstaged edits, untracked paths, nested relative paths,
      sibling exclusion for status and Stage All, literal filenames, unborn
      branches and Git errors;
      the GPUI state test covers index state and pending-operation status.
      The History tab now loads and refreshes the latest 100 commits through
      the proxy and shows each subject, author, relative time, and abbreviated
      ID. Selecting a commit opens its parent-relative patch in a virtualized,
      syntax-neutral diff view with added/removed/hunk colors, a back action,
      and a 2 MiB truncation notice. Source tests cover root and regular commit
      patches plus history/detail/back navigation; these checks have not been
      run in this source-only pass.
      The Source Control `View Diff` action now opens the first changed file
      that still exists and reveals its first inline hunk; a GPUI regression
      covers preferring an existing path. Deleted paths now load a bounded
      per-file working-tree patch through the Git proxy
      and display the removed HEAD content in the existing patch viewer; focused
      proxy regressions cover nested workspaces and reject parent traversal.
      These existing per-file regressions have not been run. Aggregate review
      is implemented below; its focused regressions have not been run.
      Clicking a changed-file path now opens that workspace file, scrolls its
      first changed line into view, and expands a read-only editor projection
      with removed rows inserted before the current hunk and added rows tinted.
      The original buffer and caret remain untouched while the projection is
      open. Deleted paths use the proxy patch viewer instead of attempting to
      open a missing file. Native rendering remains for user verification.
      Local branch switching lists local branches and performs an acknowledged
      checkout only from a clean worktree. The branch picker can also create
      and select a local branch, including naming the initial branch in an
      unborn repository. Git status reports repository-wide dirty state so a
      nested workspace disables the picker when sibling paths are modified;
      the proxy independently rejects dirty checkouts/branch creation. The
      misleading inert Publish button was removed. Focused regressions cover
      branch discovery, nested dirty-state reporting, clean/dirty checkout,
      existing/unborn branch creation, invalid/duplicate/dirty creation, RPC
      acknowledgements, and UI guards; they have not been run. Porcelain
      unmerged states are now reported repository-wide and marked on file rows;
      Stage All excludes those paths, while the user can explicitly stage a
      resolved file. The proxy refuses commits while any repository conflict
      remains, and the panel disables Commit Staged with a recovery message.
      Focused regressions cover all seven Git unmerged status pairs, preserving
      conflict entries during Stage All, the commit guard and UI gating; they
      have not been run. The Source Control View Diff action now requests one
      bounded working-tree patch for all staged, unstaged, deleted and
      untracked files under the selected workspace, then shows it in the
      existing copyable review pane; individual file rows still open that file
      in the editor. The proxy diff follows Zed's project-wide review shape and
      preserves the nested-workspace boundary. Focused proxy, client-RPC and
      GPUI regressions were added but not run. Local branch deletion is now
      available from a nested branch-picker menu. The proxy refuses to delete
      the checked-out branch, a branch with unmerged commits, or any branch
      while the repository is dirty; focused proxy and acknowledged-RPC
      regressions were added but have not been run. Source Control now exposes
      Publish for a branch without an upstream and Push once tracking is set;
      the proxy uses Git's configured push remote/branch mapping and the user's
      credential helpers, and disables terminal-only prompts so a missing
      credential fails visibly instead of hanging the editor. The adjacent
      remote menu supports explicit destinations, which the proxy validates
      against the current repository. Local bare-remote push/upstream and
      acknowledged-RPC regressions were added but not run; the bare-remote test
      also verifies choosing another remote does not silently change an
      existing upstream, matching Zed's push behavior. When no user-configured
      askpass command is configured for the transport, Git now falls back to a
      transient AHEAD credential prompt routed to the Source Control panel;
      password/passphrase prompts are masked, cancel sends no credential, and the answer is returned
      only to the Git subprocess (not session or agent storage). The UI expires
      abandoned prompts before the bridge timeout, clears both transient inputs,
      and core-RPC debug output redacts credential answers. Existing Git
      credential helpers, askpass environment variables and `core.askPass`
      remain the first choice; AHEAD installs its Git fallback only when none
      of those user-configured askpass paths would take precedence. A focused
      precedence regression was added. Core RPC, helper-protocol and UI
      regressions were added but not run; native HTTPS/SSH
      authentication remain to verify. Windows now builds and installs a
      separate console-capable `ahead-askpass.exe` for direct `SSH_ASKPASS`
      invocation, following Zed's Windows AskPass boundary; its installed
      authentication flow still needs platform verification.
      Commits still invoke Git so configured
      hooks and signing run. The editor now exposes Zed-style Use Current, Use
      Incoming and Use Both actions for the well-formed conflict containing the
      caret; standard and diff3 marker blocks are handled, and the resolution
      remains an unsaved buffer edit until the user reviews and stages it.
      Parser and GPUI interaction regressions were added but not run.
- [ ] Add a distinct pull-request review work type: open an immutable base/head
      snapshot, show each change in the context of the entire file, and let a
      human use the active AHEAD agent to interrogate the change with the
      originating session's explicitly shared checkpoints and artifacts
      (decisions, plan, conversation, evidence and provenance). Keep review
      read-only by default, pin all context to revisions, preserve findings,
      artifact review statuses and dispositions, record the reviewer relationship
      and applicable team/repository policy, and offer advisory merge readiness.
      Support handoff back to the originating work rather than treating GitHub's
      line-diff view as the canonical review experience; do not gate or perform
      the repository merge or require a second reviewer universally.
- [ ] Port Zed's high-fidelity DAP debugger surface: sessions, breakpoints,
      threads, stack frames, scopes, variables, stepping, source mapping and
      verified state. Carry upstream tests and add AHEAD policy/session tests.
      The Debug panel now retains the stopped thread's ordered stack and
      per-scope variables, displays both, and opens the source location when a
      frame or breakpoint is clicked. The proxy also fetches a single stopped
      thread's stack when an adapter reports that other threads are still
      running. `.ahead/run.toml` now has typed `[[configurations]]` entries and
      a GPUI-kit configuration picker; malformed or empty project config is
      shown instead of silently falling back. Frame selection now fetches its
      scopes, and nested variables expand lazily through DAP with stale-reply
      guards. When the adapter reports all threads stopped, the panel can
      switch threads and directs stepping to the selected thread. VS Code
      `launch.json` discovery remains open.
- [ ] Finish debugger lifecycle verification in `ahead-proxy/src/plugin/dap.rs`
      and `catalog.rs`, referencing Zed's `crates/dap/src/transport.rs` and
      `client.rs`. Adapter ownership, bounded stderr reads, early registration,
      pending-request failure and catalog shutdown are implemented. Six real
      process regressions pass: stalled initialization, unresponsive cleanup,
      EOF callbacks, stderr/replacement isolation, request deadlines, and direct
      adapter exit while a descendant holds its pipe open. Async requests now
      use the existing 30-second sync deadline; callbacks run outside locks,
      late replies are discarded, and expiry leaves the adapter available.
      Timeout messages reach the existing editor status channel. On
      2026-10-01, all 179 proxy library tests passed after rebuilding the
      sandbox helper removed by `cargo clean`; the rebuilt root executable
      passed `tests/dap-shutdown-smoke.mjs` in running, initializing and
      full-stderr modes. The 2026-10-02 rebuilt-binary rerun passed all three
      modes in `ahead-dap-shutdown-ThVFd0`. All three re-passed on 2026-10-03
      against the fresh 01:38 debug binary; the disposable fixture was removed.
      The post-clean debug binary passed all three again.
      Exercise the native debugger in a disposable project.
      Verify graceful debuggee termination, stop/restart during initialization,
      retry after initialization/launch failures, pending terminal handoff,
      rendered timeout feedback, and Linux/Windows behavior. Forced cleanup
      owns only the direct adapter child; it does not terminate escaped
      descendants or guarantee their inherited pipes close.
      Verify the new typed lifecycle/error path in
      `ahead-app/src/{proxy_client,debug_bar}.rs` and
      `ahead-rpc/src/{core,dap_types}.rs`. Startup/launch failure, EOF and
      control errors now reach the bar without relying on ignored log messages.
      Start remains Starting until a runtime update; Stop remains Stopping
      until adapter cleanup finishes. Stop interrupts initialization and sends
      running-session teardown independently of the event worker. Disconnect
      requests `terminateDebuggee: true`. Late launch/step acknowledgements and
      stale stack metadata are fenced by process generation and state revision.
      Four proxy tests cover startup cancellation, spawn/launch failure,
      step acknowledgement/error ordering and disconnect/reap; they passed in
      the 2026-10-01 full proxy suite after disk space was restored.
      Session IDs now survive the RPC boundary, and each launch gets a new ID;
      controls and breakpoint changes use it instead of hard-coded zero.
      Continue/step require a known stopped thread, stale-session events are
      rejected, and debugger updates wake the bar through the existing metadata
      subscription. Proxy EOF clears its active/stack state and blocks new
      launches until the app reconnects through restart. Verify these paths
      in the rebuilt native app. Step/Continue now admit one request while a
      stopped-session control is pending; a failed reply releases the slot,
      and a state transition clears it. The focused admission regression and
      all 15 DAP proxy tests pass on 2026-10-02. Verify rapid native shortcut
      presses and error/retry presentation. Cover restart during initialization
      and exercise rejected/timeout termination and the
      `supportsTerminateRequest` path with real debuggees.
      Verify the latest request-submission fixes: launch uses its captured
      generation; Continue/Pause now queue through the existing async request
      path instead of spawning waiting threads, and step requests retain their
      captured generation too. Terminal requests queued after Stop are rejected
      before forwarding to the editor. Three added regressions cover retired
      launch/control submission, stopped-session terminal forwarding, and async
      Continue/Pause error delivery. After the typed terminal handoff below,
      all 14 focused DAP tests and the full 180-test proxy suite passed on
      2026-10-01. Cover same-generation state changes before submission.
      Keep disconnecting adapters owned until cleanup finishes so catalog
      shutdown can still wait; completed retired handles are pruned on launch.
      The four focused app debugger tests pass, including a 20-seed GPUI test
      for failure display, retry, stale-session errors and confirmed cleanup.
      These use injected RPC notifications, not a native debugger launch.
      The `runInTerminal` handoff now carries typed argv/cwd/env and
      session/generation/request identity over RPC. The app launches its native
      PTY terminal, returns the shell PID or error to the matching DAP request,
      and owns terminal cleanup on Stop, failure and proxy disconnect. The
      skipped `RunDebugConfig.debug_command` and proxy-owned terminal-close
      path were removed. Referenced Zed's
      `crates/project/src/debugger/session.rs::handle_run_in_terminal_request`
      and `crates/debugger_ui/src/session/running.rs` plus its reverse-request
      tests. A disposable local mock adapter requested an integrated terminal:
      the native editor rendered `AHEAD_DAP_TERMINAL_OK`, the adapter received
      `shellProcessId`, Shift+F5 reached `disconnect`, and the shell PID exited.
      This does not prove real LLDB/Node/Python adapters, concurrent requests,
      cancellation during terminal spawn, or Linux/Windows behavior. Add a
      repeatable native smoke fixture; verify rejected/timeout requests, shell
      startup failures, requested terminal title/kind and the command's PID
      semantics. When no project config exists, the built-in defaults still
      point at `target/debug/ahead` even for arbitrary projects. Prefer project
      configs and implement VS Code `launch.json` discovery before calling the
      debugger broadly usable. Do not infer high-fidelity debugger features
      from this handoff.
      The rebuilt-proxy standalone shutdown smoke passes all three disposable
      mock adapter modes. The separate native mock probe above exercises
      `runInTerminal`, but not a real debuggee.
- [ ] Verify the new Zed-style terminal-tab rename flow and finish native
      resize/reflow and close/quit acceptance. Allow selected terminal output
      to attach to the AHEAD agent panel as context. `@terminal` now prefers
      non-empty selected text, falls back to bounded recent output, and labels
      which source was attached; the focused selection/fallback regression is
      added but has not been run. Source also adds the
      right-click `Rename Terminal` action and inline title editor, with Enter/
      Save to apply and Esc/Cancel to preserve the prior title; a focused
      title-state regression covers custom, dynamic and cleared names.
      `rustfmt` and `git diff --check` pass, but the latest `target/debug/ahead`
      (19:37) and app test binary (19:36) predate this source edit (20:04), so
      they are not build evidence.
      The inline rename state now hides the terminal-close X so its Cancel X is
      the only close-shaped control; rendered confirmation remains part of
      native acceptance.
      The existing dev-loop handle remains live but has emitted no rebuild
      output; process inspection is denied. Reuse that loop for compilation and
      perform the native interaction check after the desktop is unlocked.
      Separate terminal panels already appear as tabs in the bottom
      `DockLayout`.
      `ahead-app/src/terminal.rs` now retains the Alacritty IO handle, terminates
      its Unix shell/foreground groups and reaps on a background thread.
      Real PTY regressions cover HUP/TERM-resistant jobs, idempotent cleanup and
      final-output retention. macOS shutdown now flushes unread kernel queues
      after termination and before reaping; otherwise a killed shell can stay
      in exit waiting for the stopped reader. A deterministic unread-output
      regression fails without the flush and checks the shutdown receipt with
      it. Alacritty reference-test recording is disabled; a standalone PTY
      test from a disposable directory passed without creating
      `alacritty.recording`. Verify this change through native tab
      close/window close/quit after
      rebuilding. Also verify Linux/Windows, detached jobs, an IO thread
      that fails to return, and native quit within GPUI's short hook deadline.
      Rechecked 2026-10-08 18:58 EDT using the existing app test runner built
      at 15:18, newer than `terminal.rs` (07:40) and `terminal_panel.rs` (09:25):
      all 31 `terminal_` tests passed serially without Cargo. This covers the
      headless GPUI/PTY clipboard, input, mouse and lifecycle paths; native
      window resize/close and OS clipboard/menu rendering remain separate. On
      2026-10-09, the source-current app runner again passed all 31 tests,
      including edit-menu and editor-to-terminal clipboard round trips.
      Rechecked on 2026-10-12 by direct execution of
      `target/debug/deps/ahead_app-c52f10c9cb33e545` (built at 16:40, newer than
      `terminal.rs` at 07:40 and `terminal_panel.rs` at 09:25): all 35
      `terminal`-filtered tests passed. This includes real PTY paste-to-shell,
      OSC 52 clipboard, GPUI Edit-menu copy/paste, editor/terminal clipboard
      round-trip, shell-safe path paste, Ctrl+C interruption, and terminal
      cleanup. Native OS clipboard/menu rendering, resize/reflow and close/quit
      remain unverified while the desktop is locked.
      Revalidated on 2026-10-14 at 21:08 ET using the current app test runner
      built at 20:09 ET, newer than `terminal.rs` (Oct 8) and
      `terminal_panel.rs` (Oct 14 at 10:37): all 35 terminal-filtered tests
      passed serially, including real PTY paste, clipboard round trips,
      shell-safe paths, input/mouse handling and cleanup. No Cargo command was
      started; native OS clipboard/menu rendering and window lifecycle remain
      unverified while macOS is locked.
- [ ] Review the tracked root `alacritty.recording` before removing it from
      version control. Earlier terminal runs modified this file, and it may
      contain PTY output. AHEAD no longer enables Alacritty's reference-test
      recording, but the existing file has been preserved untouched.
- [ ] Polish the AHEAD agent panel with streamed Markdown/code rendering,
      follow-tail, cancellation, durable reopen/retry and terminal context.
      Assistant messages now use gpui-kit Markdown streaming fades and each
      fenced code block has its own copy control, following Zed's conversation
      renderer without adding a second Markdown parser. A focused GPUI source
      regression was added but not run. Native streamed rendering remains for
      the user's walkthrough.
      Terminal context is now an explicit `@terminal` or Add context attachment
      from the active PTY, bounded to 120 lines/16 KiB, removable in the
      composer, and labeled as untrusted context in the managed prompt. Native
      attachment rendering and a user-selected terminal-to-session turn remain
      for the user's walkthrough.
- [ ] Complete the single-human-plus-agent full-duplex journey under
      [Session voice calls](#session-voice-calls). Defer group audio until the
      single-user flow is reliable; existing shared text sessions remain separate.
- [ ] Keep all new editor chrome on GPUI/gpui-kit theme tokens and use familiar
      VS Code keybindings where they do not conflict with AHEAD actions.
- [ ] Defer remote/Kubernetes development, broad customization/profiles,
      arbitrary VS Code extension hosting and symbol outline until the core
      editor parity loop is working.

## Multiplayer collaboration

- [ ] Finish session code comments in `ahead-app/src/code_panel.rs` and
      `ahead-app/src/session_panel.rs`: gutter range rails, an editor side panel
      for reading and writing, resolution, the session-wide navigator, and clickable
      chat references are implemented locally. Changed files now relocate
      comments when their quote remains unique; ambiguous or missing quotes
      lose their gutter marker. Shared guests open referenced comments from the
      host buffer. Add diff-hunk anchors, persistent relocation when quotes
      change, explicit attachment to a linked implementation
      child and review dispositions. Directly shared session comments now
      synchronize and enforce participant roles. The host session panel now
      refreshes remote comments in the background while sharing; a headless
      two-window test proves a guest comment reaches the host navigator and
      guest poll. Verify the rendered multi-client comment journey. Before the
      2026-10-14 session-gating change, Add Comment was always visible as a
      labeled gpui-kit primary button and explained missing prerequisites in
      its tooltip; that behavior is historical, not current. Existing
      comment markers stay in the narrow gutter and use a persistent
      magenta-tinted surface with a high-contrast foreground and clearer
      hover/active states. A GPUI test checks
      visibility and feedback before selection/session, activation when ready,
      and breakpoint click-through. The focused Add Comment GPUI test passed
      against current source on 2026-10-12 (locked/offline Cargo, 1/1); this is
      headless layout/callback evidence only. A fresh `just dev` build also
      compiled the current app, but native startup again aborted before drawing
      with macOS HIServices connection errors (signal 6), so rendered acceptance
      remains open. In response to the user's 2026-10-12 screenshot, the gutter
      comment marker foreground now uses the theme sidebar foreground against
      the magenta tint; gutter dimensions and Add Comment toolbar placement are
      unchanged. The focused Add Comment GPUI test passed again (1/1) after this
      edit, followed by all 61 `code_panel::tests` passing from the rebuilt
      source-current app test runner. `just dev` rebuilt the production app after
      waiting on the shared Cargo lock, then GUI startup again aborted before
      drawing with the HIServices XPC error; its watcher session 20920 remains
      live. CUA confirms the Mac is locked, so pixel-level marker
      contrast is unverified. Native rendered acceptance remains open. The latest
      user screenshot shows only the small gutter plus; its crop does not include
      the file toolbar where the labeled primary action now lives. The current app
      binary (12:42) is newer than `code_panel.rs` (11:37), and the focused GPUI
      visibility/callback test passed 1/1 when rerun directly. Native-window
      attachment timed out on 2026-10-09, so verify the full live window before
      adding a duplicate gutter action or changing placement. Fresh visual
      acceptance remains open. The macOS
      `just dev` loop now polls on fresh startup after native events missed
      atomic source saves. Rechecked 2026-10-09: the stale watcher was replaced with
      one `just dev` loop targeting `/private/tmp/ahead-broad-smoke.im0WJM/project`
      under isolated `AHEAD_DATA_HOME`. The current app and Code Mode host build
      completed after adding the missing `Icon` and `Sizable` imports in
      `session_panel.rs`; direct launch reached the test project's
      language-server trust sheet. No trust choice was made, so native editor
      and Add Comment rendering remain open. The watcher was paused for serial
      GPUI/proxy tests and is currently stopped. Transient teaching
      presentation notes remain separate. On 2026-10-13, following the user's
      screenshot, restored a selected-range Add Comment control in the gutter's
      existing 26px footprint. It uses the gpui-kit primary button, an 18px hit
      target and the same comment-start callback as the labeled toolbar action;
      on the selected line it takes the action slot, while breakpoints remain
      clickable on adjacent lines. The focused GPUI regression passed (1/1) and
      the updated app compiled through `just dev` against
      `/private/tmp/ahead-dev-smoke.XPWP4f/project` with isolated
      `AHEAD_DATA_HOME=/private/tmp/ahead-dev-smoke.XPWP4f/data`. Native launch
      again aborted before rendering with the macOS HIServices XPC error
      (signal 6); visual contrast still needs a reachable native window. The
      single watcher (exec session 55653) remains live. After the user's
      follow-up that the gutter action was still hard to see, changed its
      persistent surface to the theme's filled magenta accent with a contrasting
      foreground. The shared dev build compiled successfully at 09:13 EDT; the
      app child again aborted before rendering, so native visual acceptance is
      still open. At 09:14 EDT, stopped watcher 55653 for the source-current
      `code_comment_action_is_visible_in_the_gutter_and_toolbar` GPUI test;
      it passed (1/1). Restarted the same `just dev` loop on the same disposable
      project and Turso data home as session 23365; its integrated build passed
      in 2.71s, then the app child again aborted before rendering with the
      HIServices XPC error (signal 6). CUA still lists `io.ahead.dev` as running,
      but attaching to its window timed out. Watcher 23365 remains live; native
      visual verification is still open.
      At 15:47 EDT, corrected the selected-line gutter action's clipping by
      fitting the primary Add Comment button to its 20px slot and reclaiming
      unused diff-rail spacing without widening the 26px gutter. The GPUI test
      now asserts the control stays inside the gutter and remains clickable;
      it passed against current source (1/1). `just dev` rebuilt in 5.82s, but
      macOS again aborted before drawing with the HIServices XPC error (signal
      6); CUA confirms the Mac is locked, so visual and pointer verification
      remain open.
      At 18:32 EDT, replaced the selected-line Add Comment action's weak primary
      outline with the existing filled magenta comment style and contrasting
      foreground, keeping its 20px hit target inside the 26px gutter. The
      source-current focused GPUI interaction test passed (1/1); the native
      appearance still needs a rendered-window check.
      Stopped the prior loop that targeted the AHEAD checkout and restarted
      `just dev` against the trusted disposable
      `/private/tmp/ahead-dev-smoke.XPWP4f/project` with isolated data at
      `/private/tmp/ahead-dev-smoke.XPWP4f/data`. The integrated build passed
      in 2.12s; macOS again aborted before rendering with the HIServices XPC
      signal-6 error. Watcher session 9162 remains on the disposable project.
      Revalidated on 2026-10-14 against a fresh disposable copy of
      `tests/fixtures/editor-smoke`: the current
      `code_comment_action_is_visible_in_the_gutter_and_toolbar` GPUI test
      passed (1/1). Restarted `just dev` with isolated data at
      `/private/tmp/ahead-broad-e2e.ikS0ar/data`; the integrated build passed
      in 14.05s, but macOS aborted before rendering with the HIServices XPC
      signal-6 error. CUA reports AHEAD Dev running, but app attachment timed
      out (-10005). Restarted the same loop after the test; watcher session
      37141 is live and its rebuilt launch reproduced the XPC abort.
      On 2026-10-14, editor comment creation is now unavailable without an
      active AHEAD session: toolbar, gutter and selection-menu affordances are
      gated, the action handler rejects direct calls, and displayed comment
      records/markers are restricted to the active session ID. The focused
      interaction regression and full `ahead-app` library suite pass (325
      tests; two local-socket tests filtered). The dev watcher is running on
      the disposable project, but native rendering remains blocked by the
      macOS HIServices XPC abort before window creation. The comment handler
      now additionally rejects stale callbacks unless their session ID matches
      the editor's current session; the focused GPUI regression passes (1/1).
      The Turso persistence regression now also asserts that a comment saved
      under one session is absent from another session's query; its focused
      proxy test passed (1/1).
      The completion-filtered suite passes (11/11) and signature-help placement
      and parameter documentation pass (1/1). The same `just dev` loop rebuilt
      the app in 2.44s on `/private/tmp/ahead-broad-e2e.ikS0ar/project`; macOS
      again aborts before window creation with the HIServices XPC error. Watcher
      session 24319 remains attached to that disposable project and data home.
      After a focused skill-loader test pause, restarted the same loop on
      `/private/tmp/ahead-broad-e2e.ikS0ar/project` with the same isolated data
      home; the build passed in 3.05s, then HIServices again aborted before
      rendering. CUA attachment still times out; watcher session 97961 remains
      live. Rechecked the source-current app runner (newer than
      `code_panel.rs` and `session_panel.rs`): all 64 editor tests and 54 chat
      panel tests pass. Coverage includes session-bound comment affordances,
      stale-session rejection, completion detail text, UTF-16 signature ranges
      through non-BMP characters, signature documentation/placement, and the
      searchable model/skill interactions. This remains headless GPUI evidence;
      the native app still aborts before rendering. Restarted the shared loop
      after the proxy test on the same project and Turso home; the integrated
      build passed in 7.60s, then the app again aborted at HIServices startup.
      Watcher session 56699 remains live.
      At 21:21 EDT, `ahead_app-0e8e9c4151d65c97` passed all 67
      `code_panel::tests`, including session-gated comments, completion
      signature details, signature-help placement/parameter documentation,
      Git-marker hit targets and above-caret completion. CUA still reports the
      Mac locked, so these are headless GPUI checks rather than rendered UI.
      Also on 2026-10-14, the current app binary passed agent-session shutdown
      smoke coverage for explicit shutdown, stdin EOF, SIGKILL recovery, durable
      partial output, failure state and exactly-once retry. DAP shutdown passed
      while running, initializing and emitting stderr; LSP shutdown passed for
      graceful shutdown, stdin EOF, initialization and an unresponsive server,
      with child processes reaped. The offline language-server smoke passed
      TypeScript, JavaScript and Python completion/navigation/diagnostic and
      recovery checks, including TypeScript import fixes. A subsequent run
      seeded with all cached servers completed successfully for Rust,
      TypeScript, JavaScript and Python, including restart recovery and graceful
      shutdown. The offline run used no provider and no network acquisition; it
      does not verify native rendered UI behavior. External ACP sessions are
      now excluded from editor comment context, and the proxy rejects comment
      list/create/resolve requests for external-agent bindings; focused app and
      proxy regressions were added but have not been run.
- [ ] Build a host-authoritative, cloudless multiplayer session for AHEAD chat,
      code and terminal surfaces. Share only from an active managed AHEAD
      session; its owner hosts the worktree, agent and provider account, so
      authorized client-started agent turns are billed to the owner. Resolve
      composer `@` mentions to current participant IDs: addressed messages
      stay human-only and out of agent replay/summaries/context; unaddressed
      messages may start a host-run agent turn under role policy. Persist the
      chosen message audience and show it before Send. Keep `@currentFile` and
      unresolved `@` text from changing the recipient. Read and adapt the pinned
      Zed checkout (`418f89714891f9d8105a3e92e60b9a7a5084d232`):
      `crates/collab/src/rpc.rs::{share_project,join_project,forward_mutating_project_request,follow}`,
      `crates/project/src/buffer_store.rs::handle_synchronize_buffers`, and
      `crates/project/src/project.rs::{set_role,disconnected_from_host}`. Zed's
      current channel-chat handlers reject messages; implement AHEAD routing in
      the session host. Keep AHEAD's session host,
      policy, attribution and workflow events authoritative; do not import
      Zed's cloud server, database or LiveKit topology.
      Direct TLS invitations, verified GitHub ID membership, host-run chat and
      agent turns, guest conversation and code comments, a team allowlist
      manifest, reconnect-on-poll and guest session-switch cleanup are implemented.
      `.ahead/team.toml` and its ignore exceptions are checked in; the team
      file is an empty example until the owner lists permitted accounts.
      The host rechecks the team manifest on every shared command, so removing
      an account rejects its next command and blocks reconnect.
      The guest panel receives a read-only snapshot of up to 500 lines of each
      host terminal's scrollback, bounded to the latest 64 KiB per update.
      Guests can open live host buffers; owner/editor tabs send debounced
      revision-checked edits, while reviewer/viewer tabs stay read-only. The
      host editor acknowledges incoming edits before its normal snapshots
      resume. Accepted incoming edits are durably recoverable before the host
      acknowledges them; the proxy rejects an edit if recovery cannot be
      persisted. Both editors preserve local text on a stale revision and offer
      explicit latest/local conflict choices. The host cannot stop sharing or
      archive while an incoming edit has not reached an open host editor.
      Connected participants now publish their active file and line, and the
      session panel offers opt-in follow that stops when the follower edits,
      plus a Return action to the previous file and line.
      Dirty guest tabs now write session-scoped recovery snapshots to the
      guest's private local store. Rejoining the same session restores drafts
      from a previous process and requires an explicit latest/local choice;
      accepted host edits also survive a host process restart. The direct TLS
      test covers guest proxy restart, rejoin and retained local draft data;
      focused GPUI tests cover draft restoration and conflict choice.
      An explicit Discard Changes clears the host's accepted-edit recovery
      after sharing stops and the edit is acknowledged; a failed cleanup
      prevents closing that buffer.
      The user chose automated tests only for now, so rendered two-app
      verification remains deferred. Verify
      simultaneous typing, reconnect, revocation, terminal lifecycle and
      connection cleanup in the UI. The direct TLS test covers competing
      revision-checked edits, role changes and terminal output clearing; a
      second proxy instance covers guest join, read, live edit, poll and leave.
      A two-window GPUI test covers host/guest editor convergence through
      separate app proxy clients. An integrated headless test now opens two
      full AHEAD Shell windows with separate authenticated host/guest proxy
      dispatchers and a local GitHub identity service; the guest joins over
      direct TLS, opens the host buffer, receives a later host edit through its
      live editor poll, edits it and observes the host editor converge, verifies
      Viewer read-only and Editor restoration without losing
      text, sends an addressed human-only chat message, creates a live
      comment visible in both session panels, reads the host's terminal and
      location presence, follows the host to its active file, and loses buffer
      access when the host stops sharing while retaining its local text in a
      read-only tab.
      The same test covers terminal output clearing and reopening; the guest
      panel now clears terminal output and presence on disconnection.
      Restoring Editor now updates the shared tab's status to live editing
      without hiding a pending edit conflict.
      That test found and fixed the host-presence RPC response mismatch in
      `ahead-proxy/src/dispatch.rs`. Rendered two-app interaction and the
      terminal/presence/comment controls remain to verify when native UI
      testing resumes.
      The source-current serial app suite passes all 333 tests on 2026-10-15,
      including `separate_ahead_processes_share_code_and_chat` and
      `two_authenticated_shells_join_a_direct_shared_session`. The old invite
      failures were stale fixtures entering JSON in the chat composer instead
      of the separate join input. These real-TLS/worker-thread fixtures now
      explicitly allow GPUI parking, matching Zed's real-I/O tests; no
      production sharing or Git behavior was changed to satisfy the scheduler.
      Keep rendered two-app interaction, native clipboard and live-session
      acceptance in this separate multiplayer milestone.
      A separate-process headless GPUI test now runs full host and guest Shells
      with independent authenticated proxies over direct TLS; it checks shared
      editor convergence, human-only chat, guest comment delivery to the host
      navigator, host terminal output, presence, follow and revocation. After
      revocation the guest loses host buffer access and retains its local text
      in a read-only tab. The guest also starts a host-run managed turn against
      a local mock model: the request uses the host-only provider key, the
      response reaches the host conversation, addressed chat is absent from
      model input, and a directly addressed turn is rejected. After that
      rejection closes the guest socket, the guest reconnects by polling and
      posts another human-only message before revocation.
      A GPUI test confirms unsynced guest text survives role loss and later
      edit access without silently choosing the remote version.
      Use a direct LAN/reachable-host transport with reconnect and stable
      authenticated participant identities. Share human/agent conversation,
      live code buffers, presence and revision-aware edits with explicit role
      policy. The host owns the worktree, agent effects and PTYs: participants
      may read terminal output but cannot send terminal input, resize, close or
      control host processes unless a later policy explicitly permits it.
      The checked-in `.ahead/team.toml` lists non-secret GitHub names, display
      names and default roles; the owner selects members per session, and
      invitation resolves GitHub names to verified identities. Acceptance
      coverage must prove
      host/participant chat and code convergence, reconnect/revocation,
      correct human-only versus agent message routing and host-paid turns,
      participant terminal read-only enforcement, role changes, and a complete
      local/direct session without AHEAD cloud infrastructure; document the
      reachable-host/NAT limitation.
      The local host resolves exact participant handles, persists addressed
      messages with recipient IDs, displays the composer audience, and gates
      agent turns by the current actor's session role. A rendered host/guest
      acceptance run remains open.

## Dependencies (Lapce-org removal; see decision 0006)

## Attribution & change tracking

- [ ] Verify dirty file tabs in a freshly rebuilt native app at narrow and
      wide tab widths. `ahead-app/src/code_panel.rs` now keeps the filename
      and dirty dot on one line, with the filename shrinking before the dot.
      The disposable-project native app showed the dot beside `main.rs` after
      an unsaved edit; narrow-width truncation remains to verify. A later
      isolated app with five open files showed `empty-smoke.txt` as the active
      editor path but not among the visible tabs; make the active tab visible
      when the tab strip overflows, without hiding its controls.
      Zed's pinned `crates/workspace/src/pane.rs` reveals the active tab when
      selection changes. gpui-component 0.6.6 already calls
      `ScrollHandle::scroll_to_item` from `TabGroupSkin::render_tabs` when the
      active index changes, so reproduce the narrow case against that tracked
      viewport before adding another app-level tab scroller.
- [ ] Verify `ahead-app/src/code_panel.rs` Git rail click, breakpoint-only
      hover target and tooltip in one freshly rebuilt native AHEAD process
      against the disposable multi-language project. The current gutter is 23
      logical pixels wide with a 20px breakpoint/action area and shared 3px
      indicator rail; the latest width/hit-target assertions were updated but
      not run. Earlier 26px-gutter test evidence below predates this change.
      The full 214-test app library suite passes with loopback-enabled local
      mocks on 2026-10-03. An isolated
      native copy opened the hunk card on Git-marker click
      without setting a breakpoint; breakpoint hover target, tooltip and
      narrow-gutter layout remain to verify; the Mac was locked during this
      native follow-up.
      Git metadata now waits for 300 ms after the latest edit, cancelling a
      pending request timer when the buffer revision changes. The new GPUI
      rapid-typing regression failed before the fix and passes after it;
      the existing marker-retention/stale-reply regression also passes. Verify
      marker stability during sustained typing in the rebuilt native app when
      the Mac is unlocked. On 2026-10-07, the cached `ahead_app` test binary
      (00:51:24) was newer than `code_panel.rs` (00:51:02); direct execution
      passed `git_change_marker_does_not_toggle_breakpoint` and
      `git_change_marker_previews_modified_and_deleted_hunks`. The first test
      checks the 26px gutter, separate Git/breakpoint hit targets, hunk-copy
      clipboard content, hover state and independent breakpoint toggling. This
      is headless interaction evidence; the breakpoint tooltip appearance and
      sustained typing still need native confirmation. The same source-matched
      test binary passed all 53 `code_panel::tests`; the suite exercises editor
      completion, navigation, rename, formatting, blame, external-file changes
      and language switching through headless/mock paths, not a live LSP server.
      On 2026-10-09, direct execution of
      `target/debug/deps/ahead_app-5b00065cae9bc61c` (12:20:14, newer than
      `code_panel.rs` at 11:37:41) passed all 61 `code_panel::tests`. This
      includes the labeled Add Comment action, separate Git/breakpoint hit
      targets, hunk preview, inline blame, and above-caret completion with its
      ranked five-item cap. These are current headless GPUI checks; live native
      rendering, pointer behavior and breakpoint-tooltip appearance remain
      open because the current AHEAD window cannot be attached.
      Revalidated on 2026-10-14 against the 20:09 app test runner, newer than
      `code_panel.rs` at 20:08: the Git metadata coalescing/stale-reply test,
      separate change-marker hit target and modified/deleted hunk preview all
      pass (3/3). This confirms debounce and retention under rapid edits in the
      headless harness; sustained native typing/marker stability and the
      breakpoint hover tooltip remain unverified while macOS is locked.
- [ ] Managed AHEAD runtime file-change completion now records `ahead` anchors from
      applied diffs. The managed-loop integration test now runs by default with
      an offline loopback Responses mock, including the startup /models request
      and retained apply_patch call; the focused test passed on 2026-10-06. It
      verifies the native file write, FileChange handling, AHEAD actor
      attribution and quoted content. Still exercise the rendered gutter and
      proxy GitCommit cleanup together. The
      recorder now anchors each added diff range only when its text still
      matches the post-edit file, and rejects paths resolving outside the
      workspace; renamed updates use the destination path. Focused agent tests
      cover multi-hunk edits, later human rewrites, traversal and moves. The
      focused dispatcher commit regression passed on 2026-09-29 (1 test,
      128 filtered): it
      commits an agent-authored file with AHEAD attribution/session trailer and
      clears the committed anchor rows.
      On 2026-10-07, `tests/agent-file-attribution-smoke.mjs` passed against the
      `target/debug/ahead` binary rebuilt at 22:31. Its loopback-only mock
      provider drove a managed Code Mode `apply_patch` turn; the disposable file
      changed and exactly one matching session/actor/path/quote CodeAnchor was
      read back from Turso. This verifies the managed edit-to-durable-anchor
      boundary, not the rendered gutter.
      External ACP remains explicitly unable to provide this guarantee.
- [ ] Verify the rendered AHEAD CodeAnchor gutter and real commit. The gutter's
      row projection now has a regression covering inclusive agent-authored
      ranges and exclusion of human anchors; it passed in a focused
      locked/offline run on 2026-09-28 (1 test, 60 filtered), and the current
      app test binary passed it again on 2026-10-06 alongside the
      nonblocking/stale attribution-refresh regression. This is headless GPUI
      evidence only. `created_at_commit`
      is removed from the AHEAD contract and schema. Old anchor schemas are
      rejected without conversion or row deletion; the database-preservation
      regression passes.
      A disposable native `gpt-5.5` loopback turn now made a real
      `apply_patch` edit; the editor rendered its violet pre-commit rail,
      and that rail cleared after the user-driven commit. Authenticated
      provider behavior remains unverified.
      On 2026-10-07, the 22:24 source-matched GPUI runner passed
      `agent_anchor_gutter_rows_include_the_full_range_and_ignore_other_actors`;
      the 18:43 proxy runner passed
      `dispatcher_commit_uses_anchor_attribution_and_clears_rows`. The latter
      verifies `ahead` author, human committer, sorted trailers for two sessions,
      matching-anchor cleanup and stale-anchor retention after a human rewrite.
      These fresh tests strengthen the separate Turso and UI/commit boundaries;
      they do not replace a fresh rendered gutter-to-user-commit run.
      Rechecked on 2026-10-08 with the current test artifacts: all 56
      `code_panel::tests` passed, including the agent-anchor gutter projection,
      inline blame, diff preview, and editor/LSP regressions. The source-current
      proxy dispatcher commit regression passed as well, covering `ahead`
      author, human committer, session trailers, staged-content attribution,
      pre-commit-hook rewrites and anchor cleanup. These are headless GPUI and
      local-Git checks; the rendered gutter-to-user-commit journey remains open.
      Latest direct run on 2026-10-08 from the 11:20 app test runner (newer
      than `code_panel.rs` at 06:22) passed all 57 `code_panel::tests` serially.
      The native breakpoint-tooltip and gutter-to-commit checks remain open
      while the desktop is locked.
      Re-ran `tests/agent-file-attribution-smoke.mjs target/debug/ahead` on
      2026-10-09 with its loopback-only fake Responses provider: Code Mode
      applied the edit, Turso returned the matching AHEAD CodeAnchor, and the
      staged Git commit used author `ahead`, retained the human committer and
      session trailer, then cleared the committed anchor. The sandbox denied
      the local listener on the first attempt; the isolated retry passed with
      loopback permission, made no external request, and removed its disposable
      project/profile. This remains headless synthetic-provider evidence; the
      rendered gutter-to-user-commit flow is still open.
- [ ] Verify the existing proxy `GitCommit` route against a real user-driven
      commit with live anchors; its git2 path is now unit-tested to set
      `author = ahead`, preserve the human committer, include the session
      trailer, and stage renamed destinations correctly. The dispatcher route
      now also has an end-to-end regression test for anchor attribution and
      cleanup. A 2026-10-02 regression exposed that any stale agent anchor on a
      selected path previously made a human-rewritten commit appear authored by
      `ahead`; it failed before the fix and passes now. Authorship and cleanup
      share the same quote/hash/content predicate, so a removed agent quote
      keeps the human author and leaves the stale anchor for explicit review.
      `GitCommit` is now an acknowledged request; Source Control sends only
      staged changes, shows failures without discarding the draft, and prevents
      duplicate submission. Proxy commit work now runs off the dispatcher
      thread; the focused dispatcher regression passed on 2026-10-03 and
      verifies the request returns while the agent host lock is held. The
      proxy computes attribution and cleanup from
      the staged index content, so unstaged human rewrites cannot change staged
      authorship; an anchor-store read error aborts the commit. Full library
      suites pass for RPC (15), proxy (195, 1 ignored; loopback-enabled) and
      app (176), including proxy-client request/result and narrow-footer
      coverage.
      In an isolated app and disposable Git project, the synthetic provider
      issued `apply_patch`, Source Control staged its sole changed file, and
      the native commit completed. Git showed `ahead` as author, the configured
      human as committer, and one `Ahead-Session` trailer; the violet rail
      disappeared after commit. This verifies the rendered single-session
      path, not authenticated models or hooks/signing. Anchor-cleanup
      errors after a successful commit are logged but not yet repairable in UI.
      A staged changeset with matching anchors from multiple AHEAD sessions
      now records one sorted `Ahead-Session` trailer per distinct source
      session; the dispatcher regression failed before this fix and passes
      with two sessions and repeated anchors. Verify that multi-session
      metadata in a rendered user-driven commit journey.
- [ ] Verify the Zed-style Git command commit path in the running app.
      `ahead-proxy/src/dispatch.rs` now invokes `git commit`, so repository
      hooks and signing configuration run normally; AHEAD attribution remains
      the author and configured Git identity remains the committer. After the
      command succeeds, anchor cleanup diffs the actual commit tree against its
      parent and only clears anchors whose quoted content still matches the
      committed blob. Disposable regressions cover hook rewrite attribution,
      hook failure, signing failure, staged-content retention, invalid HEAD,
      rename, and multi-session trailers (6 focused commit tests pass on
      2026-10-03). The full serial `ahead-proxy --lib` suite passes 221 tests
      (1 ignored) with loopback enabled. Verify configured hooks/signing and
      failure presentation in a user-driven Source Control commit when the
      native app is available.

## Agent chat panel (`ahead-app/src/session_panel.rs`)

- [ ] Verify the managed reasoning picker in a fresh native session. Its options
      now come through the session host from the active model's
      `ModelInfo::supported_reasoning_levels`, unsupported saved levels fall
      back to the model default, and selections persist per provider/model in
      `~/.ahead/settings.toml`. The model and effort controls share one compact
      GPUI-kit popover: the trigger shows model/provider/effort, and the popover
      contains the searchable model combobox plus the GPUI-kit effort slider.
      The trigger's accessible label includes the selected model, provider and
      reasoning effort so the compact visual control retains its full meaning.
      A focused unit regression covers populated and loading-state labels; it
      has not been run against the current source.
      The same popover now remains stable when the selected model has not
      loaded or does not advertise reasoning levels; only the model row appears
      until effort controls are available. A GPUI regression covers both states.
      External ACP model choices now place advertised reasoning choices in the
      same searchable model popover using the GPUI-kit slider, with the selected
      effort shown in the trigger; a UI regression exercises the advertised
      effort change. These new tests have not been run against the current source.
      Its focused layout regression asserts the effort, model row, and slider
      ordering; the latest native rendering check remains open.
      The provider-search GPUI test now checks the single-popover behavior before
      and after reasoning metadata arrives. Focused persistence and capability-
      selection tests remain unrun after this change; native rendering is open.
      On 2026-10-20, restored the Codex-style hierarchy: the compact trigger
      opens one effort-first gpui-kit popover, and its selected-model row opens
      the searchable model list. This keeps effort and model choice together
      without putting the slider below the model results. The provider-search
      GPUI regression now checks that order and refresh-on-open behavior. The
      focused test has not run against this edit; narrow-window and fresh native
      rendering also remain unverified.
      On 2026-10-20, the managed picker trigger was compacted from 190px to
      160px without splitting the model/effort popover. Rust formatting, diff
      whitespace, and the UI detector pass; the focused GPUI and native layout
      checks remain open because no app test runner is present.
- [ ] Recheck model-first provider labeling in a fresh native window after
      unlocking macOS. `ahead-app/src/session_panel.rs` now hides the legacy
      top-level model when a named connection has models; the exact disposable
      project config passes a focused deduplication test. The collapsed picker
      now keeps the model name first and shows its serving connection alongside
      in muted, width-capped text with a full-name tooltip; menu rows also show
      the model first with the connection muted beneath. The loading-state GPUI
      regression now checks that the selected provider label is rendered and
      bounded; the focused GPUI test and all five `model_picker` tests pass on
      2026-10-03. Confirm one `deepseek-coder`
      row and the provider label at narrow and wide composer widths in a fresh
      native window; the last visible window belonged to older AHEAD processes,
      and the Mac locked before the post-fix UI check.
      A 2026-10-05 observation in the running AHEAD Dev window on the disposable
      `ahead-app-readiness` project now confirms the selected `gpt-5.5` row puts
      the model first and shows `OpenAI-compatible server` as muted secondary
      text; the accessible label includes both. The exact one-row
      `deepseek-coder` config and narrow/wide composer widths remain unverified.
      On 2026-10-07, the app test runner rebuilt at 14:04:03 (newer than
      `session_panel.rs` at 13:51:56) passed all 44 `session_panel::tests`,
      including model discovery/loading, provider-scoped visibility, ACP option
      selection, slash/skill filtering and insertion, composer interaction, and
      rendered chat controls. The composer model `Select` is configured with
      gpui-kit's searchable mode, which renders a search field atop the opened
      model list; verify its visible/focused behavior in the same native check.
      These are headless GPUI checks; native behavior remains open. A direct
      rerun from the source-matched app test runner at 18:27:17 on 2026-10-07
      passed all 47 `session_panel::tests`, all 28 `settings_panel::tests`, and
      the nested model-list scroll containment regression; native menu focus
      and scrollbar appearance remain pending app launch.
      The chat model `Select` now labels its search field "Search models or
      providers…". A focused GPUI interaction opens the picker, focuses its
      search field, types a provider name, and selects that provider's model; a
      companion regression covers matching both model and provider names. The
      source-matched app runner passed all 49 `session_panel::tests` and all 6
      `terminal_panel::` tests at 19:37 on 2026-10-07. Native rendering and
      search-field focus still need the fresh-window check above. A focused
      source-current rerun of `chat_model_select_filters_by_provider_name`
      passed on 2026-10-07, confirming gpui-kit search-field typing and filtered
      model selection in the headless GPUI harness; the native check remains
      open.
      Native recheck on 2026-10-08 in the rebuilt app and approved disposable
      multi-language project found that `SelectState<Vec<ModelSpec>>` showed
      unfiltered rows while its search text changed. The picker now uses
      gpui-kit's `SearchableVec`, and the GPUI interaction regression asserts
      nonmatching rows disappear. Sixteen model-related tests pass; native
      search for `scroll-test-099` showed only that model, and provider-name
      search returned matching rows without changing the current selection.
      The app's proxy was disconnected, so no provider request was made.
      A source-newer direct rerun on 2026-10-08 (app test runner built at 11:20,
      after `session_panel.rs` at 09:36) passed all 53
      `session_panel::tests` and all 29 `settings_panel::tests`. Coverage
      includes searchable model/provider names, empty-session actions,
      provider-scoped hidden models, settings scroll containment, stale model
      discovery and the inline-blame default/persistence. These are GPUI tests;
      native provider authentication remains a separate acceptance gate.
      Native managed-chat recheck on 2026-10-08 after switching to gpui-kit's
      `Combobox`: the search field rendered above the bounded list in the
      approved 355-model disposable workspace; entering `scroll-test-003` left
      exactly that model visible. No model was selected and no inference
      request was sent. The ACP thread uses agent defaults; this check covers
      only the managed AHEAD picker. Narrow composer sizing and authenticated
      provider turns remain separate checks. Source-current focused rerun on
      2026-10-09 confirmed the provider query hides OpenRouter and leaves
      Anthropic visible, but that runner still reported OpenRouter selected
      after clicking the row. Follow-up direct runs from
      `target/debug/deps/ahead_app-5b00065cae9bc61c` (12:20:14, newer than
      `session_panel.rs` at 12:19:40) passed both
      `chat_model_select_filters_by_provider_name` and
      `model_selector_searches_model_and_provider_names`. The interaction test
      asserts the click selects Anthropic/`claude-sonnet`, then reopens without
      duplicate discovery. This supersedes the earlier failed result for
      headless GPUI selection; native rendering/focus and an authenticated
      provider request remain open. On 2026-10-09, the same source-current app
      runner passed all 54 `session_panel::tests`; the model-picker loading,
      provider search, slash palette, and composer checks all passed. Two
      focused settings regressions for provider-scoped visibility and nested
      model-list scrolling also passed. Fresh native rendering remains pending
      the host LaunchServices issue recorded above. Rechecked on 2026-10-12
      directly from `target/debug/deps/ahead_app-c52f10c9cb33e545` (built at
      16:40, newer than `session_panel.rs` at 14:17): all 54
      `session_panel::tests` and all 29 `settings_panel::tests` pass. Coverage
      includes model/provider search and discovery, per-provider model
      visibility and scroll containment, slash-skill filtering/insertion, ACP
      option selection/auth, composer behavior, and settings persistence. These
      remain headless GPUI checks; native menu focus and provider authentication
      are open. On 2026-10-13, the freshly rebuilt app test binary passed the
      focused `chat_model_select_filters_by_provider_name`,
      `model_selector_searches_model_and_provider_names`,
      `model_visibility_models_stay_scoped_to_collapsible_providers`, and
      `model_list_scroll_does_not_scroll_settings_page` tests (1/1 each).
      These verify searchable selection, provider-local visibility and wheel
      containment at the component level; native scrollbar, focus and narrow-
      width appearance remain open while the Mac is locked.
      Revalidated on 2026-10-14 from the app runner built at 16:37, newer than
      `session_panel.rs` and `settings_panel.rs`: the `model_` filter passed
      16/16 tests, covering model/provider search, per-provider visibility,
      discovery/reload, ACP model options and list scroll containment. The
      actual Shell empty-panel routing regression also passed (1/1), including
      clicks while session startup is pending. These remain headless checks.
      At 20:49 ET, the source-matched app runner also passed the searchable
      provider picker, provider-scoped collapsible visibility, scroll
      containment, slash-token filtering, skill metadata filtering/insertion,
      and keyboard navigation/escape tests (1/1 each). These headless checks do
      not verify native menu focus or appearance.
- [ ] Verify the settings-to-managed-provider path in a running GPUI app. Settings
      supports editable per-provider model catalogs and authenticated OpenAI-
      compatible `/models` discovery; stale discovery responses are discarded
      after switching connections, editing the endpoint/key, or starting a newer
      lookup. The managed picker refreshes every configured connection at
      startup and after settings reloads, appending discovered models while
      keeping the configured catalog as an offline fallback. Opening or
      reopening the picker starts one discovery pass; typing in its search field
      does not start additional lookups. A GPUI interaction regression covers
      startup/open/reopen, search and selection.
      The 2026-10-15 source now refreshes on the searchable popup's open
      transition and keeps that pass coalesced through filtering and rerenders.
      After stopping the shared watcher, the source-current locked/offline
      `ahead-app` GPUI test `chat_model_select_filters_by_provider_name` passed
      (1/1). It verifies discovery at session startup, a fresh pass on each
      picker open/reopen, no duplicate pass from rerendering or filtering, and
      provider-name search/selection. The watcher is being restarted on its
      disposable project. A native-window bind timed out; rendered picker
      behavior remains unverified.
      Per-connection
      `hidden_models` preferences default to empty (show all); Settings groups
      each provider's configured and discovered model ids in a collapsed,
      virtualized list with provider-specific visibility switches. Focused tests
      cover provider grouping, fallback/discovered catalog merging, catalog
      normalization, response parsing, and stale success/error rejection;
      the provider-row, per-connection hidden-model parsing, and stale-discovery
      tests passed in focused offline runs on 2026-10-06. `ignores_stale_model_discovery_results`
      passes with `rtk proxy cargo test --locked --offline -p ahead-app --lib
      ignores_stale_model_discovery_results -j 2`. On 2026-10-07, a fresh
      locked/offline build compiled the app, RPC, proxy and extension-host
      library tests. Source-matched serial runs passed all 27
      `settings_panel::tests` and all 7 `model_picker` tests. Settings
      gives each provider a fixed disclosure row and its own GPUI `uniform_list`
      with a 280px viewport and fixed-height rows. Connection setup and
      per-provider visibility share one AI Integration settings page. The
      gpui-kit scrollbar shows
      position while the provider label stays visible during model scrolling;
      the redundant Settings page/content headings are removed. The chat
      picker's gpui-kit `Select` uses a `List` backed by `v_virtual_list` and a
      bounded menu. Both avoid rendering the full model catalog at once. The
      component-level GPUI test
      `settings_panel::tests::model_list_scroll_does_not_scroll_settings_page`
      passed for the earlier scroll implementation: wheel input over the
      virtual model list changed its offset without reaching the page scroll
      handler. A subsequent user report identified remaining page movement, so
      the model viewport now stops the same wheel event from reaching the page,
      including at the model-list limits. A locked/offline rebuild on
      2026-10-07 passed that regression against the latest source; serial runs
      then passed all 27 Settings tests and all 7 model-picker tests. A
      subsequent rendered `just dev` run on the disposable 100-model fixture
      confirmed wheel input over both a model row and its switch advances only
      the virtual list; more wheel input at the final model leaves the Settings
      page fixed. The source-current GPUI regression and fresh app render pass.
      At that point, provider-toggle behavior and narrow-width rendering
      remained open; the later wide-window toggle result is recorded below.
      On 2026-10-07, directly reran
      `model_visibility_rows_are_provider_scoped_and_collapsed_by_default`
      from `ahead_app-bcbfa40f10585a2b` (05:07:54); the settings source
      (00:37:14) is older than that test binary, and the regression passes.
      Settings saves now stage
      private files and replace them atomically; a focused regression covers
      overwrite permissions and symlink rejection (2026-09-29). Exercise discovery,
      save/select, credential handling, and
      new/restored managed sessions end to end. External ACP remains
      adapter-default. In a 2026-10-02 disposable native run, editing the
      selected mock connection URL saved it to private settings, but a turn in
      the already-loaded managed session retried the previous endpoint until
      the app restarted. After restart, Retry reached the new endpoint. The
      managed `prompt` boundary still fails closed if called directly with a
      stale provider. The AHEAD session controller now refreshes the idle
      native harness before the next turn and reopens its durable thread. A
      two-endpoint offline regression confirms rotated endpoint and key, prior
      model history, and persisted messages. It refuses to switch while
      another managed turn uses the old harness. Verify the full Settings
      save-to-next-turn journey in a running native app, including discovery,
      selection, credential handling, and restored sessions; authenticated
      provider access was still unverified at that point. A rebuilt `just dev` bundle rendered
      the corrected reconnect notice in Settings for the disposable
      `/private/tmp/ahead-settings-smoke.ZkUwEc` project on 2026-10-02; no
      Settings-to-provider switch was exercised in that window. A subsequent
      disposable run saved a loopback Base URL, fake API key, and model catalog
      through the live Settings fields and read them back from private settings;
      the new-thread menu interaction did not complete, so no live model turn
      or connection rotation was proven by that run. The disposable mock server
      also received an authorized `/models` request and Settings displayed
      “Discovered 1 model(s)”. In a fresh 2026-10-02 native build, the API key
      rendered as bullets with a secure accessibility field; Show/Hide worked,
      and the discovery status fit below its buttons. At that time, a live
      managed turn and Settings-to-provider rotation remained unverified.
      Managed `/models`
      refresh eligibility now follows configured provider credentials (API-key
      environment variable, bearer token, command auth, headers, or cached/
      external auth) and permits unauthenticated discovery for optional-auth
      providers; focused coverage was added but still needs the shared Cargo
      test loop. On 2026-10-07, a live rebuilt `just dev` run in the disposable
      `/private/tmp/ahead-model-scroll-e2e.0bpFnT/project` saved the user-
      supplied OpenRouter credential in private settings, discovered 467
      models, and completed a managed session turn with
      `nvidia/nemotron-3-ultra-550b-a55b:free` (`AHEAD_OK`). The composer
      initially selected the first configured provider, the intentionally
      unavailable loopback fixture, so the discovered OpenRouter model had to
      be selected explicitly. The UI also warned that metadata for this
      discovered model was missing and used fallback metadata. The temporary
      OpenRouter connection was then removed in Settings and its key field
      cleared. Model-metadata resolution and restored-session rotation remain
      to test. In the same 100-model fixture, hiding `scroll-test-001` removed
      it from the chat picker and changed the selected model to
      `scroll-test-002`; re-enabling it restored its visibility. Scrolling over
      the virtualized rows advanced the model list while the provider heading
      and Settings page stayed stationary. Narrow-width rendering remains open.
      On 2026-10-13, the source-current app test binary passed all 29
      `settings_panel::tests`, including provider-scoped visibility, nested
      scroll isolation, stale-discovery rejection and fresh-settings bootstrap.
      Native layout/trackpad acceptance remains open.
      At 21:22 EDT on 2026-10-14, the source-matched app runner (20:09, newer
      than `settings_panel.rs` and `session_panel.rs`) again passed all 29
      Settings tests, plus the provider/model-name search and per-settings
      visibility picker regressions (1/1 each). These are headless GPUI checks;
      native model-menu focus and trackpad appearance remain unverified.
- [ ] Validate the end-to-end installed-agent path for the curated Pi, Codex and
      Claude Code ACP adapters against Zed's registry/install lifecycle. The
      catalog is intentionally not arbitrary ACP; installation is user-local
      and the supported npm package is resolved lazily on first launch, with a
      pinned fallback when the registry is unavailable. Verify package install
      errors/status, model and reasoning selection on new/restored sessions,
      live config-option updates, streaming and cancellation in a running GPUI
      app. On 2026-10-15 the strict public-registry protocol probe passed:
      AHEAD's background refresh supplied Pi 0.0.34, Codex ACP 2.2.2 and Claude
      ACP 0.91.0; installed provenance matched those entries. All three created
      sessions, exposed model choices, accepted a change and saved their own
      per-agent defaults. Authentication failures no longer pass as skipped
      model verification. This used disposable homes/caches, isolated npm
      config, disabled lifecycle scripts and synthetic keys, with no model
      prompt. Native install/reconnect, picker rendering and authenticated
      turns remain open. The final-source agent suite passes 169 tests (four
      opt-in ignored); the strict registry test passed separately in 32.07s.
      Config-option completion now checks the active session and request
      generation so a delayed result cannot overwrite a newer selection's
      status; `config_option_results_ignore_older_requests_and_other_sessions`
      covers that stale-result predicate and re-passed in the current worktree
      on 2026-10-03 with `rtk proxy cargo test
      --locked --offline -p ahead-app --lib
      config_option_results_ignore_older_requests_and_other_sessions -j 1`.
      At that point Pi provider access had not been supplied; the live-turn
      follow-up below supersedes that gate. On 2026-10-02, the ignored
      `pi_registry_adapter_round_trips_offline_session` test installed the real Pi ACP
      adapter into disposable storage, initialized a session with `PI_OFFLINE=1`,
      selected a different advertised model through `session/set_config_option`,
      and observed the updated option without sending a prompt. After stopping
      that adapter process, it re-resolved the installed adapter, loaded the
      saved model default, reopened the session, and observed that model again.
      It passed with
      `cargo test --locked --offline -p ahead-agent --lib
      pi_registry_adapter_round_trips_offline_session -- --ignored --nocapture` using
      an isolated npm cache. This proves neither a provider-backed turn nor
      model selection in the running editor.
      Follow-up with the real `pi-acp@0.0.34` package in an isolated,
      credential-free environment reached ACP `initialize` but `session/new`
      returned authentication required and advertised `pi_terminal_login` with
      the `--terminal-login` command. No prompt or provider request was sent.
      On 2026-10-08, AHEAD added typed ACP error preservation, `authMethods`
      parsing, method-specific `authenticate`, and terminal-login launch using
      the installed adapter's executable, base args/env, and advertised method
      args/env, and auth-required codes survive the proxy RPC boundary. The chat
      renders the advertised sign-in choices; terminal
      login uses AHEAD's integrated PTY and reconnects only after exit status 0,
      preserving the unsent message for retry. This follows Zed's ACP auth
      selection/reinitialization path in `../zed/crates/agent_servers/src/acp.rs`
      and `../zed/crates/agent_ui/src/conversation_view.rs`. Focused auth
      protocol, proxy cache, GPUI button-dispatch, and direct PTY args/env/CWD
      tests passed, as did all-target checking of the changed crates. The wider
      ACP test filter had 38 passes and five local-socket tests blocked by the
      sandbox's `Operation not permitted`; live Pi login and native rendered
      terminal auth remain unverified. At that time, provider access had not
      yet been confirmed; the live-turn follow-up below supersedes that gate.
      On 2026-10-08, with Node v24.15.0 and npm v11.12.1 on PATH, AHEAD's real
      proxy listed and installed its curated Pi, Codex and Claude Code adapters
      in isolated profiles, then prepared external ACP sessions. With
      `PI_OFFLINE=1`, Pi advertised `pi_terminal_login`; Codex surfaced its
      expected authentication-required state; Claude Code completed session
      preparation. No provider credentials were passed and no model prompt was
      sent. Disposable profiles were removed. This verifies install and ACP
      preparation only, not model/config options, authenticated turns,
      streaming/cancellation or rendered sign-in. The reused proxy binary was
      built at 10:22, before the 12:02 Cargo.lock update; repeat after the next
      shared build.
      Current 2026-10-12 checks supersede the missing model-option and install
      evidence above: the rebuilt `ahead-agent` runner passed
      `pi_registry_adapter_round_trips_offline_session` using synthetic Pi
      models and an unreachable localhost endpoint, verifying selection and
      resume under `PI_OFFLINE=1`. It also passed
      `curated_acp_agents_initialize_with_isolated_homes`, which installed the
      three curated adapter packages, launched their CLI-backed processes with
      isolated homes, and completed ACP initialization without prompts or
      credentials. This does not verify Codex/Claude session config options,
      an authenticated model turn, streaming/cancellation or native auth UI.
      On 2026-10-12, the source-current `ahead-agent` test runner's
      `adapters::tests` filter passed 14 tests covering the curated catalog,
      install isolation/concurrency, package and executable path safety,
      config-default persistence, and safe install-cache cleanup. Four tests
      were ignored by default. The locally packed npm fixture was separately
      run with local Node/npm and passed, using disposable packages offline.
      The installed Pi/Codex/Claude ACP initialization and Pi offline
      session/resume tests were separately run with credential-free isolated
      homes and passed as recorded above. That offline pass did not attempt a
      provider-backed Pi turn; the authenticated follow-up below supersedes
      that gap.
      Rechecked on 2026-10-13 against `ahead_agent-0e1e93ffd7e62eb3` (11:54,
      newer than `adapters.rs` at 20:36 on 2026-10-12 and `acp_client.rs`):
      16 active adapter tests passed, 4 opt-in tests were ignored, and four
      focused ACP regressions passed for model selection, early config-update
      ordering, live option republishing, and boolean options. These use local
      synthetic peers and do not exercise native controls or another provider.
      On 2026-10-12, the source-current ignored
      `pi_registry_adapter_completes_an_acp_turn` test passed (1/1) with the
      user-authorized OpenRouter model. It created a disposable Pi home and
      `models.json`, selected the provider-qualified model ID, and received a
      streamed `PONG` AgentDelta through the curated `pi-acp` adapter. The key
      was supplied only through the test process environment; no user Pi
      profile was read or changed, and the temporary workspace was removed.
      This verifies one Pi ACP turn, not cancellation/resume or Codex/Claude
      authenticated turns. The installed ACP bridge launched the separate Pi
      v0.87.1 CLI and announced v1.2.0 as available; the bridge package does
      not pin or install the Pi CLI.
      A source-current rebuilt `ahead-app` test runner then passed all 35
      serial terminal-filtered tests, including keyboard/Edit-menu clipboard
      round-trips, shell-safe path paste, OSC 52, Ctrl+C, and the terminal-login
      exit callback. The callback regression now draws the GPUI terminal view
      before waiting for its asynchronous PTY exit event. This remains headless
      GPUI/PTY evidence; native clipboard and rendered terminal-auth behavior
      are still blocked by the locked desktop.
      On 2026-10-04, Pi CLI v0.87.1 printed help with an isolated `HOME`. The
      npm cache contains `pi-acp` 0.0.33/0.0.34 tarball entries, but offline
      install returned `ENOTCACHED` for package metadata and direct packing
      returned `EPERM` while creating a temp file in the root-owned shared
      cache. No package install or ACP session resulted at that time; provider
      access was later supplied and the live ACP turn passed as recorded above.
      The live adapter installer now
      directs npm to AHEAD-owned `external_agents/registry/npm-cache` instead
      of the global npm cache. The focused
      `acp_install_uses_an_ahead_owned_npm_cache` regression passes in this
      checkout. The built-in language-server installer uses its own AHEAD-owned
      npm cache as well.
      ACP-side shell, file and permission effects remain adapter-owned and
      cannot receive AHEAD `CodeAnchor` enforcement. On 2026-10-07, the running
      disposable app opened **New External Agent Thread** and rendered the
      curated Pi, Codex and Claude Code choices with their Install actions.
      This verifies the chooser only; no install or external process was
      started during this offline pass. Rechecked offline on 2026-10-07 22:58
      ET: the source-current `ahead-agent` test runner's ignored
      `npx_generations_launch_locally_packed_packages_offline` passed by
      packing/installing local npm archives and launching both versions with
      `npm --offline`; the current app test runner passed all 50
      `session_panel::tests` and 14 `threads_panel::tests`, including ACP model
      option changes, update acknowledgements, picker recovery and adapter
      install error/stale-response handling. These are local package and GPUI
      harness checks, not native installation of Pi/Codex/Claude Code or a live
      provider-backed turn. The live Pi gate remains closed pending provider
      access.
      Rechecked on 2026-10-08 against the 00:21 `ahead-agent` test runner,
      newer than the relevant agent and retained-runtime sources: the local
      npm-package generation test passed again using `npm --offline`. Fifteen
      focused offline regressions also passed for hierarchical instruction
      loading, the curated ACP catalog and saved defaults, ACP model selection
      and updates, shared file-search context, paged skills, and selected editor
      context. The Pi CLI tests were skipped: the offline-session test requires
      public npm and an installed Pi CLI, while the live-turn test also requires
      Pi auth. No registry or provider was contacted.
      On 2026-10-08, the source-current `ahead-agent` runner (00:21:43, newer
      than `adapters.rs` and `acp_client.rs`) passed all 14 non-ignored
      `adapters::tests`; three opt-in Pi/local-npm cases remained ignored. This
      includes the exact three-agent allowlist, install-before-launch,
      user-local install state, saved config defaults, cache generations and
      npm-cache isolation. No external agent or registry was contacted.
      Rechecked 2026-10-07 23:04 ET: added and passed the fresh-workspace
      `first_settings_load_bootstraps_private_workspace_config_once` regression;
      all 29 Settings-panel tests and all 29 terminal-filtered app tests passed
      offline. The latter exercise GPUI-routed clipboard shortcuts through the
      disposable shell PTY and copy selected output back to the test clipboard.
      `just dev` rebuilt the app for the approved disposable project in 11.53s,
      then macOS again aborted before rendering on the locked desktop's
      `hiservices-xpcservice` connection error (signal 6). Native OS clipboard
      and menu interaction, first-open rendered settings and other live UI
      acceptance therefore remain unverified; the dev watcher is left running
      on that disposable project.
      Rechecked offline on 2026-10-12 with the existing
      `ahead_agent-503e45a14d926542` runner (14:53, newer than `adapters.rs`,
      `acp_client.rs` and `native_client.rs`): 14 curated-adapter tests and
      four focused auth, terminal-login and cancellation tests passed. This
      covers the Pi/Codex/Claude allowlist, install gating, per-agent saved
      defaults, ACP auth-method handling and settling a pending editor
      permission request on cancellation. The three npm/provider-dependent
      adapter tests remained ignored; no external package or provider was used.
      The ignored `npx_generations_launch_locally_packed_packages_offline`
      test also passed (1/1), packing two local adapter versions and
      installing/launching both through an isolated npm cache with `--offline`.
      Rechecked on 2026-10-12 using the later source-current
      `ahead_agent-b1c7a7aff42dd683` runner: 14 curated-adapter tests passed,
      three provider/npm-dependent cases stayed ignored, and the local-package
      `npx_generations_launch_locally_packed_packages_offline` test passed
      separately. This confirms install gating, per-agent defaults and offline
      local-package launch, but not a provider-backed ACP turn or rendered UI.
      Fresh clean-build ACP recheck on 2026-10-14: the source-current
      `ahead-agent --lib` suite passed 166 tests (4 ignored). The ignored
      `curated_acp_session_model_options_offline` integration then passed for
      Pi, Codex and Claude Code using temporary homes and public npm packages;
      all three completed ACP session creation and model-option selection.
      Direct `--nocapture` rerun emitted no Codex synthetic-auth early-return
      diagnostic. The test directs child HTTP proxies to an unreachable local
      endpoint and uses only synthetic keys. The fresh Pi
      `pi_registry_adapter_round_trips_offline_session` test also passed with
      `PI_OFFLINE=1`, restoring the selected model after ACP session reload.
      These verify install/protocol/config/resume behavior without provider
      prompts; native chooser/picker rendering, provider-backed cancellation,
      and Codex/Claude authenticated turns remain open.
      Revalidated at 21:14 ET on 2026-10-14 with the existing
      `ahead_agent-61f82332d76d33b8` runner (17:44, newer than the ACP client,
      adapter and session-panel sources): all 43 `acp_client::tests` passed
      serially with ephemeral loopback peers. Coverage includes advertised
      model/provider and boolean config options, saved defaults, live option
      updates, permission review, streaming deltas, cancellation and shutdown.
      This did not install external agents or exercise native controls or
      authenticated Codex/Claude turns.
      At 21:19 ET, the source-matched `ahead_app-0e8e9c4151d65c97` runner
      (20:09, newer than `session_panel.rs` and `threads_panel.rs`) passed all
      four ACP-filtered session-panel interaction tests and all 14 thread-panel
      tests. This covers advertised auth/model option actions, update
      acknowledgements, picker/install error recovery, and stale adapter/session
      replies in the GPUI harness; native installation and reconnect rendering
      remain unverified.
- [ ] Verify native managed runtime fidelity on multi-tool turns, compaction,
      unsaved-buffer reads, unmapped runtime events, and model-specific tool
      selection against the pinned runtime. The managed `/compact` composer
      action now submits the retained runtime's `Op::Compact` and consumes its
      streamed completion. The AHEAD-owned `native_agent_streams_a_responses_api_turn_in_process`
      regression passes against an ephemeral loopback mock and was re-run on
      2026-09-30 in the full agent suite (100 passed, authenticated Pi ignored).
      On 2026-10-19, OpenAI-compatible catalog `supported_parameters` now flows
      into model metadata. Models explicitly lacking `tools` receive an empty
      direct tool plan; absent capability metadata preserves existing behavior.
      Catalog parsing and production-router regressions are added but were not
      run in this turn.
      It also verifies that a selected project skill body reaches the model
      request. `native_agent_ignores_runtime_mcp_and_telemetry_with_path_preflight`
      passes, confirming runtime-home MCP declarations stay disabled. On
      2026-10-06, direct execution of the current `ahead-agent` test binary
      passed `native_agent_streams_code_mode_only_without_unavailable_host_tools`,
      `native_agent_cancel_interrupts_an_active_child_before_the_parent`, and
      `acp_permission_waits_for_review_and_cancels_with_the_turn`. These tests
      needed permission for loopback-only mocks; no external provider or
      credentials were used. This is headless runtime evidence, not rendered
      permission/cancellation UI or authenticated-provider proof. These offline
      checks do not replace an authenticated running-GPUI turn; verify
      both there. Run that UI journey in a disposable project with
      `just dev /absolute/path/to/disposable-project`, never in a real user
      project. The same AHEAD-owned loopback regression now runs manual
      `/compact` after a streamed answer, checks the actual compaction model
      request and harness-store replay checkpoint, then reopens the managed thread;
      it passes with the focused `ahead-agent` test on 2026-10-02. This is
      offline boundary evidence, not authenticated or rendered compaction proof.
      The focused buffer-snapshot round-trip now invokes native `file_search`
      and verifies unsaved editor text replaces saved disk content in a
      disposable workspace (2026-10-02). An offline managed `gpt-5.5` turn
      now issues `file_search`, receives that editor snapshot, and returns the
      unsaved match to the next model request. On 2026-10-02 its first
      cold-link rerun timed out before the buffer request; an immediate retry
      and six warm repeats passed. The test now reports the prompt outcome and
      model-request count on that timeout. Investigate if it recurs; this is
      not evidence of authenticated provider reliability. In the running GPUI
      app, a disposable `gpt-5.5` loopback turn advertised direct editor tools; the
      fake model issued `file_search` and `read_editor_buffer` together.
      Search found the Python marker, while buffer read returned its expected
      closed-file error. Both results reached the next model request, and the
      UI showed the completed/failed tool activity and final response. A
      uniquely identified test bundle then created a fresh thread in a separate
      disposable project, opened `python/main.py`, and typed an unsaved marker.
      In a `gpt-5.5` loopback turn, `read_editor_buffer` returned that marker to
      the next model request; the UI showed the completed tool and reply, while
      the disk file still lacked the marker. A later `gpt-5.5` loopback turn
      invoked `apply_patch`, returned the successful tool output to the model,
      and changed only `python/arithmetic.py` in that disposable Git project.
      A real-provider edit-effect turn remains open.
      The same isolated GPUI app submitted `/compact` to the loopback provider
      and received the retained runtime's checkpoint response. It previously
      rendered an empty-agent-response placeholder; successful explicit
      compaction now stores and shows "Context compacted.". A rebuilt-app run
      rendered that notice, and it survived a process restart. Authenticated
      provider and cancellation/retry compaction checks remain open.
      Directory-first CLI launch now selects that directory as the
      workspace without opening it as a file; the focused startup regression
      passes. On 2026-10-01 the rebuilt `just dev` app created and restored a
      managed session in the disposable project. The first read-only send
      exposed a missing expected policy hash in `ahead-app/src/session_panel.rs`;
      restoring the active `SessionView` on every attach path fixed that gate.
      The same prompt then appeared in chat and entered streaming;
      Stop produced a cancelled turn and a Retry control, both still visible
      after a fresh `just dev` rebuild and durable-session restore. The selected
      `deepseek-coder` model reported missing metadata and a network reconnect,
      so no authenticated answer or tool calls were observed. Verify rendered
      retry, compaction and multi-tool turns once
      a reachable provider is configured. Also verify the composer retains a
      draft when a proxy startup error rejects a turn.
      A 2026-10-02 `just dev` run in the disposable project saved a loopback
      provider URL and fake key through Settings, completed the two-step
      AHEAD-thread wizard, selected `gpt-5.6-sol`, and restored that thread
      after a process restart. A follow-up native run with the same disposable
      project used the Agent shortcut to focus the composer, sent an answer-only
      turn to the loopback provider with its fake key, rendered the streamed
      response, and restored both messages after another process restart.
      The focused GPUI regression covers the shortcut before and after a thread
      exists; the status-bar Agent button now shares that focus path. macOS
      accessibility still did not expose the labeled composer as a focused
      element, so screen-reader access remains unverified. This fake-provider
      first turn did not verify tool use, a real provider, retry or compaction
      UI; the subsequent `gpt-5.5` direct-tool probe is described above.
      Durable session title/objective, intent, phase, linked issue, work items,
      recent events and summaries now reach the native model prompt through a
      separate host-populated `session_context`, never by rewriting the user's
      message. New turns discard caller-supplied context; retry requests retain
      their persisted snapshot. Focused regressions verify the actual first
      native model request and exact retry DTO persistence, including a slash
      command remaining at the start of the prompt. Authenticated rendered
      behavior remains open.
      On 2026-10-05, a read-only `file_search` + `exec_command` turn was started
      in a separate disposable native session using the configured
      `gpt-5.5` OpenAI-compatible model. The app entered Streaming, then showed
      `Reconnecting… waiting for network`; no tool calls or answer were
      observed. Auto-retry was left active. Do not count this as authenticated
      multi-tool acceptance.
- [ ] Durable messages are now persisted (`conversation_messages` table) and
      streamed into the panel. The panel renders headings, unordered/ordered
      lists, blockquotes, tables, emphasis, inline code, fenced code blocks and
      validated HTTP(S) inline links; rendered link behavior and the complete
      Markdown UI still need a running-window pass. A running-window pass on
      2026-09-23 verified durable sidebar hydration, active-session restore and
      the composer/model/context controls across two clean app restarts. It also
      verified that archiving a long-titled test thread remains hidden after a
      restart while preserving its durable record. One unreadable legacy
      session is now quarantined instead of hiding every healthy session.
      A 2026-09-30 disposable-project native-window pass also restored a newly
      created empty session after process termination; it did not exercise
      message-history replay.
      Latest plan, tool-card, token-usage and advertised-command state now has
      a Turso presentation snapshot for session restore; reasoning text and
      pending input payloads are intentionally excluded. A file-backed reopen
      regression test passes; panel restore still needs a running-window pass.
      Checkpoint export/restore now carries that same presentation state, from
      the recovered ACP audit fix; verify the restored plan/tool cards in GPUI
      as well. The file-backed export/import/reopen regression passed in the
      141-test proxy suite on 2026-09-30. The archived-snapshot inventory and
      validation are recorded in
      `docs/development/zed-tracking.md#archived-agent-audit-recovery-2026-09-30`.
      Visible chat restore/refresh now requests indexed 50-message keyset pages;
      "Load older messages" prepends earlier pages while retaining the current
      virtual-list scroll anchor. The native runtime's ordered replay history
      remains complete and separate from this UI projection. The focused
      `conversation_message_pages_use_exclusive_keyset_and_recover_stale_streams`
      regression passes with `rtk proxy cargo test --locked --offline -p
      ahead-proxy --lib
      conversation_message_pages_use_exclusive_keyset_and_recover_stale_streams
      -j 1`; it re-ran on 2026-09-29 (1 test). Verify the page/anchor behavior
      in GPUI. A clean-cache focused
      `ahead-app` test took about five minutes to reach Rust diagnostics; the
      workspace target cache is now populated. Keep validation focused and use
      only one shared Cargo build loop.
- [ ] Conversation history now uses a tracked GPUI `ScrollHandle` with an
      explicit Latest/Follow control and auto-follow during streamed updates;
      verify the control, manual scrolling, and jump-to-latest behavior in a
      running GPUI window.
- [ ] Enter-to-send is wired via `subscribe_in` on `InputEvent::PressEnter` but
      unverified in a running app. Confirm Shift+Enter inserts a newline.
- [ ] The `session_panel` -> proxy turn path now has a live managed streaming
      roundtrip, durable failed-turn persistence, explicit interrupted-turn
      retry and durable reopen verification. Still drive an authenticated turn
      through the stop button, verify rendered failure details and complete the
      provider/model setup path in that same running GPUI window.
      Revalidated 2026-10-08 against the current `target/debug/ahead` with
      `node tests/agent-code-mode-smoke.mjs target/debug/ahead`: a disposable
      OpenAI-compatible SSE fixture completed a managed Code Mode cell and
      delegated `file_search`, cancelled a second turn while editor snapshots
      were held, reopened the Turso-backed session after proxy shutdown, then
      explicitly retried and completed it. No external provider or credentials
      were used; this closes the current-binary managed controller/store smoke,
      not authenticated provider or rendered stop-button acceptance.
      If the worker thread cannot be created, the controller now marks its
      message failed, streams the startup error, releases only the matching
      active-turn entry, emits the final state and retains the saved request
      for retry. `worker_spawn_failure_persists_error_and_keeps_turn_retryable`
      now injects an `io::Error` at that spawn-result boundary and checks the
      durable failure detail/status, notifications and retained retry request.
      The expanded focused test passes with `rtk proxy cargo test --locked
      --offline -p ahead-agent --lib
      worker_spawn_failure_persists_error_and_keeps_turn_retryable -j 1`
      (2026-09-29; 1 test).
      The regression injects the OS spawn error rather than exhausting process
      threads; it asserts that the matching slot is released and a stale
      failure cannot clear a replacement turn for the same session.
      `ahead-agent-engine/core/src/session/mod.rs::persist_rollout_items`
      logs append failures without failing the retained loop. The archived
      restore probe exposed this blind spot: chat completed while runtime
      history was missing the new turn. AHEAD's Turso thread store now latches
      its first replay or metadata append failure and rejects later writes in
      that process;
      `NativeClient` requests an interrupt on its next event, returns the
      persistence error without waiting for the model, and refuses
      another prompt, compaction or resumed child turn until restart, warning
      the user to review any tool effects before retrying. Focused store and
      stalled local-provider regressions pass. After the interrupt change, the
      full agent library suite passed with serial test execution (114 active,
      2 ignored), and `cargo build --locked --offline -p ahead --bin ahead -j 1`
      passed. Three existing socket-driven tests timed out in the first
      parallel suite, then each passed alone and the complete parallel rerun
      passed (114 active, 2 ignored). The turn controller now streams its
      harness error delta to the open chat as well as persisting it; the
      focused chat-state regression passes, all 114 active agent tests
      re-pass serially, and the app binary rebuilds. Verify the
      failed-turn status and retry
      request in the rendered GPUI chat; the 2026-10-01 isolated app attempt
      was stopped when the Mac locked, and its disposable database was restored
      without the injected trigger. Verify that a model tool call already in
      flight is cancelled or visibly attributed after a durable write failure;
      do not claim a failed turn is safe to replay automatically.
- [ ] Verify startup workspace selection in a running app. macOS LaunchServices
      starts the bundle with `/` as its working directory, so AHEAD now asks for
      one directory when no file/workspace argument was supplied and the cwd is
      a filesystem root. The selected path is canonicalized and checked as a
      directory; cancelling exits instead of opening `/`. Focused startup and
      GPUI prompt-selection/cancellation tests pass. A rendered macOS smoke app
      selected `/private/tmp/ahead-agent-smoke.zskmnC` on 2026-09-29; verify the
      native cancellation path and normal packaged build. The smoke bundle
      remained running after automated Cmd+Q and window close; verify with a
      physical shortcut and add a Quit binding if reproducible. The same
      behavior recurred in the 2026-09-30 temporary schema-test bundle;
      only the specifically identified test processes were stopped for restart.
- [ ] Verify atomic session/harness creation in the disposable native app.
      `StartWork` now validates a selected curated ACP adapter's local install
      marker before writing, inserts the full session and initial harness
      binding in one immediate Turso transaction, and publishes active/voice
      state only after commit. A database-trigger failure regression confirms
      rollback leaves no session or active state and retry creates exactly one
      correctly bound session; unsupported adapters are rejected before any
      write. The full `ahead-proxy` library suite passes (182 tests). Still
      exercise the selected managed and installed-agent paths through the
      rebuilt `just dev` app and verify session restore.
- [ ] Finish live verification of the W17 implementation handoff with a
      launched ACP agent: send a delegated turn and return to the parent with
      partial changes and review still pending. Child nesting and return
      navigation survive app restart in a disposable project. The handoff
      blocks open unsaved buffers, but parent and child still share one
      checkout; add conflict handling or isolation before supporting
      simultaneous edits to the same files.
- [ ] Let each session/thread optionally bind to a Git branch, created from a
      chosen base or selected from existing branches. When a user switches to a
      thread bound to another branch, activate that branch too; leave unbound
      threads on the current branch. Preserve dirty work: use a separate
      worktree for concurrent branch-bound sessions or stop before a checkout
      that would overwrite changes, and never stash or reset without an explicit
      user action. Cover branch assignment, thread switching, session restore
      and concurrent sessions.

## Harness integration (new 2026-09-18)

- [ ] Resolve the optional code-mode host boundary in
      `ahead-agent-engine/code-mode/src/remote_session.rs` and
      `ahead-agent-engine/core/src/tools/code_mode/mod.rs`: AHEAD now
      looks for `ahead-code-mode-host` beside its executable. The provider's
      `availability()` checks the adjacent regular file and, on Unix, its
      execute bit before `TurnContext` propagates that state. The focused
      non-executable-host regression passes. Unavailable
      `ToolMode::CodeMode` falls back to direct tools only when the in-process
      fallback is allowed; `CodeModeOnly` remains fail-closed. Verify the
      model-facing tool specs and worker omit unavailable Code Mode (especially
      `CodeModeOnly`) in a native turn as an interim safety check. The upstream
      Deno archive still returns 404 for sandboxed V8 150.4.0, but OpenAI's
      `rusty-v8-v150.4.0` release supplies the exact archive and Rust binding.
      Both macOS arm64 assets were downloaded and matched the SHA-256 values
      in OpenAI's build manifest. `ahead-code-mode-host` and
      `ahead-code-mode-runtime` now distill the pinned `rust-v0.152.0` source
      into this root workspace. The unused copied gRPC client, proto generator,
      transport dependencies and tests are removed; the managed path is stdio-only.
      The host and runtime library tests pass (13 and 68); an AHEAD-owned
      process smoke test executes both a JavaScript cell and a nested delegated
      tool through the managed stdio client. Non-null `max_heap_size_bytes` now
      fails closed at host open and direct runtime execution instead of being
      silently discarded. Per-session heap limits remain unsupported because
      the pinned V8 hard limit can terminate the shared host process. The
      rejection tests remain unrun after this change. The AHEAD-owned
      process-level regression in `ahead-agent/tests/code_mode_host_recovery.rs`
      runs through `just test-all` on macOS/Linux. It kills the real host during
      a yielded cell, asserts the active-cell error surfaces, then checks that
      the same logical session completes a later cell on a newly spawned host.
      These changes remain unverified: Cargo stopped while archiving
      `libstarlark` with about 220 MiB free. Preserve shared build artifacts
      and rerun after disk space is safely recovered. `just dev`,
      `just test-all` and the
      macOS bundle recipes now prepare checksum-verified V8 and build the
      sidecar beside the editor, but their complete app/package paths have not
      been exercised. CI now builds the editor and sidecar together on macOS
      and Linux and runs the sidecar unit/process smoke tests on Linux; those
      jobs have not run yet. Windows still builds the editor alone because this
      pinned V8 release has no configured Windows sandbox archive. Add a
      Windows host before advertising Code Mode models there. After reclaiming
      disk space from rebuildable compiler caches and duplicate test binaries,
      combined locked/offline editor-and-host `cargo check` and `cargo build`
      both pass on macOS arm64 after the gRPC prune. The host/runtime suites
      pass again (13 and 68),
      as does the delegated-tool stdio smoke test; the remaining retained
      Code Mode client/protocol tests pass (68). AHEAD's managed config no
      longer disables the default-on `CodeModeHost` feature, which had made the
      new sidecar unreachable; its regression and the full `ahead-agent`
      library suite pass (148 passed, three ignored). The missing-host warning
      now names `ahead-code-mode-host`. The 2026-10-03 cold `just dev` build
      produced the editor and sidecar, but GUI launch exited with signal 6
      after macOS reported an invalid `hiservices-xpcservice` connection while
      the desktop was locked; rendered Code Mode remains unverified. Verify
      authenticated CodeModeOnly model requests, delegated tool effects and
      cancellation in the rendered app, verify app-bundle signing, and test the
      Linux and universal-macOS recipes before claiming those models work.
      `tests/agent-code-mode-smoke.mjs` now passes against the built editor and
      sibling host in a disposable project with a loopback mock provider. Its
      `gpt-5.6-sol` turn advertises `exec`/`wait` in Responses Lite's
      `additional_tools` item, executes a JavaScript cell that delegates to
      AHEAD `file_search`, returns the search hit and `42` to the next model
      request, persists a completed chat and shuts down. A second cell waits
      on an editor snapshot; explicit cancellation persists `cancelled` without
      another model request. After a clean proxy shutdown and Turso reopen,
      the cancelled status is restored and explicit retry runs the cell and
      continuation successfully. This proves the headless managed path, not
      authenticated or rendered behavior; repeat with a real provider and
      verify the signed bundle. A fresh combined editor/host build passed on
      2026-10-03 after reclaiming ignored Cargo outputs; the current-source
      headless smoke re-passed against that binary. The post-cut agent library
      suite also passes (149 passed, 3 ignored). After the cold rebuild and
      pinned V8 cache restore, the Code Mode smoke passed again on 2026-10-03
      for delegated search, cancellation and retry after Turso reopen.
      On 2026-10-06, the exact shared `just dev` build and sibling host passed
      `rtk node tests/agent-code-mode-smoke.mjs target/debug/ahead`: delegated
      file search returned into model continuation, cancellation prevented a
      late snapshot reply, Turso restored the cancelled turn, and explicit
      retry completed; the script removed its disposable workspace/profile.
      On 2026-10-07, reran the smoke against `target/debug/ahead` (14:55:55),
      newer than `settings_panel.rs` (14:54:07) and `session_panel.rs`
      (14:55:43); synthetic-provider tool call, delegated search, cancellation,
      Turso reopen/retry and shutdown passed. The shared `just dev` app also
      launched in the disposable 100-model project and rendered Settings and
      nested model-list scrolling, but no chat turn was sent in that window.
      This is native startup/UI evidence plus headless Code Mode evidence, not
      rendered managed-chat or authenticated-provider proof.
      On 2026-10-07, `target/debug/ahead` (19:37:34) and the sibling
      `ahead-code-mode-host` (16:33:04, newer than all host Rust sources) passed
      the smoke again with a disposable loopback mock model. Managed `exec`/
      `wait`, delegated AHEAD file search and continuation, cancellation
      without a late snapshot, Turso reopen/retry, and clean shutdown all
      passed; the script removed its successful workspace/profile. This is
      current headless runtime evidence, not authenticated-provider or
      rendered-chat verification.
      On 2026-10-07, the 21:21 `just dev` build passed
      `tests/agent-code-mode-smoke.mjs` again: managed `exec`/`wait`, delegated
      AHEAD file search, cancellation without a late editor snapshot, Turso
      reopen/retry, and clean proxy shutdown all passed with the local mock
      provider. The script removed its temporary workspace and profile. This
      is headless runtime evidence only.
      Rendered and authenticated
      Code Mode behavior remains unverified.
      Do not substitute direct tools without a deliberate
      model-mode decision. Native runtime
      warnings now persist separately from ephemeral reasoning in the latest
      turn presentation state and appear in the panel warning area after
      restore. A disposable native run with `gpt-5.6-sol` and an unavailable
      loopback provider confirmed the missing-host warning above the chat,
      separate from Thinking, through cancellation and app restart. A real
      provider-backed turn and rendered host/tool execution remain unverified.
      `register_code_mode_executors` now gates `exec`/`wait`; the router gates
      its worker on `TurnContext.code_mode_available`. The AHEAD-owned native
      regression uses bundled `gpt-5.6-sol` metadata and checks the actual model
      request, including the valid omission of `tools` when none can be
      advertised. A 2026-10-02 native turn against a loopback fake provider
      returned and restored an answer before the host was added, but its
      `gpt-5.6-sol` request still had
      no top-level tools and the missing-host warning remained visible. This
      was a pre-host historical failure. The 2026-10-03 headless Code Mode
      smoke above now verifies delegated execution, cancellation, retry and
      Turso restore; authenticated provider use and rendered app execution
      remain release gates. Planner coverage checks fail-closed mode
      does not launch a worker. The focused native regression and full agent
      library suite pass on 2026-10-01 (110 passed, two ignored). This verifies
      fail-closed tool omission; the headless smoke above covers synthetic
      Code Mode execution, not an authenticated native turn. Do not depend on
      a Codex installation layout.

- [ ] Keep regression coverage for retained native-loop behavior in AHEAD-owned
      test targets; do not repair or restore the copied `codex-core` / `codex-mcp`
      upstream unit-test harness.
      A 2026-10-15 inventory found 109 test-named Rust files under
      `ahead-agent-engine/core/` and `ahead-agent-engine/codex-mcp/`, whose
      library test targets are disabled. Port the cases that protect AHEAD's
      active model/tool contracts into `ahead-agent` tests, then remove the
      obsolete copied cases and fixtures; do not delete them wholesale before
      the needed coverage is identified. The latest `codex-core --lib` attempt
      failed before filtering/running tests (299 errors, 189 warnings) on removed
      test-support crates/dependencies and stale product APIs; an earlier
      attempt had 383 errors and 196 warnings. On 2026-10-03,
      `cargo test --locked --offline -j1 -p codex-mcp --lib` failed before
      execution (32 errors, 10 warnings) on removed upstream-only test crates
      and Tokio `test-util`; an earlier copied `codex-mcp` test attempt had
      119 errors and 8 warnings. These
      copied-harness failures are not product test results or release gates.
      `just test-all` explicitly runs maintained AHEAD package targets plus
      retained patch-library security and fixture tests; copied runtime test
      harnesses are outside that recipe, while their production libraries
      still compile as dependencies. The full recipe passed on 2026-10-02.
      The copied `codex-rmcp-client --lib` test target also fails before running
      (73 missing stale fixture/dependency errors); do not restore its removed
      test dependency graph just to run that target.
      The obsolete standalone patch CLI and its CLI-only tests were removed;
      73 patch-library tests now pass through the in-process API. Copied
      `codex-model-provider` tests also depend on removed ChatGPT/shared-state
      fixtures and are not product gates. `cargo check --locked --offline -p
      codex-core --lib -j 1` passed. Keep removed product fixtures
      and dependencies pruned; port only still-supported behavior that lacks
      AHEAD-owned coverage. Keep subagent context isolation and durable resume
      covered through the AHEAD host; MCP elicitation/approval coverage must
      likewise use that path rather than reviving the copied suite.

- [ ] Finish the native-runtime closure cut. The 2026-09-30 decision is that
      backward compatibility with retired Codex features is not required;
      do not retain aliases, fixtures or runtime branches just for old formats.
      On 2026-10-01 the retained source still had 65 Cargo manifests under
      `ahead-agent-engine/`; a 2026-10-02 normal-dependency
      `cargo tree -p ahead` audit found all 65 in the executable's closure,
      so none is a safe orphan-directory deletion. It is one root
      workspace, not a nested one, but the native dependency closure is still
      far larger than AHEAD's intended surface. Remove whole crates only after
      tracing their current runtime callers and preserving the model-facing
      loop, sandbox, persistence and MCP behavior actually used by AHEAD.
      The two callers of `codex-response-debug-context` now use its unchanged
      error-context logic in `codex-api/src/error_context.rs`, removing that
      standalone crate. The AHEAD-owned `ahead_error_context` integration target
      passes all three tests, and a locked offline `ahead-agent --lib` check
      passes through `codex-api`, `codex-model-provider`, and `codex-core`.
      The full `ahead-agent --lib` suite after the cut passes (149 passed,
      3 ignored) with an isolated temporary profile and loopback access.
      The copied `codex-api --lib` test harness still fails on removed upstream
      dev dependencies; it is not a product gate. Locked Cargo metadata now
      resolves 64 retained runtime-source packages, with no remaining manifest
      or Rust reference to the removed crate.
      A fresh 2026-10-03 comparison of root metadata and `cargo tree -p ahead`
      confirms all 64 retained runtime-source packages are in the normal
      executable dependency closure; `ahead-agent-engine/` has no nested Cargo
      manifest. Further package pruning requires removing an active dependency
      edge and its behavior, not relocating the source subtree.
      Rechecked on 2026-10-14 with locked/offline metadata and the normal/build
      Cargo tree: all 53 runtime-source packages are reachable from `ahead`, so
      there is no package-level orphan deletion to make. This does not rule out
      dead modules inside active crates.
      Current 2026-10-20 worktree removes the duplicate nested
      `ahead-agent/runtime/source` tree (1,628 deleted paths). Locked/offline
      Cargo metadata resolves one workspace at the repository root with 67
      packages; all 52 packages under `ahead-agent-engine/` appear in the
      `ahead` binary's normal dependency tree, and a repository search finds no
      remaining `runtime/source` references. This verifies workspace
      membership and dependency reachability, not compilation or runtime
      behavior; a source-current build and tests remain open. `cargo machete`
      reports no unused dependencies, `cargo fmt --all -- --check` passes, and
      `git diff --check` is clean. Only 2.5 GiB is currently free, so no build
      was started.
      Retained Linux bubblewrap diagnostics now use AHEAD wording and no longer
      claim a bundled fallback that the code does not provide. The sandbox
      crate declares its existing `pretty_assertions` and `tempfile` test-only
      dependencies. macOS excludes these Linux tests; cross-compiling their
      target here is blocked by the missing `x86_64-linux-gnu-gcc`, so execute
      them on Linux CI.
      A 2026-10-02 `cargo shear --locked --offline` pass removed the unused
      direct `libc` dependency, unused workspace `semver` declaration, an
      unlinked placeholder Work Items panel, and three unlinked Bazel-only
      exec-server fixtures. The checker now reports zero unused-dependency
      errors; `cargo check --locked --offline -p ahead -j 1` passes. This is
      source hygiene, not a measured build-speed improvement or a native-loop
      closure reduction.
      On 2026-10-07, read-only locked/offline Cargo metadata confirmed all 54
      retained runtime-source packages remain reachable from the root `ahead`
      package, so none is an orphan-package deletion. `cargo shear` found six
      unused workspace dependency declarations (`axum`, `clatter`, `quick-xml`,
      `tokio-stream`, `tonic`, `tonic-prost`); these were removed from the root
      manifest. Its rerun reports zero dependency errors and 10 existing
      doctest/disabled-upstream-test warnings. No Rust build was run, and this
      declaration cleanup is not a measured compile-time improvement.
      A current reverse Cargo tree shows `starlark` enters through
      `codex-execpolicy`, which remains active in shell and network policy,
      approvals, world-state, and child turns. It is not a safe orphan delete.
      If replacing that parser/evaluator with an AHEAD-owned allow/deny/confirm
      engine, use Zed's `crates/agent/src/tool_permissions.rs` as the behavior
      reference and preserve sandbox, prefix-rule, network and child-turn
      security tests before measuring any build-size win. The root profiles
      already use limited debug info for development and tests; changing them
      would cold-rebuild the shared target.
      Managed sessions now explicitly disable the retained Codex hook feature;
      the legacy `notify` config, turn-completion runner and hook payload types
      are removed. The focused managed-config test and normal AHEAD build check
      pass (2026-10-02); the isolated `just dev` binary also rebuilt, without
      rendered interaction. A copied `codex-hooks --lib` test build was stopped
      before linking when its cold dependency graph reduced free disk to 2.4
      GiB; it did not run. Four exact test executables were removed from
      `target/debug/deps` to restore the build loop; they are rebuildable.
      Continue pruning unused hook discovery/config code without touching MCP,
      skills or Git commit hooks. The unused runtime-only
      `bypass_hook_trust` override and misleading startup warning are removed;
      the retained hook engine receives `false` from AHEAD config. The
      `ahead-agent --lib` production check passes on 2026-10-03. This does not
      remove the hook engine or its active core call sites. The zero-caller
      `ThreadManager::refresh_hook_runtimes` entry point is also removed; the
      same agent check passes. Remaining hook calls are integrated with
      session start/stop, tool dispatch, approvals and compaction, so remove
      those branches together with behavior tests rather than orphaning them.
      On 2026-10-04, the retained agent-lifecycle hook crate, dispatch DTOs,
      output parser, tool-output feedback wrapper, MCP input echo and approval-
      only command attribution were removed with their core call sites. Normal
      MCP approval and skill behavior remain; network MITM hooks and user Git
      hooks are unrelated and remain. `cargo check --locked --offline -p
      codex-core --lib -j 1` passes. The filtered tool-output test build still
      fails in the copied upstream test harness on removed test-support
      crates/APIs, before tests run; keep AHEAD-owned tests as the product gate
      rather than restoring that harness. After rebuilding `ahead` for the
      sandbox helper and allowing loopback-only test servers, the maintained
      `ahead-agent --lib` suite passes: 156 passed, 3 ignored (2026-10-04).
      The zero-caller App Server client-info setter and legacy instruction-path
      conversion entry points are removed from the retained thread/session API;
      focused `codex-core` and `ahead-agent` library checks pass. Client-info
      fields still feed retained metadata and require a separate reachability
      audit before removal.
      The retained core's dead TUI config writers, reader/keymap, projected
      fields and copied TUI tests are removed; model/tool collaboration-mode
      choices remain as active agent behavior. AHEAD's GPUI editor settings
      and keybindings stay outside the managed runtime config. The focused
      strict-config regression, `cargo check --locked --offline -p ahead -j 1`,
      and isolated `just dev` relink pass (2026-10-02); rendered interaction
      remains unverified while the macOS session is locked.
      The copied nested toolchain, Cargo config and standalone Windows setup
      script are removed; the needed Windows flags live in `.cargo/config.toml`.
      A 2026-10-05 source scan found no live Guardian or credential-broker
      implementation and no rusqlite/SQLx SQLite persistence path; Turso/libSQL
      remains the durable session store. Remaining Guardian mentions are
      negative tests for retired feature keys plus a test path, so keep those
      rejection tests. The separate browser/computer-use `auto_review` policy
      is not a Guardian execution path. Retired Guardian feature keys are no
      longer in the schema whitelist; managed feature requirements now report
      `auto_review` and `guardian_approval` as unknown rather than silently
      accepting them. Strict-config regressions cover root and profile feature
      tables. The copied feature-alias registry and top-level
      `experimental_use_unified_exec_tool` setting are removed; strict config
      rejects retired aliases while canonical feature keys remain available.
      The 29 unused removed-stage feature entries and their ignore branches
      are deleted; strict config rejects those keys. The default-on
      `unified_exec_zsh_fork` gate had no AHEAD settings path, so it is removed
      from feature registration, shell-mode selection and tool-plan fixtures.
      Zsh composition remains guarded by `shell_tool`, `unified_exec`,
      `shell_zsh_fork` and Zsh selection, preserving the former default. Strict
      config rejects the retired key. `codex-features` passes 19 tests and the
      focused retired-feature strict-config test passes. The copied
      `codex-tools` test target still fails to compile on unrelated removed
      MCP/plugin APIs and missing `pretty_assertions`; its focused shell-mode
      regression therefore cannot run until the test harness is replaced with
      AHEAD-owned coverage. The two remaining removed-stage Windows flags
      (`experimental_windows_sandbox`, `elevated_windows_sandbox`) still have
      runtime readers. They feed `WindowsSandboxLevel::from_features` and the legacy
      `features.*` fallback in
      `ahead-agent-engine/core/src/windows_sandbox.rs`; replace that
      fallback with AHEAD-owned typed sandbox settings before deleting the
      flags. Do not weaken the Windows sandbox or silently accept the retired
      keys.
      Unused Codex plugin command attribution fields and their event plumbing
      are removed; they had no production producer. AHEAD CodeAnchors are
      independently produced from verified file-change deltas by the session
      controller. The copied plugin-relative path validator/tests,
      recommended-plugin context test and plugin URI schema example are removed.
      Continue replacing remaining compatibility types, remote
      execution-environment compatibility and analytics providers with
      AHEAD-owned settings, editor search/filesystem,
      approval, MCP, skills/instruction and memory seams.
      The AHEAD `ThreadManager` now constructs an explicitly local-only
      environment manager, so managed sessions ignore runtime-home
      `environments.toml` and inherited remote exec-server environment
      variables. The copied TOML/environment-variable discovery and bootstrap
      path is removed, with a focused regression for the remaining local-only
      environment set. Generic remote transport still remains in the retained
      runtime and needs deliberate pruning and locked test coverage. On
      2026-10-04, the unused `environment_toml` and `environment_bootstrap`
      modules, Codex-home/environment-variable discovery constructors, and
      their discovery-only tests were removed; explicit remote transport
      fixtures remain. The AHEAD-unused environment-health query and planned
      executor-replacement/refresh API are removed, including their status RPC
      and copied socket tests. Its replacement-only cancellation/retirement
      state is also gone; ordinary connection startup and recovery remain.
      The unreferenced accepted-WebSocket manager/client adapter, its
      replacement handoff and focused tests are also removed. A 2026-10-05
      source audit confirms the retained execution backend has no generic
      remote URL/Noise transport: `EnvironmentManager` creates only the local
      executor, and the execution crate documents that it has no remote
      client/server or relay. Its unused Noise-auth re-export is now removed;
      the protocol keeps that variable only in the child-environment secret
      scrubber. Do not confuse Responses WebSockets/remote compaction or Code
      Mode's separate worker boundary with remote execution. The remaining
      cleanup is the multi-environment selection/configuration shape and
      remote-ID-only test branches; simplify those without losing local
      workspace config, MCP readiness, or environment-scoped permission and
      sandbox behavior. The unused public
      `EnvironmentManager::upsert_environment` URL-registration API, its private
      replacement/validation helpers, and remote-only tests were removed. The
      MCP cache regression now exercises sanitization, stale-fetch ordering and
      expiry against the local environment; the unsupported remote-generation
      case is no longer a product test. Deferred Noise provisioning/readiness
      state, its nested transport, and the Noise rendezvous connection/retry
      API were unused by the product and are removed; persisted thread roots
      and owner-configured roots remain. The active AHEAD executor uses one
      local environment; the remaining `TurnEnvironmentSelection` list and
      per-environment configuration lifecycle still need an AHEAD-owned
      reduction. On 2026-10-04, the default-off `DeferredExecutor` feature,
      model-visible `wait_for_environment` tool/configuration, remote-startup
      prompt fragment and rollback marker, and feature-controlled nonblocking
      snapshot switch were removed. Pending local configuration remains
      nonblocking, and MCP readiness is separate. Locked offline
      `cargo check --locked --offline -p ahead-agent --tests -j 1` passes. The
      stale streamable-HTTP remote-exec test and its Codex exec-server process
      launch/WebSocket fixture helpers are also removed. The copied
      `codex-rmcp-client --tests` targets still depend on intentionally removed
      fixture tooling; do not restore that test-only dependency graph. The
      orphaned environment-connect/disconnect event DTOs and their rollout
      filtering arms are removed as well. The
      remote-host registration service/registry API and its
      tests are removed; Code Mode's separate WebSocket worker boundary stays.
      On 2026-10-04, `EnvironmentManager` was reduced to an optional local
      environment (no one-entry map or plural default list), the unused
      test-only `LocalProcess::exec` helper and remote PowerShell selector were
      removed, and a stale remote-only test was deleted. Local PowerShell
      fallback and sandbox behavior remain. The locked offline
      `codex-exec-server` suite passes (47 tests) and
      `ahead-agent --tests` type-check passes. Review remaining per-environment
      core policy/context branches before deleting them.
      On 2026-10-05, Codex-only remote MCP environment-variable sourcing was
      removed: `env_vars` is now a list of local variable names, resolved from
      the editor process and passed through the managed executor's core env
      policy. The `source` object form and executor-side variable forwarding
      are gone. The AHEAD-owned runtime-support regression accepts a local name
      and rejects the remote selector. `codex-config --lib` tests could not
      build: copied requirement tests still reference deleted network-policy
      types; do not restore that harness. Remote MCP environment IDs and
      remaining per-environment core state are separate cleanup work.
      The raw MCP config schema no longer accepts Codex `environment_id` or
      the legacy `name` display alias; AHEAD's workspace declaration loader
      rejects both explicitly. Its focused unsupported-config regression
      passes. Internal local environment IDs remain only for runtime permission
      and sandbox correlation; further per-environment core-state pruning is
      still open.
      The AHEAD model-auth crate's obsolete `auth` compatibility facade,
      unused login/request-builder aliases, and dead Codex-originator classifier
      are removed. OTEL originator labels now allow only `ahead` and `none`
      (everything else is `other`); its focused bounded-tag test passes. The
      provider/runtime build passes with no AHEAD-owned warnings. Zed's
      `crates/agent/src/agent.rs` remains the reference for the integrated
      editor-owned agent path; AHEAD's MCP config no longer exposes Codex's
      executor-environment selector.
      Separately, on 2026-10-04 the unused exec-server JSON-RPC lifecycle
      notifications and remote proxy-launch configuration were removed, along
      with stdin write IDs used only for retry deduplication across that
      transport and process metrics never wired into production. The local
      executor now streams process events in-process and writes stdin directly;
      shell-snapshot metrics remain. `cargo check -p ahead-agent --tests -j 1`
      passes and all 47 `codex-exec-server` unit tests pass after restoring their
      minimal dev-dependencies and updating stale test expectations. The
      follow-up local-only environment manager and PowerShell cleanup above are
      type-checked; `TurnEnvironmentSelection` still carries per-environment
      state in core.
      The dedicated Guardian inference routes and `free_guardian` toggle are
      removed; do not restore them while replacing approval call sites.
      The legacy managed `auto_review.ignore_rules` requirement and its
      prefix-rule reclassification are also removed; retired requirements now
      fail closed in layer parsing. The cyber model-specialty rule and ordinary
      human approval flow remain. The focused `codex-config` retired-policy
      regression passes (1 test), and both `codex-config` and `codex-core`
      production library checks pass. After pruning stale generated build
      outputs, `cargo check --locked --offline -p ahead-agent --lib -j 1`
      also passes. Full AHEAD build and native interaction remain open.
      Responses metadata now rejects the retired `window_number` and
      `forked_from_ordinal_exclusive` user-config keys instead of accepting
      and silently stripping them; these remain core-owned metadata fields.
      `codex-core --lib` and `ahead-agent --lib` production checks pass after
      that change; the copied core test target is not an AHEAD release gate.
      The unreferenced copied `policy_template.md`, `node_repl_policy.md` and
      bundled `policy.md` assets are removed, along with the unused policy
      resolver and `guardian_policy_config` / `[auto_review].policy` config
      paths. The dormant Guardian v2 feature/config are removed. The four
      Guardian-only Node REPL/reviewer feature toggles are also removed from the
      active feature registry; their old keys are no longer whitelisted and
      strict config validation rejects them. The
      model-instruction schema no longer exposes model-catalog auto-review
      policy or reviewer-specific approval text; stale catalog fields are
      ignored and their policy payloads removed from the bundled catalog. The
      unused `ModelInfo.auto_review_model_override` field and bundled values
      are removed. `auto_review_enabled` and
      `node_repl_auto_review_required` are removed from model and turn metadata.
      The unused approval-review model selector, provider defaults and hidden
      `codex-auto-review` catalog entry are also removed. `node_repl_disabled`
      remains solely as the MCP capability gate. Retired review metadata names
      are filtered from caller-supplied metadata so they cannot revive a Codex
      review path. The retired `guardian_approval` feature key and `auto_review`
      feature requirements are now unknown and have no effect. Model-based automatic-review
      selection has been removed from native startup, step changes and MCP
      permission policy; the strict Codex reviewer override and its wire field
      are removed. AHEAD's sandbox and scoped effect checks remain
      authoritative. Retired
      `approval_policy = "on-failure"` no longer parses as an
      alias for `on-request`; an AHEAD-owned config regression now covers the
      rejection, and the unusable copied protocol test is removed. The test
      passes with `rtk cargo test --locked --offline -p ahead-agent --lib
      retired_on_failure_approval_policy -j 1`. The
      production `codex-protocol` library check passes. Core approval contexts,
      request
      DTOs, action formatters and module paths are now AHEAD-owned under
      `core/src/approval`. Serialized `GuardianAssessment*` event DTOs and
      rollout-preservation tests are removed. The typed `Internal::Guardian`
      and `ThreadSource::GuardianReview` variants are removed. `ThreadSource`
      now accepts only AHEAD's `user`, `subagent`, and `memory_consolidation`
      values; unknown serialized labels fail deserialization. Removed the
      generic `Feature` source and redundant create/resume guards plus copied
      tests; an AHEAD-owned parser regression covers known and unknown labels.
      Persisted-session compatibility is not required, so no migration is
      added.
      The unused `GuardianPolicy` context
      wrapper and special developer-prompt assembly are removed.
      Guardian-specific downstream session-policy,
      world-state, remote MCP discovery, tool-planning, prompt-schema and
      analytics branches have now been removed. Do not add migration code or
      alter user data. A
      production-source search found no creator for Guardian-labeled reviewer
      subagents. The dead
      `routes_policy_to_automatic_review` route and its configuration-based
      permission-request shortcut are removed; such requests now follow the
      ordinary host permission flow. The serialized source compatibility
      branch is gone; finish pruning the remaining approval/session
      compatibility elsewhere. All six cached
      `ahead-agent` `runtime_support::tests` pass for workspace-bounded scope,
      edit-scope/read-only enforcement, traversal rejection and provider
      settings. After the latest retired-feature cleanup, the full
      `codex-features` library suite passed (31 tests), the `codex-config`
      `strict_config_` filter passed (6 tests, including root/profile feature
      rejection), and the AHEAD-owned `ahead-agent --lib` suite passed
      (111 tests, 2 ignored) on 2026-10-01. The latter requires local loopback
      socket access; six mock-socket tests failed with `PermissionDenied` in
      the restricted sandbox and passed on the permitted rerun. The copied
      core-only managed-requirement regression still needs AHEAD-owned coverage
      because that upstream test harness is not a product gate. An AHEAD-owned
      offline managed stdio MCP test now covers deferred tool discovery, two
      parallel calls with independent human approvals, exact JSON arguments,
      and continuation after both approvals; rendered app and
      authenticated-provider approval journeys remain open.
      The uncalled Guardian root-snapshot/version API, its answer-evidence
      cache, and the dead AgentControl provider are removed. Normal
      `request_user_input` handling still serializes and returns the host answer;
      it no longer stores a duplicate fragment solely for the removed reviewer.
      The catalog on-request message is shared. Approval timeout messages use
      fixed AHEAD-owned text. The inert approvals-reviewer selector and
      model-specific auto-review requirement have been removed; stale
      `approvals_reviewer` and `required_on_models` fields are ignored rather
      than normalized to a user choice. The legacy
      `auto_review.ignore_rules` prefix-rule override is removed; the
      model-specialty rule and separate browser/computer-use review policy
      remain. Prompt assembly no
      longer branches on a reviewer selector.
      The unused Guardian analytics event model and plugin-install/external-
      import telemetry facts are removed too. The approval-request
      compatibility path now uses AHEAD-owned types; automatic review remains
      deliberately fail-closed. The transitional review context no longer
      carries unused model or reasoning settings; it holds only the approval
      and environment inputs read by
      compatibility call sites. The command-source enum is AHEAD-owned
      `ApprovalCommandSource`, preserving its serialized values while removing
      Guardian from the shared approval type. The dead
      `ApproveGuardianDeniedAction` op and `GuardianApprovedAction` prompt
      injection are removed. Preserve the
      normal approval behavior while removing `GuardianAssessment*` rollout
      event DTOs and the obsolete legacy-event deserialization regression.
      The unused extension approval-review contributor API and DTOs are
      removed; serialized Guardian assessment event types and their rollout
      preservation tests are now removed. A 2026-09-29 closure audit confirms Zed's
      agent/agent-server source has no Guardian path and uses ordinary
      tool-permission settings. The retained runtime now rejects every
      `Feature` thread source at thread-manager create/resume; arbitrary
      subagent labels are inert metadata and cannot become root turns.
      Downstream Guardian-only session setup,
      policy, world-state, remote MCP discovery, tool routing, prompt-schema
      and analytics branches are removed; no Guardian-specific source variant
      remains, and unsupported feature-thread rejection stays fail-closed.
      Backward compatibility is not required. The
      Guardian-history and legacy-thread import regressions are removed; only
      current Turso records restore managed thread history.
      Keep rejection of unsupported automatic-review entry points fail-closed.
      Do not rewrite or delete existing user data during source cleanup.
      The Codex `[analytics]` and profile toggles are removed, and the inert
      sink no longer accepts credentials or a destination URL; its unused
      no-op `flush` method is deleted (no callers existed). The telemetry-only
      per-turn resolved-config fact and its first-turn history scan are now
      removed too; this drops a telemetry-only lock and full-history scan from
      managed turns without changing model-visible context. Code Mode's no-op analytics facts,
      guard, and per-response tool-call ID aggregation are also removed;
      actual tool dispatch, rollout traces, and local host-duration timing
      remain.
      The Code Mode cut removes the unused classifier and discarded guard while
      retaining pre/post-use hooks, dispatch tracing and actual tool results.
      The per-turn sampling/compaction/tool-wait profiler was also only sent to
      the inert sink; its `StdMutex`, phase guards and counters are removed.
      Turn duration, TTFT/TTFM and session token histograms remain. This latest
      cut has passed formatting and reference checks only; rerun the focused
      agent/core tests once shared disk space is available.
      Compaction's disabled analytics event path is removed as well, including
      its extra pre/post token-usage snapshots, deleted-token estimate export
      and retained-image count scan; actual compaction, token-budget accounting
      and rollout traces remain.
      On 2026-10-03 the post-cut locked/offline `ahead-agent --lib` check and
      full library test suite passed (149 passed, 3 ignored). The fresh combined
      editor/host build and current-source Code Mode smoke pass; the live
      approval-path regression still needs an unlocked app and provider access.
      No runtime speedup has been measured. The skill-invocation analytics calls
      that only reached the inert sink are also removed; AHEAD's local skill
      counters and model-facing skill injection remain. The subsequent focused
      agent check and full library suite pass (149 passed, 3 ignored).
      On 2026-10-04 the inert `AnalyticsEventsClient`, its session/child-agent
      plumbing, and V2 wrappers that only called it were removed; actual
      `CollabAgentToolCall` and `SubAgentActivityItem` lifecycle events remain.
      The `codex-analytics` crate is deleted. Compaction request metadata now
      lives in core, and implicit skill detection returns an AHEAD-owned host
      skill value; unused resource-origin/invocation types and the unreferenced
      `TurnAnalyticsMetadata` trait are gone. Locked offline
      `cargo check --locked --offline -p ahead-agent --tests -j 1` passes.
      After `cargo clean`, the earlier full AHEAD library test run had 126
      passes and 30 environment failures (25 require the missing
      `target/debug/ahead` helper; five loopback socket tests were denied in
      the restricted sandbox); all 40 ACP tests pass with loopback permission.
      Rebuild the helper through the shared `just dev` loop before rerunning
      those native tests. No build-time gain has been measured.
      The no-op app
      mention/use tracking branches and discarded `AppInvocation` record are
      removed; MCP calls and skill injection remain. The write-only connector
      selection cache, app-ID collection from prompts/skills, and the final
      core-plugin mention module are now removed. Skill-name selection no
      longer depends on dormant app connector names. Legacy `app://` links
      remain excluded from skill selection; they do not activate connectors.
      The retained skills suite passed 44 tests; the AHEAD agent suite passed
      100, with authenticated Pi ignored (2026-09-30). The native streaming
      regression now verifies that a selected project skill body reaches the
      model request. These checks use local mocks, not an authenticated GPUI
      turn.
      The retained `codex-core` and `codex-analytics` libraries compiled through
      the focused `ahead-agent --lib` regression on 2026-09-30. Keep the
      separate `codex-otel` trace/context and session log-privacy behavior until
      its provider/export policy is replaced with an AHEAD-owned setting. The
      retained metrics default is now `none`, and the managed AHEAD config
      forces every exporter off; the managed loader now ignores a stale
      runtime-home config entirely. The focused config regression and 131
      active AHEAD agent library tests pass with local loopback, but a
      native-turn privacy check remains. On 2026-10-04, OTLP exporting became
      an opt-in `codex-otel/otlp` feature; AHEAD's workspace dependencies
      disable it by default. The normal `ahead` graph now omits the OTLP
      exporter, appender and Tonic transport crates plus exporter-only SDK
      features. The base SDK remains for W3C trace context and session metrics.
      AHEAD's featureless provider rejects explicit exporter configuration
      while its managed loader keeps all exporters disabled. The featureless
      library tests pass (11), the opt-in OTLP library tests pass (25, 1
      ignored), and the complete OTLP crate suite passes with localhost access
      (61, 1 ignored). Build time has not been measured, so no speedup is
      claimed.
      On 2026-10-14, removed the Codex Statsig exporter, embedded endpoint/key,
      config variant and Windows WFP telemetry forwarding while retaining
      generic opt-in OTLP, W3C trace context, session telemetry and log privacy.
      The focused `codex-otel` library suite passed (23 passed, 1 ignored), the
      native runtime-home telemetry-isolation regression passed, and
      `cargo check --offline -p ahead-agent --all-targets` passed. Windows-only
      code was source-audited because no Windows target is installed here.
      The AHEAD MCP Guardian-review adapter and duplicate policy branch are now
      removed. The MCP client retains elicitation auto-accept/decline behavior,
      and `native_client.rs` now presents supported, bounded flat server form
      elicitations through the AHEAD chat card. The agent validates typed
      answers before consuming the pending request, and the card has explicit
      submit, decline and cancel controls; unsupported schemas fail closed.
      The copied `openai/form` and
      `openai/elicitation/create` handlers, protocol modes and unused client
      capability selector are removed. The unused `openai/standard-form-input`
      flag and its approval-policy bypass are removed too; standard MCP
      `elicitation/create` remains.
      The AHEAD-owned approval projection regression, stdio MCP approval test,
      full 134-test agent library suite and `ahead` package check pass locked/offline;
      the stdio test requires local loopback binding for its mock model.
      Finish the Zed-style dedicated field controls in the GPUI panel.
      Bounded HTTPS URL elicitation now has an explicit server-labelled
      Open/Decline/Cancel card, with a displayed host and full destination;
      native server-to-browser verification is still required. Choice titles
      and descriptions now
      display separately from submitted values, and an opaque `Skip` sentinel
      lets a literal `Skip` be entered as form text. Valid schema
      defaults now pass through the question DTO and preselect the matching
      answers when a new form arrives; invalid defaults remain unselected.
      Focused form, stdio MCP and headless GPUI regressions pass for the
      value/label split; the latest `just dev` native pass restarted repeatedly
      during the build and did not reach a window.
      Managed config now advertises both form and URL capabilities. URL
      validation, action handling, headless browser-consent and disposable
      stdio-server form/URL roundtrip regressions pass. The URL server trace
      receives `accept` with empty `{}` content, not chat-collected secrets.
      The maintained `just test-all` gate passes after the URL changes on
      2026-10-02.
      Run a rebuilt native click-through before treating the capability as
      release-ready. An elevated `just dev` run initialized the disposable
      project, but the UI tool could not attach to its window; no URL-card
      click was observed. A managed-turn integration test now
      sends a typed form response through an opted-in disposable stdio MCP
      server and verifies the server receives the accepted values. A 2026-10-02
      rebuilt `just dev` run in `/private/tmp/ahead-mcp-native.C7ra5w` opened a
      first-run project, rejected the unapproved server declaration, approved
      its exact command in Settings, then displayed tool approval and the
      three-field form in chat. The user-facing card sent integer `count=2`,
      string `nickname=Nora`, and skipped optional `color`; the stdio server
      trace confirmed the typed response and the chat completed on a local
      mock-model reply. This is native UI proof for form acceptance, not an
      authenticated provider turn. The same rebuilt native app also sent
      separate `decline` and `cancel` results to the stdio server and
      completed both turns; cancellation left no stale Thinking card after
      the terminal-state cache fix in `ahead-app/src/proxy_client.rs`.
      The maintained `just test-all` gate passes on 2026-10-02 after the
      MCP form tests stopped assuming JSON property iteration order and the
      opt-in proxy harness test adopted the actor-aware turn signature. After
      the value/label change, six parallel native mock-model tests timed out;
      all 141 active agent tests and the maintained gate pass with the recipe's
      single test thread. Investigate safe parallelism in
      `ahead-agent/src/native_client.rs` before removing that setting.
      Verify malformed answers across the full turn and form/URL resume after
      restart. The focused agent URL test already rejects malformed answers
      without consuming the pending request.
      Tool-call approval through chat questions is a separate path. The
      Codex strict automatic-review request and response-metadata path are
      removed; they no longer override AHEAD's configured policy. The optional
      model-reviewer API was also removed from the retained MCP client. The
      copied-core session round-trip regression was not run; if that behavior
      needs additional coverage, port it into an AHEAD-owned test target. The
      unused Guardian MCP approval-metadata session
      cache and request builder are also removed; normal AHEAD tool-approval
      metadata remains.
      The standalone Codex `Op::Review`/`/review` task is removed with its
      second model turn, review-only model setting, protocol DTOs/events/items,
      prompts/rubric and rollout mappings. Ordinary agent chat and AHEAD's
      source-control workflow remain; human tool authorization still uses
      `ReviewDecision`. `codex-core --lib` and `ahead-agent --lib` checks pass;
      protocol (313), config (207), prompts (23), models-manager (47), rollout
      (103), rollout-trace (61), and ahead-agent (156 passed, 3 ignored) suites
      pass. The agent tests require permission to bind local mock sockets.
      The unreferenced Codex `codex_delegate` runner and its test module are
      deleted; AHEAD's used thread manager remains. The dead
      `record_response_item_and_emit_turn_item` helper and delegate-only Code
      Mode provider getter are removed. Rechecked `codex-core --lib` and the
      full ahead-agent suite (156 passed, 3 ignored); the only core warning is
      an upstream `proc-macro-error2` future-compatibility notice.
      The duplicate Codex file-search crate and unused Bedrock/AWS provider
      closure are removed; do not restore either to the default build graph.
      Model credentials now use the AHEAD-owned `ahead-model-auth` package;
      on 2026-10-07, a checkout-wide reference search found its
      `AheadAuth::is_api_key_auth` predicate and `AuthManager` change-watch
      channel unobserved; both dead APIs were removed. `AuthManager::reload`
      had only `auth()` as a caller, so its resolver was inlined and its unused
      change-result plumbing removed. The one-use
      `from_optional_auth_for_testing` wrapper was inlined into its sole caller;
      `shared_empty()` still covers unauthenticated tests.
      Keep `read_openai_api_key_from_env`: its remaining caller is the separate
      realtime voice path in `core/src/realtime_conversation.rs`, not managed
      chat. The current locked/offline metadata lists 54 runtime-source packages
      in the root workspace; package membership alone does not justify pruning
      active behavior. No compile followed this cleanup. On 2026-10-07,
      `cargo fmt --all -- --check` and `git diff --check` passed; stable rustfmt
      emitted warnings that it ignores the configured nightly import grouping.
      On 2026-10-14, moved the AHEAD-owned `ahead-model-auth` crate from
      `ahead-agent-engine/login` to
      `ahead-model-auth`; the root dependency and runtime
      architecture doc now use the AHEAD-owned path. The shared `just dev`
      build resolved the moved crate and rebuilt `ahead` successfully in
      53.77s. macOS again aborted the app before its first frame with the
      HIServices XPC signal 6; no auth behavior changed.
      Codex account login/storage/refresh, account keyring, workload identity
      and Agent Identity are removed from the app graph and source. Do not
      restore them. The unused Codex auth bootstrap/config trait, its copied
      tests, and the no-op managed `shared_from_config` adapter are now removed;
      the native thread manager requests an empty auth manager directly while
      AHEAD's provider connection supplies the selected model credential.
      The unused Codex login-method/ChatGPT-workspace config fields, managed
      auth policy and remote-app-server bypass are removed; strict config and
      requirements-layer regressions reject their retired keys. The no-op
      copied `AuthManager::new/shared` constructors and their stale auth.json
      and Agent Identity bootstrap tests are removed too.
      The uncalled account-switch auth provider, product-login constants and
      no-op logout/workload helpers are removed; provider bearer/header auth
      still serves managed requests. ChatGPT OAuth/token variants, account
      identity telemetry and FedRAMP bearer headers are now removed as well.
      The ignored `use_agent_identity` feature,
      request-scope wrapper and stale copied Agent Identity assertion tests are
      removed; strict config rejects the retired feature key.
      `cargo check -p codex-model-provider --tests` still fails on pre-existing
      removed `shared_state`/Agent Identity imports and missing test-only
      dependencies; port only useful provider behavior tests to an AHEAD-owned
      target instead of restoring retired login features.
      Managed MCP is local stdio only. Remote transport, OAuth/keyring flows,
      HTTP-specific client tests and their dependencies have been removed across
      config, core, MCP and the RMCP client. `Cargo.lock` has been refreshed.
      The remaining RMCP integration files referenced missing Codex test-server
      binaries and are removed; retain the AHEAD native-agent test that launches
      the disposable stdio server for approval and real tool-call coverage, plus
      the low-level environment-forwarding tests. Verify with focused agent/RMCP
      tests and confirm remote MCP/keyring packages are absent from the resolved
      app graph.
      Marketplace catalog/recommendation/install paths, plugin prompt/skill/hook
      injection, plugin MCP projection and the manager/store/remote package
      closure are removed. The orphaned copied `request_plugin_install`
      prompt asset and extension-API tutorial example are removed too; neither
      had a production caller. CodeAnchor attribution remains in AHEAD's
      session controller; do not restore the old core-plugin facade.
      The hosted-Apps-only OpenAI file-upload rewrite, upload client pool,
      `codex-api/src/files.rs`, and `codex-mcp` `fileParams` schema masking are
      removed. Ordinary MCP tool calls still pass their JSON arguments to the
      exact prepared client. The AHEAD agent suite passes (117 active, 2
      ignored), the app binary builds, and the production agent graph checks.
      The copied `codex-core --all-targets` test target remains unbuildable
      from inherited missing test dependencies; do not restore those simply
      for this cleanup. Verify an actual stdio MCP tool call and approval in
      the disposable app before claiming live coverage. The unreachable
      hosted-Apps consequential approval-template module and its bundled
      connector template asset are removed; generic approvals still display
      the server, tool and JSON
      arguments. The hosted `_codex_apps` request metadata envelope and its
      connector-ID exception to session approvals are removed; ordinary call
      and turn metadata and per-server approval keys remain. Hosted connector,
      link, app-action and plugin identity fields are removed from MCP call
      items and legacy events; the MCP Apps `ui://` resource URI and tool
      read-only hint remain. The `openai/outputTemplate` URI fallback is
      removed. Hosted connector, account and plugin fields are also removed
      from MCP approvals, tool catalog entries, telemetry and request metadata;
      generic server/tool approvals and annotations remain. The hosted-only
      resource filter and `orchestrator.mcp.enabled` config flag are removed;
      ordinary registered MCP servers remain eligible for resource listing
      and reading. The disposable stdio MCP fixture exercises aggregate
      listing and an explicit read through a mock-model managed turn; verify
      the same path in the native app. Extend this check to negotiated
      `2026-07-28` resources with required result and cache fields before
      claiming current-version resource coverage. Follow-up on 2026-10-15:
      AHEAD now prefers `McpProtocolMode::V20260728` for managed stdio MCP and
      uses RMCP's standards-based `server/discover` probe with legacy
      `initialize` fallback. The disposable `mcp_echo.py` default scenario now
      requires modern per-request metadata plus `resultType`, `ttlMs`, and
      `cacheScope` on discovery, listing and resource results; form/URL modes
      return the legacy unknown-method response and cover fallback. The shared
      `just dev` build compiled the changes, and the current binary passes
      `tests/agent-mcp-smoke.mjs`: its modern-only fixture rejects legacy
      `initialize`, the managed client discovers and calls the stdio tool, the
      proxy surfaces a per-call approval which the smoke answers, and the
      model receives tool and modern resource results before completing. A
      second phase gets the form fixture's discovery error, follows legacy
      initialization, and successfully calls the tool after approval. This is
      real proxy/runtime evidence, not rendered Settings/chat approval UI. The
      standalone Python fixture check also passes. Source tracing at 23:42 EDT
      on 2026-10-15 identified an in-flight cancellation gap; the managed call
      path now carries `ToolInvocation` cancellation through `McpHandler` and
      `PreparedMcpCall::call_with_preparation` into
      `McpClient::call_tool_cancellable`. That RMCP boundary races the active
      request handle against cancellation and sends `notifications/cancelled`
      with the handle's request ID; its caller allows bounded cleanup before
      aborting the handler. On 2026-10-16, `tests/agent-mcp-smoke.mjs` passed
      against the integrated binary with loopback-only access for its fake
      model. It verifies modern discovery/resources, request-state retry, stdio
      tool execution, per-call approval, model continuation, legacy initialize
      fallback, and in-flight cancellation: the fixture observed the exact
      active request ID and the cancelled model turn was not resumed. This
      closes the identified cancellation-protocol gap; it is not native approval
      UI evidence. Disk headroom was rechecked at 5.6 GiB; use the shared dev
      loop instead of a competing Cargo build. The
      copied `codex_apps` name reservation and executor-local HTTP MCP discovery
      branch are removed; the former hosted name now has ordinary MCP server-ID
      semantics. The Zed reference (`../zed/crates/context_server/src/client.rs`)
      allocates the request ID before sending, races the response against
      per-request cancellation, then sends `notifications/cancelled` with that
      same ID. The implementation preserves RMCP's input-required rounds while
      exposing the request handle at the AHEAD client boundary; the tool
      dispatcher grants bounded cleanup time before aborting a cancelled
      handler. This follows the MCP requirement that cancellation identify the
      in-flight request:
      https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/cancellation.
      Compared against Zed and the pinned SDK at 23:52 EDT on 2026-10-15.
      Managed
      sessions still admit only reviewed workspace stdio declarations. AHEAD
      does not render MCP Apps UI resources; do not claim MCP Apps host support
      without sandboxed rendering and a disposable end-to-end test. No
      compatibility shim for the former hosted server name is required.
      Preserve ordinary per-server tool caching, exact-client bindings,
      cancellation, server-owned credentials and generic MCP approval.
      The custom retry loop uses `Peer::send_cancellable_request` and the
      existing `ElicitationClientService`; the disposable MCP smoke separately
      verifies request-state retry and identified cancellation. Form/URL
      elicitation behavior remains tracked with the native approval-card and
      response walkthrough, rather than being inferred from this cancellation
      smoke.
      The disposable fixture cleaned itself on success. Native approval
      rendering remains unverified.
      The unreachable `codex-mcp` hosted-Apps tool-name normalizer and its
      startup branch are removed; standard listed tools now take the ordinary
      server-name/JSON path. The duplicate lower-client tool-list API that
      parsed untrusted connector IDs is removed; `codex-mcp` now uses the
      existing SDK-backed `list_tools` result and strips connector fields.
      MCP initialization identifies AHEAD to the server,
      and HTTP requests use an AHEAD user agent. The disposable stdio MCP
      roundtrip still passes with two independently approved calls. The unused
      hosted event subscription API and its write-only extension resource
      client are removed; ordinary MCP resource listing/reads still use
      `McpRuntime` directly. The hosted-only catalog permission exemption,
      8,192-item limit and cache bypass are removed; generic server policies
      now apply uniformly. The now-unreachable Plugin Runtime
      `events/stream` request, notification wrapper, HTTP cancellation/timeout
      branch and copied event-only tests are removed from `rmcp-client`;
      ordinary HTTP SSE handling remains. The root app check and AHEAD agent
      suite pass, but the copied HTTP SSE integration target cannot compile
      without its retired test-only `wiremock` and `pretty_assertions`
      dependencies. Port meaningful HTTP transport coverage to an AHEAD-owned
      test if HTTP MCP is enabled in the editor. Remaining hosted-Apps references
      elsewhere in the retained runtime are not yet removed. The current full
      AHEAD agent suite passes 117 tests with two opt-in tests ignored, and
      the root app binary passes a locked offline compile check; neither
      result verifies a rendered app session. Model-invoked resource listing,
      template listing and reading now use per-server human review before
      dispatch; the disposable server trace proves a denied aggregate list
      never reached the server. Aggregate calls prompt for each configured
      server. Verify these approval cards and cancellation in the native app.
      Resource operations now follow an AHEAD-owned, workspace-local
      per-server auto-approval or review choice, independently of server-owned
      MCP tool names. The Settings control is available only after declaration
      approval. The disposable managed stdio test now covers reviewed and
      auto-approved tool/resource calls in separate sessions. Verify the
      Settings control, approval cards, and cancellation in a running native
      app; the offline fixture does not prove
      rendered behavior.
      Use Zed's pinned `context_server_store.rs` and agent
      `context_server_registry.rs` as lifecycle references. Completed pruning
      and test evidence are recorded in `docs/development/zed-tracking.md`.
      Add and test the server-auth UI before allowing workspace HTTP MCP
      declarations. Managed stdio MCP now supports bounded form and URL
      elicitation, but browser auth completion and native URL review still
      need end-to-end verification. Internal MCP tool-approval forms now
      reach the existing human-review card with arguments visible; the
      disposable stdio roundtrip passes, but rendered approval and OAuth
      journeys remain release gates.
      The app now retains parallel input requests per session in arrival order,
      shows their pending count, and removes only the answered request; the
      focused client regression passes through the actual answer RPC and checks
      the second request remains. A headless GPUI test renders the first card,
      clicks Allow and keeps the second card hidden. Verify the full queued
      sequence and cancellation in a running native session.
      The managed runtime event loop now continues streaming while separate
      approval responses wait for human input. The disposable parallel MCP
      regression failed before that change and passes with both approvals
      delivered before either is answered. Per-turn cleanup now retires only
      that turn's unanswered approvals on exit, preserving concurrent child
      requests; its focused regression and the full agent library suite pass
      (124 tests, two opt-in tests ignored). The locked offline `ahead` binary
      check passes. Exiting a child turn now notifies the active chat to retire
      only its unanswered approval card; a headless GPUI test shows the next
      card without carrying over the first choice, and all 167 app library
      tests pass. Verify the full timing and cancellation in native GPUI.
      Host approval IDs are now generated per request, so parent and child
      runtime threads cannot alias pending answers even if model call IDs match;
      the retained runtime still receives its original IDs. Verify concurrent
      parent/child review cards in a running native session.
      The disabled Guardian reviewer route and its no-op review stubs are now
      removed; strict approval requests route to the human in the retained
      runtime. The copied `strict_mcp_review_routes_to_the_human` test binary
      still fails before filtering. An AHEAD-owned end-to-end test now covers
      ordinary human review of internal MCP approval forms. The old
      `strict_auto_review` override was never set by AHEAD and is now removed;
      do not port its copied test unless AHEAD adds an explicit human-review
      policy distinct from the existing allow/deny/confirm settings. The rendered review journey
      remains unverified. Legacy
      reviewer selectors are
      ignored instead of normalized. The reviewer enum and selector fields
      are removed from config, profiles, managed requirements, analytics, MCP
      policy and runtime state; model-required Guardian review constraints are
      also removed. Existing old-session compatibility tests are candidates for
      pruning, not release requirements. Managed
      The legacy `auto_review.ignore_rules` requirement is removed; the
      name-specific Guardian source guards
      are replaced by a generic unsupported-feature rejection at thread-manager
      creation and restore, and no `ThreadSource::Feature` may start a root turn.
      The delegate keeps its approval-never boundary but has no Guardian label
      branch. A current source search finds no production Guardian tool-filter
      branches; current AHEAD session restore and permission enforcement still
      need broader coverage.
      The AHEAD-owned create/restore regression passes after a fresh native
      client reads a mutated persisted source (1 test, 123 filtered); it does
      not replace a running-app permission check.
      A read-only inventory of this checkout's `.ahead/session.db` on
      2026-09-28 found 7 sessions and 7 harness bindings, including one legacy
      `managed-codex-app-server` backend, but no session events or native
      runtime-thread/item rows. `.ahead/runtime` contains no rollout JSONL.
      This is historical inventory, not a requirement to preserve retired
      harness bindings or Codex replay formats. Do not delete user data while
      removing those code paths.
      Focused `ahead-agent --lib` tests compile the retained runtime after this
      pruning; the copied core test target was not run.
      Workspace-local `.ahead/settings.toml` now accepts
      `[mcp.tool_permissions.<server>]` with named `allow`, `deny`, and
      `confirm` choices; unlisted tools inherit the workspace-local server
      choice and tracked declarations cannot auto-approve. Add Zed-style
      one-time and persistent named-tool choices to
      AHEAD's managed MCP review UI, with rendered coverage. The existing
      workspace settings writer now uses private atomic replacement rather
      than truncating provider credentials and MCP policy in place. The MCP
      approval question now selects one choice in chat; the native client and
      retained parser reject mixed Allow/Cancel answers. The GPUI choice-state
      and native-boundary regressions pass, but the copied core-only regression
      is not runnable through its disabled library test target. The managed
      per-turn sandbox now denies workspace/user
      `.ahead` and explicit `AHEAD_HOME` reads and writes in Assist and Learn,
      and scoped native paths cannot target project `.ahead` or symlink into it;
      verify this in a live native shell/patch turn before treating stored
      allow choices as a production security boundary. MCP opt-in now pins the
      parsed tracked declaration with a `sha256:` fingerprint; old ID-only
      selections fail closed, and changed command/args/env references require
      reapproval. The Settings panel now lists tracked declarations and local
      approval state and can enable or revoke a reviewed fingerprint without
      manual TOML editing. The proxy binds those requests to its active
      workspace and rechecks the declaration before saving. Verify the
      rendered flow in `just dev`, including changed declarations, orphaned
      approvals, and an already running MCP server; revocation currently only
      affects new managed sessions. Settings autosave and proxy-owned MCP
      approval now use a shared workspace-local lock across atomic replacement;
      focused lock-contention and symlink-lock regressions pass in both crates.
      Verify simultaneous edits through the running app and proxy.
      The local-policy regression and all 16 MCP-filtered AHEAD agent library
      tests pass in a loopback-enabled runner on 2026-09-29. The full
      `ahead-agent --lib` slice passes 94 tests with 1 Pi test ignored after
      workspace/user-home `.ahead` sandbox, declaration-fingerprint, and
      MCP answer-validation changes (2026-09-29, serial test threads and
      loopback access). A parallel run hit
      an unrelated adapter install-lock failure that passed in isolation;
      keep that flake separate from the managed MCP boundary.
      The host denies MCP in Learn/read-only sessions and never
      passes user-configured servers to external ACP agents. Keep the editor
      bridge separate. Add Streamable HTTP only after AHEAD owns and verifies
      its authentication and secret-storage path; current managed config is
      stdio-only. Use
      `../zed/crates/agent/src/thread.rs::authorize_third_party_tool` and
      `../zed/docs/src/ai/tool-permissions.md` as the primary reference; add
      persistence-scope, mode-transition and rendered-review coverage before
      allowing choices from the reviewer.
      Disabled plugin-measurement and artifact-operation telemetry, the
      sidecar's extra shell permissions, and environment injection/filter hooks
      are removed; requested command permissions and sandboxing remain
      unchanged.
      The copied `ahead-thread-history` App Server DTO crate and its no-op
      schema-macro crate are removed with the unused thread-store turn, item
      and timeline projection APIs. Focused Cargo resolution no longer finds
      `ahead-thread-history` in AHEAD's normal dependency graph. Compile/test
      validation remains open. The 2026-09-25 host check found 92 GiB free, and
      a focused `ahead-app` test has since populated this checkout's `target/`.
      Keep validation focused and do not clear another workspace's cache.
      Skill loading now uses only project `.agents/skills/` and user
      `~/.agents/skills/`; the copied executor-environment and MCP-orchestrator
      skill providers are removed and must not be restored.
      The copied `codex-network-proxy` crate, its config and serialized
      compatibility fields, proxy startup and remote-launch call sites, and
      proxy-only telemetry have been removed. AHEAD retains only binary
      `NetworkSandboxPolicy::{Restricted, Enabled}` behavior; there is no
      proxy, per-domain allowlist, or proxy exception. Finish pruning the
      unbuilt copied proxy-only test fixtures and verify the restricted policy
      on each supported OS before broadening network access. The unreferenced
      Codex CLI/App Server manuals for `core`, `protocol` and `execpolicy` were
      removed too; the AHEAD runtime README and standards doc describe the
      retained boundary.
      Zed's
      `../zed/crates/agent/src/sandboxing.rs` and
      `tools/terminal_tool.rs` remain the reference for command network scope.
      AHEAD now has a binary `NetworkSandboxPolicy`; built-in turns select
      `Restricted`, while `Enabled` means ordinary OS networking without
      proxy-mediated destination filtering. Verify the restricted policy on
      every supported OS and host-specific enforcement where offered before
      broadening network access. The current managed-runtime guarantee is
      documented in
      `docs/development/ahead-agent-standards.md#managed-effects`.
      Migrate any still-relevant retained-loop unit coverage into `ahead-agent`;
      the normal CI gate intentionally uses AHEAD default members instead of
      rebuilding the copied upstream product test harness and its removed
      test-only dependencies. The focused `ahead-agent` library suite passes
      47 tests with one ignored credentialed Pi live test when loopback sockets
      are available; the sandboxed run fails six local fixture binds with
      `Operation not permitted`. The ignored test was run in an isolated config
      directory on 2026-09-24; Pi ACP
      initialized and accepted the prompt but emitted no response for 60 seconds,
      so it was cancelled before the client's 1800-second timeout. Retry after
      configuring a catalog-listed model on an active provider: direct requests
      found `opencode/x-preview-f-free` unavailable, the OpenCode free-tier model
      restricted to its own UI, and `opencode-go` said an active subscription is
      required despite local auth-check `ready`. Require a successful direct and
      ACP PONG on the same model. This does not replace the running-GPUI
      managed-agent acceptance gate.
      Keep only model/provider request code, turn state, streaming,
      cancellation and compaction. Use the boundary in
      `docs/development/ahead-agent-standards.md` and compare both
      `../zed/crates/agent` and the retained Codex implementation before
      deleting each dependency. `ahead-agent-engine/SOURCE_BASELINE.toml` and
      the scoped runtime guidance now name the upstream `rust-v0.152.0` commit
      and AHEAD fork seed separately. Discarded executor-capability discovery
      work has been removed from MCP refresh/config paths while remaining in
      managed model-step context; the unused MCP thread-init clone and field
      are removed.

- [ ] Verify memory search, append and reviewed consolidation in a running GPUI
      session. Project
      `.ahead/memories/MEMORY.md` and user
      `~/.ahead/memories/MEMORY.md` stay isolated; workspace startup indexes
      their current contents and immutable hash revisions in
      `.ahead/session.db` without making the database the human-readable
      authority. Native sessions do not inject memory automatically; explicit
      search selection or a bounded memory-review command supplies it as
      context. Model-visible source labels use conventional paths rather than
      absolute project or user-home paths. The composer searches both sources
      through the Turso index and attaches a selected, labeled excerpt as
      explicit context;
      a per-line token index ranks multi-term matches, supports exact and
      token-prefix search, filters to the current content hash and keeps
      absolute host paths out of the prompt. Revision, token-index and
      current-source writes are transactional. On 2026-10-07, the
      source-current `ahead_proxy-f12d2fbf5fdfa3b0` test binary (18:43, newer
      than `host.rs` at 15:42 and `store.rs` at 15:35) passed all 16 `memory_`
      tests (233 filtered) directly without Cargo, including the
      parent-directory symlink-swap, non-regular-file and size-limit
      regressions. A sandboxed 2026-10-02 complete `ahead-proxy`
      library run passed 200 tests and failed 12: 9 local socket/process
      operations were denied by the sandbox, and 3 catalog tests assumed no
      startup extension-issues notification. Those catalog fixtures now drain
      that notification; their focused suite passes 10 tests (1 ignored).
      After the extension-host fix below, the unrestricted serial proxy suite
      passes 212 tests (1 ignored) on 2026-10-02.
      Workspace startup and search now validate memory sources through the
      shared AHEAD path guard; search refreshes serialize against AHEAD writes.
      On Unix, memory reads, appends, reviewed replacements and index refresh
      use a no-follow directory handle rooted at the canonical workspace or
      user home. Leaf opens use `O_NOFOLLOW | O_NONBLOCK` and verify a regular
      file; appends also reject hard-linked files. Replacement temp creation
      and rename use the same directory handle. Indexing rejects documents
      over 32 KiB and consumes content read from that handle, so a swapped
      parent symlink cannot redirect these operations. Other non-Unix,
      non-Windows memory file operations remain path-based. Windows memory I/O
      now uses an AHEAD-owned `memory_fs.rs`: it opens the canonical root with
      `NtCreateFile`/`OBJ_DONT_REPARSE`, walks `.ahead` and `memories` by
      `RootDirectory` handles, reads/appends relative to that handle, and
      atomically replaces with `SetFileInformationByHandle` and a relative
      destination. Missing user-home components are created during a
      reparse-safe walk; only the final workspace/home root is reopened with
      child-creation rights. A Windows-only regression covers DOS, verbatim
      DOS, UNC and verbatim UNC path conversion. This extends the retained
      Codex no-follow reference, whose own path opens use `RootDirectory = 0`.
      The parent-symlink-swap regression now runs on Windows and Unix and is
      wired into Windows CI; Windows compile/run evidence is still pending.
      The memory path now verifies current-user ownership and applies a
      protected current-user-only DACL to `memories` and its files. A focused
      Windows regression inspects the owner, sole allow ACE, directory
      inheritance flags and protected-DACL bit; Windows compile/run evidence is
      still pending.
      Windows CI runs the focused `ahead-proxy` memory search/write/review
      tests; symlink regressions fail rather than silently pass if the runner
      lacks symlink creation rights. The parent-swap test checks handle-relative
      reads, appends and replacement; the new ACL assertion is included in the
      `memory_` filter and awaits Windows CI execution.
      Composer-side memory search prefix and bounded replacement parsing pass
      their two focused app tests (re-ran 2026-10-01; 2 passed, 162 filtered;
      `rtk cargo test --locked --offline -p ahead-app --lib memory_ -j 1`). On
      2026-10-07, the source-current app runner `ahead_app-601e2b645b3fbe4a`
      (19:36, newer than `session_panel.rs` at 19:36:32) passed both
      `memory_` composer tests directly, with no Cargo build.
      In the disposable native project on 2026-10-01, the rendered composer
      found a synthetic project-memory line after the file was created,
      attached its project-relative excerpt, refreshed to a changed line after
      an external edit, and removed the hit after the source was deleted.
      `/review-project-memory` loaded the current snapshot and review prompt;
      no model turn was sent. A second disposable app run with an isolated
      `AHEAD_USER_HOME` rendered a synthetic user-memory search hit labeled
      `~/.ahead/memories/MEMORY.md:3`. Use an isolated home before touching
      the real `~/.ahead` memory.
      A 2026-10-01 retry against a new disposable workspace and isolated user
      home built the app, but the sandboxed process aborted in macOS AppKit
      registration before AHEAD startup (crash report:
      `~/Library/Logs/DiagnosticReports/ahead-2026-10-01-154645.ips`). The
      native-access retry created the disposable `.ahead/session.db`, but UI
      automation selected an older open AHEAD window, so this retry proves
      startup only, not memory interaction. A subsequent uniquely identified
      temporary app bundle let automation select the disposable window without
      closing the older one: `@memory saffron` displayed the user-scoped hit,
      selecting it attached the labeled excerpt, and `/review-user-memory`
      loaded the current snapshot and prepared the review prompt. A later
      offline native run in that isolated window used a loopback Responses
      mock to render a managed chat turn. Saving its human message to user
      memory appended once; repeating the action left the file hash unchanged.
      The completed `/review-user-memory` turn produced a replacement that
      the explicit context-menu action applied to the user file. A second
      proposal was rejected after an external source edit, preserving the
      changed file hash. Saving an assistant message to project memory then
      refreshed the composer search: `@memory saffron` showed separate
      project- and user-scoped results. This proves the native interaction
      against synthetic local responses, not authenticated provider behavior.
      A follow-up isolated native turn completed `/review-project-memory`,
      applied its project-scoped proposal, left the user memory file unchanged,
      and immediately found the replacement through `@memory violet compass`.
      A message context menu now explicitly saves a selected
      message to the user-chosen project or user file, with bounded,
      message-idempotent append, an 8 KiB note and 32 KiB document cap,
      symlink checks and immediate index refresh.
      Append and review now share a bounded read of the same opened descriptor;
      the oversized-existing-file regression and all 16 focused memory tests
      pass (re-run against the current worktree on 2026-10-04).
      Managed AHEAD slash commands now attach a bounded current snapshot for
      project/user memory review and run that turn with the native read-only
      filesystem profile. A complete replacement can be applied only from its
      completed assistant message; the host checks the snapshot hash,
      atomically replaces the markdown file and refreshes the Turso index.
      Repeat the managed memory turns with an authenticated provider when
      available. Keep screenshot documentation for this and other app features
      as the final phase.
      On 2026-10-07, direct execution of existing test binaries newer than the
      affected sources again passed `ahead-proxy --lib memory_` (16 passed)
      and `ahead-app --lib memory_` (2 passed); no Cargo build was started.
      This is automated evidence only. The Mac is locked, so this pass adds no
      native interaction or authenticated-provider evidence; the earlier
      mock-backed native search/append/review run remains the rendered evidence.
      On 2026-10-07, `composer_memory_search_attaches_the_selected_project_excerpt`
      drove the composer through the `SearchMemory` RPC, rendered the result in
      the gpui-kit Command list and attached it as explicit context. Its
      responder asserted the exact query and result limit, and verified that
      selecting memory did not send an `AgentTurn`. All 50
      `session_panel::tests` passed serially with the new regression. This is
      headless GPUI evidence; native confirmation still relies on the earlier
      isolated synthetic-provider run, and authenticated-provider turns remain
      unverified.
      On 2026-10-08, the current `ahead-app-601e2b645b3fbe4a` runner (07:53,
      newer than `session_panel.rs` at 07:41) passed the composer-search,
      project/user-save and current/stale-replacement GPUI regressions directly
      (3 tests). A locked/offline rebuild then passed all 16 current
      `ahead-proxy --lib memory_` tests serially. The shared `just dev` watcher
      was stopped before that Cargo build to avoid competing build loops. These
      are headless GPUI and proxy results; the Mac remains locked, so this adds
      no rendered interaction or authenticated-provider evidence beyond the
      earlier isolated synthetic-provider run. The watcher uses a disposable
      `AHEAD_DATA_HOME`; `justfile::_run-dev` derives isolated runtime/user homes
      from it while preserving explicit overrides. Its app launch still aborts
      before rendering with macOS's `hiservices-xpcservice` connection-invalid
      error (signal 6).
      On 2026-10-14, the source-current `ahead_app` runner (14:17:51, newer
      than `session_panel.rs` at 14:17:02) passed all five memory-focused
      composer/save/review tests. The current `editor-recovery-smoke.mjs` also
      passed Unicode recovery after proxy SIGKILL, active-owner protection,
      discard/stale-write fencing, and owner-only database permissions in a
      retained disposable workspace. These are headless tests; current native
      AX/screenshot calls time out, and authenticated-provider memory review
      remains unverified.
      On 2026-10-15, `tests/agent-memory-smoke.mjs` passed against the current
      integrated binary in an isolated workspace/profile/user home: project
      and user appends/search stayed isolated, portable source labels omitted
      host paths, reviewed replacement refreshed the Turso index, stale hashes
      were rejected, and both scopes survived graceful proxy restart. This is
      proxy/RPC and persistence evidence only; rendered composer selection and
      managed-session memory review still need native confirmation.
      Revalidated later on 2026-10-15 with the same integrated binary; the
      isolated project/user/Turso smoke again passed and cleaned its fixture.
      Re-ran on 2026-10-16 against the current integrated binary; project/user
      append, scoped search, portable source labels, reviewed replacement,
      stale-snapshot rejection and Turso persistence after graceful proxy
      restart all passed. The smoke removed its temporary project/profile on
      success; no model provider, network or Cargo build was used. Native memory
      selection/review remains unverified.

- [ ] Validate and polish the agent-composer slash palette, using the attached
      Codex menu as the interaction reference. Treat this as a grouped
      chat-action and skill catalog, not just a tool picker: surface supported
      AHEAD chat/session actions, installed skills as first-class entries, and
      active ACP commands; borrow the menu's filtering and grouping behavior
      without copying Codex-only actions that AHEAD does not support. The
      composer now fuzzy-ranks native actions, the managed runtime's discovered
      skills and active ACP commands by name/description, then orders sections
      by their best match, following Zed's
      `../zed/crates/agent_ui/src/completion_provider.rs::search_slash_commands`
      and `group_by_relevance`; rows include
      descriptions and keyboard navigation and selection routes through the
      owning action/runtime. Managed skill
      discovery reuses Codex's `HostSkillsService`; choosing a skill inserts
      `/skill-name` syntax for unique names and source-qualified `/user:name`,
      `/project:name`, `/system:name`, or `/admin:name` forms for collisions
      without a legacy `/:name` alias; the
      native runtime
      resolves it to a structured `UserInput::Skill`, loading instructions
      rather than treating it as a tool. Skill discovery is keyed by
      session/model/provider and discards late results from an earlier model
      selection. Opening the palette now refreshes
      a forced host skill snapshot, so project/user additions and metadata edits
      appear without restarting AHEAD; closing or switching scope invalidates
      in-flight catalog replies. The RPC route regression also checks refreshed
      project metadata, scope and host-path privacy. Headless GPUI tests now pass
      for filtering by skill metadata, Enter selection, arrow navigation and
      Escape dismissal (`cargo test --locked --offline -p ahead-app -j 2 slash`).
      A follow-up GPUI regression now types successive query prefixes while the
      palette stays open, checking description-only skill matches and ACP
      command filtering before selection. Selection now resets to the
      highest-ranked visible result after each filter change, with a regression
      covering a stale row that otherwise could activate a different command.
      Empty groups are suppressed, and the headless palette regression checks
      that external ACP does not display an unsupported empty Skills section.
      The 2026-09-28 rendered regression found status notices leaking into
      description matches during type-ahead; notices now match by title only,
      while actionable entries still match by description. The test selects a
      skill through its filtered row rather than assuming it is the default
      first command. Focused slash-palette tests pass (10 passed, 60 filtered)
      with `rtk proxy cargo test --locked --offline -p ahead-app --lib slash -j 1`;
      this includes a headless GPUI rendered mouse-selection regression.
      Managed context composition now preserves a selected slash skill at the
      beginning of the prompt so the native runtime can resolve its standard
      skill package; a regression covers commands with leading whitespace and
      editor context.
      Open-standard instruction/skill paths and the explicit non-support for
      Zed `.rules` are documented in
      `docs/development/ahead-agent-standards.md#filesystem-placement`.
      A 2026-09-30 native-window pass used the current `just dev` binary in a
      disposable macOS bundle and fake project. The grouped action/Skills menu
      rendered; typing filtered by command or skill name and by skill
      description, an unmatched query showed the empty state, Enter inserted
      `/ahead-teaching`, and clicking a description-matched result inserted
      `/ahead-prototype` without sending a turn. The unbundled `just dev`
      process itself was not discoverable by CUA. Still verify project/user
      skill refresh, active ACP commands, narrower-window layout and final
      visual polish in a running app.
      Rechecked on 2026-10-19 in the already-running disposable `just dev`
      app: `/` opened the grouped picker and typing `skill` filtered it to the
      two project-installed skill entries. The draft was cleared without
      selecting an entry or sending a turn. Native filtering is verified at
      this window size; ACP-command presence, narrow layout and refresh after
      editing skill files remain open.

- [ ] Finish end-to-end behavior validation for Turso-backed managed session
      persistence. Pre-production scope is the working initial schema; schema
      checks and schema-version/migration machinery are deferred. The copied
      `LocalThreadStore` and its legacy SQLite feature, rollout backfill/listing
      and SQLite config path, SQLite queue exports, `codex-state` crate, and
      root SQLx/SQLite dependencies are removed. Managed sessions use
      `TursoThreadStore` as the only live thread store. The sole import
      exception is a bounded one-time import when the requested Turso thread
      header is absent: require one regular rollout under the configured runtime
      home, validate its ID and canonical workspace, reject active/archive
      ambiguity and symlinks, then atomically persist its header and ordered
      replay rows in Turso without changing the source file. No JSONL fallback,
      schema compatibility or migration layer is supported.
      The importer is wired through `ahead-agent/src/native_client.rs` and the
      Turso transaction in `ahead-proxy/src/ahead/store.rs`; plain/compressed
      reader, ambiguity/symlink lookup and atomic/idempotent storage regressions
      are added. Run those focused tests and verify resume after process restart;
      they have not been run against this source revision yet.
      thread-settings event path now persists through the normal thread store
      without probing for a local rollout. Shell-snapshot cleanup expires
      snapshots by their own modification time instead of treating every
      Turso-only session as an orphan. Managed startup no longer queries a local
      rollout path, and `SessionConfiguredEvent`/the live thread no longer
      expose or compare one; the dead `LiveThread`/`Session` local-path stubs
      are removed, and active resume identity is the Turso thread ID.
      Session-start errors no longer tell Turso users to repair a Codex
      `sessions/` directory. `codex-protocol` (313) and `codex-rollout-trace`
      (61) tests pass. The copied `codex-core` test harness is not runnable
      and still contains old `RolloutRecorder`/session-file and retired
      `codex_extension_api` test calls; do not restore those deleted runtimes
      just to revive this unsupported harness. Use the production library
      check for that crate. The unreferenced copied `source/ext` extension API,
      built-in items and skill host were removed; AHEAD's own skills and
      editor-owned lifecycle remain the product paths. The Codex rollout crate was
      reduced from 11,626 to 963 Rust lines: it now keeps only replay policy,
      model-context scanning and fork cutoff logic. Local rollout reading or
      writing, compression maintenance,
      filesystem session listing/search/name indexing, and persistence
      telemetry were removed. Its direct runtime dependency tree no longer
      includes `codex-git-utils`, `codex-otel`, or `tokio`; `cargo machete`
      reports no unused direct dependencies. The five focused importer/history
      tests pass, as does the serial offline managed-agent suite (156 passed,
      3 ignored) when its localhost mock sockets are permitted. On 2026-10-07,
      the existing `ahead_proxy` test binary (00:44:18, newer than
      `ahead-proxy/src/ahead/store.rs` but older than the current
      `ahead-rpc/src/ahead.rs` DTO changes) ran all 40 `ahead::store::tests`:
      38 passed in the restricted run; the two
      mock-provider tests initially failed at loopback bind with `EPERM` and
      then passed individually with local-socket permission. Coverage included
      Turso reopen/session history, memory-index revisions, one-time legacy
      import safety, anchor persistence and native-thread spawn/resume. No Cargo
      build or external provider was used.
      Rechecked on 2026-10-08 with the existing `ahead_proxy` runner; it is newer
      than the `ahead-proxy/src/ahead`, `ahead-agent/src`, and `ahead-rpc/src`
      Rust sources exercised here (the later plugin-catalog edit is unrelated).
      All 41 store tests passed with local-only mock-provider sockets enabled,
      including durable spawn/resume and child-edit anchor attribution. This is
      still mock-provider evidence, not a live configured-provider turn.
      The dependency audit also removed
      unused `anyhow` and `codex-git-utils` edges from
      `codex-prompts`; its locked/offline check and 23 tests pass. A full
      `cargo machete --with-metadata` scan of `ahead-agent` and the retained
      runtime source now finds no unused direct dependencies. A fresh
      native-window pass was not possible on 2026-10-05 because the Mac was
      locked, so no rendered-app verification is claimed for this prune.
      serial locked/offline library suite passed 100 agent and 138 proxy
      tests, including current-child restore and missing-thread no-fallback
      checks. The follow-up schema cleanup also passed the full 100-agent /
      138-proxy suite: current databases reopen unchanged, unsupported database
      files remain byte-identical, and workspace storage failure disables
      agent requests while editor saves still work. The schema uses application
      ID `0x41484544` and version `1`; there are no upgrade/backfill branches
      or non-durable workspace fallbacks. A 2026-09-30 running-app pass in
      `/private/tmp/ahead-schema-smoke.oKBqbO` verified fresh bootstrap,
      a wizard-created session restored after restart, and the storage-disabled
      status with continued file opening in a separate invalid-database
      project. Starting a session in that project attached none; the invalid
      file remained byte-identical. The follow-up native check displayed the
      full failure inline in the thread list, creation wizard and external
      agent picker; Back preserved the description and cleared the error.
      No provider turn was sent. Pi and the opt-in
      Node/npm smoke were ignored in this suite.
      The copied SQLite agent-graph adapter and its direct DB tests are also gone;
      Codex-core test managers use the in-memory graph fixture. The rollout
      suite passes (102 tests), as does the focused `codex-thread-store`
      paginated suite (2 tests). `Cargo.lock` contains no `codex-state`, SQLx
      or `libsqlite3-sys`, and `cargo tree -p ahead-agent -i sqlx` finds no
      such package in the normal dependency graph. The final
      `cargo check --offline -p ahead-agent --lib -j 1` could not finish because
      the first `ahead-agent` check ran out of space while writing an `.rmeta`
      file and emitted no Rust diagnostic. After temporary test artifacts
      cleared, the retry passed with 0 errors and 3 warnings. The volume had
      2.3 GiB free after that check. Do not clear the shared 122-GiB `target/`
      cache while agent builds may depend on it.
      The runtime `experimental_thread_store` selector and config-driven helper
      are removed; the old key now fails fast rather than silently selecting or
      falling back to another store. Retained core test managers and
      agent-control fixtures inject `InMemoryThreadStore` explicitly. Those
      scenarios verify loop behavior only, not durable history paging.
      `removed_thread_store_selector_is_rejected` covers the obsolete setting;
      the copied core test target is not maintained, so product behavior must
      be verified through AHEAD-owned tests.
      `TursoThreadStore` now creates
      paginated threads, uses reverse ordinal-keyset pages with
      `ModelContextScan` for model context, and keeps metadata-only hydration
      separate from replay rows. Cold metadata reads now preserve the Turso
      header's created/updated timestamps instead of replacing them with the
      in-memory store's current time; the focused regression failed before the
      fix and passes after it (2026-10-02). The agent library suite passes 145
      tests with 3 ignored. Fresh in-memory threads now retain stable
      created/updated timestamps across reads; the regression failed before
      the fix, all 19 thread-store tests and the 145 active agent tests pass
      with one test thread. A parallel agent-suite run timed out seven local
      mock tests without receiving model requests. Ubuntu CI now uses the
      same serial library-test setting as `just test-all`; investigate fixture
      contention before treating default-parallel runs as reliable.
      Core resume-by-ID requests full history only for
      legacy threads. Indexed native-thread listing now
      filters the latest source, provider and workspace metadata patches before
      the keyset limit;
      the focused promoted/demoted thread regression and two companion filter
      tests pass (2026-09-29). The
      `native_agent_model_issued_spawn_and_resume_survives_turso_reopen` test in
      `ahead-proxy/src/ahead/store.rs` exercises model-issued child creation,
      paginated history after 71 child turns, Turso reopen and cold child resume.
      AHEAD now exposes a native `spawn_agent` dynamic tool without enabling
      copied `Collab`/v1 or `MultiAgentV2`. The shape follows Zed's
      `../zed/crates/agent/src/tools/spawn_agent_tool.rs` and
      `../zed/crates/agent/src/agent.rs::create_subagent_thread`: a distinct
      persisted child thread can be created or resumed by `session_id`, at most
      one level below the root. Child turns use the parent's workspace,
      model/provider, mode, file scope, and editor-buffer/presentation route;
      tool replies still go to the child thread. Child chat text is returned to
      the parent model rather than being streamed as duplicate parent chat.
      The child edge is persisted in Turso before startup and reopened/closed
      around its turn; cancelling the work session interrupts loaded descendants
      before the root. A focused UTF-8-safe bound limits child output. This path
      has a file-backed Turso regression that scripts model-issued spawn,
      71 child turns, process/client reopen and model-issued resume by child ID.
      The current regression passes with its context-boundary and open-standard
      assertions: project `AGENTS.md` reaches the native request and colocated
      Zed-specific `.rules` does not. It uses an offline mock model server on
      loopback and passes with `cargo test --locked --offline -p ahead-proxy
      native_agent_model_issued_spawn_and_resume_survives_turso_reopen -j 1`
      (also passed in the full 140-test proxy library run on 2026-09-30; re-ran
      successfully in the current worktree on 2026-10-03). The 2026-10-05
      rerun caught a stale fixture: its first scripted spawn response was sent
      to the runtime's `GET /v1/models` catalog request, so the tool never ran.
      The mock now answers that catalog request separately and counts only
      Responses requests; the Turso reopen/resume regression then passed again.
      The fixture uses an offline mock on `127.0.0.1`; restricted runners must
      allow loopback binding. `ahead-agent` now has a focused CodeAnchor test
      for the parent work-session ID, AHEAD actor, changed path/range and quote
      hash, plus `native_agent_cancel_interrupts_an_active_child_before_the_parent`,
      which verifies an active child is interrupted and its Turso edge closes
      (re-ran 2026-09-29; 1 test). On 2026-10-06, direct execution of the
      current proxy test binary passed
      `native_agent_child_file_edit_records_parent_work_session_anchor`,
      `dispatcher_commit_uses_anchor_attribution_and_clears_rows`, and
      `test_anchor_attribution_coalesces_and_clears`. The child edit used a
      loopback-only mock provider; no external model or credentials were used.
      These verify persisted attribution and commit cleanup, not the native
      gutter-to-commit interaction.
      On 2026-10-07, reran `test_anchor_attribution_coalesces_and_clears` and
      `dispatcher_commit_uses_anchor_attribution_and_clears_rows` from the
      05:10:06 proxy test binary, newer than both `store.rs` and `dispatch.rs`;
      both passed without Cargo or an external provider request. Native gutter
      rendering and click-through remain unverified.
      `native_agent_child_file_edit_records_parent_work_session_anchor` now
      drives a model-issued spawn and child `exec_command`/`apply_patch` through
      `AheadSessionHost`, then verifies Turso persisted the changed-file anchor
      against the parent work session; it passes with
      `cargo test --locked --offline -p ahead-proxy
      native_agent_child_file_edit_records_parent_work_session_anchor -j 1`
      (1 passed; re-ran 2026-09-29).
      The fixture canonicalizes its temporary workspace for the macOS sandbox
      and uses a local mock on `127.0.0.1`; restricted runners must allow
      loopback binding. The spawn/resume/edit/cancel regression gates now pass;
      the broader managed-agent production journey still needs a real turn and
      rendered editor validation.
      On 2026-10-02, a serial unrestricted full proxy run aborted in Wasmtime's
      macOS Mach trap-handler thread near the longer spawn/resume regression;
      the child-edit test passed in isolation. AHEAD's extension host created
      one Wasmtime engine per catalog, unlike Zed's process-wide engine.
      `ExtensionHost::new` now reuses a process-wide engine and selects
      Wasmtime's Unix-signal trap handling on macOS, where its pinned docs warn
      that Mach ports do not work well with `fork()`. Sharing the engine alone
      did not resolve the abort; with signal mode the unrestricted serial proxy
      suite passes 212 tests (1 ignored), and the extension-host suite passes
      21 tests (2 ignored). Recheck after future Wasmtime upgrades and in the
      running GPUI app. Only older generated test binaries and one older proxy
      incremental cache were removed to recover linker space; they can be
      rebuilt.
      Removed `core/Cargo.toml`'s SQLite-only dev-dependencies and test-only
      state/config seam. The focused `codex-thread-store` paginated tests passed
      after its local adapter was removed, while the rollout library check
      passed after its SQLite listing/backfill removal. A current
      source/manifest and lockfile audit, rechecked 2026-09-29, finds no
      `codex-state`, SQLx, `rusqlite`, or `libsqlite3-sys` package; `ahead-proxy`
      depends only on the Turso `libsql` client for this database path. Managed
      native sessions construct
      `TursoThreadStore`, while retained local and in-memory runtime stores are
      test-only. The direct `codex-rollout` library suite passed 103 tests on
      2026-10-02. Retain model-loop tests that
      can use `InMemoryThreadStore`; remove only tests whose behavior is
      specifically the deleted SQLite adapter and is covered by AHEAD's
      file-backed Turso regressions.
      A prior Core filtered test-target attempt failed before execution because
      retained tests used the `test_case` proc macro, absent from the workspace
      and lockfile. Those invocations are now removed across `core/src`; the
      behavioral cases use ordinary loops or separate Tokio test functions.
      `tools/handlers/request_user_input_tests.rs` now expresses its
      five answer cases as separate `#[tokio::test]` functions, and
      `context/world_state/context_window_guidance_tests.rs` covers ten cases in
      two plain tests. `mcp_tool_call_tests.rs` now has two named Tokio tests;
      config-edit, history metadata, terminal sandbox backend, turn-input,
      step-activation, rollout reconstruction, session terminal/compaction,
      Guardian authorization, and tool-output truncation cases use ordinary
      loops. Step-settings provenance cases now use separate Tokio tests.
      Plugin-manager fixtures and plugin-only config tests listed here have since
      been removed or migrated to AHEAD's current MCP and skill paths.
      Rustfmt parses all converted source, but the forced
      `cargo test --locked --offline -p codex-core --lib -j 1 --no-run` target
      failed with 299 errors on 2026-10-02; a later no-run check still fails
      with 291 errors. The disabled copied test modules
      still import removed dev fixtures/crates (`core_test_support`,
      `wiremock`, `tracing_subscriber`, `pretty_assertions`, and others) and
      reference at least one deleted helper. Do not restore the entire upstream
      test dependency graph just to make this non-product target compile;
      identify and port the model-loop, compaction, tool-dispatch and
      cancellation cases not already covered by AHEAD-owned integration tests,
      then prune obsolete copied test modules. The unused
      `x-oai-attestation` provider/header seam and its 131-line ChatGPT-only
      copied test block are removed; AHEAD never supplied an attestation
      provider. Production crate checks and the 134-test managed-agent suite
      pass after that removal. The
      `session/tests/approval_authority_tests.rs` retains the ordinary safety
      cases after removing obsolete automatic-review routes and the dead
      Guardian-subagent fixture.
      Retained coverage checks AHEAD permission events, captured turn authority,
      retries and legacy-history compatibility. The unified-exec missing-input
      case remains as `unified_exec_rejects_missing_additional_permissions`
      because it tests required arguments, not Guardian behavior.
      The orphaned Guardian fallback/model-safety block was removed from
      `step_activation_tests.rs`; retained AHEAD coverage exercises destination
      model metadata resolution, task replacement and retained turn authority.
      The copied-core test
      `spawn_agent_fork_from_paginated_parent_uses_model_context_prefix` is
      intentionally not ported: it forks full parent history, while Zed's
      `SpawnAgentToolInput` explicitly says child agents do not see the parent
      conversation. AHEAD follows that boundary: `spawn_subagent_session` uses
      `InitialHistory::New`, and the explicit `message` is the context handoff.
      The Turso model-loop regression now asserts that a parent-only sentinel is
      absent from the child's first request while the explicit child message is
      present. The unused copied `ThreadManager::spawn_subagent` full-history
      fork entry point is removed, and its ID-factory test now covers actual
      child creation only. The file-backed Turso model-loop regression passes
      after that removal (1 passed, 196 filtered), as do the 134 managed-agent
      library tests; the copied core test target is still not a product gate.
      Copied rollout-path resume/fork tests now use thread IDs or loaded history;
      the `ThreadStore` path lookup and Turso's unsupported stub are removed.
      The copied core test target still fails with 291 existing missing-fixture
      errors; none of the reported errors points to these migrated test calls.
      The zero-caller reference-backed prepared-fork DTO, store operation and
      session branch are removed; all 19 thread-store and 134 managed-agent
      tests pass after this cleanup.
      The separate zero-caller `ForkSnapshot`/`fork_thread_from_history`
      path, its copied fork-only tests, and orphaned user-message truncation
      helpers are removed. The 134 managed-agent tests pass after this prune.
      The disabled core V1 `spawn_agent` tool no longer advertises or accepts
      `fork_context`; its fork-only tests are removed, while the independent
      non-fork service-tier case remains. The core V2 `fork_turns` option,
      shared agent-control history-copy branch, unused rollout-truncation
      module and copied fork-only tests are also removed. Both core tiers
      create fresh children with explicit task messages; AHEAD's managed
      `spawn_agent` dynamic tool is unchanged. The deleted truncation module
      and tests are recoverable from Git if needed.
      Keep the copied core test harness pruned.
      Turso-backed thread listings and metadata-only reads hydrate headers and
      patches without replay rows; legacy full-history callers still load the
      complete replay, while paginated resume reads backward in bounded pages.
      Replay rows already have the composite primary key `(thread_id, ordinal)`,
      so keyset paging needs no duplicate index. Native adapter regressions
      cover exclusive page cursors, a 300-row no-cutoff scan, and stopping at a
      safe checkpoint across page boundaries. The existing Turso model-loop
      regression covers paginated child history across reopen/resume. A store
      regression now proves one invalid-UTF-8 session ID cannot crash listing
      or hide a healthy session, and selecting a session with damaged workflow
      text returns an error instead of panicking. These reads validate text as
      bytes because the retained libSQL decoder panics on malformed SQLite TEXT.
      Native thread header, patch and replay reads now use the same byte
      validation; a regression corrupts replay, timestamp and metadata text
      and confirms full-history, keyset-page and header-list reads return
      errors without panicking. It also binds the damaged replay to a work
      session and confirms that session remains in the metadata-only sidebar
      list (focused test passes on 2026-10-02). Verify
      large/partially-corrupt history startup and selection in the rendered app.
      Harness binding and native thread linkage now commit in one Turso
      transaction; an injected second-write failure rolls back the binding,
      and a subsequent retry succeeds in the focused regression.
      `SessionStore::list_sessions`
      projects sidebar metadata and harness bindings before the app loads a
      full `SessionView` on selection, following Zed's metadata/detail split.
      Its task-parent projection now groups task links once instead of running
      two unindexed `session_tasks` lookups for every sidebar row; the focused
      store test preserves one row per session and delegated-parent metadata
      with multiple tasks, without a database migration. The proxy library
      suite passes 196 tests (1 ignored) after this query change.
      File-backed tests cover native-thread reopen, chat writes from a Tokio
      worker, and paginated child resume after database reopen. `native_agent_spawn_edges_survive_reopen_and_thread_deletion`
      verifies Turso-backed spawned-agent edges across database reopen, while
      the native adapter test verifies descendant ordering and status filters.
      The model-issued spawn regression persists and reopens the parent/child
      link, asserts paginated history for both threads, completes 71 child
      model turns against a local mock Responses API and verifies more than 128
      durable replay rows. After reopening file-backed Turso, it cold-resumes
      the child by model-issued `spawn_agent(session_id)` and verifies that
      persisted child context reaches the next model request.
      Scalar and structured source, model-provider and cwd filters now apply in
      SQL before the timestamp keyset/limit, with expression indexes retaining
      created/updated ordering. Direct-child and transitive-descendant filters
      apply in SQL before the timestamp keyset/limit using persisted
      `parent_thread_id`; a recursive CTE plus parent expression index keeps
      descendants across page boundaries without hydrating every header. The
      Mapping regressions re-ran in the current worktree on 2026-10-03: the
      agent page-filter set passes 3 tests and the proxy native-thread header
      page set passes 5 tests. The mapping test passes with
      `rtk cargo test --locked --offline -p ahead-agent --lib
      indexed_header_page_request -j 1` (3 passed); the proxy page regressions,
      including mixed scalar/structured source matching, pass with
      `rtk cargo test --locked --offline -p ahead-proxy --lib
      native_thread_header_page -j 1` (5 passed). Case-sensitive substring
      search now filters name, preview, title and first-user-message metadata
      in SQL before the timestamp keyset/limit, preserving current-patch and
      name-clear semantics without hydrating nonmatching headers. Correlated
      latest-patch reads use the existing `(thread_id, ordinal)` primary key;
      this is still substring matching, not indexed full-text ranking. Assigned
      project/section filters and recency/section-position ordering still use
      full header hydration pending indexed summaries. Requests for unassigned
      projects and unsectioned threads now take the indexed keyset path because
      this hard fork cannot assign either; the focused request regression
      failed before that change and passes after it. The indexed keyset path
      hydrates metadata only for the requested page.
      Verify startup and selection in a rendered app with large and partially
      corrupt history, then drive the persisted model-issued child spawn/resume
      path through a GPUI app restart. The file-backed NativeClient/Turso
      regression covers model-issued resume after reopening the client and
      database; it does not prove the visible app restore flow. A focused
      `NativeClient` regression covers root-only tool exclusion after
      restoring a child from current Turso metadata. Resume reads Turso first;
      only a missing Turso header may trigger the validated one-time import.
      All subsequent resume and writes use Turso, with no live file fallback.
      Keep visible conversation and model-replay rows separate within `.ahead/session.db`;
      they have distinct UI and replay contracts. Managed turn startup now uses
      the indexed next-message sequence to detect the first message instead of
      materializing the full conversation for title generation. `retry_request`
      now scans bounded indexed pages for the target failed/cancelled agent turn
      instead of materializing the full conversation; its worker-failure
      regression places the retry target behind 100 newer messages. The focused
      locked/offline test passed on 2026-09-28 (1 passed, 78 filtered).
- [ ] Finish managed-skills provenance and policy: the runtime system-skill
      bundle now comes from `ahead-agent-skills/builtins/` rather than Codex sample
      assets, while project/user discovery remains `.agents/skills/` and
      `~/.agents/skills/`. The current-binary 2026-10-13 `/calendar` managed
      turn smoke selected a project skill and verified its body reached the
      first model request. Native metadata-only palette presentation remains
      unverified. The root-resolution regression now creates `.agent/skills/`
      and `.skills/` decoys and asserts only the project `.agents/skills/` root
      is discovered. The slash catalog preserves same-named skills by source
      and uses source-qualified names without exposing absolute
      paths. A stale or ambiguous slash-skill selection now returns an error at
      native invocation instead of silently becoming plain text; the focused
      `slash_skill_selection_rejects_ambiguous_names_and_resolves_qualified_names`
      test passes. Managed turns now persist each explicitly selected skill's
      opaque ID, scope, source-file SHA-256, exact injected-prompt SHA-256, and
      human selection actor in the existing Turso turn-request JSON; no table
      migration or schema-version check is needed. The 2026-10-14 native
      managed-turn regression verifies the prompt hashes against the mock model
      request; the Turso reopen regression verifies durable storage. Nested
      reads now persist the opaque skill ID, scope/name, package-relative path,
      source-content SHA-256, exact returned-page SHA-256 and `agent` loader in
      that same request JSON. The 2026-10-14
      `native_agent_advertises_and_dispatches_ahead_dynamic_tools` regression
      uses a mock model in bundled GPT-5.5 direct-tool mode to verify the host
      advertises and dispatches `skill_resource_read`, returns nested content
      in the follow-up model request, and persists provenance without a host
      path. The Turso close/reopen regression verifies deduplicated durability.
      Model-specific CodeModeOnly filtering remains unchanged; this test does
      not force a hidden call through that mode. Host-enforced
      skill-resource authorization now requires the opaque ID from that turn's
      `SkillContextLoaded` event; the managed mock-model regression also checks
      that a fabricated, unselected handle is denied while the selected skill's
      nested reference remains readable. This source regression has not been
      run here; broader host-enforced capability policy remains open.
      Host discovery now retains same-named entries across scopes; opaque host
      locators include scope plus a SHA-256 path fingerprint, and slash
      activation rejects ambiguous unqualified names. On 2026-09-28, the full
      `ahead-agent-skills` library suite passed (15 tests), including project/user
      duplicate-name policy, `.agents/skills/` root selection, resource
      containment and oversized-file diagnostics. The native resolver
      regression for ambiguous and qualified slash names also passed (1 test).
      The host snapshot resolves opaque package handles to enabled skills; its
      discovering filesystem enforces a 1 MiB resource bound, rejects traversal
      and checks canonical containment against symlink escapes. AHEAD's native
      `skill_resource_read` calls that AHEAD-owned host boundary directly (not
      the removed generic extension provider) and pages large UTF-8 responses.
      Package-resource containment coverage is included in the passing
      library suite; the native UTF-8 pagination/stale-cursor regression passed
      on 2026-09-28 (1 test).
      Selected-skill prompt bodies now stay within an 8,200-byte cap after
      metadata wrappers and envelope-tag neutralization; the skills suite
      covers repeated `</skill` content that previously exceeded the estimate.
      `SKILL.md` loading now enforces the Agent Skills specification's required
      name/description, name syntax, parent-directory match and length limits;
      parser regressions now cover repository and bundled packages, and an
      offline host-catalog regression checks that an invalid project package is
      excluded and increments a path-free warning count. The slash palette now
      displays a non-selectable warning entry alongside valid skills; verify
      the rendered row against invalid project/user skill packages in `just dev`.
      The rendered slash-palette filtering/selection regressions pass in the
      focused AHEAD app suite (9 tests). The
      `agent_skills_rpc_returns_project_source_without_a_host_path` regression
      also passed on 2026-09-28 (1 test), confirming path-free project metadata
      and skipped-package counts across the RPC boundary.
      Discovery now rejects SKILL.md files over 100 KiB from metadata before
      streaming and rechecks the byte count while reading, so stale metadata
      cannot bypass the cap. It allows two concurrent skill loads per root and
      eight roots at once, for at most 16 concurrent loads. The focused
      size-limit regression passed as part of the full skills library suite.
      The Agent Skills specification requires valid name syntax, a parent-
      directory match, and the 64-character name limit. Its client guide
      recommends warning-and-loading mismatched or overlong names as an
      interoperability policy; AHEAD currently follows the normative
      constraints and rejects those packages. Verify invalid-package
      diagnostics in the running app and ensure they omit absolute host paths.
      The runtime now refreshes its workspace-root-to-turn-cwd instruction
      snapshot once per new turn, so edits are picked up without re-reading docs
      for every tool step. The model context now labels each loaded project
      source path with the full-file SHA-256 and states
      nearest-applicable-file precedence.
      The native/Turso loop regression now asserts project `AGENTS.md` content
      reaches the first model request while a colocated `.rules` sentinel does
      not; `native_agent_model_issued_spawn_and_resume_survives_turso_reopen`
      passed in a focused locked/offline run on 2026-09-28 (1 passed, 115
      filtered).
      The managed runtime gives bounded guidance to inspect applicable
      ancestor instruction files before side effects, without recursively
      scanning unrelated subtrees. The host now also resolves structured
      active-editor and attached-file targets inside the selected workspace
      and adds their nested `AGENTS.md` instructions and full-file hashes to
      the first native prompt; it does not parse target paths from free-form
      text or reinterpret retained `UserInput::Mention`, which is for
      app/plugin connector mentions. Outside-workspace and symlinked targets
      are ignored. AHEAD caps each nested instruction file at 1 MiB and the
      combined prompt addition at 32 KiB. Focused tests cover nested active and
      attached targets, unrelated/outside paths and symlink rejection; the
      `loads_nested_agents_instructions_for_only_structured_editor_targets`
      case passed on 2026-09-29 (1 test). The attached file's
      captured text remains fallible reference context, not instructions.
      On 2026-10-06, the existing ahead-agent test binary reran all three
      `fim_` hierarchy/budget regressions successfully (3/3); this verifies
      instruction assembly, not rendered or provider-backed completion.
      On 2026-10-07, all seven `instructions::tests` passed directly from the
      existing ahead-agent test binary, which postdates `instructions.rs`;
      this adds headless coverage for root/ancestor hierarchy, repository-root
      discovery, and symlink/replacement rejection. It does not verify the
      separate host skill-root tests or native skill invocation.
      Unix target-instruction reads now use the shared no-follow directory
      handle, so replacing an ancestor with an outside symlink cannot redirect
      the read after target validation. The shared helper's focused
      disposable-project regression and all 34 `ahead-core` tests pass
      (2026-09-29), and
      a locked/offline `ahead-agent --lib` check passes. The agent-specific
      `target_instruction_read_rejects_replaced_parent_directory` regression
      passed on 2026-09-30 (1 test). Windows uses the retained runtime's
      `NtCreateFile` no-reparse open, with a regression for replacing an
      ancestor by a reparse directory. Its shared path conversion now rejects
      parent-directory components to match Unix; the focused Windows-only
      regression has not been run on this host.
      The host now persists each added target instruction's workspace-relative
      source path, complete-file SHA-256 and affected targets in Turso's
      `turn_instruction_sources` table, in the same transaction as the turn
      request. These records survive retry-state cleanup; the retained core's
      root-to-working-directory instruction loader remains core-owned. New
      loader and file-backed Turso reopen regressions cover path privacy,
      source/hash mapping and retention after request cleanup. Both passed on
      2026-09-28 with locked offline tests and were re-run on 2026-09-29:
      `cargo test --locked --offline -p ahead-agent --lib
      loads_nested_agents_instructions_for_only_structured_editor_targets -j 1`
      and `cargo test --locked --offline -p ahead-proxy
      instruction_sources_survive_turso_reopen_after_retry_state_is_removed -j 1`.
      Explicit managed retry now rejects a changed target-scoped instruction
      set, and the author context is replaced rather than duplicated. The
      agent library suite (148 passed, 3 ignored), focused proxy Turso reopen
      regression, and full proxy suite (221 passed, 3 ignored) pass on
      2026-10-02. The retained core now exposes the exact loaded root-to-working-directory
      source paths and full-file hashes for the completed core turn. The native
      host writes that snapshot into the saved Turso turn request after the
      core refresh; an empty list is valid, while a missing snapshot fails
      retry closed. Explicit retry reloads the managed thread, runs core-owned
      discovery again, and compares ordered path/hash pairs as well as the
      target-scoped source set. No second AHEAD project-root scanner is used.
      Locked/offline library and all-target checks pass for `ahead-agent` and
      `ahead-proxy` (2026-10-03). The focused Turso reopen regression and
      worker-start fail-closed retry test each pass (1 test each). The
      managed mock-model file-search test also passes (1 test, loopback),
      proving the `AGENTS.md` text reached the model and its source hash reached
      the AHEAD store; its same-thread rediscovery check accepts unchanged
      sources and detects an edited `AGENTS.md`. The native durable-child
      fixture now also passes after a clean Turso/native reopen: it records
      the managed parent's project source snapshot, then exercises the real
      `retry_request` gate against a persisted failed message: unchanged
      instructions permit the saved request and an edited `AGENTS.md` rejects
      it. The failure status in that fixture is simulated after a clean
      shutdown. `tests/agent-shutdown-smoke.mjs` previously passed against the
      rebuilt proxy for explicit shutdown, stdin EOF and SIGKILL. Its SIGKILL
      case preserved the partial message in Turso but rejected retry because
      core provenance had not reached the store. The managed loop now watches
      the exact Core snapshot and persists it as soon as it becomes available,
      while the request is still streaming; retry remains fail-closed if no
      snapshot was captured. A focused mocked-model regression holds its first
      response open and requires the path/hash snapshot to be durable before
      releasing it. The SIGKILL smoke now expects one explicit retry and still
      asserts no automatic replay. On 2026-10-08, the updated
      `tests/agent-shutdown-smoke.mjs` passed against `target/debug/ahead`
      (built at 10:22, newer than `native_client.rs` at Oct 7 14:15,
      `session.rs` at Oct 8 07:35, and the retained Core session sources):
      explicit shutdown and stdin EOF persisted partial output and cancellation
      through Turso reopen; SIGKILL persisted failure plus instruction
      provenance, did not auto-replay, and allowed exactly one explicit retry.
      Provider authentication and a live rendered chat remain separate gates.
      On 2026-10-08, the disposable `tests/agent-file-attribution-smoke.mjs`
      passed against the existing app binary: the actual managed model request
      included root `AGENTS.md` and the active `src/AGENTS.md`, excluded the
      unrelated `data/AGENTS.md`, applied its Code Mode patch, and persisted the
      resulting CodeAnchor in Turso. Its loopback-only mock did not exercise
      crash-retry or an external provider.
      The new core
      cache regression has not run: the copied `codex-core --all-targets`
      target still fails with 298 unrelated missing-fixture/import errors.
      A retry can still
      repeat already-run tools;
      keep it an explicit human action, not automatic replay.
      Remaining: cover
      concrete targets at direct editor-owned effect boundaries, and do not
      claim host preflight for arbitrary shell command text. Keep model guidance
      until shell effects have a separately mediated boundary.
      External ACP processes retain their own AGENTS.md discovery behavior.
      AHEAD uses repository `AGENTS.md` hierarchy only; no AHEAD-specific
      home-level instruction file is required or loaded.
      Namespace local skill provenance, reject policy conflicts, and never
      auto-install or execute discovered scripts/network effects.
      Activate end-of-session `ahead-code-review` against an immutable snapshot;
      findings must remain non-mutating until a human chooses a disposition.
      Revalidated on 2026-10-14: locked/offline `ahead-agent-skills --lib`
      passed all 16 tests. This is loader/provenance unit coverage, not a
      rendered slash-palette or native invocation check.
- [ ] The built-in AHEAD runtime now directly hosts the retained core through
      `ahead-agent/src/native_client.rs` and is the default tier; verify a real
      multi-tool turn against the agreed policy. Learn must stay read-only;
      Assist writes are workspace-bounded by default and explicit non-empty
      paths narrow that scope. Per-edit approval cards are not part of the
      managed UX; ACP remains compatibility-only. The streamed-turn active-file
      preflight now uses the same segment-aware normalized path matcher as the
      native write boundary. Its sibling-prefix regressions,
      `scope_rejects_parent_traversal_and_accepts_descendants` and
      `test_streamed_agent_turn_stale_policy_and_scope_reject` passed on
      2026-10-02. The real managed multi-tool policy journey remains open.
- [ ] Finish removing unused upstream config-loader code. The retained
      loader no longer has a system or project config-file layer, and Codex
      project-config discovery remains removed. On 2026-10-20, a caller audit
      found only unused generic wrappers and no AHEAD consumer for global MCP
      config loading or the old OSS-provider writer; those surfaces and the
      explicit system-config layer were removed while retaining the active
      ConfigBuilder path and administrator requirements. On 2026-10-22, the
      follow-up audit also removed the zero-caller `ahead-agent-engine/core/src/config/edit.rs`
      TOML mutation module, the unread `[desktop]`, `[goals]`,
      `[ghost_snapshot]` and `oss_provider` config sections
      (`validate_oss_provider`, `resolve_oss_provider`), and replaced
      `strict_config_accepts_opaque_desktop_keys` with
      `strict_config_rejects_retired_codex_product_sections` (`rtk cargo test
      --locked --offline -p codex-config --lib -- strict_config` passed 9/9).
      The broader core/workspace compile remains pending while `cola-v2`'s
      workspace Cargo check is active (25 GiB free).
      Host-managed requirements remain a separate policy layer. The AHEAD
      runtime also ignores Codex login requirements. The
      unused Codex cloud-config bundle loader, fragment parser, and
      enterprise-managed provenance variants are removed from the retained
      config crate; local/system and MDM requirement layers remain. Maintained
      config tests now cover system-policy precedence directly. The Codex
      `managed_config.toml` and MDM `config_toml_base64` config layers, their
      precedence/source variants, and the cloud insertion index in the executor
      config RPC are now removed; only AHEAD-namespaced system and modern MDM
      requirements remain. The unused executor `environmentConfig/read` RPC,
      its protocol DTOs, and the local config projection loader are also removed;
      the executor no longer reads host configuration on its own. The disabled
      copied `codex-core` test sources still contain stale cloud fixtures but
      are excluded by that crate's `test = false` setting and are not an AHEAD
      test gate. Model-visible shell, patch, image and permission tools no
      longer accept Codex's remote `environment_id`; they resolve against the
      turn's primary local environment, while internal IDs remain for scoped
      approval bookkeeping. Remote-executor-only tool-spec tests were removed.
      The default workspace metadata carve-out now protects `.ahead` rather
      than the copied `.codex` directory, including missing-path and symlink
      cases. Windows sandbox setup no longer changes ACLs on Codex-install
      runtime directories; the AHEAD sandbox helper path is unchanged. Native
      Windows setup still needs host verification.
      On 2026-10-05, `codex-config --lib` passed all 189 retained tests and
      `cargo check --locked --offline -p ahead-agent --tests -j 1` passed with
      zero errors after the config-layer removal. `cargo +nightly fmt --all -- --check`
      and `git diff --check` also pass. The Codex config-schema generator and its only in-tree consumers (fixture tests
      and editor-integration notes) have been removed. AHEAD does not edit or
      publish the Codex config schema, and the shell-snapshot test no longer
      names its removed generator binary. Runtime config parsing remains because
      managed provider settings still use the retained loader. The unused
      thread-config injection abstraction, remote gRPC client, generated
      protocol and codegen script have been removed; AHEAD has no remote config
      service boundary. The
      AHEAD-owned MCP configuration is now wired: tracked `.ahead/config.toml`
      declares local stdio servers and ignored `.ahead/settings.toml` explicitly
      opts in by server ID; unknown IDs, inline tracked secrets and unimplemented
      HTTP authentication fail closed. `NativeClient::build_config` replaces
      inherited Codex/runtime MCP configuration with only the selected AHEAD
      servers. Assist defaults every tool's approval mode to `prompt`, except
      named workspace-local allow/deny choices; read-only
      and Learn sessions disable them, and external ACP receives no
      user-configured servers. This schema is AHEAD-owned, not MCP-standard or
      Zed-compatible configuration. Focused config and mode-policy regressions
      cover selection, disable, secret rejection and local tool choices. All
      16 `ahead-agent --lib mcp`-filtered tests passed on 2026-09-29 in a
      loopback-enabled runner; the ACP stdio cancellation test needed loopback
      permission. The 1 MiB
      config-file size regression passed in its focused locked/offline run
      (2026-09-29; 1 test). MCP TOML reads cap each file; Unix reads now reuse
      the shared no-follow component walker in `ahead-core/src/secure_fs.rs`.
      Ordinary MCP declaration/opt-in reads now reuse
      `ahead_core::config::read_ahead_config`, so they share the tracked-secret,
      size and symlink boundary with managed providers; four focused
      `workspace_mcp_` tests and the approval/private-settings regression pass.
      The Unix approval transaction keeps its directory-handle reader to bind
      the checked declaration to the lock; it now calls the same tracked-secret
      validator as the ordinary reader. A focused regression verifies both
      listing and direct approval reject a tracked credential without writing
      private settings.
      Unix approval now binds the lock, declaration/settings reads and atomic
      settings rename to one no-follow `.ahead` directory handle; new lock and
      temp files are owner-only. A disposable-project regression replaces the
      `.ahead` pathname after the handle opens and checks that the outside
      target remains untouched. The full `ahead-agent --tests` Cargo check
      passes; the parent-replacement and approval/private-settings regressions
      also pass in focused locked/offline runs.
      MCP approval now uses the shared `ahead-core::secure_fs` directory-handle
      boundary on Unix and Windows for declaration reads, lock creation,
      settings reads and atomic replacement. Windows uses `NtCreateFile` with
      `OBJ_DONT_REPARSE` for handle-relative opens and renames replacement files
      relative to the already-open directory. A Windows regression covers
      replacing the visible `.ahead` path while the operation retains its
      original handle; it has not been run. Verify the helper and approval flow
      on Windows before claiming cross-platform race resistance. Remaining: verify
      the full tool-call approval journey in a running GPUI window and keep the
      boundary explicit—AHEAD cannot control side effects internal to an
      opted-in server, and its writes do not receive managed CodeAnchors. The
      separate AHEAD-owned ACP editor bridge only exposes open-buffer reads and editor
      presentation actions. ACP `mcp/message` is an experimental extension,
      not stable ACP v1; use the standard `mcpServers` session configuration and
      stdio bridge as the interoperability baseline, matching Zed's
      `mcp_servers_for_project` and shared new/load/resume request builders in
      `../zed/crates/agent_servers/src/acp.rs`. Keep any `mcp/message` support
      optional and separately probed. Both transports implement modern
      `2026-07-28` discovery, required per-request version/capability metadata
      and response envelopes, while keeping legacy version negotiation through
      Zed's `2025-11-25` baseline. New metadata-validation and discovery
      regressions are covered. The stdio bridge now multiplexes up to 16 tool
      calls while continuing to read notifications; `notifications/cancelled`
      reaches the authenticated local bridge and settles pending editor
      presentation and buffer-snapshot requests. A bounded 30-second tombstone
      handles cancellation racing ahead of local request registration, and late
      responses are suppressed even if the JSON-RPC id is reused. A full
      loopback-enabled serial `acp_client::tests` run passed all 40 tests in the
      current worktree on 2026-10-03, including the end-to-end stdio regression.
      The 2026-10-01 socket
      regression also verifies a delayed, fragmented request on an accepted
      nonblocking socket; the blocking worker now normalizes its mode so
      macOS cannot fail mid-frame. Stdin and local socket requests now stop
      reading after 1 MiB plus one byte, rejecting oversized frames without
      waiting for a newline or EOF. The stdin reader terminates on oversized
      or invalid UTF-8 input rather than queuing trailing bytes as commands.
      Both regressions failed against the old unbounded reader; all 121
      enabled agent tests now pass, including fragmented reads and stdio
      cancellation (two opt-in tests ignored). Verify these fixes in the
      rebuilt app with an external agent once disk space and provider access
      are available.
      The raw stdio event queue in `ahead-agent/src/acp_client.rs` now has
      32 slots and a backpressure regression; local socket workers are now
      capped at 20, leaving four slots beyond the 16 pending stdio tool calls
      so normal full-capacity turns can still send cancellations. Local
      socket reads now have whole-frame deadlines (20 seconds for requests,
      30 seconds for tool responses, two seconds for cancellations), and bridge
      responses are capped at 2 MiB. Slow-drip and oversized-response
      regressions pass. Add end-to-end overload and cancellation races at the
      worker/queue limits; adversarial slow sockets can still occupy every
      slot. The caps do not bound all serialized UI results or
      prove native ACP interoperability.
      Keep ACP `mcp/message` optional and separately probed; it is
      experimental and not the interoperability baseline. Still verify against
      official protocol fixtures and Pi, Codex and Claude clients before
      claiming production interoperability. The user reconfirmed on 2026-09-28
      that Pi provider access is not yet available; do not attempt a live Pi
      turn until access is restored.
      The dormant credential-broker
      project-loader trust/env binding and requirements path is now removed;
      proxy configuration and managed network-requirement plumbing are no
      longer part of the retained runtime. The remaining broker fields,
      conversion, environment shims and broker-only copied tests have also
      been removed.
      The 2026-10-05 proxy cleanup removed `Feature::NetworkProxy`, the
      `codex-network-proxy` implementation and its call chain. The binary
      AHEAD sandbox network policy remains; cross-platform restricted-network
      verification is still required before exposing broader network access.
      Zed's HTTP transport and OAuth lifecycle remain reference material at
      `../zed/crates/context_server/src/transport/http.rs` and
      `../zed/crates/project/src/context_server_store.rs`; AHEAD has chosen the
      local stdio boundary for now and removed the dormant remote/auth code.
      The current workspace manifests and lockfile no longer contain
      `codex-keyring-store` or `codex-secrets`. Do not conflate MCP transport
      support with AHEAD's BYOK provider credentials. The retained core
      config fallback and Windows sandbox home lookups resolve AHEAD home rather
      than `~/.codex`; the unused executor config-read RPC is removed. Verify
      Windows setup and launch on a Windows host before claiming that boundary
      works.
      A locked offline `ahead-agent --lib` check and AHEAD-home resolver tests
      passed on macOS; native Windows launch remains unverified.
      The unreferenced Codex marketplace/plugin config-edit modules, public
      reexports, and unused runtime config fields/types are now removed. AHEAD's
      config reader ignores unknown legacy TOML keys. Re-run focused config
      validation when the shared Cargo build loop is available. The copied
      plugin feature toggles, recommendation gate and network-backed
      recommendation test are now removed; stale values are ignored. The
      plugin/marketplace managed-requirement schemas and MCP environment policy
      projection are also removed while general server requirements remain.
      The unused `tool_suggest` config, feature, and config-edit path and its
      copied tests are now removed. The copied plugin-catalog tests are pruned;
      finish validating the standard-frontmatter-only skill runtime; sidecar-
      only metadata and automatic MCP dependency installation are removed.
      Executor-side discovery now materializes only bounded `SKILL.md` content;
      it no longer reads plugin manifests, MCP declarations, or
      `agents/openai.yaml`. The unreferenced generic `SkillRootLoader` /
      snapshot-cache API, executor-environment metadata and protocol
      `SkillMetadata` DTO are removed. The copied skills-extension catalog,
      providers, installer, tools, selector experiments, telemetry and
      marketplace-facing config/render branches are now pruned. The renamed
      `ahead-agent-skills` crate retains only host-root discovery/cache,
      standard skill parsing, prompt fragments, invocation detection, and
      contained resource reads. Managed discovery and prompt injection use the
      cached host `.agents/skills` snapshot. AHEAD's direct
      `skill_resource_read` tool resolves handles through
      `HostSkillsSnapshot::read_package_resource` and pages bounded UTF-8 with
      content-fingerprinted cursors. The AHEAD-owned host/native library suite
      passes after the current pruning (91 passed, 1 ignored on 2026-09-29;
      the ignored Pi integration needs configured provider access) with
      `rtk proxy cargo test --locked --offline -p ahead-agent --lib -j 1`
      in a runner that allows loopback binds, both with default and serial test
      threads. The restricted sandbox denied three socket fixtures and one
      adapter-lock assertion failed once there, then passed in isolation and
      in both complete loopback-enabled runs. Verify a selected skill can read
      a referenced file in a live managed turn; do not use the copied
      `codex-core` unit-test target as a product validation gate (see the
      test-coverage decision above).
      On 2026-10-14, removed the unreferenced cloud-config bootstrap resolvers
      for system-proxy selection from `core::config`; repository-wide search
      confirmed there were no call sites. The regular post-load HTTP client
      factory and its system-proxy policy remain intact. The existing shared
      `just dev` watcher rebuilt `target/debug/ahead` at 21:16:58; rustfmt and
      diff checks passed. A 2026-10-20 source sweep found the only remaining
      `network_policy_decider` references were stale positional placeholders in
      two unified-exec tests; those call sites are now aligned with the current
      API. No Codex network-proxy feature or network-policy-decider production
      API remains. Keep the outbound HTTP proxy policy and binary
      `NetworkSandboxPolicy`: the former supports provider traffic on proxied
      networks, while the latter enforces the managed execution sandbox.
      On 2026-10-20, removed the unused Codex config-profile selector and
      profile-v2 file layering, including its protocol name type, profile
      schema, special merge paths, and reload/loader tests. Runtime config now
      has one user-config layer; AHEAD permission profiles under `[permissions]`
      are separate and remain. This pruning slice still needs a shared watcher
      build and workspace compilation; focused config tests pass below.
      A repository-wide callsite audit found no AHEAD consumers for Codex's
      in-product notice model. Removed its config schema/runtime projection,
      notice mutations and tests, plus the unreferenced model-preset notice-key
      module. A source sweep of the affected config, core-config and
      models-manager directories found no remaining notice, migration or
      model-preset references; scoped rustfmt checks passed. The focused
      `codex-config` test binary completed on 2026-10-20 with all
      165 tests passing after correcting stale test imports, duplicate test
      annotation, precedence ordering and expected merged config. These test
      fixes do not change runtime behavior. The model-picker GPUI test build
      was stopped before reaching its test when free space fell to 2.2 GiB;
      app/workspace compilation and native UI verification remain outstanding.
- [ ] **ACP guardrail gap (live-verified 2026-09-18):** the Codex ACP adapter
      executed shell/`edit` tool calls with **zero** `session/request_permission`
      and never called client `fs/write_text_file`, even in `read-only` mode, and
      wrote the target file in every mode. Do not present external ACP agents as
      satisfying teaching-task read-only enforcement, scope allowlists or edit
      attribution.
- [ ] Finish the curated external-agent settings flow. Keep the user-facing
      catalog limited to Pi, Codex and Claude Code; the ACP registry supplies
      install metadata for those entries, not arbitrary ACP connections. Verify
      install/remove/launch, reconnect, and advertised model/provider plus
      select/boolean options in the running app. The picker already requires an
      explicitly installed supported agent and routes advertised options through
      ACP. Accepted changes are now saved per agent under
      `~/.ahead/agents/external-acp/` and applied to new or restored sessions
      only while the value remains advertised; an explicitly requested session
      model takes precedence. This follows Zed's `default_config_options` flow
      in `../zed/crates/agent_servers/src/acp.rs`. The socket-free
      `config_defaults_persist_per_supported_agent` regression passes. On
      2026-10-06, the existing `ahead-agent` test binary (newer than
      `acp_client.rs`) passed all 40 `acp_client::tests` with loopback
      permitted; a restricted run's five socket cases failed at bind with
      `EPERM` before their assertions. These use synthetic peers only and make
      no provider requests. Rechecked on 2026-10-08 against the current
      `ahead-agent` runner (00:21:43, newer than `acp_client.rs`): all 40 tests
      passed serially with ephemeral loopback peers. Coverage includes advertised
      model/provider and boolean options, saved defaults, human permission
      decisions, streaming updates, cancellation, shutdown and the editor MCP
      bridge; no external agent or provider was contacted. Synthetic ACP
      fixtures omit the unused editor
      MCP bridge; they cover model selection, saved defaults, typed boolean
      options and option updates but do not verify the production bridge. A new
      GPUI interaction regression opens the model menu, selects an advertised
      provider/model, toggles a boolean option, and verifies both exact update
      requests and acknowledgement recovery. It still uses the fake proxy, not
      an installed Pi process or rendered desktop app. `ThreadsPanel` now
      fetches the supported-agent catalog
      and persists install/remove state off the GPUI foreground thread; catalog
      failures show an in-place Retry action and stale replies are ignored. A
      GPUI regression covers the catalog error/retry state. Verify default
      application, model/provider changes, reconnect, loading/error states and
      responsiveness in the running app. ACP option defaults now publish
      through a unique atomic temporary file; run the new symlink regression
      in the shared Cargo loop and verify replacement behavior on Windows.
      On 2026-10-12, the offline fallback pins were refreshed from the live
      official ACP registry: Pi `0.0.34`, Codex ACP `2.2.2`, and Claude Agent
      `0.89.1`. A catalog regression asserts these fallback package specs.
      Registry versions can move independently. Later 2026-10-12 installed-
      package checks verified curated ACP initialization, and the Pi live-turn
      smoke verified one streamed response; rendered reconnect remains open.
      Pi's registry package is the `pi-acp` bridge only; upstream requires a
      separate Pi CLI v0.81+ on `PATH`. The live turn passed with the host Pi
      v0.87.1, which announced v1.2.0. The external-agent catalog and guide now
      explain that AHEAD installs the bridge, not the Pi CLI. A launch-time
      preflight runs `pi --version` for selected Pi sessions before adapter
      installation, and gives install/update guidance for missing, failed,
      unparsable or older-than-v0.81.0 CLIs. The focused locked/offline adapter
      filter passed 3/3, including missing, minimum, old-version and malformed
      version cases. On Windows it runs npm's command shim through `COMSPEC`.
      Rendered install/reconnect verification remains open because the Mac is
      locked and AHEAD aborts before rendering; see
      `docs/development/zed-tracking.md`. The catalog prerequisite and GPUI
      visibility checks also passed headlessly (2/2). The shared `just dev`
      loop's 5.17s integrated build passed, then GUI startup again aborted with
      the HIServices XPC signal 6. The focused test first exhausted disk space;
      removing only eight older Rust incremental-cache directories recovered
      3.7 GiB while preserving binaries, source and disposable data. Native
      ACP install/reconnect rendering remains unverified.
      Latest loop restart after adding Windows npm-shim handling is session
      85670; its integrated build passed in 3.15s and the GUI child again
      aborted before rendering with the same HIServices XPC signal 6. The
      watcher remains live for edits.
      The pre-launch notice and active-session banner now state that AHEAD does
      not mediate shell/file effects or guarantee teaching read-only, path-scope
      enforcement or CodeAnchor attribution; verify their rendered layout in
      the running app. On 2026-10-12, the 09:56 source-current app test runner
      passed all four `acp_` session-panel interaction tests (cached option
      updates, duplicate-click suppression, advertised auth action and model
      option selection/recovery). These are headless GPUI checks; CUA reports
      AHEAD Dev running but window attachment times out, so native rendering,
      install/reconnect and external-process behavior remain unverified.
- [ ] Verify external ACP permission review in the running GPUI app. Assist
      now queues an advertised `session/request_permission` choice in the
      existing question card instead of auto-selecting `allow_*`; Learn,
      unknown modes, turn cancellation and a switch to read-only answer
      `cancelled`; turn completion also closes any outstanding card. A fake
      ACP peer confirms exact selected/cancelled wire outcomes, no response
      before human review, and late-request/early-end races. `just test-all`
      passes on 2026-10-02. Add an explicit,
      user-controlled allowlist auto-approval policy only if needed; a client
      response never enforces effects an external process performs without
      asking. Rendered multi-request ordering, disconnect and retry still
      need verification.
- [ ] Verify ACP Registry binary installation and update status in the native
      app on Windows and macOS/Linux. AHEAD now prefers a checksummed binary for
      the current platform, falls back to the registry Npx package, requires
      HTTPS and SHA-256, and supports ZIP, gzip-tar, bzip2-tar and raw binaries
      with bounded, path-safe extraction. Registry and installed versions are
      surfaced in the curated Pi/Codex/Claude picker; updates install on next
      launch. The code follows Zed's `AgentRegistryStore` and
      `LocalRegistryArchiveAgent` selection/install lifecycle. Focused source
      tests were added but not run; verify actual download, launch, update and
      rendered status behavior before removing this item. Registry cache reads
      remain capped at 4 MiB, malformed refreshes preserve the last good cache,
      and cache publication uses a unique atomic temporary file. A source review
      against the live ACP registry found Windows targets that use a bare
      executable filename; AHEAD now accepts safe bare relative names without
      resolving them through PATH. The offline Claude fallback pin is refreshed
      to `0.94.0` from the registry current on 2026-10-20.
- [ ] Finish ACP adapter-cache lifecycle verification in
      `ahead-agent/src/{adapters,acp_client}.rs`. New generations carry
      shared file leases through resolved launch configs; on Unix the ACP
      child inherits the lease descriptor so a live agent keeps its generation
      locked even if AHEAD exits. Install-locked cleanup reclaims unlocked
      versioned generations across restarts and preserves old-format entries
      conservatively. Other platforms retain process-local cleanup and keep
      generations from prior app processes. Cleanup runs when an adapter is
      resolved, not as a startup sweep. Do not sweep existing user caches as
      part of source cleanup. Unix regressions for inherited live-child leases
      and reclaiming an unleased generation from another app instance were
      added, but were not run in this checkout. Safe cross-restart reclamation
      on non-Unix platforms remains open.
      Verify real npm install/update failures and concurrent launches through
      the installed-agent UI; current failure/concurrency tests use disposable
      fake packages. The offline real-npm two-generation Node smoke test passed
      again on 2026-10-02 without registry/provider access or user npm config.
      Missing current-generation/package/executable/lockfile entries now
      reinstall through the existing locked staging/publication path. A failed
      repair retains the manifest and earlier leased generations; malformed
      manifests, path escapes and other I/O errors remain fail-closed. The
      focused adapter tests pass on macOS. Windows npm installation now selects
      `npm.cmd`; a Windows-only shim test uses a disposable PATH containing
      spaces and an ampersand, and the `npx_` filter is wired into macOS/Windows
      CI. Run that CI check before claiming Windows installation works.
      On 2026-10-15 all 169 non-opt-in agent library tests passed (four ignored).
      The extended real offline npm smoke passed too: deleting the current
      executable triggers repair, and both repaired and earlier leased package
      versions still execute with Node. The separate strict public-registry
      probe now verifies the production refresh and package/session/model
      path for all three curated adapters. Rendered install/reconnect and
      provider-backed ACP gates remain open.
## Editor

- [ ] Verify the refreshed `icons/ahead_logo.svg` and Green Serenity themes in
      a freshly rebuilt app: title bar, status bar, thread badges, editor and
      terminal colors, light/dark switching, and the macOS Dock icon set in
      `ahead-app/src/app.rs`. The app check, both `theme::tests`, SVG geometry,
      PNG/ICNS parity, and all ICO sizes pass. Fresh build/live inspection is
      still pending after shared Cargo build-lock contention.
- [ ] Finish FIM context budgeting, invalidation and native acceptance in
      `ahead-proxy/src/ahead/prediction.rs`, `ahead-proxy/src/ahead/host.rs`,
      `ahead-proxy/src/dispatch.rs` and `ahead-app/src/code_panel.rs` per
      `docs/development/ahead-editor-mvp.md` §8.3. Measure full context-build and
      acceptance costs for large files and sessions, including cold-cache Turso
      reads. Durable work context now has an eight-session, revision-keyed cache;
      session/instruction/related-buffer inputs are capped at 64 KiB and final
      prompts at 128 KiB. Oversized required context returns an error instead of
      dropping constraints. The cold Turso reader checks row lengths before
      decoding titles, decisions, summaries and plan JSON, so irrelevant runtime
      fields do not consume the plan budget. On 2026-10-15, the final-source
      combined app/proxy/agent/RPC prediction filter passed all 50 matching tests
      (the agent crate had no matching tests); source-current formatting and
      `git diff --check` also pass. A targeted provider-selection regression
      also passes 1/1: without `ai.active_connection`, FIM chooses the first
      configured provider instead of lexicographically sorting provider IDs.
      The host FIM context fixture now opens the canonical `.ahead/session.db`
      path instead of creating `.ahead/sessions.db`; source search confirms no
      remaining plural-path reference. Its test remains unrun in this source
      state because the runner is absent and another Cargo workspace currently
      owns the build process.
      An integrated `just dev` build completed in 2m30s and the runtime reported
      the proxy entering its main loop. A later restart found the `target/`
      directory absent after a missing `.rmeta` error; the lock-guarded clean
      rebuild then passed in 11m17s, and the watcher is active again on the
      disposable project. These process/build checks do not verify a rendered
      window or feature interaction.
      Managed FIM now follows the selected session provider/model. When the
      endpoint or connection config supplies a context window, it budgets the
      code excerpt around the caret after reserving completion/safety space and
      the complete session
      header; it fails rather than truncate required context. When the durable
      context byte cap or selected-model budget requires it, session context
      compacts to all work-item titles/statuses,
      tagged decisions/invariants/constraints, latest events, eligible summaries,
      task objective, plan progress, opening entry and all active steps (or next
      pending when none is active). This still uses a byte/3 estimate, not the
      provider tokenizer. Oversized recent work-item notes now trigger the same
      compact-cache fallback; `oversized_recent_work_history_uses_compact_context_and_keeps_invariants`
      covers the 64 KiB overflow while retaining the invariant and memoizing
      the compact result. This regression has not been run. Related-buffer
      selection remains bounded to four open buffers at 4 KiB each. After the
      language server returns no inline completion, the proxy scans a bounded
      three-line radius for nearby identifiers and issues up to eight concurrent
      LSP definition requests with one 100 ms total wait on the prediction
      worker. Only eligible open workspace buffers contribute; large buffers
      are excerpted around the nearest definition line. Successful results are
      cached for three seconds by active-buffer revision and identifier
      position; entries require the referenced open-buffer identity/revision
      to remain unchanged. Source-only regressions cover nearest-symbol
      ordering, deduplication, UTF-16 positions and stale target-buffer
      invalidation; they have not been run.
      Import/module file-stem matches, exact identifier references and sibling
      files remain the fallback ranking, with substring collisions such as
      `log` inside `catalog` excluded. New regressions cover LSP response shapes,
      definition-centered excerpts, import priority and substring collisions;
      they have not been run. This moves toward Zed's
      `../zed/crates/edit_prediction_context/src/edit_prediction_context.rs`,
      which uses Tree-sitter identifier captures and containing outline
      headers. AHEAD now prefers
      current-revision LSP semantic-token captures
      from existing editor-highlighting requests, rejecting stale buffer
      identity/revision/length and excluding comment/string token types; a
      bounded lexical scan remains the fallback when usable semantic tokens are
      unavailable. Focused regressions cover filtering and stale-token rejection;
      they have not been run. Installed Zed grammar captures from the existing
      `GetExtensionSyntax` path now fill in nearby declaration identifiers when
      current semantic tokens are unavailable. Capture caching is fenced by
      buffer ID/revision/length, bounded to known identifier scopes, and falls
      back to lexical scanning when no grammar candidates are available. Focused
      capture-selection and stale-cache regressions were added but not run.
      Preserve the active session's
      behavioral constraints during compaction; byte guards are not
      tokenizer-based context limits.
      Ancestor instruction watches now refresh when a watched `.git` marker
      changes while a nested workspace is open, using the same root discovery
      policy as native instruction loading. A regression covers creating and
      removing a nested root and delivery from the newly active ancestor; it
      has not been run. Instruction/provider changes invalidate predictions
      immediately, while Git/search/LSP refreshes remain debounced. If
      root-marker settings are exposed, share one policy across native
      instructions, skills and FIM.
      Windows reparse-point runtime
      validation still requires its CI job.
      Version-1 snapshot bindings now reach RPC/display/acceptance. Before
      insertion, the host validates the live target, remaining suggestion,
      consumed-buffer versions and fresh session/provider/instruction state,
      including archived sessions, without waiting for filesystem notifications.
      The editor rechecks its local context after that asynchronous reply. This
      is not an atomic host/editor transaction. Accepted session-bound FIM now
      records the human actor, target range, model/provider, prediction binding
      and accepted-text hash in the Turso session event log without copying
      source text. The store exposes a session-scoped read path; GPUI and Turso
      regressions cover recording and retrieval but have not run against this
      source. Verify native caret/replacement placement, undo, error
      presentation and large-file typing latency, then authenticated-provider
      acceptance. Headless GPUI and local-provider checks do not close those
      gates; evidence is in `docs/development/zed-tracking.md` under the FIM
      sections dated 2026-10-15.
- [ ] Finish live agent-panel acceptance for dynamic ACP commands, session
      restore during an active turn and running-app keyboard behavior. The
      empty-state New AHEAD and External agent actions now reveal the Threads
      column before showing their setup wizard, even when that column was
      hidden; the Shell-level GPUI regression covers both paths but has not run.
      250 ms UI timer now queues coalesced asynchronous durable/runtime reads
      only during turn startup or streaming, without blocking the UI;
      while idle it mirrors cached ACP config-option updates. A headless GPUI
      regression seeds an ACP config-option notification and drives the idle
      poll; `cargo test --locked --offline -p ahead-app
      idle_panel_mirrors_cached_acp_config_option_updates -j 2` passes. On
      2026-10-07, the source-current app test binary (05:07:54; newer than
      `session_panel.rs` at 05:07:23) passed that test plus
      `pending_acp_option_change_ignores_another_click` and
      `rendered_acp_model_option_sends_selected_value_and_recovers` (3 passed,
      no Cargo build). These are headless GPUI tests; live ACP and provider
      behavior remain unverified. Verify the timer-driven update and active-turn
      path in the editor,
      including session restore during an active turn. A rendered composer
      regression now covers plain Enter without a stray newline and Shift+Enter
      as a newline; it passes with four adjacent slash-palette tests. Added
      strict GPUI interaction coverage keeps the composer responsive behind
      delayed history/cancellation replies, preserves streamed text and cleared
      plans against stale snapshots, verifies gpui-kit's scroll-up/tail-follow
      behavior, queues Escape cancellation during startup, rejects malformed
      cancel replies and ignores late start/retry/cancel/refresh replies after
      leaving and revisiting a session. These headless checks do not replace
      authenticated-provider or native running-app acceptance.
- [ ] Managed AHEAD runtime now applies file changes through the native loop and
      records anchors after completion; verify the full apply → gutter → commit
      journey against a real authenticated turn. External ACP remains outside
      that guarantee.

## Session history and project configuration

- [ ] `SessionExportBundle` now carries readable conversation messages, restores
      them into a fresh store, and the session panel exports a shared checkpoint
      under `.ahead/sessions/<session-id>/` with `session.json`, `session.md` and
      `conversation.jsonl`; the Threads rail discovers it, imports it without
      launching the original harness, and attaches the restored conversation to
      the existing Session panel without changing the editor document. Extend
      that checkpoint with artifacts, provenance and versioned code references;
      add versioned context attachments through the existing host/viewmodel and
      `ahead-app/src/session_panel.rs`. Verify a teammate can attach a specific
      checkpoint revision to new work and feed relevant history to FIM. See
      `docs/development/ahead-workflows.md` §9 and
      `docs/development/ahead-editor-mvp.md` §7.
- [ ] Workspace `.ahead/settings.toml` now stores user-defined AI connections,
      the active internal model and credentials and remains ignored. Make tracked
      `.ahead/config.toml` the canonical writer for shareable non-secret project,
      workflow and artifact-publication settings; retain ignored
      `.ahead/config.local.toml` for checkout overrides and private
      `~/.ahead/settings.toml` for user-level defaults. The managed provider
      loader now reads that user file before workspace settings, shared config
      and local overrides; a focused test covers duplicate-provider and active
      connection precedence. The shared config reader now rejects nested
      credential-named fields in tracked `.ahead/config.toml`, while ignored
      settings layers permit API keys. The managed runtime now uses that same
      bounded reader (no-follow on Unix) for all provider layers and propagates
      invalid or unreadable layers instead of silently skipping them; all 35
      core tests and the managed-layer and precedence regressions pass offline.
      Both managed-agent and FIM provider merges now inherit a private API key
      only when a later layer keeps the exact same base URL; focused endpoint-
      switch regressions pass in `ahead-agent` and `ahead-proxy`. Surface
      inherited profiles/effective source in the Settings panel and verify
      these boundaries in a running workspace. Tracked config now rejects URL
      userinfo and credential-named query parameters as well as common secret
      fields; arbitrary value/path disguises remain a documentation and review
      concern. Those three provider readers now use
      `ahead_core::config::read_ahead_config`, which rejects symlinked and
      oversized files; focused core, app and proxy regressions pass. On Unix,
      that reader now uses the shared no-follow directory/file opener and
      limits bytes from the opened descriptor, closing the prior
      check-then-reopen race; its disposable-project regression passes
      (2026-09-29). The non-Unix path still checks metadata before a path-based
      open, so equivalent race protection remains open there.
      Managed provider settings now enter `ConfigBuilder` as in-memory session
      overrides, and its runtime-home user config layer is ignored. No provider
      credential config file is generated; a focused regression loads the
      AHEAD model, endpoint and key despite an invalid stale runtime-home file,
      and a disposable mock-provider turn asserts the selected key in the
      actual model request's bearer header. The 131-test AHEAD agent library
      suite passed with local loopback before that added header assertion;
      the focused turn passes afterward. A live provider is still unverified.
      AHEAD's managed host now skips retained managed config file/MDM layers,
      system and managed requirements, and user/project execution-policy rules;
      malformed managed-source regressions pass and the retained config suite
      passes 247 tests. AHEAD's own read-only startup, per-turn permission
      profiles, scope checks and human MCP policy remain in force. Verify this
      boundary with a real managed turn in the running app.
      The packaged runtime defaults now contain only instruction-context
      controls; unused Codex `history.jsonl` persistence and URI file-opener
      fields/types are removed, and the package no longer injects Codex
      credential-store or ChatGPT endpoint values. A focused managed-config
      regression proves those keys are absent from its effective layer while
      AHEAD provider settings still load. The unused CLI credential-store and
      ChatGPT endpoint config fields, types, and requirements projections are
      now removed; retired keys fail closed in strict config and requirements
      parsing. AHEAD model-provider auth remains in the AHEAD-owned auth crate;
      the unused MCP remote-auth path has since been removed.
      The unused Codex product-feedback setting and managed-requirement path
      are also removed; this does not change model-facing tool feedback. The
      copied CLI startup-update and TUI paste-burst switches are removed too;
      the editor owns any future versions of those behaviors.
      Copied auth-environment telemetry collection and exported `auth.env_*`
      fields are removed; six OTel routing regressions now assert those fields
      stay absent while auth-error/header reporting remains. The retained
      provider trait no longer exposes the unused app-visible account-state
      projection or its ChatGPT/Bedrock presentation types. The unused Agent
      Identity and personal-access-token auth variants, records, and routing
      matches and Agent Identity request-telemetry DTO/attributes are removed.
      The ChatGPT-only remote-model
      replacement policy is removed; provider catalogs retain bundled models
      and merge refreshed entries by slug. ChatGPT-only CyberAccessProgram
      request/context metadata is removed from the protocol and Responses API;
      dead auth-mode aliases/predicates and unused ChatGPT user/plan accessors
      and fields are removed too. A 2026-10-04 source audit found no ChatGPT
      OAuth/account branch in
      `ahead-model-auth/src/ahead_auth.rs`; it retains AHEAD's API-key,
      arbitrary-header, Bedrock and external-bearer auth. The 401 retry
      in `core/src/client.rs` is provider-generic and remains necessary for
      configured provider token refresh; MCP no longer has a remote OAuth path.
      The image-generation
      ChatGPT-tier gate and related
      `core/tools/spec_plan.rs` branch have already been removed.
      Image-generation eligibility no longer checks a ChatGPT subscription tier;
      the existing feature, provider-capability and model-modality gates remain.
      Provider construction no longer switches to ChatGPT's Codex endpoint based
      on auth mode or exposes the unused Codex-route detector; explicit AHEAD
      provider URLs and the public OpenAI default remain authoritative. The
      unreferenced copied `Product` enum (`Chatgpt`/`Codex`/`Atlas`) and its
      app-platform mapper were removed after a source search found no consumers.
      On Unix, default runtime-home creation still walks no-follow directory
      handles and rejects symlinked `.ahead`/`runtime` parents. The retained
      runtime later addresses that home by pathname for other state, so audit
      parent replacement before claiming full workspace-state race safety.
      Explicit user-selected `AHEAD_HOME` and the non-Unix fallback remain
      path-based. Invalid
      provider-layer errors are now visible in the model picker and Settings
      status, while FIM returns a provider error and the proxy sends a
      deduplicated `ShowMessage` into the shell status bar. A disposable viewable
      macOS bundle in `/private/tmp` passed the closed-Settings warning and
      recovery journey on 2026-09-29. No real project data or provider was used;
      repeat in the normal `just dev` loop when a viewable dev build is available.
      Settings now checks workspace-layer plus user-global settings timestamps
      in its background loader and invokes the chat model reload callback. The
      proxy watcher notifies Settings and the picker on workspace changes even
      while Settings is closed, without reindexing search. User-global
      `~/.ahead/settings.toml` provider profiles now layer below workspace
      overrides, and the proxy forwards user-settings file paths so Settings
      and the chat picker force a refresh even when file timestamps collide.
      Focused source regressions cover provider precedence and user-layer model
      visibility; run them in the disposable project. A 2026-09-29
      disposable-project GPUI pass verified
      separate title/status/warning lines and non-overlapping two-line skill
      rows. `/smoke` found a newly added project skill after reopening the
      palette; Down changed selection and clicking the skill inserted
      `/smoke-skill` without sending a turn. Still verify active ACP commands,
      narrow panel widths and authenticated skill invocation.
      Keep databases, sidecars and personal overrides ignored.
- [ ] Implement the working-document convention in
      `docs/development/ahead-workflows.md` §9 using the existing session host,
      store and editor save path: topic-named Markdown under the configured
      `[documentation].root` (default `docs/`), grouped by research, design,
      plans, verification and reviews rather than session ID. The host now passes
      the root and kind convention to agent/FIM context, but automatic document
      creation, canonical document links, revision/hash indexing, external-edit
      conflict handling, and Settings UI for this configuration remain open.
      Keep `.ahead/sessions/<id>/` as an explicit session checkpoint, separate
      from lasting documentation; retain libSQL only for expiring runtime history.

## Guided investigation, debugger and voice

- [ ] Finish task-local intent lifecycle: sessions now create durable
      `teaching` or `assistance` tasks from the starting request, persist the
      initial teaching mission and expose the task intent to policy/prediction;
      still link a later teaching request to its parent assistance task and
      add the full verified-source/concept/learner-response/review record flow.
- [ ] Implement durable teaching arcs for explicit teaching tasks: mission,
      verified source references, concept state, learner responses, evidence and
      review queue. Render one native conversation card at a time, preserve the
      human caret, keep teaching read-only and export selected records through
      the `.ahead` Markdown artifact convention.
- [ ] Integrate the graduated `diagnosing-bugs` loop into investigation and
      corrective-debugging assistance tasks: red reproduction gate, minimization,
      ranked falsifiable hypotheses, selected experiment, regression evidence,
      cleanup and diagnostic-evidence redaction. Stop honestly when no tight
      loop can be built; do not let a linked teaching arc turn an untested
      hypothesis into a fact.
- [ ] Validate the adapter-neutral Presentation Core contract in real native
      GPUI sessions for each supported managed model/provider route and through
      every supported ACP adapter: verify `present_code` opens the right tab
      without taking keyboard focus, `read_editor_buffer` returns unsaved
      open-buffer text, quote changes clear stale cues, and
      `move_code_pointer` moves its arrow independently of the highlighted
      range without changing the human caret. Confirm both editor actions
      acknowledge after the visible frame,
      and reject queued actions from a cancelled or superseded turn in the same
      session.
      A headless GPUI test now verifies that inserting text before the quoted
      code keeps its inline label/note attached to the same quote. Focused
      presentation tests now verify that the gutter pointer follows
      `pointer_target` independently of the quoted line while preserving the
      human caret. The floating arrow now uses `EditorState::range_to_bounds`
      with overlay-relative coordinates; verify tabs, wrapping, font scaling
      and horizontal scrolling in a running window.
      Switching threads stops speech and microphone capture, flushes pending
      transcript text to its original session, clears the prior session's cue
      and rejects queued editor actions from the inactive session; verify this
      in the rendered app.
      Verify ACP stdio MCP startup, discovery and cancel behavior
      across adapters. Local ACP bridge tests cover modern MCP discovery/tool
      listings, presentation dispatch, editor acknowledgements and cancellation
      settlement, including a serialized stdio loop regression. They do not
      verify end-to-end ACP stdio startup/discovery for real Pi, Codex or Claude
      agents, or a rendered frame. Durable inline discussion is tracked under
      Multiplayer collaboration. Compare the full journey with
      `docs/development/ahead-workflows.md` §6.
- [ ] Extend `ahead-app/src/{proxy_client,debug_bar}.rs` and the existing
      `ahead-proxy/src/plugin/dap.rs` route for guided breakpoint setup: retain
      source/debug-session identity, ownership, actual binding/verified status
      and paused thread/frame identity. Preserve the complete per-source set
      when adding an agent breakpoint; replace hardcoded active-line targeting.
      Expose the tested capability to the selected harness and verify the real
      experiment loop in `docs/development/ahead-workflows.md` §5 and §10.

### Session voice calls

Implement [the session voice call plan](docs/plans/session-voice-calls.md).
Source now contains an initial OpenAI GPT-Live transport, self-hosted
vLLM-Omni Realtime duplex adapters, native Mac capture/playback, session
composer call controls, captions, editor teaching hooks and voice settings.
GPT-Live remains the cloud default; choosing self-hosted now defaults to the
explicitly experimental Realtime-Venus profile, not the Nemotron profile whose
stock serving path lacks managed tool-call integration.
Self-hosted profiles now cover 16 kHz PCM16 in 200 ms chunks and Nemotron
VoiceChat's 16 kHz float32 input in 80 ms frames; reported PCM16 output rates
are converted to the native 24 kHz rate. Locally detected speech turns flush a
padded final frame and send an input commit; source tests cover this framing,
but a live model must verify that commits trigger expected replies. The Add
transcript path is removed.
vLLM-Omni playback acknowledgments now follow native DataPlayedBack callbacks,
carry cumulative per-response playback, and discard stale completion events
after an interruption; unit tests cover the tracker, but live server receipts
remain unverified.
GPT-Live barge-in also sends a trusted interruption instruction and gates local
playback until a fresh assistant transcript starts beyond the caller's latest
transcript timestamp. Source tests cover the gate; the primary WebSocket audio
has no timing or item IDs, so transcript/audio ordering still needs live testing.
The live frontend also receives a bounded excerpt from the active unsaved editor
buffer with its caret or selection, and debounced updates as editor context
changes; private-file contents are withheld. Provider-wire and privacy tests
cover this source path. For teaching and specific-section questions, the voice
agent prompt now requires an exact verified range highlight followed by a
separate pointer, preserving the human caret; a regression test covers this
instruction contract. Dictation-only Apple speech recognition is removed; AHEAD
requests microphone access only, and the unused `Speech` framework link and app
build script are gone.
Finalized human and spoken-agent transcript segments now enter the same durable
conversation via an owner-checked RPC; AHEAD groups fragments per speaker and
flushes them after a short pause, call end, or session switch. Raw audio is not
stored. The stored rows preserve speaker role, session order, and creation time,
and each segment uses a stable UUID so retried RPCs return the original row.
Provider-relative start/end offsets and call identity now travel with each
message and persist atomically in the session event log; retries that reuse a
segment ID with different timing are rejected.
Voice startup now also acquires an owner-checked lease from the managed session
host. The host assigns the call ID and connection epoch, permits one active call
per session, rejects controls from stale leases, and releases the lease on
hangup, session switch, and provider setup failure. Provider audio still flows
between the native app and the selected voice server.
Managed voice requests waiting behind an active coding turn are capped at eight;
the frontend gives the caller an audible queue-full response instead of
allowing spoken delegations to grow without bound. Delegation-ID deduplication
also retains only the most recent 256 IDs per call.
Neither provider has passed a live provider/device walkthrough, and no
self-hosted model is qualified. The Nemotron profile is source-tested, but the
current vLLM-Omni docs place its function-call integration behind a follow-up
duplex-plugin port; verify a deployment containing that port alongside its
published GPU recipe and live delegation/interruption behavior. Realtime-Venus
now has a separate experimental HTTP session adapter; Qwen3-Omni's current
route does not establish model tool calls. Existing source adapters and
shared-session voice callbacks/DTOs do not prove a live provider workflow.
The Realtime-Venus loopback test checks its session, audio, output, prefill,
playback-ack and cleanup requests, including bearer auth and muted silence;
server/model interoperability and GPU behavior remain open.
OpenAI mute/unmute now waits for a matching provider
acknowledgment while stopping local mic samples immediately; real-device mute
behavior and native audio-resource cleanup still need verification.

Focused source verification on 2026-10-07 passed: `cargo fmt` for `ahead-voice`
and `ahead-app`, 34 `ahead-voice` unit tests, 10 voice-focused `ahead-app` GPUI
tests plus 1 settings-default test, 2 proxy call-lease tests,
1 `ahead-proxy` transcript persistence test,
1 proxy-client transcript RPC test, 2 managed editor-presentation tests, 3
managed editor-tool tests, and all-target `ahead-voice` Clippy with warnings
denied. `cargo check --offline -p ahead-rpc -p ahead-proxy -p ahead-app -j 1`
also passed with five unrelated warnings. Clippy for `ahead-proxy` remains
blocked by existing lints in the changed working tree's host/share/dispatch
code, `ahead-proxy/tests/harness_session_e2e.rs`, and retained runtime crates.
These source checks do not replace the live provider/device or GPU benchmark
gates below.

- [ ] Certify `gpt-live-1` client delegation in `ahead-voice`: test continuous
      native capture/playback, echo cancellation, overlap/barge-in and graceful
      close on real devices. Validate transcript-window correlation from
      timestamp-only delegation events and output recovery with untimed primary
      WebSocket audio. Verify the tested transcript-gated interruption in live
      conversations. Report conversation quality, task success and latency separately.
- [ ] Validate the experimental Realtime-Venus adapter in `ahead-voice` against
      a pinned GPU server: verify HTTP audio/output framing, model/tokenizer
      compatibility, `<delegate>…</delegate>` forwarding to the managed AHEAD
      agent and `/prefill` result delivery. Verify HTTPS plus bearer-token
      enforcement through the required authenticated TLS proxy, one-session
      admission, interruption, cleanup and stale-session rejection before
      exposing it as a supported provider.
- [ ] Benchmark Realtime-Venus, Qwen3-Omni and Nemotron VoiceChat on pinned GPU
      deployments against the same AHEAD task set; verify duplex behavior,
      managed-agent delegation, correction quality, task success, interruption
      latency and serving cost. Select and qualify a self-hosted default only
      from those results; published scores and source tests are not sufficient.
- [ ] Complete the session-owned voice state in
      `ahead-viewmodel/src/voice.rs`, `ahead-rpc/src/ahead.rs` and
      `ahead-proxy/src/ahead/{host,voice}.rs`: add live status, independent
      input/output identities, bounded stream queues and stale-event handling.
      The host now issues an owner-checked call lease with a call ID and
      connection epoch, allows one active call per session, rejects stale
      controls and releases the lease on hangup, session switch and provider
      setup failure. Local mute now keeps UI state tied to provider
      acknowledgment, with immediate local sample discard on mute;
      native audio-resource cleanup remains in the app lifecycle.
- [ ] Complete the session-owned voice/call boundary across
      `ahead-viewmodel/src/voice.rs`, `ahead-rpc/src/ahead.rs`,
      `ahead-proxy/src/ahead/{host,voice}.rs` and `ahead-agent/src/session.rs`.
      Finalized transcript rows now persist in the durable conversation with
      retry-safe segment IDs and provider-relative offsets. The lease is
      session-owned while audio remains native-client to provider; connect live
      status and reconnect recovery to this boundary, and verify that hangup
      remains independent from managed-task cancellation after reconnect.
- [ ] Complete remaining lifecycle coverage for the call controls in
      `ahead-app/src/session_panel.rs`, settings in `settings_panel.rs`, and
      audio/presentation ownership in `app.rs`/`code_panel.rs`. Session switches
      already close the call and invalidate stale events; native observers now
      end the call on system sleep or audio-engine configuration changes. Verify
      those cases on real devices. `SessionPanel` destruction now closes native
      provider transport and releases its host lease; verify app/workspace
      shutdown and reconnect/error recovery while keeping the text composer
      usable.
- [ ] Verify the plan's acceptance matrix in the rendered macOS app: speakers
      and headphones, live code pointing, background work, corrections, typing,
      permissions, device changes, reconnect, sleep and session switching.
      Measure interruption/typing latency and provider usage; capture screenshots
      only after the verified experience is ready.
- [ ] Benchmark Realtime-Venus-Audio 9B against open-weight
      Qwen3-Omni-30B-A3B-Instruct and Nemotron VoiceChat for the self-hosted
      server route. Verify true full duplex, model-to-managed-agent delegation,
      corrections, interruption latency and serving cost with the same AHEAD
      backend; implement a model-specific adapter where required. Pin
      weights/runtime, then deploy the qualified candidate with endpoint
      authentication, session isolation, capacity limits and cleanup. No
      Mac-local inference or silent cloud fallback.
- [ ] Validate external ACP voice separately before offering calls in its threads.

## Final milestone: user-facing feature documentation with screenshots

- [ ] Last, after all feature work and native verification:
      publish and keep current an in-app
      Help/Guide and `docs/guide/` covering every shipped user-facing feature,
      setting, shortcut and workflow. Capture and review screenshots from the
      running AHEAD app in disposable projects for every shipped feature; include the
      editor, search, Git, terminal/tasks, language support, debugger, managed
      and external agents, MCP/skills/memory, voice, collaboration and settings
      wherever shipped. Cross-check coverage against
      `tests/fixtures/editor-smoke/README.md`
      and this TODO before calling the guide complete. Explain prerequisites and
      mark any partial or unavailable capability honestly; do not depict mocks
      as working UI. Keep the guide with the app and use
      `.agents/skills/humanizer/SKILL.md` when drafting its user-facing copy.
      The first bundled Help tab and eight text topics now cover the current
      shell, editor, search, Git, terminal/tasks, language support, debugger,
      agents, MCP/skills/memory, voice, sharing, settings and shortcuts. A
      disposable native run rendered the Start here topic. Capture and review
      feature screenshots, verify the other topics against live workflows, and
      keep this guide current as the remaining features become usable. This
      screenshot-backed coverage sweep is the final goal milestone, not a
      substitute for completing or verifying the features themselves.
