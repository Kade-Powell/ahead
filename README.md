<h1 align="center">
  <img src="extra/images/logo.png" width=200 height=200/><br>
  Ahead
</h1>

<h4 align="center">Lightning-fast, AI-native Code Editor</h4>

<div align="center">
  <a href="https://github.com/Kade-Powell/ahead/actions/workflows/ci.yml" target="_blank">
    <img src="https://github.com/Kade-Powell/ahead/actions/workflows/ci.yml/badge.svg" />
  </a>
</div>
<br/>

Ahead is an AI-native code editor written in pure Rust, forked from [Lapce](https://github.com/lapce/lapce). The interface is built with GPUI via [gpui-kit](https://gpui-kit.com), and editing stays on a fast foundation: Rope science (`lapce-xi-rope`), a remote-ready proxy, and GPU rendering.

## Features

* Conversational agent panel with integrated plans and streamed execution
* Unified threads sidebar across work sessions and external agent tasks
* True PTY-backed interactive terminal, docked under the editor
* BYOK models: OpenAI-compatible, Ollama, Anthropic direct, LM Studio
* Dock-based layout (explorer, editor, agent, threads) with shortcut tooltips on every control
* Rope-based editing core with WASI plugin groundwork inherited from Lapce

## Installation

Install Ahead directly from GitHub with Cargo:

```sh
cargo install --git https://github.com/Kade-Powell/ahead.git --bin ahead --profile release-lto --locked
```

This installs the `ahead` executable into `$HOME/.cargo/bin`. See the [build guide](docs/building-from-source.md) for operating-system dependencies and development builds.

## Contributing

Guidelines for contributing to Ahead can be found in [`CONTRIBUTING.md`](CONTRIBUTING.md). Agent-specific guidance (skills, UI policy, workflows, work tracking) lives in [`AGENTS.md`](AGENTS.md).

## Feedback & Contact

Open an issue on [GitHub](https://github.com/Kade-Powell/ahead/issues).

## License

Ahead is released under the Apache License Version 2, which is an open source license. You may contribute to this project, or use the code as you please as long as you adhere to its conditions. You can find a copy of the license text here: [`LICENSE`](LICENSE).
