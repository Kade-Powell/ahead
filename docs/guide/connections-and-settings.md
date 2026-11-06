# Connections and settings

Open Settings with **Cmd+,** / **Ctrl+,** or the gear in the status bar. Use the left navigation to open **Appearance**, **Editor**, **AI Integration**, **Voice Calls**, or **MCP Servers**. Provider setup and per-provider model visibility are together on the **AI Integration** page. Changes to AHEAD's connection settings are saved automatically for the current workspace. The status line reports loading and save errors.

## Connect a model

1. Under **AI Integration**, add or select a named OpenAI-compatible server.
2. Enter its **Base URL**, optional **API Key**, and **Fallback models**. The first fallback is active until the endpoint can be queried.
3. AHEAD discovers models from configured endpoints at startup, whenever the chat model picker opens, and when saved settings are refreshed. Searching within an open picker does not repeat discovery. Choose **Discover Models** to refresh every configured endpoint. A failed or offline endpoint leaves its saved fallback models available.
4. On the same page, the model list is grouped by provider. All discovered models are shown in the chat picker by default; expand a provider and turn off **Show … in chat model picker** only for models you want hidden from that provider. Provider sections start collapsed. Each provider heading stays visible while its virtualized model list scrolls, with a scrollbar showing the list position. Choices are saved per provider and do not hide the same model from another provider. Use **Limit** on a model row to correct that model's advertised context window; enter the exact token count, or leave it blank to clear the override. Return to the Agent composer and select a model. A reachable endpoint does not prove a model turn will succeed; try a small request and read any error.

The picker prefers each endpoint's display name and shows the connection name beneath it in muted text; requests continue to use the exact model ID. Standard OpenAI-compatible `/models` entries also provide model descriptions and context limits when advertised. Catalogs that expose `architecture.input_modalities` also inform prompt handling for text, image, and audio inputs; unknown modality labels are ignored. The runtime treats incomplete capability metadata conservatively rather than assuming Codex-specific tools are supported.

Context limits resolve in this order: the per-model **Limit** override in Settings, that model's `/models` metadata, then the provider's `context_window` fallback. Use the per-model override when an endpoint reports an incorrect value or does not report one. The same override can be configured in the connection table as `model_context_windows = { "openai/gpt-luna" = 1048576 }`; the key must exactly match the model ID. To set a provider fallback, add `context_window = 262144` alongside that connection's `name`, `base_url`, and `model` fields in `.ahead/settings.toml` (or a non-secret profile in `.ahead/config.toml`). Settings preserves both fields when saving the connection.

The composer meter uses provider-reported token usage when available. If a provider omits usage metadata, AHEAD estimates usage from the active conversation history; this is a coarse estimate and can differ from the provider's tokenizer. For managed sessions, the meter denominator is the selected model's discovered or configured context limit; when that limit is unknown, the managed meter is hidden instead of showing a guessed value. By default, automatic compaction starts near 90% of that limit, while the runtime enforces a 95% hard usable-window cap, so compaction can begin while the meter still shows room. External ACP sessions use the context limit reported by that agent.

For managed sessions, model and reasoning effort share one searchable picker. When the selected model advertises reasoning levels, its effort appears above the model name in the compact trigger; the same popover contains a slider and **Use model default** reset. Models without reasoning levels omit the effort controls.

The built-in agent initializes its model catalog from the connection selected for that chat, and rebuilds the managed runtime when the selected connection changes. This keeps model capabilities and context metadata tied to the same endpoint used for requests; the managed runtime does not share a remote model-cache file between providers. Conversation history is retained; in-flight turns keep their original connection. If another managed turn is still running, finish or stop it before retrying. Switching models within the same connection does not restart AHEAD.

The managed runtime loads the selected endpoint's `/models` metadata before resolving model-specific reasoning options. It keeps that catalog in memory for up to five minutes per provider runtime; an explicit online refresh bypasses the in-memory age check. If the endpoint is unreachable, the runtime continues with conservative fallback metadata rather than silently changing the model or provider.

The API key is hidden by default. Use **Show** to inspect it temporarily and **Hide** when finished; switching connections hides it again.

