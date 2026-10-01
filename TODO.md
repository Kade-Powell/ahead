# TODO

Incomplete work for AHEAD: unfinished features, stubs, mocks, and known gaps.
This is the single source of truth. When you finish an item and commit it,
delete it here in the same commit. Do not add a "done" section.

Anything knowingly shipped as a stub or mock must have an entry here in the
commit that introduces it. This workflow lasts until the editor is usable end
to end; after that we dogfood AHEAD to develop AHEAD and the backlog moves into
the app itself.

## Zed source reuse and editor parity

- [ ] Broad-use readiness goal (expanded by the user, 2026-09-30): finish and
      exercise every feasible shipped feature before claiming AHEAD is ready
      for daily work. Preserve the native-agent, Turso and hard-fork cleanup
      goal; no legacy migrations, aliases or backward-compatibility paths.
      Use `../zed` as the primary behavior/source reference and retain useful
      Codex model-loop behavior. Work through the detailed gaps below, fixing
      failures as they appear rather than stopping at an audit.
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
      Reuse one `just dev` build loop, app identity and disposable workspace
      across checks where practical; restart only to load changes or test
      lifecycle behavior. Do not bypass required approvals, alter real project
      data, add product features outside this scope, or publish/release.
- [ ] Restore a clean targeted Clippy gate for `ahead-agent/src/` before calling
      the build production-ready. On 2026-10-01,
      `cargo clippy --locked --offline -p ahead-app -p ahead-proxy --lib`
      stopped on 29 existing `ahead-agent` lint errors (including
      `runtime_support.rs` missing semicolon and `session.rs` lossy cast), so
      it did not certify the changed app/proxy code. The broader `-D warnings`
      attempt stopped even earlier on two retained upstream
      `codex-utils-path-uri` suggestions. Fix AHEAD-owned errors and decide a
      scoped policy for pinned upstream warnings; rerun the repo's documented
      Clippy gate without globally silencing new AHEAD diagnostics.
- [ ] Finish cross-platform storage privacy verification in
      `ahead-proxy/src/ahead/store.rs::SessionStore::open`. Unix creation and
      supported-store reopen now enforce mode 0600 for the database and
      sidecars; owner, regular-file, link and non-shared-write-directory checks
      reject unsafe paths. Focused tests cover journal/WAL/SHM modes, rejected
      old-schema bytes/modes and actual hot-journal recovery versus unchanged
      unsupported crashed files. Native startup changed the existing fake
      schema-2 database from 0644 to 0600; the real proxy recovery probe also
      passed new/reopened mode checks. Verify Linux behavior, Windows ACL
      enforcement and inherited ACLs.
      The Windows path currently checks only the main file kind and does not
      enforce an owner-only DACL. Test concurrent first-open from separate
      processes and retain errors as actionable UI state. Preserve all data;
      no migration, reset or silent in-memory replacement.
- [ ] Fix cross-language editor highlighting in `ahead-app/src/code_panel.rs`:
      language selection now follows the file type on creation and preview
      reuse, with a passing GPUI switch regression and a dynamic status footer.
      Enable the gpui-kit grammars needed for TypeScript/TSX, JavaScript/JSX
      and Python. Their three parser crates are not cached; the download was
      blocked by the offline-only constraint and awaits user approval. Rust
      and JSON grammars are available; language names alone do not prove
      parsing or colored syntax. Verify each grammar in the native editor.
      Installed Zed extension language detection currently reaches the LSP
      path only; bridge extension grammars/queries to the editor's language
      registry before claiming full extension-provided syntax support.
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
      forced reads with unchanged timestamps. Check loading presentation and
      managed/external transitions in the native app; no provider turn has
      been verified against these changes.
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
      observed events. The latest full agent suite passes 108 tests, with two
      opt-in tests ignored, but no cause has been established for this case.
      Do not mask it with retries or a longer timeout. The proxy attribution
      fixture's similar failure was traced on 2026-10-01 to macOS inheriting
      nonblocking mode on accepted sockets; that fixture now explicitly uses
      blocking reads. This shutdown fixture already has a blocking listener,
      so do not assume the same diagnosis applies. See the Zed tracking log
      for the captured stack, socket probe and validation results.
- [ ] Use `tests/fixtures/editor-smoke/` as the reusable disposable-project
      template. Copy it outside this repository, initialize Git in the copy,
      then open it in the running editor without precreating `.ahead/`.
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
      cleared it when the selection collapsed. A live provider-backed turn
      has not verified the final model payload, nor have the remaining matrix
      rows been exercised.
- [ ] Visually confirm `ahead-app/src/workspace_panels.rs` Source Control file
      rows at narrow and resized sidebar widths in the disposable project:
      long paths should show a leading ellipsis while Git status and the
      selection control remain visible. The 2026-10-01 native screenshot feed
      stayed on an older Explorer frame despite Source Control being present
      in the accessibility tree; the rendered row is not yet confirmed.
- [ ] Expand the initial GPUI command palette in
      `ahead-app/src/command_palette.rs` beyond the shipped shell commands to
      editor/buffer, Git and task actions with availability-aware dispatch.
      Cmd/Ctrl+Shift+P and F1 now open a searchable dialog backed by the same
      shell command handler as keyboard shortcuts; a focused headless test
      covers filtering and Enter. Zed registers available actions dynamically,
      but AHEAD's remaining controls are still direct callbacks. Register
      those actions instead of listing commands that cannot execute. Verify
      unavailable-action states and shortcut hints in the rebuilt native app.
      A disposable-project native pass confirmed Cmd+Shift+P before opening a
      file, F1 after Escape, `git` filtering and Enter opening Source Control;
      the compact-height layout and F1 from a focused PTY were rechecked in the
      rendered app. A scroll-handle regression and native pass confirm that
      keyboard selection remains visible past the first eight results.
      The legacy `defaults/keymaps-*.toml` files are not loaded by the GPUI
      shell.
- [ ] Pin a Zed upstream revision, bring the checkout into the development
      environment, preserve Apache-2.0 notices, and record imported source
      paths/tests in `docs/development/zed-tracking.md`.
      GPL position 2026-09-21: editor crates are GPL-3.0 (see decision 0006);
      imports proceed with a tracked clean-room obligation (next item).
- [ ] Clean-room rewrite (standing obligation per decision 0006): reimplement
      each imported GPL area from documented behavior, delete the vendored
      source, one area per commit with parity tests. Until done, preserve
      per-file copyright notices and ship license texts with builds.
- [ ] Port Zed's mature multi-file editor behavior: buffers, tabs, splits,
      selections, multi-cursor, undo/redo, IME, external-change handling and
      document-model synchronization. Validate real unsaved files before
      replacing the current single-file `CodePanel`. A disposable native-window
      pass exposed a false modified marker after Undo restored saved content;
      `CodePanel` now compares the current rope with its saved snapshot, and
      the focused GPUI Undo regression passes. The rebuilt isolated app showed
      the marker appear on typing and clear on Undo in the fake `main.rs`.
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
      Turso `editor_recoveries` (schema 2), coalesced snapshots, revision-fenced
      saved/discarded rows and OS owner leases. Restore keeps disk files and
      current dirty tabs unchanged, warns before overwriting a changed saved
      baseline and supports File > Recover Unsaved Changes. Run
      `tests/editor-recovery-smoke.mjs` after building, then the rendered
      startup/overwrite/duplicate-path recovery journey in a disposable
      project after desktop unlock. The last unacknowledged edits can be lost:
      periodic capture is 250 ms and GPUI's shutdown flush has a 200 ms budget.
      Test disk-full/disconnect behavior in the native UI, profile serialized
      DB writes with large/many buffers and define safe lease-file cleanup.
      Verify recovery path normalization and owner locking on Windows/Linux;
      current native process evidence is macOS-only.
      The 32 MiB per-buffer ceiling, untitled buffers, layout/caret/scroll
      restoration and general external-file-change/save conflict handling
      remain open. Define and verify exit behavior for running agent turns
      and terminal/debugger jobs. Old databases stay intact; no migration.
