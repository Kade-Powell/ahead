# AHEAD runtime naming

Status: accepted, 2026-09-22.

The embedded runtime that AHEAD carries and operates is the **AHEAD runtime**.
It is the managed, AHEAD-owned tier and is the default for AHEAD teaching and
assistance sessions. It is integrated as a native loop inside `ahead-proxy`,
following Zed's native-agent architecture; it is not a Codex App Server and it
does not communicate with AHEAD through ACP.

In AHEAD product language, **Codex** means the real external Codex agent: an
external ACP agent, adapter, or upstream reference being discussed explicitly.
The embedded runtime's upstream wire vocabulary, license notices, and private
compatibility environment variables remain implementation details; they do not
define the managed runtime's product identity.

User-facing labels, harness kinds, configuration keys, and canonical development
recipes use `ahead` for the managed tier. External-agent labels identify the
external harness and may name Codex when that is the actual agent selected.
