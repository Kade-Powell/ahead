# AHEAD Agent Runtime

This directory is the private execution runtime used by the AHEAD-owned
`ahead-agent` crate. It is intentionally separate from the editor, proxy, and
session code so the model/tool loop can be upgraded without widening those
boundaries.

`source/` was extracted from AHEAD's former `third-party/codex/codex-rs` at
AHEAD fork commit `7c07b2f88aceedb4ed187a5fafa145044660d07a`, whose parent is
the upstream `rust-v0.152.0` commit `316795b3cf2a45e90d121d9f46499d4658b2645c`.
The fork commit is not an upstream revision. Exact source and patch provenance
is recorded in [`SOURCE_BASELINE.toml`](SOURCE_BASELINE.toml). The extracted
tree is selectively trimmed and AHEAD-modified; it is not an upstream sync
checkout. `ahead-agent` is a library;
`ahead-proxy` hosts it in-process. The `ahead` executable re-enters itself with
`--proxy` for the crash boundary and exposes the two sandbox re-exec helper
modes required by the native runtime; there is no separate proxy executable.
There is no AHEAD Agent server or Codex App Server executable between the
editor and the loop. AHEAD owns provider
selection, durable sessions, normalized presentation events, policy validation,
scope, and attribution around that direct integration. Generated App Server
protocol exports and the copied App Server DTO crate are removed. Model-loop
history is persisted directly through AHEAD's Turso-backed thread store.

Some internal crate, environment, and wire identifiers retain historical names
because changing them would break protocol compatibility. They are private
implementation details; new AHEAD code should depend on `ahead-agent` and its
public types instead of those identifiers.

The upstream license and notice files are retained beside the embedded source.

When maintaining the embedded source, read [`source/AGENTS.md`](source/AGENTS.md)
and the scoped workspace skills under
[`../../.agents/skills/`](../../.agents/skills/). AHEAD dogfoods the same
project-skill root it exposes to users. These procedures are adapted from the
pinned Codex CLI guidance and remain subordinate to AHEAD's root instructions
and host policy.
