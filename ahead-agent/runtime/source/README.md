# AHEAD distilled native-agent runtime

This directory contains the selected Rust source used by AHEAD's managed
native agent. AHEAD maintains it as a hard fork; there is no automatic upstream
sync. The upstream baseline and initial AHEAD patch commit are recorded in
[`../SOURCE_BASELINE.toml`](../SOURCE_BASELINE.toml).

The live application supplies its Turso-backed thread store and owns durable
session state, editor tools, policy, and attribution. The copied `codex-state`
crate and its SQLite/SQLx dependency path have been removed from this hard fork.

Retain and follow the upstream license and notice files alongside this source.
