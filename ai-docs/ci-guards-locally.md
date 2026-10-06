# 推送前：把 CI 的守卫在本地跑一遍

## 为什么需要这份清单

`test` job 里有 **7 条只在 Linux 上跑的守卫**（`if: runner.os == 'Linux'`），加上 `MSRV` 与 `Cross-toolchain load` 两个 job。它们在**同一个 job 里顺序执行**：第一条失败会把后面全部遮住，于是表现成"每推一次冒一个错"。这些守卫的逻辑全都能在本机跑，只要用对工具：

- 本机 `bash` 是**未安装发行版的 WSL**，用不了 → 用 Git 自带的：`D:\Git\bin\bash.exe`、`D:\Git\usr\bin\bash.exe`。
- CI 用 `jq`，本机没有 → 用 Python / PowerShell 等价重放。
- 有 `python`（含 `tomllib`）与 `cargo metadata --format-version 1` 就够。

## 七条守卫（`test` job）

| # | 守卫 | 它检查什么 | 本地怎么跑 |
| --- | --- | --- | --- |
| 1 | `the contract crates must stay dependency-free` | `pmpx-plugin` / `pmpx-plugin-abi` / `pmpx-detect` 的 normal 依赖树里**没有工作区之外的包**（`cargo tree --prefix none` 下 registry 行没有括号路径，本地 crate 有） | `cargo tree -p <crate> -e normal --prefix none` → 丢掉含 `(` 的行，比较包名 |
| 2 | `unsafe stays in the ABI islands` | 除允许清单外，`crates/**` 里没有以 `unsafe` 开头的行（排除 `/fixtures/`、`/tests/`、`tests.rs`） | 见 `ci.yaml` 的 `allowed=` 正则，用 `Select-String '^[^/]*\bunsafe\b'` 等价重放 |
| 3 | `No async runtime anywhere` | 工作区依赖树里没有 `tokio` / `async-std` / `smol` / `async-io` / `async-executor` / `async-global-executor` | `cargo tree --workspace --prefix none` 后比对包名 |
| 4 | `Package` | `pmpx-plugin-abi` 完整打包；`pmpx-plugin` / `pmpx-loader` / `pmpx` 只 `--list`（依赖未发布时完整打包必然失败；**`--no-verify` 无效**，失败早于验证） | `cargo package -p pmpx-plugin-abi --locked` + 三个 `cargo package -p <crate> --list` |
| 5 | `The generated ABI files must be current` | `abi/surface.toml` 与 `include/pmpx_plugin.h` 与源码一致 | `cargo run -p pmpx-plugin-abi --example gen-abi-files -- --check` ✓（另一半是用 `cc` 编译 C 头，本机没有 C 编译器，只能靠 CI） |
| 6 | `Workspace deps declare a version` | normal/build 依赖不得只有 `path` 没有 `version`（**dev 已豁免**：cargo 打包时会剥离 path-only 的 dev 依赖） | `cargo metadata --no-deps --format-version 1` → 过滤 `path 且 req == "*"` 且 `kind != "dev"` |
| 7 | `Read rust-version from the contract crate` | 从 `crates/pmpx-plugin/Cargo.toml` 读出 MSRV（当前 1.82），供 `MSRV` job 使用 | `python -c "import tomllib;print(tomllib.load(open('crates/pmpx-plugin/Cargo.toml','rb'))['package']['rust-version'])"` |

## 两个 job

- **`MSRV`**（实测本机可跑，已装 1.82）：

  ```console
  $ cargo +1.82 build --release \
      --manifest-path crates/pmpx-plugin/tests/fixtures/toy-plugin/Cargo.toml \
      --target-dir "$TEMP/msrv-check"
  ```

  ⚠️ **别自作主张加 `--all-targets`**：该 job 的注释写明**故意不跑**——dev 依赖会拉进需要更新 cargo 的 crate（实测 `getrandom 0.4.3` 在 1.82 下直接解析失败）。

- **`Cross-toolchain load`**：只剩"用 MSRV 构建插件 fixture"这一步（与上面同一条命令）；"stable 宿主加载 MSRV 产物"那半步已随死步骤删除，现在由 `cargo test -p pmpx-loader --test dlopen` 覆盖。

## 本地跑不了的部分（只能交给 CI）

- 用 `cc -std=c11 -fsyntax-only` 编译 C 头（本机没有 C 编译器）。
- macOS 特有行为（unix 分支可用 `--target x86_64-unknown-linux-gnu` 覆盖）。
- **整工作区**的 Linux check：`store` 特性会拉进 `ring`，交叉编译它需要 Linux 的 C 编译器 → 只对不依赖 ring 的 crate 做（`pmpx-plugin-abi` / `pmpx-plugin` / `pmpx-loader`）。

## 一句话流程

改了 `cfg` 分支、依赖声明或 crate 结构之后，**先按本文把七条守卫与两个 job 在本地跑一遍，再 push**。
