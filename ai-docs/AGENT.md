# ai-docs

给 AI 与协作者用的**排障与经验**文档。

这里**不是**产品文档：

| 想知道什么 | 去哪 |
| --- | --- |
| 怎么安装、有哪些命令与参数 | 仓库根 [`README.md`](../README.md) / [`README_CN.md`](../README_CN.md) |
| 架构怎么演进、为什么这么拆 | [`docs/refactor.md`](../docs/refactor.md) |
| 某个 crate 是干什么的 | 各 crate 自己的 `README.md` / `README_CN.md` |
| **踩过的坑、CI 红灯怎么修的、怎么在本地复现** | **本目录** |

## 目录

| 文档 | 一句话 |
| --- | --- |
| [`ci-unix-unused-import.md`](ci-unix-unused-import.md) | `no_std` crate 里为 `#[cfg(not(unix))]` 分支加的 `use` 没带门控 → Linux/macOS 的 `-D warnings` 失败，而 Windows 通过；含本地用 `--target x86_64-unknown-linux-gnu` 复现的办法 |
| [`ci-zero-dep-guard.md`](ci-zero-dep-guard.md) | 零依赖门禁把工作区内的兄弟 crate 当成外部依赖（且它此前一直被更早的编译失败挡着没执行）→ 判定改为"不依赖工作区之外的 crate" |
| [`ci-package-unpublished-dep.md`](ci-package-unpublished-dep.md) | `cargo package` 要求依赖已在 crates.io 索引里，所以发布前只有叶子 crate 能完整打包；`--no-verify` 无效（失败早于验证），改用 `--list` 查文件装配 |
| [`ci-path-only-dev-dep.md`](ci-path-only-dev-dep.md) | "path 必须有 version"的检查没区分依赖种类，把 dev-dependency 也算上了；给它补版本的代价是发布顺序多一条边，正确修法是限定到 normal/build |
| [`ci-guards-locally.md`](ci-guards-locally.md) | **推送前必读**：`test` job 的七条 Linux 守卫 + `MSRV` / `Cross-toolchain load` 两个 job 的本地跑法（Git Bash、Python 代替 jq），以及本地跑不了的三件事 |

## 写作约定

- 文件名 `kebab-case.md`，按**现象或主题**命名（例如 `ci-unix-unused-import.md`），不要写日期。
- 每篇按这个顺序写，缺一不可：
  1. **症状** —— 附带原始报错（不翻译、不改写）。
  2. **原因** —— 指到具体文件与行号；平台差异要说清"哪个平台走哪条分支"。
  3. **修复** —— 贴出真正的改法（可与提交内容一致）。
  4. **为什么本地看不出来 / 怎么在本地复现** —— 给出可执行的命令；说明本地能覆盖到哪一步、哪一步只能靠 CI。
  5. **怎么防止复发** —— 提炼成一条可记住的规则。
  6. **相关** —— 文件、依赖来源、相关提交。
- 中文书写；命令、路径、报错原文保持原样。
- 只写**可核实**的结论；推测必须标明"推测"。

## 给 agent 的提示

1. 改代码前先扫一眼本目录：CI 红、平台差异、`cfg` 与 `no_std`、ABI 相关问题，这里可能已经有结论。
2. **推送前**按 [`ci-guards-locally.md`](ci-guards-locally.md) 把 CI 的守卫在本地跑一遍（七条 Linux 守卫 + `MSRV` / `Cross-toolchain load` 两个 job）。动过 `cfg` 分支、依赖声明或 crate 结构时**必须**跑——那些守卫编码着关于工作区形态的假设，形态一变就可能过期，而它们顺序执行，第一条失败会遮住后面全部。
2. 修完一类"容易复发且本地看不出来"的问题后，按上面约定补一篇放在这里，并在目录表里加一行。
3. 本目录只增不改历史结论：若旧结论过时，新写一篇并在旧篇顶部加一行指向新篇。
