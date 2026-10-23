# AHEAD runtime config loader

This module loads the runtime's configuration layers and administrator
requirements, then produces:

- An **effective merged** TOML config.
- **Per-key origins** metadata (which layer “wins” for a given key).
- **Per-layer versions** (stable fingerprints) used for optimistic concurrency / conflict detection.

## Public surface

Exported from `codex_config::loader`:

- `load_config_layers_state(fs, codex_home, cwd_opt, cli_overrides, options) -> ConfigLayerStack`
- `ConfigLayerStack`
  - `effective_config() -> toml::Value`
  - `origins() -> HashMap<String, ConfigLayerMetadata>`
  - `layers_high_to_low() -> impl Iterator<Item = &ConfigLayerEntry>`
  - `with_user_config(config_path, user_config) -> ConfigLayerStack`
- `ConfigLayerEntry` (one layer’s `{name, config, version, disabled_reason}`; `name` carries source metadata)
- `ConfigLoadOptions` (user-facing load behavior such as strict config validation)
- `LoaderOverrides` (host paths and test overrides)
- `merge_toml_values(base, overlay)` (public helper used elsewhere)

## Layering model

Precedence is **top overrides bottom**:

1. `SessionFlags` (runtime overrides, applied as dotted-path TOML writes)
2. `User` config (`config.toml` or the host-supplied user config path)
3. `PackagedDefaults` (AHEAD-supplied base values)

`ConfigLayerStack` stores layers in the opposite order internally: lowest
precedence first, highest precedence last, so later layers override earlier
layers when folded. Administrator requirements are separate from ordinary
configuration: Unix reads `/etc/ahead/requirements.toml`, Windows reads
`%ProgramData%\AHEAD\requirements.toml`, and macOS reads the
`com.ahead.editor` managed-preferences key `requirements_toml_base64`.

The executor does not expose a config-read RPC or reopen host configuration;
AHEAD supplies the runtime inputs it needs directly. There is no system or
project config-file layer. The AHEAD native agent receives provider settings
from the host in memory and does not load user config files. Workspace settings
live under `.ahead`; repository instructions and skills use the standard
`AGENTS.md` and `.agents/skills/` paths.

The obsolete `managed_config.toml` file and MDM config-layer formats are not
supported. System and MDM administrator requirements remain separate from
ordinary configuration layers.
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
let cwd = AbsolutePathBuf::current_dir()?;
let layers = load_config_layers_state(
    LOCAL_FS.as_ref(),
    &codex_home,
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
