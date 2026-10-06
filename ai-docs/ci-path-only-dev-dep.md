# CI 排障：`path` 必须有 `version` 的检查把 dev-dependency 也算进去了

## 症状

`ubuntu` job 的 `Workspace deps declare a version` 步骤：

```
Error: these workspace-internal dependencies declare only path, no version:
pmpx-loader
Error: Process completed with exit code 1.
```

而 `pmpx-loader` 在这里是 `pmpx-plugin` 的 **dev-dependency**（`crates/pmpx-plugin/Cargo.toml:33`），用于它的集成测试（`tests/dlopen.rs` 真的 `dlopen` 一个 cdylib）。

## 原因

检查用 `cargo metadata --no-deps` 抓"有 `path`、`req` 为通配"的依赖：

```bash
missing=$(cargo metadata --no-deps --format-version 1 \
  | jq -r '.packages[].dependencies[] | select(.path != null and .req == "*") | .name')
```

`cargo metadata` 的 `.dependencies[]` **把 normal / build / dev 三类放在同一个数组里**（用 `kind` 区分），而这个 `select` 没有过滤 `kind` → dev 依赖也被算作"必须声明版本"。

但注释里写的理由是**发布**："只有 path 本地能编译，发布时那条依赖会去 registry 解析，包验证必然失败"。这条对 **dev 依赖并不成立**：cargo 打包时会**剥离**只有 path 的 dev-dependency，所以它不可能破坏发布。

## 曾试过、被否掉的修法：给 dev-dependency 补版本

给 `pmpx-loader` 补上版本确实能让这条检查通过，但**代价是发布顺序**：`cargo publish -p pmpx-plugin` 的验证会为包生成 lockfile，其中包含 dev-dependency，于是索引里必须先有 `pmpx-loader 0.3.0` → **发布顺序从 `abi → plugin` 变成 `abi → loader → plugin`**。

为了满足一条**不适用于 dev** 的规则去给发布流程加一条边，不划算。

## 修复

把判定限定在真正会因发布而失败的种类，并在注释里写明豁免理由（`.github/workflows/ci.yaml`）：

```bash
missing=$(cargo metadata --no-deps --format-version 1 \
  | jq -r '.packages[].dependencies[] | select(.kind != "dev" and .path != null and .req == "*") | .name')
```

注释新增的一段：

> Dev-dependencies are exempt (`kind != "dev"`): cargo strips a path-only dev-dependency when
> packaging, so it cannot break publishing. Counting them is worse than useless -- giving
> `pmpx-plugin`'s dev-dependency on `pmpx-loader` a version would add a publish-order edge
> (loader before plugin) to satisfy a rule that does not apply to it.

（jq 语义上 `kind` 对 normal 依赖是 `null`，而 `null != "dev"` 为真，所以 normal/build 仍会被检查 ✓。）

## 怎么在本地复现

CI 用 `jq`（runner 上自带），本机没有 → 用 PowerShell 等价重放：

```powershell
$meta = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
$all  = $meta.packages | ForEach-Object { $_.dependencies } | Where-Object { $_.path -and $_.req -eq '*' }
$nonDev = $all | Where-Object { $_.kind -ne 'dev' }
"path-only 共 $(($all | Measure-Object).Count) 条；其中 dev $(($all | Where-Object { $_.kind -eq 'dev' } | Measure-Object).Count) 条"
if ($nonDev) { "仍会被抓: $($nonDev | ForEach-Object { $_.name })" } else { "改后判定：干净" }
```

当前输出：`path-only 共 1 条；其中 dev 1 条` → `改后判定：干净`，并列出那条是 `pmpx-loader kind=dev` ✓。

## 怎么防止复发

1. 写"必须有 version"这类断言时，**先限定依赖种类**——发布只关心 normal 与 build。
2. 记住 dev-dependency 与发布无关：cargo 打包时会把只有 path 的 dev 依赖剥离掉。
3. **不要为了过检查去改清单**：先问检查的适用范围对不对。这次若改清单，代价是发布顺序多一条边（见上）。
4. 本地改完 `[dependencies]`/`[dev-dependencies]` 后，跑一次上面那段 PowerShell（秒级）。

## 相关

- 文件：`.github/workflows/ci.yaml`（步骤 `Workspace deps declare a version`）
- 被误判的依赖：`crates/pmpx-plugin/Cargo.toml:33`（dev-dependency，供 `tests/dlopen.rs` 使用）
- 修复提交：`ci: exempt dev-dependencies from the version check`
