# macOS build and development recipes for AHEAD.

target := "ahead"
codesign_identity := "FAC8FBEA99169DC1980731029648F110628D6A32"
assets_dir := "extra"
release_dir := "target/release-lto"
app_name := "Ahead.app"
app_template := assets_dir / "macos" / app_name
app_dir := release_dir / "macos"
app_binary := release_dir / target
app_binary_dir := app_dir / app_name / "Contents/MacOS"
dmg_name := "Ahead.dmg"

# Print available recipes.
help:
    @just --list

# Run the hot-recompiling development loop.
dev $workspace='.':
    watchexec --debounce 1s -e rs,toml,lock,md,json,txt,svg,lark,wit,gitignore,icns -w Cargo.toml -w Cargo.lock -w build.rs -w .cargo -w .ahead/.gitignore -w defaults -w icons -w extra -w ahead-app -w ahead-voice -w ahead-proxy -w ahead-core -w ahead-extension-host -w ahead-rpc -w ahead-viewmodel -w ahead-agent -w ahead-tool-records -i '**/tests/**' -i '**/*_tests.rs' -i '**/AGENTS.md' -i '**/README.md' -r -- just _run-dev "$workspace"

# Keep a stable macOS app identity across watcher rebuilds and native UI checks.
[private]
_run-dev $workspace:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ "$(uname -s)" == Darwin ]]; then
        cargo build -p ahead --bin ahead
        dev_bundle="target/debug/macos/Ahead.app"
        mkdir -p "$dev_bundle/Contents/MacOS" "$dev_bundle/Contents/Resources"
        cp extra/macos/Ahead.app/Contents/Info.plist "$dev_bundle/Contents/Info.plist"
        cp extra/macos/Ahead.app/Contents/Resources/ahead.icns "$dev_bundle/Contents/Resources/ahead.icns"
        /bin/cp -c target/debug/ahead "$dev_bundle/Contents/MacOS/ahead"
        exec "$dev_bundle/Contents/MacOS/ahead" "$workspace"
    else
        exec cargo run -p ahead --bin ahead -- "$workspace"
    fi

# Run bacon's default check loop.
bacon:
    bacon

# Run bacon's focused agent, proxy, and viewmodel test loop.
bacon-test:
    bacon test

# Run maintained workspace tests; copied core/MCP harnesses are not maintained.
test-all:
    cargo test --locked --workspace --all-targets --exclude codex-core --exclude codex-mcp

# Rebuild and relaunch the app on change.
bacon-run:
    bacon run

# Build the in-process AHEAD agent and its retained native loop.
ahead-build:
    cargo build -p ahead-agent

# Check the in-process AHEAD agent without building the editor shell.
ahead-check:
    cargo check -p ahead-agent --lib

# Install Ubuntu build dependencies.
ubuntu-deps:
    apt-get update -y
    apt-get install -y clang libxkbcommon-x11-dev pkg-config libvulkan-dev libgtk-3-dev libwayland-dev xorg-dev libxcb-shape0-dev libxcb-xfixes0-dev

# Build a native macOS release binary.
binary:
    MACOSX_DEPLOYMENT_TARGET=10.11 cargo build --profile release-lto
    mkdir -p {{ app_dir }}
    lipo {{ app_binary }} -create -output {{ app_binary }}

# Build a universal macOS release binary.
binary-universal:
    MACOSX_DEPLOYMENT_TARGET=10.11 cargo build --profile release-lto --target=x86_64-apple-darwin
    MACOSX_DEPLOYMENT_TARGET=10.11 cargo build --profile release-lto --target=aarch64-apple-darwin
    mkdir -p {{ app_dir }}
    lipo target/x86_64-apple-darwin/release-lto/{{ target }} target/aarch64-apple-darwin/release-lto/{{ target }} -create -output {{ app_binary }}
    /usr/bin/codesign -vvv --deep --entitlements {{ assets_dir }}/entitlements.plist --strict --options=runtime --force -s {{ codesign_identity }} {{ app_binary }}

# Create a native Ahead.app.
app: binary
    mkdir -p {{ app_binary_dir }}
    cp -fRp {{ app_template }} {{ app_dir }}
    cp -fp {{ app_binary }} {{ app_binary_dir }}
    touch -r {{ app_binary }} {{ app_dir }}/{{ app_name }}
    xattr -c {{ app_dir }}/{{ app_name }}/Contents/Info.plist
    xattr -c {{ app_dir }}/{{ app_name }}/Contents/Resources/ahead.icns
    /usr/bin/codesign -vvv --deep --entitlements {{ assets_dir }}/entitlements.plist --strict --options=runtime --force -s {{ codesign_identity }} {{ app_dir }}/{{ app_name }}

# Create a universal Ahead.app.
app-universal: binary-universal
    mkdir -p {{ app_binary_dir }}
    cp -fRp {{ app_template }} {{ app_dir }}
    cp -fp {{ app_binary }} {{ app_binary_dir }}
    touch -r {{ app_binary }} {{ app_dir }}/{{ app_name }}
    xattr -c {{ app_dir }}/{{ app_name }}/Contents/Info.plist
    xattr -c {{ app_dir }}/{{ app_name }}/Contents/Resources/ahead.icns
    /usr/bin/codesign -vvv --deep --entitlements {{ assets_dir }}/entitlements.plist --strict --options=runtime --force -s {{ codesign_identity }} {{ app_dir }}/{{ app_name }}

# Create a native disk image.
dmg: app
    ln -sf /Applications {{ app_dir }}/Applications
    hdiutil create {{ app_dir }}/{{ dmg_name }} -volname AHEAD -fs HFS+ -srcfolder {{ app_dir }} -ov -format UDZO
    /usr/bin/codesign -vvv --deep --entitlements {{ assets_dir }}/entitlements.plist --strict --options=runtime --force -s {{ codesign_identity }} {{ app_dir }}/{{ dmg_name }}

# Create a universal disk image.
dmg-universal: app-universal
    ln -sf /Applications {{ app_dir }}/Applications
    hdiutil create {{ app_dir }}/{{ dmg_name }} -volname AHEAD -fs HFS+ -srcfolder {{ app_dir }} -ov -format UDZO
    /usr/bin/codesign -vvv --deep --entitlements {{ assets_dir }}/entitlements.plist --strict --options=runtime --force -s {{ codesign_identity }} {{ app_dir }}/{{ dmg_name }}

# Open the native disk image.
install: dmg
    open {{ app_dir }}/{{ dmg_name }}

# Open the universal disk image.
install-universal: dmg-universal
    open {{ app_dir }}/{{ dmg_name }}

# Remove build artifacts.
clean:
    cargo clean