User-level provider defaults and credentials can live in `~/.ahead/settings.toml`; workspace-private credentials and overrides are stored in the ignored `.ahead/settings.toml`. Shareable, non-secret project settings belong in tracked `.ahead/config.toml`; local overrides can use ignored `.ahead/config.local.toml`. User defaults are merged with workspace provider profiles by provider ID, with later workspace layers taking precedence. Editing an inherited profile from a workspace promotes its effective values into that workspace's private settings. A private API key is retained across a non-secret override only when its Base URL is unchanged; changing the endpoint requires a key in a private settings layer for that endpoint. Settings shows the effective source for non-secret AI values. Changes to user-level and workspace settings are watched and refresh the Settings page and chat model picker without requiring an app restart.

The built-in agent passes connection settings in memory; it no longer reads or writes `.ahead/runtime/config.toml`. Earlier builds may have left that ignored file containing credentials. AHEAD does not delete existing files automatically; you can remove it after confirming you do not use it for anything else.

## Appearance and editor

Choose **Dark Mode** or **Light Mode** under Appearance. **Show author on active line** controls the inline Git author display. These choices affect the current editor view; check the status message if saving settings fails.

## Language extensions

Open Extensions with **Cmd+Shift+X** / **Ctrl+Shift+X**, or choose **Extensions** from the Command Palette. The gallery opens in a center tab with search focused. If AHEAD is installing its built-in Python or TypeScript/JavaScript language server, progress appears above the gallery. A setup failure appears there with **Retry setup**; the **Language Servers** panel shows the full server status.

Under **Language Extensions**, the tab requests current metadata from Zed whenever it opens. Search by name or description, and use **All**, **Installed**, or **Not Installed** to narrow the list. Each row shows whether it came from the live catalog or a local installation. It shows the Wasm API version when the catalog supplies one; offline installed packages may show **unknown** because Zed embeds that version in the Wasm module rather than the manifest. The extension host validates the module before starting its server. “Compatibility candidate” means AHEAD has not verified that the extension's language server starts and works correctly. Choose **Install** or **Update** for a candidate extension package; **Update** appears only when the catalog version is newer than the installed version. **Refresh** retries if the catalog is unavailable. Development and release builds use Zed's live catalog and package-download service. AHEAD is independent and is not endorsed by Zed; extension compatibility and service availability are not guaranteed, and Zed may change or discontinue these endpoints. Install only extensions you trust; extension code can use host capabilities when the user-level `granted_extension_capabilities` setting allows them. A matching installed extension takes precedence over the built-in server adapter. Installing or updating a language-server extension reloads the active language servers automatically; use **Language Servers > Restart** for manual recovery.

For an installed extension, choose **Uninstall** to remove its package from AHEAD's user data. AHEAD stops the current language servers, removes only the selected package, rediscovers the remaining extensions, and reopens active editor buffers against the updated language support.

Extension capabilities follow Zed's host-grant model. If the setting is absent from your user-level `~/.ahead/settings.toml`, AHEAD uses Zed's default grants: process execution, file downloads and npm installs are allowed by the host. A process command must still be declared by the extension manifest. To restrict host access, add a narrower list or set it to `[]` to deny these capabilities. For example, to allow only the npm package used by Zed's HTML extension:

```toml
granted_extension_capabilities = [
    { kind = "npm:install", package = "@zed-industries/vscode-langservers-extracted" },
]
```

These grants are user-level, not project-level, so a workspace extension cannot grant itself access. Restart AHEAD after changing them. If the settings cannot be parsed, extension-host startup is disabled and the reason appears under **Language Servers > Extension issues**; fix the setting and restart AHEAD. The project-specific **Allow & Start** choice for language servers remains a separate gate.

Python starts Ruff alongside BasedPyright for linting and formatting. Ruff is a native executable: AHEAD checks the project's `.venv`, then PATH, then its managed cache. On a cache miss it downloads an official Ruff release from GitHub, checking the archive checksum when the release supplies one. The download is bounded and cancellable. Saving changes to `pyproject.toml`, `ruff.toml` or `.ruff.toml` reloads Ruff's configuration without a manual restart, including when those files are created, renamed or deleted.

For Rust, AHEAD follows Zed's worktree-aware selection pattern: when the workspace has `rust-toolchain.toml` or `rust-toolchain`, it first tries that toolchain's `rust-analyzer` and verifies it can run. It then checks a working `rust-analyzer` on AHEAD's `PATH`, followed by AHEAD's managed cache. On a cache miss, AHEAD downloads the latest official Rust Analyzer release into that cache; the first install therefore needs network access. A rustup shim by itself does not guarantee that the selected toolchain contains the component.

