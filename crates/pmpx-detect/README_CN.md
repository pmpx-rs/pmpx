# pmpx-detect

仅凭数据判定项目属于哪个已安装插件

`pmpx-detect` 是 pmpx 的决策层。它接收已安装插件（`Candidate` 值）、项目的文件（只有一个方法的
`Presence` trait），以及 pin 与排序表（普通值），返回 `Selection` 或带类型的 `DetectFailure`。
它没有任何依赖：不读文件、不打印、不认识任何生态。

## API

- `Presence::has(&self, relative: &str) -> bool` —— crate 里唯一与文件系统形状有关的东西；为
  `BTreeSet<String>` 与 `Fn(&str) -> bool` 实现。
- `Candidate { crate_name, name, family, strong, weak, problem }` 与 `Candidate::is_usable()`。
- `Pins = BTreeMap<String, String>`（family → 插件名）与
  `Preferences { family_priority, priority }`。
- `ScoredPlugin::score(plugin, presence)` —— 字段 `strong_hits`、`weak_hits`、`score`，另有 `all_hits()`。
- `score_all(candidates, presence, pins) -> BTreeMap<String, FamilyScore>`。
- `select(candidates, presence, pins, preferences, explicit: Option<&str>) -> Result<Selection, DetectFailure>`，
  以及调用方已经算过分时用的 `select_from_scores(...)`。
- `Selection { crate_name, name, family, score, reason, matched, notes }`、
  `Reason::{Scored, Pinned, Explicit}`、`DetectFailure`（五个变体，外加 `message()`）。
- 常量 `STRONG_SCORE = 100`、`WEAK_SCORE = 10`、`PIN_FLOOR = 50`。

## 说明

- 决策吃的是数据，不是文件系统：全部证据由调用方以 `Candidate::strong` / `Candidate::weak` 的标记
  文件名给出，`Presence` 只回答「这个文件在不在」。
- 打分为强证据每命中 100 分、弱证据每命中 10 分；family 取旗下插件的最高分，绝不求和。
- pin 把 family 兜底到 `PIN_FLOOR`（50），而不是覆盖它的分数，因此压得过弱证据，却压不过真正的
  lockfile。
- `-p <name>` 同时压过 pin 与证据；`Selection::matched` 就是选中赢家的证据；平局会写进 `notes`，
  但仍然返回一个赢家。

测试：`src/lib.rs` 与 `src/failure.rs` 的单元测试，外加 `tests/no_filesystem.rs`，全部在内存里跑。
License: 与工作区一致（见仓库根目录）。