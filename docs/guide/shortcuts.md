# Shortcuts

Use **Cmd** on macOS for shortcuts shown with Cmd; use **Ctrl** on other platforms where a Ctrl form is listed. The Command Palette (**Cmd+Shift+P** / **Ctrl+Shift+P**, or **F1**) is the searchable source for currently available actions. Tooltips on controls show their shortcut when one exists.

| Action | macOS | Other platforms |
| --- | --- | --- |
| Command Palette | Cmd+Shift+P or F1 | Ctrl+Shift+P or F1 |
| Quick Open | Cmd+P | Ctrl+P |
| Go to Line | Ctrl+G | Ctrl+G |
| Save file | Cmd+S | Ctrl+S |
| Close focused editor or terminal tab | Cmd+W | Ctrl+W in editor; Ctrl+F4 in terminal |
| Search workspace | Cmd+Shift+F | Ctrl+Shift+F |
| Explorer | Cmd+Shift+E | Ctrl+Shift+E |
| Source Control | Ctrl+Shift+G | Ctrl+Shift+G |
| Problems | Cmd+Shift+M | Ctrl+Shift+M |
| Settings | Cmd+, | Ctrl+, |
| Toggle Left Sidebar | Cmd+B | Ctrl+B |
| Toggle Right Sidebar | Cmd+Option+B | Ctrl+Alt+B |
| Toggle Bottom Panel | Cmd+J or Ctrl+` | Ctrl+J or Ctrl+` |
| Debugger | Cmd+Shift+D | Ctrl+Shift+D |
| Agent composer | Ctrl+Cmd+I | Ctrl+Alt+I |
| Start or continue debugging | F5 | F5 |
| Stop debugging | Shift+F5 | Shift+F5 |
| Toggle breakpoint | F9 | F9 |
| Step over / into / out | F10 / F11 / Shift+F11 | F10 / F11 / Shift+F11 |
| Go to definition | F12 | F12 |
| Go back / forward through editor locations | Ctrl+- / Ctrl+_ | Ctrl+- / Ctrl+_ |
| Find references | Shift+F12 | Shift+F12 |
| Go to implementation | Cmd+F12 | Ctrl+F12 |
| Show completions | Ctrl+Space | Ctrl+Space |
| Preview or edit Markdown | Cmd+Shift+V | Ctrl+Shift+V |

The Command Palette also lists discovered `just` recipes as **Run just _recipe_** actions. **Commit Staged Changes** appears when Source Control is connected, at least one change is staged, and a commit message is ready.

Other searchable actions include **Open File**, **Open Folder**, **Close Editor**, **Close Other Editors**, **Close Editors to the Left**, **Close Editors to the Right**, **Close All Editors**, **Move Editor to Other Group**, **Go Back**, **Go Forward**, **Rename Symbol**, **Format Document**, **Show Signature Help**, **Show Code Actions**, **Stage All Changes**, **Extensions**, **AHEAD Guide**, **New Terminal**, **Tasks**, **Language Servers**, **New AHEAD Session**, **Start External Agent**, **Toggle Threads**, and **Zoom Agent Chat**. Debugging actions are available while the debugger is connected; LSP actions appear when the active language server supports them.

Some keys are handled by the focused editor or terminal. If a shortcut does not act on the intended view, click that view and try again. A command that depends on a language server or debugger adapter also needs that service running.