For Python and TypeScript/JavaScript, AHEAD first uses `basedpyright-langserver` or `vtsls` on PATH, then an AHEAD-owned cached npm package. On a cache miss, AHEAD installs the package in the background with npm lifecycle scripts disabled and keeps the package plus npm download cache under AHEAD's cache—not the project's `node_modules` or the user's global npm cache. AHEAD uses system Node.js and npm when both are available. Otherwise, it reuses AHEAD's pinned managed Node.js release from its cache without a network check. On a cache miss, AHEAD downloads that release from the official Node.js distribution site, verifies the archive against its SHA-256 manifest, and caches Node and its bundled npm. If a newer pinned version cannot be downloaded, an older valid cache remains available offline. The download is capped and cancellable. First use without system Node/npm therefore needs network access to nodejs.org and the public npm registry. Language servers still wait for a per-project decision. On first open, choose **Allow & Start** or **Keep Blocked**; an explicit block is remembered. You can change the decision later in **Language Servers** in the left activity bar with **Allow** or **Block**. The decision is stored in your user config, not in the project. This LSP control is not a complete Zed-style Restricted Mode: other project-driven processes and settings are not gated yet, and its native UI journey is not verified. Do not open an untrusted project in this preview. npm installation times out after two minutes; the Language Servers panel shows startup or install errors, and **Restart** retries a failed install. A malformed cached package is treated as a cache miss. Both npm packages reached Ready after first-run downloads from the public npm registry in a disposable native app profile. The managed runtime was exercised without Node/npm on PATH in a disposable macOS profile; Windows behavior and native install-progress rendering remain unverified. AHEAD acquires the runtime on demand rather than bundling it.

**Compatibility is partial:** AHEAD accepts Zed extension API 0.6, 0.7 and 0.8. Offline host probes with disposable copies of installed Proto and HTML packages (API 0.7) pass, including workspace-settings refresh. A disposable native run confirmed loading the live catalog, installing the HTML extension and starting its server to Ready. The opt-in proxy smoke against the matching installed Zed HTML server also verifies workspace settings, embedded-CSS diagnostics and live clearing after a setting change, completion, formatting and rename. The live HTML package also produces grammar and highlight captures in the proxy smoke; native editor colors remain unverified. Native filesystem-watcher propagation, completion insertion/Undo and other third-party packages remain unverified; use built-in adapters for baseline Rust, TypeScript/JavaScript and Python support.

An installed Zed language extension can read the matching `[lsp.<server-id>]` table from user-level `~/.ahead/settings.toml`, tracked `.ahead/config.toml`, ignored `.ahead/config.local.toml`, and workspace-private `.ahead/settings.toml`, in that order. Later layers override individual fields in `settings` and `initialization_options`. An executable `binary` override is accepted only in the ignored `.ahead/config.local.toml`. For example:

```toml
# .ahead/config.toml
[lsp.vtsls.settings]
maxProblems = 10
```

The Zed extension API's `LanguageSettings` request is also answered from the
matching `[languages.<name>]` table in those same layers. It supports
`tab_size`, `hard_tabs`, and `preferred_line_length`, defaulting to `4`,
`false`, and `80`. AHEAD applies `tab_size` and `hard_tabs` to the editor when a
file is opened and when language settings change, using the same values
supplied to installed extensions.
`preferred_line_length` sets the editor's column ruler and is provided to extensions; it does not yet change editor wrapping. If a language-settings file is invalid,
the editor keeps its default four-space indentation and reports the problem in
the editor status.

This is an extension-host setting, not a separate AHEAD LSP command setting. At launch, AHEAD recursively merges configured `initialization_options` over the extension's defaults; object fields merge and arrays or scalar values replace. A private `binary` override can set `path`, `arguments`, and `env`, with relative paths resolved from the workspace root. AHEAD watches workspace `.ahead` settings and user-level `~/.ahead/settings.toml`. Changes to server `settings` are sent to the running extension with `workspace/didChangeConfiguration`; changes to its launch command, arguments, environment, or `initialization_options` restart only that language server. Built-in adapters do not use these extension settings. The opt-in real-server smoke verifies live workspace refresh: it disables HTML embedded-CSS validation and observes the existing server clear its diagnostic without a restart. It dispatches the same catalog refresh event as the watcher; the native watcher-to-editor journey is still unverified. Use **Restart** if a change does not take effect.

