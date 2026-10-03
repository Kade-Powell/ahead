# Search and Git

## Search this workspace

Press **Cmd+Shift+F** / **Ctrl+Shift+F**, or choose Search in the status bar. Enter text and use **Case**, **Word**, or **Regex** when needed. Include and exclude patterns narrow the file paths. Select a result to open its file at the match. Search includes supported unsaved editor buffers, so save only when you intend to keep the change.

The three search options can be combined: **Regex** changes how the query is parsed, **Case** makes matching case-sensitive, and **Word** requires whole-word matches.

For a known file, press **Cmd+P** / **Ctrl+P**. Type a file name or `path:line`. For an action rather than a file, open the Command Palette with **Cmd+Shift+P** / **Ctrl+Shift+P** or **F1**.

## Inspect Git changes

Open Source Control from the left activity bar or Command Palette. The panel lists changed files and their Git status. **Stage All** stages the current changes, including untracked files; inspect the list first. Enter a message and choose **Commit Staged** to commit only what is staged. Git errors appear in the panel, and a failed commit keeps the draft. The editor gutter also marks changed lines, and the active-line blame display can be enabled in Settings.

**Partial:** History, View Diff, Publish, and per-file selection are not wired end to end. Use the terminal for those operations until the panel workflow is finished. Do not treat a displayed control as proof that it saved or published anything. The new stage→commit click-through has automated coverage, but native visual acceptance is still pending.

## Keep private state private

The workspace `.ahead/settings.toml` and runtime state are private project files. AHEAD creates an ignore boundary for them. Review `git status` before a commit; do not add API keys or session databases to a shared repository.
