---
name: ahead-agent-remote-tests
description: Test the retained ahead-agent exec-server boundary across local and remote environments
---

Remote-executor tests are opt-in. They are relevant to the retained
`exec-server` and sandbox code, but AHEAD does not currently run the upstream
Docker/Wine lane in CI.

When adding or changing remote-executor behavior:

1. Prefer test builders and fixtures that exercise both local and remote
   execution rather than duplicating local-only tests.
2. Keep tests explicit about their host and target assumptions. A skip must name
   the missing executor or platform capability.
3. Preserve the remote boundary: do not send local-only credentials, path
   conventions or sandbox policy state to the executor unless the protocol
   explicitly requires it.
4. Validate the narrowest affected runtime package first with locked Cargo
   commands. Treat any Docker/Wine or remote-host result as separate evidence
   from local tests.
5. Do not claim cross-platform coverage from a local substitute. Record the
   exact host, target, executor and skip reason when the remote lane is absent.

The upstream Codex procedure used Docker and Wine through Bazel. Those commands
are not available in AHEAD's distilled tree; restore them only when AHEAD owns
the corresponding harness and CI lane.