A malformed installed extension appears under **Extension issues**, separate from active language servers. Fix or remove it and choose **Restart** to clear the warning.

### Choose language servers

Server order and per-feature choices apply to both built-in adapters and installed extensions. Add them to `~/.ahead/settings.toml` for user defaults or a workspace `.ahead` config file for project choices:

```toml
[languages.Python]
language_servers = ["basedpyright-langserver", "ruff"]

[languages.Python.requests]
"textDocument/hover" = "basedpyright-langserver"
"textDocument/formatting" = "ruff"
```

Use server IDs, such as those above or the IDs declared by an installed extension, not extension display names. For single-server features, AHEAD queries the first enabled server in order that supports the request and document. It does not race replies or try another server after a valid empty result or an error. Code actions normally collect from all enabled servers; each action retains its owner for resolve and command requests. A `requests` entry limits that method to the named enabled server. Choosing a disabled server reports an error rather than silently falling back.

`language_servers = ["ruff", "..."]` puts Ruff first and includes the remaining registered servers. `["!ruff", "..."]` excludes Ruff. An explicit list without `"..."` enables only the listed servers; `[]` disables all servers for that language. Without an override, AHEAD keeps its default adapter selection. Python defaults to the analyzer before Ruff.

Some Zed language extensions mark a server as opt-in for particular languages. AHEAD keeps that server available but does not start it by default. Add its server ID to the language's explicit `language_servers` list to enable it. The `"..."` wildcard skips opt-in servers, matching Zed.

Settings load from user settings, tracked `.ahead/config.toml`, ignored `.ahead/config.local.toml`, then workspace-private `.ahead/settings.toml`. A later server list replaces the earlier list; per-method choices merge. Saving a routing change restarts the servers and replays open unsaved buffers without saving them. Malformed routing settings report an error and leave the last valid configuration running. These choices follow Zed's TOML language settings rather than a separate picker; the Language Servers panel's **Config** button creates `.ahead/config.toml` when needed and opens it for shareable routing and extension-server settings. Expand a server's **Options** to see its supported configuration fields, then edit its `[lsp.<server-id>]` table in that file. Keep secrets and executable overrides in the ignored private layers described above.

## MCP servers

AHEAD currently supports local stdio MCP servers only; Streamable HTTP and
other remote transports are not enabled. The declaration and approval flow is
AHEAD-owned rather than Zed's `context_servers` format; see the
[MCP configuration and security contract](../development/ahead-agent-standards.md#managed-mcp-configuration-ahead-owned-settings-mcp-wire-protocol).
Managed sessions prefer MCP `2026-07-28` and use the protocol's stdio discovery
probe to fall back to the legacy initialization flow for older servers.
For stdio servers, `env_vars` is a list of names read from the editor's local
process environment; remote-source selectors and inline token values are not
supported. MCP declarations do not select an execution environment or carry a
separate display name: servers run in the editor's local runtime and use their
configured server ID as the label. Keep secret values outside tracked project
configuration.

Project MCP declarations are reviewed in Settings. Choose **Refresh** to load them, read the exact declaration and fingerprint, then **Approve & Enable** only for the server you intend to run. Approval is private to the workspace and applies to new managed sessions. If a declaration changes, review and approve the new version. **Disable** prevents it in new sessions; stop an existing session separately.

An enabled server starts on **review each call**. You can choose **Auto-approve permitted calls** after reviewing it, and switch back to **Require review for each call** at any time. Named tool denials and review choices still apply. The server runs with your OS permissions, so use auto-approval only for servers you trust. Changes take effect in new managed sessions.

If a managed MCP server asks to open a website, the chat shows the server name, destination host and full HTTPS URL. Check the destination before choosing **Open in browser**; **Decline** and **Cancel** leave it unopened. Enter credentials only on the trusted website, never in the chat form. This URL journey has automated card tests but has not yet been verified in a running native app.

**Partial:** model-specific tool availability and real MCP startup vary by provider and adapter. Settings can report a server as approved before a live tool call has been tested.
