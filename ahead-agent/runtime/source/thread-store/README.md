# Thread Store

`codex-thread-store` defines storage-neutral thread contracts and the
`InMemoryThreadStore` used by focused runtime fixtures. The managed AHEAD
application injects its Turso-backed implementation from `ahead-agent`; this
crate has no local database or rollout-file store.

## Responsibilities

- `ThreadStore::append_items` is the raw canonical history append API. It does
  not infer metadata from item contents.
- `ThreadStore::update_thread_metadata` is the only thread metadata write API.
  It accepts a single literal metadata patch shape, regardless of whether the
  caller is applying a user/API mutation or facts derived above the store from
  appended history.
- `LiveThread` is the preferred API for active session persistence. It owns a
  per-thread metadata sync helper, applies the rollout persistence policy,
  appends canonical history, and then sends metadata patches through
  `ThreadStore::update_thread_metadata`.
- `ThreadManager` routes metadata mutations for loaded and cold threads through
  one entrypoint. Loaded threads use their `LiveThread`; cold threads go
  directly to the store.
- Legacy rollout JSONL is imported by `ahead-agent` into Turso; it is not a
  live session-store fallback.
- `RolloutRecorder` remains a JSONL utility for the retained runtime and import
  compatibility, not an implementation of `ThreadStore`.
- `core/session` creates or resumes `LiveThread` handles and does not need to
  know the concrete implementation. AHEAD injects Turso for managed sessions;
  runtime tests may inject `InMemoryThreadStore`.

## Direction

New metadata observation semantics should live above `ThreadStore`. Stores
persist explicit metadata fields, but raw history appends remain history-only.
