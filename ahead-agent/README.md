# AHEAD agent integration

This is the editor's agent integration crate. `src/` is its ordinary Rust
source directory. AHEAD's built-in agent skills live in
[`ahead-agent-skills/builtins/`](../ahead-agent-skills/builtins/). The root
`Cargo.toml` owns the workspace and lockfile.

`src/session.rs` owns durable sessions and turn dispatch. `src/native_client.rs`
calls the managed model and tool engine in [`ahead-agent-engine/`](../ahead-agent-engine/)
directly. `src/acp_client.rs` and `src/adapters.rs` let the editor call external
ACP agents. Both paths report events through `src/events.rs`; external agent
processes own their own shell and file effects.

AHEAD-specific engine support lives in the root workspace crates
`ahead-agent-graph-store`, `ahead-agent-skills`, `ahead-model-auth`, and
`ahead-runtime-api`. The engine directory groups retained Cargo packages and
is not a second workspace.
