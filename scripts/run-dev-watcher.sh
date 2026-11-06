#!/usr/bin/env bash
set -euo pipefail

watcher_pid=""
stop_watcher() {
    if [[ -n "$watcher_pid" ]] && kill -0 "$watcher_pid" 2>/dev/null; then
        kill -TERM "$watcher_pid" 2>/dev/null || true
        wait "$watcher_pid" 2>/dev/null || true
    fi
}

trap 'exit 130' INT
trap 'exit 143' TERM
trap stop_watcher EXIT

"$@" &
watcher_pid=$!
status=0
wait "$watcher_pid" || status=$?
if kill -0 "$watcher_pid" 2>/dev/null; then
    stop_watcher
fi
watcher_pid=""
exit "$status"
