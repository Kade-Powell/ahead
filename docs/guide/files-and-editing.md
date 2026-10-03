# Files and editing

## Open and manage files

Open a file from Explorer, or press **Cmd+P** / **Ctrl+P** and type its name. Quick Open accepts a location such as `src/main.rs:12`. A tab with a dot has unsaved changes. Save with **Cmd+S** / **Ctrl+S**. Right-click an Explorer entry for path copying, duplication, adding it to `.gitignore`, or moving it to Trash. The file tab menu has close and path actions.

Markdown files open in Preview. Use the Preview Markdown control in the file header, or **Cmd+Shift+V** / **Ctrl+Shift+V**, to switch between source and preview. The editor has no Save button.

## Recover an unsaved buffer

If AHEAD restarts after you edited a file, it can restore the last acknowledged recovery snapshot as a dirty tab. Check the restored text, then save it yourself. If restoration was deferred because a dirty tab was already open, choose **File > Recover Unsaved Changes**. Recovery runs periodically; the last keystrokes before an abrupt stop may be absent. Files over 32 MiB are not included.

## Use language assistance

With a working language server, type in a supported file for completion. Use **Ctrl+Space** to request suggestions. Hover a symbol for information; use **F12** for definition, **Shift+F12** for references, and **Cmd+F12** / **Ctrl+F12** for implementation where supported. Problems shows diagnostics. Rename, formatting, and code actions depend on the server and file type. If a server exits, open Language Servers in the left activity bar and choose **Restart**.

Rust Analyzer runs when `rust-analyzer` is on AHEAD's PATH. Vtsls for TypeScript and JavaScript and BasedPyright for Python use a PATH executable when available, otherwise an AHEAD-owned cached package; on a cache miss, AHEAD attempts a background npm install if Node.js and npm are installed. Installed Zed language extensions can add language servers. AHEAD includes parser-backed syntax highlighting for Rust, JavaScript/JSX, TypeScript/TSX, Python, and JSON without a server; other extension-provided grammars are not yet supported. See Connections and settings for installation limits.

## Accept an inline suggestion

When an edit prediction appears as ghost text at the caret, press **Tab** to accept it. It uses the live buffer and, when one is active, relevant session context. Predictions need a configured provider and can be unavailable or wrong. Read the inserted text before saving. Agent edits are a separate workflow; an inline suggestion does not grant an agent permission to edit.

## Read change and attribution marks

The gutter shows additions, modifications, deleted-line boundaries, breakpoints, and agent-authored ranges. The active-line author display can be switched in Settings. Before commit, AHEAD's anchor table records agent-authored regions; after commit, Git blame supplies author information. **Partial:** the full authenticated agent edit → gutter → commit journey still needs native verification.
