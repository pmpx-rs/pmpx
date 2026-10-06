# pmpx

[![Checks](https://github.com/pmpx-rs/pmpx/actions/workflows/ci.yaml/badge.svg)](https://github.com/pmpx-rs/pmpx/actions/workflows/ci.yaml)
[![Release](https://github.com/pmpx-rs/pmpx/actions/workflows/release.yaml/badge.svg)](https://github.com/pmpx-rs/pmpx/actions/workflows/release.yaml)
[![crates.io](https://img.shields.io/crates/v/pmpx.svg)](https://crates.io/crates/pmpx)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](#license)
[![MSRV](https://img.shields.io/badge/rust-1.88%2B-blue.svg)](#环境要求)

> English see [README.md](README.md)

一个命令面，管项目的各种包管理器。`pmpx` 识别项目类型、把固定的一组动词翻译成真正的命令、
然后交给它执行 —— 而**二进制里没有一行后端代码**。

```console
$ pmpx install serde            # Rust 项目      → cargo add serde
$ pmpx install lodash           # 前端仓库       → pnpm add lodash
$ pmpx test -- --nocapture      #                → cargo test --nocapture
$ pmpx exec tsc --noEmit        #                → pnpm exec tsc --noEmit

$ pmpx                          # 这个目录是什么？会跑哪个工具？
```

## 特性

- 🎯 **代理包管理** —— 同一套命令在哪个项目都能用，自动翻译成那个项目真正的工具。
- 🔍 **自动识别项目类型** —— 只读仓库里本来就有的文件，不用初始化，也不用写配置文件。
- 🔌 **一套插件系统** —— 后端全部是插件，运行时 `dlopen`。二进制里一个都不带，
  而且插件不必和宿主用同一个 rustc。
- 🔀 **可裁决、可覆盖** —— 同分时靠一张你说了算的次序表；`.pmpx.toml` 能把后端按项目钉死。
- 🪂 **逃生舱** —— 后端给不出答案时，`exec` 退化成 pmpx 自己跑那条命令。

## 目录

- [安装](#安装)
- [更新](#更新)
- [用法](#用法)
- [它怎么决定用哪个后端](#它怎么决定用哪个后端)
- [配置](#配置)
- [它不做什么](#它不做什么)
- [写一个插件](#写一个插件)
- [仓库](#仓库)
- [许可](#许可)

## 安装

```bash
cargo install pmpx
```

装出来的二进制叫 `pmpx`。

每次发版还会附上预编译归档：Linux / Windows 的 x86_64，以及两种 macOS 架构。两种用法：

```console
# 安装脚本：下载本机对应的归档、按 release 里的 SHA256SUMS 校验、然后安装。
# 只写一个文件，不碰 PATH，也不碰 shell 配置。
$ curl -LsSf https://raw.githubusercontent.com/pmpx-rs/pmpx/main/install.sh | sh

# 或者用 cargo-binstall
$ cargo binstall pmpx
```

Windows 上用的是 `install.ps1`：

```console
$ irm https://raw.githubusercontent.com/pmpx-rs/pmpx/main/install.ps1 | iex
```

两者的默认落点分别是 `~/.local/bin`（Unix）与 `%USERPROFILE%\.pmpx\bin`（Windows），
都读环境变量 `PMPX_INSTALL_DIR`、`PMPX_VERSION`、`PMPX_BASE_URL`（镜像）。没有归档的平台
不算失败 —— 任何 Rust 能编译的地方，`cargo install pmpx` 都能装上。

### 环境要求

- Rust **1.88+** 用于构建。
- **至少一个插件。** 没有插件时 `pmpx` 什么也检测不出来 —— 这是刻意的，见
  [写一个插件](#写一个插件)。

## 更新

```console
$ pmpx self update --check         # 看看有没有新版，什么都不改
$ pmpx self update                 # 把当前二进制换成最新 release
$ pmpx self update --version 0.1.0 # 或指定版本，这也是回滚的方式
```

只有**不是 cargo 装的**安装才能自更新。`cargo install` 自己记着它放了什么到磁盘上，
绕过它去替换文件会让 `cargo install --list` 与实际不符 —— 所以这类安装会被要求执行
`cargo install pmpx --force`。判断依据是**那本账**，不是目录：`cargo binstall` 装的、
或者手工解压放进同一个目录的，都能自更新。

替换之前会先拿 release 里的 `SHA256SUMS` 校验下载内容；不一致就停在那里，正在运行的
二进制原样不动。但要清楚这**不是签名**：校验文件和压缩包来自同一个地方，它只能证明字节
完整送达，不能证明是谁构建的。

**不会自动检查**：没有后台轮询，也没有启动时的"有新版本"提示。

## 用法

| 动词 | 别名 | 无参 | 带参 |
| ---- | ---- | ---- | ---- |
| `install` | `i` `add` | 按锁文件装齐 | 添加依赖 |
| `remove` | `rm` `uninstall` | — | 卸载依赖 |
| `run` | `r` | — | 跑脚本 / 目标 |
| `build` | `b` | 构建 | 传给后端 |
| `test` | `t` | 测试 | 传给后端 |
| `update` | `up` | 更新依赖 | 更新指定依赖 |
| `exec` | `x` | — | 逃生舱，见下 |

`--` 之后的内容原样传给后端，不做任何解释：

```console
$ pmpx test -- --nocapture
$ pmpx run dev -- --port 3000
```

全局参数：

| 参数 | 含义 |
| ---- | ---- |
| `-p, --plugin <name>` | 指定插件，**压过 `.pmpx.toml`** |
| `-C, --dir <path>` | 在指定目录操作 |
| `--no-walk-up` | 只看当前目录，不向上找项目根 |
| `-q, --quiet` | 关掉 stderr 上的提示 |
| `--debug` | 打印调试信息 |

其它命令：`pmpx info`、`pmpx plugin ls|current|set|unset|add|rm|update|search|info`、
`pmpx config get|set`、`pmpx completion <shell>`、`pmpx self update`。

### 退出码

| 码 | 含义 |
| -- | ---- |
| 0 | 成功 |
| 1 | `pmpx` 自身出错，**或者插件 panic 了** |
| 2 | 用法错误，或这个后端不支持该动词 |
| 3 | 检测不到项目类型、缺少对应插件、或工具不在 `PATH` 上 |
| **N** | **后端自己的退出码，原样透传** |

最后那一行是关键：`pmpx test` 必须在测试失败时失败。

### `exec`

```
pmpx exec <cmd...>
   ├─ 插件支持 exec       → 用它的答案（npx、pnpm exec……）
   └─ 不支持，或没有项目   → pmpx 自己跑这条命令
```

所以零插件下 `pmpx exec ls` 也能用。其余动词遇到不支持就报错 ——
这是显式写入的例外，不是静默魔法。

## 它怎么决定用哪个后端

检测**不加载任何插件代码**。它拿各插件 manifest 里声明的特征文件，给已装的插件打分：

| 证据 | 分值 | 含义 |
| ---- | ---- | ---- |
| `strong` | 100 | 锁文件 / workspace 文件 / 形态标记 —— **这个后端确实被用过** |
| `weak` | 10 | 清单文件 —— **只能证明属于这个生态** |
| `.pmpx.toml` 里 pin 了 | 50（地板） | 「我写了 `node = "pnpm"`，就是在声明这是个 Node 项目」 |

然后先选生态，再在生态里选插件。同分时由两张你说了算的表裁决：

```toml
# <pmpx-config-dir>/config.toml
[plugin]
family_priority = ["node", "rust", "python", "go", "jvm", "dotnet", "php", "ruby"]
priority        = ["pnpm", "npm", "yarn", "bun", "cargo"]
```

**「混合项目默认走 Node」不是硬编码规则** —— 它只是默认 `family_priority` 的自然结果。
把 `rust` 提到首位，同一个项目就走 `cargo`。

这也正是已知的误判：库 crate 常常 gitignore 掉 `Cargo.lock`，于是 `Cargo.toml` 只拿 10 分，
和 `package.json` 打平，然后输给 Node。这是两档打分的固有代价 ——
**没有锁文件时，"是什么生态"确实判不出来**。出口是 pin 一条。

## 配置

| 什么 | 在哪 |
| ---- | ---- |
| 全局 | `<config-dir>/pmpx/config.toml` —— Linux `~/.config/pmpx`、macOS `~/Library/Application Support/pmpx`、Windows `%APPDATA%\pmpx\config` |
| 插件 | 三个平台都是 `~/.pmpx/plugins/` |
| 项目级 | `.pmpx.toml`，**可以有多份**，从 cwd 向上逐层收集 |

近者优先，所以 monorepo 可以在根上定默认、在子包里覆盖。`pmpx plugin set <name>` 写最近的一层。

```toml
# .pmpx.toml
[plugin]
node = "pnpm"      # 把 Node 固化成 pnpm，同时给 Node 一个 50 分地板
rust = "cargo"
```

两个环境变量可以覆盖这些路径，给测试与多环境用：`PMPX_CONFIG_DIR`、`PMPX_DATA_DIR`。

## 它不做什么

**`pmpx` 只写自己的文件。** `~/.pmpx/**`、全局配置，以及你明确要求时的项目 `.pmpx.toml`。
**项目里没有缓存文件** —— 命中缓存省不到 1ms，代价是一个脏工作区。

刻意永不触碰：

```text
~/.cargo/config.toml   ~/.npmrc   ~/.yarnrc   ~/.yarnrc.yml   ~/.bunfig.toml
~/.config/pip/*        ~/.gemrc   src/**     *.lock
```

**代理 / 镜像 / 换源功能整体没有** —— 连读都不读。那些在你的 shell 或各工具自己的配置里配；
`pmpx` spawn 子进程时原样继承环境。

## 写一个插件

插件实现一个 trait，再加一行：

```rust
use std::ffi::OsString;
use pmpx_plugin::{CommandSpec, Context, Family, PackageManager, PluginError, Verb};

struct CargoPlugin;

impl PackageManager for CargoPlugin {
    fn name(&self) -> &str { "cargo" }
    fn family(&self) -> Family { Family::RUST }

    fn command(&self, ctx: &Context, verb: Verb, args: &[OsString])
        -> Result<CommandSpec, PluginError>
    {
        match verb {
            Verb::Install if args.is_empty() => Ok(CommandSpec::new("cargo").arg("fetch")),
            Verb::Install => Ok(CommandSpec::new("cargo").arg("add").args(args.iter())),
            // 明说不支持，而不是猜一个 —— 宿主会替你降级 `exec`
            Verb::Exec => Err(PluginError::unsupported_verb(verb)),
            other => Err(PluginError::other(format!("还没实现：{other}"))),
        }
    }
}

pub fn create() -> Box<dyn PackageManager> { Box::new(CargoPlugin) }

pmpx_plugin::export!(create);
```

`export!` 生成整个 C ABI 外壳 —— `catch_unwind`、字符串生命周期、vtable。插件作者看不到这些。

**`command()` 是纯映射。** 输入只有动词、参数、以及宿主知道的本次调用信息。它不许读文件、
写文件、读环境变量、起进程 —— 所以形态判断走 `Context::matched`：

```rust
// yarn classic 与 Berry 只有一个动词拼法不同，而唯一证据是一个文件在不在
Verb::Update if ctx.has_matched(".yarnrc.yml") => Ok(CommandSpec::new("yarn").arg("up")),
Verb::Update => Ok(CommandSpec::new("yarn").arg("upgrade")),
```

光有文件名不够时，插件在**自己的 manifest 里声明**想读什么，宿主只读这些、把字节原样交过去，
依旧不解释内容：

```toml
[context]
files = ["package.json", ".yarnrc.yml"]
```

```rust
// 解析是插件的事，pmpx 永远不知道里面是什么
if ctx.file_str("package.json").is_some_and(|s| s.contains("\"packageManager\"")) { … }
```

上下文里还有只有宿主知道的部分：`start_dir`（用户在哪里敲的命令 —— monorepo 里这是判断"我在哪个包"
的唯一线索，**不是**命令将在哪里执行）、`reason` 与 `score`（为什么选中这个插件）、以及
`pins`、`scripts`、`config_files`（读到的项目配置）。

**插件通过宿主说话。** `pmpx_plugin::debug!` / `info!` / `warn!` / `error!`，以及把整个上下文
打成一行 `debug::context()` 都交给 pmpx 处理：宿主加上插件 ID，并决定打不打、打到哪一级：

```
pmpx debug: [pnpm] context: root=… start=… matched=[package.json pnpm-lock.yaml] verb=install …
```

开关在宿主手里，所以 `--debug` **不会**成为插件能拿到的输入：开不开 trace，执行的命令逐字节相同。
没有宿主时（插件自己的 `cargo test`）宏回退到 stderr，作者照样看得到。

契约 crate 是 [`pmpx-plugin`](https://crates.io/crates/pmpx-plugin) ——
**零依赖**，MSRV 1.82。

## 仓库

<https://github.com/pmpx-rs/pmpx>

| 路径 | 是什么 |
| ---- | ------ |
| `crates/pmpx/` | 宿主二进制 |
| `crates/pmpx-plugin/` | 插件契约：trait、C ABI、`export!` |
| `crates/pmpx-plugin/tests/` | 单测、ABI 外壳测试，以及一次真的 `dlopen` 端到端 |
| `crates/pmpx/tests/` | 端到端：真的跑构建出来的二进制，加载真的 `cdylib` 插件 |

每个后端住在自己的仓库里（`pmpx-plugin-cargo`、`pmpx-plugin-pnpm`……），独立发布。

```bash
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo package -p pmpx-plugin --locked
```

## 许可

MIT —— 见 [LICENSE](LICENSE)。
