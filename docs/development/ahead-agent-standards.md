# AHEAD agent standards boundary

Status: normative AHEAD integration boundary, 2026-09-29.

AHEAD reuses the open protocol and file conventions that already have an
editor ecosystem. The hard-forked Codex loop supplies model streaming,
conversation state and compaction; it does not define a second editor,
plugin marketplace or policy system.

Normative open standards and registries:

- [Agent Skills specification](https://agentskills.io/specification)
- [Agent Skills client implementation guide](https://agentskills.io/client-implementation/adding-skills-support)
- [AGENTS.md convention](https://agents.md/)
- [Agent Client Protocol](https://agentclientprotocol.com/protocol/v1/overview)
- [ACP agent registry](https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json) for discoverable external agents
- [Model Context Protocol specification, 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28)
- [MCP versioning and compatibility](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning)
- [MCP transport overview](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports), including standard Streamable HTTP
- [MCP stdio transport](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio)
- [MCP Streamable HTTP transport](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http)
- [MCP server discovery](https://modelcontextprotocol.io/specification/2026-07-28/server/discover)

Non-normative implementation references (not compatibility requirements):

- [Zed skills](https://zed.dev/docs/ai/skills)
- [Pinned Zed agent implementation](https://github.com/zed-industries/zed/tree/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent)
- [Pinned Zed skill loader](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent_skills/agent_skills.rs) and [strict-validation rationale](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent_skills/README.md)
- [Pinned Zed ACP registry store](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/project/src/agent_registry_store.rs), [registry UI](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent_ui/src/agent_registry_ui.rs), and [installed-agent store](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/project/src/agent_server_store.rs)
- [Pinned Zed ACP config-option UI](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent_ui/src/config_options.rs) and [ACP connection](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent_servers/src/acp.rs)
- [Pinned Zed context-server settings](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/project/src/project_settings.rs) and [server lifecycle](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/project/src/context_server_store.rs)
- [Pinned Zed HTTP transport](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/context_server/src/transport/http.rs) and [MCP OAuth implementation](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/context_server/src/oauth.rs)
- [Pinned Zed agent command/tool registration](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/agent.rs) and [in-process context-server registry](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/tools/context_server_registry.rs)
- [Pinned Zed thread store](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/thread_store.rs) and [thread database model](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/db.rs)
- [Pinned Zed project search](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/project/src/search.rs)

## Protocols

- **Local editor/proxy RPC** is AHEAD-owned, not ACP. Session creation and
  adapter-install selection remain pending until a reply or connection loss;
  their backend writes are not cancelled by a local timeout. Closing the
  creation form does not cancel creation. A late success appears in the thread
  list without replacing a newer form or active chat. A lost connection fails
  pending requests and rejects new ones. Restart AHEAD to reconnect and inspect
  restored sessions before retrying: an in-flight write may have completed.
  There is no automatic replay or backward-compatibility transport.
- **ACP** is the external-agent boundary. AHEAD speaks the Agent Client
  Protocol over the adapter's transport and renders the lifecycle it actually
  receives. External ACP processes own their own file, shell and permission
  behavior; ACP is not AHEAD's managed authorization boundary. The install
  catalog is deliberately limited to Pi, Codex and Claude Code, selected from
  the ACP registry rather than exposing arbitrary registry entries. Install
  selection is stored under the user-local `.ahead` directory; the supported
  package is resolved lazily on first launch, with a pinned package fallback
  when the registry is unavailable. This follows Zed's
  `AgentRegistryStore`/`LocalRegistryNpxAgent` lifecycle without importing its
  project/server stores. On a new or resumed external session, an explicitly
  selected model is resolved against
  the adapter's advertised model `configOptions` and sent through standard
  [`session/set_config_option`](https://agentclientprotocol.com/protocol/v1/session-config-options).
  An unavailable model fails instead of silently using the adapter default.
  The editor now prepares an external session when it is attached or restored,
  then presents the ordered advertised select options (including model and
  reasoning selectors) in the composer. User changes and agent-originated
  `config_option_update` notifications replace the complete session option
  state, as in Zed's `agent_servers/src/acp.rs`. AHEAD advertises ACP boolean
  config support and renders select options as menus and booleans as switches;
  typed changes are checked against the agent's advertised options and sent
  with their ACP JSON value types intact. The panel mirrors cached option
  updates while a turn is running as well as while idle. ACP permits config
  changes during generation, and both set responses and
  `config_option_update` notifications carry the full option snapshot; a
  rendered GPUI run is still needed to verify responsiveness and interaction.
  Unsupported future option kinds are ignored. Agent connection, turn and
  config requests run off the proxy dispatcher so a slow adapter does not stall
  unrelated editor RPC. Pi ACP initialization, session setup
  and option discovery pass. On 2026-09-24, an isolated credentialed smoke test
  sent the configured OpenCode `x-preview-f-free` prompt but received no delta
  or completion for 60 seconds; it was stopped before the client’s 1800-second
  timeout. Pi's local catalog does not contain that default model ID. Direct
  no-tools requests returned `Model is unavailable` for it, `FreeTierError` for
  OpenCode's `mimo-v2.5-free` model, and “active OpenCode Go subscription
  required” for the catalog-listed `opencode-go/mimo-v2.5`; local `pi auth check`
  nevertheless reported both providers `ready`. Earlier Bedrock validation
  also returned an expired bearer token. No live Pi ACP model turn is verified.
  The user reconfirmed on 2026-09-28 that provider access is still unavailable.
  An offline check verified Pi CLI v0.87.1 can print help with `PI_OFFLINE=1`
  and `PI_CODING_AGENT_DIR` redirected to a temporary directory; `pi-acp` was
  not found on `PATH` or in the npm cache. This did not validate an ACP session.
  Continue offline and do not retry a live turn until access is restored. Treat
  auth-readiness as advisory; select a currently available model for an active
  provider, then require both a successful direct request and ACP turn with that
  same model.
- **MCP** is the external-tool boundary. The protocol standardizes client/server
  messages, not a universal host configuration path. AHEAD's owned settings
  and managed launch path are wired: workspace declarations in tracked
  `.ahead/config.toml` require an explicit ignored `.ahead/settings.toml`
  opt-in pinned to the exact parsed declaration; only local stdio servers are
  supported, and tools prompt unless a
  workspace-local per-tool choice allows or denies them. The managed agent's
  sandbox denies reads and writes under workspace `.ahead`, user `~/.ahead`,
  and an explicit `AHEAD_HOME`, so it cannot inspect provider secrets or edit
  its own MCP permissions, server declarations or session store. Read-only/Learn
  sessions remove these servers, and
  external ACP agents receive none. This is AHEAD behavior, not Zed-compatible
  configuration. Zed's in-process `ContextServerRegistry` and agent
  command/tool registration are non-normative lifecycle references; do not
  restore Codex extension-contribution hooks.
  The running-window approval journey remains unverified. AHEAD can mediate
  protocol tool calls, but cannot enforce effects performed internally by an
  opted-in server process; do not claim CodeAnchor attribution for its writes.
  MCP servers are not installed from a Codex marketplace and must not widen
  AHEAD policy.
- **Agent Skills** use the portable `SKILL.md` package shape: a directory with
  `SKILL.md`, optional `references/`, and optional audited `scripts/`. AHEAD
  requires the standard `name` and `description` frontmatter, enforces the
  name's lowercase-letter/digit/hyphen syntax and parent-directory match, and
  applies the specification's 64-character name and 1024-character description
  limits. Unicode lowercase letters and numeric characters are accepted as the
  specification permits. The [Agent Skills
  specification](https://agentskills.io/specification) defines these constraints.
  Its [client implementation
  guide](https://agentskills.io/client-implementation/adding-skills-support)
  recommends warning and continuing for mismatched or overlong names as a
  compatibility policy; AHEAD follows the normative constraints and rejects
  those packages. The managed skill catalog reports a path-free skipped-package
  count in the chat palette, without sending host paths over RPC; verify that
  warning in a rendered project/user catalog (tracked in `TODO.md`). Discovery
  rejects files over
  100 KiB from metadata before reading contents and rechecks the streamed byte
  count; the body is not retained and is read again only after selection.
  After selection, AHEAD neutralizes skill-envelope tags and truncates the
  complete prompt body—including AHEAD's name, opaque locator and resource
  metadata wrapper—to 8,200 UTF-8 bytes. This is AHEAD's context budget, not a
  limit imposed by the Agent Skills format. Package-relative resource reads
  have a separate 1 MiB cap.
  Discovery caps reads at two skills per root and eight roots concurrently,
  keeping the effective maximum at 16 skill reads. AHEAD loads metadata first
  and the body/references progressively. The core Agent Skills format does not
  define `disable-model-invocation`; the [client implementation
  guide](https://agentskills.io/client-implementation/adding-skills-support)
  describes it as a client-level opt-out convention. AHEAD honors
  `disable-model-invocation: true` by omitting the skill from implicit model
  selection while retaining explicit slash invocation. Skills and the
  specification's experimental `allowed-tools` field do not grant AHEAD
  capabilities or authorize effects. The
  built-in AHEAD skill packages are sourced from
  `ahead-agent/skills/` and installed into the runtime's system-skill cache;
  Codex sample skills are not part of that bundle. Skill catalog metadata
  carries only a source enum, never an absolute path. When names collide, the
  composer uses source-qualified slash names for collisions: user
  `/user:name`, project `/project:name`, system `/system:name`, and admin
  `/admin:name`; unique names stay `/name`. `/:name` remains accepted as a
  legacy alias for user-scoped invocations. This adapts Zed's scoped
  disambiguation pattern; the exact spelling is AHEAD behavior, not part of the
Agent Skills format. The native runtime resolves the source against the current
model-enabled skill snapshot and reads the selected `SKILL.md` on demand.
  AHEAD-generated host-skill locators use opaque handles containing the skill
  scope, name and a path-derived fingerprint; only host code resolves them to
  files. Do not put absolute host paths in
  generated resource/package IDs, selected-instruction locators or load
  warnings. Skill descriptions and instructions remain user-authored content.
  Opening the slash palette and invoking a skill each request a fresh host
  snapshot, so additions and metadata edits do not require an app restart; the
  composer discards pending catalog replies after dismissal or a session/model
  change. This follows Zed's refresh trigger points without claiming its full
  filesystem-watcher behavior.

## Managed turn context

Before each streamed turn, the AHEAD host snapshots the session title and
objective, task intent, work kind, current phase, linked issue, work items and
their recent events, plus recent conversation summaries into
`AgentTurnRequestDto.session_context`. The snapshot is separate from the
human's `user_message`, editor buffer, project `AGENTS.md` instructions and
selected attachments. The retained runtime adds it as tagged background
context while preserving a leading slash command at the start of the prompt.
The request snapshot is persisted with retry state; a retry reuses the same
context, while a new turn refreshes it from Turso. Older sessions without an
objective remain valid and simply omit that line. Work-item event context is
bounded to the newest four events per item, and summaries to the newest four.

This context is not a permission grant. Managed effects remain governed by the
native AHEAD boundary; an external ACP process owns its own shell and file
effects even when AHEAD supplies the same session context.

## Managed effects

ACP and MCP standardize protocol behavior; neither grants AHEAD effect
enforcement. For the built-in runtime, `ahead-agent/src/runtime_support.rs`
constructs a per-turn file scope with `NetworkSandboxPolicy::Restricted`.
Unscoped shell escalation is denied, and the native runtime grants no
additional permissions in response to `RequestPermissions`. External ACP
processes do not inherit these guarantees.

The retained core still contains a macOS Seatbelt `Enabled` fallback that
allows unrestricted inbound and outbound IP networking when no managed proxy
is configured. The built-in AHEAD path currently supplies a restricted profile,
but disabling `NetworkProxy` alone does not make that legacy branch safe to
reuse. Keep its fail-closed compatibility shim until AHEAD owns and tests the
per-command sandbox network contract; Zed's
[`terminal_tool.rs`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/tools/terminal_tool.rs)
and
[`macos_seatbelt.rs`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/sandbox/src/macos_seatbelt.rs)
are the implementation references for that boundary.

## Composer slash catalog

The leading `/` palette is a grouped discovery surface, not a raw tool list.
It combines AHEAD chat/session actions, skills from the current managed-runtime
snapshot, and commands advertised by the active ACP agent. As the user types,
AHEAD fuzzy-matches each entry's label and description, ranks entries within
their groups, and orders groups by their best match. Keep descriptions visible
and route a selected entry through its owning AHEAD action, explicit skill
invocation, or ACP command path. Skills are instructions, not tools or grants
of authority. Managed prompt composition keeps a selected slash skill at the
start of the user message so the runtime can resolve it before adding editor
context.

Use Zed's pinned
[`completion_provider.rs`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent_ui/src/completion_provider.rs)
as the interaction reference, especially `search_slash_commands` and
`group_by_relevance`; AHEAD additionally matches descriptions so users can find
an entry by what it does. The Codex menu screenshot is a visual reference for
grouping, descriptions, and type-ahead filtering, not a requirement to copy
Codex-only actions. Only expose actions AHEAD actually supports. A disposable
GPUI run on 2026-09-29 verified filtered project-skill rows, Down navigation,
mouse selection, and insertion without sending a turn. Active ACP commands and
authenticated skill invocation remain in `TODO.md`.

## Native model-provider settings

The managed runtime reads provider profiles from `$AHEAD_USER_HOME/.ahead/settings.toml`
or `~/.ahead/settings.toml`, then workspace `.ahead/settings.toml`, tracked
`.ahead/config.toml`, and ignored `.ahead/config.local.toml`. Later sources
replace earlier profiles with the same `provider_id`; the last source defining
`ai.active_connection` selects the default. The Settings panel continues to
write workspace-private profiles and credentials using private, atomic file
replacement; writes reject symlink settings targets. The panel and managed MCP
approval writer hold the same workspace-local `settings.toml.lock` across
their read/merge/write operations, so concurrent edits preserve both AI and
MCP settings. Both writers explicitly unlock when that scope ends,
including on errors; dropping just one file handle can retain a Unix lock if
a concurrent fork inherited the same open-file description. Saves run on a
background worker, with one pending slot replaced by the latest edit. The
worker reads the current file under the lock before merging, preserving MCP
changes accepted by another writer. Only completion for the current workspace
and save generation updates the panel or reloads the model picker. Pending
and failed drafts are not replaced by an external reload.

Settings bootstrap and loading also run in the background. Provider fields,
add-connection and endpoint/model-discovery controls are disabled until the
initial load finishes, so empty loading values cannot overwrite existing
profiles. Workspace file-change notifications trigger a background timestamp
check and reload; rendering performs no Settings file reads. Reload requests
coalesce while another load or save is active. A failed draft defers reload
until its save succeeds, and late load results cannot replace a newer edit.

Normal window close and app quit wait for accepted Settings writes. A failed
save or a five-second wait timeout keeps the window open and reports the
problem; timeout does not cancel the write. Both paths recheck Settings and
buffer generations before closing, including after recovery acknowledgements.
Forced process termination is not covered. The separate model picker's initial
and callback-triggered configuration reads also run in the background, with
one active load and coalesced pending requests. The managed selector and Send
button stay disabled until the first load finishes. Enter preserves the draft
instead of sending with a loading placeholder, and skill discovery waits for
the model list. This does not block an external ACP session's own controls or
answers to an already-pending agent question.

Model reloads preserve the current provider/model identity when it still
exists, even if the user selects it after the read starts or its position and
display name change. Removing that model selects the first remaining choice;
no configured models means the explicit AHEAD runtime default. Reloads do not
replace conversation status or ACP/session state. A Settings completion forces
a read even with unchanged timestamps; watcher-only checks skip unchanged
files. Superseded results are not applied. Native verification remains in
`TODO.md`.

Provider reads in Settings, the model picker and FIM now share
an AHEAD-core reader that accepts only regular files under a real `.ahead`
directory, rejects symlinks and caps each file at 1 MiB. On Unix it opens the
directory and file without following symlinks and reads a bounded amount from
that descriptor; the non-Unix path still has a check-then-open race to close.
Settings identifies
invalid layers in its status text, and the chat model picker warns when a
layer is skipped. FIM fails the provider load and sends one deduplicated
warning through the proxy's `ShowMessage` notification, which the app routes
to the status bar. The model picker retains valid layers and clears its warning
when settings are repaired and reloaded. The proxy's existing recursive
workspace watcher notifies Settings and the chat picker when those three
files change, including while Settings is closed, without invalidating the
project search index. Settings checks timestamps in its background task and
tells the picker to reload after applying a changed configuration. The running-window
warning journey passed on 2026-09-29 in a disposable project: corrupting
`.ahead/config.toml` showed the chat warning with Settings closed, and repairing
it cleared the warning. Showing inherited user profiles as editable settings
and rejecting secret fields in tracked config remain open items in `TODO.md`.

## Filesystem placement

Instruction files follow the open `AGENTS.md` convention, and skills follow
the portable Agent Skills package format. AHEAD reads project instructions
only from `AGENTS.md`; it does not load Zed `.rules` files or other
editor-specific instruction formats. AHEAD uses these discovery roots:

1. `AGENTS.md` applies hierarchically from the discovered project root through
   the turn environment's working directory. The [open AGENTS.md convention](https://agents.md/)
   also supports nested files for subprojects and says the closest applicable
   file takes precedence when instructions conflict. AHEAD renders each loaded
   project source path with the SHA-256 of its complete bytes, even if prompt
   budgeting truncates the text, and tells the model to apply nearest-file
   precedence and surface unresolved conflicts. The retained core loads the
   project hierarchy from the discovered root through the turn working
   directory. In addition, managed AHEAD resolves the active editor path and
   explicit attached-file paths against the selected workspace, then loads
   nested `AGENTS.md` files along only those target ancestors. It does not
   crawl unrelated subtrees, and outside-workspace or symlinked targets are
   ignored. The target-scoped text and workspace-relative source labels with
   full-file SHA-256 hashes are included in that turn's first native prompt.
   This host-supplied prompt context is AHEAD's editor integration, not an
   extra requirement of the AGENTS.md convention. Each target instruction
   file is limited to 1 MiB and the combined prompt addition to 32 KiB; these
   are AHEAD budgets, not standard limits. This mirrors the documented
   root-to-working-directory loading shape used by Codex CLI while keeping
   project discovery grounded in the open repository convention.
   The model also receives bounded guidance to inspect applicable ancestor
   instruction files before effects. Neither that guidance nor the target
   prompt is effect enforcement: arbitrary paths appearing only in shell text
   are not preflighted. AHEAD persists these host-added target sources as
   workspace-relative paths, complete-file SHA-256 hashes and affected targets
   in Turso's `turn_instruction_sources` table, atomically with the turn
   request; the records remain after retry state is removed. This is separate
   from the retained core's root-to-working-directory instruction loader.
   The AHEAD-owned native/Turso loop regression asserts that project
   `AGENTS.md` content reaches the first model request and a colocated `.rules`
   sentinel does not.
   Explicit files attached from chat use AHEAD's `TurnContextFile` in
   `TurnEditorContext`; their in-workspace ancestor instructions are loaded
   for the managed native turn. The attached file's captured text remains
   labeled as fallible reference context, not instructions. Do not reinterpret
   retained `UserInput::Mention` as a file reference: it identifies explicit
   resource targets, including `mcp://` servers and legacy connector records.
   External ACP agents
   remain responsible for their own instruction discovery.
2. No user-global `AGENTS.md` location is defined by the open convention, so
   AHEAD does not require or load a private `~/.ahead/AGENTS.md` file. Normal
   managed chat gets project instructions from the discovered root to the turn
   working directory, then AHEAD adds target-specific ancestors for structured
   active/attached paths. FIM has no retained agent-core loader, so it receives
   the complete workspace-root-to-active-file/relevant-open-buffer hierarchy.
   Only applicable ancestors are loaded; the nearest one takes precedence for
   its target, and the user's explicit request remains authoritative. Codex
   CLI's documented `~/.codex` plus repository hierarchy is a product-specific
   implementation example, not a portable global path: see its official
   [AGENTS.md loading description](https://developers.openai.com/codex/guides/agents-md/).
3. Workspace skills are discovered from `.agents/skills/`; user skills are
   discovered from `~/.agents/skills/` (or `$AHEAD_USER_HOME/.agents/skills/`
   when the AHEAD home override is set). The Agent Skills client guide
   describes `.agents/skills/` as a widely adopted cross-client location; the
   format defines package contents, not discovery paths. AHEAD
   does not treat `.agent/skills/` or `.skills/` as application skill roots,
   and it does not download or install a discovered skill. This repository
   dogfoods that exact project/user contract with development and
   runtime-maintenance skills under `.agents/skills/`; the separately bundled
   product skills under `ahead-agent/skills/` do not change discovery paths for
   user projects. They are installed as system-scope content at
   `<runtime-home>/skills/.system/` (default
   `.ahead/runtime/skills/.system/`, overridden by `AHEAD_HOME`); this cache is
   not another project or user skill root.
4. Project memory lives at the ignored workspace
   `.ahead/memories/MEMORY.md`. User memory lives at
   `~/.ahead/memories/MEMORY.md` (or
   `$AHEAD_USER_HOME/.ahead/memories/MEMORY.md`). Workspace startup indexes
   both current files without copying one into the other. Native sessions do
   not inject memory automatically. The composer searches both current files
   through the per-workspace Turso/libSQL memory index; a search refreshes their
   content hashes before querying. Selecting a result attaches only that
   labeled excerpt (scope, conventional source label and line) to the turn.
   It is presented to the model as fallible user-selected context, not as an
   instruction. Only explicit memory-review commands attach a bounded snapshot.
   Model-visible instruction and memory source labels use
   conventional `~`/`.ahead` paths rather than host-absolute paths; the runtime
   keeps typed source provenance separate from that text. Memory is
   never injected into a turn merely because it matched a query. The Turso
   index stores an inverted token index per line and revision; queries deduplicate
   terms, rank lines matching more terms, support exact and token-prefix
   matches, and only search the current content hash. Index rows and the
   current-source pointer update transactionally. AHEAD follows the
   candidate-first separation used by Zed's pinned
   [`project_search.rs`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/project/src/project_search.rs),
   adapted to durable, human-readable memory files rather than editor buffers.
   Matching remains lexical: stemming and typo-tolerant search are not
   implemented. Idempotency marker comments are excluded from searchable
   tokens. `/` commands in the managed AHEAD chat can read one bounded memory
   snapshot (capped at 32 KiB) and attach its portable source label and content
   hash. That managed turn uses the native runtime's read-only filesystem
   profile. The model may propose one complete replacement between explicit
   markers. Only the user-facing message action applies it. The host rejects
   stale hashes and atomically replaces the file before refreshing the Turso
   search index.
   External ACP agents cannot invoke this snapshot/replacement command, though
   users may still explicitly attach search excerpts. A message context-menu
   action can also append a non-empty selected message to either scope. The
   user chooses the destination;
   the host limits each note to 8 KiB and rejects an append that would exceed
   the 32 KiB document limit. It appends only to the standard memory file,
   rejects symlinked AHEAD roots, memory directories and files before writing,
   deduplicates retries by conversation-message id and refreshes the local index
   after the write.

   A native smoke check in a disposable project on 2026-09-29 confirmed
   project-memory search, relative-path result labeling, excerpt attachment,
   chip removal, and search refresh after editing `MEMORY.md` outside the
   running app. It did not send a model turn or verify user-scope memory,
   append, or reviewed replacement in the rendered UI.

   The Markdown record is the human-readable authority. `.ahead/session.db`
   is the sole durable store for AHEAD sessions, visible conversation, managed
   agent replay history, and the local searchable memory index with immutable
   content-hash revisions. The separate `conversation_messages` and
   `agent_runtime_thread_items` rows preserve different UI and model-replay
   contracts in the same Turso/libSQL database. Native spawned-agent parent/
   child edges and their open/closed status live in `agent_spawn_edges` there
   too; the runtime thread and graph adapters open no other durable database.
   `harness_runtime_state` restores the latest plan, tool cards, token usage
   and advertised commands in the chat panel; it deliberately excludes
   ephemeral reasoning and pending user-input payloads.
   Checkpoint export includes the same `AgentRuntimeState` as a required
   `agent_runtime_state` field, and restore writes it into Turso. A session
   without an agent turn exports an empty projection. There is no fallback for
   older bundles missing that field. Checkpoints do not contain provider
   credentials, harness bindings or native replay history; importing one
   restores the readable session, not the original model conversation.
   The visible chat is a bounded projection: restore/refresh loads the newest
   50 messages with an exclusive `(sequence, message_id)` cursor backed by the
   `(session_id, sequence, id)` index, and older pages are loaded on demand.
   Returned pages stay chronological; prepending them uses the message
   scroller's anchor-preserving operation. This does not truncate or page the
   managed model's replay history. The shape follows Zed's separation of an
   ordered `Thread.messages` transcript and persisted UI scroll position
   (`crates/agent/src/thread.rs`, `crates/agent/src/db.rs`), while AHEAD bounds
   the rendered durable-chat window in Turso.
   Managed replay is Turso-only. New turns do not write Codex rollout files,
   and missing threads do not search, import or migrate JSONL from
   `sessions/` or `archived_sessions/`. Existing files are left untouched.
   The hard fork does not support old Codex/AHEAD storage formats. This
   supersedes the earlier JSONL migration plan.
   Before choosing resume-time dynamic tools, `NativeClient` reads the
   durable thread header through `TursoThreadStore`. Restored children do
   not receive the root-only `spawn_agent` tool. Child lifecycle edges are
   stored explicitly in Turso, not inferred from files.
   Memory writes remain explicit, user-approved work. Do not silently use
   Codex's home or a remote memory service as AHEAD memory.

   New managed threads default to `Paginated` history. A file-backed
   local-mock regression verifies that a child's earliest prompt reaches the
   model again after 71 turns and
   Turso reopen, across the 128-row replay-page boundary. This covers paginated
   child resume, not model-issued spawning by itself. The copied collaboration
   tool surface remains disabled. The AHEAD-owned `spawn_agent` path's
   model-issued spawn/resume, child-edit attribution and cancellation
   regressions are covered below; GPUI app-restart and visible child-selection
   validation remain open in `TODO.md`.

   The replay adapter preserves archive status, timestamp, provider and
   workspace path. Post-restart native listing reads thread headers,
   persisted timestamps and
   ordered metadata patches from Turso in two bulk queries, without reading
   replay rows. Full replay history hydrates when a thread is resumed or read,
   rather than during listing. This follows Zed's metadata-first list/detail split while
   retaining the Codex thread-creation and metadata-patch shapes for model-loop
   fidelity. Created/updated keyset listing pushes archive, source, provider,
   workspace, relation and title/preview search filters into Turso before its
   page limit; source, provider and workspace use the latest metadata patch,
   falling back to creation data. Recency, section and project listing still
   load all headers pending indexed summary data. Archive timestamps belong
   to the current schema; startup does not backfill old rows. The shared Turso store
   supports calls from the native Tokio workers; a file-backed integration test
   covers native thread reopen plus chat append/status writes from a Tokio
   worker, followed by database reopen. The
   `LocalThreadStore`, its 52-file implementation, and the `legacy-sqlite`
   feature are removed. The rollout SQLite backfill/listing path, queue-type
   exports, `codex-state` crate and root SQLx/SQLite dependencies are removed as
   well. `ProjectSortKey` stays because the normal-build project-list params
   still name it. The storage-neutral `ThreadStore` and `LiveThread` contracts
   remain for the retained core loop; AHEAD injects `TursoThreadStore` as its
   only durable managed-session backend. Guardian source markers still parse
   in the retained protocol, but AHEAD rejects them at thread-manager
   create/resume and direct delegate boundaries. These old-format markers
   are pruning work, not a supported history-compatibility contract. The
   unreachable Guardian-specific session setup, policy, world-state, remote
   MCP discovery, tool routing and prompt-schema branches have been removed;
   only fail-closed entry checks and legacy history/source handling remain.
   See `TODO.md` for the remaining marker removal.
   Shell-snapshot cleanup resolves
   rollout files directly. The copied SQLite agent-graph adapter and its DB
   tests have also been removed; Codex-core test managers no longer construct a
   persisted graph store. The native manager injects the Turso-backed adapter. A
   file-backed test verifies spawn-edge status survives database reopen and
   thread deletion, while a separate adapter test covers stable descendant
   ordering and status filters. The shared Turso reopen test also persists an
   open parent/child link and successfully resumes both native threads after
   restart. The model-issued spawn/resume sequence is covered by the
   file-backed regression below; a GPUI app restart and visible child
   selection/reopen check remain open in `TODO.md`.

   The editor's separate `ListSessions` route projects only id, title,
   lifecycle, creation/last-message time and harness binding in one Turso query. It excludes
   incomplete session rows; `GetSession` loads the full `SessionView` for an
   active or selected session. Work-item attribution resolves its owner by id
   rather than scanning full session views.

   The unused SQLite telemetry bridge was removed. The inert analytics crate
   no longer depends on `codex-state`; normal core, rollout and thread-store
   builds no longer enable it either. Codex `[analytics]` and profile toggles
   are removed, and its no-op sink no longer accepts provider credentials or a
   base URL. This does not remove `codex-otel`, which still carries trace
   propagation, metrics instrumentation and session log-privacy settings; do
   not infer an active exporter without locating its provider initializer. The
   unused standalone `prompt_debug`
   builder, which constructed a legacy local-state manager, was removed from
   the retained core.

The Codex-specific home, marketplace, plugin package and remote-task layout is
not an AHEAD placement standard.

## External ACP agent catalog

AHEAD uses the ACP registry as a distribution source, following Zed's
`AgentRegistryStore` and `AgentServerStore` lifecycle, but exposes only three
curated agents: Pi (`pi-acp`), Codex (`codex-acp`) and Claude Code
(`claude-acp`). Do not surface the full registry, arbitrary `agent_servers`
entries from Zed settings, or a free-form ACP command in the product UI. The
user explicitly installs or removes a supported agent in AHEAD; the selection
is stored under `~/.ahead/agents/external-acp/`, while registry metadata and
downloaded packages live in AHEAD's platform-local data directory. As in Zed,
the registry entry is enabled first and its package is installed on first
launch. Removing an agent removes it from AHEAD's picker; it does not delete
the cached package.

Npx installation uses a private staging directory under the selected adapter's
cache. After npm succeeds, AHEAD checks that the package executable is a regular
file inside the package, records package/lockfile hashes, and moves the directory
to a unique generation. An atomic `installed.json` replacement makes that
generation current. A launch resolves to the generation's own executable path;
later updates never rewrite that directory. A failed install leaves the
previous manifest and package available. An already-cached launch does not wait
for an updater holding the install lock, while competing installers recheck
the manifest after obtaining the lock.

ACP installation, registry refresh and saved-option writes explicitly unlock
their cache files when the operation ends, including on error. The lock file
stays in place so existing waiters and new callers use the same inode. A
fork-inherited descriptor cannot extend the completed operation's lock.

Old generations are retained because running Node processes can load more
files after startup. Lease-aware cleanup and recovery of abandoned staging
directories remain in `TODO.md`. The retired flat npm cache and provenance
file are neither read nor migrated; existing files are left untouched. There
is no backward-compatibility loader. Concurrency/failure tests use disposable
fake packages. The separate
`npx_generations_launch_locally_packed_packages_offline` smoke test uses real
npm to pack/install two local tarballs, then runs both with Node after the
update. It passed on 2026-09-30 with Node 24.15.0 and npm 11.12.1, using
temporary npm config/cache paths, offline mode and disabled lifecycle scripts.
This does not establish a registry download or rendered installation journey.

For external sessions, ACP's advertised session `configOptions` are the source
of truth for model, reasoning and other agent-specific selections. Render the
options advertised by the selected agent, send user changes through
`session/set_config_option`, and replace local option state from the response
and subsequent `config_option_update` notifications. Do not show the managed
AHEAD provider-model list as if it controlled an external ACP process. ACP
adapters own their own tool, shell, file and permission effects; the managed
AHEAD lifecycle is not enforced across that process boundary.

The last accepted value for each config option is saved per agent in
`~/.ahead/agents/external-acp/<agent-id>.config-options.json`. Apply a saved
default only when the connected agent still advertises the option and, for a
select, the saved choice. Apply defaults when creating or restoring a session;
an explicitly requested session model takes precedence. These files contain
agent option values only, not provider credentials.

## Code attribution lifecycle

Managed-session anchors are pre-commit attribution. The Turso `anchors` table
drives the AHEAD gutter until a commit lands; after the host verifies that the
quoted, hashed text is present in committed content, it removes that anchor and
Git blame becomes authoritative. The schema does not retain a commit marker.
Old anchor schemas are unsupported; startup does not drop or convert their
rows. External ACP edits do not receive managed CodeAnchors.

## Session storage decision

Zed is the product reference for session behavior: its
[`ThreadMetadataStore`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent_ui/src/thread_metadata_store.rs)
keeps lightweight sidebar metadata (identity, title, created/updated time,
agent and workspace paths) apart from the conversation body, and sorts by
recent activity. AHEAD's `SessionStore::list_sessions` likewise projects
sidebar metadata without loading each `SessionView`; the app loads detail on
selection. `TursoThreadStore::list_threads` reads header and metadata-patch
rows without replay history. Metadata-only reads stay header-only; legacy
resume/full-history callers hydrate the complete replay. Paginated resume uses
reverse keyset pages over `(thread_id, ordinal)` and Codex's `ModelContextScan`
to stop at a compatible completed-turn context and valid compaction checkpoint.
If no safe cutoff exists, it continues to the beginning and returns the scanned
history. Paginated full-history APIs remain unsupported; AHEAD's visible
conversation uses its separately paged message store.
The retained Codex
[`CreateThreadParams`](../../ahead-agent/runtime/source/thread-store/src/types.rs)
and [`RolloutItem`](../../ahead-agent/runtime/source/history/src/lib.rs),
ordered replay history and parent/fork IDs are useful *runtime* shapes because
the managed loop must resume, compact and spawn correctly. They do not dictate
AHEAD's database layout or a second Codex-owned session store.
The managed composer exposes `/compact` through the retained loop's explicit
compaction operation; external ACP agents do not receive this host command.

Workspace close and proxy stdin disconnect stop the session controller before
the proxy exits. The controller rejects new work, marks active turns cancelled,
stops their runtimes and waits for workers to persist the final message status.
Partial text and the saved retry request remain; shutdown does not delete the
conversation. The native client uses the retained thread manager's concurrent
shutdown-and-wait operation, including child threads, with a five-second budget.
New thread creation and MCP reconfiguration cannot race past its thread snapshot.
Pending editor/input requests are released. External ACP shutdown settles pending
requests before killing and reaping its direct child, including requests awaiting
initialization. This does not guarantee termination of an adapter's descendants.
The controller allows six seconds for turn settlement; timeout or persistence
failures are logged and are not evidence of a successful flush. Native close/quit
with authenticated agents and blocked storage still needs verification.

In AHEAD's Turso/libSQL `.ahead/session.db`, `sessions` owns the visible work
session and policy; `harness_bindings` links it to a managed thread or external
ACP session. `conversation_messages` owns readable UI messages and turn status.
`agent_runtime_threads`, ordered `agent_runtime_thread_items`, metadata patches
and `agent_spawn_edges` own reconstructible managed-loop state. UI text is not
used as a substitute for model replay, and replay items do not become a second
user-facing conversation. The local database uses application ID
`0x41484544` (`AHED`) and schema version `2`. A new empty database receives
the complete schema and both markers in one transaction. Database layout
changes must advance `SESSION_SCHEMA_VERSION`; startup rejects older versions
instead of upgrading them. Opening a current database does not run DDL or
row backfills. Nonempty unversioned databases,
other application IDs and other schema versions are rejected without changing
the file; there is no migration, reset or automatic archive step.

On Unix, new database files are created with mode `0600` before Turso writes
them. The database directory must be owned by the current user and must not
be group/world writable. Symlinks, hard-linked files, special files and
unexpected owners are rejected for the database and its `-journal`, `-wal`
and `-shm` sidecars. Orphaned sidecars are kept for recovery; startup does not
silently pair them with a new empty database. A supported database and its
existing sidecars are tightened to `0600` after schema validation, without
rewriting their content. New sidecars inherit the main file's mode through
Turso's bundled VFS.

Existing stores first get a read-only, immutable main-header check through
Turso. Schema markers never change during a supported store's lifetime in
this hard fork, so this check does not need to replay journals or create
shared memory. Rejected stores keep their bytes and permissions. A supported
store then opens normally and can recover an interrupted write. Permission
updates use the checked parent directory descriptor, without opening and
closing raw database descriptors that could release another local
connection's POSIX locks. This does not isolate programs running as the same
user. Windows ACL enforcement and inherited ACL behavior remain unverified.

A workspace storage error disables agent-session requests and displays the
cause. AHEAD does not substitute an in-memory workspace session. File editing
and LSP remain available; projectless initialization may use an explicit
in-memory host. Keep old database files intact. Use a matching old build to
read them, or choose a new disposable project for development.

Unsaved editor buffers use the same database, in `editor_recoveries`, not a
second database or a directory of text copies. Each row has a buffer UUID,
workspace-relative path, increasing revision, optional dirty text and the
SHA-256 digest of its saved baseline. A saved/discarded row retains its revision
with null text so delayed writes cannot resurrect it. Each proxy holds an OS
file lock in ignored `.ahead/editor-leases/`; a new proxy can claim another
owner's rows only after acquiring that owner's abandoned lock. Live windows
cannot take each other's recovery rows. The owner explicitly unlocks when it
is dropped; temporary claim leases also unlock on success or error. A retained
duplicate descriptor cannot extend either lease beyond its intended lifetime.
Old lock files remain in place to
avoid unlink/reopen inode races; safe cleanup is still tracked in `TODO.md`.

Startup lists recovery metadata, then loads each buffer separately. Restore
does not write source files or replace a currently dirty tab. A changed saved
baseline requires an explicit overwrite decision before saving the restored
text. This is not general external-change detection or an atomic compare-and-
swap save. Only acknowledged snapshots are durable; the 250 ms editor polling
loop and best-effort shutdown flush can leave recent edits unpersisted.
Snapshots are limited to 32 MiB per buffer. Untitled buffers and full workspace
layout/caret restoration are not implemented. Schema 1 databases stay intact
and are rejected, with no migration or compatibility path.

Retain Codex's serialized replay shapes only at the runtime adapter boundary. Old JSONL files are not an import format. Unlike Zed's compressed
whole-thread blob, the append-only replay rows let a turn become durable
without rewriting the whole history. The visible session list derives its
`updated_at` from indexed conversation-message timestamps (falling back to
creation time) and reopens the most recently active session; it does not
duplicate a mutable timestamp in the session row. Native-thread listings use
indexed keyset pages for created/updated ordering, archive,
source/provider/workspace, search and relation filters, loading metadata only
for those page rows. The source/provider/workspace predicates honor their
latest metadata patches before limiting results. Recency/section/project
listing still uses the complete header path until those fields have indexed
summary data; see `TODO.md` for the remaining scale-up and verification work.

One `SessionStore` currently shares one libSQL connection across editor and
agent callers. Access to that connection is serialized so concurrent agent
transactions cannot interleave. Separate read connections are an optimization
only if measured contention warrants them.

MCP standardizes the client/server protocol, not a universal host-side server
configuration filename, directory, or settings shape. Zed's pinned
`ProjectSettings` uses its own `context_servers` map and extension sources;
that format is editor-specific. AHEAD's proposed settings are AHEAD-owned and
map to MCP's standard transports. They do not promise compatibility with Zed's
configuration or auto-start behavior.

The current MCP specification is `2026-07-28`; that modern protocol removes the
`initialize` handshake and requires per-request version metadata plus
`server/discover`. Zed's current context-server implementation still targets
the legacy handshake family through `2025-11-25` (`LATEST_PROTOCOL_VERSION` in
`../zed/crates/context_server/src/types.rs`). AHEAD's ACP editor bridge exposes
only the MCP tools feature. Its `mcp/message` route is an optional experimental
ACP extension, not part of the stable ACP v1 protocol; see the
[ACP `MessageMcpRequest` schema](https://agent-client-protocol.github.io/typescript-sdk/types/MessageMcpRequest.html).
Do not make external-agent interoperability depend on that route. The baseline
is ACP's standard `mcpServers` session configuration with the editor bridge's
stdio transport, following Zed's `mcp_servers_for_project` and its shared
new/load/resume session-request builders in `../zed/crates/agent_servers/src/acp.rs`.
Both AHEAD transports currently support the `2026-07-28` MCP request envelope
(`server/discover`, per-request version and client capabilities, `resultType`,
and response `serverInfo`) and Zed's legacy handshake versions through
`2025-11-25`; unknown legacy offers select Zed's latest legacy version. The
bridge does not claim MCP resources, prompts, subscriptions or other
unimplemented features. The stdio bridge reads input independently from tool
workers, supports up to 16 pending `tools/call` requests, and handles standard
`notifications/cancelled`. Each call receives an opaque host request id over
the authenticated local bridge; cancellation settles a pending editor
presentation or open-buffer snapshot. A bounded 30-second tombstone handles a
cancel that reaches the host before its tool request, and completion responses
are suppressed after cancellation, including across JSON-RPC id reuse. This
follows MCP's cancellation behavior for stdio in both legacy and modern
protocol versions; see the [MCP SDK's 2026-07-28 compatibility guidance](https://ts.sdk.modelcontextprotocol.io/v2/migration/support-2026-07-28).
Stdin and local socket request reads consume at most 1 MiB plus one byte per
frame; that extra byte detects an oversized frame without waiting for newline
or EOF. Stdin stops after oversized or invalid UTF-8 input. This host limit is
not an MCP protocol requirement and does not bound the event queue, socket
worker count or responses; those limits remain in `TODO.md`.
The optional ACP `mcp/message` route remains experimental and is not part of the
interoperability baseline. This wire subset is not a complete MCP
implementation. Host tests cover cancellation of editor presentations and
buffer snapshots, cancellation-before-registration, JSON-RPC id reuse,
fragmented socket input and bounded frames. The full agent library suite
passes 107 tests, with two opt-in tests ignored (2026-10-01). Restricted runners
may need permission for an ephemeral `127.0.0.1` bind. Interoperability with Pi,
Codex and Claude remains unverified; require protocol fixtures and adapter
probes before claiming production interoperability. The configured user MCP
path is separate from this AHEAD-owned editor bridge.

**Managed MCP configuration (AHEAD-owned settings; MCP wire protocol):**

- Put shareable, non-secret server declarations in tracked `.ahead/config.toml`
  under `[mcp.servers.<id>]`. AHEAD currently supports local stdio servers
  using `command` and optional `args`; the transport is inferred from those
  fields, so do not add a `transport` key. The MCP wire protocol also defines
  Streamable HTTP, but AHEAD does not enable it until its authentication and
  secret-storage path is owned and verified. This file shape is AHEAD-owned,
  not defined by MCP or compatible with Zed's `context_servers` settings. A
  declaration alone never starts a server, even if it contains `enabled = true`.
- Explicit opt-in is workspace-local in ignored `.ahead/settings.toml`:
  `[mcp]` with `enabled_servers = ["docs"]` plus the declaration's
  `sha256:` fingerprint under `[mcp.approved_declarations]`. AHEAD hashes the
  parsed declaration, so comments and formatting do not affect approval, but
  command, args, environment references and other field changes invalidate
  it. Missing or stale fingerprints fail closed and the error supplies the
  current value to approve after inspecting the tracked declaration. Existing
  ID-only opt-ins must be reapproved; they are not silently migrated. Unknown
  server IDs are configuration errors. AHEAD intentionally does
  not use `~/.ahead/settings.toml` to start project commands across unrelated
  workspaces. Literal stdio `env` values are rejected; use `env_vars` to refer
  to local process environment variables. MCP OAuth and app-managed secret
  storage are not verified. Opting in launches the configured command with the
  user's OS permissions; server startup and calls are not sandboxed by AHEAD.
  The managed agent cannot use native tools to read or write workspace or
  user-home `.ahead` files (including `$AHEAD_USER_HOME` when set), even in a
  workspace-write or Learn session;
  host-owned settings, sessions and memory
  operations remain separate. External ACP agents do not inherit that sandbox.
  Each MCP TOML file is limited to 1 MiB and must resolve to a regular
  in-workspace file; Unix reads open the resolved path through no-follow
  directory handles, including its parent components. On Unix, approval opens
  one no-follow `.ahead` directory handle and uses it for the owner-only lock,
  declaration/settings reads and temporary-file rename. Replacing the `.ahead`
  pathname with a symlink after that open cannot redirect the write. The
  non-Unix approval fallback remains path-based and needs an equivalent
  boundary before claiming cross-platform symlink-race safety.
  The Settings panel lists the tracked declaration, its fingerprint and local
  approval state, and can enable or revoke it after review. Approval is
  checked again against the current declaration by the proxy and applies to
  new managed sessions; disabling it does not stop an already running server.
  The rendered approval journey has not yet been verified.

For example, the tracked `.ahead/config.toml` declares a local server while
the ignored `.ahead/settings.toml` opts into it:

```toml
# .ahead/config.toml
[mcp.servers.docs]
command = "docs-mcp"
args = ["--stdio"]
```

```toml
# .ahead/settings.toml
[mcp]
enabled_servers = ["docs"]

# Replace this with the sha256: value AHEAD reports after inspecting the
# current declaration. A changed declaration requires a new approval.
[mcp.approved_declarations]
docs = "sha256:<current declaration fingerprint>"

# Optional workspace-local choices; unspecified tools still prompt every call.
[mcp.tool_permissions.docs]
search = "allow"
delete = "deny"
```

- AHEAD forces the effective server default and every per-tool approval mode
  from tracked declarations to `prompt`. The ignored, workspace-local
  `[mcp.tool_permissions.<server>]` table may set named tools to `allow`
  (skip review), `deny` (do not expose the tool), or `confirm` (review each
  call). Unlisted tools still prompt every call. These are tool-name choices,
  not argument patterns or a sandbox around the MCP server; opting in starts
  the server with the user's OS permissions. Managed Assist sessions apply
  these choices; Learn/read-only
  sessions remove MCP servers. The external ACP adapter does not receive these
  user-configured servers because AHEAD cannot enforce its review policy over
  an agent-owned process. Zed's
  `../zed/crates/agent/src/thread.rs::authorize_third_party_tool` and
  `../zed/docs/src/ai/tool-permissions.md` remain the reference for one-time
  and persistent choices made from the review UI. AHEAD currently reads local
  choices from settings, but the review UI cannot save an "always" choice yet.
  The managed MCP question is single-select in chat; the native client and
  retained approval parser reject conflicting answers instead of treating a
  mixed Allow/Cancel response as approval.

Requests that reach the reviewer now go to the human after explicit
permission hooks; the disabled Guardian review executor and per-app Codex
reviewer override are removed. Legacy `approvals_reviewer` and
`required_on_models` selector fields are ignored rather than normalized:
there is no reviewer enum or selector in config, profiles, managed
requirements, analytics, MCP policy or runtime state. Old session JSON
remains readable because unknown reviewer fields are ignored. The
model-specific required-review constraint is removed; managed
`auto_review.ignore_rules` remains because it removes executable prefix
allow-rules for selected models. Legacy Guardian session-source markers and
tool-filter branches still remain, but are queued for removal under the
hard-fork decision. Current AHEAD restore and permissions must stay tested.
Legacy `persist=always` metadata is reduced to approval of that call, and the
copied runtime no longer writes approval rules into Codex user or project
config.
The external ACP adapter's separate AHEAD-owned editor bridge is limited to
reading open worktree buffers and editor presentation actions; it is not a
user-configured or model-declared MCP server. Do not add a marketplace, remote
plugin manager or Zed extension source to this contract.

The managed bootstrap loads only workspace-declared MCP servers selected by
the ignored workspace opt-in, applies local tool permissions with human review
as the default, and
refreshes the server set when the session enters or leaves read-only mode. ACP
sessions receive no user-configured MCP servers; when its local bridge is
available, AHEAD passes the external agent a session-scoped `ahead-editor` MCP
process for buffer reads and presentation actions. The
managed native bootstrap also skips the platform Codex system-config layer and
Codex project-config discovery; the focused loader regression verifies that an
ignored system file contributes no configuration. The retained loader no
longer infers a system-config file from Codex platform paths; an AHEAD host must
explicitly supply a path before that layer can load. System requirements are a
separate layer and are not disabled by this config-file opt-out. Its
credential-broker-specific
project trust, provider-environment binding, requirements path, core conversion
fields and broker-only copied tests have been removed; project-local
`features.network_proxy` is discarded because AHEAD disables that feature. The
broader fail-closed proxy compatibility surface and copied call sites remain
cleanup work.

## Ownership and reuse

Zed references are the editor-side source for worktree snapshots, ignore-aware
search, path matching, progressive skills and agent-panel interaction. The
retained Codex sources are the reference for the model-facing turn loop,
streaming, cancellation, compaction and provider request details. Reuse stops
at the editor boundary: file search, file reads/writes, buffer context,
instructions, skills, memories and approvals must enter through AHEAD-owned
services so the native harness and the UI do not grow overlapping tools.

The old Codex fuzzy file-search crate is deleted. AHEAD's shared ignore-aware
matcher is the only workspace content-search implementation. Following Zed's
[`project_search.rs`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/project/src/project_search.rs),
the panel overlays eligible open unsaved editor tabs and proxy search overlays
its open buffer snapshots before falling back to disk. All three search callers
now use one workspace-bound rule for unsaved new buffers and search those before
the disk walk; existing ignored files still follow the worktree's ignore rules.
The shared walker applies `.gitignore` even when the opened folder is not a Git
repository, matching the editor's project-search expectation.
The shared index caches visible directories as well as files so watcher events
can ignore unrelated paths. Invalidating a changed worktree drops both
snapshots; until the next walk, events are conservatively treated as relevant
so a newly created nested directory cannot be filtered by stale parent state.
Unlike Zed, AHEAD does not yet have a multi-buffer project search stream. Its
panel and native-agent path filters now accept comma-separated patterns,
preserve commas inside brace globs, and treat literal directory names as path
prefixes; the panel offers include and exclude fields while the agent tool
offers an include argument.
The native agent's `file_search` requests current open-buffer snapshots from
the GPUI editor at tool-call time and overlays them on disk search; unsaved new
files under existing workspace directories can participate. This bridge uses
the shared AHEAD matcher, not a second agent search implementation. It still
needs a live tool-call smoke test. Snapshot replies are capped at 128 buffers
and 8 MiB of path/content bytes; an oversized reply fails the tool explicitly
instead of silently falling back to stale disk content. Snapshot paths are
workspace-relative, symlink escapes and private files are excluded, the
managed scope is checked before searching, and an unanswered editor request
fails the tool rather than silently searching stale disk contents. Agent matches
come in 20-match pages with an opaque next cursor bound to the query,
open-buffer contents, current search revision and emitted-result prefix.
Relevant content-only watcher events advance the search revision without
rebuilding the cached path list; file-set and ignore-rule changes advance it
while invalidating that list. A stale cursor fails with a restart-from-first-page
error instead of skipping or duplicating results; recreating the workspace
index also invalidates prior cursors. Numeric offsets remain accepted only for
older saved conversations. Dropping the native tool future
cancels an in-progress content scan and its cold path-index walk; the
panel's request-scoped cancellation also stops a superseded, closed or timed-out
`WorkspaceFiles` walk. This search applies Zed's
default private-file patterns (`.env*`, `*.pem`, `*.key`, `*.cert`, `*.crt`,
`secrets.yml`) and excludes workspace-private `.ahead/` files before searching,
independent of gitignore. Explicitly open
unsaved files may still appear even when gitignored, as in Zed's
entryless-buffer search. The private-file filter applies only to this search
tool; other agent tools and external ACP adapters have separate effect
boundaries.
Following Zed's
[`ProjectSearchView`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/search/src/project_search.rs),
the panel exposes case, whole-word and regex toggles and waits 250 ms after
the last query change before starting a scan. The shared matcher treats both
LF and CRLF as line breaks for anchored regexes and streams matching files to
the panel through a bounded channel, so results appear during the scan. The
panel shows when its 500-match limit is reached. Superseded scans stop through
a generation check. Clicking a match opens the
file at that match, converting the searcher's UTF-8 byte column to the live
editor's character column. Workspace enumeration also excludes symlink
entries, so search does not follow a link outside the root.
Directory entries are sorted so offset-based pages remain stable while the
worktree is unchanged. The matcher stops at NUL bytes in binary files.
Proxy `GlobalSearch` and the managed agent reuse one lazy path snapshot
invalidated by the proxy's filesystem watcher on file-set and ignore-rule
changes. The panel obtains a generation-tagged copy through RPC, caches it
across queries and refreshes on `WorkspaceFileChange`, including external
content modifications without rebuilding the path snapshot; without a proxy it
falls back to a fresh walk. AHEAD does not yet have Zed's concurrent candidate
search. Concurrent cold searches now wait for one shared path-snapshot build
instead of walking the same worktree per caller. It also lacks Zed's binary
detection beyond NUL, non-UTF-8 decoding and configurable private-file rules.
After a snapshot exists, watcher events under ignored trees are excluded from
search invalidation using that snapshot's visible entries. The first walk can
still be invalidated by raw events, and the proxy still runs Git diff refresh
for ignored-tree events.
The next architectural step is exposing the same snapshot to Explorer. Live
watcher delivery and large-worktree performance still need verification.
Zed's native [`GrepTool`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/tools/grep_tool.rs)
delegates to `Project::search` and consumes a cancellable result stream; the
AHEAD buffer-snapshot IPC bridge is interim until the editor-owned search
service offers that same shared project view to the managed loop.
Zed's separate [`file_finder.rs`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/file_finder/src/file_finder.rs)
ranks path candidates from the maintained worktree index. AHEAD's shared
`ahead_core::search::rank_file_paths` now ranks the proxy's workspace snapshot,
and `ahead-app/src/quick_open.rs` exposes it in a `Cmd+P` GPUI modal with
current-directory affinity and match highlighting. Rendered focus, keyboard,
open and file-watcher behavior still need live GPUI verification in `TODO.md`.
Rollout recovery uses a plain filename walk because it is not an editor search
surface. The unused Bedrock provider and AWS SDK/auth closure are also removed
from normal builds; custom gateways continue through the generic provider path.

The 2026-09-30 hard-fork decision does not require backward compatibility with
retired Codex features, settings or history formats. Remove unused compatibility
code and its tests. Current AHEAD session restore and permission enforcement
remain requirements; pruning code does not authorize deleting user data.

The native path has no Codex App Server process or copied App Server protocol
crate. The latter was only holding unused turn, item and timeline projections
in the generic thread-store API; AHEAD persists model-loop replay directly
through its Turso-backed store. There is no AHEAD marketplace, remote plugin
   manager or Guardian reviewer. Marketplace catalog fetches,
recommendations, install tools, plugin prompt injection, plugin skill roots,
plugin hooks and selected-plugin MCP contributions are removed from the native
turn path. The copied plugin manager, marketplace, remote, store and sync
implementation and old core-plugin facade are deleted. Command attribution and
local tool records now live in the AHEAD-owned `ahead-tool-records` crate.
The write-only connector-selection cache and app-ID collection from prompts
and skill bodies are also removed, along with the final core-plugin mention
module. MCP readiness reads explicit `mcp://` targets through the shared
mention parser. Skill selection still honors exact paths, disabled packages
and ambiguous-name checks; dormant connector names no longer participate.
Legacy `app://` links remain excluded from skill selection and do not activate
connectors. This follows Zed's separation between registered MCP server tools
and agent context in `crates/agent/src/tools/context_server_registry.rs`;
no Zed source was copied for this cleanup.
Inert plugin-measurement and artifact-operation telemetry, the sidecar's
analytics facts, additional-permission merge and shell environment hooks are
removed; this does not alter the user's requested command permissions or
sandbox policy.
The retained core still compiles hosted-Apps tool formatting, approval metadata
and file-upload rewriting that need removal; none is a compatibility
requirement. AHEAD rejects `codex_apps` as a workspace server name.
Automatic hosted-server registration
and its endpoint/config factory are removed, along with the product headers,
connector-token environment lookup and MCP-only originator plumbing.
The Apps feature flag, its `connectors` alias and the removed Apps path-override
parser/schema are deleted.
Standalone connector discovery, its process-global app-list cache, app metadata
types and the core connector helper are removed. There is no Apps instruction
generator, world-state section, model guidance flag or old Apps context-tag
handling. Unused Apps config and managed-requirement types still need removal
from the copied config crate. The retired product-SKU setting is also
removed, including its dedicated compatibility test and denylist entry.
Hosted Apps auth-failure parsing, ChatGPT sign-in elicitations and synthesized
retry results are also removed. The Apps-only forced-refresh chain, override
cache, lock, feature flag and copied fixtures are gone. Standard MCP tool
results still pass through the retained result processing and sanitization.
Generic server approval and environment restrictions are unchanged.
The MCP `auth = "chatgpt"` selector and model-account credential injection are
removed, including the HTTP client's provider hook and account/token fields in
the transport identity. MCP transport authentication uses explicit server
bearer/header credentials or server-specific stored OAuth credentials. Model
provider authentication is unchanged. AHEAD still rejects workspace HTTP MCP
declarations until the auth UI is implemented; this cleanup does not enable
them. Zed's `crates/project/src/context_server_store.rs` is the reference for
server-owned auth challenge handling and session storage.
The Apps-only startup cache, disk persistence, reconnect/backoff state and
cached server metadata are removed. MCP no longer subscribes to model-account
changes or stores model credentials in its published runtime. Explicit config
and environment changes still refresh MCP state, and server-specific OAuth
recovery remains. The ordinary in-memory per-server tool cache in
`codex-mcp/src/tool_catalog_cache.rs` still supports bounded startup discovery;
tool execution binds to the exact ready client and current permissions.
The Apps policy evaluator and `ahead-mcp-state` package are deleted. MCP tool
exposure now uses the published server catalog and model-visibility metadata;
approval uses the prepared call's per-server policy. The hosted-widget resource
reader, event observer and widget provenance checkpoints are also removed from
runtime state, compaction and serialization. Standard MCP resource reads and
AHEAD's Turso session replay remain. The remaining hosted-Apps metadata,
file handling and event registration are tracked in `TODO.md`.
There is no backward-compatibility requirement for retired Codex features or
settings, and pruning their source does not authorize deleting user data.
For new managed Assist sessions, AHEAD projects only explicitly approved
workspace-local stdio MCP declarations from `.ahead/config.toml` and
`.ahead/settings.toml` into the native runtime. Learn/read-only sessions receive
none. The settings-to-runtime path has focused tests, but the rendered approval
and tool-call journey remains unverified. Conventional host skills remain
available through the documented discovery roots.
Skill discovery has one host implementation: flat packages under project
`.agents/skills/` and user `~/.agents/skills/`, with project names taking
precedence. The copied executor-environment and MCP-orchestrator skill providers
and plugin namespace loaders are deleted. The retained exec-server's separate
selected-root discovery only materializes bounded `SKILL.md` files; it does not
parse plugin manifests, MCP declarations or `agents/openai.yaml` sidecars.
The native bootstrap also disables Codex product features for code mode,
web search, multi-agent routing and MCP apps. Four Guardian-only feature keys
are no longer runtime features; old config values are accepted and ignored.
The copied managed
network proxy, MITM, SOCKS, DNS, certificate and credential-broker engine is
deleted. A small fail-closed compatibility surface remains only for serialized
sandbox/exec-server records and copied call sites; attempts to start it error.

Keep both copied `Collab`/v1 and `MultiAgentV2` disabled. AHEAD's native
`spawn_agent` dynamic tool follows Zed's
[`SpawnAgentTool`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/tools/spawn_agent_tool.rs)
by creating or resuming a distinct child thread; Zed's
[`NativeThreadEnvironment`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/agent.rs)
persists parent/depth in `SubagentContext`, with round-trip and recursive
deletion tests in
[`db.rs`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/db.rs).

AHEAD advertises `spawn_agent` only to root sessions; child depth is capped at
one. Child turns inherit the parent's workspace, model/provider, mode and
permission scope. `NativeClient::consume_turn` drives the actual child
`CodexThread`, sends tool responses to that child, and routes its editor-context
requests and file-change notifications through the owning parent work session.
Child assistant text is returned as the tool result instead of being presented
as duplicate parent chat. `ThreadManager::spawn_subagent_session` reserves the
child ID and persists its Turso parent edge before startup; the native client
reopens/closes that edge around each child turn and interrupts loaded descendants
when cancelling the root.

A file-backed regression now scripts model-issued spawn, 71 child turns,
client/Turso reopen, and model-issued resume by child ID in
`native_agent_model_issued_spawn_and_resume_survives_turso_reopen` under
`ahead-proxy/src/ahead/store.rs`. It passes with
`cargo test --locked --offline -p ahead-proxy
native_agent_model_issued_spawn_and_resume_survives_turso_reopen -j 1`. The
fixture uses an offline mock model on `127.0.0.1` and runs its test body with an
8 MiB stack; restricted runners must allow loopback binding. The
`native_agent_child_file_edit_records_parent_work_session_anchor` regression
now drives a model-issued spawn and child `exec_command`/`apply_patch` through
`AheadSessionHost`, then verifies the edit's Turso `CodeAnchor` belongs to the
parent work session. It passes with
`cargo test --locked --offline -p ahead-proxy
native_agent_child_file_edit_records_parent_work_session_anchor -j 1`. The
fixture canonicalizes its temporary workspace for the macOS sandbox. A focused
session test also checks anchor actor, path, range and quote hash, while
`native_agent_cancel_interrupts_an_active_child_before_the_parent` verifies
that cancellation interrupts an active child and closes its Turso edge. The
spawn/resume/edit/cancel regression gates pass; the broader managed-agent
production journey still needs a real turn and rendered editor validation.

The managed authorization policy is a small AHEAD allowlist and sandbox, not a
per-tool approval UI. Learn is read-only. In Assist, the user's instruction
authorizes the requested work; writes default to the active workspace, while
an explicit non-empty path scope narrows them. An absent or empty path list
means the workspace root, not a failed authorization. Out-of-workspace paths
and unsupported escalation fail closed. There is no Guardian/model reviewer
or per-edit proposal card. This follows Zed's host-side permission decision
boundary in `../zed/crates/agent/src/tool_permissions.rs`; a model requirement
does not switch AHEAD to a second model reviewer. External ACP authorization
remains adapter-owned and is labeled as such.

MCP server elicitation has no AHEAD review UI yet. `native_client.rs` declines
`EventMsg::ElicitationRequest`; it does not display server-requested forms or
URLs. Managed config keeps the existing empty elicitation capability and does
not advertise URL support. Zed's user-event handoff
(`../zed/crates/agent/src/agent.rs`, `ThreadEvent::Elicitation`) is the reference
for implementing that UI, not evidence that AHEAD has it. MCP client
auto-accept/decline behavior and strict-request rejection remain. Tool-call
authorization is separate: managed MCP calls use AHEAD chat questions, whose
rendered end-to-end verification is still open.
AHEAD does not emit Codex `approvals_reviewer` response metadata.

The Guardian transcript/context crate, second pre-compaction history copy,
model reviewer, prompt assembly, metrics, review-session cache and denial
circuit breaker are removed. Its unused analytics event model and orphan
plugin-install/external-import telemetry facts are also deleted; no event is
sent or persisted. The extension API no longer exposes approval-review
contributors or their assessment/input/error DTOs; serialized Guardian
assessment events remain in the copied protocol and are scheduled for
removal, not supported legacy history. The uncalled root
snapshot/version API, Guardian answer-evidence cache and dead AgentControl
provider are removed; ordinary `request_user_input` keeps returning the same
serialized host response. Approval contexts, request DTOs and formatters now
live under AHEAD's `core/src/approval` module; copied tool call sites still use
that compatibility seam. AHEAD has no configuration-routed automatic reviewer;
the retained strict-legacy review handler denies rather than approving.
Approval timeout messages use fixed AHEAD-owned text. The `ModelMessages`
instruction schema does not expose model-catalog auto-review policy or
reviewer-specific approval text; stale upstream catalog fields are ignored and
the bundled catalog's long review policy is removed. The unused
`ModelInfo.auto_review_model_override` field and bundled values are removed as
well. The separate `node_repl_auto_review_required` and `node_repl_disabled`
fields still flow to MCP request metadata and remain until that compatibility
path is removed or replaced. The catalog on-request message is shared, and
permission prompt assembly no longer has a reviewer-selection branch. Legacy
automatic-review config values normalize to user approval.
The retained inference client and API wrapper now use only `/responses`; the
dedicated Guardian and classifier routes and the `free_guardian` config switch
are removed. The dormant Guardian v2 feature/config schema is removed too. The
four Guardian-only toggles are absent from the feature registry and remain only
as ignored compatibility keys for old config files. The
model-instruction schema and bundled catalog no longer carry Guardian policy
instructions or reviewer-specific approval text. The command-source enum used
by the retained approval serialization path is now the generic
`ApprovalCommandSource`. Approval contexts, action DTOs, annotation types and
formatters now live under AHEAD's `core/src/approval` module. Strict legacy
review remains fail-closed. Serialized `GuardianAssessment*` events and the
legacy `guardian` session-source marker still need pruning. The legacy
`guardian_approval` feature key and `auto_review` feature requirements are
ignored; model-specific automatic-review selection is absent from native
session startup, step changes and MCP reviewer selection. Explicit legacy
automatic-review requests continue to fail closed.
Guardian-specific downstream session-policy/prompt branches are removed;
removal of old-format source markers and event DTOs remains tracked
in `TODO.md`.

The copied browser/device login server, OAuth callback UI, hosted success pages,
PKCE login, account storage, token refresh/revoke, JWT handling, account
keyring, workload identity and Agent Identity code are removed. The separate
Codex enterprise-managed MCP identity/ID-JAG integration and its private
`ema-idp:` credential namespace are also removed because AHEAD has no caller
for them. `ahead-model-auth` is the AHEAD-owned model-request seam: it accepts
bearer/header credentials from the selected `.ahead` provider, supports an
optional provider-owned token command, and supplies shared HTTP clients. It has
no product account or login surface. Standard MCP OAuth remains separate from
model authentication. The retained `codex-rmcp-client` Streamable HTTP branch
still compiles `codex-keyring-store` and `codex-secrets` for that protocol path,
but AHEAD's current workspace configuration rejects HTTP and does not invoke
this auth flow. This transport/auth boundary is tracked in `TODO.md`; Zed's HTTP
transport and OAuth code are references, not proof that AHEAD supports them.

The copied Codex memory generation/tool pipeline is not present in the hard
fork. AHEAD reuses its useful instruction and citation shapes, but keeps memory
access explicit: the editor can append a user-selected message to either
AHEAD-owned memory file, and a managed review command can propose a complete
replacement from a bounded snapshot. Only the user-facing action applies that
replacement after a content-hash check; the native model runtime does not write
memory directly, and there is no automatic background consolidation. The
editor indexes current memory and immutable revisions in `.ahead/session.db`;
do not restore a parallel Codex home or unattended memory writer.

The normal build and CI gates use the workspace's AHEAD default members. The
retained runtime crates compile as dependencies, but their copied upstream unit
test harness is not a product boundary: its removed marketplace, cloud and
test-support dependencies are not restored. Retained-loop regression coverage
belongs in `ahead-agent`, where the native streaming, scope, instruction,
memory-root and ACP adapter seams are exercised.
