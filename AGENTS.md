# Agent Guidance for AHEAD

## UI Components & Framework Architecture

- **UI Framework**: Production UI is built using **GPUI** and **gpui-kit** (https://gpui-kit.com).
- **Component Policy**: Make sure we use **gpui-kit components whenever possible** (https://gpui-kit.com/component/). gpui-kit provides pre-built, accessible, high-performance native components for:
  - **Dock & Panels**: `DockArea`, `DockLayout`, `DockSkin`, `BasePanel`, `Panel`, `PanelStyle`, `PanelControl`, `TabBar`
  - **Text & Code Editing**: `Editor`, `EditorState`, `Input`, `InputState` (with line numbers, code folding, tab size, and Tree-sitter syntax highlighting)
  - **Interactive Controls & Buttons**: `Button`, `ButtonVariants`, `Checkbox`, `Switch`, `Dropdown`, `Modal`, `Dialog`
  - **Layout & Containers**: `v_flex`, `h_flex`, `Root`, `Scroll`, `Divider`, `Badge`, `Card`
  - **Navigation & Menus**: `Menu`, `ContextMenu`, `Tabs`, `Breadcrumb`
- **Avoid Ad-Hoc HTML/Floem Widgets**: Always prefer native `gpui-kit` components over custom styling or legacy Floem idioms. gpui-kit has most if not all components needed for AHEAD's editor, panels, and dialogs.
- **Use gpui-kit Icons, Not Emojis**: Always use `gpui_kit::component::IconName` (Lucide catalog) or `Icon::new(IconName::...)` / `Button::icon(IconName::...)` for all status indicators, buttons, tags, chips, and chrome icons. Never use Unicode emojis in UI controls or buttons.
- **Backend & Viewmodel Separation**: Keep views thin by delegating state transitions, policy checks, and turn serialization to `ahead-viewmodel`, `ahead-rpc`, and `ahead-proxy`.
- **`ahead-harness` crate**: AHEAD-owned AI harnessing lives in the separate `ahead-harness` workspace crate (external ACP client, durable streamed-session controller, side tasks, `HarnessStore` boundary). The editor integrates deeply with it; do not put the agent harness back inside `ahead-proxy`. Codex is an implementation detail of the managed tier, not the crate's identity.

## Editor Component Architecture & Implementation References

- **Primary UI**: Use `gpui-kit` components for all standard chrome, dock, status bar, and panels.
- **Custom Editor Component**:
  - The code editor part can be customized or ported using:
    1. **`gpui-kit`'s built-in Editor**: `gpui_kit::component::input::Editor` / `EditorState` (Tree-sitter highlighting, folding, line numbers, clean/dirty state).
    2. **`gpui-editor`** (https://github.com/iamnbutler/gpui-editor/): Standalone GPUI editor component designed to fit into `gpui-kit` (GapBuffer-backed text editing, goal-column cursor tracking, selections, syntax highlighting, keymaps).
    3. **Zed source reference** (https://github.com/zed-industries/zed): Pinned patterns for editor selection, IME, and rendering.
    4. **Lapce rope math**: For rope position math and document handling, reference `lapce-xi-rope` (used by `ahead-core`/`ahead-rpc`); the old Floem shell has been removed.

## FIM Completion Context

- FIM must receive live editor context (current file prefix/suffix, caret/selection, relevant open unsaved buffers and related code) **plus the active AHEAD session**: task/outcome, current plan and step, decisions, invariants, progress and relevant discussion/artifacts.
- Pass this context to the actual FIM function/provider request. Keep it current as buffers and session state change; discard stale suggestions and never carry context across unrelated sessions. Editor-only FIM remains available without a session.
- In Assist, humans write business logic with FIM and explicitly accept suggestions at the caret. This is not restricted to mechanical code and does not authorize autonomous agent edits.
- The required context, budgeting and verification contract is in [Editor MVP §8.3](docs/development/ahead-editor-mvp.md#83-edit-prediction-is-a-separate-inference-path).

## Workflow Decisions and Review Diagrams

- Keep agreed workflow/product decisions documented as the user and agent make them. Use the [workflow atlas](docs/development/ahead-workflows.md) as the review entry point for diagrams, human/AI roles and the decision table; mark proposed defaults and open questions explicitly rather than treating an agent suggestion as accepted.
- Update the relevant flow, role boundary and artifact convention together when a decision changes. Link supporting research from the atlas and keep implementation gaps in `TODO.md`.
- Preserve Maieutic-style separate highlights/pointers and the human caret during teaching. The atlas documents proposed debugger assistance and synchronized voice behavior; do not claim those work from DTOs or scaffold tests alone.

## In-Editor Agent & Codex Extraction Plan (`third-party/codex`)

- **Harness Fidelity (confirmed requirement, 2026-09-18)**: When using Codex, preserve the pinned runtime's model-facing tool names, descriptions, schemas/grammars, tool results and error semantics, instruction layering, conversation/tool-call ordering, compaction, streaming and cancellation. Reusing protocol DTOs or renaming AHEAD tools to resemble Codex is not sufficient. Keep model-specific upstream tool selection rather than freezing one tool list for every model.
- **Harness tiers (decided 2026-09-18)**: AHEAD has two explicit tiers. (1) **Managed**: run the Codex App Server directly and own the effect boundary so Learn read-only, Assist mechanical scope, per-tool human decisions and `CodeAnchor` attribution are enforceable; this is the default for managed Learn/Assist sessions. (2) **External ACP agents**: compatibility only (streaming, cancellation, resume, conversation); their shell/file effects happen in their own process and carry no AHEAD enforcement guarantee. A live probe showed the Codex ACP adapter ran shell/`edit` calls with zero permission requests and never called client `fs/write_text_file`, even in `read-only` mode, so ACP alone cannot carry the lifecycle. See [harness decision](docs/development/ahead-humanlayer-workflows.md#harness-decision-2026-09-18).
- Prefer consuming the real harness through a maintained integration boundary before extracting or rewriting its loop. Add AHEAD session context and narrowly scoped editor capabilities without replacing native tools with proposal-only substitutes. Use supported runtime configuration for scope/sandboxing; removing product telemetry or UI must not remove core harness behavior. Harness fidelity is a requirement, not a guarantee of equal model performance.
- The runtime choice is settled as two tiers (managed Codex App Server vs external ACP agents). The current built-in loop is a fallback, not the managed runtime. See the [harness decision](docs/development/ahead-humanlayer-workflows.md#harness-decision-2026-09-18) and live probe results; do not claim an ACP-only path enforces AHEAD's lifecycle.
- **Core Conversation Engine to Retain**:
  - `codex-app-server-protocol`, `codex-protocol`: JSON-RPC 2.0 wire contract (`turn/start`, `turn/steer`, `turn/interrupt`, streaming items).
  - `codex-core::session::turn::run_turn`: Local turn loop, compaction engine, and conversation state machine.
  - `codex-execpolicy`: Tool approval protocol and prefix-rule execution engine.
  - `codex-app-server::in_process`, `codex-app-server-client`: In-process embedded host facade.
  - `codex-model-provider`, `codex-ollama`, `codex-lmstudio`: Multi-provider local/cloud inference.
- **Bloat to Gut**:
  - `codex-analytics`: Eliminates cloud telemetry and the 35-second shutdown flush timeout.
  - `codex-app-server-daemon`: Eliminates hourly update daemons and external curl scripts.
  - `cli/src/desktop_app`: Eliminates Electron DMG downloaders.
  - `codex-cloud-tasks`, `codex-cloud-config`, `codex-chatgpt`: Eliminates cloud task sync and proprietary upsells.
  - Monolithic CLI multi-tool subcommands and crossterm TUI.

## Architectural & UI Design Decisions (Zed / VS Code Parity)

1. **Interactive Terminal (No Text Input Boxes)**:
   - The terminal must be a true PTY-backed terminal emulator grid (via `portable-pty` / `alacritty_terminal`), NEVER a text input box with a "Run" button.
   - Keystrokes forward directly from the focused terminal grid to the PTY stdin writer (`\r`, `\x7f`, `\t`, VT arrows, `Ctrl+C`, `Ctrl+D`, `Ctrl+L`, etc.).
   - Shell stdout/stderr stream into scrollback rows with ANSI VT sequence stripping.

2. **Conversational Agent Panel & Integrated Plans**:
   - The Agent panel is a conversational chat interface matching Zed (clean thread header, message bubbles, bot avatar, bottom composer with `@` context, model selection, Send arrow).
   - Work items and plan steps must NOT be a separate duplicated pane; they live directly inside the conversation flow as an interactive plan card with status checkboxes (`CircleCheck` for completed items, `Circle` for pending).
   - **No approval gate.** The agent makes changes when instructed. Do NOT render "Authorize & Apply" / "Reject" proposal cards in the conversation. Attribution is tracked instead: every agent-authored region is recorded as a `CodeAnchor` (`actor_id = AHEAD_ACTOR_ID`) with a violet gutter rail, and agent commits use author `ahead` (committer stays the human). Pre-commit the anchor table is authoritative; once committed, `git blame` takes over and the rows are cleared.

7. **Work Tracking (`TODO.md`)**:
   - `TODO.md` at the repo root is the single source of truth for incomplete work: unfinished features, stubs, mocks, and known gaps.
   - When you finish an item and commit it, DELETE it from `TODO.md` in the same commit. Do not keep completed items or a "done" section.
   - Keep entries actionable and specific (file paths + what remains), not vague wishes.
   - Anything knowingly shipped as a stub/mock/placeholder MUST have a `TODO.md` entry in the commit that introduces it.
   - This workflow lasts until the editor is usable end to end; after that we dogfood AHEAD to develop AHEAD, and the backlog moves into the app itself.

3. **Editor Conventions (No Save Buttons, No Alert Banners)**:
   - Editors must NEVER have a Save button in the tab bar. Save is bound to `Cmd+S` / `Ctrl+S` (`on_key_down`).
   - File tab displays file name + dirty indicator dot (`•`) when modified.
   - Agent pointers must NEVER show intrusive alert banners at the top of the file. Point directly to lines/ranges with clean status info.

4. **Unified Threads Sidebar**:
   - Single list under workspace category (`ahead`) with top search.
   - All threads (AHEAD work-item threads + external agent tasks) live in this unified list with timestamps (`1m`, `1h`) and close buttons.

5. **BYOK (Bring Your Own Key / Model) AI Connections**:
   - Dedicated Settings view (`ahead-app/src/settings_panel.rs`) allowing users to bring any model (OpenAI-compatible, Ollama local, Anthropic direct, LM Studio). User-level defaults may live in `~/.ahead/settings.toml`; workspace-private connections and credentials live in ignored `.ahead/settings.toml`; shareable non-secret project settings live in tracked `.ahead/config.toml`.
   - AI connection credentials must never be hardcoded and must never be sent to external telemetry.

6. **Shortcut Tooltips on All Controls**:
   - Every toolbar button, dock toggle, and action icon must have a `.tooltip("Action (Shortcut)")` so keyboard shortcuts are discoverable on hover just like in Zed.

## Agent Skills (`.agents/skills/`)

- Skills are task playbooks ported from Zed (see provenance headers in each file). Load the relevant one when the task matches:
  - **`gpui-test`**: writing, debugging, or reproducing `#[gpui::test]` tests — seeds, `ITERATIONS`/`SEED` reproduction, parking failures, pending task traces.
  - **`gpui-bench`**: designing and interpreting production-shaped GPUI Criterion benchmarks — responsiveness over throughput, `test-support` isolation, before/after evidence.
  - **`lint-creator`**: writing custom `dylint` lints (Zed's `tooling/lints` layout as template). Natural first AHEAD lints: enforcing this file's UI policies (gpui-kit components, Lucide icons, tooltips).
  - **`humanizer`**: removing AI-writing patterns from docs, PR descriptions, and release notes.
- Skill bodies are upstream reference; the `AHEAD note` header in each file records what does not exist here yet.

## Rust Coding Guidelines (ported from Zed `.rules`)

- Prioritize code correctness and clarity. Speed and efficiency are secondary unless otherwise specified.
- Do not write organizational comments that summarize the code. Comments explain "why" only when the reason is tricky / non-obvious.
- Prefer implementing functionality in existing files unless it is a new logical component. Avoid creating many small files.
- Avoid panicking functions like `unwrap()`; propagate errors with `?`.
- Be careful with indexing operations that panic on out-of-bounds indexes.
- Never silently discard errors with `let _ =` on fallible operations:
  - Propagate with `?` when the caller should handle them.
  - Use `.log_err()` or similar when ignoring with visibility.
  - Use `match` / `if let Err(...)` for custom logic.
- Async operations that may fail must propagate errors to the UI layer so users get meaningful feedback.
- Never create new files with `mod.rs` paths — prefer `src/some_module.rs` over `src/some_module/mod.rs`. (Existing `mod.rs` files are grandfathered.)
- New crates: prefer `[lib] path = "...rs"` in `Cargo.toml` for a descriptive root name instead of the default `lib.rs`.
- Avoid creative additions unless explicitly requested.
- Full words for variable names (no `q` for `queue`).
- Scope clones in async contexts with shadowing to minimize borrow lifetimes:
  ```rust
  executor.spawn({
      let task_ran = task_ran.clone();
      async move {
          *task_ran.borrow_mut() = true;
      }
  });
  ```

### Timers in tests

- In GPUI tests prefer GPUI executor timers over `smol::Timer::after(...)` for timeouts/delays driving `run_until_parked()`:
  - Use `cx.background_executor().timer(duration).await` so work lands on GPUI's dispatcher.
  - `smol::Timer` may be untracked by the scheduler and cause "nothing left to run" while pumping.
- See `.agents/skills/gpui-test/SKILL.md` for seed/iteration reproduction.

### Build & lint

- Zed uses `./script/clippy`; AHEAD has no script wrapper — run `cargo clippy --workspace --all-targets` directly. `bacon` runs the fast local check loop; CI runs `cargo clippy` and `cargo fmt --all --check`.

## GPUI Primitives (ported from Zed `.rules`)

GPUI also provides state and concurrency primitives. `gpui-kit` re-exports/wraps these, so the rules below apply through it.

### Context

Context types allow interaction with global state, windows, entities, and system services. They are typically passed to functions as the argument named `cx`. When a function takes callbacks they come after the `cx` parameter.

- `App` is the root context type, providing access to global state and read and update of entities.
- `Context<T>` is provided when updating an `Entity<T>`. This context dereferences into `App`, so functions which take `&App` can also take `&Context<T>`.
- `AsyncApp` and `AsyncWindowContext` are provided by `cx.spawn` and `cx.spawn_in`. These can be held across await points.

### `Window`

`Window` provides access to the state of an application window. It is passed to functions as an argument named `window` and comes before `cx` when present. It is used for managing focus, dispatching actions, directly drawing, getting user input state, etc.

### Entities

An `Entity<T>` is a handle to state of type `T`. With `thing: Entity<T>`:

- `thing.entity_id()` returns `EntityId`
- `thing.downgrade()` returns `WeakEntity<T>`
- `thing.read(cx: &App)` returns `&T`.
- `thing.read_with(cx, |thing: &T, cx: &App| ...)` returns the closure's return value.
- `thing.update(cx, |thing: &mut T, cx: &mut Context<T>| ...)` allows the closure to mutate the state, and provides a `Context<T>` for interacting with the entity. It returns the closure's return value.
- `thing.update_in(cx, |thing: &mut T, window: &mut Window, cx: &mut Context<T>| ...)` takes a `AsyncWindowContext` or `VisualTestContext`. It's the same as `update` while also providing the `Window`.

Within the closures, the inner `cx` provided to the closure must be used instead of the outer `cx` to avoid issues with multiple borrows.

Trying to update an entity while it's already being updated must be avoided as this will cause a panic.

`WeakEntity<T>` is a weak handle. It has `read_with`, `update`, and `update_in` methods that work the same, but always return an `anyhow::Result` so that they can fail if the entity no longer exists. This can be useful to avoid memory leaks - if entities have mutually recursive handles to each other they will never be dropped.

### Concurrency

All use of entities and UI rendering occurs on a single foreground thread.

`cx.spawn(async move |cx| ...)` runs an async closure on the foreground thread. Within the closure, `cx` is `&mut AsyncApp`.

When the outer cx is a `Context<T>`, the use of `spawn` instead looks like `cx.spawn(async move |this, cx| ...)`, where `this: WeakEntity<T>` and `cx: &mut AsyncApp`.

To do work on other threads, `cx.background_spawn(async move { ... })` is used. Often this background task is awaited on by a foreground task which uses the results to update state.

Both `cx.spawn` and `cx.background_spawn` return a `Task<R>`, which is a future that can be awaited upon. If this task is dropped, then its work is cancelled. To prevent this one of the following must be done:

- Awaiting the task in some other async context.
- Detaching the task via `task.detach()` or `task.detach_and_log_err(cx)`, allowing it to run indefinitely.
- Storing the task in a field, if the work should be halted when the struct is dropped.

A task which doesn't do anything but provide a value can be created with `Task::ready(value)`.

### Elements

The `Render` trait is used to render some state into an element tree that is laid out using flexbox layout. An `Entity<T>` where `T` implements `Render` is sometimes called a "view".

UI components that are constructed just to be turned into elements can instead implement the `RenderOnce` trait, which is similar to `Render`, but its `render` method takes ownership of `self` and receives `&mut App` instead of `&mut Context<Self>`. Types that implement this trait can use `#[derive(IntoElement)]` to use them directly as children.

The style methods on elements are similar to those used by Tailwind CSS.

If some attributes or children of an element tree are conditional, `.when(condition, |this| ...)` can be used to run the closure only when `condition` is true. Similarly, `.when_some(option, |this, value| ...)` runs the closure when the `Option` has a value.

### Input events

Input event handlers can be registered on an element via methods like `.on_click(|event, window, cx: &mut App| ...)`.

Often event handlers will want to update the entity that's in the current `Context<T>`. The `cx.listener` method provides this - its use looks like `.on_click(cx.listener(|this: &mut T, event, window, cx: &mut Context<T>| ...)`.

### Actions

Actions are dispatched via user keyboard interaction or in code via `window.dispatch_action(SomeAction.boxed_clone(), cx)` or `focus_handle.dispatch_action(&SomeAction, window, cx)`.

Actions with no data are defined with the `actions!(some_namespace, [SomeAction, AnotherAction])` macro call. Otherwise the `Action` derive macro is used. Doc comments on actions are displayed to the user.

Action handlers can be registered on an element via the event handler `.on_action(|action, window, cx| ...)`. Like other event handlers, this is often used with `cx.listener`.

### Notify

When a view's state has changed in a way that may affect its rendering, it should call `cx.notify()`. This will cause the view to be rerendered. It will also cause any observe callbacks registered for the entity with `cx.observe` to be called.

### Entity events

While updating an entity (`cx: Context<T>`), it can emit an event using `cx.emit(event)`. Entities register which events they can emit by declaring `impl EventEmitter<EventType> for EntityType {}`.

Other entities can then register a callback to handle these events by doing `cx.subscribe(other_entity, |this, other_entity, event, cx| ...)`. This will return a `Subscription` which deregisters the callback when dropped. Typically `cx.subscribe` happens when creating a new entity and the subscriptions are stored in a `_subscriptions: Vec<Subscription>` field.

## Pull Request Hygiene (ported from Zed `.rules`)

When an agent opens or updates a pull request, it must:

- Use a clear, correctly capitalized, imperative PR title (for example, `Fix crash in project panel`).
- Avoid conventional commit prefixes in PR titles (`fix:`, `feat:`, `docs:`, etc.).
- Avoid trailing punctuation in PR titles.
- Optionally prefix the title with a crate name when one crate is the clear scope (for example, `ahead-proxy: Add retry policy`).
- Include a `Release Notes:` section as the final section in the PR body.
- Use one bullet under `Release Notes:`:
  - `- Added ...`, `- Fixed ...`, or `- Improved ...` for user-facing changes, or
  - `- N/A` for docs-only and other non-user-facing changes.
- Format release notes exactly with a blank line after the heading, for example:

```
Release Notes:

- N/A
```

## Review Gate (adapted from Zed `.rules` HARD RULE)

- Zed forces human review by prepending `> [!IMPORTANT]` confirmation lines to `README.md`. AHEAD does **not** use that mechanism — do NOT dirty `README.md` for review gating.
- AHEAD equivalent: keep `TODO.md` accurate per Work Tracking above (update it in the same commit as the work). Never claim human review is complete, never remove human-review gates yourself, and never finalize or submit on the human's behalf. Confirming review is strictly a manual step for the human author.

## Crash Investigation (adapted from Zed `.rules`)

- Zed's Sentry integration (`.factory/prompts/crash/*`, `script/sentry-fetch`, `script/crash-to-prompt`) is Zed-specific and was not ported — AHEAD has no crash-report pipeline yet.
- For crashes: reproduce locally, capture `RUST_BACKTRACE=full` plus relevant logs, record the crash in `TODO.md` with file paths, and link the tracking issue.

## Agent Rules Hygiene (ported from Zed `.rules`)

`AGENTS.md` is read by every agent session. Keep it high-signal.

- After a session, if you discover a non-obvious reusable pattern, propose it under a **"Suggested AGENTS.md additions"** heading in the PR description. Do NOT edit `AGENTS.md` inline during normal feature/fix work. Reviewers decide what gets merged.
- New rules must meet all three criteria: non-obvious (someone familiar with the codebase would still get it wrong), repeatedly encountered, and specific enough to act on (a concrete instruction, not a vague principle).
- Rules scoped to a single crate belong in that crate's own `AGENTS.md`, not the repo root.
- No architectural maps in rules (module layout, data flow — they rot; gather by reading code). Rules are traps to avoid, not maps to follow. No drive-by additions: note the pattern, validate in review, land in a dedicated commit with the why.
