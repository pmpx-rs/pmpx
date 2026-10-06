# pmpx-plugin

— 插件作者实现的契约

这是插件作者实现的契约：一个 `PackageManager` trait，以及把它在 `dlopen` 边界上安全搬运过去的外壳。插件 crate 是一个纯 rlib，其 `pub fn create() -> Box<dyn PackageManager>` 就是生成出来的包装层所调用的入口。这个 crate 没有特性开关，只依赖 `pmpx-plugin-abi`，并保持 `rust-version = "1.82"`，因为这条 MSRV 决定谁能写插件。

## API

- `PackageManager` —— 实现 `name`、`family` 与 `command`。
- `Context<'a>` —— `project_root`、`start_dir`、`matched`、`config_files`、`pins`、`reason`、`score`，以及方法 `has_matched`、`was_pinned`、`pinned_for`、`file`、`file_str`。
- `CommandSpec` —— `new(program)`、`arg`、`args`、`cwd`，公开字段 `program`、`args` 与 `cwd`。
- `Verb` —— 封闭集合 `Install`、`Remove`、`Update`、`Run`、`Build`、`Test`、`Exec`，外加 `ALL`、`to_abi`、`from_abi` 与 `as_str`。
- `Family` —— 开放字符串类型，常量有 `NODE`、`RUST`、`PYTHON`、`GO`、`JVM`、`DOTNET`、`PHP`、`RUBY`，以及 `new(name)`。
- `PluginError` —— `UnsupportedVerb`、`InvalidArgs`、`Other`，配 `unsupported_verb`、`invalid_args`、`other`、`code()` 与 `message()`。
- `debug!` / `info!` / `warn!` / `error!` —— 日志宏，以及它们调用的 `debug` 模块（`Level`、`wants`、`emit`、`context`）。
- `export!`、`shell` 与 `abi` —— 生成 ABI 的宏、插件侧的辅助函数（`dispatch`、`guard`、`PANIC_MARKER`、`BUILD_RUSTC`、`BUILD_TARGET`），以及线格式。

## 说明

- `export!` 写在被编译成 cdylib 的包装层 crate 里，而不是插件 crate 里：两处都写会定义两次 `pmpx_plugin_entry_v3`，在链接期失败。
- `command()` 是纯映射——不读文件、不看环境、不启动子进程、不访问网络；项目信息只通过 `Context::matched` 与声明过的 `[context] files` 抵达，没声明的名字永远读不到。
- `command` 里的 panic 变成 `PMPX_ERR_INTERNAL`，`name` 或 `family` 里的 panic 变成 `PANIC_MARKER`；trait 是同步的，没有 `async`，也没有回调。
- 这个 crate 需要 `std`（只有 `pmpx-plugin-abi` 是 `no_std`），而 `Context::builder()` 可以在插件自己的测试里手工拼一个上下文。

测试：`cargo test -p pmpx-plugin` —— 代码旁的单元测试，加上 `tests/in_process.rs`、`tests/short_log_table.rs` 与 `tests/dlopen.rs`（后者把 `tests/fixtures/toy-plugin` 构建成 cdylib 并加载它）。
License: 与工作区一致（见仓库根目录）。