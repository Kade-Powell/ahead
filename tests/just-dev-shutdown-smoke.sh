#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/ahead-dev-shutdown-test.XXXXXX")"
ready_path="$test_root/starts"
stopped_path="$test_root/stops"
pid_path="$test_root/service-pid"
trigger_path="$test_root/trigger"
runner_pid=""

cleanup() {
    if [[ -n "$runner_pid" ]] && kill -0 "$runner_pid" 2>/dev/null; then
        kill -TERM "$runner_pid" 2>/dev/null || true
        wait "$runner_pid" 2>/dev/null || true
    fi
    if [[ -f "$pid_path" ]]; then
        service_pid="$(cat "$pid_path")"
        kill -TERM "$service_pid" 2>/dev/null || true
    fi
    rm -rf "$test_root"
}
trap cleanup EXIT

if ! command -v watchexec >/dev/null 2>&1; then
    printf 'just dev shutdown smoke requires watchexec.\n' >&2
    exit 127
fi

: > "$trigger_path"
bash "$repo_root/scripts/run-dev-watcher.sh" \
    watchexec --watch "$trigger_path" --restart --debounce 100ms \
    --stop-timeout 2s --shell none -- \
    bash "$repo_root/scripts/run-dev-watcher.sh" \
    python3 -c '
from pathlib import Path
import os
import signal
import sys
import time

starts_path, stops_path, pid_path = map(Path, sys.argv[1:])
generation = int(starts_path.read_text()) + 1 if starts_path.exists() else 1
def stop(*_):
    with stops_path.open("a") as stopped:
        stopped.write(f"{generation}\n")
    raise SystemExit(0)

signal.signal(signal.SIGTERM, stop)
signal.signal(signal.SIGINT, stop)
pid_path.write_text(str(os.getpid()))
starts_path.write_text(str(generation))
while True:
    time.sleep(3600)
' "$ready_path" "$stopped_path" "$pid_path" &
runner_pid=$!

wait_for_generation() {
    expected="$1"
    for attempt in {1..100}; do
        if [[ -f "$ready_path" ]] && [[ "$(cat "$ready_path")" == "$expected" ]]; then
            return 0
        fi
        if ! kill -0 "$runner_pid" 2>/dev/null; then
            break
        fi
        sleep 0.05
    done
    printf 'watchexec did not reach generation %s.\n' "$expected" >&2
    return 1
}

wait_for_stops() {
    expected="$1"
    for attempt in {1..100}; do
        if [[ -f "$stopped_path" ]] && [[ "$(wc -l < "$stopped_path")" -ge "$expected" ]]; then
            return 0
        fi
        sleep 0.05
    done
    printf 'the nested reaper stopped fewer than %s command(s).\n' "$expected" >&2
    return 1
}

wait_for_generation 1
printf 'restart\n' >> "$trigger_path"
wait_for_generation 2
wait_for_stops 1

kill -TERM "$runner_pid"
runner_status=0
wait "$runner_pid" || runner_status=$?
runner_pid=""
if [[ "$runner_status" -ne 143 ]]; then
    printf 'the watcher wrapper exited with unexpected status %s.\n' "$runner_status" >&2
    exit 1
fi
wait_for_stops 2

printf 'Watchexec restart and shutdown both stopped the nested command (wrapper status %s).\n' "$runner_status"
