#!/usr/bin/env bash
# Source this from the repository root before building ahead-code-mode-host.

case "${AHEAD_V8_TARGET:-$(uname -m)-$(uname -s)}" in
    arm64-Darwin|aarch64-Darwin|aarch64-apple-darwin)
        v8_target=aarch64-apple-darwin
        archive_sha=00adbb48798848c77550441c68673a5e8529b8e1b73eabcdee232cb39b40f4a1
        binding_sha=ca5adf0cf89c9a70ad460ae73648b2fe89b74aa113b3cb7f757b6a02b758394f
        ;;
    x86_64-Darwin|x86_64-apple-darwin)
        v8_target=x86_64-apple-darwin
        archive_sha=e0d9bb64e8b3a034c2930c83972f3f35760211148342fa0407b38250ef330856
        binding_sha=ca5adf0cf89c9a70ad460ae73648b2fe89b74aa113b3cb7f757b6a02b758394f
        ;;
    aarch64-Linux|aarch64-unknown-linux-gnu)
        v8_target=aarch64-unknown-linux-gnu
        archive_sha=d1517eed405468537029b005d5fe997ec74d5c8d351f916b3a6df20b7d2811ba
        binding_sha=7727826ae479bdb645e807239fb12d1f8e2e23de7a6cf16f5ee592690d1d8506
        ;;
    x86_64-Linux|x86_64-unknown-linux-gnu)
        v8_target=x86_64-unknown-linux-gnu
        archive_sha=a35c75d1f26e6a983885a45b33490a4ebe54f05050568b32b89cfb421b30b583
        binding_sha=7727826ae479bdb645e807239fb12d1f8e2e23de7a6cf16f5ee592690d1d8506
        ;;
    *)
        echo "AHEAD Code Mode V8 setup does not support this target" >&2
        return 1
        ;;
esac

if [[ -n "${XDG_CACHE_HOME:-}" ]]; then
    cache_home="$XDG_CACHE_HOME"
elif [[ -n "${HOME:-}" ]]; then
    case "$(uname -s)" in
        Darwin) cache_home="$HOME/Library/Caches" ;;
        Linux) cache_home="$HOME/.cache" ;;
        *)
            echo "AHEAD Code Mode V8 cache location is unsupported on this platform" >&2
            return 1
            ;;
    esac
else
    echo "Set HOME or XDG_CACHE_HOME to cache AHEAD Code Mode V8 assets" >&2
    return 1
fi

v8_cache_root="${AHEAD_V8_CACHE_DIR:-$cache_home/ahead/code-mode-v8}"
if [[ "$v8_cache_root" != /* ]]; then
    echo "AHEAD_V8_CACHE_DIR must be an absolute path" >&2
    return 1
fi
v8_cache="$v8_cache_root/v150.4.0/$v8_target"
mkdir -p "$v8_cache"
v8_base=https://github.com/openai/codex/releases/download/rusty-v8-v150.4.0

ensure_v8_asset() {
    local filename="$1" expected_sha="$2" destination="$v8_cache/$1" temporary
    if [[ -f "$destination" ]] && [[ "$(hash_v8_asset "$destination")" == "$expected_sha" ]]; then
        return 0
    fi
    temporary="$(mktemp "$destination.XXXXXX")"
    if ! curl -fLsS --retry 2 -o "$temporary" "$v8_base/$filename"; then
        rm -f "$temporary"
        return 1
    fi
    if [[ "$(hash_v8_asset "$temporary")" != "$expected_sha" ]]; then
        echo "AHEAD Code Mode V8 checksum mismatch: $filename" >&2
        rm -f "$temporary"
        return 1
    fi
    mv "$temporary" "$destination"
}

hash_v8_asset() {
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    else
        sha256sum "$1" | cut -d ' ' -f 1
    fi
}

if ! ensure_v8_asset "librusty_v8_ptrcomp_sandbox_release_$v8_target.a.gz" "$archive_sha"; then
    return 1
fi
if ! ensure_v8_asset "src_binding_ptrcomp_sandbox_release_$v8_target.rs" "$binding_sha"; then
    return 1
fi
export RUSTY_V8_ARCHIVE="$v8_cache/librusty_v8_ptrcomp_sandbox_release_$v8_target.a.gz"
export RUSTY_V8_SRC_BINDING_PATH="$v8_cache/src_binding_ptrcomp_sandbox_release_$v8_target.rs"
