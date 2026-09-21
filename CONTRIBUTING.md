# How to contribute
Thank you for your interest in contributing to Ahead! No contribution is too small and we consider _all_ contributions to the project. There are many ways to contribute (a few are listed here) but if you think of something else, let us know via an [issue](https://github.com/Kade-Powell/ahead/issues).

Agent-specific guidance (skills, UI policy, workflows, `TODO.md` tracking) lives in [`AGENTS.md`](AGENTS.md) — read it before writing code with an agent.

## Questions

If you're only participating on GitHub, you can open a [Discussion](https://github.com/Kade-Powell/ahead/discussions) or an [issue](https://github.com/Kade-Powell/ahead/issues).

## Feature Requests

A feature request is _editor behaviour that you want to have included in Ahead_. We track feature requests on GitHub via [issues](https://github.com/Kade-Powell/ahead/issues). There are generally few kinds of features:

### Core features

A feature more suited to the core development of Ahead. If this is the case please make a suggestion in an [issue](https://github.com/Kade-Powell/ahead/issues).

### Programming language support (autocompletion/intellisense/formatting)

A feature that relates to specific programming language or development tool that provides intellisense, or various editor commands.
We do not track plugins development here, each plugin should have own issue tracker with eventual issues linked/referenced to the main Ahead issue tracker.

### Syntax highlighting

For syntax highlighting grammar support, please open an [issue](https://github.com/Kade-Powell/ahead/issues) describing the language and the gap.

---

To reduce the number of duplicate requests, please search through the issues to see if something has already been suggested. If a feature you want has been suggested, then comment on that issue to let us know it's popular. You can use emoji-reactions to show us just how popular.

## Bug Reports

Bugs should also be reported on GitHub via [issues](https://github.com/Kade-Powell/ahead/issues). This allows us to track them and see how prevalent they are.

If you encounter a bug when using Ahead, check the issues to see if anyone else has encountered it. If it already exists, you can use emoji reactions so we can see community interest in specific issues and how important they are.

Please follow the rule of [NoPlusOne](https://github.com/golang/go/wiki/NoPlusOne)

## Pull Requests

If you want to write some code, develop the documentation, or otherwise work on a certain feature or bug, let us know by replying to or creating an [issue](https://github.com/Kade-Powell/ahead/issues).

Run `cargo fmt --all` and `cargo clippy` on your code before submitting pull requests and fix any issues; this makes sure that the CI runs only fail on genuine build errors and not formatting/Clippy lints. Keep `TODO.md` accurate per `AGENTS.md` work tracking, and follow the PR hygiene section there (`Release Notes:` footer, imperative title).

We are currently in the process of improving the documentation for new developers/code contributors. Feel free to get started, or open an [issue](https://github.com/Kade-Powell/ahead/issues) to see what can be done.

## Contact

As always, if you have any questions or are just not sure where to start, open an [issue](https://github.com/Kade-Powell/ahead/issues).
