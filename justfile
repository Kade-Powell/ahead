# macOS build and development recipes for AHEAD.

target := "ahead"
codesign_identity := "FAC8FBEA99169DC1980731029648F110628D6A32"
assets_dir := "extra"
release_dir := "target/release-lto"
app_name := "Ahead.app"
app_template := assets_dir / "macos" / app_name
app_dir := release_dir / "macos"
app_binary := release_dir / target
host_binary := release_dir / "ahead-code-mode-host"
app_binary_dir := app_dir / app_name / "Contents/MacOS"
dmg_name := "Ahead.dmg"
shared_repo_root := `git_common_dir="$(git rev-parse --path-format=absolute --git-common-dir)" && dirname "$git_common_dir"`
cargo_target_dir := env_var_or_default("CARGO_TARGET_DIR", shared_repo_root / "target")
export CARGO_TARGET_DIR := cargo_target_dir

# Reuse compiled artifacts across linked worktrees for every Just Cargo recipe.

# Print available recipes.
help:
    @just --list

# Run the hot-recompiling development loop.
dev $workspace='.':
    #!/usr/bin/env bash
    set -euo pipefail
    git_common_dir="$(git rev-parse --path-format=absolute --git-common-dir)"
    lock_key="$(printf '%s' "$git_common_dir" | cksum | cut -d ' ' -f 1)"
    lock_path="${TMPDIR:-/tmp}/ahead-dev-$(id -u)-${lock_key}.lock"
    watch_args=(
        --debounce 2s
        -e rs,toml,lock,md,json,txt,svg,lark,wit,gitignore,icns,sh
        -w Cargo.toml -w Cargo.lock -w build.rs -w .cargo -w .ahead/.gitignore
        -w defaults -w icons -w extra -w scripts
        -w ahead-app -w ahead-voice -w ahead-proxy -w ahead-core
        -w ahead-extension-host -w ahead-rpc -w ahead-viewmodel -w ahead-agent
        -w ahead-agent-engine -w ahead-agent-graph-store -w ahead-agent-skills
        -w ahead-model-auth -w ahead-runtime-api
        -w ahead-code-mode-host -w ahead-code-mode-runtime
        -i '**/tests/**' -i '**/*_tests.rs' -i '**/AGENTS.md' -i '**/README.md'
        -i 'TODO.md' -i 'docs/development/**' -i 'docs/building-from-source.md'
        -r -- just _run-dev "$workspace"
    )
    if [[ "$(uname -s)" == Darwin ]]; then
        # Native macOS watching missed atomically replaced Rust source files in this checkout.
        watch_args=(--poll 1s "${watch_args[@]}")
    fi
    report_existing_loop() {
        printf '%s\n' "An AHEAD dev loop already holds this repository's build lock:" "$lock_path" >&2
        if command -v lsof >/dev/null 2>&1; then
            lsof -nP "$lock_path" >&2 || true
        elif command -v fuser >/dev/null 2>&1; then
            fuser -v "$lock_path" >&2 || true
        fi
        printf '%s\n' 'Reuse its existing dev terminal; do not start another Cargo loop.' >&2
    }
    if command -v lockf >/dev/null 2>&1; then
        if lockf -t 0 -k "$lock_path" watchexec "${watch_args[@]}"; then
            exit 0
        else
            status=$?
            if [[ "$status" -eq 75 ]]; then
                report_existing_loop
            fi
            exit "$status"
        fi
    elif command -v flock >/dev/null 2>&1; then
        if flock -n -E 75 "$lock_path" watchexec "${watch_args[@]}"; then
            exit 0
        else
            status=$?
            if [[ "$status" -eq 75 ]]; then
                report_existing_loop
            fi
            exit "$status"
        fi
    else
        printf '%s\n' 'just dev requires lockf or flock to prevent duplicate Cargo watchers' >&2
        exit 127
    fi

# Verify that a second development loop is refused before Cargo starts.
dev-lock-smoke:
    bash tests/just-dev-lock-smoke.sh

# Keep a distinct, stable macOS dev identity across watcher rebuilds.
[private]
_run-dev $workspace:
    #!/usr/bin/env bash
    set -euo pipefail
    git_common_dir="$(git rev-parse --path-format=absolute --git-common-dir)"
    shared_repo_root="$(dirname "$git_common_dir")"
    export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$shared_repo_root/target}"
    if [[ -n "${AHEAD_DATA_HOME:-}" ]]; then
        profile_root="${AHEAD_DATA_HOME%/}"
        if [[ -z "$profile_root" ]]; then
            profile_root="/"
        fi
        if [[ "$profile_root" == "/" ]]; then
            echo "AHEAD_DATA_HOME must be a dedicated directory, not /" >&2
            exit 2
        fi
        export AHEAD_USER_HOME="${AHEAD_USER_HOME:-$profile_root/user-home}"
        export AHEAD_HOME="${AHEAD_HOME:-$profile_root/runtime-home}"
    fi
    if [[ "$(uname -s)" == Darwin ]]; then
        source scripts/ensure-code-mode-v8.sh
        cargo build -p ahead -p ahead-code-mode-host --bins
        dev_bundle="target/debug/macos/Ahead.app"
        mkdir -p "$dev_bundle/Contents/MacOS" "$dev_bundle/Contents/Resources"
        cp extra/macos/Ahead.app/Contents/Info.plist "$dev_bundle/Contents/Info.plist"
        /usr/bin/plutil -replace CFBundleIdentifier -string io.ahead.dev "$dev_bundle/Contents/Info.plist"
        /usr/bin/plutil -replace CFBundleDisplayName -string 'AHEAD Dev' "$dev_bundle/Contents/Info.plist"
        /usr/bin/plutil -replace CFBundleName -string 'AHEAD Dev' "$dev_bundle/Contents/Info.plist"
        cp extra/macos/Ahead.app/Contents/Resources/ahead.icns "$dev_bundle/Contents/Resources/ahead.icns"
        /bin/cp -c target/debug/ahead "$dev_bundle/Contents/MacOS/ahead"
        /bin/cp -c target/debug/ahead-code-mode-host "$dev_bundle/Contents/MacOS/ahead-code-mode-host"
        exec "$dev_bundle/Contents/MacOS/ahead" "$workspace"
    else
        source scripts/ensure-code-mode-v8.sh
        cargo build -p ahead -p ahead-code-mode-host --bins
        exec target/debug/ahead "$workspace"
    fi

