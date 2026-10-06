# CI 排障：零依赖门禁把工作区内的兄弟 crate 当成外部依赖

## 症状

`ubuntu` job 在**编译错误修好之后**暴露出下一条，其他 job（`macos` / `MSRV` / `Cross-toolchain load` / windows）都绿：

```
Error: pmpx-plugin promises zero dependencies, but the normal dependency tree contains:
pmpx-plugin-abi
Error: Process completed with exit code 1.
```

## 原因

- 门禁脚本（`.github/workflows/ci.yaml`，步骤 `the contract crates must stay dependency-free`）断言 `pmpx-plugin`、`pmpx-plugin-abi`、`pmpx-detect` 的 `cargo tree -e normal` **只包含它自己**。
- 但 `pmpx-plugin` 本来就依赖工作区内的 `pmpx-plugin-abi`（`crates/pmpx-plugin/Cargo.toml:10`，且带 `features = ["std"]`）——这是设计：安全契约层包着原始 ABI 层。
- 门禁的**真实意图**写在它自己的注释里：插件作者每多一个依赖就多一份成本（编译量、可能抬高 MSRV）。工作区内的兄弟 crate 不引入任何 registry 依赖（`pmpx-plugin-abi` 自身零依赖，而且它也在被检查的名单里），不构成这个成本。
- **为什么现在才炸**：这条 job 在更早的一步（`pmpx-plugin-abi` 的 unused import 编译错误，见 [`ci-unix-unused-import.md`](ci-unix-unused-import.md)）就失败了，门禁根本没执行到。修掉编译错误后，潜伏的矛盾才露出来。
  → 教训：**"更早一步失败"会把后面的门禁伪装成通过。**

## 修复

把判定从"零依赖"改成"不依赖工作区之外的 crate"。`cargo tree --prefix none` 对本地（path / workspace）crate 会打印括号里的路径，registry 包没有路径，于是过滤就是一行：

```bash
external=$(cargo tree -p "$crate" -e normal --prefix none 2>/dev/null \
  | grep -vF '(' | awk '{print $1}' | sort -u | grep -vx "$crate" || true)
if [ -n "$external" ]; then
  echo "::error::$crate must depend on nothing outside this workspace, but it does:"
  echo "$external"
  exit 1
fi
```

这样既保住原意（真有 registry 依赖就会出现 ✓），也不再把工作区内部的边算成成本 ✓。将来某个兄弟 crate 若拖进 `serde`，这条同样会抓到（`serde` 会以无路径的形式出现）✓。

## 为什么本地看不出来 / 怎么复现

- 这条门禁**只在 Linux 上跑**（`if: runner.os == 'Linux'`），本机是 Windows，所以只能复现它的**过滤逻辑**，不能跑 job 本身：

```powershell
foreach ($crate in 'pmpx-plugin','pmpx-plugin-abi','pmpx-detect') {
  $lines = cargo tree -p $crate -e normal --prefix none
  $external = $lines | Where-Object { $_ -notmatch '\(' } | ForEach-Object { ($_ -split '\s+')[0] } |
              Sort-Object -Unique | Where-Object { $_ -ne $crate -and $_ -ne '' }
  if ($external) { "FAIL $crate -> $($external -join ', ')" } else { "OK   $crate" }
}
```

- 用合成样例验证过滤本身有效（应只剩 registry 包）：

```console
$ printf 'serde v1.0.0\npmpx-plugin-abi v0.3.0 (/path)\nonce_cell v1.20.0\n' | grep -vF '(' | awk '{print $1}' | sort -u
once_cell
serde
```

- 本机的 `bash` 是 WSL 的（未安装发行版），所以上面那条 shell 片段只能用 PowerShell 等价重放。

## 怎么防止复发

1. 加门禁时先问一句"这条断言与设计矛盾吗"：`pmpx-plugin → pmpx-plugin-abi` 是设计，不是违规。
2. 断言"零依赖"这类绝对命题时，明确**范围**（仅 registry？还是全体？），并把范围写进错误信息里，否则下一个人在别处修好前面一步后会被它绊倒。
3. 门禁失败时，先确认它**上面每一步都真的执行过**——`set -euo pipefail` 的 job 里，前面失败会让后面的步骤看起来"从没红过"。

## 相关

- 文件：`.github/workflows/ci.yaml`（步骤 `the contract crates must stay dependency-free`）
- 依赖来源：`crates/pmpx-plugin/Cargo.toml:10`
- 被暴露的前置失败：[`ci-unix-unused-import.md`](ci-unix-unused-import.md)
- 修复提交：`ci: allow workspace crates in the zero-dep guard`