- [ ] Verify nonblocking attribution refresh in the native editor's gutter.
      `ProxyClient::refresh_anchors` no longer blocks file open or save on its
      RPC reply; the client regression covers delayed/out-of-order results,
      close invalidation, view wakeups and visible failure. Exercise the
      apply-to-anchor-to-gutter-to-commit journey. Git gutter/blame now use a
      debounced background proxy request against the live buffer and HEAD,
      including staged changes; render/caret movement read a cache, stale or
      closed-buffer replies are discarded, and repository notifications
      invalidate metadata even when the branch/file-status summary is equal.
      Verify this in rendered GPUI after unlocking the Mac, including external
      amend/checkout and a linked worktree or opened repository subdirectory.
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
      cover independent clearing and dynamic save support. Handle multiple/null
      save registrations and unregistration. Stopped-server diagnostics now clear
      by process instance; other servers' diagnostics remain, and late updates
      from retired processes are ignored. Verify this in the native Problems
      view after desktop unlock.
      An initial completion reply may be
      `null` until indexing finishes, so the editor can initially show no
      popup; improve that startup experience. Tab also inserted the suggestion,
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
      cancellation and late-response rejection. Verify delayed startup and
      timeout feedback in the native app; add a per-server timeout setting
      when the language settings owner/event path below is implemented.
      Test repeated restarts for inherited-pipe or
      descendant-process leaks, plus restart during edits and tab closure.
      Verify native Cmd+Q/window-close process cleanup after desktop unlock.
      Closing an authorized window now stops its proxy after recovery is
      confirmed. GPUI tests cover Cancel, failed/stale recovery and final
      buffer-close-before-shutdown ordering. The proxy now waits for catalog
      cleanup before exiting on shutdown or stdin EOF. The standalone
      `tests/lsp-shutdown-smoke.mjs` passes cooperative, delayed-initialization
      and unresponsive-server cases; it fails against the previous binary
      that exited before the LSP handshake. The installed-server smoke also
      verifies natural proxy exit and an empty test process group.
      Bound/cancel a catalog blocked in extension acquisition, verify repeated
      restarts with escaped descendants and cover Linux/Windows cleanup.
      Verify active native/ACP cleanup in the unlocked app with real providers.
      The proxy now stops the session controller; native parent/child shutdown,
      pending editor requests, ACP startup interruption and persisted partial
      cancellation have focused tests. `tests/agent-shutdown-smoke.mjs` passes
      real proxy exit and Turso reopen on explicit shutdown and stdin EOF; the
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
      rendered selection/resolve/Undo path with Vtsls. Snippets
      remain unadvertised until insertion is implemented; completion commands
      and large-span edit performance still need review. Standard initialization
      now supplies ready/error states for every server, with consistent names
      for experimental updates; retired-process status updates cannot overwrite
      the replacement server. Restart rendering and hung-server handling
      remain open as described above.
      `tests/lsp-smoke.mjs` now runs real installed Vtsls and BasedPyright
      against a fresh disposable copy. On 2026-09-30 it passed TypeScript,
      JavaScript and Python completion, cross-file definition, diagnostics and
      repair, plus TypeScript auto-import resolve. It reproduces the old
      Vtsls `douconst` corruption when shortening an unsaved buffer; explicit
      old-buffer ranges for incremental sync fix it. This is proxy/runtime
      evidence, not native UI proof. The Mac locked during the rendered pass;
      resume native verification after the user unlocks it.
- [ ] Finish Zed language-extension configuration lifecycle. AHEAD now
      advertises and answers `workspace/configuration` from the extension's
      startup snapshot. Zed's `ExtensionLspAdapter::workspace_configuration`
      ignores `requested_uri`, and the Zed 0.8.0 guest callback accepts a
      worktree but no scope URI; Zed's per-scope lookup in
      `../zed/crates/project/src/lsp_store.rs` is generic adapter behavior.
      AHEAD's extension-provided response should therefore remain identical
      across scopes (covered by a two-scope regression). The remaining gap is
      that AHEAD has no language-server settings-change event in the current
      app/proxy path. Define the settings owner and event source, then reevaluate
      the extension callback instead of retaining only its startup snapshot and
      send `workspace/didChangeConfiguration` on changes. Run
      `cargo test --locked --offline -p ahead-proxy --lib
      workspace_configuration_tests -j 2` when the shared Cargo loop is free,
      then validate a real extension guest receives changed settings without
      restarting its server.
- [ ] Finish language-server acquisition and discovery in
      `ahead-proxy/src/plugin/catalog.rs`, `ahead-extension-host/src/host.rs`
      and `ahead-app/src/settings_panel.rs`. Normal builds now include the
      extension host; built-in Rust Analyzer, Vtsls and BasedPyright use PATH
      and installed extensions can supply additional languages. Add managed
      binary/package acquisition with visible install failures and a practical
      extension picker instead of requiring a package URL. Follow Zed's
      `crates/languages/src/{rust,vtsls,python}.rs` and extension store; avoid
      a second LSP transport or per-project command overrides. Verify clean
      first-run setup, cached offline startup, upgrades, TypeScript/JavaScript
      sharing, custom-language mappings and a real installed extension in
      disposable projects. Do not mark the normal-build wiring as live proof.
- [ ] Trim the extension-host build to the used WASI Preview 2 interface in
      `Cargo.toml`. `wasmtime-wasi` currently enables unused Preview 1 and
      `wiggle` through its defaults; AHEAD registers only `p2`. Remove that
      unused feature after the current native verification pass, check the
      dependency graph, and rerun host/proxy tests. No Preview 1 compatibility
      layer is required.
- [ ] Port Zed's project search panel with search/replace, keyboard result
      navigation, match highlighting,
      include/exclude globs and paginated results. Clicked matches open at the
      start and select the full range, including cross-line matches; the Search
      tab remains beside code tabs so results can be revisited without rerunning
      the query. Verify range selection and tab navigation in rendered GPUI.
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
      The shared matcher now returns every match up to each caller's limit and
      searches eligible dirty open `CodePanel` tabs and proxy-owned buffers.
      The panel has case, whole-word and regex toggles plus comma-separated
      include/exclude path-pattern fields, and the
      native agent has an include-pattern argument; both use the shared
      worktree path filter. The retained runtime has no separate grep/content
      search handler: its `tool_search` is deferred-tool metadata discovery,
      while managed worktree search is the single AHEAD `file_search` backed
      by this service. The editor still needs a multi-buffer project
      model, untitled-buffer search, and a live GPUI tool-call smoke test for
      the new managed-agent buffer snapshot request/response path. Snapshot
      replies now cap at 128 buffers
      and 8 MiB and fail the tool explicitly when exceeded; verify the UI
      error, cancellation and timeout paths under real turns. Proxy
      `GlobalSearch` and the managed agent now reuse a lazy file snapshot
      invalidated by file-set and ignore-rule watcher events. The panel
      receives generation-tagged copies over RPC and refreshes on the
      `WorkspaceFileChange` notification, including external content changes
      without rebuilding the path snapshot; expose the same view to Explorer.
      Benchmark large worktrees and verify create/delete/ignore-rule changes
      and panel refresh on both file-set and content changes under live watcher
      delivery. The panel streams matching files through a bounded channel and
      falls back to a direct walk only without a proxy. A 2026-09-30
      native-window search in the disposable project found `AHEAD smoke test`
      once in `src/main.rs`; clicking the row opened line 2
      with the match selected. It also found `UNSAVED_SEARCH_NEEDLE` once in an
      unsaved `CodePanel` buffer and returned to zero results after Undo; the
      disk file stayed unchanged. This small fixture does not prove live watcher
      delivery or large-worktree quality.
      Concurrent cold searches now share one index build. After the first
      snapshot, ignored build-output events no longer
      invalidate the path list or restart search. Before that first walk,
      raw watcher events can still churn generation; Git diff refresh also
      still runs for those events. Index invalidation now also drops the
      visible-directory snapshot, so newly created nested paths fail open
      until the replacement walk; run
      `cargo test -p ahead-core workspace_file_index_rebuilds_after_invalidation`.
      AHEAD Quick Open now shows up to 20 session-local recent indexed files
      for an empty query, promotes the active file among fuzzy results, and
      parses `path:line`, `path:line:column` and `path:start-end` before opening
      the requested location. `:start-end` selects the complete inclusive line
      range, ending at the following line's start; goto columns use character
      positions while content-search selections retain UTF-8 byte offsets.
      Ranking/suffix and permanent-location tests passed before these range and
      column-unit changes; rerun them. The CodePanel suite now also covers
      Unicode external columns and byte-range positions. The last 20 canonical,
      workspace-relative paths are now persisted in `.ahead/session.db`, with
      traversal and symlink escapes rejected; Quick Open merges restored recents
      behind current-session navigation. New Turso reopen, host path-boundary
      and merge regressions still need focused Cargo runs after the shared
      Cargo loop is free: `rtk proxy cargo test --locked --offline -p ahead-proxy
      --lib recent_workspace_files -j 1`. All 7 `ahead-app --lib quick_open`
      tests passed on 2026-09-30, including a GPUI Enter-key confirmation and
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
      complete `ahead-core --lib` suite passes (31 tests), the complete
      `ahead-agent --lib` suite passes (88 passed, one provider test ignored),
      and `ahead-proxy --lib` checks with the watcher revision update. The
      agent suite's loopback-only mock tests required the local-test permission;
      no real provider was contacted. Remaining: configurable private-file
      rules and broader binary detection beyond the shared prefix heuristic.
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
- [ ] Add an optional watch/restart action for parsed just recipes using
      watchexec's supervisor/process-group and clear-screen support; direct
      recipe runs should remain the default.
