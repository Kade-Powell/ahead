<h1 align="center">
  <img src="extra/images/logo.svg" width="128" height="128" alt="AHEAD logo"/><br>
  AHEAD
</h1>

<h4 align="center">A native code editor for human-led development</h4>

<div align="center">
  <a href="https://github.com/Kade-Powell/ahead/actions/workflows/ci.yml" target="_blank">
    <img src="https://github.com/Kade-Powell/ahead/actions/workflows/ci.yml/badge.svg" />
  </a>
</div>
<br/>

AHEAD is an AI-native code editor written in pure Rust, forked from [Lapce](https://github.com/lapce/lapce). The interface is built with GPUI via [gpui-kit](https://gpui-kit.com), and editing stays on a fast foundation: Rope science (`lapce-xi-rope`), a remote-ready proxy, and GPU rendering.

## Why AHEAD

AI can help engineers search broadly, compare options, preserve evidence, and move faster. But using AI indiscriminately can make people fall further behind by outsourcing the understanding and judgment needed to debug, maintain, and improve a system. AHEAD uses AI to extend human capability while keeping humans responsible for framing the problem, making decisions, understanding the result, and reviewing consequential work.

The working principle is:

> Human thinks first → AI amplifies and challenges → Human decides.

That is what AHEAD means: **Assisted Human Engineering and Development**. We aim to get ahead by compounding human mastery with machine leverage, not by maximizing AI autonomy. The [AHEAD Constitution](CONSTITUTION.md) records the durable principles.

## Features

* Conversational agent panel with integrated plans and streamed execution
* Unified threads sidebar across work sessions and external agent tasks
* True PTY-backed interactive terminal, docked under the editor
* BYOK models: OpenAI-compatible, Ollama, Anthropic direct, LM Studio
* Dock-based layout (explorer, editor, agent, threads) with shortcut tooltips on every control
* Rope-based editing core with WASI plugin groundwork inherited from Lapce

## Installation

Install AHEAD directly from GitHub with Cargo:

```sh
cargo install --git https://github.com/Kade-Powell/ahead.git --bin ahead --profile release-lto --locked
```

This installs the `ahead` executable into `$HOME/.cargo/bin`. See the [installation notes](docs/installing-with-package-manager.md) for checkout builds and packaging commands.

## Contributing

Guidelines for contributing to AHEAD can be found in [`CONTRIBUTING.md`](CONTRIBUTING.md). Agent-specific guidance (skills, UI policy, workflows, work tracking) lives in [`AGENTS.md`](AGENTS.md).

The [brand and theme guide](docs/development/branding.md) covers the shared logo assets, colors, and their use in the editor.

## Feedback & Contact

Open an issue on [GitHub](https://github.com/Kade-Powell/ahead/issues).

## License

AHEAD is released under the Apache License Version 2, which is an open source license. You may contribute to this project, or use the code as you please as long as you adhere to its conditions. You can find a copy of the license text here: [`LICENSE`](LICENSE).
