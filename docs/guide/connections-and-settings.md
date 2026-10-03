# Connections and settings

Open Settings with **Cmd+,** / **Ctrl+,** or the gear in the status bar. Changes to AHEAD's connection settings are saved automatically for the current workspace. The status line reports loading and save errors.

## Connect a model

1. Under **AI Connections**, add or select a named OpenAI-compatible server.
2. Enter its **Base URL**, optional **API Key**, and **Model Catalog**. The first model in the catalog is active.
3. Choose **Test Endpoint** to check reachability, or **Discover Models** to list model IDs reported by the server.
4. Return to the Agent composer and select the model. A reachable endpoint does not prove a model turn will succeed; try a small request and read any error.

An open built-in agent chat uses a changed endpoint, credential, or provider on its next turn and keeps its conversation history. In-flight turns keep their original connection. If another managed turn is still running, finish or stop it before retrying. Switching models within the same connection also works without restarting AHEAD.

The API key is hidden by default. Use **Show** to inspect it temporarily and **Hide** when finished; switching connections hides it again.

Credentials are stored in the workspace's ignored `.ahead/settings.toml`. Shareable, non-secret project settings belong in tracked `.ahead/config.toml`; local overrides can use ignored `.ahead/config.local.toml`. AHEAD rejects common credential fields in tracked config, including nested API keys and tokens, plus URL userinfo and credential-named query parameters. Still review every tracked value for secrets. A private API key is retained across a non-secret override only when its Base URL is unchanged; changing the endpoint requires a key in ignored settings for that endpoint. Settings shows the effective source for non-secret AI values. Changes to the user-level `~/.ahead/settings.toml` are not currently watched live; reopen or reload the app after changing it outside Settings.

The built-in agent passes connection settings in memory; it no longer reads or writes `.ahead/runtime/config.toml`. Earlier builds may have left that ignored file containing credentials. AHEAD does not delete existing files automatically; you can remove it after confirming you do not use it for anything else.

## Appearance and editor

Choose **Dark Mode** or **Light Mode** under Appearance. **Show author on active line** controls the inline Git author display. These choices affect the current editor view; check the status message if saving settings fails.

## Language extensions

Open Extensions with **Cmd+Shift+X** / **Ctrl+Shift+X**, or choose **Extensions** from the Command Palette. The gallery opens in a center tab with search focused. If AHEAD is installing its built-in Python or TypeScript/JavaScript language server, progress appears above the gallery. A setup failure appears there with **Retry setup**; the **Language Servers** panel shows the full server status.

Under **Language Extensions**, the tab requests current metadata from Zed when opened. Search by name or description, and use **All**, **Installed**, or **Not Installed** to narrow the list. Each row shows whether it came from that live catalog or a local installation, plus its Wasm API version; “compatibility candidate” is not a verified server-start result. Choose **Install** or **Update** for a candidate extension package; **Update** appears only when the catalog version is newer than the installed version. **Refresh** retries if the catalog is unavailable. This gallery is a development preview: direct third-party use of Zed's package service is not cleared for release. Install only extensions you trust; extensions may acquire or run language-server binaries within their declared capabilities. A matching installed extension takes precedence over the built-in server adapter. Open **Language Servers** in the left activity bar to inspect status or choose **Restart** after changing an installation.

Built-in Rust Analyzer uses `rust-analyzer` on PATH. For Python and TypeScript/JavaScript, AHEAD first uses `basedpyright-langserver` or `vtsls` on PATH, then an AHEAD-owned cached npm package. If neither is present, opening a supported file starts an npm install into AHEAD's cache in the background; Node.js and npm must be on PATH. The managed install disables npm lifecycle scripts and writes to AHEAD's cache, not the project's `node_modules`. Language servers now wait for a per-project trust decision: open **Language Servers** in the left activity bar and choose **Trust**, then confirm **Trust & Start**. Choose **Restrict** there to stop workspace servers again. The decision is stored in your user config, not in the project. This LSP trust control is not a complete Zed-style Restricted Mode: other project-driven processes and settings are not gated yet, and its native UI journey is not verified. Do not open an untrusted project in this preview. Installation times out after two minutes; the Language Servers panel shows startup or install errors, and **Restart** retries a failed install. A malformed cached package is treated as a cache miss. Both packages reached Ready after first-run downloads from the public npm registry in a disposable native app profile. Updates, runtime failure repair, and bundled Node/npm remain unfinished.

**Compatibility is partial:** AHEAD accepts Zed extension API 0.6, 0.7 and 0.8. Offline host probes with disposable copies of installed Proto and HTML packages (API 0.7) passed, including workspace-settings refresh. Installing and starting their language servers in the running editor is not verified yet. Use the built-in adapters for Rust, TypeScript/JavaScript and Python while that live check remains open.

An installed Zed language extension can read the matching `[lsp.<server-id>]` table from user-level `~/.ahead/settings.toml`, tracked `.ahead/config.toml`, ignored `.ahead/config.local.toml`, and workspace-private `.ahead/settings.toml`, in that order. Later layers override individual fields in `settings` and `initialization_options`. An executable `binary` override is accepted only in the ignored `.ahead/config.local.toml`. For example:

```toml
# .ahead/config.toml
[lsp.vtsls.settings]
maxProblems = 10
```

This is an extension-host setting, not a separate AHEAD LSP command setting. AHEAD watches workspace `.ahead` settings and forwards changed server `settings` to running extensions; restart the language server after changing its `binary` or `initialization_options`. User-level `~/.ahead/settings.toml` edits are not watched live. Built-in adapters do not use these extension settings. Automated tests cover the refresh path, but a real installed extension has not yet been verified against it; use **Restart** if a change does not take effect.

A malformed installed extension appears under **Extension issues**, separate from active language servers. Fix or remove it and choose **Restart** to clear the warning.

## MCP servers

Project MCP declarations are reviewed in Settings. Choose **Refresh** to load them, read the exact declaration and fingerprint, then **Approve & Enable** only for the server you intend to run. Approval is private to the workspace and applies to new managed sessions. If a declaration changes, review and approve the new version. **Disable** prevents it in new sessions; stop an existing session separately.

An enabled server starts on **review each call**. You can choose **Auto-approve permitted calls** after reviewing it, and switch back to **Require review for each call** at any time. Named tool denials and review choices still apply. The server runs with your OS permissions, so use auto-approval only for servers you trust. Changes take effect in new managed sessions.

If a managed MCP server asks to open a website, the chat shows the server name, destination host and full HTTPS URL. Check the destination before choosing **Open in browser**; **Decline** and **Cancel** leave it unopened. Enter credentials only on the trusted website, never in the chat form. This URL journey has automated card tests but has not yet been verified in a running native app.

**Partial:** model-specific tool availability and real MCP startup vary by provider and adapter. Settings can report a server as approved before a live tool call has been tested.
