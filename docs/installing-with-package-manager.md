## Installation With Package Manager

Ahead is not yet published to upstream package repositories (the existing `lapce` listings there carry Lapce, the project Ahead is forked from).

For now, install Ahead from:

- **GitHub releases** (https://github.com/Kade-Powell/ahead/releases):
  - macOS: `Ahead-macos.dmg`
  - Windows: `Ahead-windows.msi` or `Ahead-windows-portable.zip`
  - Debian/Ubuntu: `.deb` packages built per distribution version
  - Linux (other): `ahead-linux-*.tar.gz`
- **Cargo**, from a checkout (see [Building from source](building-from-source.md)):
  ```sh
  cargo install --path . --bin ahead --profile release-lto --locked
  ```
- **macOS bundle**, from a checkout:
  ```sh
  just app
  open target/release-lto/macos/Ahead.app
  ```