- [ ] Port a Git source-control/review view with changed files, diffs,
      stage/unstage, commit, branches and conflicts; preserve AHEAD anchors,
      attribution and session trailers.
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
      full-stderr modes. Exercise the native debugger in a disposable project.
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
      in the rebuilt native app. Add pending-control admission to prevent
      duplicate step/continue submissions before a response, cover restart
      during initialization and exercise rejected/timeout termination and the
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
      semantics. The default debug configs still point at `target/debug/ahead`
      even for arbitrary projects and `.ahead/run.toml` parsing is insufficient
      for selecting a custom adapter/target; replace with project-scoped debug
      configurations and a working picker before calling the debugger broadly
      usable. Do not infer high-fidelity debugger features from this handoff.
      The rebuilt-proxy standalone shutdown smoke passes all three disposable
      mock adapter modes. The separate native mock probe above exercises
      `runInTerminal`, but not a real debuggee.
- [ ] Extend the PTY terminal to multiple named tabs, resize/reflow and
      lifecycle-safe sessions. Allow selected terminal output to attach to the
      AHEAD agent panel as context.
      `ahead-app/src/terminal.rs` now retains the Alacritty IO handle, terminates
      its Unix shell/foreground groups and reaps on a background thread.
      Real PTY regressions cover HUP/TERM-resistant jobs, idempotent cleanup and
      final-output retention. macOS shutdown now flushes unread kernel queues
      after termination and before reaping; otherwise a killed shell can stay
      in exit waiting for the stopped reader. A deterministic unread-output
      regression fails without the flush and checks the shutdown receipt with
      it. Verify this change through native tab close/window close/quit after
      rebuilding. Also verify Linux/Windows, detached jobs, an IO thread
      that fails to return, and native quit within GPUI's short hook deadline.
- [ ] Polish the AHEAD agent panel with streamed Markdown/code rendering,
      follow-tail, cancellation, durable reopen/retry and terminal context.
- [ ] Complete the single-human-plus-agent full-duplex voice journey. Defer
      multi-human agent threads until the single-user flow is reliable.
- [ ] Keep all new editor chrome on GPUI/gpui-kit theme tokens and use familiar
      VS Code keybindings where they do not conflict with AHEAD actions.
- [ ] Defer remote/Kubernetes development, broad customization/profiles,
      arbitrary VS Code extension hosting and symbol outline until the core
      editor parity loop is working.

## Multiplayer collaboration

- [ ] Build a host-authoritative, cloudless multiplayer session for AHEAD chat,
      code and terminal surfaces. Read and adapt the pinned Zed collaboration
      patterns from `crates/collab/src/rpc.rs`,
      `crates/project/src/buffer_store.rs` and
      `crates/project/src/project.rs`, while keeping AHEAD's session host,
      policy, attribution and workflow events authoritative; do not import
      Zed's cloud server, database or LiveKit topology.
      Use a direct LAN/reachable-host transport with reconnect and stable
      authenticated participant identities. Share human/agent conversation,
      live code buffers, presence and revision-aware edits with explicit role
      policy. The host owns the worktree, agent effects and PTYs: participants
      may read terminal output but cannot send terminal input, resize, close or
      control host processes unless a later policy explicitly permits it.
      Add a checked-in `.ahead/team.toml` allowlist of GitHub names/display
      names and default roles, let the session owner select members per
      session, and keep credentials, private keys, local endpoints and runtime
      state out of the file. Resolve GitHub names to verified identities during
      invitation, and update `.gitignore` for the shared team manifest without
      exposing existing private `.ahead` state. Acceptance coverage must prove
      host/participant chat and code convergence, reconnect/revocation,
      participant terminal read-only enforcement, role changes, and a complete
      local/direct session without AHEAD cloud infrastructure; document the
      reachable-host/NAT limitation.

## Dependencies (Lapce-org removal; see decision 0006)

## Attribution & change tracking

- [ ] Replace `ahead-app/src/code_panel.rs`'s anchored Git hunk review card
      with Zed-style inline diff expansion in the editor flow. The current
      gpui-kit `EditorState` exposes text decorations but no inserted display
      rows/blocks, so the preview floats over code instead of shifting it.
      Port the relevant display-map/block approach from Zed
      `crates/editor/src/git.rs`, `crates/editor/src/element.rs` and
      `crates/buffer_diff/src/buffer_diff.rs`; preserve buffer text and caret,
      align deleted/added rows with folding and scrolling, and verify native
      click-to-expand/collapse on modified, added and deleted hunks.
- [ ] Managed AHEAD runtime file-change completion now records `ahead` anchors from
      applied diffs; a real native-loop acceptance test now verifies the
      native file write, `FileChange` event, actor attribution and quote. Still
      exercise the rendered gutter and proxy `GitCommit` cleanup together. The
      focused dispatcher commit regression passed on 2026-09-29 (1 test,
      128 filtered): it
      commits an agent-authored file with AHEAD attribution/session trailer and
      clears the committed anchor rows.
      External ACP remains explicitly unable to provide this guarantee.
- [ ] Verify the rendered AHEAD CodeAnchor gutter and real commit. The gutter's
      row projection now has a regression covering inclusive agent-authored
      ranges and exclusion of human anchors; it passed in a focused
      locked/offline run on 2026-09-28 (1 test, 60 filtered). `created_at_commit`
      is removed from the AHEAD contract and schema. Old anchor schemas are
      rejected without conversion or row deletion; the database-preservation
      regression passes.
      A rendered gutter and user-driven commit remain unverified.
- [ ] Verify the existing proxy `GitCommit` route against a real user-driven
      commit with live anchors; its git2 path is now unit-tested to set
      `author = ahead`, preserve the human committer, include the session
      trailer, and stage renamed destinations correctly. The dispatcher route
      now also has an end-to-end regression test for anchor attribution and
      cleanup; it passed in a focused locked/offline run on 2026-09-28. The
      rendered user-driven commit journey remains.

## Agent chat panel (`ahead-app/src/session_panel.rs`)

- [ ] Verify the settings-to-managed-provider path in a running GPUI app. Settings
      supports editable per-provider model catalogs and authenticated OpenAI-
      compatible `/models` discovery; stale discovery responses are discarded
      after switching connections, editing the endpoint/key, or starting a newer
      lookup. Focused tests cover catalog normalization, response parsing, and
      stale success/error rejection; `ignores_stale_model_discovery_results`
      passes with `rtk proxy cargo test --locked --offline -p ahead-app --lib
      ignores_stale_model_discovery_results -j 2`. Settings saves now stage
      private files and replace them atomically; a focused regression covers
      overwrite permissions and symlink rejection (2026-09-29). Exercise discovery,
      save/select, credential handling, and
      new/restored managed sessions end to end. External ACP remains
      adapter-default.
