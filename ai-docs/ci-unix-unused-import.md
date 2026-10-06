# CI 排障：`no_std` crate 里被 `cfg` 分平台的 `use`（unused import）

## 症状

四条 CI 同时红，**windows 通过**：

- `Cross-toolchain load`、`MSRV`、`ubuntu`、`macos`

原始报错（三个平台完全一致）：

```
error: unused import: `std::string::String`
  --> crates/pmpx-plugin-abi/src/os.rs:12:5
   |
12 | use std::string::String;
   |     ^^^^^^^^^^^^^^^^^^^
   |
   = note: `-D unused-imports` implied by `-D warnings`
   = help: to override `-D warnings` add `#[allow(unused_imports)]`

error: could not compile `pmpx-plugin-abi` (lib) due to 1 previous error
Error: Process completed with exit code 101.
```

## 原因

`pmpx-plugin-abi` 是 `no_std`（`#![no_std]` + 可选 `std` 特性），所以 `String` **不在 prelude 里**，必须显式 `use std::string::String;`。问题在于它只被**一个平台分支**用到：

- `crates/pmpx-plugin-abi/src/os.rs` 第 18–22 行 `#[cfg(unix)]` 的 `bytes_to_os` 用 `OsString::from_vec`，**不碰 `String`**；
- 第 25–28 行 `#[cfg(not(unix))]` 的 `bytes_to_os` 用 `String::from_utf8_lossy(...)`，**需要 `String`**；
- 而 `use std::string::String;` 写在第 12 行，**没有门控**。

于是：

| 平台 | 编译的分支 | 该 import | 结果 |
| --- | --- | --- | --- |
| Windows | `not(unix)` | 被用到 | 通过 |
| Linux / macOS | `unix` | 未使用 | `-D warnings` → 编译失败 |

**触发面**：`pmpx-loader` 与 `pmpx-plugin` 都以 `features = ["std"]` 依赖 abi，所以任何编译它们的 job 都会编译 `os.rs`：

- `crates/pmpx-loader/Cargo.toml:16` — `pmpx-plugin-abi = { workspace = true, features = ["std"] }`
- `crates/pmpx-plugin/Cargo.toml:23` — 同上

（`use std::vec::Vec;` 不需要动：`os_to_bytes` 两个分支的返回值都是 `Vec<u8>`，两支都用到。）

## 修复

让 import 与它的使用**带同一个门控**，并把踩坑原因写在旁边（`crates/pmpx-plugin-abi/src/os.rs`）：

```rust
use std::ffi::{OsStr, OsString};
use std::vec::Vec;

// `String` is in the prelude of a `std` crate, not of this `no_std` one, and only the `not(unix)` path
// below needs it -- so the import carries the same gate as its use. Without the gate it is an unused
// import on Unix, which this workspace's `-D warnings` turns into a build failure that Windows, where
// the other arm compiles, can never show.
#[cfg(not(unix))]
use std::string::String;
```

## 为什么本地（Windows）看不出来，以及怎么在本地复现

- 本机是 Windows，走的正是 `not(unix)` 那一支，import 恰好被用到 → 本地 `cargo check` / `clippy` / `test` 全绿。
- 要看 Unix 侧，**必须显式指定目标**（本机已装 `x86_64-unknown-linux-gnu`；没装则 `rustup target add x86_64-unknown-linux-gnu`）：

```console
$ cargo check -p pmpx-plugin-abi --features std --target x86_64-unknown-linux-gnu --all-targets
$ cargo check -p pmpx-plugin     --target x86_64-unknown-linux-gnu --all-targets
$ cargo check -p pmpx-loader    --target x86_64-unknown-linux-gnu
```

- `cargo check` 不需要链接器，所以这几条在 Windows 上能跑通。
- **整工作区**不行：`pmpx-engine` 的 `store` 特性会拉进 `ring`，交叉编译它需要 Linux 的 C 编译器（本机没有）：

```
error: failed to run custom build command for `ring v0.17.14`
cargo:warning=Compiler family detection failed ... failed to find tool "x86_64-linux-gnu-gcc"
```

  所以本地只覆盖 abi / 契约 / loader 这几个不依赖 ring 的 crate；**其余平台差异交给 CI**。

## 怎么防止复发

1. **经验规则**：`no_std` crate 里，为某个 `cfg` 分支加的 `use` 必须带**同样的 `cfg`**。否则必然出现"一个平台绿、另一个平台红"。
2. 改动 `pmpx-plugin-abi` / `pmpx-plugin` 的 `cfg` 分支之后，本地至少跑一次上面那两条 linux-target `cargo check`（几十秒）。
3. 终极确认交给 CI：`ubuntu` / `macos` / `MSRV` / `Cross-toolchain load` 四条覆盖三个平台与两个工具链。

## 相关

- 文件：`crates/pmpx-plugin-abi/src/os.rs`（第 11–13 行的 import 区、第 18–40 行的平台分支）
- 打开 `std` 特性者：`crates/pmpx-loader/Cargo.toml:16`、`crates/pmpx-plugin/Cargo.toml:23`
- 本地验证命令：见上文（`--target x86_64-unknown-linux-gnu`）
- 修复提交：`fix: gate the abi's String import by platform`
