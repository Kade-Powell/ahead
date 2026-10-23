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

## Current reference baseline

- Checked 2026-10-04: the clean `../zed` checkout is at
  `418f89714891f9d8105a3e92e60b9a7a5084d232` (2026-09-21). Feature links
  below use this immutable revision. The extension WIT copy records its
  provenance and Apache-2.0 terms in `ahead-extension-host/NOTICE`; individual
  source imports retain their applicable notices. Zed's mixed-license source
  boundary and the outstanding clean-room obligation are recorded in
  [Decision 0006](decisions/0006-dependency-replacement-and-zed-reuse-correction.md)
  and `TODO.md`.

## Review log

- 2026-10-13: checked AHEAD's `FileWatcher` against Zed's OS watcher boundary
  in `crates/fs/src/fs_watcher.rs`. Both route native `notify` events through a
  tokenized watcher layer. Added a regression that creates a settings-like
  file under a filtered non-recursive watch and waits for the native event to
  reach AHEAD's path/token queue; the focused proxy test passed. The restricted
  sandbox itself returned `EMFILE` when registering a watch, while the same
  temporary-directory probe and Rust test worked with local filesystem-event
  access. A second regression edits a real temporary workspace
  `.ahead/config.toml` and confirms the event reaches the catalog's
  `RefreshWorkspaceConfigurations` queue. Alongside the installed HTML-server
  refresh smoke, this covers both sides of watcher-to-server refresh, though
  not in one live end-to-end scenario or in rendered editor behavior.

- 2026-10-13: compared AHEAD's project-search worker scheduling with Zed's
  shared background executor in `crates/project/src/project_search.rs`. To
  avoid per-request thread multiplication under concurrent agent searches,
  AHEAD now uses one bounded Rayon pool per proxy process while retaining
  per-query cancellation, ordering and result limits. The source-current search
  suite passes 40 tests. A synthetic 10,000-file profile caps search workers at
  13 instead of up to 104 for eight simultaneous requests, with similar but
  slightly noisier/slower high-concurrency latency; mixed editor/agent
  responsiveness still needs validation.

- 2026-10-13: added a core regression that runs eight independent workspace
  searches concurrently through the shared pool; all return their complete
  256-file match sets, and the current search suite passes 41 tests. An
  eight-request probe through one proxy `global_search` channel was canceled by
  that endpoint's latest-query-wins behavior, so it is not a valid proxy for
  independent agent searches. A focused `NativeClient::file_search` test now
  runs two separate ACP session requests concurrently and verifies each gets
  its own editor snapshot and complete results. This validates the managed
  tool boundary without a provider, not model-issued parallel calls or mixed
  editor/agent performance; that broader load check remains open.

- 2026-10-13: reran the 10,000-file search smoke against the current integrated
  binary with an in-process ripgrep comparison. Indexing took 5.08s; AHEAD's
  no-hit scans took 468/490ms versus ripgrep's 403/369ms. AHEAD returned its
  5,000-result cap in 278ms versus ripgrep's 473ms to count all 10,000 matches;
  those last figures do different work. This single run is not a stable
  benchmark: it identifies cold indexing and no-hit latency for follow-up, not
  an overall performance win.

- 2026-10-13: a fresh integrated-binary run on the same disposable 10,000-file
  fixture indexed in 544ms, with 391/380ms cold/warm no-hit scans and 226ms to
  return the capped 5,000 matches. Same-process ripgrep measured 367/408ms for
  no-hit scans and 441ms to count all matches. This was one cache-sensitive,
  sequential sample, and capped results differ from a full count; it is not a
  general speedup claim. The smoke removed its temporary workspace and profile.

- 2026-10-14: the current integrated binary passed the same 10,000-file smoke
  with same-process ripgrep comparison. Indexing took 4.888s; AHEAD no-hit
  scans took 430/393ms and returning the 5,000-result cap took 254ms. Ripgrep
  measured 414/422ms for no-hit scans and 451ms to count all 10,000 matches.
  This is a single cache-sensitive sequential sample, and capped results versus
  a full count are not equivalent work; it is not a broad speedup claim.

- 2026-10-14, 21:37 EDT: repeated the 10,000-file comparison against the
  integrated binary rebuilt at 21:16. Indexing took 709ms; AHEAD no-hit scans
  took 494/503ms versus ripgrep's 325/325ms. AHEAD returned its 5,000-result
  cap in 367ms, while ripgrep counted all 10,000 matches in 367ms; those are
  different workloads. This sample shows AHEAD's no-hit path about 1.5x slower
  in this fixture, so optimize/profile that path before claiming parity. The
  cache-sensitive single run is diagnostic, not a general performance result.

- 2026-10-14, 22:00 EDT: repeated the same fixture against the 21:16 integrated
  binary. Indexing took 885ms; AHEAD no-hit scans took 447/547ms versus
  ripgrep's 326/341ms. AHEAD returned its 5,000-result cap in 333ms, while
  ripgrep counted all 10,000 matches in 372ms; these are not equivalent hit
  workloads. The inconsistent cold/warm ordering reinforces that these
  sequential timings are diagnostic only; no-hit latency remains a profiling
  target, not a demonstrated regression or parity claim. The smoke cleaned up
  its disposable workspace and profile.

- 2026-10-14: updated the 10,000-file smoke to warm both searchers, then
  alternate execution order over five paired no-hit samples and report
  medians. Against the same integrated binary, indexing took 298ms; AHEAD's
  median no-hit scan was 484ms versus ripgrep's 342ms. AHEAD returned its
  5,000-result cap in 250ms, while ripgrep counted all 10,000 hot matches in
  458ms, which is different work. The paired median confirms a repeatable
  no-hit profiling target in this fixture, not a general performance claim.
  The smoke removed its disposable workspace and profile.

- 2026-10-14: enabled grep-regex's candidate-line prefilter for ordinary
  single-line literal queries, while retaining multiline matching for regexes
  and literal patterns containing CR/LF. The source-current integrated binary
  passed the 10,000-file smoke, including a new multiline-literal regression.
  Five alternating no-hit samples measured 458ms median for AHEAD versus
  ripgrep's 338ms; the prior paired run was 484ms versus 342ms. This is a
  modest directional change, not a controlled before/after result, and the
  no-hit gap remains. AHEAD returned its 5,000-result cap in 730ms while
  ripgrep counted all 10,000 hot matches in 410ms, which is different work.
  Indexing took 5.154s in this cache-sensitive run; continue profiling rather
  than claiming broad search parity.

- 2026-10-13: changed the cached workspace-index cold walk to the pinned
  `ignore` crate's parallel walker, preserving ignore rules, cancellation and
  sorted results; the lazy iterator used by interactive searches is unchanged.
  All 40 `ahead-core` search tests pass. Five alternating samples on a
  synthetic 10,000-small-file tree compared the old serial index walk with the
  new parallel cached index: median 71ms versus 55ms, with identical ordered
  file paths. The fixture was OS-cache-warm and this is one local run, so it
  does not establish broad-workspace startup improvement. The earlier
  lazy-listing comparison did unequal work and was discarded. The integrated
  app compiled, then macOS again aborted before rendering at HIServices
  initialization. Re-run the broader disposable workspace smoke before
  claiming a general indexing win.

- 2026-10-13: repeated the isolated managed Code Mode and file-attribution
  smokes against the freshly rebuilt binary. Search/cancellation, retry after
  Turso reopen, project `.agents/skills/` body loading, applicable `AGENTS.md`
  hierarchy, edit-to-CodeAnchor persistence, staged Git attribution and clean
  shutdown passed with loopback-only mock providers. These tests do not verify
  native rendering or an authenticated provider turn.

- 2026-10-13: the integrated `target/debug/ahead` binary rebuilt by the shared
  `just dev` loop passed a disposable live Zed language-extension/LSP smoke.
  With `AHEAD_LSP_INSTALL_MISSING=1` and
  `AHEAD_LSP_INSTALL_ZED_HTML=1`, AHEAD queried Zed's public extension catalog,
  installed HTML 0.3.2 into the fixture's isolated data home, and received the
  `div` completion for `<di`. The same run acquired Vtsls and BasedPyright into
  that isolated cache and passed TypeScript, JavaScript and Python completion,
  definition, diagnostics/repair, semantic tokens, TypeScript import fixes and
  auto-import, close/reopen, restart recovery, and graceful shutdown. Fixture:
  `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-aKhGzu`.
  It used no model credentials or provider calls. This exercises the live
  catalog-to-install-to-server boundary, not the rendered extension gallery or
  editor completion insertion path. Native app startup still aborts before
  drawing with the macOS HIServices XPC signal 6. The 11:11 EDT crash report
  (`ahead-2026-10-13-111131.ips`) places the abort in AppKit/LaunchServices
  registration during GPUI's `NSApplication` initialization, before AHEAD
  renders; there is no AHEAD-owned frame in the failing stack.

- 2026-10-13: repeated the managed language-server matrix against the latest
  integrated build, with fresh acquisition enabled. Vtsls and BasedPyright
  installed into the disposable project's cache; AHEAD also installed HTML
  0.3.2 from Zed's live catalog. TypeScript, JavaScript, Python and Rust passed
  completion/navigation, diagnostics and repair, semantic tokens, import
  actions, Rust formatting/rename, restart recovery and graceful shutdown.
  Fixture: `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-T8mhdS`.
  This is current proxy/catalog/server evidence, not rendered completion UI or
  extension-provided grammar highlighting.

- 2026-10-13: a fresh managed-acquisition rerun against the current integrated
  binary failed before the first TypeScript diagnostics because npm DNS returned
  `ENOTFOUND`. Seeding a new disposable fixture from the previously verified
  LSP cache restored the full TypeScript/JavaScript/Python/Rust matrix without
  downloads. Completion, navigation, diagnostics/repair, semantic tokens,
  TypeScript import actions, Rust formatting/rename, restart recovery and
  graceful shutdown passed. New fixture:
  `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-mi9Ybz`.
  This is cache-seeded proxy/server evidence; fresh npm acquisition and native
  editor rendering remain unverified in this run.

- 2026-10-13: reran the cache-seeded Rust/TypeScript/JavaScript/Python LSP
  matrix against the shared-loop binary rebuilt at 17:49, using the previously
  verified Zed/Vtsls/BasedPyright/Rust Analyzer cache. Completion, definitions,
  diagnostics/repair, semantic tokens, TypeScript import fixes/resolve, Rust
  formatting and cross-file rename, close/reopen, unsaved-buffer restart
  recovery and graceful shutdown all passed. The same binary passed all four
  fake-LSP shutdown modes. Disposable fixture:
  `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-OyDqHe`.
  This follows Zed's installed-server lifecycle at the proxy boundary; rendered
  editor insertion and extension-gallery UI remain separate gates.

- 2026-10-14: fresh managed acquisition against the current integrated binary
  passed the Rust/TypeScript/JavaScript/Python smoke after scoped npm access;
  package lifecycle scripts were disabled and all caches/data stayed in the
  disposable fixture. Coverage includes completion, navigation, semantic
  tokens, diagnostics/repair, Rust formatting/rename, restart recovery and
  graceful shutdown. A second disposable run fetched Zed's live extension
  catalog, installed HTML 0.3.2 and verified HTML completion alongside the
  TypeScript/JavaScript/Python flows. Fixtures:
  `ahead-lsp-smoke-k0kZUx` and `ahead-lsp-smoke-UJ5Ols`. The sandboxed npm
  attempt still failed with `ENOTFOUND`; the scoped registry run succeeded.
  This confirms proxy, managed acquisition and catalog/install behavior, not
  rendered editor completion or extension-gallery appearance.

- 2026-10-13: the latest serial `ahead-extension-host --lib` suite passed 37
  tests with 2 ignored when ephemeral loopback was enabled for in-process
  fixtures. The latest edits also fixed the fixture HTTP server to read a
  bounded full request header instead of assuming one TCP read receives it all.
  At that point (2026-10-13), Zed's extension grammar/query assets were parsed
  only by the host-side asset reader; the production editor/proxy path was
  added the next day and is recorded below. Restarted
  `just dev` against the disposable project at
  `/private/tmp/ahead-dev-smoke.7T4vXa/project`; the integrated build passed in
  21.54s, then macOS aborted before drawing at HIServices XPC initialization
  (signal 6). After a scoped rustfmt check, that watcher rebuilt the current
  source in 14.32s and encountered the same pre-render abort. Watcher session
  90764 remains active. Native UI appearance and extension-provided syntax
  coloring remain unverified.

- 2026-10-15: compared the shipped path with Zed's
  `crates/language/src/language_registry.rs::get_or_load_grammar`,
  `crates/language/src/syntax_map.rs`, and
  `crates/language/src/language.rs::build_highlight_map`. AHEAD now parses
  installed grammar/query assets in the proxy's extension host and applies
  returned capture ranges as themed editor decorations with stale-response
  fencing; unlike Zed, it uses a separate parser boundary rather than registering
  grammars in the shared language registry. Source-level route and freshness
  tests exist, but rendered colors remain unverified. The opt-in live HTML
  extension smoke could not resolve `api.zed.dev` (`ENOTFOUND`) and stopped
  before installing anything.

- 2026-10-12 storage-scope update: because AHEAD is pre-production, managed
  sessions use one initial Turso schema and do not carry old rollout-file
  compatibility. Removed the one-time JSONL importer, its path-discovery and
  bounded-reader modules, Turso import transaction API and compatibility-only
  tests. Thread resume now depends on the current Turso thread record. Schema
  version checks and migration machinery remain deferred; runtime history
  types and replay policy required by the native agent loop remain. Reaffirmed
  2026-10-13: pre-production database work is limited to the one working initial
  Turso schema; formal schema checks/versioning wait until production readiness.

- 2026-10-20 scope clarification: retain one narrowly bounded exception to
  Turso-only resume. If the requested thread has no Turso header, AHEAD may
  import exactly one matching rollout from that workspace's configured runtime
  home. Validate the thread ID and canonical workspace; reject active/archive
  ambiguity, symlinks and out-of-root paths; bound plain and compressed input;
  atomically persist the header and ordered replay rows in Turso; and leave the
  source untouched. This is not a live file fallback or schema compatibility
  layer; do not add version checks or migrations.

- 2026-10-12: compared AHEAD's live Pi ACP install/turn with Zed's pinned
  `crates/project/src/agent_registry_store.rs` and
  `crates/agent_ui/src/agent_registry_ui.rs`. Zed opens the registry gallery
  from cached agents, then refreshes the official ACP registry; AHEAD follows
  the same cache-and-refresh/install shape while intentionally filtering to Pi,
  Codex and Claude Code. The current official Pi registry item installs
  `pi-acp@0.0.34`, not the Pi CLI. Upstream `pi-acp` starts a separate `pi`
  executable in RPC mode and requires Pi v0.81.0+ already on `PATH`. AHEAD's
  authenticated smoke passed using the host's Pi v0.87.1, which announced v1.2.0
  as an update. AHEAD now checks `pi --version` when resolving an installed Pi
  session, before installing or launching the cached ACP bridge, and gives
  install/update guidance if the CLI is missing, fails, is unparsable or is
  older than v0.81.0. The focused adapter tests cover a missing CLI, the
  minimum accepted version and a rejected older version. Bridge installation
  alone is still not presented as installing Pi. This follows Zed's separation
  between registry adapter installation and agent launch configuration in
  `crates/agent_servers/src/acp.rs`; no Zed source was copied for this check.

- 2026-10-13: followed Zed's ACP configuration path in
  `crates/agent_ui/src/conversation_view.rs` and
  `crates/agent_ui/src/config_options.rs`: session config options take priority
  over separate model/mode selectors and changes go back through
  `set_config_option`. AHEAD uses the same ACP option-first path. The isolated
  `curated_acp_session_model_options_offline` test verified Pi's synthetic
  offline catalog and Claude Code's native model choices, including selection
  updates without a provider turn. Codex first gated `session/new` on
  authentication; with a synthetic API key, the test invoked its advertised
  ACP `authenticate` method, then created a session and changed the model
  option. This verifies the option/auth handshake, not a real provider login
  or model turn.

- Later on 2026-10-09: the freshly rebuilt `target/debug/ahead` binary (15:00:37)
  passed `AHEAD_LSP_INSTALL_MISSING=1 rtk node tests/lsp-smoke.mjs` across
  TypeScript, JavaScript, Python and Rust. AHEAD acquired the servers into the
  isolated disposable fixture, then verified completion, navigation,
  diagnostics/repair and semantic tokens; TypeScript quick-fix/auto-import,
  Rust formatting/hover/signature help/references/cross-file rename,
  close/reopen, unsaved-buffer recovery after restart and graceful shutdown
  passed as well. The fixture and package cache remain at
  `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-jDE6ah`;
  no user project data or model credentials were sent. This is live
  proxy/server evidence, not rendered popup/navigation/Problems-view evidence.

