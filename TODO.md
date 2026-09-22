# TODO

Incomplete work for AHEAD: unfinished features, stubs, mocks, and known gaps.
This is the single source of truth. When you finish an item and commit it,
delete it here in the same commit. Do not add a "done" section.

Anything knowingly shipped as a stub or mock must have an entry here in the
commit that introduces it. This workflow lasts until the editor is usable end
to end; after that we dogfood AHEAD to develop AHEAD and the backlog moves into
the app itself.

## Zed source reuse and editor parity

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
      replacing the current single-file `CodePanel`.
- [ ] Add LSP rename, find references, formatting and code actions through the
      existing proxy/RPC boundary; keep symbol outline explicitly deferred.
- [ ] Port Zed's project search panel with search/replace, regex, case/word
      filters, result navigation and unsaved-buffer behavior.
- [ ] Add an optional watch/restart action for parsed just recipes using
      watchexec's supervisor/process-group and clear-screen support; direct
      recipe runs should remain the default.
- [ ] Port a Git source-control/review view with changed files, diffs,
      stage/unstage, commit, branches and conflicts; preserve AHEAD anchors,
      attribution and session trailers.
- [ ] Port Zed's high-fidelity DAP debugger surface: sessions, breakpoints,
      threads, stack frames, scopes, variables, stepping, source mapping and
      verified state. Carry upstream tests and add AHEAD policy/session tests.
- [ ] Extend the PTY terminal to multiple named tabs, resize/reflow and
      lifecycle-safe sessions. Allow selected terminal output to attach to the
      AHEAD agent panel as context.
- [ ] Polish the AHEAD agent panel with streamed Markdown/code rendering,
      follow-tail, cancellation, durable reopen/resume and terminal context.
- [ ] Complete the single-human-plus-agent full-duplex voice journey. Defer
      multi-human agent threads until the single-user flow is reliable.
- [ ] Keep all new editor chrome on GPUI/gpui-kit theme tokens and use familiar
      VS Code keybindings where they do not conflict with AHEAD actions.
- [ ] Defer remote/Kubernetes development, broad customization/profiles,
      arbitrary VS Code extension hosting and symbol outline until the core
      editor parity loop is working.

## Dependencies (Lapce-org removal; see decision 0006)

- [ ] Decide the extension host (Zed WIT component model on wasmtime 48 vs
      deferred). `wasmtime 14` is already out of the graph; the host decision
      picks the new runtime (Zed uses wasmtime 48) and the guest ABI. It also
      covers the deferred Zed extension install/update/runtime boundary. Direct
      settings-configured LSP and DAP startup is already the MVP path.

## Attribution & change tracking

- [ ] Managed Codex file-change completion now records `ahead` anchors from
      applied diffs; a real managed App Server acceptance test now verifies the
      native file write, `FileChange` event, actor attribution and quote. Still
      exercise the rendered gutter and proxy `GitCommit` cleanup together.
      External ACP remains explicitly unable to provide this guarantee.
- [ ] `anchors.created_at_commit` is never populated (clear-on-commit deletes the
      rows instead). Decide whether to keep the column or drop it.
- [ ] Verify the existing proxy `GitCommit` route against a real user-driven
      commit with live anchors; its git2 path is now unit-tested to set
      `author = ahead`, preserve the human committer, include the session
      trailer, and stage renamed destinations correctly. The dispatcher route
      now also has an end-to-end regression test for anchor attribution and
      cleanup; the rendered user-driven commit journey remains.

## Agent chat panel (`ahead-app/src/session_panel.rs`)

- [ ] Settings now supplies the selected model and provider id to managed Codex
      thread start/resume. Expand the single configured-model entry into a real
      provider catalog and connect base URL/credential selection; external ACP
      remains adapter-default.
- [ ] `AheadAgentLoop` in `ahead-proxy/src/ahead/agent.rs` is retained as the
      deterministic `AgentTurn` fallback. The real streamed conversation now runs
      through `ahead-harness/src/acp_client.rs` + `ahead-harness/src/session.rs` and `session_panel.rs` calls
      `AgentTurnStart`/`AgentTurnCancel`. Remaining fidelity work: verify native
      Codex tool contracts on multi-tool turns, compaction, unsaved-buffer reads,
      and model-specific tool selection against the pinned runtime.
- [ ] Durable messages are now persisted (`conversation_messages` table) and
      streamed into the panel. The panel renders headings, bullets and fenced
      code blocks and opens validated HTTP(S) inline links; full Markdown
      semantics (emphasis, ordered lists, blockquotes, tables) and rendered
      link behavior still need a UI pass.
- [ ] Conversation history now uses a tracked GPUI `ScrollHandle` with an
      explicit Latest/Follow control and auto-follow during streamed updates;
      verify the control, manual scrolling, and jump-to-latest behavior in a
      running GPUI window.
- [ ] Enter-to-send is wired via `subscribe_in` on `InputEvent::PressEnter` but
      unverified in a running app. Confirm Shift+Enter inserts a newline.
- [ ] The `session_panel` -> proxy turn path is compile-verified only; the
      streamed turn, stop button and reopen have not been driven in a running
      GPUI window.

## Harness integration (new 2026-09-18)

- [ ] Bundle and load the AHEAD-owned skills under `ahead-harness/skills/`:
      expose only each skill's `name`/`description` initially, load the body and
      references on selection, pin the skill revision in task/session records,
      and make the host—not the skill—enforce capabilities and authorization.
      Read applicable `AGENTS.md` files from the workspace root through every
      target directory before skill selection or task actions; record their
      paths and hashes and surface conflicting instructions.
      Discover workspace-local skills from `.agents/skills/`, `.agent/skills/`
      and `.skills/` plus explicitly configured user roots; namespace their
      provenance, reject policy conflicts, and never auto-install or execute
      discovered scripts/network effects.
      Activate end-of-session `ahead-code-review` against an immutable snapshot;
      findings must remain non-mutating until a human chooses a disposition.
