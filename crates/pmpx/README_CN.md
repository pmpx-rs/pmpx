# pmpx

二进制本体：argv 进，插件给出的命令被执行，输出被渲染

`pmpx` 是本项目的 CLI 二进制：它解析 `argv`，打开一个 `pmpx_engine::Session`，通过它跑一个 verb，并把引擎的 `Event` 渲染到终端。检测、加载插件、启动进程都不由它负责——那是引擎的事。安装与用法只在仓库根目录的 [`README.md`](../../README.md) 里讲一次。

## API

- `cli` — `clap` 表面：`Cli` 与子命令表；把 argv 变成结构体，不含语义
- `app` — 本宿主对一次运行的视图：`options`、`session`、`select`、`load_backend`、`run_verb`、`run_script`、`explain`、`Sink`
- `commands` — 子命令处理：`dispatch`，以及 `info`、`plugin`、`plugin_pin`、`plugin_store`、`config`
- `runtime` — 呈现引擎报告的内容：`set_json`、`render_event`、`render`、`json`
- `style` — 终端样式；转义码只写在这里
- `selfupdate` — `pmpx self update`：`release`、`checksum`、`archive`、`swap`、`ledger`、`version`
- `debug` — `--debug`：每个阶段一行暗色输出，走 stderr
- `error` — `PmpxError` 与退出码映射：`EXIT_OK`、`EXIT_USAGE`、`EXIT_NOT_FOUND`、`EXIT_INTERNAL`

## 说明

- 全局 flag（写在子命令之后也生效）：`--json`——stdout 是 JSON、每行一个对象，后端的输出落在 stderr；`--explain`；`-p, --plugin <NAME>`；`-C, --dir <PATH>`；`--no-walk-up`；`-q, --quiet`；`--debug`。
- verb 有 `install`（`i`、`add`）、`remove`（`rm`、`uninstall`）、`run`（`r`）、`build`（`b`）、`test`（`t`）、`update`（`up`）、`exec`（`x`）；另有 `info`、`plugin`、`config`、`completion <shell>` 与 `self update`。
- `pmpx run <name>` 会先查项目的 `[scripts]` 表，再问插件，因此项目不需要任何插件也能有 `pmpx fmt`。
- 终端输出归本 crate 所有：引擎什么都不打印，它的 `Event` 在这里变成一行行输出。

测试：`crates/pmpx/tests/cli/` 里的端到端套件运行构建好的二进制（`CARGO_BIN_EXE_pmpx`），跑在临时沙箱上。
License: 与工作区一致（见仓库根目录）。