- [ ] Validate the end-to-end installed-agent path for the curated Pi, Codex and
      Claude Code ACP adapters against Zed's registry/install lifecycle. The
      catalog is intentionally not arbitrary ACP; installation is user-local
      and the supported npm package is resolved lazily on first launch, with a
      pinned fallback when the registry is unavailable. Verify package install
      errors/status, model and reasoning selection on new/restored sessions,
      live config-option updates, streaming and cancellation in a running GPUI
      app. Config-option completion now checks the active session and request
      generation so a delayed result cannot overwrite a newer selection's
      status; `config_option_results_ignore_older_requests_and_other_sessions`
      covers that stale-result predicate and passes with `rtk proxy cargo test
      --locked --offline -p ahead-app --lib
      config_option_results_ignore_older_requests_and_other_sessions -j 2`.
      Pi provider access remains unavailable as of 2026-09-28; do not attempt a
      live Pi turn until the user confirms access is restored. Offline probing
      confirmed the installed `pi` CLI can print help with `PI_OFFLINE=1` and
      `PI_CODING_AGENT_DIR` redirected to `/private/tmp`; `pi-acp` is not on
      PATH or in the npm cache, so no local ACP handshake was available.
      ACP-side shell, file and permission effects remain adapter-owned and
      cannot receive AHEAD `CodeAnchor` enforcement.
- [ ] Verify native managed runtime fidelity on multi-tool turns, compaction,
      unsaved-buffer reads, unmapped runtime events, and model-specific tool
      selection against the pinned runtime. The managed `/compact` composer
      action now submits the retained runtime's `Op::Compact` and consumes its
      streamed completion. The AHEAD-owned `native_agent_streams_a_responses_api_turn_in_process`
      regression passes against an ephemeral loopback mock and was re-run on
      2026-09-30 in the full agent suite (100 passed, authenticated Pi ignored).
      It also verifies that a selected project skill body reaches the model
      request. `managed_config_disables_unmanaged_mcp_and_includes_path_preflight`
      passes, confirming runtime-home MCP declarations stay disabled. These
      offline checks do not replace an authenticated running-GPUI turn; verify
      both there. Run that UI journey in a disposable project with
      `just dev /absolute/path/to/disposable-project`, never in a real user
      project. Directory-first CLI launch now selects that directory as the
      workspace without opening it as a file; the focused startup regression
      passes. On 2026-09-30 the current `just dev` binary opened a disposable
      project in a temporary viewable macOS bundle and restored an empty managed
      session after process termination. Authenticated multi-tool turns and
      rendered stream/cancel/retry/compaction remain open.
      Durable session title/objective, intent, phase, linked issue, work items,
      recent events and summaries now reach the native model prompt through a
      separate host-populated `session_context`, never by rewriting the user's
      message. New turns discard caller-supplied context; retry requests retain
      their persisted snapshot. Focused regressions verify the actual first
      native model request and exact retry DTO persistence, including a slash
      command remaining at the start of the prompt. Authenticated rendered
      behavior remains open.
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
- [ ] Make session creation and its harness binding one Turso transaction in
      `ahead-proxy/src/ahead/{host,store}.rs`. `StartWork` currently commits the
      session and updates active/voice state before `set_harness_preference`
      writes the backend binding. A binding failure can return an error after
      a session was already created. Validate the selected external adapter
      before writing, roll back the whole creation on failure, and only publish
      active state after commit. Add a failure-injection regression showing
      that retry cannot leave an extra or incorrectly typed session.
- [ ] Let each session/thread optionally bind to a Git branch, created from a
      chosen base or selected from existing branches. When a user switches to a
      thread bound to another branch, activate that branch too; leave unbound
      threads on the current branch. Preserve dirty work: use a separate
      worktree for concurrent branch-bound sessions or stop before a checkout
      that would overwrite changes, and never stash or reset without an explicit
      user action. Cover branch assignment, thread switching, session restore
      and concurrent sessions.

## In-app user documentation

- [ ] Add a Help/Guide view reachable from AHEAD's main UI that explains the
      shipped features, settings, shortcuts and common workflows. Keep the guide
      with the app, use `.agents/skills/humanizer/SKILL.md` when writing or
      revising user-facing copy, and include current screenshots where they make
      a workflow or control easier to understand. Mark partial or unavailable
      features clearly.

## Harness integration (new 2026-09-18)

- [ ] Keep regression coverage for retained native-loop behavior in AHEAD-owned
      test targets; do not repair or restore the copied `codex-core` / `codex-mcp`
      upstream unit-test harness. The latest `codex-core --lib` attempt failed
      before filtering/running tests (329 errors, 192 warnings) on removed
      test-support crates/dependencies and stale product APIs; an earlier
      attempt had 383 errors and 196 warnings. The copied `codex-mcp` test
      attempt likewise failed before execution (119 errors, 8 warnings). These
      copied-harness failures are not product test results or release gates.
      `just test-all` now excludes
      both copied harnesses while retaining their libraries as buildable workspace
      dependencies. `cargo check --locked
      --offline -p codex-core --lib -j 1` passed. Keep removed product fixtures
      and dependencies pruned; port only still-supported behavior that lacks
      AHEAD-owned coverage. Keep subagent context isolation and durable resume
      covered through the AHEAD host; MCP elicitation/approval coverage must
      likewise use that path rather than reviving the copied suite.

