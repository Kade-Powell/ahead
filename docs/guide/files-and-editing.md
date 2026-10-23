# Files and editing

## Open and manage files

Open a file from Explorer, or press **Cmd+P** / **Ctrl+P** and type its name. Quick Open accepts a location such as `src/main.rs:12`. A tab with a dot has unsaved changes. Save with **Cmd+S** / **Ctrl+S**. Right-click an Explorer entry for path copying, duplication, adding it to `.gitignore`, or moving it to Trash. The file tab menu has close and path actions.

The Command Palette also offers **Open File**, **Open Folder**, **Save File**, and
editor-tab management. **Close Other Editors**, **Close Editors to the Left**,
and **Close Editors to the Right** apply within the active editor group; **Close
All Editors** closes tabs across both groups. Each close action uses the normal
save/discard/cancel flow. With at least two open files, **Move Editor to Other
Group** moves the active tab between side-by-side groups.

Markdown files open in Preview. Use the Preview Markdown control in the file header, or **Cmd+Shift+V** / **Ctrl+Shift+V**, to switch between source and preview. The editor has no Save button.

UTF-8 files open normally. BOM-marked UTF-16LE and UTF-16BE files can also be edited; saving preserves their byte order and BOM. Binary files, malformed UTF-16, and invalid UTF-8 remain unavailable for editing.

## Recover an unsaved buffer

If AHEAD restarts after you edited a file, it can restore the last acknowledged recovery snapshot as a dirty tab. Check the restored text, then save it yourself. If restoration was deferred because a dirty tab was already open, choose **File > Recover Unsaved Changes**. Recovery runs periodically; the last keystrokes before an abrupt stop may be absent. Files over 32 MiB are not included.

## Use language assistance

With a working language server, type in a supported file for completion. Use **Ctrl+Space** to request suggestions. Hover a symbol for information; use **F12** for definition, **Shift+F12** for references, and **Cmd+F12** / **Ctrl+F12** for implementation where supported. Use **Cmd+Shift+Space** on macOS or **Ctrl+Shift+Space** elsewhere (or **Editor → Show Signature Help**) for the active function signature; Up/Down cycles overloads and Escape closes it. Use **Cmd+.** on macOS or **Ctrl+.** elsewhere (or **Editor → Show Code Actions**) to list language-server actions; use Up/Down and Enter to select/apply, or Escape to close. AHEAD validates and applies language-server text edits across workspace files, leaving affected tabs dirty for review and saving; a user-invoked edit is grouped into one undo step per affected file. Server-originated `workspace/applyEdit` is routed through the same validation path and reports whether the edit was applied; verify this flow against a live server before relying on it. File create/rename/delete operations remain unsupported and fail closed. Use **Shift+Alt+F** or **Editor → Format Document** to format the file. Problems shows diagnostics. Rename, formatting, signature help, and code actions depend on the server and file type. If a server exits, open Language Servers in the left activity bar and choose **Restart**. These actions are also searchable in the Command Palette while a language server is connected: **Go to Definition**, **Find All References**, **Go to Implementation**, **Rename Symbol**, **Format Document**, **Show Completions**, **Show Signature Help**, and **Show Code Actions**. LSP location jumps are tracked per window; use **Go Back** (**Ctrl+-**) and **Go Forward** (**Ctrl+_**) in the title bar, Go menu, or Command Palette to retrace them.

Completion items that insert snippets select their placeholders. Use **Tab** or **Shift+Tab** to move between placeholders, and **Up/Down** to cycle snippet choices. Snippets can include values from the current selection, file/workspace and clipboard, plus comment delimiters from built-in mappings or installed language configuration.
Repeated placeholders expand to the same initial default; editing one occurrence does not yet update its linked occurrences.

For Rust, AHEAD follows Zed's worktree-aware resolver: it tries a runnable `rust-analyzer` from the workspace's rustup toolchain override, then AHEAD's PATH, then its managed cache. On a cache miss it downloads the latest official Rust Analyzer release into AHEAD's cache, so first use may need network access. For TypeScript/JavaScript and Python analysis, AHEAD uses Vtsls or BasedPyright from PATH, then an AHEAD-owned cached npm package. System Node.js/npm are preferred; if either is unavailable, AHEAD can acquire a checksum-verified Node.js LTS runtime into its cache and use its bundled npm for the server install. Installed Zed language extensions can add language servers. AHEAD includes parser-backed syntax highlighting for Rust, JavaScript/JSX, TypeScript/TSX, Python, and JSON without a server. Installed extension grammars also have a parsing path, but their rendered colors still need native verification. See Connections and settings for trust and installation limits.

Python also starts Ruff for linting and formatting. It looks for Ruff in the project's `.venv`, then PATH, then AHEAD's managed cache; on a cache miss it downloads an official release. BasedPyright supplies typed completion and hover, while Ruff handles **Format Document**. Both servers' diagnostics and code actions appear together. Each action keeps its server owner for resolve and command requests, so a fix is not sent to the other server. Saving changes to `pyproject.toml`, `ruff.toml` or `.ruff.toml` reloads Ruff's configuration without a manual restart. Formatting, fixes, configuration reload and restart pass the proxy/server smoke; native formatting and code-action acceptance still need verification.

Single-server features use the first enabled server in order that supports the request, rather than racing overlapping analyzers. You can [choose server order, disable a server or select one for a specific feature](connections-and-settings.md#choose-language-servers) in your user or project configuration.

## Accept an inline suggestion

When an edit prediction appears as ghost text at the caret, press **Tab** to accept it. It uses the live buffer and, when one is active, relevant session context. Predictions need a configured provider and can be unavailable or wrong. Read the inserted text before saving. Agent edits are a separate workflow; an inline suggestion does not grant an agent permission to edit.

## Read change and attribution marks

The gutter shows additions, modifications, deleted-line boundaries, breakpoints, and agent-authored ranges. The active-line author display can be switched in Settings. Before commit, AHEAD's anchor table records agent-authored regions; after commit, Git blame supplies author information. **Partial:** the full authenticated agent edit → gutter → commit journey still needs native verification.
