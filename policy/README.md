# Policy

These Markdown fragments become compact instructions for the active AHEAD phase.

- `common.md` defines shared behavior.
- Workflow directories define phase-specific behavior.
- `shared/` holds reusable engineering-tail phases.
- `methods/` holds optional practices mapped by `methods/index.json`.

Policy explains permitted behavior; executable authority, gates, and transitions remain in `spec/workflows`. Edit these sources, never `integrations/pi/generated`, then run `npm test` from `integrations/pi`.