- [ ] Finish the native-runtime closure cut. The 2026-09-30 decision is that
      backward compatibility with retired Codex features is not required;
      do not retain aliases, fixtures or runtime branches just for old formats.
      Remove the remaining Guardian replay/safety branches without weakening
      current AHEAD permissions or deleting user data. Replace plugin attribution and
      compatibility types, remote execution-environment compatibility and
      analytics providers with AHEAD-owned settings, editor search/filesystem,
      approval, MCP, skills/instruction and memory seams.
      The dedicated Guardian inference routes and `free_guardian` toggle are
      removed; do not restore them while replacing approval call sites.
      The unreferenced copied `policy_template.md`, `node_repl_policy.md` and
      bundled `policy.md` assets are removed, along with the unused policy
      resolver and `guardian_policy_config` / `[auto_review].policy` config
      paths. The dormant Guardian v2 feature/config are removed. The four
      Guardian-only Node REPL/reviewer feature toggles are also removed from the
      active feature registry; their legacy config keys remain accepted but are
      ignored. The
      model-instruction schema no longer exposes model-catalog auto-review
      policy or reviewer-specific approval text; stale catalog fields are
      ignored and their policy payloads removed from the bundled catalog. The
      unused `ModelInfo.auto_review_model_override` field and bundled values
      are removed. The separate `node_repl_auto_review_required` and
      `node_repl_disabled` fields still flow to MCP request metadata and must
      stay until the retained Node REPL compatibility path is removed or
      replaced. The legacy `guardian_approval` feature key and `auto_review`
      feature requirements are now ignored. Model-based automatic-review
      selection has been removed from native startup, step changes and MCP
      reviewer selection; AHEAD's sandbox
      and scoped effect checks remain authoritative, while explicit legacy
      AutoReview requests still fail closed. Core approval contexts, request
      DTOs, action formatters and module paths are now AHEAD-owned under
      `core/src/approval`. Serialized
      `GuardianAssessment*` DTOs and the `guardian` source marker still need
      removal; persisted-session compatibility is not required. The unused `GuardianPolicy` context
      wrapper and special developer-prompt assembly are removed. Legacy source
      markers remain readable and still affect thread metadata; AHEAD rejects
      them before session setup. Guardian-specific downstream session-policy,
      world-state, remote MCP discovery, tool-planning, prompt-schema and
      analytics branches have now been removed. Keep the create/resume/delegate
      rejection boundaries until unsupported source values cannot enter
      the runtime. Do not add migration code or alter user data. A
      production-source search found no creator for Guardian-labeled reviewer
      subagents. The dead
      `routes_policy_to_automatic_review` route and its configuration-based
      permission-request shortcut are removed; such requests now follow the
      ordinary host permission flow. Finish pruning the remaining
      approval/session compatibility, including serialized event/source handling
      retained only to read old sessions. All six cached
      `ahead-agent` `runtime_support::tests` pass for workspace-bounded scope,
      edit-scope/read-only enforcement, traversal rejection and provider
      settings. The full `codex-features` suite passes (37 tests), the strict
      config compatibility regression passes (1), and
      `rtk cargo check --locked --offline -p ahead-agent --lib -j 1` passes with
      3 warnings. Focused AHEAD approval-journey tests remain open.
      The uncalled Guardian root-snapshot/version API, its answer-evidence
      cache, and the dead AgentControl provider are removed. Normal
      `request_user_input` handling still serializes and returns the host answer;
      it no longer stores a duplicate fragment solely for the removed reviewer.
      The catalog on-request message is shared. Approval timeout messages use
      fixed AHEAD-owned text. The inert approvals-reviewer selector and
      model-specific auto-review requirement have been removed; stale
      `approvals_reviewer` and `required_on_models` fields are ignored rather
      than normalized to a user choice. `auto_review.ignore_rules` remains an
      active fail-closed filter for executable-prefix allow rules, and the
      separate browser/computer-use review policy remains. Prompt assembly no
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
      removed; serialized Guardian assessment DTOs still need removal. A 2026-09-29 closure audit confirms Zed's
      agent/agent-server source has no Guardian path and uses ordinary
      tool-permission settings. The retained runtime rejects
      `Internal::Guardian`, subagent `Other("guardian")`, and
      `ThreadSource::GuardianReview` at thread-manager create/resume and direct
      delegate entry boundaries. Downstream Guardian-only session setup,
      policy, world-state, remote MCP discovery, tool routing, prompt-schema
      and analytics branches are removed; legacy event/source decoding and
      history metadata remain. Remove the remaining Guardian history/source
      compatibility DTOs and their parser tests; backward compatibility is not
      required. JSONL import and its Guardian-history regression are removed.
      Keep rejection of unsupported automatic-review entry points fail-closed.
      Do not rewrite or delete existing user data during source cleanup.
      The Codex `[analytics]` and profile toggles are removed, and the inert
      sink no longer accepts credentials or a destination URL; its unused
      no-op `flush` method is deleted (no callers existed). The no-op app
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
      its provider/export policy is replaced with an AHEAD-owned setting.
      The AHEAD MCP Guardian-review adapter and duplicate policy branch are now
      removed. The MCP client retains elicitation auto-accept/decline behavior,
      but `native_client.rs` currently declines server elicitation events rather
      than presenting them. Add a bounded, cancellable form/URL review path in
      the AHEAD session and GPUI panel, advertise only supported capabilities,
      and verify request/response plus cancellation in a disposable project.
      Tool-call approval through chat questions is a separate path. Strict
      automatic-review requests fail closed. The optional
      model-reviewer API was also removed from the retained MCP client. The
      copied-core session round-trip regression was not run; if that behavior
      needs additional coverage, port it into an AHEAD-owned test target. The
      unused Guardian MCP approval-metadata session
      cache and request builder are also removed; normal AHEAD tool-approval
      metadata remains.
      The duplicate Codex file-search crate and unused Bedrock/AWS provider
      closure are removed; do not restore either to the default build graph.
      Model credentials now use the AHEAD-owned `ahead-model-auth` package;
      Codex account login/storage/refresh, account keyring, workload identity
      and Agent Identity are removed from the app graph and source. Do not
      restore them. The separate MCP OAuth/local-secrets path still compiles
      `codex-keyring-store`; retain its credential safety until an AHEAD-owned
      replacement is specified and tested.
      Marketplace catalog/recommendation/install paths, plugin prompt/skill/hook
      injection, plugin MCP projection and the manager/store/remote package
      closure are removed. Command attribution now lives in the AHEAD-owned
      `ahead-tool-records` crate; do not restore the old core-plugin facade.
      Prune the remaining hosted-Apps tool formatting, approval metadata,
      file-upload rewriting and event registration in
      `core/src/mcp_tool_call.rs`, `core/src/mcp_openai_file.rs` and
      `codex-mcp/src/`. Remove the unused Apps config/schema and managed
      requirement types in `config/src/config_toml.rs`, `config/src/types.rs`
      and `config/src/config_requirements.rs`, plus their copied fixtures.
      AHEAD rejects `codex_apps` as a workspace server; no compatibility shim
      is required.
      Preserve ordinary per-server tool caching, exact-client bindings,
      cancellation, server-owned credentials and generic MCP approval.
      Use Zed's pinned `context_server_store.rs` and agent
      `context_server_registry.rs` as lifecycle references. Completed pruning
      and test evidence are recorded in `docs/development/zed-tracking.md`.
      Add and test the server-auth UI before allowing workspace HTTP MCP
      declarations. AHEAD currently declines server elicitation and advertises
      no form/URL capability; the rendered approval and OAuth journeys remain
      release gates, not outcomes proven by config-projection tests.
      The disabled Guardian reviewer route and its no-op review stubs are now
      removed; strict approval requests route to the human. The new
      `strict_mcp_review_routes_to_the_human` regression is in the copied
      `codex-core` suite, but that test binary fails before filtering; port the
      human-review path to an AHEAD-owned end-to-end test instead of restoring
      copied test dependencies. The AHEAD host's 14 MCP-filtered tests now pass,
      covering opt-in/config policy but not the rendered review journey. Legacy
      reviewer selectors are
      ignored instead of normalized. The reviewer enum and selector fields
      are removed from config, profiles, managed requirements, analytics, MCP
      policy and runtime state; model-required Guardian review constraints are
      also removed. Existing old-session compatibility tests are candidates for
      pruning, not release requirements. Managed
      `auto_review.ignore_rules` remains because it filters executable prefix
      allow-rules for selected models. Remove the remaining Guardian
      session-source and tool-filter branches; current AHEAD session restore
      and permission enforcement still need coverage.
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
      `confirm` choices; unlisted tools still prompt and tracked declarations
      cannot auto-approve. Add Zed-style one-time and persistent choices to
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
      AHEAD disables the copied `NetworkProxy` feature and the config loader
      discards `features.network_proxy`, but `codex-network-proxy` remains in
      `core`/`exec-server` and the retained core still owns proxy startup,
      remote-launch, protocol and policy call sites. The crate is already a
      fail-closed compatibility shim: its `disabled.rs` build/launch/run APIs
      return errors, while legacy config, remote DTOs and call sites remain.
      This is compile-time residue, not a running proxy service. Zed's
      `../zed/crates/agent/src/sandboxing.rs` and
      `tools/terminal_tool.rs` enforce command network scope at the sandboxed
      tool boundary. AHEAD's per-turn profile builder currently sets
      `NetworkSandboxPolicy::Restricted` for both Learn and Assist, and the
      copied config tests preserve that restriction when the proxy feature is
      disabled. The copied macOS Seatbelt builder is closed by default, but if
      a profile ever becomes `Enabled` while the proxy remains disabled it
      emits unrestricted inbound/outbound network rules. So the present AHEAD
      default remains denied, but disabling `NetworkProxy` alone is not a safe
      basis for future network grants. Before removing the proxy crate and
      serialized compatibility types, replace its launch/policy call sites with
      an AHEAD-owned sandbox-boundary policy modeled on Zed's
      `NetworkRequest` -> `SandboxNetworkAccess` -> `SandboxNetPolicy` flow (or
      keep managed command networking permanently restricted); verify deny
      behavior on every supported OS and host-specific enforcement where
      offered. The current managed-runtime guarantee and this removal gate are
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
      deleting each dependency. `ahead-agent/runtime/SOURCE_BASELINE.toml` and
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
      current-source writes are transactional. The focused memory suite passes
      15 tests (114 filtered; re-ran 2026-09-29) with `rtk cargo test --locked --offline -p
      ahead-proxy --lib memory_ -j 1`, including raced-symlink,
      non-regular-file and size-limit regressions. The complete `ahead-proxy`
      library suite previously passed 103 tests.
      Workspace startup and search now validate memory sources through the
      shared AHEAD path guard; search refreshes serialize against AHEAD writes.
      Memory indexing, snapshot reads and appends open the leaf with
      `O_NOFOLLOW | O_NONBLOCK` on Unix and verify the opened descriptor is a
      regular file; indexing also rejects documents over 32 KiB. This avoids
      following a raced symlink or blocking on a raced FIFO.
      Composer-side memory search prefix and bounded replacement parsing pass
      their two focused app tests (re-ran 2026-09-29; 2 passed, 71 filtered;
      `rtk cargo test --locked --offline -p ahead-app --lib memory_ -j 1`).
      In the disposable native project on 2026-09-29, the rendered composer
      found a project memory line, attached its relative-path excerpt as
      context, removed the chip, and found a new line after an external file
      edit without restarting the app or sending a model turn. User-scope
      search and the reviewed-replacement journey remain unverified live.
      A message context menu now explicitly saves a selected
      message to the user-chosen project or user file, with bounded,
      message-idempotent append, an 8 KiB note and 32 KiB document cap,
      symlink checks and immediate index refresh.
      Managed AHEAD slash commands now attach a bounded current snapshot for
      project/user memory review and run that turn with the native read-only
      filesystem profile. A complete replacement can be applied only from its
      completed assistant message; the host checks the snapshot hash,
      atomically replaces the markdown file and refreshes the Turso index.
      Verify user-scope search, append, snapshot review, stale-write rejection
      and replacement in a running GPUI session; run focused agent/proxy/app
      tests when build-disk headroom permits.

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
      (`/:name` remains accepted for existing user-skill invocations); the
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

