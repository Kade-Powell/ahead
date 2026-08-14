# Integrations

Integrations connect the reusable Rust/WASM workflow engine to a host without redefining AHEAD semantics.

An adapter supplies identity, timestamps, persistence, UI, and explicit tool-capability mappings. It must preserve human gates and deny unknown model tools until classified.

[`pi/`](pi/README.md) is the first integration. [`vscode/`](vscode/README.md) adapts the same records and engine to VS Code and GitHub Copilot. See the [development guide](../docs/development/README.md) for architecture and contribution guidance.
