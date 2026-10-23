# AHEAD agent engine

This directory groups the retained Cargo packages for AHEAD's managed agent.
It is not a Cargo package or a nested workspace. The repository root owns their
membership, dependency versions, lockfile, toolchain, and build configuration.

`ahead-agent/` owns the editor-facing integration: the durable session
controller, direct native client, and external ACP client. The ACP client calls
separate agent processes; it does not route AHEAD's managed turns through this
engine. AHEAD-owned support crates sit at the workspace root:
`ahead-agent-graph-store`, `ahead-agent-skills`, `ahead-model-auth`, and
`ahead-runtime-api`. The engine receives AHEAD's session store, editor tools,
provider configuration, and effect policy through those boundaries.

The packages here retain the model and tool loop behavior AHEAD uses, including
model-specific tool selection, instruction layering, compaction, streaming,
cancellation, and sandboxed tool execution. Their `codex-*` package names record
the hard fork's provenance; they do not define the editor's architecture. New
AHEAD capabilities belong in first-party workspace crates unless they must
change the managed model loop itself.

The source began as a hard fork of `codex-rs` at AHEAD fork commit
`7c07b2f88aceedb4ed187a5fafa145044660d07a`, based on upstream commit
`316795b3cf2a45e90d121d9f46499d4658b2645c`. It is selectively trimmed
and AHEAD-modified, not an upstream sync checkout. See
[`SOURCE_BASELINE.toml`](SOURCE_BASELINE.toml), and retain the pinned
[`LICENSE`](LICENSE) and [`NOTICE`](NOTICE).

Before editing these packages, read [`AGENTS.md`](AGENTS.md) and the applicable
skills under [`.agents/skills/`](../.agents/skills/).