- [ ] Finish validation of the Turso-only managed session path. The copied
      `LocalThreadStore` and its legacy SQLite feature, rollout backfill/listing
      and SQLite config path, SQLite queue exports, `codex-state` crate, and
      root SQLx/SQLite dependencies are removed. Managed sessions use
      `TursoThreadStore` only. The JSONL fallback/importer and its dedicated
      transactional import API are removed; old files are left untouched.
      No backward compatibility or migration layer is required. The current
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
      separate from replay rows. Core resume-by-ID requests full history only
      for legacy threads. Indexed native-thread listing now filters the latest
      source, provider and workspace metadata patches before the keyset limit;
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
      (also passed in the full 140-test proxy library run on 2026-09-30).
      The fixture uses an offline mock on `127.0.0.1`; restricted runners must
      allow loopback binding. `ahead-agent` now has a focused CodeAnchor test
      for the parent work-session ID, AHEAD actor, changed path/range and quote
      hash, plus `native_agent_cancel_interrupts_an_active_child_before_the_parent`,
      which verifies an active child is interrupted and its Turso edge closes
      (re-ran 2026-09-29; 1 test).
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
      Removed `core/Cargo.toml`'s SQLite-only dev-dependencies and test-only
      state/config seam. The focused `codex-thread-store` paginated tests passed
      after its local adapter was removed, while the rollout library check
      passed after its SQLite listing/backfill removal. A current
      source/manifest and lockfile audit, rechecked 2026-09-29, finds no
      `codex-state`, SQLx, `rusqlite`, or `libsqlite3-sys` package; `ahead-proxy`
      depends only on the Turso `libsql` client for this database path. Managed
      native sessions construct
      `TursoThreadStore`, while retained local and in-memory runtime stores are
      test-only. Rollout test compilation remains open. Retain model-loop tests that
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
      Rustfmt parses all converted source, but the Core test target still needs
      a shared Cargo build slot for compile/runtime validation. The
      `session/tests/guardian_tests.rs` is substantially smaller after removing
      obsolete automatic-review routes and the dead Guardian-subagent fixture.
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
      present. Rerun the focused test above when the shared Cargo slot is free;
      keep the copied core test harness pruned.
      Turso-backed thread listings and metadata-only reads hydrate headers and
      patches without replay rows; legacy full-history callers still load the
      complete replay, while paginated resume reads backward in bounded pages.
      Replay rows already have the composite primary key `(thread_id, ordinal)`,
      so keyset paging needs no duplicate index. Native adapter regressions
      cover exclusive page cursors, a 300-row no-cutoff scan, and stopping at a
      safe checkpoint across page boundaries. The existing Turso model-loop
      regression covers paginated child history across reopen/resume; verify
      large/partially-corrupt history startup and selection in the rendered app.
      `SessionStore::list_sessions`
      projects sidebar metadata and harness bindings before the app loads a
      full `SessionView` on selection, following Zed's metadata/detail split.
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
      mapping test passes with
      `rtk cargo test --locked --offline -p ahead-agent --lib
      indexed_header_page_request -j 1` (3 passed); all three proxy page
      regressions, including mixed scalar/structured source matching, pass with
      `rtk cargo test --locked --offline -p ahead-proxy --lib
      native_thread_header_page -j 1` (4 passed). Case-sensitive substring
      search now filters name, preview, title and first-user-message metadata
      in SQL before the timestamp keyset/limit, preserving current-patch and
      name-clear semantics without hydrating nonmatching headers. Correlated
      latest-patch reads use the existing `(thread_id, ordinal)` primary key;
      this is still substring matching, not indexed full-text ranking. Project
      and section filters, plus recency/section-position ordering, still use
      full header hydration pending indexed summaries. The indexed keyset path
      hydrates metadata only for the requested page.
      Verify startup and selection in a rendered app with large and partially
      corrupt history, then drive the persisted model-issued child spawn/resume
      path through a GPUI app restart. The file-backed NativeClient/Turso
      regression covers model-issued resume after reopening the client and
      database; it does not prove the visible app restore flow. A focused
      `NativeClient` regression covers root-only tool exclusion after
      restoring a child from current Turso metadata. JSONL import is removed.
      Keep visible conversation and model-replay rows separate within `.ahead/session.db`;
      they have distinct UI and replay contracts. Managed turn startup now uses
      the indexed next-message sequence to detect the first message instead of
      materializing the full conversation for title generation. `retry_request`
      now scans bounded indexed pages for the target failed/cancelled agent turn
      instead of materializing the full conversation; its worker-failure
      regression places the retry target behind 100 newer messages. The focused
      locked/offline test passed on 2026-09-28 (1 passed, 78 filtered).
- [ ] Finish managed-skills provenance and policy: the runtime system-skill
      bundle now comes from `ahead-agent/skills/` rather than Codex sample
      assets, while project/user discovery remains `.agents/skills/` and
      `~/.agents/skills/`. Verify metadata-first/body-on-selection behavior in
      a live turn. The root-resolution regression now creates `.agent/skills/`
      and `.skills/` decoys and asserts only the project `.agents/skills/` root
      is discovered. The slash catalog preserves same-named skills by source
      and uses source-qualified names without exposing absolute
      paths. Still pin
      each selected skill revision in task/session records,
      and make the host—not the skill—enforce capabilities and authorization.
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
      Unix target-instruction reads now use the shared no-follow directory
      handle, so replacing an ancestor with an outside symlink cannot redirect
      the read after target validation. The shared helper's focused
      disposable-project regression and all 34 `ahead-core` tests pass
      (2026-09-29), and
      a locked/offline `ahead-agent --lib` check passes. The agent-specific
      `target_instruction_read_rejects_replaced_parent_directory` regression
      passed on 2026-09-30 (1 test); Windows still has a path-based read
      boundary.
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
- [ ] The built-in AHEAD runtime now directly hosts the retained core through
      `ahead-agent/src/native_client.rs` and is the default tier; verify a real
      multi-tool turn against the agreed policy. Learn must stay read-only;
      Assist writes are workspace-bounded by default and explicit non-empty
      paths narrow that scope. Per-edit approval cards are not part of the
      managed UX; ACP remains compatibility-only. The streamed-turn active-file
      preflight now uses the same segment-aware normalized path matcher as the
      native write boundary; its sibling-prefix regression still needs the
      focused offline agent/proxy test run after the shared Cargo loop stops:
      `cargo test --locked --offline -p ahead-agent --lib
      scope_rejects_parent_traversal_and_accepts_descendants -j 1` and
      `cargo test --locked --offline -p ahead-proxy --lib
      test_streamed_agent_turn_stale_policy_and_scope_reject -j 1`.
