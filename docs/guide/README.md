# AHEAD user guide

The eight Markdown files in this directory are bundled into the app's Help tab by `ahead-app/src/help_panel.rs`. Update them with the feature change; a rebuilt app receives the new text. Keep instructions task focused and call out prerequisites, incomplete controls, and unverified paths. The guide is for users; `docs/development/` and `TODO.md` track implementation work.

## Coverage check for each release

Walk the current app in a disposable workspace and compare every Help topic with `tests/fixtures/editor-smoke/README.md`, the current Command Palette, Settings, and `TODO.md`. Capture a fresh screenshot only when a control or sequence is easier to understand visually; remove an image when its UI changes. Do not publish a screenshot of a mock, a secret, or a feature whose native workflow has not been verified. The current guides use text while the screenshot set is pending native capture and review.

| Area | Help topic | Check in app |
| --- | --- | --- |
| Project setup and navigation | Start here | Folder picker, docks, Command Palette |
| Explorer, editor, recovery, LSP, prediction, attribution | Files and editing | Dirty tab, Markdown preview, server-backed edit |
| Workspace search and Git | Search and Git | Search filters, status, Stage All; identify inert Git controls |
| PTY, Just tasks, debugger | Terminal and debugging | Real command, task output, adapter-backed debug session |
| Managed/ACP agents, context, skills, memory, checkpoints | Agent sessions | Configured provider and real turn; external adapter separately |
| BYOK, appearance, extensions, MCP | Connections and settings | Settings sections and saved values |
| Voice and collaboration | Voice and sharing | Permissions, transcript; mark live multiplayer unavailable |
| Keyboard actions | Shortcuts | Active handlers and Command Palette, not legacy keymap files |

Screenshots should live under `docs/guide/images/` with a short descriptive name and alt text in the relevant topic. Re-capture them from the rebuilt native app in a disposable project when the shown controls change. The guide's embedded Markdown renderer must be checked in the app before relying on image display there.
