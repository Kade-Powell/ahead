# Terminal and debugging

## Use the terminal

Open the bottom panel with **Cmd+J** / **Ctrl+J** or **Ctrl+`**. The terminal is a live PTY: type commands directly, use shell history and completion, and press **Ctrl+C** to interrupt a running command. The plus control or **New Terminal** in the Command Palette opens another terminal. **Cmd+W** closes the focused terminal tab on macOS; **Ctrl+F4** does so on Linux and Windows, where **Ctrl+W** stays available to the shell. A task launched from AHEAD opens in a terminal, so you can see its real output and exit status.

Drag to select text; double-click selects a word and triple-click selects a line. The **Edit** menu and terminal right-click menu both provide **Copy**, **Paste**, and **Select All**; right-click also provides **Paste Text**, **Clear**, and **Close Terminal Tab**. **Clear** removes scrollback while keeping the current command line. The tab's right-click menu offers New, Rename, Close, Close Others, Close Left/Right, Close Clean and Close All. **Select All** includes scrollback; use **Cmd+A** on macOS or **Ctrl+Shift+A** elsewhere so plain **Ctrl+A** remains available to the shell. **Paste** follows the clipboard type (text, shell-quoted file paths, or image paste); **Paste Text** always inserts clipboard text.

Mouse input is forwarded to terminal apps that enable mouse tracking, including clicks, drags, movement, and wheel scrolling. Hold **Shift** to select terminal text or scroll the terminal history instead.

Clipboard shortcuts follow Zed's platform defaults: on macOS use **Cmd+C** / **Cmd+V**, with **Ctrl+Cmd+V** for text-only paste; on Linux use **Ctrl+Shift+C** / **Ctrl+Shift+V** or **Ctrl+Insert** / **Shift+Insert**, leaving **Ctrl+C** and **Ctrl+V** for the shell; on Windows use **Ctrl+Shift+C** / **Ctrl+Shift+V**, **Ctrl+Insert** / **Shift+Insert**, and **Ctrl+V** to paste, with **Ctrl+Alt+V** for text-only paste. On Windows, **Ctrl+C** copies a selection and otherwise interrupts the running process. Pasting an image forwards **Ctrl+V** to terminal applications that support image paste. Terminal applications can use OSC 52 to read or write clipboard text, following Zed's terminal behavior.

On Linux and FreeBSD, selecting terminal text also updates the primary selection; middle-click pastes it when the terminal application has not enabled mouse reporting.

## Run a project task

Open **Tasks** in the left activity bar. AHEAD looks for `justfile` recipes in the workspace. Select a recipe to open and focus a new terminal tab named after it, running from the exact justfile that defined it. Every launch gets a fresh terminal, even when rerunning the same recipe; existing terminals and their output stay intact. The Command Palette's task actions use the same behavior. Use the adjacent **Watch** control to run the recipe under `watchexec`, restarting it when files change; this opens a separate `watch: <recipe>` terminal and requires `watchexec` on `PATH`. **Run** remains a one-shot task. If no recipes appear, check that the project has a `justfile` and refresh the Tasks panel. The task's own prerequisites still apply; AHEAD does not install dependencies for it.

## Debug a program

Use **Show Debugger** in the Command Palette or **Cmd+Shift+D** / **Ctrl+Shift+D**. Click a code gutter line or press **F9** to toggle a breakpoint. **F5** starts or continues, **Shift+F5** stops, **F10** uses Step Over, **F11** uses Step Into, and **Shift+F11** uses Step Out. When execution pauses, the debugger shows the stopped thread's call stack and the top frame's variable scopes; click a stack frame or breakpoint to open its source location. Stop the session when done.

Project debug configurations live in `.ahead/run.toml`. The Debug toolbar lets you select a configuration before starting. For example:

```toml
[[configurations]]
name = "Python main"
type = "python"
debug-adapter = "python"
debug-adapter-args = ["-m", "debugpy.adapter"]
program = "main.py"
args = []
cwd = "."
```

Add more `[[configurations]]` tables for other targets. `debug-adapter` and its optional arguments start the DAP server; `program`, `args`, `cwd`, and `env` are sent as the launch request. A relative `cwd` is resolved from the project root. Install the adapter separately. If the file is malformed or a required field is missing, the debugger shows the configuration error instead of silently using a default.

The built-in AHEAD launch configurations are offered only in the AHEAD source workspace. Other projects need their own `.ahead/run.toml`; this avoids showing an unrelated `target/debug/ahead` launch in every workspace.

**Partial:** AHEAD currently reads its own `.ahead/run.toml` format; it does not discover `.vscode/launch.json` or obtain DAP adapters from extensions yet. A visible debugger control does not mean an adapter is connected.
