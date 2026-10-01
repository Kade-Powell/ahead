---
name: ahead-agent-path-types
description: Choose safe path representations when editing ahead-agent or its distilled runtime
---

Use host-local `PathBuf` or the repository's absolute-path type for filesystem
operations. Use URI/path-wrapper types only at a protocol boundary that must
represent a foreign platform; convert once at the boundary and keep internal
operations typed.

Model-generated tool arguments should remain ordinary `String` values with
feature-specific path handling. Do not silently reinterpret a relative path as
a host path before the runtime has established its workspace.

Path conversion and sandbox-boundary errors fail closed for security-relevant
operations. UI diagnostics may surface a lossy representation, but persisted
session/rollout data must use the established AHEAD contract rather than
introducing a new URI format casually.

Before changing a path-bearing type, check native tools, ACP serialization,
workspace scoping, persisted sessions and Windows/macOS/Linux behavior.
