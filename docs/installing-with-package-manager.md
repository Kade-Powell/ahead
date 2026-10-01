## Installation With Package Manager

AHEAD is not yet published to upstream package repositories (the existing `lapce` listings there carry Lapce, the project AHEAD is forked from).

For now, install AHEAD from:

- **GitHub releases** (https://github.com/Kade-Powell/ahead/releases):
  - macOS: `Ahead-macos.dmg`
  - Windows: `Ahead-windows.msi` or `Ahead-windows-portable.zip`
  - Debian/Ubuntu: `.deb` packages built per distribution version
  - Linux (other): `ahead-linux-*.tar.gz`
- **Cargo**, from a checkout:
  ```sh
  cargo install --path . --bin ahead --profile release-lto --locked
  ```
- **macOS bundle**, from a checkout:
  ```sh
  just app
  open target/release-lto/macos/Ahead.app
  ```
