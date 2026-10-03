# Terminal and debugging

## Use the terminal

Open the bottom panel with **Cmd+J** / **Ctrl+J** or **Ctrl+`**. The terminal is a live PTY: type commands directly, use shell history and completion, and press **Ctrl+C** to interrupt a running command. The plus control or **New Terminal** in the Command Palette opens another terminal. A task launched from AHEAD opens in a terminal, so you can see its real output and exit status.

## Run a project task

Open **Tasks** in the left activity bar. AHEAD looks for `justfile` recipes in the workspace. Select a recipe to run it in a terminal. If no recipes appear, check that the project has a `justfile` and refresh the Tasks panel. The task's own prerequisites still apply; AHEAD does not install dependencies for it.

## Debug a program

Use **Show Debugger** in the Command Palette or **Cmd+Shift+D** / **Ctrl+Shift+D**. Click a code gutter line or press **F9** to toggle a breakpoint. **F5** starts or continues, **Shift+F5** stops, **F10** uses Step Over, **F11** uses Step Into, and **Shift+F11** uses Step Out. Choose a compatible debug adapter/configuration for the program first. Inspect the paused line and variables in the debugger, then stop the session when done.

**Partial:** debugger setup depends on an installed adapter and the project configuration. The guided breakpoint experiment loop and the full native launch, pause, step, and cleanup journey are still being verified. A visible debugger control does not mean an adapter is connected.
