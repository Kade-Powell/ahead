# Managed agent tool contracts

`codex-tools` holds the retained model-facing tool types shared by the managed
engine. Its package name records source provenance; AHEAD owns the integration
through `ahead-agent` and the editor-facing proxy.

The crate contains `ToolSpec`, `ToolCall`, `ToolOutput`, and `ToolExecutor`;
tool discovery models; and adapters for schema sanitization, MCP and dynamic
tools, Code Mode, and image detail. `codex-core` still owns tool selection,
dispatch, and turn orchestration. AHEAD-specific editor capabilities belong at
the `ahead-agent` integration boundary unless the managed model loop needs a
change to its tool contract.

Changes here can alter model-visible tool names, descriptions, schemas, or
results. Keep those contracts aligned with the pinned engine while pruning
unused host plumbing.
