# Workflow Specifications

`workflows/` contains the versioned executable contracts for AHEAD phases, artifacts, actors, gates, transitions, returns, and AI capabilities.

`config-v0.schema.json` defines repository configuration, including optional per-workflow work-item boundaries.

Published workflow versions are immutable. Add a new version for changed semantics and retain the old definition under `workflows/legacy/` so existing runs remain replayable.

Keep human-readable workflows and policy fragments aligned. Validate changes with `cargo test --workspace --all-features --locked` and `npm test` from `integrations/pi`.