- [ ] Finish removing unused upstream config-loader code. The retained
      loader no longer infers a system-config path from Codex platform
      locations; the managed NativeClient also skips any explicitly supplied
      system-config layer and Codex project-config discovery. The focused
      `ignored_system_config_is_not_loaded` and
      `system_config_is_not_inferred_from_the_platform` regressions pass.
      Host-managed requirements remain a separate policy layer. The AHEAD
      runtime also ignores Codex login requirements. The
      Codex config-schema generator and its only in-tree consumers (fixture tests
      and editor-integration notes) have been removed. AHEAD does not edit or
      publish the Codex config schema, and the shell-snapshot test no longer
      names its removed generator binary. Runtime config parsing remains because
      managed provider settings still use the retained loader. The
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
      Unix approval now binds the lock, declaration/settings reads and atomic
      settings rename to one no-follow `.ahead` directory handle; new lock and
      temp files are owner-only. A disposable-project regression replaces the
      `.ahead` pathname after the handle opens and checks that the outside
      target remains untouched. The full `ahead-agent --tests` Cargo check
      passes, but run `cargo test --locked --offline -p ahead-agent --lib
      mcp_settings_write_stays_with_open_directory_after_parent_replacement
      -j 1` and `mcp_declaration_approval_is_pinned_and_preserves_private_settings`.
      The non-Unix approval
      fallback still uses path-based reads
      and writes; implement and test an equivalent boundary before claiming
      cross-platform symlink-race safety. Remaining: verify
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
      responses are suppressed even if the JSON-RPC id is reused. The 28
      non-network `acp_client::tests::` pass with
      `rtk cargo test --locked --offline -p ahead-agent --lib
      acp_client::tests:: -j 1 -- --skip
      stdio_mcp_loop_reads_cancellation_while_editor_tool_call_is_pending`.
      The end-to-end stdio regression also passes in a loopback-enabled runner:
      `rtk cargo test --locked --offline -p ahead-agent --lib
      acp_client::tests::stdio_mcp_loop_reads_cancellation_while_editor_tool_call_is_pending -j 1`.
      Together these runs pass all 29 ACP client tests. The 2026-10-01 socket
      regression also verifies a delayed, fragmented request on an accepted
      nonblocking socket; the blocking worker now normalizes its mode so
      macOS cannot fail mid-frame. Stdin and local socket requests now stop
      reading after 1 MiB plus one byte, rejecting oversized frames without
      waiting for a newline or EOF. The stdin reader terminates on oversized
      or invalid UTF-8 input rather than queuing trailing bytes as commands.
      Both regressions failed against the old unbounded reader; all 107
      enabled agent tests now pass, including fragmented reads and stdio
      cancellation (two opt-in tests ignored). Verify these fixes in the
      rebuilt app with an external agent once disk space and provider access
      are available.
      Bound the raw stdio event queue and local socket worker admission in
      `ahead-agent/src/acp_client.rs`; the 16 pending-tool limit does not bound
      either. Add an absolute frame deadline (the socket's current 20-second
      timeout resets per read) and separately sized response limits, with
      slow-input, overload and cancellation regressions. The input frame cap
      alone does not establish total memory or worker bounds.
      Keep ACP `mcp/message` optional and separately probed; it is
      experimental and not the interoperability baseline. Still verify against
      official protocol fixtures and Pi, Codex and Claude clients before
      claiming production interoperability. The user reconfirmed on 2026-09-28
      that Pi provider access is not yet available; do not attempt a live Pi
      turn until access is restored.
      The dormant credential-broker
      project-loader trust/env binding and requirements path is now removed;
      project-local `features.network_proxy` is discarded because AHEAD disables
      it. The remaining broker fields, conversion, environment shims and
      broker-only copied tests have now been removed from the retained runtime.
      The 2026-09-29 Cargo graph audit still finds `codex-keyring-store` and
      `codex-secrets` transitively through `codex-rmcp-client`, which remains a
      dependency of `codex-core` and `codex-mcp` for standard MCP OAuth. AHEAD's
      managed MCP config accepts local stdio only, so its retained Streamable
      HTTP/OAuth branch is not reached by the product. Zed implements the HTTP
      transport and OAuth as editor-owned MCP features in
      `../zed/crates/context_server/src/transport/http.rs` and
      `../zed/crates/project/src/context_server_store.rs`. The Codex-specific
      enterprise-managed MCP Identity/ID-JAG exchange, OIDC claim validators,
      `ema-idp:` credential namespace and their isolated tests are now removed;
      there were no callers outside `codex-rmcp-client`. Open scope decision:
      either implement Streamable HTTP and credential handling as AHEAD-owned
      editor behavior following Zed, or prune the dormant HTTP/auth branch and
      its keyring/secrets dependencies if local stdio is the intended boundary.
      Source search finds no remaining EMA/ID-JAG references, and focused
      rustfmt passes. After the shared dev loop is confirmed idle, run
      `cargo test --locked --offline -p ahead-agent --lib` to compile and
      exercise the retained MCP client through AHEAD's runtime. Do not conflate
      MCP auth with AHEAD's BYOK provider credentials.
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
      The broader fail-closed proxy compatibility types and copied
      exec-server/network-policy call sites still need removal or AHEAD-owned
      replacements.
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
      `config_defaults_persist_per_supported_agent` regression and all 22
      `acp_client::tests` pass. Synthetic ACP fixtures omit the unused editor
      MCP bridge; they cover model selection, saved defaults, typed boolean
      options, and option updates but do not verify the production bridge or
      rendered controls. `ThreadsPanel` now fetches the supported-agent catalog
      and persists install/remove state off the GPUI foreground thread; catalog
      failures are shown and stale replies are ignored. Verify default
      application, model/provider changes, reconnect, loading/error states and
      responsiveness in the running app.
      The pre-launch notice and active-session banner now state that AHEAD does
      not mediate shell/file effects or guarantee teaching read-only, path-scope
      enforcement or CodeAnchor attribution; verify their rendered layout in
      the running app.
- [ ] Permission policy currently auto-selects the first `allow_*` option in
      assistance tasks; unknown or unmapped ACP session modes now cancel rather
      than inheriting Assistance. With the ACP tier, this is still only a client
      response, not an effect-enforcement boundary.
- [ ] Add ACP Registry binary-distribution support for platform archives, SHA-256
      verification and update status. Npx registry agents are discovered from
      the cached ACP registry, refreshed asynchronously like Zed's
      `AgentRegistryStore`, and installed with package-lock/package.json
      provenance under AHEAD's shared user-local data directory; binary-only
      entries are intentionally not offered yet.
- [ ] Bound ACP adapter-cache growth in `ahead-agent/src/adapters.rs` without
      deleting package generations still used by live processes or resolved
      launch commands. Atomic publication now retains older generations;
      add lease-aware cleanup and safe recovery of abandoned staging directories
      after acquiring the install lock. Do not sweep existing user caches as
      part of source cleanup. Also verify real npm install/update failures and
      concurrent launches through the installed-agent UI; the current
      concurrency/failure regressions use disposable fake packages. A separate
      offline smoke test packs/installs two local tarballs with real npm and
      launches both generations with Node; it passed on 2026-09-30 without
      reading user npm config or contacting a registry/provider.
- [ ] Diagnose the live Pi ACP model-response failure in
      `ahead-agent/src/adapters.rs::pi_registry_adapter_completes_an_acp_turn`.
      A 2026-09-25 run with Pi v0.87.1 selected the advertised
      `amazon-bedrock/amazon.nova-micro-v1:0` option and confirmed the updated
      model through ACP, but the prompt ended with `end_turn` and only Pi's
      startup/extension notice, not `PONG`. A direct Pi request to that same
      model with tools, extensions and persisted sessions disabled returned
      `AccessDeniedException: Bearer Token has expired`; do not make another
      provider request until the user reauthenticates. On 2026-09-26 the user
      reconfirmed access is not ready and asked us to continue offline. The
      configured OpenCode model `opencode/x-preview-f-free` is not among Pi
      ACP's advertised choices. Unsupported-model errors now include a bounded
      sample of offered choices. Earlier, a 2026-09-24 standalone check found
      Pi's configured OpenCode default model unavailable; a listed OpenCode free
      model was rejected outside OpenCode, the stored OpenAI key was rejected, and
      OpenCode Go required an active subscription. Pi's local auth-readiness
      check alone did not prove live provider access. Provider access remains
      unavailable per the user's 2026-09-26 confirmation; continue offline and
      do not retry a live provider request until access is restored. AHEAD now
      applies an explicitly selected ACP model through advertised
      `configOptions`
      on new and resumed sessions. A real ACP rerun selecting
      `opencode-go/mimo-v2.5` reached `session/prompt` but ended after three
      retries with no model answer, consistent with the standalone subscription
      rejection. A fresh 2026-09-24 auth check reported `ready` for both
      OpenCode Go and Bedrock, but this was not proof of access: ACP with
      `amazon-bedrock/amazon.nova-micro-v1:0` also emitted no answer. The
      ignored live test asserts that ACP reports the requested model after
      `session/set_config_option`, shuts down the adapter before assertions,
      and summarizes relevant events rather than dumping every model choice.
      The offline
      `updates_and_republishes_a_live_acp_config_option` fixture now verifies
      advertised-option validation, the live config-option request, and
      republishing the updated value; its focused test passes when the local
      loopback bind is allowed. A new stdio regression sends an
      agent-originated `config_option_update` before the `session/new` response
      and asserts that the newer full snapshot is published only once. It
      passed on 2026-09-29:
      `rtk cargo test --locked --offline -p ahead-agent --lib
      agent_config_update_before_new_session_response_remains_authoritative -j 1`
      (1 passed, 88 filtered).
      The controller-level regression also verifies that early config events
      are delivered after the ACP-to-AHEAD session binding is established. It
      passed on 2026-09-29:
      `rtk cargo test --locked --offline -p ahead-agent --lib
      buffers_acp_config_options_until_session_binding -j 1`
      (1 passed, 88 filtered).
      A headless GPUI regression also passes for an
      idle panel mirroring the cached option notification. The rendered app
      path and responsiveness during slow session startup remain unverified.
      The offline `cancellation_settles_an_in_flight_prompt_without_agent_response`
      fixture verifies that client cancellation settles a pending ACP prompt
      and its worker without waiting for the external agent's response.
      The editor now prepares external sessions on attach and
      renders the adapter's select options, but the UI update path still needs
      a live GPUI/ACP check, including adapter-originated updates and editor
      RPC responsiveness during slow session startup. Provide valid Pi provider
      credentials and rerun the ignored live test
      with `AHEAD_PI_TEST_MODEL=provider/model` when the default remains stale
      before claiming model-response parity.

