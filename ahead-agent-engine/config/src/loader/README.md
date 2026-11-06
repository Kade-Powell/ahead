# Embedded AHEAD runtime config loader

This module loads the runtime's configuration layers and administrator
requirements, then produces:

- An **effective merged** TOML config.
- **Per-key origins** metadata (which layer “wins” for a given key).
- **Per-layer versions** (stable fingerprints) used for optimistic concurrency / conflict detection.

## Public surface

Exported from the retained runtime's `codex_config::loader` module:

- `load_config_layers_state(fs, runtime_home, cwd, cli_overrides, overrides) -> ConfigLayerStack`
- `ConfigLayerStack`
  - `effective_config() -> toml::Value`
  - `origins() -> HashMap<String, ConfigLayerMetadata>`
  - `layers_high_to_low() -> impl Iterator<Item = &ConfigLayerEntry>`
- `ConfigLayerEntry` (one layer’s `{name, config, version, disabled_reason}`; `name` carries source metadata)
- `LoaderOverrides` (host paths and test overrides)
- `merge_toml_values(base, overlay)` (public helper used elsewhere)

## Layering model

Precedence is **top overrides bottom**:

1. `SessionFlags` (runtime overrides, applied as dotted-path TOML writes)
2. `User` config (`config.toml` or the host-supplied user config path)
3. `PackagedDefaults` (AHEAD-supplied base values)

`ConfigLayerStack` stores layers in the opposite order internally: lowest
precedence first, highest precedence last, so later layers override earlier
layers when folded. The retained API still names its runtime-root argument
`codex_home`; AHEAD passes its isolated runtime home there. The name is
inherited from the runtime source and does not identify a separate Codex
installation or account.

The managed AHEAD client builds this stack from embedded defaults and
host-derived runtime overrides. It sets `ignore_user_config` and
`ignore_managed_requirements`, so it does not read a runtime `config.toml`,
project config file, system requirements file, or macOS managed-preferences
requirements. Provider settings arrive from AHEAD in memory. Workspace
settings live under `.ahead`; repository instructions and skills use the
standard `AGENTS.md` and `.agents/skills/` paths. Host policy is enforced at
the AHEAD boundary rather than delegated to this retained loader.

The obsolete `managed_config.toml` file and MDM config-layer formats are not
supported. Generic administrator-requirement loading remains in this library,
but the managed AHEAD runtime explicitly opts out of those inputs.
The copied thread-config injection path, gRPC transport and generated protocol
have been removed; AHEAD has no remote config-service boundary.

Layers with a `disabled_reason` are still surfaced for UI, but are ignored when
computing the effective config and origins metadata. This is what
`ConfigLayerStack::effective_config()` implements.

## Typical usage

Most callers want the effective config plus metadata:

```rust
use codex_config::LoaderOverrides;
use codex_config::loader::load_config_layers_state;
use codex_exec_server::LOCAL_FS;
use codex_utils_absolute_path::AbsolutePathBuf;
use toml::Value as TomlValue;

let cli_overrides: Vec<(String, TomlValue)> = Vec::new();
let runtime_home = AbsolutePathBuf::current_dir()?;
let cwd = AbsolutePathBuf::current_dir()?;
let layers = load_config_layers_state(
    LOCAL_FS.as_ref(),
    runtime_home.as_path(),
    Some(cwd),
    &cli_overrides,
    LoaderOverrides::default(),
).await?;

let effective = layers.effective_config();
let origins = layers.origins();
let layers_for_ui = layers.layers_high_to_low().collect::<Vec<_>>();
```

## Internal layout

Implementation is split by concern:

- `state.rs`: public types (`ConfigLayerEntry`, `ConfigLayerStack`) + merge/origins convenience methods.
- `layer_io.rs`: reading TOML config layers.
- `tests.rs`: config-layer precedence and file-loading regressions.
- `overrides.rs`: CLI dotted-path overrides → TOML “session flags” layer.
- `merge.rs`: recursive TOML merge.
- `fingerprint.rs`: stable per-layer hashing and per-key origins traversal.
- `macos.rs`: administrator requirements from managed preferences (macOS only).