- 2026-10-09: queried Zed's public extension catalog read-only while comparing
  AHEAD's install path with `crates/extension_host/src/extension_host.rs`.
  Separate `provides=language-servers` and `provides=icon-themes` filters
  returned 414 and 32 entries; the combined CSV filter returned only 2, while
  the unfiltered catalog returned 1,000. The combined filter is an intersection,
  so AHEAD's two filtered requests are needed to build the intended union; do
  not combine them or fetch unrelated extension categories. Metadata only: no
  package download, installation, or rendered AHEAD gallery was tested.

- Later on 2026-10-09: the current `target/debug/ahead` binary (12:42) passed
  `AHEAD_LSP_INSTALL_MISSING=1 AHEAD_LSP_INSTALL_ZED_HTML=1 rtk node
  tests/lsp-smoke.mjs`. AHEAD acquired Vtsls, BasedPyright and Rust Analyzer
  into the smoke's isolated temporary homes, then installed HTML 0.3.2 from the
  live Zed catalog using the install/download path compared with
  `crates/extension_host/src/extension_host.rs`. HTML completion and the full
  Rust/TypeScript/JavaScript/Python matrix passed, including TypeScript
  quick-fix/auto-import, unsaved-buffer recovery after server restart and
  graceful shutdown. The fixture/cache remain at
  `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-mz6z4G`;
  no global package cache or model credentials were used. This is real-server
  proxy evidence, not native popup/Problems-view verification.

