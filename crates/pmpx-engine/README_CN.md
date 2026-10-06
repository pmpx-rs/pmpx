# pmpx-engine

执行插件给出的答案，并以值的形式报告结果

`pmpx-engine` 负责 `pmpx` 中"执行"的那一半：插件只回答*运行什么*，本 crate 决定*如何*运行——解析真实路径、选择解释器、继承 stdio、等待、转换退出状态。整条流水线是 open → select → load → run：`Session::open`、`select`、`load_backend`/`load_plugin`，再到 `flow::run_verb` 与 `run`。它是 workspace 里唯一构造 `std::process::Command` 的 crate，且从不打印：一次运行以 `Event` 报告。

## API

- `Plan` — 本 crate 自己的"运行什么"：`program`、`args`、`cwd`
- `Session` — 一次运行的状态，在 `Session::open` 中读取一次；`Options` 携带命令行给出的内容
- `Event` — 一次运行的全部报告，包括 `Phase { name, micros, detail }`
- `ChildOutput` — 被启动程序的输出去哪：`Inherit`，或为 `--json` 准备的 `OnStderr`
- `run` — 解析、启动、等待，把退出码透传出去；不需要任何插件
- `resolve` / `ProgramKind` — 真实路径，以及它是 `Native`、`CmdShim` 还是 `PowerShellShim`
- `store` feature（默认关闭）——`install`、`uninstall`、`update`、`search`、`view` 与已安装插件集合；打开它才会引入 `crate-plugin-kit`

## 说明

- 没有 `store` 时，`detect`、`session`、`flow`、`store` 四个模块不被编译；剩下的是执行内核，且不链接 `crate-plugin-kit`。
- 后端的退出码原样透传——返回 `Ok(code)`，绝不是 `Err`；超出 8 位的取低 8 位，并给出一个 `Warning`。
- 它自己测量各阶段并以 `Event::Phase` 报告，使跟踪能归位后端运行时间之外的那部分时间。
- 决策在 `pmpx-detect`，ABI 在 `pmpx-loader`；本 crate 不被它们依赖。

测试：单元测试就在代码旁边，store 部分另跑 `cargo test -p pmpx-engine --features store`。
License: 与工作区一致（见仓库根目录）。