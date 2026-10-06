# pmpx-project

读取、合并并写入 pmpx 自己的配置文件

`pmpx-project` 掌管 pmpx 的两层配置：全局的 `<config-dir>/pmpx/config.toml`（个人偏好，不提交）与
项目的 `.pmpx.toml`（每个目录一个，应当提交）。它把两者解析成带类型的结构体，把项目层按「就近优先」
合并，并以原子方式写回。它不打印任何东西，也不做任何决策。

## API

- `paths`：`ENV_CONFIG_DIR`（`"PMPX_CONFIG_DIR"`）、`ENV_DATA_DIR`（`"PMPX_DATA_DIR"`）、
  `global_config_path()`、`default_data_dir()`、`expand_tilde(raw)`。
- `GlobalConfig { plugin, discovery, plugin_store, extra }`，带 `load()` 与 `load_from(path)`。
- `GlobalPluginConfig { family_priority, priority }` 与
  `DiscoveryConfig { walk_up, max_depth, stop_at_git }`。
- `PluginStoreConfig { data_dir, prefer_prebuilt }`，带 `effective_data_dir()` 与
  `effective_prefer_prebuilt()`（默认 `true`）。
- `ProjectConfig { plugin, scripts, extra }`，带 `load_from(path) -> Result<Option<Self>>` 与
  `overlay(other)`。
- `Script::{Line(String), Args(Vec<String>)}`，带 `tokens() -> Vec<String>`。
- `MergedProjectConfig { plugin, scripts, sources }`，带
  `from_paths_near_to_far(paths) -> Result<Self>` 与 `pinned_plugin(family)`。
- `atomic_write(path, text) -> Result<()>`。

## 说明

- 合并规则是近到远：`from_paths_near_to_far` 接收近到远的路径，近的一层覆盖远的一层。
- 未知键会被保留：两个配置结构体都带 `extra: toml::Table`，所以 pmpx 不认识的键能活过读-改-写
  （`pmpx config set`、`pmpx plugin set/unset`）。
- `atomic_write` 绝不留下半截文件：它先在同一个目录里写临时文件，再 rename 覆盖目标。
- 两个环境变量覆盖：`PMPX_CONFIG_DIR` 替换全局配置目录，`PMPX_DATA_DIR` 替换插件数据目录。

测试：`src/tests.rs` 与 `src/write.rs` 覆盖各层合并、未知键与原子写入。
License: 与工作区一致（见仓库根目录）。