## Editor

- [ ] Verify the refreshed `icons/ahead_logo.svg` and Green Serenity themes in
      a freshly rebuilt app: title bar, status bar, thread badges, editor and
      terminal colors, light/dark switching, and the macOS Dock icon set in
      `ahead-app/src/app.rs`. The app check, both `theme::tests`, SVG geometry,
      PNG/ICNS parity, and all ICO sizes pass. Fresh build/live inspection is
      still pending after shared Cargo build-lock contention.
- [ ] Complete the FIM context contract in `docs/development/ahead-editor-mvp.md`
      §8.3: the live buffer, active session title/phase, durable work items,
      work-item notes, conversation summaries and caller work context now reach
      the prediction assembler; the editor now rejects stale async
      completion/FIM responses after edits or file switches, and accepted
      completions now replace/insert at the live editor selection instead of
      appending to the document. Production fallback code is removed; the host
      now dispatches the assembled context to an explicit no-tools
      OpenAI-compatible completion route and fails closed when unavailable.
      Editor-only prediction remains available without a durable session. The
      host now adds applicable workspace `AGENTS.md` files from root through
      the active path and any supplied relevant open-buffer paths to the actual
      FIM request. The session's initial objective is now persisted on
      `SessionTask` in the current Turso schema and included in
      FIM context. A host-to-provider regression now reopens the Turso session
      before capturing the actual Qwen-style outbound prompt; it covers the
      objective, instructions, work-item plan/invariant/decision, summary,
      caller context, unsaved buffer and active-file prefix. Work-item context
      queries only the newest four events per item and returns them
      chronologically with deterministic timestamp ties; a host regression
      checks that later notes and decisions replace stale event context. Inline
      completion now uses the complete live
      buffer at its UTF-16 cursor and adds up to four unsaved buffers that share
      its directory or whose filename is mentioned in the active file, capped at
      4 KiB each. Tests cover Unicode cursor splitting, relevant-buffer
      selection and excerpt bounds. This initial relevance heuristic still
      needs evaluation against real project edits; the full rendered
      context/cancellation contract remains unverified.
      Authenticated-provider, rendered cancellation and acceptance-provenance
      verification still need end-to-end support.
- [ ] Expand the first headless `#[gpui_kit::test]` for the agent question card
      into rendered interaction coverage for streaming, stop, scroll-follow,
      dynamic ACP commands and Enter/Shift+Enter behavior. The 250 ms UI timer
      now polls durable/runtime state only during turn startup or streaming;
      while idle it mirrors cached ACP config-option updates. A headless GPUI
      regression seeds an ACP config-option notification and drives the idle
      poll; `cargo test --locked --offline -p ahead-app
      idle_panel_mirrors_cached_acp_config_option_updates -j 2` passes. Verify
      the timer-driven rendered update and active-turn path in the editor,
      including session restore during an active turn.
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
      connection precedence. Surface inherited profiles/effective source in the
      Settings panel, reject secret fields from tracked config, and verify those
      boundaries in a running workspace. Those three provider readers now use
      `ahead_core::config::read_ahead_config`, which rejects symlinked and
      oversized files; focused core, app and proxy regressions pass. On Unix,
      that reader now uses the shared no-follow directory/file opener and
      limits bytes from the opened descriptor, closing the prior
      check-then-reopen race; its disposable-project regression passes
      (2026-09-29). The non-Unix path still checks metadata before a path-based
      open, so equivalent race protection remains open there. Invalid
      provider-layer errors are now visible in the model picker and Settings
      status, while FIM returns a provider error and the proxy sends a
      deduplicated `ShowMessage` into the shell status bar. A disposable viewable
      macOS bundle in `/private/tmp` passed the closed-Settings warning and
      recovery journey on 2026-09-29. No real project data or provider was used;
      repeat in the normal `just dev` loop when a viewable dev build is available.
      Settings now checks all three layer timestamps in its background loader
      and invokes the chat model reload callback. The proxy's existing workspace
      watcher notifies Settings and the picker on changes even while Settings
      is closed, without reindexing search. User-global `~/.ahead/settings.toml`
      changes are not watched; decide whether they need live reload and test
      that path if enabled. A 2026-09-29 disposable-project GPUI pass verified
      separate title/status/warning lines and non-overlapping two-line skill
      rows. `/smoke` found a newly added project skill after reopening the
      palette; Down changed selection and clicking the skill inserted
      `/smoke-skill` without sending a turn. Still verify active ACP commands,
      narrow panel widths and authenticated skill invocation.
      Keep databases, sidecars and personal overrides ignored.
- [ ] Implement the working-document convention in
      `docs/development/ahead-workflows.md` §9 using the existing session host,
      store and editor save path: named Markdown artifacts with one canonical
      path, private/shared separation, revision/hash indexing and external-edit
      conflict handling. Render from `defaults/artifacts/` with optional tracked
      `.ahead/templates/` overrides; create only `session.md` initially and split
      other documents on first meaningful content. Update artifacts during normal
      work, preview selected publication into `.ahead/sessions/<id>/`, and point
      agents and UI links at the same canonical files. Retain libSQL for runtime
      history instead of duplicating editable documents.

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
      Switching threads should stop speech and microphone capture, discard old
      transcript drafts, clear the prior session's cue and reject queued editor
      actions from the inactive session; verify this in the rendered app.
      Verify ACP stdio MCP startup, discovery and cancel behavior
      across adapters. Local ACP bridge tests cover modern MCP discovery/tool
      listings, presentation dispatch, editor acknowledgements and cancellation
      settlement, including a serialized stdio loop regression. They do not
      verify end-to-end ACP stdio startup/discovery for real Pi, Codex or Claude
      agents, or a rendered frame. Add durable inline code discussion alongside
      the current transient label/note. Compare the full journey with
      `docs/development/ahead-workflows.md` §6.
- [ ] Extend `ahead-app/src/{proxy_client,debug_bar}.rs` and the existing
      `ahead-proxy/src/plugin/dap.rs` route for guided breakpoint setup: retain
      source/debug-session identity, ownership, actual binding/verified status
      and paused thread/frame identity. Preserve the complete per-source set
      when adding an agent breakpoint; replace hardcoded active-line targeting.
      Expose the tested capability to the selected harness and verify the real
      experiment loop in `docs/development/ahead-workflows.md` §5 and §10.
- [ ] Verify the voice loop in a rendered macOS session: grant microphone and
      speech permissions, confirm on-device-only partial/final transcription,
      add and correct a transcript before sending through both managed and ACP
      agents, test mute/stop and microphone barge-in, and verify editor/composer
      typing and debugger controls interrupt speech; confirm stale
      session/generation events are ignored.
      Implement any failures; source plumbing does not prove this journey.
