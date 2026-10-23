#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
git_common_dir="$(git rev-parse --path-format=absolute --git-common-dir)"
lock_key="$(printf '%s' "$git_common_dir" | cksum | cut -d ' ' -f 1)"
lock_path="${TMPDIR:-/tmp}/ahead-dev-$(id -u)-${lock_key}.lock"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/ahead-dev-lock-test.XXXXXX")"
ready_path="$test_root/ready"
release_path="$test_root/release"
lock_holder_pid=""

cleanup() {
    if [[ -e "$ready_path" && -p "$release_path" ]]; then
        printf 'release\n' >"$release_path"
    fi
    if [[ -n "$lock_holder_pid" ]]; then
        wait "$lock_holder_pid" 2>/dev/null || true
    fi
    rm -r "$test_root"
}
trap cleanup EXIT

mkdir "$test_root/bin"
mkfifo "$release_path"
ln -s /usr/bin/true "$test_root/bin/watchexec"

lock_command='touch "$1"; IFS= read -r _ < "$2"'
if command -v lockf >/dev/null 2>&1; then
    lockf -t 0 -k "$lock_path" sh -c "$lock_command" sh "$ready_path" "$release_path" &
elif command -v flock >/dev/null 2>&1; then
    flock -n "$lock_path" sh -c "$lock_command" sh "$ready_path" "$release_path" &
else
    printf 'just dev requires lockf or flock; cannot test its lock guard.\n' >&2
    exit 2
fi
lock_holder_pid=$!

for attempt in {1..50}; do
    [[ -e "$ready_path" ]] && break
    if ! kill -0 "$lock_holder_pid" 2>/dev/null; then
        break
    fi
    sleep 0.05
done

if [[ ! -e "$ready_path" ]]; then
    printf 'Could not acquire the AHEAD dev lock; stop any existing watcher before this test.\n' >&2
    exit 2
fi

cd "$repo_root"
set +e
PATH="$test_root/bin:$PATH" just dev >"$test_root/dev.log" 2>&1
dev_status=$?
set -e

if [[ "$dev_status" -eq 0 ]]; then
    printf 'just dev bypassed its lock and reached the inert watchexec stub:\n' >&2
    cat "$test_root/dev.log" >&2
    exit 1
fi

if [[ "$dev_status" -ne 75 ]] \
    || ! grep -Fq "An AHEAD dev loop already holds this repository's build lock" "$test_root/dev.log"; then
    printf 'just dev failed for an unexpected reason (exit %s):\n' "$dev_status" >&2
    cat "$test_root/dev.log" >&2
    exit 1
fi

printf 'just dev refused a second watcher before starting Cargo (exit %s).\n' "$dev_status"
