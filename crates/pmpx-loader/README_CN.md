# pmpx-loader

插件 ABI 的宿主侧

`pmpx-loader` 打开插件的 cdylib、协商插件提供的能力表（`identity`、`command`，以及可选的 `attach`），然后调用它。它是本工作区指定的 `unsafe` 孤岛之一（CI 门禁 `unsafe stays in the ABI islands` 把它们全列出来了）。插件通过 context 读到的一切都只为那一次调用复制出来，并在之后释放一次。

## API

- `unsafe fn Plugin::open(path: &Path) -> Result<Plugin, LoadError>` —— 整条加载路径；`Plugin` 持有动态库使其一直有效。
- `Plugin::tables()`、`Plugin::name()`、`Plugin::family()` —— 已加载插件上的访问器。
- `Tables` —— 同一套能力表，但不含库句柄：`unsafe Tables::from_root(...)`、`build_info()`、`attach()`、`call()`。
- `Command { program, args, cwd }` —— 插件的回答，已复制成宿主拥有的 Rust 值；`cwd` 为 `None` 表示项目根目录。
- `ContextSource<'a>` —— 宿主为一次调用能回答的全部内容；`Files::contents(name)` 对每个名字最多问一次。
- `NoFiles` —— 什么都不提供的 `Files`。
- `LoadError` —— `Open`、`NoEntrySymbol`、`Major`、`MissingCapability`、`ShortTable`。
- `CallError` —— `UnsupportedVerb`、`InvalidArgs`、`Internal`、`Unknown(u32)`、`ShortCommand`。

## 说明

- 找库文件是 store 的职责（`crate_plugin_kit::find_library`），加载才是本 crate 的 —— `Plugin::open` 从不搜索。
- 字符串与命令都是跨界复制，插件的 `free_str` / `free_command` 只调用一次；插件的借用不会活过它所在的那次调用。
- 缺少能力表或未知返回码都会被如实报成类型化错误，而不是猜；`abi_major` 与 `PMPX_ABI_MAJOR` 按相等比较。
- `Tables` 与 `Plugin` 持有裸指针，所以两者都不是 `Send` 或 `Sync`。

测试：针对手写能力表的单元测试，以及 `tests/dlopen.rs` 加载真实 cdylib fixture。
License: 与工作区一致（见仓库根目录）。