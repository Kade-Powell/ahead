# Search and Git

## Search this workspace

Press **Cmd+Shift+F** / **Ctrl+Shift+F**, or choose Search in the status bar. Include and exclude patterns narrow the file paths. Select a result to open its file at the match. Search includes supported unsaved editor buffers, so save only when you intend to keep the change.

Choose exactly one match mode: **Text**, **Word**, or **Regex**. Text matches the query literally, including punctuation such as `(` and `.`; for example, `console.log(` is searched exactly as typed. Word also treats the query literally and requires whole-word boundaries. Only Regex interprets regular-expression syntax. **Match case** is a separate modifier and can be combined with any mode.

Regex supports look-ahead and look-behind patterns. It reads each file for cross-line context, while literal search remains streaming.

Choose **Replace** to expand the replacement field. **Replace Match** changes only the selected result; **Replace All** changes every matching occurrence in the filtered workspace. The current match mode, case setting, and include/exclude patterns apply to replacement too. Text and Word replacements are literal; Regex replacements can reference capture groups (for example, `$1` or `$name`) and use `\n`, `\t`, and `\\` escapes. AHEAD checks every target against its current editor buffer before applying any changes. Replacements become undoable unsaved editor edits; affected files open as tabs, and nothing is saved automatically. Review the changed tabs and save only when you want to keep the edits.

For a known file, press **Cmd+P** / **Ctrl+P**. Type a file name or `path:line`. For an action rather than a file, open the Command Palette with **Cmd+Shift+P** / **Ctrl+Shift+P** or **F1**.

## Inspect Git changes

Open Source Control from the left activity bar or Command Palette. The panel lists changed files and their Git status. **View Diff** opens a copyable patch for the workspace's staged, unstaged, deleted, and untracked changes; click a file row to open that file in the editor. **Stage All** in the panel or **Stage All Changes** in the Command Palette stages eligible changes, including untracked files; unresolved conflicts remain unstaged. Per-file controls stage or unstage individual paths. Enter a message and choose **Commit Staged** to commit only what is staged. Git runs the commit, so configured hooks and signing are honored. Git errors appear in the panel, and a failed commit keeps the draft. The editor gutter also marks changed lines, and the active-line blame display can be disabled in Settings.

Choose **History** to load the latest 100 commits, shown with subject, author, relative time, and short ID. Select a commit to open its patch in the panel; added and removed lines are tinted, and the back arrow returns to the history list. Use the copy button to copy the displayed patch. Very large patches are capped at 2 MiB and labeled when truncated. Use the refresh icon to reload history.

Use the branch menu to create, switch, or delete an eligible local branch. **Publish** sets an upstream on a branch that has none; afterward, the action is **Push**. The adjacent remote menu can choose a different push destination without silently changing the branch's configured upstream. Push honors Git's configured credential helpers and askpass settings (`GIT_ASKPASS`, `SSH_ASKPASS`, and `core.askPass`). When a transport has no configured askpass command, AHEAD offers a temporary authentication prompt in Source Control; passwords and passphrases are masked, **Cancel** aborts authentication, and entered credentials are sent only to that Git process, not saved in session or agent data.

## Keep private state private

The workspace `.ahead/settings.toml` and runtime state are private project files. AHEAD creates an ignore boundary for them. Review `git status` before a commit; do not add API keys or session databases to a shared repository.
