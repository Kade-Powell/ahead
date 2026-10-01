# AHEAD memory

`ahead-memory` retains the model-output citation parser used by the native
agent loop. AHEAD loads project and user `MEMORY.md` files at its integration
boundary; this crate does not own filesystem placement or writes.

The copied Codex extraction/consolidation pipeline and memory-read telemetry
are not shipped. Future indexing and consolidation belong in the AHEAD session
store described in `docs/development/ahead-agent-standards.md`.
