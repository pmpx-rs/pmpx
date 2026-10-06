# CI 排障：`cargo package` 遇到尚未发布的 workspace 依赖

## 症状

`ubuntu` job 的 `Package` 步骤：

```
   Packaging pmpx-plugin-abi v0.3.0 (/home/runner/work/pmpx/pmpx/crates/pmpx-plugin-abi)
    Packaged 15 files, 61.3KiB (18.2KiB compressed)
   Verifying pmpx-plugin-abi v0.3.0 (/home/runner/work/_temp/package-target/package/pmpx-plugin-abi-0.3.0)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s
   Packaging pmpx-plugin v0.3.0 (/home/runner/work/pmpx/pmpx/crates/pmpx-plugin)
    Updating crates.io index
error: failed to prepare local package for uploading

Caused by:
  no matching package named `pmpx-plugin-abi` found
  location searched: crates.io index
  required by package `pmpx-plugin v0.3.0 (/home/runner/work/pmpx/pmpx/crates/pmpx-plugin)`
Error: Process completed with exit code 101.
```

`pmpx-plugin-abi` 打包成功，紧接着的 `pmpx-plugin` 失败。

## 原因

- `cargo package` 会把包清单里的 **path 依赖改写成 registry 依赖**，因此在"为上传准备包"的阶段就要求那个依赖**已经存在于索引里**。
- `pmpx-plugin` 依赖工作区内的 `pmpx-plugin-abi`（`crates/pmpx-plugin/Cargo.toml:10`），而它要到**发布时**才会出现在 crates.io 上。
- 于是本地/CI 在发布前只能对**依赖已发布的 crate** 做完整打包；这一步的注释此前写着"`pmpx-plugin` 与 `pmpx-plugin-abi` 都没有 registry 依赖，所以两个都能完整打包 + 编译"，这个前提**不成立**。

## 修复

`.github/workflows/ci.yaml` 的 `Package` 步骤，把 `pmpx-plugin` 归入"只查文件装配"那一组（与该步里 `pmpx-loader`、`pmpx` 的做法一致），并把注释改成事实：

```bash
cargo package -p pmpx-plugin-abi --locked
cargo package -p pmpx-plugin --list
cargo package -p pmpx-loader --list
cargo package -p pmpx --list
```

## 关键事实：`--no-verify` **不能**绕过

失败发生在**验证之前**（"prepare local package for uploading"），所以 `--no-verify` 无效。本地实测：

```console
$ cargo package -p pmpx-plugin                 # 复现 CI 的报错
error: failed to prepare local package for uploading
Caused by:
  no matching package named `pmpx-plugin-abi` found

$ cargo package -p pmpx-plugin --no-verify     # 同样失败，exit 101
error: failed to prepare local package for uploading
```

而 `--list` 不解析 registry，所以能过：

```console
$ cargo package -p pmpx-plugin --list | head -4
Cargo.toml
Cargo.toml.orig
README.md
README_CN.md
```

（顺带：`--list` 也会把 `readme` 指向的文件列出来，所以"readme 字段写错"这类问题它同样能暴露。）

## 怎么防止复发

1. **完整打包只对"依赖都已发布"的 crate 做**；依赖尚未发布的，用 `--list` 查文件装配，把编译/验证留给发布流程。
2. 别把 `--no-verify` 当成绕过手段——它跳过的不是失败点。
3. 发布顺序由 release 工作流按依赖图计算（见该步骤注释："Forcing it to pass here would block the release"）；CI 不应该也无法在发布前让依赖未发布的 crate 完整打包通过。
4. 改动 `[dependencies]` 的 path/version 关系后，本地把这一步的四条命令跑一遍（几十秒）。

## 相关

- 文件：`.github/workflows/ci.yaml`（步骤 `Package`）
- 依赖来源：`crates/pmpx-plugin/Cargo.toml:10`
- 修复提交：`ci: package pmpx-plugin for assembly only`