- [ ] Managed Codex App Server transport now exists in
      `ahead-harness/src/managed_client.rs` and is the default tier; complete
      AHEAD-owned per-tool decision UI and verify a real multi-tool turn. The
      managed boundary now fails closed for native shell approvals and bounds
      file-change approvals to workspace paths (with explicit tests), but the
      current `AheadAgentLoop` remains a deterministic fallback and ACP remains
      compatibility-only.
- [ ] **ACP guardrail gap (live-verified 2026-09-18):** the Codex ACP adapter
      executed shell/`edit` tool calls with **zero** `session/request_permission`
      and never called client `fs/write_text_file`, even in `read-only` mode, and
      wrote the target file in every mode. Do not present external ACP agents as
      satisfying teaching-task read-only enforcement, scope allowlists or edit
      attribution.
- [ ] `AHEAD_ACP_COMMAND`/`AHEAD_ACP_ARGS` override exists for tests; expose
      external-agent selection in settings. The conversation header labels the
      active tier and warns that external shell/edits are not AHEAD-mediated;
      the remaining unsupported-capability list should be actionable.
- [ ] Permission policy currently auto-selects the first `allow_*` option in
      assistance tasks; with the ACP tier this is presentation only. Do not claim it is an
      enforcement boundary.
- [ ] The ACP adapter is launched via `npx -y @agentclientprotocol/codex-acp`
      (unpinned). Pin/package the adapter and record provenance; consider the ACP
      Registry (`https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`)
      for external-agent discovery and `CODEX_PATH` for the managed runtime.
## Editor

- [ ] Exercise the real editor completion/diagnostic path in a running window;
      the code panel no longer seeds sample code, fake completions or fabricated
      diagnostics, and should remain empty/clean until the file and proxy supply
      actual state.
- [ ] Complete the FIM context contract in `docs/development/ahead-editor-mvp.md`
      §8.3: the live buffer, active session title/phase, durable work items,
      work-item notes, conversation summaries and caller work context now reach
      the prediction assembler; the editor now rejects stale async
      completion/FIM responses after edits or file switches, and accepted
      completions now replace/insert at the live editor selection instead of
      appending to the document. Production fallback code is removed; the host
      now dispatches the assembled context to an explicit no-tools
      OpenAI-compatible completion route and fails closed when unavailable.
      Editor-only prediction remains available without a durable session.
      Real outbound-provider, cancellation and acceptance-provenance
      verification still need end-to-end support.
- [ ] `recommended_cursor` is produced (`ahead-proxy/src/ahead/agent.rs`) and
      persisted (`store.rs`) but never consumed by any panel. Offer explicit
      navigation/follow behavior; ordinary agent cues must preserve the human
      caret and selection. See `docs/development/ahead-workflows.md` §6.
- [ ] `ahead-app` has no real UI tests (only `proxy_client` and `ross` unit
      tests); the panels are compile-verified only.
- [ ] Managed Codex now applies file changes through the native app-server and
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
      `docs/development/ahead-humanlayer-workflows.md` §4.1–4.2 and §6.
- [ ] Shared `.ahead/config.toml` and ignored `.ahead/config.local.toml` now
      layer non-secret AI settings over `~/.ahead/settings.toml`; the settings
      panel exposes the winning source and credentials remain user-local. Add
      the remaining effective project/workflow settings and verify precedence
      in a running workspace; keep databases, sidecars and personal overrides
      ignored. User saves now use TOML serialization and a private user-file
      mode; the remaining effective project/workflow settings are still open.
- [ ] Implement the working-document convention in
      `docs/development/ahead-workflows.md` §9 using the existing session host,
      store and editor save path: named Markdown artifacts with one canonical
      path, private/shared separation, revision/hash indexing and external-edit
      conflict handling. Point agents and UI links at the same artifacts;
      retain libSQL for runtime history instead of duplicating editable documents.

## Guided investigation, debugger and voice

- [ ] Replace the session-wide Learn/Assist switch with task-local intent:
      create `teaching` tasks only when the human asks to learn and default all
      other work to `assistance`; link teaching arcs to parent debugging or
      feature tasks without changing their effect policy. Update
      `ahead-viewmodel`, `ahead-proxy`, `ahead-app` and the generated Rust/RPC
      contract before claiming the new workflow is live.
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
- [ ] Connect session-scoped presentation actions to real GPUI focus/pointer
      rendering through `ahead-rpc/src/ahead.rs::PresentationCue` and
      `ahead-app/src/code_panel.rs`; acknowledge actual rendering, reject stale
      targets and preserve the human caret. Validate against the Maieutic
      interaction described in `docs/development/ahead-workflows.md` §6.
- [ ] Extend `ahead-app/src/{proxy_client,debug_bar}.rs` and the existing
      `ahead-proxy/src/plugin/dap.rs` route for guided breakpoint setup: retain
      source/debug-session identity, ownership, actual binding/verified status
      and paused thread/frame identity. Preserve the complete per-source set
      when adding an agent breakpoint; replace hardcoded active-line targeting.
      Expose the tested capability to the selected harness and verify the real
      experiment loop in `docs/development/ahead-workflows.md` §5 and §10.
- [ ] Wire `ahead-proxy/src/ahead/voice.rs` and `ahead-viewmodel/src/voice.rs`
      probes to real microphone, transcript and playback plus presentation
      acknowledgements. Verify speech interruption while typing/debugging,
      stale-cue/audio rejection and separate speech/task/debug controls in the
      rendered editor; queue-only tests do not prove that journey.