- 2026-10-09: added `tests/agent-openrouter-smoke.mjs` and ran it against the
  12:42 AHEAD binary with the user-provided OpenRouter connection. One managed
  turn completed through OpenRouter's documented Responses endpoint, exposed a
  streaming conversation row, and used Code Mode `file_search` to return a
  randomly generated fixture value absent from the prompt. The test isolated
  the trusted workspace, Turso data/runtime/user homes and private provider
  settings under one temporary directory, then removed it. The API key was
  written only to a mode-0600 temporary settings file and was not passed to the
  proxy through its environment. The prompt and search result were synthetic;
  no source or personal project data was sent. OpenRouter's [Responses API
  documentation](https://openrouter.ai/docs/api/api-reference/responses/create-responses)
  and [free-model page](https://openrouter.ai/nvidia/nemotron-3-ultra-550b-a55b%3Afree)
  confirm the endpoint, tool-call support and provider data-use notice. This is
  authenticated managed-runtime evidence, not rendered chat, provider-side
  cancellation/resume or general model compatibility proof.

- 2026-10-08: compared the completion popup with the pinned Zed implementation
  after feedback that its list obscured too much code. AHEAD's
  `rank_completions` fuzzy-ranks matching labels/filter text and keeps the top
  eight; the current regression asserts both ordering and the cap. Zed's
  `crates/editor/src/code_context_menus.rs::CompletionsMenu` renders its full
  filtered candidate set through GPUI's `uniform_list` and honors the editor's
  completion-scrollbar setting instead of truncating it. AHEAD keeps the
  requested compact eight-item cap at that review. The cached 15:18 app test binary passed
  the four-test focused completion filter and the above-caret popup-render
  regression; it includes the cap test but predates later `CodePanel` edits.
  After a newer screenshot showed eight rows were still visually dense, the
  source cap and regression were tightened to five, with fuzzy score preceding
  preselect and server sort keys. The cached test evidence predates this change;
  native rendering remains unverified while the desktop is locked.

- 2026-10-07: rechecked managed CodeAnchor persistence and commit attribution
  against current existing binaries. `tests/agent-file-attribution-smoke.mjs`
  used a loopback-only model mock to apply a Code Mode patch and read the
  matching session/actor/path/quote anchor back from Turso. The 22:24 GPUI test
  runner passed the agent-anchor gutter projection test; the 18:43 proxy runner
  passed the dispatcher GitCommit regression for AHEAD author, human committer,
  multi-session trailers, matching-row cleanup and stale-anchor retention.
  These are fresh managed/proxy/GPUI checks, not a single current rendered
  gutter-to-commit run; the desktop is locked.

- 2026-10-07: the `target/debug/ahead` binary built at 22:31 passed the managed
  Code Mode smoke (delegated search, streamed tool result/continuation,
  cancellation with a pending editor snapshot, Turso reopen, explicit retry and
  clean shutdown) and the shutdown smoke (explicit shutdown, stdin EOF and
  SIGKILL recovery with partial response/provenance retained and no automatic
  replay). Current proxy tests also passed memory-index revision/safety and
  file-backed reopen, and the GPUI composer test attached a selected project
  `@memory` excerpt without starting a turn. These used temporary homes and
  loopback-only mocks, with no Cargo build or authenticated provider. The
  desktop remains locked, so this does not establish rendered chat behavior.

- 2026-10-07 22:55 ET: the cached `ahead-agent` test runner (17:03, newer than
  every file under `ahead-agent/`) passed 164 tests serially; 3 were ignored.
  Coverage includes retained native tools, ACP lifecycle/configuration, MCP,
  instruction loading, file-search symbol context, cancellation, Turso history
  and CodeAnchor attribution. The existing app runner (22:24, newer than
  `extensions_panel.rs`) passed all 7 extension-gallery tests: Zed query
  construction, capability/API filtering, offline installed entries and mocked
  install/reload dispatch. No live catalog request, package download, Cargo
  build or provider request occurred. Workspace metadata still resolves to one
  root with 68 members and no nested runtime workspace. Native rendering remains
  unverified while the desktop is locked.

- 2026-10-07 22:58 ET: the ignored ACP install regression passed using two
  locally packed npm archives with `npm --offline`; both installed generations
  launched and returned their expected version, without network or provider
  access. The same source-current app test runner passed all 50
  `session_panel::tests` and 14 `threads_panel::tests`, covering GPUI ACP model
  options and catalog/install error state. This does not verify native OS-level
  installation of Pi/Codex/Claude Code or an authenticated external turn.

- 2026-10-07 23:04 ET: the new fresh-workspace Settings loader regression passed,
  and the focused Settings and terminal app suites passed 29 tests each. The
  terminal checks route platform clipboard shortcuts through the GPUI key
  handler into a disposable shell PTY and verify copied output via the test
  clipboard. This follows Zed's `crates/terminal_view/src/terminal_view.rs`
  split between clipboard-aware Paste, text-only Paste Text, and Copy, plus
  platform-specific key handling; AHEAD's corresponding paths are in
  `ahead-app/src/terminal_panel.rs`. `just dev` rebuilt the app for the
  approved disposable project in 11.53s, but macOS aborted before rendering
  because the desktop session is locked and `hiservices-xpcservice` is
  unavailable. The simulated clipboard is not evidence of native OS clipboard
  or context-menu behavior, and the Settings test is not rendered first-open
  acceptance.

- 2026-10-07 23:07 ET: `just verify` passed offline in the user-approved
  disposable Rust/TypeScript/JavaScript/Python project: 1 Rust test, 2
  TypeScript/JavaScript tests and 1 Python test. Its `.ahead/` directory
  already exists, so this is fixture verification, not first-open editor setup.
  The new fresh-workspace Settings loader regression covers file creation, but
  native initialization remains unverified while the desktop cannot render.

- 2026-10-07 23:12 ET: the current `target/debug/ahead` binary (rebuilt by
  `just dev` at 23:09) passed the real-server `tests/lsp-smoke.mjs` matrix with
  Zed-cached Vtsls, BasedPyright and Rust Analyzer, without downloads or model
  calls. TypeScript, JavaScript, Python and Rust passed proxy-level completion,
  navigation, diagnostics/repair and supported edits; restart recovery and
  graceful shutdown also passed. The companion `tests/lsp-shutdown-smoke.mjs`
  passed explicit shutdown, stdin EOF, initialization disconnect and
  unresponsive-server cleanup. These are headless proxy/server checks, not
  rendered editor behavior.

- 2026-10-07 23:14 ET: the same current binary passed
  `tests/agent-code-mode-smoke.mjs` with a temporary project and a `127.0.0.1`
  mock model. Delegated `file_search`, Code Mode execution/continuation,
  cancellation while awaiting an editor snapshot, Turso reopen/retry and
  shutdown all passed. No external provider was contacted; this is managed
  runtime evidence, not rendered chat or authenticated-provider acceptance.

- 2026-10-07 23:27 ET: the source-current app test runner built at 23:26:14
  (newer than `terminal_panel.rs` at 23:25:34) passed all 30 `terminal` tests.
  The new GPUI `control_c_interrupts_the_foreground_pty_process` regression
  sends Ctrl-C through the focused terminal and confirms a foreground `sleep`
  exits with status 130 and the shell continues. This mirrors Zed's explicit
  Linux `ctrl-c` -> `terminal::SendKeystroke` binding in
  `../zed/assets/keymaps/default-linux.json`. Clipboard-to-PTY paste, output
  copyback and interactive TUI mouse reports passed in the same test group.
  The shared `just dev` build compiled in 9.91s, then macOS aborted before
  rendering with the locked desktop's `hiservices-xpcservice` error; its
  watcher remains on the disposable project. Native OS clipboard and menu
  behavior remain unverified.

- 2026-10-07 23:29 ET: after pausing the shared watcher, the focused
  `shell_osc52_copy_reaches_the_gpui_clipboard` GPUI test passed. It sends an
  OSC 52 clipboard-write request from `/bin/sh`, exercises AHEAD's terminal
  event pump, and reads the decoded text back from GPUI's clipboard. This
  follows Zed's `ClipboardStore` handling in
  `../zed/crates/terminal/src/terminal.rs`. The rebuilt app test runner passed
  all 31 `terminal` tests, including keyboard clipboard round-trip, shell
  interrupt, and interactive mouse reporting. Restarted the same disposable
  `just dev` command; it rebuilt in 10.52s, then macOS aborted before rendering
  with the locked desktop's `hiservices-xpcservice` error. Its watcher remains
  active. Native desktop clipboard/menu rendering remains unverified.

- 2026-10-07 23:32 ET: `tests/lsp-smoke.mjs` passed against the `target/debug/ahead`
  binary rebuilt at 23:30:25, newer than the proxy/catalog/editor LSP sources.
  Existing cached Vtsls and BasedPyright packages plus the cached Rust Analyzer
  were supplied explicitly; no package download or provider call occurred.
  TypeScript, JavaScript, Python and Rust completion/navigation/diagnostics,
  TS quick-fix/auto-import, Rust formatting/cross-file rename, unsaved-buffer
  replay after server restart, and graceful shutdown passed in a fresh
  disposable project. This follows Zed's versioned buffer/server lifecycle in
  `../zed/crates/project/src/lsp_store.rs`; it is proxy/server evidence, not
  rendered popup or editor insertion acceptance. Fixture:
  `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-dbvif7`.

- 2026-10-07 23:50 ET: normal editor changes now cross the app/proxy boundary
  as minimal UTF-8-byte `AheadDelta` splices; opening, reloading and recovering
  still use full snapshots. The proxy validates the base byte length, advances
  its own buffer revision and forwards the edit to LSP. This follows Zed's
  versioned per-server buffer snapshots and `edits_since` path in
  `../zed/crates/project/src/lsp_store.rs::on_buffer_edited`. Focused app
  delta-generation/resync tests and proxy apply/stale-base tests passed. The
  freshly rebuilt `target/debug/ahead` at 23:49:46 passed
  `tests/lsp-smoke.mjs` for TypeScript, JavaScript, Python and Rust through the
  incremental notification path, including diagnostics, completion, navigation,
  formatting, rename, close/reopen and server restart. Existing cached language
  servers were used; the smoke made no downloads or provider calls. This is
  real-server protocol evidence, not rendered editor popup/insertion acceptance.
  Disposable fixture:
  `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-qRlPic`.

- 2026-10-06: `tests/agent-file-attribution-smoke.mjs` passed a headless
  managed turn where a scripted local Responses provider used Code Mode `exec`
  to invoke nested `tools.apply_patch`; the file changed and its CodeAnchor was
  read back from Turso with the expected session, actor, path and quote. This
  used the existing `target/debug/ahead` binary, which predates a later
  `native_client.rs` edit, so it is not current-source acceptance or proof of
  the rendered gutter-to-commit flow. Zed keeps structured buffer edits in
  `crates/agent/src/tools/edit_session.rs::agent_edit_buffer` separate from
  terminal execution in `crates/agent/src/tools/terminal_tool.rs`; AHEAD still
  needs exact-delta attribution for supported shell/MCP/generator writes if
  those effects are to receive anchors. See the actionable gap in `TODO.md`.

- 2026-10-08: wired the existing proxy semantic-token route into CodePanel's
  independent gpui-kit decoration collection. Requests are debounced and
  rejected when the editor generation, path, or byte length has changed;
  standard LSP token names map to the active gpui-kit syntax theme. Three
  focused tests passed for type/scope mapping, UTF-8/malformed ranges, and the
  proxy request/response route. The rebuilt app binary also passed semantic
  token requests from Zed-cached Vtsls for TypeScript/JavaScript, BasedPyright
  and Rust Analyzer through `tests/lsp-smoke.mjs`, with no downloads or model
  calls. This borrows Zed's stale-result discipline in
  `../zed/crates/editor/src/semantic_tokens.rs` while keeping AHEAD's existing
  LSP proxy as the provider boundary. `just dev` compiled the current app, but
  macOS aborted before rendering on the locked desktop's invalid
  `hiservices-xpcservice` connection, so rendered semantic coloring remains
  unverified.

- 2026-10-08: reran the source-matched app terminal suite; all 31
  terminal-filtered tests passed. The GPUI shortcut-to-clipboard-to-PTY paste
  test executes a pasted command exactly once in disposable `/bin/sh`, then
  copies the output back through GPUI's clipboard. OSC 52, Ctrl-C and
  interactive TUI mouse reports also pass. This follows Zed's clipboard and
  platform-key routing in `../zed/crates/terminal_view/src/terminal_view.rs`;
  native OS clipboard and context-menu behavior remain unverified because the
  locked desktop aborts AHEAD before rendering.

- 2026-10-06: the exact shared `just dev` build passed
`tests/lsp-smoke.mjs` against Zed's already-installed Vtsls, BasedPyright and
Rust Analyzer binaries; a command-scoped `PATH`/Rust Analyzer override
avoided downloads. TypeScript passed completion, definition, diagnostics,
quick-fix and resolved auto-import; JavaScript shared Vtsls; Python passed
completion, definition and diagnostics/repair; Rust passed completion,
definition, hover, signature help, references, cross-file rename edits,
formatting and diagnostics/repair. All three servers restarted with unsaved
snapshots and shut down cleanly. This is current-source proxy/protocol
evidence, not rendered completion acceptance or native workspace-edit
application. The `just dev` binary compiled, but its GUI launch aborted with
macOS `hiservices-xpcservice` while the desktop was locked.

- 2026-10-07: revalidated the language-server protocol smoke against
`target/debug/ahead` rebuilt by the 22:31 shared `just dev` loop. One combined
TypeScript/JavaScript, Python and Rust run passed in a disposable workspace
using Zed-cached Vtsls, BasedPyright and Rust Analyzer. Checks covered
completion, navigation, diagnostics/repair, TypeScript import resolution,
formatting and rename where supported, unsaved-snapshot recovery across restart,
and graceful shutdown. No servers were installed and no provider was contacted.
This is fresh proxy/protocol evidence only; rendered editor insertion,
diagnostics UI and native workspace-trust interaction remain unverified while
macOS is locked. Workspace: `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-MwcTIE`.

- 2026-10-06: re-ran `tests/editor-recovery-smoke.mjs` against the current
  shared-dev `target/debug/ahead` binary. Acknowledged Unicode edits survived
  SIGKILL; a second process could not steal a live owner's recovery rows;
  explicit discard and stale-revision fencing survived restart; and the new
  and reopened Turso database remained owner-only (`0600`). The disposable
  project is retained at
  `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-recovery-smoke-LAJVHw`.
  This verifies the proxy/process boundary, not the rendered startup,
  overwrite-confirmation or restore flow in GPUI.

- 2026-10-06: model visibility settings now render one virtual GPUI
  `uniform_list` grouped by provider. Provider sections start collapsed; each
  provider's discovered and configured models are shown only when expanded,
  and hidden IDs are saved and applied within that provider. Models stay visible
  by default. The serial `cargo test -p ahead-app --lib model` run passed all
  17 matching tests, including grouping/filtering, discovery metadata, stale
  response handling, reload coalescing and repaired-config warning behavior.
  The native visual/performance check remains open because the desktop is
  locked; source and GPUI test evidence do not establish rendered scrolling.

- 2026-10-07: rebuilt the app test target from current source and reran the
  model settings coverage serially: all 24 settings-panel tests, all 7
  model-picker tests and all 18 tests matching `model` passed. The test build
  also compiled the current session-panel changes. Provider-specific hidden
  model preferences remain independent, models show by default, and both the
  settings rows and picker menu use virtualized lists. Native scrolling and
  toggle interaction remain unverified while the desktop is locked.

- 2026-10-07: the model-list viewport uses GPUI Base's `ScrollableMask` to
  capture wheel input before the Settings page's virtual `List` can consume it.
  The prior bubble-phase `on_scroll_wheel` handler was too late, and its test
  checked only the offset within the current page item, missing movement to a
  different item. The regression now uses a scrollable 10-item page and checks
  both item index and item offset. This intermediate capture-phase approach
  was replaced by the bubble-phase containment fix documented below; its rebuilt
  regression now passes. Native verification remains open. The vertical mask
  chained to the Settings page when the model list reached its scroll edge.
  The Settings page header is already hidden in source through gpui-kit
  `SettingPage::header_style(hidden)`; the screenshot's title and subtitle do
  not come from the current SettingsPanel content. Confirm that with a rebuilt
  native render before making another header change.

- 2026-10-07: investigated the nested-scroll repro against pinned
  `gpui-base` 0.6.6. `ScrollableMask` captures before an ancestor `List`, but
  deliberately chains vertical wheel input at the inner scroll edge. AHEAD's
  model settings instead need contained scrolling, so a small local boundary
  guard supplements `ScrollableMask` and consumes that handoff. The GPUI test
  now covers the edge; a rebuilt run and native render are still needed.

- 2026-10-07 follow-up: removed the custom shifted hitbox and manual scroll
  offset handling. The model viewport now lets `uniform_list` process the
  wheel event, then calls `stop_propagation` in bubble phase so the Settings
  page cannot consume the same gesture, even at either list edge. This follows
  Zed GPUI's event ordering in `../zed/crates/gpui/src/elements/list.rs`, where
  child scroll handlers run before the ancestor list. The rebuilt, locked,
  offline GPUI regression passes for rows, switches, page movement, and list
  limits. Native wheel/drag behavior remains unverified because the disposable
  `just dev` app repeatedly aborts at macOS `com.apple.hiservices-xpcservice`
  with “Connection Invalid” (signal 6).
  A subsequent locked/offline rebuild from the latest `settings_panel.rs`
  passed the nested-scroll test, all 27 Settings tests, and all 7 model-picker
  tests; five empty-state GPUI click/hit tests also passed against that rebuilt
  app runner. The Settings-page heading is already hidden in source. These
  headless checks still do not verify native wheel behavior or appearance.
- 2026-10-07: the rebuilt app test runner also passed all 44
  `session_panel::tests`, covering model discovery/selection, provider-scoped
  visibility, slash-command and skill filtering/insertion, composer input,
  ACP option controls and chat rendering. The source-current extension-host
  runner passed 26 tests (2 ignored), and its opt-in installed Proto package
  smoke passed separately. These remain headless tests, not rendered native
  acceptance.

- 2026-10-07: ran the current app test binary directly for the slash-command
  picker; all 10 matching GPUI tests passed. The binary is newer than
  `session_panel.rs`; this covers typed filtering, skill metadata, mouse and
  keyboard selection, insertion and Escape without sending. Native picker use
  and a provider-backed skill turn remain separate checks.

- 2026-10-07: a fresh copy of `tests/fixtures/editor-smoke/` at
  `/private/tmp/ahead-editor-smoke.hmqu87/project` passed offline `just verify`
  (Rust 1/1, TypeScript/JavaScript 2/2, Python 1/1). This proves the fixture's
  local tests only; it has not yet been opened in AHEAD or initialized with
  `.ahead/`.

- 2026-10-07: after threading extension IDs through the restart notification,
  the current-source extension-host suite passed 25 tests (2 ignored), the
  proxy catalog suite passed 22 (1 ignored), and the pending-resolution
  invalidation regression passed. A fresh no-run build compiled app, RPC,
  proxy and extension-host tests. The full proxy suite has not been rerun after
  this protocol change; an earlier broad run had 10 local-socket failures in
  the restricted test environment.

- 2026-10-07: dependency inspection found all 52 remaining `codex-*` packages
  in AHEAD's normal dependency closure, so no crate-level removal is justified
  by reachability alone. The managed runtime injects `TursoThreadStore` and
  `TursoAgentGraphStore`; only `ahead-proxy` declares `libsql`, and production
  Core has no rollout-file writer. The guarded JSONL reader is for legacy
  import only. The root app and Code Mode host both exclude
  `opentelemetry-otlp`; session event metadata and W3C trace propagation remain
  without a Codex exporter.

- 2026-10-06: the serial current `ahead-app --lib` suite passed 248 tests when
  excluding only two shared-session integration tests, which are a separate
  multiplayer milestone. The unfiltered run also found a lock-contention test
  fixture that omitted the matching endpoint for its workspace API key; after
  fixing the fixture, that test passed all five GPUI seeds. The remaining
  shared-session failures are recorded under Multiplayer in `TODO.md`.

- 2026-10-05: refreshed current-source managed-agent and live editor evidence.
  The shared `just dev` binary was newer than the changed native-agent and
  language-server catalog sources. `tests/agent-code-mode-smoke.mjs` passed
  against that binary with a disposable loopback model, covering a managed
  Code Mode `exec`, delegated project search, model continuation, cancellation,
  retry after Turso reopen and clean shutdown. In the running GPUI app on the
  disposable multi-language fixture, restarting language servers and invoking
  completion produced a Rust completion menu. The terminal's `rust-analyzer`
  is a broken rustup shim, and the AHEAD language-server cache has no managed
  Rust Analyzer binary; the menu does not prove which executable served it or
  that the acquisition fallback ran. A temporary Rust type error rendered
  the expected inline diagnostic; the editor buffer was restored and the
  fixture's tracked file remained unchanged. Vtsls reached Ready and rendered a
  TypeScript member-completion popup for the imported `doubled` function.
  Opening Python installed BasedPyright into AHEAD's cache; it reached Ready,
  rendered an undefined-name diagnostic, and showed semantic Python attribute
  completions while typing. F12 opened `arithmetic.py` at `double`, and
  Shift+F12 showed references spanning the Python source and test files.
  Temporary test edits were undone and the fixture was confirmed clean. AHEAD
  now adds F2 and Editor-menu Rename Symbol, stages the new name inline after
  `prepare_rename`, then routes the server's `WorkspaceEdit` through an
  all-target preflight for workspace containment, open-document versions,
  editor text, overlapping ranges and shared-buffer write policy. File/resource
  operations are rejected before mutation; code actions remain restricted to
  the active file. The current `just dev` app target compiled against the
  disposable fixture. Focused rename/preflight tests could not start because
  the macOS Data volume is full (252 MiB available; `target` is 71 GiB), and the
  native window exposed only menu chrome to accessibility inspection, so this
  is not rendered UI acceptance. Re-run the focused tests after safe disk
  cleanup and verify F2/Enter/Escape plus a multi-file rename in the fixture.
  On 2026-10-08, AHEAD's manual completion trigger was wired through the same
  named `ShowCompletions` action and `ctrl-space` binding used by Zed's
  `crates/editor/src/actions.rs` and default macOS/Linux keymaps. The focused
  GPUI regression confirms one request with the active file, prefix and caret;
  the existing null-response/retry regression confirms a returned item opens
  the completion menu. Native keypress/rendered verification is still open:
  Zed notes macOS may reserve Ctrl+Space, and this desktop is currently locked.
  Shift+Alt+F invoked Vtsls document formatting and changed a temporary
  `value*2` to `value * 2`; the test edit was restored and the TypeScript file
  remained clean on disk. Still trace Rust executable provenance, test Rust
  navigation, and test Python formatting with an installed formatter. The
  Code Mode smoke uses a local mock and is not authenticated-provider or
  multi-tool GPUI evidence.

- 2026-10-05: compared the managed `spawn_agent` input/output contract with
  pinned Zed's
  [`SpawnAgentToolInput` and `SpawnAgentToolOutput`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/tools/spawn_agent_tool.rs).
  AHEAD now treats omitted, null or blank `session_id` as a new child and
  preserves the created child ID in structured error output, allowing a later
  retry to resume it. The existing single AHEAD tool still creates/resumes
  through `ThreadManager` and Turso rather than exposing a parallel cohort tool
  alias. Delegation guidance also follows Zed's self-contained/non-overlapping
  task and concise-follow-up guidance. This follows Zed's single-tool surface;
  Codex MultiAgentV2's separate wait/message/follow-up/interrupt/list tools are
  not exposed as parallel aliases solely for upstream parity; their compatible
  handler/control behavior remains a candidate to adapt if AHEAD's concurrent
  workflows require it. The user has directed us to reuse retained Codex
  implementations and adapt their effects to AHEAD, not rewrite equivalent
  tools. AHEAD currently reuses Codex Core dispatch and thread execution with
  its narrow Turso/editor-policy adapter. The
  focused regressions and full
  locked/offline agent-library suite pass (158 passed, 3 ignored); rendered app
  behavior remains a separate gate.

- 2026-10-05: a follow-up source trace found that AHEAD's
  `ahead-agent/src/native_client.rs::consume_turn` awaited each dynamic client
  tool inline, serializing multiple model-issued `spawn_agent` calls. The host
  now uses Codex Core's in-flight `FuturesOrdered` pattern from
  `ahead-agent-engine/core/src/session/turn.rs::try_run_sampling_request`
  to poll calls concurrently while returning responses in request order. The
  existing Turso spawn/resume regression now gates two child model responses
  until both requests arrive. Formatting, diff checks and locked/offline Cargo
  metadata pass; that test and native rendered behavior still need verification
  through the shared development loop.

- 2026-10-04: compared opening a changed file with Zed's
  `crates/git_ui/src/git_panel.rs` (`open_selected_entry_on_click` and
  `open_diff`). AHEAD's changed-file row now opens the file, scrolls to its
  first changed line and expands the anchored hunk review card after Git
  metadata loads, including when that hunk is below the initial viewport. This
  reuses the existing editor preview instead of adding a second diff surface;
  it is not Zed's inserted inline diff rows. The focused GPUI regression uses
  a line-40 hunk and asserts scrolling
  (`source_control_open_expands_first_git_hunk_after_refresh`). Native
  rendering remains unverified while macOS is locked. Deleted-file rows still
  reach the regular missing-file error page; a Zed-like deleted diff needs the
  old blob from HEAD rather than a live-buffer read.

- 2026-10-04: followed Zed's ACP `default_config_options` handling in
  `../zed/crates/agent_servers/src/acp.rs` through the AHEAD composer controls.
  A GPUI interaction regression selects a second advertised model and toggles
  an advertised boolean option, asserting the exact ACP requests and that both
  acknowledgements clear the pending state. This verifies the fake-proxy UI
  path, not an installed provider or the rendered desktop app. The duplicate-
  click regression also clicks both the rendered disabled model selector and
  Boolean switch during a pending update and verifies that neither sends a
  second RPC; native affordance verification remains open.

- 2026-10-04: compared AHEAD's Git hunk preview with Zed's
  `../zed/crates/editor/src/git.rs` diff-hunk renderer. AHEAD draws changed
  rows as a continuous gutter rail; clicking a marker opens an anchored
  old/new preview with a Copy action. The GPUI regression verifies modified
  and deleted hunk text and clipboard output. Zed integrates hunk controls
  with editor diff rows, while AHEAD currently overlays a preview; rendered
  comparison is still needed before expanding the implementation.

- 2026-10-04: made `tests/lsp-smoke.mjs` selectable with
  `AHEAD_LSP_LANGUAGES`, so an available server can be tested without requiring
  every language adapter. The Rust-only attempt found that PATH's
  `rust-analyzer` is a rustup shim but the component is not installed; the
  script now probes `--version` and stops before creating a fixture in that
  case. Pointing it at the already-cached Zed Vtsls, BasedPyright and versioned
  Rust Analyzer then passed the full TypeScript/JavaScript/Python/Rust matrix,
  plus Rust-only, TypeScript/JavaScript-only and Python-only selections,
  including server restart and graceful process cleanup, without downloads.
  The tests used disposable proxy fixtures and do not verify rendered GPUI.

- 2026-10-03: investigated the screenshot showing `basedpyright-langserver`
  in Error beside a Ready HTML server, plus a `Managed by ahead-proxy` row.
  AHEAD's Python npm-cache installer has mocked-package tests for success,
  failed-upgrade rollback and concurrent installs, but this host could not
  reproduce the rendered state while locked. A later rebuilt disposable app
  confirmed the old hard-coded workspace/proxy note is absent from current
  source and rendered UI; the latest screenshot still shows it, so verify the
  same `just dev` process before changing presentation. The current catalog
  passes real server names from extension resolution into the panel.
  Pinned Zed's current Python guide treats Python as built-in and names
  basedpyright as its primary server; its language-extension contract requires
  an extension to resolve and return its server command. Zed's activity
  indicator shows install and actionable health/error states rather than an
  extension-host row. Sources: [Python support](https://github.com/zed-industries/zed/blob/main/docs/src/languages/python.md),
  [language-extension server commands](https://github.com/zed-industries/zed/blob/main/docs/src/extensions/languages.md),
  and `../zed/crates/activity_indicator/src/activity_indicator.rs`. Recheck
  extension precedence and the actual AHEAD cache/command before inferring an
  installation bug from catalog presence.

- 2026-10-03: after rebuilding `ahead` from the current checkout, disposable
  proxy smokes passed for managed Code Mode (`exec`/`wait`, delegated shared
  file search, cancellation and retry after Turso reopen), unsaved editor
  recovery after `SIGKILL`, all four fake-LSP shutdown modes, and all three
  fake-DAP shutdown modes. The real TypeScript/Python/Rust LSP smoke was not
  run: this host has `rust-analyzer` but lacks `vtsls` and
  `basedpyright-langserver`, and offline work must not silently download
  them. The Mac remained locked, so these are rebuilt-proxy results, not
  rendered GPUI verification. Source comparison this pass used Zed's
  `crates/search/src/project_search.rs` option handling and
  `crates/extension_host/src/extension_host.rs` gallery flow; AHEAD still
  needs its own reviewed extension feed rather than relying on Zed's service.
  Later the same day, the four headless smokes re-passed against the existing
  00:55 debug binary. That binary predates the latest analytics-source cleanup;
  the post-edit core/agent compile and rendered app checks remain open while
  the desktop is locked and the Data volume has under 200 MiB free.

- Later 2026-10-03: `just dev` rebuilt the current checkout against the same
  disposable editor workspace, but the native process again aborted before
  showing a window with macOS `hiservices-xpcservice` `Connection invalid`
  (signal 6). Using already-installed cached Vtsls, BasedPyright and Zed Rust
  Analyzer binaries—without downloads—the rebuilt proxy passed
  `tests/lsp-smoke.mjs` across TypeScript, JavaScript, Python and Rust,
  including the Vtsls import edit/advertised command and restart recovery.
  The same binary passed editor-recovery, LSP-shutdown and DAP-shutdown
  smokes. These remain protocol/process evidence, not rendered editor checks.
  The focused snippet GPUI regression now proves insertion, one-step Undo,
  placeholder edits and Tab/Shift+Tab; the installed HTML adapter's tested
  `<di` tag item is plain text, so it is not evidence of server-provided
  snippet insertion. AHEAD's LSP snippet support follows Zed's
  `crates/lsp/src/lsp.rs` capability and the LSP 3.18 grammar, without copying
  Zed's GPL snippet crate.

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
  On 2026-10-03, the decision was resolved in favor of lossless BOM-marked
  UTF-16LE/BE editing. Zed's `crates/language/src/file_content.rs` decodes
  BOM-marked input and its `crates/worktree/src/worktree.rs::write_file`
  preserves the encoding and BOM on save. AHEAD now uses a small stdlib codec
  in `ahead-core/src/encoding.rs` and stores the detected encoding with the
  proxy buffer; malformed UTF-16 and binary input remain rejected. Core codec,
  proxy byte-round-trip, editor file-open, and GPUI proxy-snapshot tests pass.
  No Zed code was copied. Native reused-preview rendering remains unverified.

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

- 2026-10-03: the existing debug binary passed the disposable DAP shutdown,
  editor recovery, managed Code Mode and managed-agent shutdown smokes after
  their test profiles were isolated from the normal AHEAD data/runtime/user
  homes. The Code Mode smoke covers delegated search, cancellation and retry
  after Turso reopen; the agent shutdown smoke retains a cancelled partial
  answer on explicit shutdown and stdin EOF. These are current headless checks,
  not a freshly linked app, authenticated provider or rendered GPUI pass.

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

- 2026-10-03: the LSP smoke fixture now sets an isolated user data home and
  grants language-server permission only to its disposable project, matching
  AHEAD's current trust gate. The existing debug binary passed the full
  Rust/TypeScript/JavaScript/Python smoke with local Zed server binaries,
  including unsaved-buffer replay after restart and graceful shutdown
  (`ahead-lsp-smoke-dIoaPW`). This is current proxy evidence, not a fresh
  linked binary or rendered editor proof.

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

- 2026-10-20: extended the registry path to install supported binary archives
  as well as npx agents, following Zed's registry/store separation while
  keeping AHEAD's generation leases. Registry executable paths may be safe
  relative names without a `./` prefix (including Windows `.exe` names), but
  are never resolved through `PATH`. Refreshed the offline Claude ACP fallback
  to `0.94.0`, which the live registry advertised on this date. Source
  regressions were added but not run in this feature pass.

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
  Following Zed's `GitStoreEvent::RepositoryUpdated` / `schedule_update`
  pattern, the active AHEAD sidebar now reacts to the proxy's debounced Git
  generation and reads status on a background task. Headless tests cover an
  external edit in both panels. Following Zed's project panel handling of
  `project::Event::WorktreeUpdatedEntries`, Explorer also rebuilds its tree
  on proxy file-set generations (not content-only edits), preserving expanded
  folders and selection; a headless create/remove regression passes. Native
  open-panel verification remains open. Authenticated models, multi-session
  trailers and hooks/signing remain open.
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

- 2026-10-02: compared Zed's `crates/agent/src/tools/edit_session.rs`
  `agent_edit_buffer` flow, which tags a buffer edit as agent-authored in the
  same effect cycle, with AHEAD's managed `PatchApplyEnd` event. AHEAD receives
  a post-apply unified diff rather than Zed's buffer transaction, so its
  CodeAnchor recorder now checks every added range against current file text
  before attributing it; a later human rewrite cannot be relabeled as an
  agent edit. Renames resolve to their destination, and workspace escapes are
  rejected. Focused agent regressions pass; a rendered multi-hunk/rename
  acceptance run is still open. No Zed source was copied.

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

- 2026-10-04: followed Zed's scoped `git_status_args` in
  `crates/git/src/repository.rs` and clickable Git rows in
  `crates/git_ui/src/git_panel.rs`. AHEAD now limits status and `Stage All` to
  the opened folder inside a parent repository, keeps returned paths relative
  to that folder, and opens a changed file from its keyboard-accessible row
  button so the existing editor gutter preview is reachable. Per-file
  stage/unstage remains on the proxy worker and validates the current status
  before applying literal paths. Focused proxy and GPUI tests pass; native
  rendering remains unverified while the Mac is locked. No Zed source was
  copied.

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
  rejection and rollback tests pass. On 2026-10-06, the corrected unsafe-source
  assertion was rerun with the locked/offline `ahead-proxy --lib legacy_`
  filter: all four tests passed. The Windows stable-handle identity branch
  still requires its Windows CI compile/run result. See `TODO.md` and
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
- 2026-10-03: the persisted-session FIM regression also asserts that a sibling
  `AGENTS.md` outside the active-file/open-buffer ancestry is omitted from the
  actual provider prompt. The focused host test passes with local loopback
  access; this proves context scope, not rendered editor behavior.
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
  command and multiple configuration calls. Following Zed's modified-extension
  reload boundary, AHEAD now carries the installed extension ID with the
  restart notification and evicts only that package's cached guest sessions;
  a manual restart clears all guests. Restart generations reject pending
  startup resolutions from the previous extension set. The focused host and
  proxy regressions pass. Language-server startup and live refresh in a native
  window remain unverified; keep the smoke on disposable copies, not the user's
  Zed installation.
- 2026-10-07: reran the opt-in package smoke against the locally installed Zed
  Proto extension after fixing its archive fixture to include the declared
  `grammars/proto.wasm`. The source-current extension-host test binary passed
  loopback download, AHEAD install/discovery, Wasm command and initialization
  callbacks, and repeated changed workspace-settings reads. No registry or
  provider was contacted; native gallery activation remains unverified.
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
  independent code, not an imported Zed file. It accepts compiled
  include/exclude globs for panel content search and native-agent search.
  Managed `file_search` renders worktree-relative result paths, as
  Zed's agent grep renders project paths, while keeping AHEAD's revision-bound
  cursor validation. The panel caches a proxy-owned path snapshot across
  queries, and simultaneous cold callers share its initial walk, but AHEAD
  still scans candidate file contents serially and lacks Zed's incremental
  worktree index, candidate pipeline and full query controls. AHEAD's `Cmd+P`
  modal now consumes its shared path ranker and generation-tagged proxy
  snapshot. A disposable native-window pass confirmed `Cmd+P`, filename
  filtering and Enter opening a selected file; restart persistence and
  large-worktree responsiveness remain unverified. See the search item in
  `TODO.md`;
  do not describe the current matcher as Zed-equivalent.
- 2026-10-07: rechecked Zed's `crates/agent/src/tools/grep_tool.rs`, which
  adds containing-symbol context from parsed open buffers. AHEAD now uses its
  existing LSP `GetDocumentSymbols` RPC as best-effort context for managed
  `file_search` only: at most 16 open buffers, a 300 ms collection deadline,
  bounded symbol labels, and matching-line fallback. This does not reproduce
  Zed's in-process syntax-tree ancestors. New focused tests and a native
  LSP-backed Rust/TypeScript/Python smoke still need execution.
- 2026-10-07 23:09 ET: a locked/offline Cargo build ran all three current
  `ahead-agent` `native_client::tests::file_search` tests successfully. They
  cover paged cursor/path safety, query and unsaved-buffer fingerprinting, and
  flat/nested/deep LSP symbol ancestry formatting. This closes the uncompiled
  formatter regression only; native LSP-backed search output and Zed-equivalent
  syntax-tree context remain open.
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
- 2026-10-03: ran the disposable `ahead-core/examples/search_profile.rs`
  pre-parallel baseline twice (10,000 files, 10 matches): path indexing took
  88-120 ms;
  sorted search passes were 1,512/2,803/24,657 ms and 1,574/2,826/27,384 ms.
  The >24-second outlier repeated, but the harness sorts its timings, so its
  pass order is unknown. Pinned Zed uses `max(num_cpus - 1, 1)` scan workers;
  this shared-machine synthetic result was too noisy to assess concurrency;
  native large-worktree latency also remains open.
- 2026-10-03: after comparing Zed's bounded candidate worker pool in
  `../zed/crates/project/src/project_search.rs`, AHEAD now scans with up to
  four scoped workers and a bounded in-flight/reorder window. Regression tests
  prove stable input order, the global cap and first-match cancellation (at
  most four file paths consumed). The same 10,000-file harness measured a
  101-ms index and sorted search passes of 656/690/9,150 ms (690-ms median)
  versus the pre-parallel 2,826-ms median. This is preliminary, not a stable
  speedup: shared load and the 9.15-second outlier require controlled
  profiling. No Zed code was copied.
- 2026-10-06: Explorer now consumes the same workspace-file generation signal
  as project search, refreshing external create/delete changes while preserving
  expansion and selection. Direct execution of the current app test binary
  passed `nested_and_empty_directories_remain_expandable` and
  `external_file_set_changes_refresh_tree_without_resetting_state`. These
  headless checks do not establish live watcher delivery or rendered refresh;
  verify those in the disposable editor project.
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
- 2026-10-02: compared Zed's `crates/agent_ui/src/config_options.rs`
  selectors with AHEAD's ACP option picker. AHEAD now disables the picker
  while an option update is pending and serializes ACP option requests through
  response publication and default persistence, so a delayed response cannot
  overwrite a later choice. The out-of-order ACP fixture and duplicate-click
  GPUI test pass; the offline root `ahead` executable builds, and the complete
  `ahead-agent` and `ahead-app` library suites pass with local mock-server
  access. A locked Mac still prevents rendered picker verification, and no
  provider inference is claimed.
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

- 2026-10-14: compared Rust Analyzer resolution against Zed's
  `crates/languages/src/rust.rs`: try the workspace's `rustup` selection,
  validate a runnable `PATH` server, then use a managed official-release cache.
  AHEAD's `ahead-proxy/src/plugin/catalog.rs` follows that fallback and checks
  bounded downloads plus GitHub's release digest when available. On the rebuilt
  disposable app, the managed Rust-only smoke acquired Rust Analyzer into its
  temporary data home and passed formatting, completion, navigation, semantic
  tokens, hover/signature help, references, cross-file rename, diagnostics and
  repair, restart/reopen and graceful shutdown. No global toolchain was
  modified. The same build passed cached TypeScript/JavaScript/Python LSP
  checks and the mocked managed-turn plus CodeAnchor-to-staged-commit smokes.
  Native editor insertion/rendering and authenticated-provider behavior remain
  separate acceptance gates.

- 2026-10-14 follow-up: the current `just dev` build passed the disposable
  managed Code Mode smoke for delegated `file_search`, streaming, cancellation,
  retry after Turso reopen and shutdown. With scoped network access, the live
  LSP acquisition smoke fetched the Zed catalog, installed HTML extension
  0.3.2, and acquired Vtsls, BasedPyright and Rust Analyzer into its temporary
  project cache. TypeScript, JavaScript, Python and Rust passed completion,
  navigation, diagnostics/repair, semantic tokens, supported formatting and
  rename, close/reopen and server-restart recovery. All four fake-server LSP
  shutdown modes passed too. The 240 MiB smoke fixture and shutdown fixture
  were removed after verifying the installed package paths; no global server
  or toolchain was changed. This verifies real-server proxy/install behavior,
  not the Extensions UI or rendered editor interactions. The app child still
  aborts before rendering during macOS HIServices initialization, and no
  authenticated provider turn ran because `OPENROUTER_API_KEY` was absent from
  the test process environment. After stopping the shared watcher, the same
  disposable project's documented `just verify` passed offline with an
  isolated fixture `target/`: Rust 1/1, TypeScript/JavaScript 2/2 and Python
  1/1. The same isolated watcher restarted as session 46782 and rebuilt the
  integrated app in 2.64s; its child then exited before drawing on the macOS
  HIServices XPC signal 6. The watcher remains live. Against that current
  `target/debug/ahead` (13:08), reran both local-fake managed-agent smokes:
  Code Mode search/stream/cancel/retry/shutdown passed, and the file-attribution
  path persisted a session CodeAnchor through staged commit, author/committer
  metadata, session trailer and anchor cleanup. This exercises the managed
  effect path but does not replace authenticated-provider acceptance.

- Fresh clean-build recheck (2026-10-14, 13:47): after the user-requested
  `cargo clean`, the shared `just dev` loop rebuilt `ahead` and the Code Mode
  host from scratch in 12m22s. Compilation passed; the app child again aborted
  before its first frame with the macOS HIServices XPC signal 6. Against that
  fresh binary, the local-fake Code Mode and CodeAnchor-to-staged-commit smokes
  passed, as did the Turso-backed editor recovery smoke. The live Zed catalog
  installed HTML 0.3.2 into a disposable cache, and Vtsls, BasedPyright and
  Rust Analyzer passed the TypeScript/JavaScript/Python/Rust LSP matrix,
  including HTML completion, TypeScript auto-import, Rust formatting and
  cross-file rename. This is fresh proxy/runtime acceptance; the editor's
  rendered popup/install interactions and authenticated-provider behavior
  remain unverified.
  The fresh source-current serial `ahead-app --lib` suite passed 321 tests;
  two multiplayer/session-invite tests failed because the child UI did not
  expose its invite-JSON prompt. These collaboration tests remain with the
  separately tracked collaboration work, not the core editor release gate.

- 2026-10-14, 21:26 EDT: the integrated binary rebuilt at 21:16 passed the
  cache-seeded `tests/lsp-smoke.mjs` for TypeScript, JavaScript and Python.
  Completion (including typed Python signature help), definitions, diagnostics
  and repair, semantic tokens, TypeScript import/auto-import actions,
  close/reopen, server restart recovery and graceful shutdown passed without
  downloads or model calls. Fixture:
  `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-PDywjc`.
  Rust was not included because the current `rust-analyzer` PATH entry is a
  rustup shim whose selected stable toolchain lacks the component.

- 2026-10-14, 21:27 EDT: the same integrated binary passed the disposable
  Turso editor-recovery smoke: acknowledged Unicode text survived proxy
  termination, live-owner protection held, explicit discard and stale-write
  fencing survived restart, and the database remained owner-only. Fixture:
  `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-recovery-smoke-mXQ02o`.
  This verifies the storage/proxy boundary, not the rendered recovery UI.

- 2026-10-14, 21:29 EDT: a current-binary Code Mode smoke could not start in
  this restricted shell because Node's ephemeral `127.0.0.1` mock bind returned
  `EPERM`, before the AHEAD proxy was spawned. This is an execution-environment
  limitation, not a smoke result; managed search/cancel/retry needs a runner
  with local-loopback access.

- 2026-10-14, 21:31 EDT: reran the full cache-seeded Rust, TypeScript,
  JavaScript and Python LSP matrix against the integrated binary. Rust Analyzer
  1.98.1 was already installed in a non-default Rust toolchain, so the test
  used its explicit binary path; no tools were installed. Rust passed
  formatting, completion, definition, semantic tokens, hover, signature help,
  references, cross-file rename, unsaved diagnostics/repair, close/reopen,
  server restart recovery and graceful shutdown. TypeScript/JavaScript and
  Python passed in the same run, including typed Python completion/signature
  help and TypeScript import/auto-import. Fixture:
  `/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-noQj81`.
  This closes current proxy/LSP protocol coverage, not native editor
  insertion/rendering or first-run project bootstrap.

- 2026-10-14, 21:34 EDT: the source-matched app test runner built at 20:09
  passed 330 tests with the two separate-process collaboration tests filtered
  (their local sockets/UI prompt are unavailable in this environment). The
  run includes editor/session/settings, terminal/PTY, search/Git and extension
  gallery coverage. All 14 focused terminal-panel tests also passed, including
  VTE output, input/shortcut routing, mouse reporting, scroll behavior and
  shell-safe path paste. These are headless GPUI/unit checks, not native input
  or rendering verification.

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

## Code Mode sidecar source and protocol check (2026-10-02)

The historical missing-host observation above remains valid for that build.
The pinned Codex `rust-v0.152.0` stdio host and V8 runtime are now distilled
into AHEAD-owned crates in the root Cargo workspace. The Deno release lacks
the sandbox-enabled V8 150.4.0 archive; the matching OpenAI release provided
the macOS arm64 archive and Rust binding, and both matched its published
SHA-256 hashes. The 13 host and 68 runtime tests pass, including the linked
V8-sandbox check. An AHEAD-owned process test launched the new sidecar through
the retained client, executed JavaScript, delegated one tool call with exact
JSON arguments, and received the tool result.

This is protocol-level evidence, not a GPUI or authenticated-model
observation. `just dev` and release recipes now build/place the sibling host,
but a combined editor/host debug build was stopped while still compiling when
free disk fell below 1 GiB. Native CodeModeOnly request/tool behavior and the
signed bundle are still open in `TODO.md`. Zed's direct-tool request assembly
remains the primary editor reference; the sidecar is retained specifically for
models whose pinned Codex tool contract requires Code Mode.

A later 2026-10-02 pass recovered space by removing only the rebuildable
incremental compiler cache. A combined locked/offline `cargo check` and
`cargo build` of `ahead` and `ahead-code-mode-host` now pass. The host's 13
tests, runtime's 68 tests, and delegated-tool stdio smoke pass again. Review
then found AHEAD's managed configuration still disabled `CodeModeHost`, so the
sidecar was unreachable despite packaging. That override is removed; a
managed-config regression and the complete `ahead-agent` library suite pass
(148 passed, three ignored), and the rebuilt binary names the AHEAD host in
its missing-host warning. The desktop was locked, so these results still do
not prove a GPUI CodeModeOnly turn, tool effect, or signed-bundle launch.
The unused copied gRPC client and proto generator have since been removed;
no product caller used them. The remaining Code Mode client/protocol tests pass
(68), as do the 13 host tests, 68 V8 runtime tests, and delegated-tool stdio
smoke after that removal. The combined locked/offline editor-and-host check
and binary build also pass after the prune. Generated compiler caches and
duplicate test executables were removed to keep the disk from filling; all
are rebuildable.

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

## Display-bound windows and search modes (2026-10-01)

Zed's `crates/workspace/src/workspace.rs` leaves `window_bounds` unset for a
new workspace and lets GPUI choose display-aware defaults. AHEAD's fixed
1600×1000 opening window put the new-thread wizard's final control outside the
clickable region on the disposable native smoke display. AHEAD now uses
`TitleBar::window_options()` without overriding the bounds. Rebuilt native
verification of the new size and wizard completion is still pending.

At `../zed` commit `418f89714891f9d8105a3e92e60b9a7a5084d232`,
`crates/search/src/search.rs::SearchOptions::build_query` treats case,
whole-word and regex as independent options; regex selects the query parser,
while the other two refine its matching. In response to the user's feedback
that these looked like mutually exclusive search types, AHEAD's panel now
selects one of Text, Word or Regex, with Case as a separate modifier. This is
an intentional UI departure from Zed's combinable toggles; the shared matcher
still supports combined options for other callers. A GPUI regression covers
mode switching and Case independence; rerun it in the shared Cargo loop. In
the rebuilt disposable native app, selecting Word then Regex left only the
latest mode active, while Case stayed independently selectable. The same
native project confirmed a live-buffer Git rail click opens the hunk review
card without turning that line into a breakpoint; the card still overlays
code instead of expanding editor rows as Zed does.

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
Zed's `crates/project/src/lsp_store.rs::get_language_server_binary` returns a
pending task, then starts the server after acquisition. AHEAD previously ran
installed-extension command resolution inside `ensure_lsp_server` on the
catalog thread, where it could wait on npm. The catalog now runs resolution in
a worker, posts a generation-fenced result, and replays the latest unsaved
Rope snapshot when that server starts. A late result after document close is
discarded; startup rechecks workspace trust. The focused regressions and all
17 active catalog tests pass. The opt-in test also copied the locally
installed Zed HTML extension into an isolated temporary project and observed
a successful real HTML-server handshake through the new async route. This is
headless integration evidence, not native first-open UI proof. Document-open
no longer synchronously rescans installed extensions: the catalog is refreshed
at startup and after explicit install/update restarts, keeping package scans
and the install lock off the `didOpen` path. A filesystem edit made outside the
gallery requires Restart Language Servers. The extension host's child
timeout/cancellation/reaping behavior is recorded below; startup and restart
discovery are still synchronous.
The first opt-in run exposed an AHEAD trust-path bug: macOS's `/var` parent
alias made the secure directory opener reject a valid temporary data home.
`workspace_trust::trust_path` now canonicalizes that path only after rejecting
a symlinked config-directory leaf. A focused Unix regression covers both
parent-alias acceptance and leaf rejection; the real HTML test passes again
using the original `/var` path and an isolated data home.
The extension host runs npm/process children with Tokio async pipes and a
120-second deadline. Timeout and caller cancellation kill and wait for the
child, with Unix regressions covering both paths. Zed's pinned
`crates/extension_host/src/wasm_host/wit/since_v0_8_0.rs::run_command` awaits
`command.output()`; AHEAD preserves its output contract while adding explicit
cleanup for the editor's cancellable install/shutdown lifecycle. No Zed source
was copied.

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
cached npm packages before downloading. AHEAD now gates language-server
startup on workspace trust. Its built-in Rust Analyzer, BasedPyright, and Vtsls
commands still prefer PATH. When the BasedPyright or Vtsls executable is absent from
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
On 2026-10-07, the headless LSP smoke gained a default-off
`AHEAD_LSP_INSTALL_MISSING=1` mode to exercise this real cache-miss path without
changing its offline default. Separate disposable runs installed BasedPyright
1.40.2 and Vtsls 0.3.0 through AHEAD's npm installer (lifecycle scripts
disabled). Python passed completion, definition, diagnostics/repair,
close/reopen, restart and graceful shutdown. TypeScript passed completion,
definition, diagnostics, quick fix, resolved auto-import and restart; JavaScript
shared the Vtsls process and passed completion, definition and diagnostics. The
installer caches and all AHEAD homes remained inside each fixture. The first
Python run exposed a readiness race in the smoke, now fixed by waiting for the
server's Ready/error status before requesting completions. This closes the
headless acquisition/protocol gate, not the current native UI gate; the latter
remains blocked by the repeated macOS XPC launch error.
A combined offline run reused those fixture-local npm caches with the cached
Zed Rust Analyzer and passed Rust, TypeScript, JavaScript and Python in one
project, including the Rust navigation/format/rename path and server restart.
The exact `target/debug/ahead` binary postdates the relevant proxy/RPC sources.
The passing fixture remains at
`/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-g84nPb`.
Zed runs npm through its async Node runtime and directs it to a runtime-owned
cache in `crates/node_runtime/src/node_runtime.rs::build_npm_command_args`.
AHEAD prefers system Node.js/npm and directs installation to
`cache/language-servers/npm-cache`, rather than inheriting a potentially
unwritable user-global cache. When system Node or npm is unavailable, the
extension host and built-in LSP installer share an AHEAD-owned on-demand
runtime: it selects the newest LTS from the official Node index, verifies the
archive against that release's SHA-256 manifest, then atomically publishes a
bounded extraction into `cache/node-runtime/`. Valid cached runtimes are used
offline; a failed online update falls back to a valid cached copy. Unit tests
cover LTS parsing, checksum selection, archive extraction and publication; they
do not yet prove a real clean-PATH network install or Windows launch behavior.
No Zed source was copied for this boundary. The installer also polls its child
with a 120-second total deadline and signals cancellation on proxy shutdown,
waiting briefly for the installer to stop. The existing fake-npm shutdown
regression verified that an active child exits before publication;
the rebuilt disposable macOS app also quit with fake npm active, leaving no
child process or cache entry. Windows still needs a live pass.
The first run also exposed stale health rows after restart. Restart now clears
old server statuses before replay, while the toolbar and panel distinguish a
server still starting from one that failed. Focused proxy and app regressions
pass. In the rebuilt disposable app, switching from Python to JavaScript left
only Vtsls Ready in the panel and the toolbar also reported ready.
For BasedPyright/Vtsls packages launched from AHEAD's cache, the npm `latest`
lookup now starts only after that exact managed server reports Ready. If a
newer version exists, the existing staged publisher installs it and then asks
the dispatcher to restart servers. The currently running server remains Ready
during this background refresh; a failed refresh preserves the existing cache
and does not overwrite healthy status with an install error. PATH-selected
binaries and installed extension servers do not trigger registry requests or
cache restarts, and the check never fetches Node solely to query npm. This
follows Zed's installable npm adapter lifecycle while keeping AHEAD's
editor-owned cache and workspace-trust boundary.
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
The app GPUI test fixture now uses the panel's workspace-aware constructor,
and `cargo check --locked --offline -p ahead-app --tests -j1` passes. Its
permission copy now names language servers specifically, rather than claiming
that the entire workspace is trusted or restricted.
This is source/protocol evidence, not a rebuilt native trust journey. AHEAD's
user-level store now distinguishes undecided, allowed and blocked projects;
the editor defers a first-open language-server choice until its window exists,
and records either button so an explicit block is not prompted again. Focused
store tests and the app test-target check pass. AHEAD's restricted coverage
is still narrower than Zed's: project settings, MCP startup and other
project-driven processes remain open.
Windows ACL/reparse safety and native Linux behavior are also unverified.
The 2026-10-02 `just dev` binary linked for a disposable smoke project, but
two launches exited with signal 6 and a macOS `com.apple.hiservices-xpcservice`
connection-invalid error before a window appeared. The second launch used
`RUST_BACKTRACE=full` and produced no Rust stack. This did not verify the
Allow/Block interaction.

On 2026-10-03, a cold `ahead-app --lib` run compiled but two collaboration
tests could not bind loopback listeners in the restricted sandbox. With
loopback access, one of those tests still required the debug `ahead` sandbox
helper removed by `cargo clean`. After building that binary and isolating
`AHEAD_DATA_HOME`, `AHEAD_HOME` and `AHEAD_USER_HOME` under `/private/tmp`, the
full app library suite passed: 204 tests, 0 failures. `just test-all` and CI
now build the debug helper first. This does not establish rendered UI or
provider-backed agent behavior; the Mac remained locked for this pass. A
separate locked offline `ahead-agent --lib` run passed 149 tests, with three
opt-in tests ignored, using isolated temporary homes and loopback mock
services. The full `ahead-proxy --lib` run passed 225 tests, with one ignored.
An initial proxy run had three legacy/reopen failures because the test command
overrode `AHEAD_HOME`, moving the runtime away from each temporary workspace's
`.ahead/runtime`; rerunning without that override passed. `just test-all` now
uses temporary app/user homes and clears any inherited `AHEAD_HOME`.
The fresh offline `tests/lsp-smoke.mjs` fixture passed Rust, TypeScript,
JavaScript and Python server behavior, including restart and graceful cleanup,
using locally installed Zed language-server binaries. The first attempt
resolved `rust-analyzer` to a rustup shim without the component and failed
before Rust diagnostics; a temporary PATH alias to Zed's standalone binary
made the same smoke pass. This is proxy/LSP evidence, not rendered-editor proof.
The fresh debug binary also passed disposable proxy-process smokes for managed
agent shutdown/EOF/SIGKILL with Turso reopen, all three DAP adapter shutdown
modes, all four LSP shutdown modes, and editor recovery/permissions. These do
not replace native window interaction or authenticated-provider acceptance.
The offline `just test-all` recipe then passed in full after the pinned Code
Mode V8 archive was restored and hash-checked by the repository setup script.
It built the debug sandbox helper, isolated app/user homes, ran the grouped
AHEAD package tests and the new focused API target. `cargo fmt --all -- --check`
and `git diff --check` passed. Credential-requiring ACP/native cases remained
ignored; no rendered window was available because the Mac was locked.
The fresh `target/debug/ahead` and sibling Code Mode host then passed the
disposable loopback `tests/agent-code-mode-smoke.mjs`: delegated file search,
cell output, cancellation, Turso reopen, explicit retry and shutdown.

On 2026-10-05, AHEAD's Rust Analyzer resolver was updated against Zed's
`crates/languages/src/rust.rs::rustup_rust_analyzer_for_worktree` and
`LspInstaller::check_if_user_installed`: for a workspace containing
`rust-toolchain.toml` or `rust-toolchain`, run `rustup which rust-analyzer`
from that worktree, validate the selected binary, then fall back to PATH and
AHEAD's managed cache/download. Unix regression coverage uses a fake rustup
executable for both override-file names. The rustup lookup now uses the
existing cancellable, three-second child-probe lifecycle; a Unix regression
exercises cancellation of a stalled lookup through the shared kill-and-wait
helper. `rustfmt --check` and `git diff --check` pass. Available `ahead_proxy`
test binaries predate this source, so these focused Cargo tests remain unrun.
The live disposable AHEAD window showed BasedPyright Ready and Problems
(0); that is native Python-server health evidence, not proof of Rust toolchain
selection.

On 2026-10-03, AHEAD's LSP completion client began advertising snippet support
after adding a clean-room parser for the supported subset of the
[LSP 3.18 snippet syntax](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.18/specification/):
numbered tabstops, nested defaults, choices and escapes, plus caret placement
and Tab/Shift+Tab navigation.
Completion insertion remains one bounded editor replacement. This is not yet
Zed parity: gpui-kit exposes one selection, so repeated placeholder ranges do
not update together; choice alternatives and variable transforms are not
implemented. Those cases fail closed where unsupported. No code was copied
from Zed's GPL-3.0-or-later `crates/snippet`. Retest the installed HTML server
and native completion/Undo behavior in the disposable editor project. The
focused GPUI regression verifies insertion, one-step Undo, placeholder editing,
Tab/Shift+Tab, final-caret exit and restored native indentation; the proxy
`snippetSupport` capability test passes. The full app-library suite now passes
220 tests with loopback enabled; an earlier sandboxed attempt could not bind
its two collaboration mock servers. The opt-in real-extension proxy smoke was
then extended and passed against the installed Zed HTML adapter and its
matching bundled server: the project setting enabled embedded-CSS validation,
which reported an unknown CSS property; completion at `<di` returned the
`html` tag, formatting unformatted markup returned multiline edits, and rename
changed both paired tag names. These are independent checks—HTML tag items are
plain text; the GPUI test exercises snippet parsing and placeholder
navigation. The first probe exposed that AHEAD returned `null` for an
unconfigured `css` section; the HTML server passed that value to the CSS
validator and panicked. AHEAD now returns an empty object for a missing named
configuration section, and the real-server regression passes. The test also
requires isolated temporary `AHEAD_DATA_HOME` and `AHEAD_USER_HOME`; the latter
now controls the shared user-home resolver rather than silently leaving
extension settings tied to the machine account. The server dynamically registers
`workspace/didChangeWorkspaceFolders`. AHEAD currently owns one immutable root
per app/proxy, so there is no supported root-change workflow to notify. Defer
folder deltas until multi-root support exists; then mirror Zed's
`LanguageServer::set_workspace_folders`. The smoke also turns embedded-CSS
validation off through the catalog's workspace-configuration refresh path and
observes the warning clear without restarting the server. This does not verify
the native filesystem-watcher-to-editor journey. Rendered native editor
insertion/Undo remains unverified.

## Fresh native smoke after `cargo clean` (2026-10-05)

The shared `just dev` app opened the disposable multi-language fixture. In the
rendered Settings panel, the loopback-only OpenAI-compatible fixture passed the
endpoint check, exposed `gpt-5.5` through model discovery, and completed a
text-only managed-agent turn with `AHEAD_TEXT_SMOKE_OK`. This exercises the
native settings and managed request/response path with a deterministic fake
provider; it is not evidence of real-provider quality, tool execution or MCP
approval. No MCP process was approved or launched.

The rendered Language Servers panel showed Rust Analyzer exiting with status
1. PATH resolves to a rustup shim for which the Rust Analyzer component is not
installed; a standalone binary already cached by Zed runs successfully. In
`../zed/crates/languages/src/rust.rs`, `RustLspAdapter::check_if_user_installed`
executes `--help` and declines a broken PATH candidate so Zed can install its
managed release. AHEAD's `ahead-proxy/src/plugin/catalog.rs::ensure_lsp_server`
currently trusts `which::which` and starts the broken shim. AHEAD still needs
the equivalent executable validation and managed Rust Analyzer fallback before
native Rust diagnostics/completion can pass.

In this same app session, opening the TypeScript smoke file installed Vtsls in
AHEAD's cache and the panel reached `Ready`. An unsaved unused-binding edit
produced a Vtsls diagnostic; Undo restored the buffer and the diagnostic
cleared. Opening `python/main.py` then left BasedPyright in error and changed
the Rust/Vtsls statuses to `AHEAD proxy connection closed`. The shared `just
dev` session logged EOF while reading from the proxy, and the in-app language
server restart control could not reconnect. Python support is not verified;
diagnose the proxy-process exit before continuing native LSP checks.

Follow-up in the still-running AHEAD Dev window on the same disposable fixture:
`Cmd+P` filtered to `python/main.py` and Enter opened it. With that editor
focused, `Ctrl+Space` rendered a `double` suggestion through the proxy-backed
completion path, and the Problems view showed zero issues. This confirms the
visible completion flow in this live window, but not the Python server's
readiness or the earlier proxy-EOF failure; inspect Language Servers and
recheck diagnostics/navigation before marking Python LSP complete. Typing
`/skill` filtered the grouped slash catalog to one entry; picking it inserted
`/smoke-skill ` into the composer, and the draft was cleared without sending a
turn. This confirms live skill discovery/filter/insertion, not source ownership,
refresh behavior or narrow-window layout.

## Isolated native fixture check (2026-10-06)

After the earlier UI-broker launch reused the normal profile, this check started
the freshly built app-only `target/debug/ahead` directly with separate
`AHEAD_DATA_HOME`, `AHEAD_USER_HOME` and `AHEAD_HOME` directories under
`/private/tmp`, and opened a fresh copy of `tests/fixtures/editor-smoke/`.
Before opening, the fixture had no `.ahead/`; AHEAD created `.ahead/session.db`
with mode `0600`. The three external profile directories remained empty, so
the AHEAD data/runtime/user homes were isolated from the normal profile. No
provider request or extension download occurred.

Rendered checks: Rust, Python and TypeScript files opened with their language
identifiers; Cmd+P filtered to and opened the TypeScript path; a temporary
Python edit showed the modified dot inline beside the tab filename and Undo
restored the clean state; a breakpoint could be added and removed; and the
embedded PTY returned the fixture path for `pwd`. The source file was not saved
with the temporary edit. This is native UI evidence for these paths only.

The workspace trust prompt was kept blocked because starting language servers
allows project-selected commands to run with user permissions. Thus this did
not verify live completion/diagnostics/navigation or authorize server installs.
The app-only build omitted `ahead-code-mode-host`; the managed Code Mode smoke
cannot be counted until the exact `just dev` package set and pinned V8 helper
are rebuilt. The terminal did read the user's zsh startup file and reported
missing `zoxide` and `starship`; `pwd` still succeeded. Shell startup isolation
remains a separate check.

The disposable project was then initialized as a local Git repository. Clicking
a single-line Python change marker opened an in-editor hunk with removed and
added source lines and a `Copy change diff` control; copying and pasting into a
blank editor line produced the unified hunk text. A second adjacent two-line
change rendered as one continuous gutter rail; its preview showed both removed
and added lines, and Copy pasted the complete unified hunk. Both temporary
pastes were undone, the source was restored, and the disposable Git worktree is
clean. This verifies the rendered adjacent-change appearance and copy flow, not
discontiguous changes within one hunk or the final screenshot guide.

Source Control was also opened in this isolated project. Three temporary long,
nested untracked paths showed responsive ellipses; status markers, individual
stage checkboxes, `Stage All` and `Commit Staged` stayed visible at the pane's
current narrow layout. The temporary files were removed and the panel returned
to `Changes (0)`.

The current Language Servers panel showed the workspace-level “Language servers
blocked” state and explained that servers will not start for this
project. The obsolete `Managed by ahead-proxy` note was absent, so the older
screen capture is stale rather than evidence of current panel state. The `Allow`
action was intentionally not activated; native server startup remains pending
explicit trust and local server availability.

In the local managed chat, typing `/skill` filtered to the fixture's
`/smoke-skill`. Down selected it and Enter inserted `/smoke-skill ` without
sending; Escape closed the picker and kept the draft. The draft was cleared,
the context counter remained `0.0k`, and no provider turn was made. Native
Up-navigation and provider-backed skill execution remain unverified.

## External file updates in open buffers (2026-10-06)

At `../zed` commit `418f89714891f9d8105a3e92e60b9a7a5084d232`,
`crates/project/src/buffer_store.rs::local_worktree_entry_changed`
updates the open buffer's `File`; `crates/language/src/buffer.rs::file_updated`
emits `BufferEvent::ReloadNeeded` when the buffer is clean, and
`crates/project/src/project.rs` reloads that buffer. This is the reference
behavior: route the changed worktree entry to its buffer, reload clean content,
and retain locally edited content rather than replacing it.

AHEAD now carries changed paths on `CoreNotification::WorkspaceFileChange`.
`CodePanel` reads only the affected file on a background executor, compares it
to the saved hash, reloads a clean buffer while rebasing the selection through
unchanged prefix/suffix text, then syncs the editor snapshot and refreshes Git
metadata/anchors. Dirty buffers remain unchanged and are marked for explicit
overwrite review on Save. The focused GPUI tests pass for clean reload/caret
rebasing and dirty-buffer retention; the proxy test confirms the file watcher
delivers the changed path. On 2026-10-06, a rebuilt isolated native app showed
an external edit in a clean open Python buffer. A second disk edit did not
replace the locally dirty buffer; Cmd+S showed the changed-file warning, and
Cancel preserved the local draft and newer disk contents. This check used
only a disposable fixture. In a separate clean Python buffer, inserting a line
above the caret on disk reloaded the file and rebased the caret through its
unchanged suffix. Workspace language-server trust remained blocked, and no
model/provider request was made. Native ignored-file/deletion behavior,
external rename handling and rapid-write races remain unverified.

The proxy also has a distinct open-buffer notification for paths that are not
in workspace search results (for example, ignored generated files). AHEAD
routes that event to a buffer-only subscriber rather than fanning it out as a
workspace change, which would unnecessarily reload Settings and model choices.
A focused GPUI regression verifies that the editor reads current disk contents
even when an event carries an older text payload; a routing regression verifies
that workspace subscribers do not receive the open-buffer-only event. This
path has automated coverage but has not yet been exercised with a native
ignored file.

Zed's `language::Buffer::file_updated` also updates the buffer's file handle
when a worktree entry changes, and emits `ReloadNeeded` only for a clean buffer
whose replacement disk state is Present. AHEAD handles a missing open path
conservatively: it keeps the buffer, distinguishes NotFound from other read
errors, and tells the user that Save recreates the prior path. A GPUI test
deletes a file with local edits, confirms the buffer survives, then cancels the
Save prompt and verifies the path stays absent. This is automated evidence;
native deletion UX is not yet checked. AHEAD does not yet adopt the new path for
external renames. After this state change, all 239 serial `ahead-app --lib`
tests passed with local loopback; the restricted run's only two failures were
mock-server bind denials.

## Session-bound comments and signature-aware completion (2026-10-14)

Compared AHEAD's editor affordances with Zed at
`../zed/crates/editor/src/completions.rs` and
`../zed/crates/editor/src/signature_help.rs`. Zed presents provider completion
metadata alongside ranked suggestions and provides a separate signature-help
popover with the active parameter and its documentation. AHEAD's completion
rows now show LSP-provided signatures/types and source context; its signature
help is positioned above the caret and renders the active parameter's docs.
Editor comments are bound to the active AHEAD session: session changes reload
only that session's comments, clear-session removes gutter actions, and stale
session IDs cannot start or save a draft. The source-matched app runner passed
the three focused GPUI regressions for comment gating, completion details and
signature-help placement/content. Native rendering remains unverified while
the macOS window is unavailable.

## Current automated acceptance (2026-10-15)

The source-current `ahead-app` library suite passes all 336 tests serially.
The sharing fixtures now enter invite JSON in the dedicated join field rather
than the chat composer. Their real dispatcher/TLS threads also require
GPUI's real-I/O parking mode, as used for external-process tests in
`../zed/crates/editor/src/editor_tests.rs` and agent installation tests in
`../zed/crates/project/src/agent_server_store.rs`. No production Git or
sharing behavior was changed to satisfy the deterministic scheduler; the
Git-refresh coalescing/stale-reply tests still run in strict mode.

Task launches use a fresh focused terminal with the recipe name, following
Zed's `TerminalPanel::spawn_task` / `spawn_in_new_terminal` boundary. A real-PTY
GPUI regression verifies repeated launches use distinct terminals, retain the
title through shell updates, and execute the selected hidden justfile even
when its directory contains spaces and an apostrophe.

The integrated binary passes the real Rust, TypeScript/JavaScript and Python
LSP matrix using copied Vtsls/BasedPyright caches and the installed Rust
Analyzer 1.98.1. This includes completion, navigation, diagnostics/repair,
semantic tokens, import fixes, Rust formatting/signatures/references/rename,
unsaved-buffer replay after restart, and graceful shutdown. The retained
fixture is
`/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-AifXLE`.

The local fake-provider attribution smoke also passes with its workspace
under `/private/tmp`: a shell source write is denied and reported as a failed
command, generated Cargo output succeeds, Code Mode applies the patch, and
Turso-backed attribution survives through an AHEAD-authored/human-committed
Git commit and anchor cleanup. Project/user memory append, search, reviewed
replacement, stale-snapshot rejection and Turso reopen pass separately.
All 82 `codex-sandboxing` tests also pass with real Seatbelt available,
including `scratch_write_roots_do_not_reopen_the_active_workspace`; this
closes the earlier unrun unit-regression gap without disabling the sandbox.

These results do not qualify native rendering/bootstrap or authenticated
provider behavior. No OpenRouter key is available in the current environment
or checked AHEAD connection layers, so no new authenticated turn was attempted.
The shared `just dev` process is running again on
`/private/tmp/ahead-broad-e2e.ikS0ar/project`. CoreGraphics lists its on-screen
windows, but accessibility reports none and targeted capture fails. macOS's
permission probes return true for capture/accessibility, while its session
dictionary reports the desktop is locked. Native interaction and screenshot
acceptance require an unlocked desktop; the current capture failure is not
evidence of a new app startup crash.

The subsequent current-source agent/proxy run passes 167 agent tests and
257 proxy tests, with five opt-in tests ignored. Rebuilding caught a missing
test-only ACP presentation-action import and a real policy-layer mismatch:
`TMPDIR` could make a temporary workspace writable even though Seatbelt's
workspace carveout denied the shell effect. The managed profile now adds an
explicit read-only workspace entry beneath the scratch grant, retaining the
deeper generated-output write entries. Regression coverage checks source
denial, generated output, sibling scratch access, read-only mode and canonical
aliases under both the system temp directory and `/tmp`.
The same shared `just dev` loop rebuilt the integrated app with this policy;
the complete `/private/tmp` attribution smoke passed again, including real
Seatbelt source-write denial, generated Cargo output, the Code Mode patch,
Turso CodeAnchor and staged Git commit attribution/cleanup. The watcher was
left running on the same approved disposable project/profile.

The subsequent FIM regression run passes 167 agent tests and 258 proxy tests,
with five opt-in tests ignored. This includes all 22 prediction tests and seven
instruction tests. The actual outbound FIM request after Turso reopen retains
its comment-prefixed runtime plan. Consecutive provider captures verify changed
decisions and explicit session selection, including selecting an older session
after a newer one was created, switching sessions, and editor-only prediction
without another chat's objective or decisions leaking in.

Following Zed's captured-buffer-snapshot boundary in
`../zed/crates/edit_prediction/src/fim.rs` and edit interpolation in
`../zed/crates/edit_prediction_types/src/edit_prediction_types.rs`, AHEAD now
binds inline replies to observed session/configuration and consumed
related-buffer revisions. The editor rechecks these before display and
acceptance and dismisses displayed suggestions when those revisions change.
Only the editor's explicitly selected session is sent; the host's implicit
last-created-session fallback has been removed. Request-scoped RPC callbacks
also remove the old arbitrary pending-file response routing.

Three inline-prediction regressions cover out-of-order replies, selected-session
changes, consumed-buffer edits and close/reopen, provider changes, disconnect,
shutdown, acceptance-time validation and one-step undo. Unrelated unsaved edits
retain the suggestion. Successive matching keystrokes advance the insertion
offset, retaining the remainder instead of dismissing it after one character.
These are automated GPUI checks, not rendered native acceptance.

The inline GPUI regression also passes across 20 scheduler seeds; both adjacent
proxy-client regressions pass in the same focused run. All 21 RPC contract
tests pass. The shared `just dev` process then rebuilt the integrated binary
and was left running on the same approved disposable project/profile. Fresh
local smoke runs pass source-write denial/generated-output writes, Code Mode
patch attribution through staged Git commit cleanup, isolated project/user
memory with Turso reopen, and Code Mode concurrent searches, cancellation,
retry after reopen and shutdown. These use local fake providers, not external
model credentials.

This is not yet the full authoritative `PredictionContextSnapshot` contract.
Instruction/provider file changes are observed after the filesystem debounce,
instructions above a nested opened workspace need watched changes, and stale
UI-result rejection does not abort the blocking provider request. Versioned
route/policy/scope validation, session-summary caching, generated-text provenance,
native acceptance and authenticated-provider checks remain in `TODO.md`.

### Nonblocking conversation refresh and Stop

Zed's `crates/agent_ui/src/conversation_view/thread_view.rs::cancel_generation`
stores an asynchronous cancel task instead of waiting on the foreground thread.
AHEAD now uses asynchronous RPC callbacks for turn start/retry, cancellation
and the active conversation refresh. The 250 ms tick mirrors notification
caches immediately and coalesces durable reads while one refresh is pending.
Slow history reads no longer prevent composer input or Stop from being handled.
An accepted turn remains pending until its stream or terminal message is
observed, including when Stop was pressed before startup finished.

Session-visit generations reject late replies even after leaving a chat and
returning to the same ID. History reads cannot overwrite newer streamed text;
runtime snapshots seed missing caches without replacing newer plan/tool/usage
notifications. Repeated Stop requests are suppressed while cancellation is
pending; malformed and failed cancellation replies leave a retryable control.

The existing gpui-kit `MessageScrollerState` owns scroll anchoring and tail
following. Strict GPUI interaction tests exercise delayed history, composer
typing, scroll-up and return-to-tail, stale history/runtime replies, cleared
plans, Stop/Escape during startup, cancellation errors and late replies after
chat switching. Startup and completion-before-acceptance regressions also pass
across 20 scheduler seeds. These tests use the native component's rendered
test tree and input events, not macOS screenshots or an authenticated model.

The final `cargo test --locked --offline -p ahead-app --lib --
--test-threads=1 --quiet` passes all 341 app tests, including the real-PTY
Justfile task regression: each launch opens a fresh focused terminal with the
recipe title, preserves earlier shells, and uses the declaring justfile.
The final chat filters and late start/retry regression also pass across 20
scheduler seeds. The shared `just dev` loop rebuilt the integrated app in
15.5 seconds and is running on the same approved disposable project/profile.
The rebuilt managed Code Mode smoke passes eight concurrent delegated
searches, cancellation, retry after Turso reopen and shutdown with a local
fake provider. Native visual and authenticated-provider acceptance remain open.

### Optional rename preparation and non-Rust LSP acceptance

Zed's `crates/project/src/lsp_command.rs::PrepareRename` checks that rename is
enabled and uses `response_without_request` when no applicable server advertises
preparation. AHEAD previously treated any `renameProvider` field as support for
both requests, including `false`, and sent unsupported preparation requests.
The existing proxy route now distinguishes these capabilities and returns the
editor's existing default-word behavior for unprepared rename. Advertised
preparation replies remain authoritative; the fallback does not replace them.

A child-process regression exercises six capability forms and confirms actual
rename requests still reach the server. The full source-current proxy suite
passes 259 tests with one opt-in test ignored. Its first direct invocation from
the repository root hit the path-dependent CLI fixture; the complete rerun from
Cargo's normal `ahead-proxy` working directory passes. No test behavior was
changed to hide that invocation error.

The shared `just dev` loop rebuilt the integrated binary after an initial
disk-full link failure. Only two generated proxy test executables from this
work were removed; source, dependency caches and user data were retained.
The loop is running on the same approved disposable project and profile.

Against that rebuilt binary, `tests/lsp-smoke.mjs` passed with existing cached
Vtsls and BasedPyright, without downloading packages or using credentials.
TypeScript, JavaScript and Python declaration renames update unsaved buffers
across files, preserve definition/hover results, pass a typed-error/repair
cycle and leave source files unchanged on disk. Rename preparation and
references are exercised from the call site; the cross-file rename starts at
the declaration because an imported binding may have local-alias semantics.
TypeScript formatting is applied and checked for idempotence; typed signatures,
quick-fix/auto-import, close/reopen, restart with unsaved replay and graceful
process cleanup pass too. Fixture:
`/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-adsDTy`.

The formatting check uses an error-to-clear diagnostic transition rather than
assuming a server repeats unchanged empty diagnostics. These are real
proxy/server checks, not native F2, popup, per-file undo or formatting acceptance.
Installing and routing a Python formatter alongside its analyzer remains open.
The same rebuilt binary also passes the local managed Code Mode smoke:
eight concurrent delegated searches, cancellation, retry after Turso reopen
and shutdown. No external model credentials were used.

### Python analyzer and formatter integration (2026-10-15)

Zed's `assets/settings/default.json` selects BasedPyright plus Ruff for Python,
with Ruff as formatter; `crates/languages/src/python.rs` resolves the Ruff
binary and disables the analyzer's duplicate import organizer. AHEAD now uses
that split through its existing LSP processes, capability routing, diagnostics
and restart path. The catalog retains multiple server IDs per language and
keys pending resolutions by server rather than suppressing a second adapter.
Ruff prefers the project virtualenv, PATH and AHEAD's managed cache, then uses
the same bounded download/staged publication path as Rust Analyzer. Its hover
capability is disabled so an empty Ruff reply cannot replace a typed analyzer
hover, following [Ruff's integration guidance](https://docs.astral.sh/ruff/editors/setup/).
The shared first-success request helper now admits only one successful callback.

The expanded proxy suite passed 265 tests with one opt-in extension test ignored
before the final null-formatting correction. A sandboxed run failed on loopback
and native watcher permissions; the unchanged executable passed with those
permissions from Cargo's package working directory. Only rebuildable test
executables and validated app/proxy incremental directories were removed to
make room for linking; source and user data were retained.

The rebuilt integrated binary downloaded Ruff 0.17.1 into disposable fixture
`/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-E3MyjM`.
Its archive SHA-256 matched the release metadata and its executable passed the
version probe. The real servers published combined Ruff and BasedPyright
diagnostics. The smoke initially expected no diagnostics from a Python snippet
with no import separator; Ruff correctly reported I001. The snippet and cursor
positions are corrected without relaxing the diagnostic assertions.

A direct probe also confirmed that Ruff returns LSP's valid `null` result for
an already-formatted document. AHEAD's formatting route now normalizes that to
an empty edit list, with a regression for null, empty/nonempty edits and errors.
Generated app and bundle outputs were externally removed during the first
smoke, with free disk rising from roughly 0.5 to 44 GiB. The single `just dev`
loop completed its clean rebuild in 16m 25s and started the previously approved
disposable project. The integrated Python smoke then passed, followed by the
combined TypeScript/JavaScript/Python smoke against that rebuilt binary:
Ruff formatting and idempotence, combined diagnostics, typed analyzer hover,
unsaved cross-file rename, close/reopen, fresh processes after restart and
graceful shutdown. The combined fixture is
`/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-dmACY3`.
No downloads or credentials were needed for these reruns; the acquired servers
were reused from disposable caches. After stopping the watcher, the one-off
proxy test build completed in 3m 45s. The null-formatting regression passes,
as does the full source-current proxy suite: 266 passed, zero failed, one opt-in
test ignored (66.75s with local networking and native watcher access).
Scoped rustfmt and whitespace checks pass. The user guides now describe the
Python analyzer/formatter split, Ruff acquisition and the remaining limits.
The same `just dev` command rebuilt in 16.17s after testing and is running on
the previously approved disposable project/profile; its proxy entered the
main loop. This confirms startup, not rendered UI acceptance.
TODO retains native acceptance, multi-server code-action aggregation, explicit
overlapping-server selection and watched Ruff configuration reload.

### Language-server watched-file registrations (2026-10-15)

Reference: Zed's `crates/project/src/lsp_store.rs::register_watcher`,
`worktree_and_path_for_file_watcher`, unregister handling and file-event
delivery. AHEAD uses the same ownership model: server ID plus registration
ID, workspace-scoped unrooted globs, explicit relative-pattern bases and
cleanup when a server unregisters, stops or restarts. The existing native
watcher filters paths before sending events to the catalog, shares roots
between registrations and preserves other watcher consumers. Registrations
honor create/change/delete kinds and translate rename events. Invalid globs,
non-file bases, traversal outside a relative base and recursive filesystem-root
watches are rejected.

The first real Ruff probe caught a macOS URI-identity bug: native events used
`/private/var/...`, while the server's workspace URI used `/var/...`. Matching
now uses canonical paths but notifications preserve the workspace's original
URI spelling, or an external relative pattern's declared base URI. A regression
covers both aliases, including deleted files. This matches the workspace lookup
in [Ruff 0.17.1's session index](https://github.com/astral-sh/ruff/blob/0.17.1/crates/ruff_server/src/session/index.rs).
Watcher backend errors are logged without ending subsequent event delivery.

The source-current proxy suite passes 279 tests, zero failed, one opt-in
extension test ignored. Coverage includes owner isolation, glob/kind filtering,
rename mapping, native external-root delivery, unrelated-path filtering,
unregister/stop cleanup, recursive-watch sharing and URI aliases. The shared
`just dev` loop rebuilt in 17.02s and its proxy entered the main loop.

The real Python smoke passes with cached BasedPyright and Ruff 0.17.1, followed
by the combined Rust/TypeScript/JavaScript/Python smoke. Creating, editing,
renaming and deleting `pyproject.toml`/`ruff.toml` changes actual formatting
without replacing Ruff's process. These checks repeat after an explicit
server restart; formatting, combined diagnostics, code actions, navigation,
unsaved-buffer recovery and graceful shutdown still pass. No model credentials
or downloads were needed. Retained combined fixture:
`/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-Bf8GhZ`.
This is proxy/server acceptance, not rendered native UI acceptance. Native
code-action selection/undo and explicit overlapping-server selection remain
in `TODO.md`.

### Multi-server code-action ownership (2026-10-15)

Zed's `crates/project/src/lsp_store.rs::code_actions` and
`request_filtered_lsp_locally` collect responses from multiple servers;
`crates/project/src/project.rs::CodeAction` keeps the producing server ID.
AHEAD now follows that ownership pattern through its existing catalog,
RPC and client. `LspCodeAction` is the single shared shape rather than an app
wrapper around a response-wide server ID. Empty/null replies cannot win a
race and suppress another server's actions. A partial failure retains healthy
actions; total failure reaches the caller. Replies have stable owner ordering,
and same-title actions from different servers are not incorrectly deduplicated.
Code actions use a four-second server deadline so aggregation can finish
inside the editor's five-second wait. Other requests retain their existing
deadline; expired requests send cancellation and ignore late replies.

Four focused proxy regressions pass, followed by the full source-current
proxy suite: 270 passed, zero failed, one opt-in ignored. The mounted editor
regression passed 20 scheduler seeds, selecting the second server's lazy
action, routing resolve and its command to that owner, and undoing the edit in
one step. The full app suite passes all 341 tests. The shared `just dev` loop
rebuilt the integrated binary in 35.32s on the same approved disposable
project/profile and its proxy entered the main loop.

The real proxy smoke passed with cached Vtsls, BasedPyright and Ruff, then
passed with all three language stacks using Zed's existing versioned Rust
Analyzer binary too. No downloads or model credentials were needed. Fixture:
`/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-NbF2BB`.
The Python fixture explicitly enables BasedPyright's unused-import warning
to exercise both servers' actions for the same range, before and after restart.
Ruff returned an inline edit; its application clears both diagnostics without
saving the buffer. This probe does not exercise Ruff's optional resolve path;
the mounted test covers lazy resolve/command ownership. TypeScript's real
import fix retains Vtsls ownership for its advertised command. Native menu,
keyboard selection and undo acceptance remain open, as do explicit
overlapping-server selection and watched Ruff configuration reload.

### Ordered language-server routing (2026-10-15)

Zed's `crates/project/src/lsp_store.rs::request_lsp` queries the first
applicable server in adapter order rather than racing replies.
`crates/language/src/language_settings.rs::language_servers` defines explicit
server order, `!id` exclusions and `...` expansion. AHEAD now follows those
rules through its existing catalog and server queues. Capability and document
checks happen before sending the request; valid null/empty replies and server
errors do not trigger another server. Optional rename preparation keeps its
existing fallback. Code actions still aggregate enabled servers, and explicit
owner IDs keep completion/action resolve and commands on their producing server.

The shared settings layers now accept `[languages.<language>]` server lists
and `[languages.<language>.requests]` method-to-server choices. A disabled
feature target returns an error instead of falling back. Routing changes use
the existing restart path with the latest unsaved documents; malformed settings
leave the last valid routing running and report an error. Built-in Python no
longer suppresses Ruff's advertised hover capability: analyzer-first defaults
and explicit routes decide which server answers it.

The first real routing probe exposed another macOS alias mismatch, this time
in workspace settings detection rather than LSP watched-file URI delivery.
The notifier now normalizes its comparison root and recognizes settings paths
through aliases even after file deletion, without rewriting editor or LSP URIs.
The real native watcher regression and the existing private-provider/search
regression both pass. The full source-current proxy suite passes 285 tests,
zero failed, one opt-in ignored (74.59s). Focused rustfmt, Node syntax and
whitespace checks pass. No dependency was added.

The same shared `just dev` loop rebuilt the integrated binary in 12.87s on
the previously approved disposable project/profile and its proxy entered the
main loop. The final Rust/TypeScript/JavaScript/Python smoke passes with cached
servers: completion, typed signatures, diagnostics, formatting, rename,
multi-server actions, watched Ruff configuration and restart. Native settings
events also reorder Python servers, pin hover/formatting, disable/re-enable
Ruff and replay unsaved buffers without saving source files. Retained fixture:
`/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-HStpx4`.
No downloads or model credentials were used. These are proxy/server checks,
not rendered popup, keyboard-selection or undo acceptance. Those native gates
remain in `TODO.md`; the user guide documents the TOML configuration path.

### Concurrent Turso startup and failed-commit recovery (2026-10-15)

Zed's `crates/sqlez/src/connection.rs::open_with_flags` sets a five-second
busy timeout because multiple editor instances share agent-thread storage.
Reference checkout: `418f89714891f9d8105a3e92e60b9a7a5084d232`.
AHEAD now applies that bounded wait to its existing Turso connection before
initial schema creation. No schema version, migration or journal-mode change
was added.

A four-process first-open test initially failed with `database is locked`.
Adding the timeout exposed a second failure: history appends read their next
ordinal in a deferred transaction, then attempted a lock upgrade. They now
reserve the writer before reading. Ten consecutive four-process runs preserve
all eight history items and eight metadata patches per worker after reopening.

Held-reader regressions then reproduced failed commits leaving the connection
inside a transaction. The remaining eight manual session-write transactions
now use Turso's existing transaction guard. Tests force a commit timeout for
history, metadata, titles, workflow, thread deletion, recent files, memory
indexing and saved turn requests. Prior data survives, the connection returns
to autocommit, and a retry on that same connection persists after reopen.
The tests shorten only their own timeout; production waits up to five seconds
and still reports persistent contention rather than retrying indefinitely.

All 11 focused storage tests pass. The full source-current proxy suite passes
289 tests, zero failed, one opt-in ignored (88.14s). The shared `just dev`
loop rebuilt the integrated binary in 17.80s and its proxy entered the main
loop on the same approved disposable project/profile. Real proxy probes pass
unsaved-buffer crash recovery, project/user memory isolation and reviewed
replacement, plus managed Code Mode with concurrent search, cancellation and
retry after Turso reopen. Explicit shutdown and stdin EOF retain the cancelled
turn and partial answer; SIGKILL retains the answer and instruction provenance
without automatically replaying the turn. An explicit retry starts once.
These use disposable data and a local fake provider,
not model credentials or native UI automation. Recovery fixture retained at
`/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-recovery-smoke-ryw9cP`.

macOS and Windows CI now run the storage filter explicitly; Linux includes it
in the workspace library suite. Removed CI steps referenced the deleted
`ahead-app/portable` feature and the deleted legacy-rollout import tests.
The YAML parses, and scoped rustfmt and whitespace checks pass. Linux/Windows
runtime checks, Windows ACL acceptance and actionable native contention errors
remain unverified; they are still in `TODO.md`.

### Curated ACP cache repair (2026-10-15)

Zed's `crates/project/src/agent_server_store.rs::LocalRegistryNpxAgent::get_command`
runs npm installation before resolving the package executable. Reference
checkout: `418f89714891f9d8105a3e92e60b9a7a5084d232`.
The [live ACP registry](https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json)
still supplies npm distributions for Pi, Codex and Claude Code; AHEAD's
curated catalog remains unchanged. Platform archives and update-status UI
remain separate work, not a reason to expose arbitrary registry agents.

A new regression reproduced an install marker referring to a missing package
generation. AHEAD returned `No such file or directory` without attempting a
repair. Missing generations, package metadata, executables and lockfiles now
reuse the existing install lock, private staging directory and atomic manifest
publication. A failed repair preserves the manifest and earlier leased
generations. Malformed manifests, executable path escapes and other I/O errors
still fail closed; no user cache is swept or migrated. The docs now describe
the actual lock serialization rather than claiming cached launches bypass it.

Windows npm selection now uses `npm.cmd` explicitly through Rust's command
API; no shell command string is assembled. A Windows-only regression creates
a shim in a disposable PATH containing spaces and an ampersand. The `npx_`
cache filter now runs explicitly in macOS/Windows CI; Windows execution is not
verified on this host.

All 18 non-opt-in adapter tests pass, followed by the full source-current
agent suite: 169 passed, zero failed, four opt-in ignored (31.00s). The extended
offline npm smoke passes with Node 24.15.0 and npm 11.12.1: it packs and installs
two disposable local packages, deletes the current executable, repairs it from
the local tarball, and runs both repaired and earlier leased versions. Its
private npm config/cache, offline mode and disabled lifecycle scripts avoid
registry/provider access. Scoped rustfmt, YAML parsing and whitespace checks
pass. The same shared `just dev` loop rebuilt the integrated binary in 21.01s
on the approved disposable project/profile. Native install/reconnect and
authenticated external-agent turns remain open in `TODO.md`.

### Live curated-agent registry and model options (2026-10-15)

Rechecked Zed's `LocalRegistryNpxAgent::get_command` in
`crates/project/src/agent_server_store.rs` and option defaults in
`crates/agent_servers/src/acp.rs` at
`418f89714891f9d8105a3e92e60b9a7a5084d232`. The installer resolves the registry
package before launching its executable; advertised session options remain
the source of truth for model selection.

AHEAD's opt-in `curated_acp_session_model_options_offline` test now waits for
the production background registry refresh, loads its cache and verifies
installed package provenance against the live entries. It no longer treats
failed Codex authentication or auth-required session creation as a successful
model check. Per-agent diagnostics report initialization and option results;
saved defaults are checked on disk too.

The final-source run passed in 32.07s. Pi `pi-acp@0.0.34`, Codex
`@agentclientprotocol/codex-acp@2.2.2` and Claude
`@agentclientprotocol/claude-agent-acp@0.91.0` all initialized, created sessions,
changed an advertised model option and saved the selection. Pi exposed two
model choices from its synthetic catalog, Codex eight and Claude five.
Claude's fallback package now matches that tested 0.91.0 version. The run used
temporary projects/homes/package caches, isolated npm configuration, disabled
lifecycle scripts and synthetic keys. No model prompt was sent. Registry/npm
access is required despite the test's historical name; the build guide now
documents that distinction and the isolated command.

All 169 non-opt-in agent library tests pass on the final source (four ignored,
34.52s). The focused adapter suite passes 18 tests. Scoped rustfmt and
whitespace checks pass. The shared `just dev` loop rebuilt the integrated
binary in 14.87s on the same approved disposable project/profile; its proxy
entered the main loop. The watcher remains running.

This closes the protocol-level live-registry consumption check, not native
install/reconnect, picker rendering, real provider authentication or model
turns. Those remain in `TODO.md`.

### FIM request cancellation and host context checks (2026-10-15)

Rechecked Zed's `EditPredictionStore::cancel_pending_prediction` and
`PendingPrediction::drop_on_cancel` in
`crates/edit_prediction/src/edit_prediction.rs` at
`418f89714891f9d8105a3e92e60b9a7a5084d232`. For cancellable predictions, Zed
drops the task and its HTTP future rather than only hiding the eventual result.

AHEAD's inline fallback previously held the session-host read lock throughout
a blocking provider call inside the LSP response callback. It now captures an
owned context snapshot, releases the lock and runs inference on a worker.
Before returning, the host rereads the selected session/work state, provider
configuration and applicable instructions. A changed snapshot rejects the
result. This check does not yet bind a versioned snapshot hash to editor
acceptance.

The three HTTP paths use the existing async reqwest dependency with a
cancellation signal. Active or consumed-buffer edits/closure, a newer request
for the same file, workspace replacement and shutdown drop the request or
response-body future. Unrelated edits and identical buffer snapshots leave the
request alone. A cancelled late LSP reply cannot launch new provider work.
Cursor-only movement and session/mode/provider changes still need immediate
cancellation; UI stale-result rejection remains in place for those changes.

All 32 prediction-filter tests pass on the final source (0.19s; incremental
build 19.17s). Six loopback cases stall generic OpenAI-compatible, Qwen FIM
and Ollama calls before headers or during the response body, then verify
prompt cancellation and socket closure before the five-second timeout.
Dispatcher tests verify host-lock release and consumed-buffer/late-reply
cancellation. Host tests change instructions, provider model or a durable
decision after receiving the outbound prompt and verify rejection of the old
completion. No external model or credentials were used. Native caret/undo,
generated-text provenance, latency and authenticated-provider acceptance remain
in `TODO.md`; the specification now distinguishes these gaps from the checked
host and transport behavior.

The rebuilt proxy library suite passes 293 tests, with one opt-in test ignored
(69.06s). Scoped rustfmt and whitespace checks pass.
The same shared `just dev` loop rebuilt the integrated binary in 22.27s on
the approved disposable project/profile; its proxy entered the main loop.
The watcher remains running. This is build/process evidence, not a native
visual acceptance pass.

### Editor-owned prediction cancellation (2026-10-15)

The preceding transport work left cursor-only movement and route changes
without an explicit cancellation path. Rechecked Zed's task ownership in
`EditPredictionStore::cancel_pending_prediction` at
`418f89714891f9d8105a3e92e60b9a7a5084d232`: dropping the editor's pending
prediction also drops its provider future.

AHEAD now stores the pending GPUI task instead of detaching it. Its request
guard owns the RPC ID. Dropping that guard retires the local callback and sends
`CancelInlineCompletions` for that ID; the proxy cancels only the matching job.
Repeated cancellation and cancellation after completion are no-ops. An old
task cannot cancel its replacement, including another request for the same
file.

Caret/selection changes, selected-session switches, observed session/mode or
provider invalidation, Escape and existing editor request-generation changes
drop the pending task. Unrelated session/buffer notifications leave it alone.
The shared request-invalidation method also covers file reload/error and
completion-acceptance transitions. Displayed ghost text clears on selection
movement; final acceptance synchronously checks the caret and empty selection
before editor observers run. Matching typed prefixes still retain the remaining
suggestion.

The focused final-source run passes all four app, two proxy and one RPC
inline-prediction tests. The proxy's loopback regression now checks both buffer
invalidation and explicit cancellation of a stalled HTTP request. The complete
proxy suite passes 293 tests, with one opt-in ignored (76.99s), and all 22 RPC
tests pass. These are disposable local-provider and headless GPUI checks, not
native visual or authenticated-provider acceptance. Debounced instruction and
provider file notifications, ancestor-instruction watching, versioned snapshot
binding, summary caching and generated-text provenance remain in `TODO.md`.

Both GPUI inline-prediction regressions also pass across 20 scheduler seeds
(`ITERATIONS=20`; the four-test app filter finishes in 3.33s). Scoped rustfmt
and whitespace checks pass.
All 342 app library tests pass on the final source (53.93s), including real-PTY
task behavior and existing completion insertion/interpolation/undo checks.
The shared `just dev` loop rebuilt the integrated binary in 25.87s using the
same approved disposable project/profile. Its proxy entered the main loop;
the watcher remains running. Native rendering is not verified by that startup.

### Snapshot-bound prediction acceptance (2026-10-15)

Rechecked `interpolate_edits` in Zed's
`crates/edit_prediction_types/src/edit_prediction_types.rs` at
`418f89714891f9d8105a3e92e60b9a7a5084d232`. It compares old/new buffer
snapshots and retains a predicted insertion only when intervening human edits
match its prefix. AHEAD follows that behavior for insertion suggestions and
adds host-owned session, route, policy and instruction checks.

Managed FIM replies now carry a version-1 binding through the proxy response,
displayed suggestion and acceptance request. The binding contains a request
identity, snapshot digest and target revision. The host retains the private
snapshot, consumed-buffer identities/revisions and the original target's
shared rope. There is one cached entry per path, capped at 32 entries with a
two-minute lifetime; retained prefix/suffix strings use bounded inference
excerpts. API keys are excluded from serialization, while private snapshot
equality still rejects a changed credential.

Tab sends asynchronous validation before insertion. The host checks the
binding, full live target hash, caret, exact remaining suggestion, related
buffers and a fresh session/provider/instruction snapshot. Matching typed
prefixes remain valid; unrelated edits do not invalidate the suggestion.
Validation consumes the binding once and does not edit or write attribution.
After the reply, the editor rechecks its generation and local context before
inserting. Repeated Tab cannot submit duplicate acceptance requests. Ordinary
LSP inline replies keep their existing local acceptance path.

The new session-archive regression first failed: archiving removes a session
from the active list but does not change its stored `SessionView`. FIM
preparation now also queries the existing Turso archive table and rejects
archived context. No schema or migration was added.

The final-source focused run passes five app, 36 proxy and one RPC tests
(44.79s incremental build). New coverage includes changed on-disk instructions,
provider/credential changes without notifications, archive state, reopened
buffers, forged replacement text, expired/replayed bindings and large Unicode
targets using bounded excerpts. A real loopback provider verifies the actual
outbound instructions, bound reply and serialized acceptance RPC. Headless
GPUI checks delay validation replies while typing, moving the caret, switching
session/provider or disconnecting; valid insertion remains one undo action.
The five-test app filter also passes across 20 scheduler seeds (14.45s).

All 343 app library tests and all 22 RPC tests pass. The app suite includes the
real-PTY Justfile regression: fresh focused terminals retain the task name and
run the selected hidden justfile without replacing earlier shells. These are
automated tests, not native visual or authenticated-provider acceptance.
The full proxy suite passes 297 tests, with one opt-in test ignored (77.38s).
Its direct runner must use the `ahead-proxy` package directory, as Cargo does;
an earlier repository-root run failed only the existing `../Cargo.toml`
relative-path fixture. No CLI code was changed for that invocation error.
Summary caching, context budgets, ancestor-instruction watching, debounced UI
invalidation, generated-text provenance and native latency/placement remain in
`TODO.md`. Host validation and editor insertion are not one atomic transaction.

Scoped rustfmt and whitespace checks pass. The same shared `just dev` loop
rebuilt the integrated binary in 44.41s on the approved disposable
project/profile, and its proxy entered the main loop. The watcher remains
running. This confirms build/startup, not native rendering.

### Instruction-watch invalidation (2026-10-15)

Rechecked Zed's instruction watcher in
`crates/agent_settings/src/user_agents_md.rs` and `watch_config_file` in
`crates/settings/src/settings_file.rs` at
`418f89714891f9d8105a3e92e60b9a7a5084d232`. Instruction changes have their
own notification path. AHEAD uses that separation with standard project
`AGENTS.md` files; this does not add Zed-specific file locations or aliases.

The proxy now registers filtered, non-recursive directory watches for
instructions above a nested workspace, stopping at the same repository root
as the loader. The existing `ahead-agent` root discovery is shared rather than
duplicated. Directory watches handle files absent at startup and atomic
replacement without following an old file handle. Instruction events and
workspace/user provider settings invalidate predictions immediately. Git,
search and language-server configuration refreshes remain debounced;
instruction changes do not restart language servers.

Workspace refreshes no longer advance the prediction configuration revision
for every disk change. They invalidate affected open-buffer revisions only,
leaving suggestions that consumed unrelated buffers alone. A pathless rescan
still invalidates conservatively. The separate immediate provider signal also
avoids invalidating a new suggestion merely because the delayed settings
refresh arrives later.

Native watcher regressions create, atomically replace and delete ancestor
instruction files in disposable nested workspaces, including a `.git` worktree
marker file. Synthetic event checks cover immediate delivery, search-ignored
instructions, rename events and exclusion of unrelated ancestors/subtrees and
non-standard instruction filenames. The client regression covers consumed
versus unrelated buffers, older events, rescans and a delayed provider refresh.
The existing GPUI prediction test now routes the watcher notification through
the core channel and verifies displayed ghost text clears; unrelated workspace
notifications preserve it.

The final instruction filter passes seven agent and four proxy tests (43.55s
incremental build). The prediction filter passes six app, 36 proxy and one RPC
tests. These are automated filesystem/protocol/GPUI checks, not a native visual
or authenticated-provider pass. Ancestor subscriptions are fixed at workspace
initialization; refreshing them when a repository boundary changes remains in
`TODO.md`, along with summary budgets, attribution and native acceptance.

The final-source full library suites pass: app 344 (86.16s), proxy 299 with one
opt-in ignored (98.32s), agent 169 with four opt-in ignored (37.72s), and RPC 22.
The direct proxy and agent runners used their package directories. Scoped
rustfmt and whitespace checks pass. These suites use disposable projects,
local mock providers and real PTYs; they do not qualify an external model.
The six-test prediction filter also passes across 20 GPUI scheduler seeds
(`ITERATIONS=20`, 14.71s), without retries.
The shared `just dev` loop rebuilt the integrated binary in 17.67s on the same
approved disposable project/profile; its proxy entered the main loop. The
watcher remains running. Native rendering is still unverified.

### FIM session-context caching and size guards (2026-10-15)

Rechecked `crates/edit_prediction/src/fim.rs` and
`crates/zeta_prompt/src/zeta_prompt.rs::format_prompt_with_budget_for_format`
in the same Zed checkout, `418f89714891f9d8105a3e92e60b9a7a5084d232`.
Zed bounds the active excerpt and budgets formatted context. AHEAD retains
that separation while adding required session and standard instruction text;
it does not copy Zed's hosted prediction service or invent a replacement loop.

The host now caches compact durable work text for at most eight sessions,
64 KiB per entry. Keys include a projection-format version and a Turso source
revision; the private prediction snapshot includes that version. Triggers on
the five consumed work/runtime/conversation tables cover insert, update and
delete, including another connection's writes. Revision changes roll back
with their transaction. Unrelated sessions and editor recovery/recent-file
writes do not invalidate the selected session's text. Workspace replacement
clears the cache, and history expiry removes its durable version row.

Cold assembly fetches only the latest four summaries, leaving full history
and exports intact. It checks stored text lengths as rows stream from Turso,
then decodes only the fields that enter the prompt; plan JSON is selected
without loading unrelated runtime warnings or commands. All explicitly tagged
decision/invariant/constraint events survive newer notes; the current plan
stays complete. Existing human-only audience checks suppress summaries after
a private message invalidates a previously warm cache.
FIM formatting no longer drops invariants after the third one.

The input guard covers session text, instructions, related excerpts and their
metadata together at 64 KiB. Both generic and FIM routes also guard the final
formatted prompt at 128 KiB, including line-comment expansion. Over-budget
required context fails before HTTP dispatch without silent truncation or
provider fallback. These are byte guards, not model-specific token limits;
faithful compaction and model-aware budgets remain in `TODO.md`.

The final cache, byte-budget and Turso row-reader source passed the combined
app/proxy/agent/RPC prediction filter (50 tests; the agent crate had no matching
tests). The shared target directory had been removed before the clean 11m46s
build; the dev watcher was stopped and no competing AHEAD Cargo build was
running during verification. Native and authenticated-provider acceptance
remain open.

Afterward the shared `just dev` loop rebuilt the integrated app in 2m30s. The
link completed with the existing macOS compact-unwind warning, then the runtime
reported `AHEAD proxy child entering mainloop`. The watcher remains active on
the approved disposable multi-language project. This confirms build and process
startup, not rendered-window or feature-interaction acceptance.

The follow-up provider-selection regression passes 1/1: absent
`ai.active_connection`, FIM now follows configured connection order rather than
sorting provider IDs. Restarting the watcher after this test first failed with a
missing `unicode_ident` `.rmeta`; `target/` was absent at inspection. A
lock-guarded clean rebuild then passed in 11m17s and again reported the proxy
entering its main loop. The current watcher is active on the same disposable
project. Native rendering remains unverified.

The current integrated `target/debug/ahead` passed
`tests/agent-shutdown-smoke.mjs`: explicit shutdown, stdin EOF and SIGKILL each
preserved partial streamed text and Turso history across reopen; a crashed turn
was not replayed until explicit retry. This used only the loopback fake model.
The Rust-only `tests/lsp-smoke.mjs` also passed with Zed's cached Rust Analyzer
provided explicitly because the PATH rustup shim lacks that component. It
covered formatting, completion, definition, semantic tokens, hover, signature
help, references, cross-file rename, unsaved diagnostics/repair, close/reopen,
restart and graceful server shutdown. The disposable workspace is
`/var/folders/3n/wsjx_vr53cb7v2g6qk51dc2h0000gp/T/ahead-lsp-smoke-NImXD4`.
These are proxy/protocol checks, not rendered editor or authenticated-provider
acceptance. The TypeScript/JavaScript/Python matrix was not repeated in this
pass; those languages last passed against the Oct 14 integrated binary.

The same current proxy binary passed `tests/agent-memory-smoke.mjs` for
project/user-scoped append, ranked search, reviewed replacement, stale-snapshot
rejection and Turso reopen. The source-current app test runner also passed six
focused GPUI regressions: Just task opens a fresh task-named terminal, code
comments require an active session, completion detail shows signatures, fuzzy
completion ranking respects the five-item cap/server order, the model picker
searches model/provider names, and model visibility stays scoped under
collapsible providers. These are headless interaction checks; native rendering
and OS input remain unverified.

On 2026-10-15, the integrated binary built after the managed MCP source changes
passed `tests/agent-mcp-smoke.mjs` with a loopback model. The modern-only stdio
fixture rejects legacy initialization and requires the 2026-07-28 discovery
envelope and metadata. The smoke approved the tracked declaration, observed
and answered a separate prompt for each operation, and verified one
`tools/call`, `resources/list`, `resources/templates/list` and `resources/read`
reached the server only after approval. Tool and resource results returned to
the model, which completed the turn; shutdown was clean. A second phase made
the `--form` fixture reject modern discovery, then confirmed RMCP's legacy
initialization fallback and an approved tool call. No external model or MCP
server was contacted. In-flight cancellation and the rendered approval UI
remain open.

Revalidated on 2026-10-16 against the current integrated binary with access
limited to the smoke's disposable loopback model. Modern discovery/resources,
stdio tool execution, per-call approval, model continuation, and legacy
initialization fallback passed again. This still does not exercise in-flight
cancellation or native approval rendering.

## Model-specific reasoning controls (2026-10-19)

Zed's `crates/agent_ui/src/conversation_view/thread_view.rs` builds the
reasoning menu from the selected model's supported effort levels, restores the
selected/default level, and saves model-specific choices through its agent
settings. AHEAD now queries the retained model manager's
`ModelInfo::supported_reasoning_levels` through the managed session host,
renders only those choices, and stores a user's override by provider/model in
`~/.ahead/settings.toml`. Unsupported saved values resolve to the model default.
The corresponding app and settings tests were added but not run in this pass;
the user will verify the rebuilt native behavior.

## ACP Registry binary distributions (2026-10-20)

Followed Zed's `AgentRegistryStore` platform selection and
`LocalRegistryArchiveAgent` install lifecycle while keeping AHEAD's Pi, Codex
and Claude Code catalog curated. AHEAD now prefers a checksummed binary target
for the current platform, falls back to the registry Npx package, and reports
registry/installed-version differences in the picker. Binary installs verify
HTTPS archive SHA-256 before extracting ZIP, gzip-tar, bzip2-tar or raw files;
unsafe paths, links/special files and oversized archives are rejected. Installs
use AHEAD's locked, atomically published, leased generations. The official
schema makes the checksum optional, but AHEAD requires it before executing a
binary distribution. Focused tests were added but not run; native app and
Windows installation behavior remain unverified. References:
[Zed registry store](https://github.com/zed-industries/zed/blob/main/crates/project/src/agent_registry_store.rs),
[Zed binary installer](https://github.com/zed-industries/zed/blob/main/crates/project/src/agent_server_store.rs),
[ACP registry schema](https://github.com/agentclientprotocol/registry/blob/main/agent.schema.json).

## Managed npm language-server updates (2026-10-20)

Compared against Zed commit
`418f89714891f9d8105a3e92e60b9a7a5084d232`. Its
`crates/node_runtime/src/node_runtime.rs` resolves the latest npm package
version and compares it with the installed version using
`should_install_npm_package`; `crates/languages/src/vtsls.rs` and
`crates/languages/src/python.rs` update only when their cached package is
behind. AHEAD follows that lifecycle for its
managed Vtsls and BasedPyright packages: on first use in a catalog session, a
background `npm view` checks the configured registry's `latest` tag. A newer
semver installs through AHEAD's existing staged/locked cache publisher, and
the running servers restart only after publication. An offline or failed check
leaves the cached server working. A check uses an npm/Node runtime already on
PATH or in AHEAD's cache, so it does not download Node merely for version
discovery. System-installed servers remain the user's package-manager concern.
The Extensions tab surfaces installation progress, and failed managed servers
offer a scoped cache repair action. The version parser, bounded subprocess
output, and fake-npm lookup tests are source coverage only until run.
