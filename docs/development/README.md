# Developing AHEAD

Audience: AHEAD framework and tooling maintainers

This is the starting point for changing AHEAD itself: its methodology, executable workflows, instruction system, integrations, tests, or publication process. It is not practitioner workflow guidance and is excluded from the Pi runtime reference catalog.

## Methodology design

- [Process taxonomy](process-taxonomy.md) defines the six workflow families and the test for adding another.
- [Debugging and operational investigation](debugging-and-operations.md) preserves the design reasoning behind those two flows.
- [Adapted skill guidance](adapted-skill-guidance.md) records reviewed external ideas and how AHEAD changed them.

## Executable framework

- [Executable workflows](executable-workflows.md) defines sources of truth, state, enforcement, trust boundaries, and reuse.
- [Instruction authoring](instruction-authoring.md) defines how canonical policy becomes compact agent context.
- [Review workbench](review-workbench.md) defines the editor-neutral review contract and Pi host behavior.
- [Pi integration development](pi-integration.md) covers local builds, tests, and packaging.
- [Releasing the Pi extension](releasing-pi.md) defines trusted npm publishing and rollback.
- [Releasing the VS Code extension](releasing-vscode.md) defines Marketplace publishing and identity setup.

## Repository source map

| Concern | Source |
|---|---|
| Practitioner framework | `CONSTITUTION.md`, `docs/guide/**` |
| Evidence and provenance | `docs/evidence/**` |
| Maintainer documentation | `docs/development/**` |
| Executable workflow contracts | `spec/workflows/**` |
| Generated agent-policy sources | `policy/**` |
| Deterministic workflow engine | `crates/ahead-core` |
| WebAssembly boundary | `crates/ahead-wasm` |
| Host integrations | `integrations/pi`, `integrations/vscode` |
| Reference and instruction generator | `scripts/build-instructions.mjs` |

## Documentation distribution rule

The Pi reference generator packages the Constitution, practitioner guide, and evidence library. Each packaged entry declares its audience, authority, and distribution. Maintainer documents are repository-only. A document that mixes audiences should be split before it is added to the catalog.
