# AHEAD local agent execution

This retained library provides local process and filesystem capabilities to
AHEAD's in-process managed agent. It is not an executable, standalone agent,
or remote execution service. There is no exec-server client/server, WebSocket
transport, rendezvous relay, or reconnect protocol in AHEAD's managed path.

`ahead-agent` owns durable session integration and calls the managed loop in
this engine directly. The editor reaches it through AHEAD's proxy boundary.
This crate is the local effect backend beneath the loop:

- `ExecBackend` starts local child processes with the selected platform
  sandbox; `ExecProcess` streams output and owns write, signal and termination.
- `ExecutorFileSystem` applies the same local workspace/sandbox context to
  file operations. Sandboxed operations re-enter the `ahead` executable using
  its hidden filesystem helper mode.
- Capability discovery scans selected roots for bounded `SKILL.md` files;
  AHEAD's skill loader remains responsible for standards parsing and policy.

The sandbox and path validation are deliberate security boundaries, not old
approval UI. Removing remote execution did not turn child processes into
unrestricted host operations. External ACP agents remain a separate process
boundary and do not inherit this managed-runtime enforcement.

The shape follows Zed's integrated agent: see its pinned
[`Agent::run_turn`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/agent.rs) and
central [`tools`](https://github.com/zed-industries/zed/blob/418f89714891f9d8105a3e92e60b9a7a5084d232/crates/agent/src/tools.rs)
registry. AHEAD keeps its own managed loop and editor tools rather than
inserting a Codex App Server or generic exec-server hop.

The source is a selectively retained hard fork in the root Cargo workspace.
Internal Codex crate/type names remain only where further renaming would not
reduce behavior or dependency closure; they are not compatibility promises.
The import baseline and required license notices are kept in the parent
[`ahead-agent-engine` directory](../README.md).
