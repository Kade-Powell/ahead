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
dev:
    watchexec -e rs,toml -w ahead-app -w ahead-proxy -w ahead-core -w ahead-rpc -w ahead-viewmodel -r -- cargo run --bin ahead

# Run bacon's default check loop.
bacon:
    bacon

# Run bacon's proxy and viewmodel test loop.
bacon-test:
    bacon test

# Rebuild and relaunch the app on change.
bacon-run:
    bacon run

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
