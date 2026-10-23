# AHEAD agent standards boundary

Status: normative AHEAD integration boundary, 2026-09-29.

AHEAD reuses the open protocol and file conventions that already have an
editor ecosystem. The hard-forked Codex loop supplies model streaming,
conversation state and compaction; it does not define a second editor,
plugin marketplace or policy system.

The AHEAD-owned packages (`ahead-agent`, `ahead-agent-graph-store`,
`ahead-agent-skills`, `ahead-model-auth`, and `ahead-runtime-api`) are
first-class crates at the root of the Cargo workspace. The managed engine's
internal package set is grouped under `ahead-agent-engine/`; it uses the
same root workspace and lockfile, but its upstream-shaped package layout does
not define AHEAD's application architecture. Put new AHEAD capabilities in the
first-party crates and keep the retained engine behind `ahead-agent`'s
integration boundary. Preserve engine package boundaries where they still
carry required loop behavior; prune unused code and dependency edges against
the actual `ahead` dependency tree and call sites.

Normative formats, protocols, and conventions:

- [Agent Skills specification](https://agentskills.io/specification)
- [AGENTS.md convention](https://agents.md/)
- [Agent Client Protocol](https://agentclientprotocol.com/protocol/v1/overview)
- [ACP authentication](https://github.com/agentclientprotocol/agent-client-protocol/blob/main/docs/protocol/v1/draft/authentication.mdx) and [terminal-auth method](https://github.com/agentclientprotocol/agent-client-protocol/blob/main/docs/rfds/auth-methods.mdx)
- [Model Context Protocol specification, 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28)
- [MCP versioning and compatibility](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning)
- [MCP transport overview](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports), including standard Streamable HTTP
- [MCP stdio transport](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio)
- [MCP Streamable HTTP transport](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http)
- [MCP server discovery](https://modelcontextprotocol.io/specification/2026-07-28/server/discover)
- [MCP resources](https://modelcontextprotocol.io/specification/2026-07-28/server/resources)
- [MCP Skills extension](https://skills.extensions.modelcontextprotocol.io/specification/stable/skills)
- [MCP elicitation](https://modelcontextprotocol.io/specification/2026-07-28/client/elicitation)

Implementation guidance and registries:

- [Agent Skills client implementation guide](https://agentskills.io/client-implementation/adding-skills-support)
  describes discovery and loading choices; the specification defines the
  skill-file format.
- [ACP agent registry](https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json)
  lists discoverable external agents; its contents can change independently
  of the protocol.

Non-normative implementation references (not compatibility requirements):

- [Zed skills](https://zed.dev/docs/ai/skills)
- [Pinned Zed agent implementation](https://github.com/zed-industries/zed/tree/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent)
- [Pinned Zed conversation tool-diff renderer](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent_ui/src/conversation_view/thread_view.rs#L10270-L10272)
- [Pinned Zed skill loader](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent_skills/agent_skills.rs) and [strict-validation rationale](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent_skills/README.md)
- [Pinned Zed ACP registry store](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/project/src/agent_registry_store.rs), [registry UI](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent_ui/src/agent_registry_ui.rs), and [installed-agent store](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/project/src/agent_server_store.rs)
- [Pinned Zed ACP config-option UI](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent_ui/src/config_options.rs) and [ACP connection](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent_servers/src/acp.rs)
- [Pinned Zed context-server settings](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/project/src/project_settings.rs) and [server lifecycle](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/project/src/context_server_store.rs)
- [Pinned Zed elicitation form/card](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent_ui/src/conversation_view/elicitation.rs)
- [Pinned Zed HTTP transport](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/context_server/src/transport/http.rs) and [MCP OAuth implementation](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/context_server/src/oauth.rs)
- [Pinned Zed agent command/tool registration](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/agent.rs) and [in-process context-server registry](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/tools/context_server_registry.rs)
- [Pinned Zed thread store](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/thread_store.rs) and [thread database model](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/db.rs)
- [Pinned Zed project search](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/project/src/search.rs)

## Retained runtime configuration

The native AHEAD host supplies model/provider configuration from `.ahead` in
memory. The retained loader does not read Codex `managed_config.toml` files or
the old MDM `config_toml_base64` layer. Its separate administrator-requirements
reader uses `/etc/ahead/requirements.toml` on Unix, `%ProgramData%\AHEAD\requirements.toml`
on Windows, and the `com.ahead.editor` `requirements_toml_base64` preference on
macOS. `NativeClient` currently disables those managed requirements for its
turn config; do not treat their presence as proof of an AHEAD authorization
policy. The AHEAD host remains responsible for approvals and tool effects.

## Shared search and retained tools

`ahead-core::search` is the shared workspace content/path-search implementation.
The editor's project search and replace, proxy search routes, and managed
agent `file_search` use its traversal, path filters, unsaved-buffer overrides,
and match engine; callers add only their UI/session scope and pagination. Keep
that boundary shared instead of adding a second editor or harness search engine.

The retained Codex `tool_search` is different: it searches deferred tool
metadata (including MCP tools) so the model can discover capabilities. It does
not search workspace files. AHEAD's model-facing `file_search` is therefore a
first-party adapter over `ahead-core::search`, not a duplicate of a Codex file
search tool. Conversely, keep useful retained handlers such as Codex's
`apply_patch` implementation and preserve their model-facing contracts; do not
rewrite them merely to make the crate layout look AHEAD-specific.

For OpenAI-compatible catalog models, expose `apply_patch` with the ordinary
JSON-schema function-tool form only when the endpoint explicitly advertises
tool-call support (`supports_tools`, `supports_tool_calls`, or
`supported_parameters` containing `tools`). Keep Codex model metadata's
freeform patch format unchanged; unknown or conflicting capability metadata
must not enable the tool. Generic function-call support is not evidence that a
model accepts the Responses API's custom/freeform tool format. Both wire
formats dispatch to the same retained patch handler and AHEAD-managed edit
boundary. Standard Responses function-argument deltas incrementally decode the
`patch` JSON string into that handler's existing diff-preview parser, with a
bounded argument size; malformed preview data does not block the actual tool
result. This follows the user-defined
function-call format documented by [OpenRouter](https://openrouter.ai/docs/guides/features/server-tools/overview),
but catalog metadata alone is not proof that a specific model will execute a
managed edit correctly; authenticated tool, attribution and commit acceptance
remain separate.

The managed native config enables the retained runtime's
`ApplyPatchStreamingEvents` feature and routes `PatchApplyUpdated` through an
ephemeral `AgentPatchProgress` notification. AHEAD keeps the latest bounded
preview per tool call in the live chat and clears it on the next turn; these
partial diffs are not written into Turso session history. Successful final
`FileChange` events remain authoritative for CodeAnchor attribution. This
follows Zed's separation of tool-call identity from diff content, where
`ToolCallContent::Diff` is rendered by the conversation view's diff editor;
AHEAD's current card is a compact text preview, not Zed's full editable diff
surface. The decoder, native forwarding path, and rendered card still require
source-current test execution before acceptance.

## Protocols

- **Managed execution is editor-local.** AHEAD creates a single local
  environment from its editor-owned helper paths and does not read the runtime
  home's `environments.toml` or inherit remote exec-server settings from the
  process environment. The Codex TOML/environment-variable discovery and
  bootstrap path, the unused remote-host registration service and registry
  client, Noise rendezvous API, remote process/filesystem clients and server
  transport have been removed. The retained `exec-server` source path now
  serves only as AHEAD's in-process local process/filesystem/HTTP backend; it
  does not expose a remote executor or a standalone server. Further pruning of
  Codex-specific internals and focused coverage remain tracked in `TODO.md`.
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
  behavior; ACP is not AHEAD's managed authorization boundary. For requests
  it actually receives, AHEAD uses ACP v1's
  [`session/request_permission`](https://agentclientprotocol.com/protocol/v1/tool-calls#requesting-permission)
  when an adapter sends one: Assist queues the advertised choices for human
  review, while Learn, unknown modes and cancelled turns answer `cancelled`.
  Changing to Learn or ending the turn also cancels pending choices. AHEAD
  never silently picks an `allow_*` option. Agents may still execute effects
  without requesting permission, so this is a client interaction, not sandbox
  enforcement. ACP authentication follows each method advertised in
  `initialize.authMethods`: `agent` methods call ACP `authenticate`; `terminal`
  methods launch the same configured adapter executable with its base
  arguments/environment plus the method's advertised arguments/environment,
  wait for exit status zero, then restart and initialize the ACP process. AHEAD
  exposes terminal authentication through its integrated PTY and preserves the
  unsent chat draft so the user can retry after sign-in. It does not call ACP
  `authenticate` for a terminal method. These are ACP client semantics, not
  AHEAD enforcement of the external process. Follow Zed's method selection and
  reinitialization shape in `agent_servers/src/acp.rs` and
  `agent_ui/src/conversation_view.rs`.
  The install catalog is deliberately limited to Pi, Codex and
  Claude Code. Entries come from the ACP registry, but AHEAD does not expose
  arbitrary registry agents. Install
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
  On 2026-10-12, the current isolated `ahead-agent` test installed `pi-acp`
  into a temporary cache, seeded the temporary Pi agent directory with two
  synthetic models and an unreachable localhost endpoint, and passed
  `session/new`, model-option discovery/change and resume with `PI_OFFLINE=1`.
  A separate test installed and initialized all three curated ACP packages
  (Pi, Codex and Claude Code) with empty per-agent homes. No provider request
  or model turn was sent; real authenticated streaming/cancellation remains
  unverified.
  The user reconfirmed on 2026-09-28 and again on 2026-10-03 that provider
  access is not ready and asked us to continue offline.
  An earlier offline check verified Pi CLI v0.87.1 can print help with
  `PI_OFFLINE=1` and `PI_CODING_AGENT_DIR` redirected; this did not validate
  ACP. On 2026-10-04, the CLI also printed help with an isolated `HOME`. The
  npm cache has `pi-acp` 0.0.33 and 0.0.34 tarball entries, but installing
  0.0.34 offline failed with `ENOTCACHED` for package metadata; packing its
  cached tarball then failed with `EPERM` creating a temporary file in the
  existing root-owned cache. No package was installed and no ACP session was
  attempted.
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
  stdio `env_vars` accepts only local variable names and reads their values
  from the editor process. MCP declarations do not expose Codex's
  `environment_id` routing or legacy `name` alias; the runtime uses its local
  executor and the configured server ID for display.
  Keep secret values out of tracked project config. The managed agent's
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
  The managed agent presents supported flat MCP form elicitations as a
  server-labelled, blocking chat card. The user reviews typed answers before
  sending them and can explicitly decline or cancel. AHEAD validates the
  restricted schema and submitted values at the agent boundary; unsupported
  shapes fail closed. The current form UI uses the chat composer for free-text
  fields and choice buttons for selections. It does not collect secrets:
  per the MCP standard, a server must use URL mode for credentials or payment
  details. The managed client now advertises both `form` and `url` modes.
  URL requests show the server name, destination host and full HTTPS URL;
  Open, Decline and Cancel are explicit actions, and the editor never asks for
  website credentials in chat. Unsafe or malformed URLs fail closed. This is
  the retained runtime's server-requested elicitation path, separate from the
  editor's 2026 tools-only MCP bridge. Disposable stdio-server integration
  tests confirm that typed form answers and explicit URL consent reach the
  server and the managed turn resumes. A rebuilt native
  disposable-project run also verified exact-declaration approval, tool-call
  approval, the form card, typed answer delivery, explicit decline and cancel,
  and turn completion with a local mock model. The stdio-server trace received
  `accept` with typed content, then separate `decline` and `cancel` results.
  The URL card and explicit browser consent pass a headless GPUI test, but URL
  mode remains unverified in the running native UI and with a non-test MCP
  server. The retained MCP client has 2026-07-28 MRTR URL tests; AHEAD's
  managed agent and editor have not been verified end to end on that path.
  Zed's dedicated `ElicitationCard` and field validation are the reference
  for remaining typed field controls and URL review polish.
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
  `ahead-agent-skills/builtins/` and installed into the runtime's system-skill cache;
  Codex sample skills are not part of that bundle. Skill catalog metadata
  carries only a source enum, never an absolute path. When names collide, the
  composer uses source-qualified slash names for collisions: user
  `/user:name`, project `/project:name`, system `/system:name`, and admin
  `/admin:name`; unique names stay `/name`. There is no legacy `/:name`
  alias. This adapts Zed's scoped
  disambiguation pattern; the exact spelling is AHEAD behavior, not part of the
Agent Skills format. The native runtime resolves the source against the current
model-enabled skill snapshot and reads the selected `SKILL.md` on demand.
If the selected name disappears or becomes ambiguous before invocation, the
turn reports an error instead of forwarding the slash command as plain text.
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
On retry, the host replaces the persisted message-author line with the
retrying actor and rejects the retry if any recorded target-scoped
`AGENTS.md` source path, hash or target set has changed. The retained core
loads root-to-working-directory instructions independently. After its turn
refresh, the managed host stores the exact local source paths and full-file
hashes alongside the saved Turso request. Retry fails closed if that snapshot
was not captured (including a crash before the refresh), then asks the same
core loader to discover current sources and compares the ordered path/hash
list. An empty completed snapshot is distinct from an absent one. The core
snapshot is retry state and is removed with the saved request after success;
target-scoped source records remain in `turn_instruction_sources`. This does
not make repeated tool effects idempotent, so retry remains an explicit human
action. Focused Turso reopen and absent-snapshot retry tests pass; a local
mock-model turn also confirms model-visible instructions and their stored
source hash, and core rediscovery detects a changed file. The native
durable-child fixture now confirms that a managed turn's source snapshot
survives a Turso/native reopen; its explicit retry gate accepts the saved
request unchanged and rejects an `AGENTS.md` edit. The failed message is
simulated after clean shutdown. A rebuilt-proxy smoke in a disposable project
also kills the proxy during a partial model stream: after Turso reopen, the
partial message is marked failed and explicit retry rejects the missing core
snapshot without another model request. This is deliberate fail-closed
recovery, not a successful replay of a crashed turn; any future crash-retry
support needs a pre-effect provenance snapshot.

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

The managed network policy is binary: `Restricted` denies network access at
the platform sandbox boundary, while `Enabled` permits ordinary OS networking.
AHEAD does not retain proxy, MITM, per-domain allowlist, or proxy-exception
behavior. Built-in sessions currently use `Restricted`; verify each supported
platform's enforcement before exposing broader network grants. Zed's
[`terminal_tool.rs`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/tools/terminal_tool.rs)
and
[`macos_seatbelt.rs`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/sandbox/src/macos_seatbelt.rs)
are the implementation references for that boundary.

The managed AHEAD config ignores the retained runtime-home `config.toml`,
injects AHEAD provider settings as in-memory session overrides, forces log,
trace and metrics exporters to `none`, and disables user-prompt telemetry.
It also skips retained managed config file/MDM layers, system and managed
requirements, and user/project execution-policy rules; AHEAD's own per-turn
permission profiles and scoped effect checks still govern the built-in agent.
The retained config default also sets `exporter`, `trace_exporter`, and
`metrics_exporter` to `none` with `log_user_prompt = false`. The unused core
`otel_init` provider builder and process-start counter helper are removed;
`codex-otel`'s `otlp` feature remains disabled across the workspace, and its
featureless `OtelProvider::try_new` rejects any non-`none` exporter.
The packaged runtime defaults now retain only instruction-context controls.
The unused Codex `history.jsonl` persistence and URI file-opener settings and
their schema/runtime fields are removed; they never governed AHEAD's Turso
thread store or GPUI file navigation. The unused `[desktop]`, `[goals]`,
`[ghost_snapshot]` and `oss_provider` config sections, the unused core
`config::edit` TOML mutation module and Windows sandbox config-writer wrapper,
the zero-caller `detached_memory_responses_metadata` helper, and the empty
`ExtraConfig` thread-store placeholder are also removed. The packaged layer no
longer injects a Codex credential-store mode or ChatGPT endpoint. The unused
CLI credential-store and ChatGPT endpoint config paths, including
managed-requirements projections, are removed. The unused Codex
product-feedback config path is removed too; this is distinct from model-facing
tool feedback. The copied CLI startup-update and TUI paste-burst switches are
gone; any replacement would be editor-owned. Strict config and
requirements-layer parsing reject the retired keys. The
selected AHEAD provider supplies its endpoint directly. Managed MCP is
stdio-only: remote Streamable HTTP, MCP OAuth, keyring credential storage and
their login/status flows have been removed from the retained runtime, not merely
hidden by editor configuration. Model-provider authentication remains separate
and is owned by AHEAD. Zed's editor-owned HTTP transport and OAuth lifecycle are
references only if AHEAD later adopts remote MCP.
The unused Codex login-method and ChatGPT-workspace restrictions are removed
from config and managed requirements. Strict config and requirements-layer
parsing reject those retired keys; AHEAD provider selection owns model access.
The unused Codex auth bootstrap trait and config helper are gone; the native
thread manager starts with an empty auth manager and uses the AHEAD-selected
provider credential for model requests. A disposable mock-provider turn now
asserts the actual HTTP `Authorization` header, beyond the config-layer test.
Provider auth no longer redirects the default OpenAI endpoint to ChatGPT's
Codex service; AHEAD's explicit provider endpoint (or the public OpenAI default)
is authoritative.
The retired `use_agent_identity` toggle and its ignored request-scope wrapper
are removed; requests use the selected provider's bearer/header auth directly.
Unused Agent Identity and personal-access-token auth modes and their dead
provider-routing branches are also removed. Agent-identity telemetry DTOs and
request attributes are removed as well; AHEAD tracks its own work/session IDs.
The retained model provider no longer exposes Codex app-visible account state;
remote model entries merge with the bundled catalog rather than replacing it
for a ChatGPT account.
The remaining ChatGPT OAuth/token variants and their account identity and
FedRAMP bearer-header plumbing are removed. Host-supplied request headers and
provider bearer credentials remain; session account IDs and email are no longer
emitted in telemetry or network-policy audit events.
The retained core config fallback and Windows sandbox launch paths likewise
resolve AHEAD home; the unused executor config-read RPC has been removed. The
managed host passes its project-specific `.ahead/runtime` home explicitly to
core configuration; other retained helpers use `AHEAD_HOME` or the user-level
`~/.ahead` default.
The Windows path change still needs a native Windows launch test.
This is a hard-fork privacy boundary; the Codex OTEL types remain temporarily
because trace context and session timing still use them internally.

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
`ai.active_connection` selects the default. Within one source, the last
connection record with a repeated `provider_id` is the effective profile;
Settings and the chat picker use that same identity. Give separately routed
endpoints distinct IDs. The Settings panel continues to
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

The retained managed runtime constructs its model provider when a thread is
created or resumed. AHEAD refreshes the selected model for each turn, but an
existing thread cannot safely swap its provider object in place. Before a
managed prompt, AHEAD rebuilds the effective connection and compares a
process-local SHA-256 fingerprint of its provider ID and full provider
definition with the loaded snapshot. When an idle session's endpoint,
credential, or provider changes, the controller initializes a fresh managed
harness and resumes the same durable thread before the next turn. A direct
stale-client prompt still fails closed, and an active managed turn blocks the
swap until it finishes. The fingerprint and credentials are not written to
session storage. A local two-endpoint regression verifies the new connection
and model-visible history; native Settings-to-turn verification remains open.

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
   crawl unrelated subtrees. Targets outside the workspace or symlinks
   detected during path resolution are ignored. Target-scoped `AGENTS.md`
   reads use the retained `codex-exec-server::open_regular_file_no_follow`
   implementation on Unix and Windows rather than AHEAD's former
   check-then-open fallback; Windows compile and reparse-race regression
   evidence remain tracked in `TODO.md`. The target-scoped text and
   workspace-relative source labels with
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
   active/attached paths. FIM has no retained agent-core loader, so it discovers
   the nearest ancestor containing a `.git` directory or worktree marker file
   (falling back to the opened workspace when none exists) and includes the
   repository-root-to-active-file/relevant-open-buffer hierarchy. FIM targets
   remain restricted to the opened workspace; only their applicable ancestor
   instruction files may be read above it. FIM follows the nearest `.git` root
   marker with the opened workspace as fallback. It does not currently consume
   the retained runtime's `project_root_markers` config layer. This is not
   required by the AGENTS.md convention; if AHEAD exposes root-marker
   configuration, share one AHEAD-owned policy across native instruction,
   skill and FIM discovery. The AGENTS.md convention standardizes repository
   and nested files, not a root-marker setting.
   FIM has no tool to fetch omitted instructions: if the applicable hierarchy
   exceeds AHEAD's 32 KiB prompt addition, prediction fails instead of
   silently dropping a nearer `AGENTS.md`. The exact limit succeeds when no
   further applicable file exists.
   Only applicable ancestors are loaded; the nearest one takes precedence for
   its target, and the user's explicit request remains authoritative. Codex
   CLI's documented `~/.codex` plus repository hierarchy is a product-specific
   implementation example, not a portable global path: see its official
   [AGENTS.md loading description](https://developers.openai.com/codex/guides/agents-md/).
   Zed also reads `~/.config/zed/AGENTS.md` as personal instructions in its
   pinned [`templates.rs`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/templates.rs).
   AHEAD does not adopt that product-specific global path because the open
   convention does not define a portable user-global instruction location.
3. Workspace skills are discovered from `.agents/skills/`; user skills are
   discovered from `~/.agents/skills/` (or `$AHEAD_USER_HOME/.agents/skills/`
   when the AHEAD home override is set). The Agent Skills client guide
   describes `.agents/skills/` as a widely adopted cross-client location; the
   format defines package contents, not discovery paths. AHEAD
   does not treat `.agent/skills/` or `.skills/` as application skill roots,
   and it does not download or install a discovered skill. This repository
   dogfoods that exact project/user contract with development and
   runtime-maintenance skills under `.agents/skills/`; the separately bundled
   product skills under `ahead-agent-skills/builtins/` do not change discovery paths for
   user projects. They are installed as system-scope content at
   `<runtime-home>/skills/.system/` (default
   `.ahead/runtime/skills/.system/`, overridden by `AHEAD_HOME`); this cache is
   not another project or user skill root.
   The optional [MCP Skills extension](https://skills.extensions.modelcontextprotocol.io/specification/stable/skills)
   defines server-hosted discovery (`skills/list`, `skills/get`) and resource
   reads for the same Agent Skills package format. AHEAD does not advertise
   that extension: only local project, user and bundled packages appear in the
   slash catalog; MCP resources remain on the MCP resource path and are not
   implicitly treated as skills.
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
   reads the opened file through the same 32 KiB bound used for review,
   deduplicates retries by conversation-message id and refreshes the local index
   after the write. On Unix, the host opens `.ahead/memories` relative to an
   opened canonical workspace or user-home directory with no-follow directory
   and file flags. Reads, appends, reviewed replacement temp creation/rename,
   and index refresh use the resulting directory handle, so swapping the
   parent path for a symlink cannot redirect them. Appends reject hard-linked
   files. On Windows, AHEAD's `ahead-proxy/src/ahead/memory_fs.rs` opens the
   canonical root with `NtCreateFile`/`OBJ_DONT_REPARSE`, then opens `.ahead`,
   `memories`, and `MEMORY.md` relative to held directory handles. Appends and
   atomic replacement stay rooted at that handle; replacement uses
   `SetFileInformationByHandle` with a relative destination. A missing user
   home is created by a reparse-safe component walk; existing ancestors are
   opened read-only and only the final home/workspace root requests permission
   to create children. The retained
   Codex `exec-server/src/no_follow/windows.rs` informed the no-reparse kernel
   open, but its own opens are absolute and are not used as AHEAD's effect
   boundary. The parent-symlink-swap regression is wired into Windows CI but
   still needs a passing Windows run. Windows now applies owner-only ACLs to
   memory directories/files and the Turso session database plus existing
   sidecars; new database sidecars inherit the protected directory ACL. These
   Windows-only ACL changes are covered by the Windows `memory_` test filter,
   but remain unverified until that CI job passes.

   Native checks in disposable projects confirmed project- and user-memory
   search, labeled excerpt attachment, and refresh after external edits. A
   2026-10-01 isolated run with a loopback Responses mock also exercised
   message-idempotent append, project/user review turns, explicit replacement,
   and stale-snapshot rejection. This proves the local UI/host path with
   synthetic responses, not authenticated-provider behavior. Equivalent
   non-Unix path-race safety remains open.

   The Markdown record is the human-readable authority. `.ahead/session.db`
   is the sole durable store for AHEAD sessions, visible conversation, managed
   agent replay history, and the local searchable memory index with immutable
   content-hash revisions. The separate `conversation_messages` and
   `agent_runtime_thread_items` rows preserve different UI and model-replay
   contracts in the same Turso/libSQL database. Native spawned-agent parent/
   child edges and their open/closed status live in `agent_spawn_edges` there
   too; the runtime thread and graph adapters open no other durable database.
   Binding a work session to a harness thread updates the binding and native
   thread link in one transaction; a failed link rolls both back before retry.
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
   Managed replay is Turso-only after restore. New turns do not write rollout
   files. Thread-settings events use the normal `ThreadStore` persistence
   path; they do not probe for a Codex local rollout file. Shell-snapshot
   cleanup uses the snapshot's own modification time, so a valid Turso-backed
   session is not treated as an orphan merely because it has no rollout file.
   Managed startup and `SessionConfiguredEvent` do not expose a local rollout
   path; live resume identity is the Turso thread ID. AHEAD does not discover
   or import session JSONL files. A missing Turso thread is not reconstructed
   from a local rollout; pre-production session data may be discarded when the
   initial schema changes.
   Following Zed's native thread/storage split (`crates/agent/src/thread.rs`,
   `crates/agent/src/db.rs`), the retained rollout crate no longer owns normal
   disk persistence. The AHEAD-managed dependency retains only the history
   types, replay policy, model-context scanning and fork-boundary logic used by
   the in-process agent. Rollout-file reading/writing, compression maintenance,
   filesystem thread discovery, JSONL import and persistence telemetry are not
   product paths. Managed history is restored from Turso only; schema version
   checks and migration machinery are intentionally deferred while AHEAD is
   pre-production.
   Copied rollout-path resume/fork helpers and `ThreadStore` path lookup are
   removed; durable threads are addressed by ID or already-loaded history.
   The unused reference-backed prepared-fork store contract and session branch
   are removed; AHEAD's child agents start with an explicit task handoff rather
   than inheriting a parent rollout prefix.
   The unused `ForkSnapshot`/`fork_thread_from_history` API and its copied
   snapshot tests are removed. The disabled core V1 `fork_context` and V2
   `fork_turns` options, their agent-control history-copy branch, copied
   fork-only tests, and now-unused rollout-truncation module are removed.
   Stock Codex multi-agent feature wiring remains disabled; AHEAD's managed
   child path uses an explicit task message, fresh history and Turso edges.
   This is an implementation boundary, not a decision that useful Codex cohort
   behavior should be omitted; see the adapter direction below.
   Focused integration tests are tracked in `TODO.md` until run in the shared
   Cargo loop.
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
   fidelity. Native header, metadata-patch and replay-page reads cast stored
   text to bytes before UTF-8 validation, so malformed history returns an
   error instead of triggering libSQL's TEXT-decoder panic; a damaged-row
   regression covers the full, paged and listing paths. A corrupt replay does
   not remove its work session from the separate metadata-only sidebar list.
   Created/updated keyset
   listing pushes archive, source, provider,
   workspace, relation and title/preview search filters into Turso before its
   page limit; source, provider and workspace use the latest metadata patch,
   falling back to creation data. Unsectioned/unassigned-project filters also
   take this path; assigned section/project and recency ordering still load
   all headers pending indexed summary data. Archive timestamps belong
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
   only durable managed-session backend. If an ordered replay append or its
   metadata patch fails,
   that adapter latches the first error and rejects further writes for the
   loaded thread. At the next native event, the host requests an interrupt
   and fails the turn without waiting for the model to finish. It refuses
   another prompt, compaction or resumed child turn until restart;
   users must review any tool effects before retrying from durable history.
   A stalled local-provider test covers prompt interruption and retry refusal.
   The turn controller sends the resulting harness error to the open chat and
   persists it with the failed message; its chat-state regression covers the
   streamed error.
   An effect already dispatched before the host observes the write failure
   cannot be assumed cancelled; rendered-chat and tool-effect confirmation
   remain open in `TODO.md`. The typed `Internal::Guardian` and
   `ThreadSource::GuardianReview` variants are removed. The thread manager
   rejects every unsupported `ThreadSource::Feature` on create and resume;
   arbitrary subagent labels remain non-root metadata and cannot select an
   automatic reviewer. The
   unreachable Guardian-specific session setup, policy, world-state, remote
   MCP discovery, tool routing and prompt-schema branches have been removed;
   generic fail-closed source checks and legacy history/source handling remain.
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
   base URL. The discarded per-turn resolved-config fact no longer causes a
   full restored-history scan solely to label telemetry's first turn; model
   context and durable replay are unchanged. Code Mode no longer constructs
   discarded tool-call lifecycle facts or collects call IDs per sampling
   response. Its actual dispatch, rollout trace, and local host-duration
   timing are retained. Control-tool calls no longer construct a discarded
   analytics fact; pre/post-use hooks, tool-result reporting and dispatch
   tracing remain. Skill-invocation facts sent only to the inert sink are also
   removed; AHEAD's local skill counters and model-facing skill injection remain.
   Hook completion no longer constructs a discarded analytics payload;
   synchronous lifecycle events and local hook metrics remain.
   Subagent-start analytics and its inherited client-metadata lookup are also
   removed; subagent source, persistence and lifecycle events remain.
   The per-turn sampling/compaction/tool-wait profiler was also only sent to the
   inert sink, so its phase guards, retry counters and `StdMutex` are removed;
   turn duration, TTFT/TTFM and session token histograms remain.
   Compaction's disabled analytics event path and its extra token-usage
   snapshots, deleted-token export and retained-image count scan are also
   removed; compaction behavior, rollout-budget accounting and traces remain.
   Image-history preparation likewise no longer accumulates analytics-only
   dimensions, detail, message roles or call IDs; image resizing, detail policy,
   resize notices and durable history processing remain unchanged.
   This does not remove `codex-otel`,
   which still carries trace
   propagation, metrics instrumentation and session log-privacy settings; the
   dead `codex_core::otel_init` module and `record_process_start_once` helper
   are removed, and no production path constructs an active exporter. The
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
downloaded agents live in AHEAD's platform-local data directory. For the
current OS/architecture AHEAD prefers a registry binary target with a valid
SHA-256; otherwise it uses that entry's Npx distribution. Entries absent from
the cached registry use AHEAD's pinned package fallback. A curated binary-only
entry without a compatible checksummed target is unavailable rather than
silently launching an unrelated package. npm uses
`external_agents/registry/npm-cache` instead of inheriting the user's global
npm cache. As in Zed, the registry entry is enabled first and its distribution
is installed on first launch. Removing an agent removes it from AHEAD's picker;
it does not delete the cached package or binary.

Binary URLs must use HTTPS. AHEAD supports `.zip`, `.tar.gz`, `.tgz`,
`.tar.bz2`, `.tbz2`, and raw executables; it rejects installer/unsupported
formats, requires the archive SHA-256 (stricter than the registry schema's
optional field), and caps downloads at 256 MiB and expanded files at 1 GiB.
Extraction rejects path escapes and links/special files. The selected command
must be `node` or a safe relative path inside the installed generation; bare
executable names are accepted when a registry entry omits the optional `./`
prefix. Registry versions appear in the picker; a newer version is labeled as
an update and is installed on the next agent launch. If the registry is
unavailable, current fallback pins are Pi `0.0.34`, Codex ACP `2.2.2`, and
Claude Agent ACP `0.94.0`. These restrictions follow the
[ACP Registry schema](https://github.com/agentclientprotocol/registry/blob/main/agent.schema.json)
and Zed's `AgentRegistryStore` / `AgentServerStore` patterns, without importing
Zed's UI or project store.

Npx installation uses a private staging directory under the selected adapter's
cache. After npm succeeds, AHEAD checks that the package executable is a regular
file inside the package, records package/lockfile hashes, and moves the directory
to a unique generation. An atomic `installed.json` replacement makes that
generation current. A launch resolves to the generation's own executable path;
later updates never rewrite that directory. A failed install does not publish
a new manifest or overwrite earlier generations. Launch resolution and
installation share the adapter's install lock and recheck the current manifest
under that lock. If the recorded generation, package metadata, executable or
lockfile is missing, the next launch reinstalls through the same staging and
atomic-publication path. Permission errors, malformed manifests and executable
path escapes still fail closed. This repair does not sweep user caches or
rewrite a running generation. On Windows, npm installation selects `npm.cmd`
explicitly, as [Rust's Windows executable lookup requires the extension for
non-`.exe` files](https://doc.rust-lang.org/std/process/struct.Command.html#method.new),
and keeps its arguments separate through `Command` rather than building a
shell command string. Its Windows-only shim regression must pass in CI before
claiming Windows runtime acceptance.

ACP installation, registry refresh and saved-option writes explicitly unlock
their cache files when the operation ends, including on error. The lock file
stays in place so existing waiters and new callers use the same inode. A
fork-inherited descriptor cannot extend the completed operation's lock.

Running Node processes can load more files after startup. Resolved commands
and live children retain a shared generation lease. Under the install lock,
cleanup removes abandoned staging directories and unleased generations. On
Unix, versioned cross-process markers let a later app instance identify
eligible generations, then an exclusive advisory lock prevents deletion while
an ACP child in another process still holds its inherited shared lease.
Unrecognized and older markers are retained. On non-Unix platforms, markers
remain process-instance scoped, so generations from a previous app process are
conservatively retained; cross-process reclamation there remains open. The
retired flat npm cache and provenance file are neither read nor migrated;
existing files are left untouched. There is no backward-compatibility loader.
Concurrency/failure tests use disposable fake packages. The separate
`npx_generations_launch_locally_packed_packages_offline` smoke test uses real
npm to pack/install two local tarballs, then runs both with Node after the
update. It also removes the current fixture's executable and repairs it from
the local tarball while the earlier generation remains usable. It uses
temporary npm config/cache paths, offline mode and disabled lifecycle scripts.
The extended repair smoke passed on 2026-10-15 with Node 24.15.0 and npm
11.12.1, alongside all 169 non-opt-in agent library tests (four opt-in tests
ignored). These are local process tests, not a Windows or native UI pass.

On 2026-10-15, the separate opt-in
`curated_acp_session_model_options_offline` check waited for AHEAD's production
background registry refresh, consumed that cache and checked each installed
package against its live registry entry. Pi 0.0.34, Codex ACP 2.2.2 and Claude
ACP 0.91.0 all initialized, created sessions, advertised model choices, accepted
a model change and saved the selection under their own agent defaults. They
exposed 2, 8 and 5 model choices respectively; Pi used a synthetic local model
catalog. Claude's pinned fallback is now the tested 0.91.0 package. The test
fails on authentication/session errors instead of reporting an unverified
model path as passing.

Despite its name, this opt-in test needs registry/npm network access. It uses
temporary projects, agent homes and package caches, empty npm config paths,
disabled npm lifecycle scripts and synthetic keys; no model prompt is sent.
See [the test command](../building-from-source.md#agent-development-loop) for
isolation settings.
Pi's separate offline test also verifies model-option persistence across
resume. These checks prove registry/package/protocol behavior, not a real
provider login or turn, Windows runtime acceptance, or the rendered
install/reconnect journey.

The binary-distribution path uses the same versioned, leased generation
lifecycle as Npx. Archive checksums are verified before extraction; registry
updates are shown in the external-agent picker and fetched on the next launch.
Unit coverage covers platform selection, URL/command/path rejection and
versioned generation replacement. Runtime installation, archive fixtures on
Windows, and the rendered update state remain release checks; do not report
them as verified from source alone.

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
The commit author is `ahead` only for matching staged agent anchors, while the
human remains committer; one `Ahead-Session` trailer is written for each
distinct source session, in stable order.
Old anchor schemas are unsupported; startup does not drop or convert their
rows. External ACP edits do not receive managed CodeAnchors.

## Session storage decision

Zed is the product reference for session behavior: its
[`ThreadMetadataStore`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent_ui/src/thread_metadata_store.rs)
keeps lightweight sidebar metadata (identity, title, created/updated time,
agent and workspace paths) apart from the conversation body, and sorts by
recent activity. AHEAD's `SessionStore::list_sessions` likewise projects
sidebar metadata without loading each `SessionView`; the app loads detail on
selection. Its task-parent projection now groups task links once before joining
sessions, avoiding two unindexed task scans per sidebar row without changing
existing database schemas. `TursoThreadStore::list_threads` reads header and metadata-patch
rows without replay history. Metadata-only reads stay header-only; legacy
resume/full-history callers hydrate the complete replay. Paginated resume uses
reverse keyset pages over `(thread_id, ordinal)` and Codex's `ModelContextScan`
to stop at a compatible completed-turn context and valid compaction checkpoint.
If no safe cutoff exists, it continues to the beginning and returns the scanned
history. Paginated full-history APIs remain unsupported; AHEAD's visible
conversation uses its separately paged message store.
The retained Codex
[`CreateThreadParams`](../../ahead-agent-engine/thread-store/src/types.rs)
and [`RolloutItem`](../../ahead-agent-engine/history/src/lib.rs),
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
user-facing conversation. In pre-production, an empty local database receives
the complete current schema in one transaction. There is no schema-version
marker, compatibility gate, migration, reset or automatic archive step yet.
Startup reruns the initial transactional `CREATE ... IF NOT EXISTS` statements;
it does not reconcile or validate existing table definitions. If a
pre-production database has an incompatible layout, affected operations can
fail normally. Use disposable workspaces while the schema is evolving. Add
formal schema checks and a versioning policy after the shape stabilizes.

The only legacy-history exception is a one-time import when a requested thread
has no Turso header. AHEAD searches only that workspace's configured runtime
home and accepts one regular rollout whose metadata identifies the requested
thread and the same canonical workspace. Plain and compressed input and record
count are bounded; malformed records, symlinks, out-of-root paths and
active/archive ambiguity fail closed. Header and replay rows are committed
atomically to Turso, and the source file is left untouched. Resume and all
future writes then use Turso; there is no live JSONL fallback or schema
migration.

Each Turso connection sets a five-second busy timeout before initial schema
creation. Separate editor processes can wait for a transient database lock
instead of immediately losing a write. History and metadata appends reserve
the writer before reading the next ordinal; a deferred read-to-write upgrade
cannot use that wait reliably. Session write transactions use Turso's existing
transaction guard, which rolls back after an early error or failed commit.
An expired lock wait still returns an error; there is no extra automatic retry
loop. The connection remains usable for a caller's retry after the lock clears.

On Unix, new database files are created with mode `0600` before Turso writes
them. The database directory must be owned by the current user and must not
be group/world writable. Symlinks, hard-linked files, special files and
unexpected owners are rejected for the database and its `-journal`, `-wal`
and `-shm` sidecars. Orphaned sidecars are kept for recovery; startup does not
silently pair them with a new empty database. An opened database and its
existing sidecars are tightened to `0600` without rewriting table contents.
New sidecars inherit the main file's mode through Turso's bundled VFS.

Existing stores open normally through Turso and can recover an interrupted
write. Startup runs the initial transactional DDL on each open using
`CREATE ... IF NOT EXISTS`; it does not inspect schema versions or reconcile
existing table definitions. Permission updates use the checked parent
directory descriptor, without opening and closing raw database descriptors
that could release another local connection's POSIX locks. This does not
isolate programs running as the same user. On Windows, startup applies a
protected current-user-only DACL to the
database directory and existing database/sidecar files before opening libSQL;
new sidecars inherit from that directory. The Windows helper rejects reparse
points and hard links. Its ACL and inheritance regression is wired into the
Windows CI filter, but this implementation has not yet been compiled or run on
Windows; require that CI result before relying on the guarantee.

A workspace storage error disables agent-session requests and displays the
cause. AHEAD does not substitute an in-memory workspace session. File editing
and LSP remain available; projectless initialization may use an explicit
in-memory host. Do not rely on pre-production databases across schema changes;
use a fresh disposable project when validating first-run storage.

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
layout/caret restoration are not implemented. Schema-version checks and
compatibility handling are deferred until the pre-production layout settles;
nonmatching databases are not migrated automatically.

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
configuration filename, directory, or settings shape. The MCP repository's
[configuration-schema proposal](https://github.com/modelcontextprotocol/modelcontextprotocol/issues/292)
is still open; `.mcp.json` and other `mcpServers` files are ecosystem
conventions, not a required MCP placement or schema. Zed's pinned
`ProjectSettings` uses its own `context_servers` map and extension sources;
that format is editor-specific. AHEAD's `.ahead/config.toml` and explicit
local opt-in are likewise AHEAD-owned and map to MCP's standard transports.
They do not promise compatibility with Zed's configuration or auto-start
behavior.
The copied Codex Apps `[apps]` config, schema and managed requirement branch
have been removed: they had no production consumer in AHEAD. Ordinary
`mcp_servers` settings and per-server tool approvals remain the managed-agent
path. Zed's `ContextServerStore` and `ContextServerRegistry` are the reference
for keeping server identity and tool registration with the editor's project.

The current MCP specification is `2026-07-28`; that modern protocol removes the
`initialize` handshake and requires per-request version metadata plus
`server/discover`. Zed's current context-server implementation still targets
the legacy handshake family through `2025-11-25` (`LATEST_PROTOCOL_VERSION` in
`../zed/crates/context_server/src/types.rs`). AHEAD's ACP editor bridge exposes
only the MCP tools feature. Its `mcp/message` route is an optional experimental
ACP extension, not part of the stable ACP v1 protocol; see the
[ACP `MessageMcpRequest` schema](https://agent-client-protocol.github.io/typescript-sdk/types/MessageMcpRequest.html).
Do not make external-agent interoperability depend on that route. The baseline
is ACP's standard v1 `mcpServers` session configuration with the editor
bridge's stdio transport, following Zed's `mcp_servers_for_project` and its
shared new/load/resume session-request builders in
`../zed/crates/agent_servers/src/acp.rs`. This is a protocol-level session
configuration, not a project or user config-file convention. ACP v2 also
defines session-scoped `mcpServers` with transport and capability negotiation;
do not claim that v2 wire shape until AHEAD's ACP client is upgraded and tested.
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
not an MCP protocol requirement. The stdio event queue is bounded to 32 events
with reader backpressure, and the local bridge admits at most 20 socket
workers, reserving four slots beyond the 16 pending tool calls for cancellation
traffic. Socket reads have whole-frame deadlines (20 seconds for requests,
30 seconds for tool results, two seconds for cancellations); responses are
limited to 2 MiB. Oversized server replies become tool errors, and oversized
or slow-dripped inbound bridge frames fail before parsing. End-to-end overload
and native ACP verification remain open in `TODO.md`.
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

The managed native agent prefers MCP `2026-07-28` for local stdio servers.
The retained runtime's `McpProtocolMode::V20260728` uses the official RMCP
client's `Auto` lifecycle: it probes with `server/discover`, then falls back to
legacy `initialize` for older servers as specified by
[MCP version negotiation](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning)
and the [stdio binding](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio).
This is separate from the ACP editor bridge above. A disposable managed-MCP
fixture now requires modern request metadata and result/cache fields for its
default tool/resource scenario; the form and URL fixtures exercise legacy
fallback. `tests/agent-mcp-smoke.mjs` now exercises the real managed client via
the rebuilt proxy for modern discovery, per-call approval, stdio tool execution,
resource/template listing, resource reads and model continuation. A second
phase uses the form fixture, which rejects discovery, then verifies legacy
initialization and tool execution. In-flight cancellation and the rendered
approval UI remain unverified; do not claim full current-spec or MCP Apps
support.

- Put shareable, non-secret server declarations in tracked `.ahead/config.toml`
  under `[mcp.servers.<id>]`. AHEAD currently supports local stdio servers
  using `command` and optional `args`; the transport is inferred from those
  fields, so do not add a `transport` key. The MCP wire protocol also defines
  Streamable HTTP, but AHEAD currently implements only stdio and does not ship
  the remote transport or its authentication lifecycle. This file shape is AHEAD-owned,
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
  to local process environment variables. MCP OAuth and remote secret storage
  are not implemented. Opting in launches the configured command with the
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
  approval state, and can enable or revoke it after review. Once enabled, the
  server can stay on per-call review or auto-approve permitted tool and
  resource calls. Approval is
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

# Optional: auto-approve permitted calls for this reviewed server.
# Omit this table to review every call.
[mcp.server_permissions]
docs = "allow"

# Optional named-tool exceptions to the server policy.
[mcp.tool_permissions.docs]
delete = "deny"
publish = "confirm"
```

- AHEAD ignores approval modes in tracked declarations. The ignored,
  workspace-local `[mcp.server_permissions]` table accepts `allow` for
  server-wide auto-approval of permitted tool and resource calls or `confirm`
  for per-call review; an absent entry also means per-call review. The
  `[mcp.tool_permissions.<server>]` table may override named tools with
  `allow` (skip review), `deny` (do not expose the tool), or `confirm` (review
  each call). Unlisted tools inherit the server choice. The server choice
  applies only to the reviewed
  declaration. Reapproving a changed declaration clears it; disabling the
  server removes it. Named overrides use tool names, not argument patterns,
  and neither policy sandboxes the MCP server. Opting in starts
  the server with the user's OS permissions. Managed Assist sessions apply
  these choices; Learn/read-only sessions remove MCP servers. The external ACP
  adapter does not receive these
  user-configured servers because AHEAD cannot enforce its review policy over
  an agent-owned process. Zed's
  `../zed/crates/agent/src/thread.rs::authorize_third_party_tool` and
  `../zed/docs/src/ai/tool-permissions.md` remain the reference for one-time
  and persistent choices made from the review UI. AHEAD currently reads local
  choices from settings, but the review UI cannot save an "always" choice yet.
  The managed MCP question is single-select in chat; the native client and
  retained approval parser reject conflicting answers instead of treating a
  mixed Allow/Cancel response as approval.

Tool calls follow AHEAD's configured allow/deny/confirm policy; requests that
need confirmation go to the human. There is no automatic reviewer or per-app
Codex reviewer override. `approvals_reviewer` and `required_on_models` are not
recognized selectors and cannot route a request to a reviewer:
there is no reviewer enum or selector in config, profiles, managed
requirements, analytics, MCP policy or runtime state. Reading old session JSON
is not a compatibility promise. The model-specific required-review constraint
and the legacy `auto_review.ignore_rules` requirements path are removed.
Prefix-rule handling still honors the model's cyber specialty; no copied
reviewer configuration may reclassify a model. Guardian session-source
variants are removed. Unsupported feature-thread sources are rejected at
creation and restore, while arbitrary subagent labels cannot become root
turns or select an automatic reviewer. Current AHEAD restore and permissions
must stay tested.
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
managed native bootstrap does not load a Codex system-config file or project
config. The retained loader has no ordinary system/project config-file layer;
system and MDM administrator requirements remain separate policy inputs. The unused
Codex cloud-config bundle loader and enterprise-managed provenance variants
have been removed; local/system and MDM policy inputs remain. Its
credential-broker-specific
project trust, provider-environment binding, requirements path, core conversion
fields and broker-only copied tests have been removed. The retained config
loader has no network-proxy requirements or `features.network_proxy` behavior;
the managed session's binary `NetworkSandboxPolicy` is the only network control.
The proxy runtime, compatibility DTOs and startup call chain have also been
removed.

## Code Mode sidecar

Some retained model catalogs require `CodeModeOnly`: their model-facing `exec`
and `wait` tools need a JavaScript host, not a direct-tool substitution. The
managed client launches `ahead-code-mode-host` beside the editor executable
over the pinned stdio protocol. AHEAD owns the host and runtime crates in the
root Cargo workspace; their initial implementation is distilled from the
`rust-v0.152.0` Codex host/runtime source pinned in
`ahead-agent-engine/SOURCE_BASELINE.toml`. The managed Code Mode path is
stdio-only: its unused gRPC client, proto generator, and transport dependencies
were removed. The host is an execution sidecar, not an App Server or
an ACP agent. When it is missing, Code Mode remains unavailable and
`CodeModeOnly` remains fail-closed. AHEAD leaves the default-on `CodeModeHost`
feature enabled in managed config; disabling it would make the bundled
sidecar unreachable even when the executable is present.

`max_yield_time_ms` is enforced. The protocol also carries
`max_heap_size_bytes`, but AHEAD rejects non-null values at both the host and
in-process runtime boundaries rather than silently dropping them. V8's hard
heap limit can terminate the shared host process, so it is not safe to present
as per-session isolation while one host multiplexes sessions.

The V8 dependency is pinned to `150.4.0` with `v8_enable_sandbox`. Deno's
ordinary release lacks that archive. On supported macOS and Linux targets,
`scripts/ensure-code-mode-v8.sh` obtains the matching archive and Rust binding
from the [OpenAI release](https://github.com/openai/codex/releases/tag/rusty-v8-v150.4.0),
verifies the hashes published in the [upstream build manifest](https://github.com/openai/codex/blob/12b3e88/MODULE.bazel),
and exports the paths for Cargo. The runtime test asserts that the linked V8
sandbox is enabled. An AHEAD-owned stdio smoke test launches the sidecar and
roundtrips both a JavaScript cell and a delegated tool call through the
managed client. A provider-backed native turn and signed-bundle execution
remain separate gates.

## Ownership and reuse

Zed references are the editor-side source for worktree snapshots, ignore-aware
search, path matching, progressive skills and agent-panel interaction. The
retained Codex sources are the default implementation for the model-facing
turn loop, tool contracts and handlers, streaming, cancellation, compaction and
provider requests. Keep and adapt those handlers where they fit—such as shell,
`apply_patch`, MCP and model-selected utility tools—and route their effects
through AHEAD's editor, permission, attribution and Turso boundaries. Do not
build a parallel tool for an operation the retained runtime already provides.
AHEAD-specific tools are justified only for missing host capabilities or
different persistence semantics: `file_search` overlays unsaved buffers on
the shared `ahead-core::search` engine, while `spawn_agent` binds child
lifecycles to Turso. Pruning a Codex product surface or disabling its feature
flag is not, by itself, evidence that the underlying agent behavior is
unneeded; adapt useful behavior before removing it.

The bundled model catalog contains model and capability metadata, not copied
vendor prompt templates. Missing model instructions use the shared AHEAD
template at `ahead-agent-engine/protocol/src/prompts/base_instructions/default.md`;
an explicit provider-supplied `instructions_template` remains model-specific.
Catalog and unknown-model first requests are covered by the native mock-provider
test. The model response shape no longer emits or promotes the retired Codex
top-level `base_instructions` field.

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
files under existing workspace directories can participate. Its optional
`include_pattern` and `exclude_pattern` arguments use the same comma-separated
worktree path-filter syntax as the panel, with exclusions taking precedence.
Both filters are part of the revision-bound cursor fingerprint. This bridge
uses the shared AHEAD matcher, not a second agent search implementation. Managed
`file_search` requests also ask the editor for best-effort LSP document symbols
through the existing `GetDocumentSymbols` route, capped at 16 open buffers and
300 ms. Matching lines are labeled with the narrowest containing symbol when
available; missing servers, errors and timeouts retain the line-preview result.
The snapshot RPC caps symbols at 128 per buffer, labels at 256 bytes, and the
whole path/content/symbol payload at 8 MiB. `read_editor_buffer` and external
ACP buffer reads do not request symbol context. This is LSP-provided outline
context, not the syntax-tree ancestor lookup Zed can perform on its parsed
buffer. A prior disposable GPUI run exercised single-buffer `file_search` and
`read_editor_buffer` calls through a mocked-provider turn. Existing agent
regressions cover multiple unsaved buffers, workspace/scope/private filtering,
unsaved-buffer query fingerprints and cursor paging. The new symbol bridge
still needs focused test execution and a native LSP-backed smoke test. Snapshot
paths are workspace-relative, symlink escapes and private files are excluded,
the managed scope is checked before searching, and an unanswered editor
request fails the tool rather than silently searching stale disk contents.
Agent matches come in 20-match pages with an opaque next cursor bound to the
query, open-buffer contents, current search revision and emitted-result prefix.
Relevant content-only watcher events advance the search revision without
rebuilding the cached path list; file-set and ignore-rule changes advance it
while invalidating that list. A stale cursor fails with a restart-from-first-page
error instead of skipping or duplicating results; recreating the workspace
index also invalidates prior cursors. The model-facing tool accepts only the
opaque cursor; it rejects the former raw `offset` argument. The cursor carries
an internal position, query fingerprint, search revision and emitted-result
fingerprint so pagination cannot silently skip or duplicate results. Dropping
the native tool future
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
implementation and old core-plugin facade are deleted. AHEAD CodeAnchors are
derived from verified file-change deltas in `ahead-agent/src/session.rs` and
passed through `AgentStore::record_edit_anchor`; retained command events carry
no Codex plugin identity.
The Codex agent-lifecycle hook crate, configuration and dispatch path are
removed; AHEAD does not install or execute these hooks. The legacy `notify`
command and turn-completion runner are also removed, so a Codex-style config
cannot spawn a notifier. MCP approvals, skills, network MITM hooks and user Git
commit hooks are separate supported behaviors and remain unaffected.
Retained TUI config writers, reader/keymap and projected runtime fields are
gone: AHEAD has no Codex terminal UI, and editor settings belong to its GPUI
shell. The active default/plan collaboration modes remain part of the managed
agent behavior, not terminal configuration.
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
The retained core uses ordinary MCP server/tool identity for approvals,
matching Zed's `mcp:<server_id>:<tool_name>` authorization key. Hosted
connector/account/plugin approval metadata, connector-derived tool identity
and connector telemetry are removed. The copied `codex_apps` name reservation
and executor-local HTTP MCP discovery branch are removed: it has ordinary MCP
server-ID semantics now, with no hosted connector behavior. Managed sessions
still load only reviewed workspace stdio declarations. The hosted-only MCP
resource filter and `orchestrator.mcp.enabled` switch are removed. Configured servers
use the ordinary MCP runtime for resource listing and reading. The
disposable managed stdio fixture now exercises `resources/list` and
`resources/read` through a mock-model turn. That fixture uses the negotiated
legacy protocol; the modern 2026-07-28 resource result envelope and cache
fields still need a separate probe before claiming current-version coverage.
Model-invoked resource listing, template listing and reading use the MCP
human-review path before dispatch, once per actual configured server, unless
the reviewed server has the workspace-local `allow` choice. Aggregate listings
wait for every required review before
contacting any server and filter
execution to that approved set. The disposable fixture proves a denied list
never contacts the server, then exercises approved list, template and read
calls. Resource operations use only the server-wide choice, never
`[mcp.tool_permissions]` names, avoiding a collision with real MCP tools.
The Settings panel exposes the server choice after declaration approval. The
disposable mock-model fixture also starts a second managed session under the
server-wide `allow` choice and proves its tool and resource calls reach the
stdio server without a per-call review request. Native approval-card and
cancellation behavior remain unverified. Server
opt-in alone is not per-resource-call authorization.
The unreachable consequential tool approval-template module and its
`codex_apps`-only bundled JSON asset are removed. Ordinary approvals still
show server/tool identity, original arguments, optional title/description
and the session-remember choice; no connector-specific copy is selected.
The hosted `_codex_apps` request metadata envelope and its connector-ID
session-approval exception are removed. Ordinary MCP requests keep their
call/turn metadata and generic per-server approval keys.
MCP call items and legacy events no longer carry hosted connector, link,
app-action or plugin identity. They retain `readOnlyHint` and the MCP Apps
tool UI resource URI from `_meta.ui.resourceUri` (preferred) or the older
`_meta["ui/resourceUri"]`, following the
[MCP Apps tool-UI linkage](https://apps.extensions.modelcontextprotocol.io/api/documents/overview.html).
The proprietary `openai/outputTemplate` fallback is removed. This is
metadata preservation, not an MCP Apps renderer or host-capability claim.
The unreachable hosted-Apps tool-name normalizer and startup branch in
`codex-mcp` are removed. Ordinary MCP tools keep their server namespace and
listed tool name, with untrusted connector metadata stripped. The client
uses the SDK-backed `list_tools` result directly; the duplicate helper that
parsed connector IDs from server-supplied metadata is removed. The client
announces `AHEAD` in MCP `clientInfo` and uses an AHEAD HTTP user agent, as
Zed announces its own editor identity when initializing context servers.
The unused hosted `events/list`/`events/stream` resource adapter, its
write-only extension injection and hosted-server publication watcher are
removed. Standard MCP resource operations still use `McpRuntime` through
`CodexThread`; the hosted-only catalog policy exemption and cache bypass are
also removed, so configured servers follow the same permission and caching
rules. The unreachable Plugin Runtime `events/stream` request and notification
transport are removed from the lower-level MCP client, including its
HTTP-only cancellation/timeout branch. The managed agent supports local stdio
MCP; remote HTTP/SSE transport is not implemented or part of its test surface.
The hosted-only OpenAI file-upload rewrite and upload client pool are now
removed from the core, along with the API upload module and `codex-mcp`'s
`fileParams` schema masking. Ordinary MCP calls pass their JSON arguments to
the exact prepared client. This follows Zed's direct MCP call path in
`crates/agent/src/tools/context_server_registry.rs`; no Zed code was copied.
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
removed. AHEAD supports local stdio MCP only; remote bearer/header transport,
OAuth challenges and server credential storage are not part of the retained
runtime. Model-provider authentication is unchanged. Zed's
`crates/project/src/context_server_store.rs` remains the reference if AHEAD
chooses to add editor-owned remote MCP later.
The Apps-only startup cache, disk persistence, reconnect/backoff state and
cached server metadata are removed. MCP no longer subscribes to model-account
changes or stores model credentials in its published runtime. Explicit config
and environment changes refresh local stdio MCP state. The ordinary in-memory
per-server tool cache in
`codex-mcp/src/tool_catalog_cache.rs` still supports bounded startup discovery;
tool execution binds to the exact ready client and current permissions.
The Apps policy evaluator and `ahead-mcp-state` package are deleted. MCP tool
exposure now uses the published server catalog and model-visibility metadata;
approval uses the prepared call's per-server policy. The hosted-widget resource
reader, event observer and widget provenance checkpoints are also removed from
runtime state, compaction and serialization. Standard MCP resource reads and
AHEAD's Turso session replay remain. The remaining hosted-Apps metadata
and event registration are tracked in `TODO.md`.
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
are no longer runtime features; strict config validation rejects those retired
keys instead of whitelisting them. AHEAD also removed the copied feature-alias
registry and the top-level `experimental_use_unified_exec_tool` setting.
`[features]` and managed requirements use canonical keys; strict validation
rejects retired aliases instead of silently mapping them to active behavior.
The copied no-op removed-stage feature entries and special-case ignore branches
are gone too; strict config rejects their old keys. The retained Windows
sandbox feature flags (`experimental_windows_sandbox` and
`elevated_windows_sandbox`) still have runtime readers.
The copied managed
network proxy, MITM, SOCKS, DNS, certificate and credential-broker engine is
deleted, along with its serialized sandbox/exec-server fields and startup call
chain. AHEAD's retained network permission is binary and does not inspect or
broker individual destinations.
The copied `x-oai-attestation` provider hook is also removed. Managed AHEAD
never supplied a provider, so no request used that header; this removes the
OpenAI-only extension point without changing Responses transport or native
tool contracts.

Stock Codex `Collab`/v1 and `MultiAgentV2` dispatch remains disabled in the
current runtime; that avoids activating its unadapted feature wiring, not the
underlying cohort behavior.
AHEAD's native `spawn_agent` dynamic tool follows Zed's
[`SpawnAgentTool`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/tools/spawn_agent_tool.rs)
by creating or resuming a distinct child thread; Zed's
[`NativeThreadEnvironment`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/agent.rs)
persists parent/depth in `SubagentContext`, with round-trip and recursive
deletion tests in
[`db.rs`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/db.rs).
The copied `ThreadManager::spawn_subagent` path that forked full parent
history is removed; only AHEAD's explicit child-task handoff remains.

**Tool reuse direction (2026-10-05):** Zed remains the behavior reference;
Codex's retained implementation is the default reuse source. The managed loop
already reuses Codex Core's dynamic-tool dispatcher, `ThreadManager`,
`CodexThread` and turn execution. AHEAD's current adapter follows Zed's single
spawn/resume-and-return-result interaction and binds child execution to Turso
edges, AHEAD permissions and editor tools; this is not a second agent runtime.
That Zed interaction is not evidence that Codex's other cohort capabilities
are useless, but those extra model-visible controls are not part of AHEAD's
current Zed-aligned workflow. The removed stock MultiAgentV2 surface included
`wait_agent`, `send_message`, `followup_task`, `interrupt_agent` and
`list_agents`; AHEAD currently exposes one awaited, resumable `spawn_agent`
instead. Its thin host adapter delegates child creation/resume to Codex Core's
`ThreadManager`, child turns to `CodexThread`, and durable parentage to Turso.
Keep using retained Core behavior rather than recreating its loop or effects.
If a concrete AHEAD workflow later needs background steering or another
control, adapt the pinned Core handler behavior at AHEAD's Turso and
permission/editor boundary; do not add a parallel implementation or duplicate
model-visible aliases. Child scope, tool inheritance, cancellation, cold
resume and rendered behavior remain verification gates in `TODO.md`.

For reuse research, compare the upstream Codex
[`MultiAgentV2` dispatcher](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/multi_agents_v2.rs),
[`spawn`](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/multi_agents_v2/spawn.rs),
[`wait`](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/multi_agents_v2/wait.rs)
and [shared message handler](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/multi_agents_v2/message_tool.rs).
These are behavior references, not an upstream sync requirement: verify them
against the retained fork revision, then adapt the existing Core implementation
at the AHEAD session/persistence/effect boundary rather than cloning handlers
into `NativeClient`.

Implementation placement is constrained by the retained architecture:
MultiAgentV2 executors call Core's crate-private `AgentControl`, so
`ahead-agent::NativeClient` cannot invoke those removed handlers as an external
API. This does not require replacing AHEAD's current thin `spawn_agent`
adapter: it already uses Core's `ThreadManager::spawn_subagent_session` and
`resume_thread_by_id_with_tools`, then runs the child with the shared
`CodexThread` turn consumer. If additional control behavior is justified,
adapt it inside Core or expose one narrow Core adapter so Turso edge creation
and AHEAD policy stay authoritative; never expose both stock and AHEAD aliases.

The agent `file_search` adapter and project-search panel both use
`ahead-core::search`. Keep that shared engine and integrate editor-specific
buffer snapshots at the adapter boundary rather than adding another search
implementation.

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

`native_client.rs` handles AHEAD's tool-approval form and bounded
server-requested forms and HTTPS URL elicitations through chat cards.
The copied `openai/form` and `openai/elicitation/create` handlers and protocol
modes are removed; the unused `openai/standard-form-input` client flag and its
approval-policy bypass are removed too. Standard MCP `elicitation/create`
remains. Managed config advertises `{form: {}, url: {}}`; unsupported requests
are declined. Zed's user-event handoff
(`../zed/crates/agent/src/agent.rs`, `ThreadEvent::Elicitation`) and URL card
(`../zed/crates/agent_ui/src/conversation_view/elicitation.rs`) are the UI
references. The AHEAD URL review card has a headless GPUI regression, but its
native server-to-browser journey remains to be verified. Tool-call
authorization remains a separate human-review path.
AHEAD does not emit Codex `approvals_reviewer` response metadata.

The Guardian transcript/context crate, second pre-compaction history copy,
model reviewer, prompt assembly, metrics, review-session cache and denial
circuit breaker are removed. Its unused analytics event model and orphan
plugin-install/external-import telemetry facts are also deleted; no event is
sent or persisted. The extension API no longer exposes approval-review
contributors or their assessment/input/error DTOs; serialized Guardian
assessment event types are removed. Since AHEAD no longer imports rollout
files, obsolete assessment records are not a supported restore format.
The uncalled root
snapshot/version API, Guardian answer-evidence cache and dead AgentControl
provider are removed; ordinary `request_user_input` keeps returning the same
serialized host response. Approval contexts, request DTOs and formatters now
live under AHEAD's `core/src/approval` module; copied tool call sites still use
that compatibility seam. AHEAD has no configuration-routed or strict-legacy
automatic reviewer. Codex review metadata is not emitted or interpreted;
caller-supplied `auto_review_enabled` and `node_repl_auto_review_required`
values are filtered.
Approval timeout messages use fixed AHEAD-owned text. The `ModelMessages`
instruction schema does not expose model-catalog auto-review policy or
reviewer-specific approval text; stale upstream catalog fields are ignored and
the bundled catalog's long review policy is removed. The unused
`ModelInfo.auto_review_model_override` field and bundled values are removed as
well. The Codex-only `node_repl_auto_review_required` model signal is removed;
`node_repl_disabled` remains as the capability gate for Node REPL-compatible
MCP calls. The catalog on-request message is shared, and
permission prompt assembly no longer has a reviewer-selection branch. Retired
reviewer config keys do not select or normalize to a policy. The retired
`auto_review.ignore_rules` requirements path is rejected; only the model's
cyber specialty can suppress executable-prefix allow rules.
The standalone Codex `/review` operation is not an AHEAD feature and has been
removed with its second model turn, review-only model setting, protocol items,
prompts, rubric and rollout mappings. Users can request code review in normal
agent chat; AHEAD's source-control review is a separate editor workflow. Keep
`ReviewDecision` for actual human tool authorization; it is unrelated to the
removed review task. The copied Codex `codex_delegate` second-session runner
is also not used by AHEAD and has been removed; AHEAD's session/thread host
path remains the owner of managed sessions.
Retired `window_number` and `forked_from_ordinal_exclusive` keys in user
`responses_api_metadata` are rejected as core-owned fields, not accepted and
silently dropped for old configurations.
The retained inference client and API wrapper now use only `/responses`; the
dedicated Guardian and classifier routes and the `free_guardian` config switch
are removed. The dormant Guardian v2 feature/config schema is removed too. The
four Guardian-only toggles are absent from the feature registry, and strict
config validation rejects their old keys. The
model-instruction schema and bundled catalog no longer carry Guardian policy
instructions or reviewer-specific approval text. Approval contexts, action DTOs, annotation types and
formatters now live under AHEAD's `core/src/approval` module. Strict legacy
review remains fail-closed. The retired `on-failure` approval-policy spelling
is rejected instead of being silently interpreted as `on-request`. Serialized
`GuardianAssessment*` event types and
rollout-preservation tests are removed. Typed Guardian session-source variants
are also removed; all feature-thread sources are rejected at creation and
restore, while arbitrary subagent labels remain non-root metadata rather
than selecting a reviewer. The legacy
`guardian_approval` feature key and `auto_review` feature requirements are
unknown, have no effect, and are covered by strict-config and managed-requirement
regressions. Model-specific automatic-review selection is absent from native
session startup, step changes and MCP policy. The strict Codex reviewer
override and its response field are removed; AHEAD's configured permission
policy remains authoritative.
Guardian-specific downstream session-policy/prompt branches are removed;
removal of old-format source markers and event DTOs remains tracked
in `TODO.md`.
The retained approval-authority tests cover ordinary network, shell and
permission decisions under `session/tests/approval_authority_tests.rs`; they
are not a Guardian execution path.

The copied browser/device login server, OAuth callback UI, hosted success pages,
PKCE login, account storage, account token refresh/revoke, JWT handling, account
keyring, workload identity and Agent Identity code are removed. Standalone MCP
browser login, callback handling, client registration and auth-status display
are removed too. The separate Codex enterprise-managed MCP identity/ID-JAG
integration and its private `ema-idp:` credential namespace are also removed
because AHEAD has no caller for them. `ahead-model-auth` is the AHEAD-owned
model-request seam: it accepts bearer/header credentials from the selected
`.ahead` provider, supports an optional provider-owned token command, and
supplies shared HTTP clients. It has no product account or login surface.
The copied MCP Streamable HTTP/OAuth branch and its keyring/secrets dependencies
have been removed. The only managed MCP transport is local stdio; no remote
authentication status is exposed. Zed's `ContextServerStore` owns
`AuthRequired`/`Authenticating` state and persists `OAuthSession` through the
editor's credentials provider; use that editor-owned lifecycle as the reference
if remote MCP is adopted in a future product decision.
The copied auth-environment collector has been removed: model-list and session
telemetry no longer export whether credential environment variables are set.
Provider request authentication and actionable auth-error reporting remain.

The copied Codex memory generation/tool pipeline is not present in the hard
fork. AHEAD reuses its useful instruction and citation shapes, but keeps memory
access explicit: the editor can append a user-selected message to either
AHEAD-owned memory file, and a managed review command can propose a complete
replacement from a bounded snapshot. Only the user-facing action applies that
replacement after a content-hash check; the native model runtime does not write
memory directly, and there is no automatic background consolidation. The
editor indexes current memory and immutable revisions in `.ahead/session.db`;
do not restore a parallel Codex home or unattended memory writer.

The copied `source/ext` extension API, built-in extension items and extension
skill host are removed. No production path called them; AHEAD uses its own
standards-based skill loader and editor-owned tool/session lifecycle instead.
Their upstream-only tests are not a reason to restore a second extension
runtime.

The normal build and CI gates use the workspace's AHEAD default members. The
retained runtime crates compile as dependencies, but their copied upstream unit
test harness is not a product boundary: its removed marketplace, cloud and
test-support dependencies are not restored. Retained-loop regression coverage
belongs in `ahead-agent`, where the native streaming, scope, instruction,
memory-root and ACP adapter seams are exercised.