# Run bacon's default check loop.
bacon:
    bacon

# Run bacon's focused agent, proxy, and viewmodel test loop.
bacon-test:
    bacon test

# Run maintained AHEAD tests and targeted retained-runtime boundary regressions.
test-all:
    #!/usr/bin/env bash
    set -euo pipefail
    source scripts/ensure-code-mode-v8.sh
    test_profile="$(mktemp -d)"
    trap 'rm -r "$test_profile"' EXIT
    export AHEAD_DATA_HOME="$test_profile/data"
    export AHEAD_USER_HOME="$test_profile/user"
    unset AHEAD_HOME
    if [[ "$(uname -s)" == "Darwin" || "$(uname -s)" == "Linux" ]]; then
        cargo build --locked -p ahead -p ahead-code-mode-host --bins
    else
        cargo build --locked -p ahead --bin ahead
    fi
    RUST_TEST_THREADS=1 cargo test --locked --all-targets -p ahead -p ahead-app -p ahead-voice -p ahead-proxy -p ahead-rpc -p ahead-core -p ahead-extension-host -p ahead-viewmodel -p ahead-agent -p ahead-agent-graph-store -p ahead-agent-skills -p ahead-model-auth -p ahead-runtime-api -p ahead-code-mode-host -p ahead-code-mode-runtime -p codex-apply-patch
    RUST_TEST_THREADS=1 cargo test --locked -p codex-api --test ahead_error_context

# Run deterministic managed-tool attribution and direct apply_patch acceptance.

# Requires an already-built proxy binary; this recipe does not start Cargo.
agent-attribution-smoke:
    #!/usr/bin/env bash
    set -euo pipefail
    binary="{{ cargo_target_dir }}/debug/ahead"
    if [[ ! -x "$binary" ]]; then
        printf 'Missing %s; build through the existing just dev loop first.\n' "$binary" >&2
        exit 2
    fi
    AHEAD_ATTRIBUTION_SMOKE_ROOT="{{ cargo_target_dir }}" node tests/agent-file-attribution-smoke.mjs "$binary"

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
    #!/usr/bin/env bash
    set -euo pipefail
    source scripts/ensure-code-mode-v8.sh
    MACOSX_DEPLOYMENT_TARGET=10.11 cargo build --profile release-lto -p ahead -p ahead-code-mode-host --bins
    mkdir -p {{ app_dir }}

# Build a universal macOS release binary.
binary-universal:
    #!/usr/bin/env bash
    set -euo pipefail
    for AHEAD_V8_TARGET in x86_64-apple-darwin aarch64-apple-darwin; do
        export AHEAD_V8_TARGET
        source scripts/ensure-code-mode-v8.sh
        MACOSX_DEPLOYMENT_TARGET=10.11 cargo build --profile release-lto --target="$AHEAD_V8_TARGET" -p ahead -p ahead-code-mode-host --bins
    done
    mkdir -p {{ app_dir }}
    lipo target/x86_64-apple-darwin/release-lto/{{ target }} target/aarch64-apple-darwin/release-lto/{{ target }} -create -output {{ app_binary }}
    lipo target/x86_64-apple-darwin/release-lto/ahead-code-mode-host target/aarch64-apple-darwin/release-lto/ahead-code-mode-host -create -output {{ host_binary }}
    /usr/bin/codesign -vvv --deep --entitlements {{ assets_dir }}/entitlements.plist --strict --options=runtime --force -s {{ codesign_identity }} {{ app_binary }}

# Create a native Ahead.app.
app: binary
    mkdir -p {{ app_binary_dir }}
    cp -fRp {{ app_template }} {{ app_dir }}
    cp -fp {{ app_binary }} {{ app_binary_dir }}
    cp -fp {{ host_binary }} {{ app_binary_dir }}
    touch -r {{ app_binary }} {{ app_dir }}/{{ app_name }}
    xattr -c {{ app_dir }}/{{ app_name }}/Contents/Info.plist
    xattr -c {{ app_dir }}/{{ app_name }}/Contents/Resources/ahead.icns
    /usr/bin/codesign -vvv --deep --entitlements {{ assets_dir }}/entitlements.plist --strict --options=runtime --force -s {{ codesign_identity }} {{ app_dir }}/{{ app_name }}

# Create a universal Ahead.app.
app-universal: binary-universal
    mkdir -p {{ app_binary_dir }}
    cp -fRp {{ app_template }} {{ app_dir }}
    cp -fp {{ app_binary }} {{ app_binary_dir }}
    cp -fp {{ host_binary }} {{ app_binary_dir }}
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
