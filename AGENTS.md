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

## Editor Component Architecture & Implementation References

- **Primary UI**: Use `gpui-kit` components for all standard chrome, dock, status bar, and panels.
- **Custom Editor Component**:
  - The code editor part can be customized or ported using:
    1. **`gpui-kit`'s built-in Editor**: `gpui_kit::component::input::Editor` / `EditorState` (Tree-sitter highlighting, folding, line numbers, clean/dirty state).
    2. **`gpui-editor`** (https://github.com/iamnbutler/gpui-editor/): Standalone GPUI editor component designed to fit into `gpui-kit` (GapBuffer-backed text editing, goal-column cursor tracking, selections, syntax highlighting, keymaps).
    3. **Zed source reference** (https://github.com/zed-industries/zed): Pinned patterns for editor selection, IME, and rendering.
    4. **Lapce rope math**: For rope position math and document handling, reference `lapce-xi-rope` (used by `ahead-core`/`ahead-rpc`); the old Floem shell has been removed.

## In-Editor Agent & Codex Extraction Plan (`third-party/codex`)

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
