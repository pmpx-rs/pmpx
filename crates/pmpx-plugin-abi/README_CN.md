# pmpx-plugin-abi

— pmpx 与插件之间的原始 C ABI

这是 `dlopen` 边界两侧共同编译的线格式：`#[repr(C)]` 结构体、与之配套的数字、宿主可能回答的上下文键名，以及两侧各自可能提供的能力名。它不依赖任何东西、保持 `no_std`，并且可以手工转写成 C，所以用 C、Zig 或 Go 写插件是受支持的目标。插件作者写的安全表面在 `pmpx-plugin`，宿主一侧在 `pmpx-loader`。它的 `rust-version` 是 1.82，与 `pmpx-plugin` 同一底线，因为它的 MSRV 决定谁能写插件。

## API

- `PmpxStr` / `PmpxSlice<T>` —— 借用的字节与切片视图。
- `PmpxContext`、`PmpxHost`、`PmpxPlugin`、`PmpxCommand`、`PmpxCommandCap`、`PmpxIdentity`、`PmpxAttach`、`PmpxLog` —— 各张表，外加 `PmpxCommandCap::run` 的别名 `RunFn`。
- `PMPX_ABI_MAJOR`、`PMPX_ENTRY_SYMBOL`、`PMPX_MAX_ITEMS`、`PMPX_OK`、`PMPX_ERR_*`、`PMPX_VERB_*`、`PMPX_REASON_*`、`PMPX_LEVEL_*` —— 各种数字。
- `PMPX_KEY_*` 与 `PMPX_CAP_*` —— 键名与能力名，外加 `PMPX_KEYS`、`PMPX_CAPS` 与 `PMPX_REQUIRED_CAPS` 列表。
- `surface::write_snapshot` / `surface::write_c_header` —— 生成 `abi/surface.toml` 与 `include/pmpx_plugin.h` 的渲染器。
- `bytes_to_os` / `os_to_bytes` —— 位于默认关闭的 `std` 特性之后。

## 说明

- 缺席与空值不同：`ptr == null` 表示宿主在这个键下什么都没有（不是错误），而 `len == 0` 且 `ptr` 非空表示值存在且为空。
- 内存由分配它的一侧释放，借用视图只在它随之到来的那次调用期间有效，panic 不得跨越 `extern "C"`。
- 两个方向上“不认识”都不是错误：不认识的键回答缺席，插件对不认识的动词必须回答 `PMPX_ERR_UNSUPPORTED_VERB`。
- `file.<name>` 只回答插件在 `[context] files` 里声明过的名字；每张表都带 `size` 前缀，读取方不会越过对方写下的 `size`。

测试：`cargo test -p pmpx-plugin-abi` —— 全部是 `src/` 里的单元测试，通过 `include_str!` 比对两份提交进仓库的渲染结果，不需要 C 编译器，也不需要文件系统。
License: 与工作区一致（见仓库根目录）。