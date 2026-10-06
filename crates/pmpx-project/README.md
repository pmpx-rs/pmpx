# pmpx-project

Reads, merges and writes pmpx's own configuration files

`pmpx-project` owns pmpx's two configuration layers: the global `<config-dir>/pmpx/config.toml`
(personal, not committed) and the project's `.pmpx.toml` files (one per directory, meant to be
committed). It parses them into typed structs, merges the project layers nearest-wins, and writes them
back atomically. It prints nothing and decides nothing.

## API

- `paths`: `ENV_CONFIG_DIR` (`"PMPX_CONFIG_DIR"`), `ENV_DATA_DIR` (`"PMPX_DATA_DIR"`),
  `global_config_path()`, `default_data_dir()`, `expand_tilde(raw)`.
- `GlobalConfig { plugin, discovery, plugin_store, extra }` with `load()` and `load_from(path)`.
- `GlobalPluginConfig { family_priority, priority }` and
  `DiscoveryConfig { walk_up, max_depth, stop_at_git }`.
- `PluginStoreConfig { data_dir, prefer_prebuilt }` with `effective_data_dir()` and
  `effective_prefer_prebuilt()` (default `true`).
- `ProjectConfig { plugin, scripts, extra }` with `load_from(path) -> Result<Option<Self>>` and
  `overlay(other)`.
- `Script::{Line(String), Args(Vec<String>)}` with `tokens() -> Vec<String>`.
- `MergedProjectConfig { plugin, scripts, sources }` with
  `from_paths_near_to_far(paths) -> Result<Self>` and `pinned_plugin(family)`.
- `atomic_write(path, text) -> Result<()>`.

## Notes

- The merge is near to far: `from_paths_near_to_far` takes the paths near to far and lets a nearer layer
  override a farther one.
- Unknown keys are preserved: both config structs carry `extra: toml::Table`, so a key pmpx does not
  recognise survives read-modify-write (`pmpx config set`, `pmpx plugin set/unset`).
- `atomic_write` never leaves a half-written file: it writes a temporary file in the same directory and
  renames it over the target.
- Two environment overrides: `PMPX_CONFIG_DIR` replaces the global config directory, `PMPX_DATA_DIR`
  replaces the plugin data directory.

Tests: `src/tests.rs` and `src/write.rs` cover the layers, the unknown keys and the atomic write.
License: same as the workspace (see the repository root).