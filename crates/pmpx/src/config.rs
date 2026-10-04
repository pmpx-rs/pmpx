//! 配置：全局一份，项目里可叠多层。
//!
//! 全局是 `<pmpx-config-dir>/config.toml`（唯一一份，个人偏好，不进 git，由
//! `pmpx config set` 写）；项目是各目录下的 `.pmpx.toml`（可有多份，从 cwd 向上逐层
//! 收集，应当提交，由 `pmpx plugin set/unset` 写）。合并规则：**近者优先**。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ---- 路径 -----------------------------------------------------------------

/// 覆盖全局配置目录的环境变量（给测试与多环境用）。
pub const ENV_CONFIG_DIR: &str = "PMPX_CONFIG_DIR";

/// 覆盖插件数据目录的环境变量。理由同上。
pub const ENV_DATA_DIR: &str = "PMPX_DATA_DIR";

/// 全局配置文件路径（各平台的配置目录位置不同，交给 `directories` 判断）。
/// [`ENV_CONFIG_DIR`] 可以整体覆盖，那是个**目录**不是文件。
pub fn global_config_path() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os(ENV_CONFIG_DIR) {
        return Ok(PathBuf::from(dir).join("config.toml"));
    }
    let dirs = directories::ProjectDirs::from("", "", "pmpx")
        .context("拿不到配置目录（既没有 HOME 也没有 APPDATA？）")?;
    Ok(dirs.config_dir().join("config.toml"))
}

/// 插件数据目录：`~/.pmpx`，三个平台都是这一个位置（不是 `directories` 的 data_dir），
/// 因为插件目录要出现在用户手边。
///
/// 它显式交给 `crate-plugin-kit` 的 `KitConfig::with_data_dir` —— 否则"pmpx 显示的位置"
/// 与"kit 实际用的位置"会成为两处实现。可用 [`ENV_DATA_DIR`] 覆盖。
pub fn default_data_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os(ENV_DATA_DIR) {
        return Ok(PathBuf::from(dir));
    }
    let dirs =
        directories::UserDirs::new().context("拿不到用户目录（HOME / USERPROFILE 都没设？）")?;
    Ok(dirs.home_dir().join(".pmpx"))
}

/// 把开头的 `~` 展开成用户主目录（不处理 `~user`）。
pub fn expand_tilde(raw: &str) -> PathBuf {
    let rest = raw
        .strip_prefix("~/")
        .or_else(|| raw.strip_prefix("~\\"))
        .or_else(|| if raw == "~" { Some("") } else { None });

    match rest {
        Some(rest) => match directories::UserDirs::new() {
            Some(dirs) => dirs.home_dir().join(rest),
            None => PathBuf::from(raw),
        },
        None => PathBuf::from(raw),
    }
}

// ---- 全局配置 -------------------------------------------------------------

/// `<pmpx-config-dir>/config.toml`，启动时一次读完 —— `[discovery]` 影响所有命令。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalConfig {
    /// 插件相关的次序表。
    pub plugin: GlobalPluginConfig,
    /// 项目根发现的行为。
    pub discovery: DiscoveryConfig,
    /// 插件库的位置与安装偏好。
    pub plugin_store: PluginStoreConfig,

    /// 不认识的键原样保留 —— `pmpx config set` 是读-改-写，丢了就会静默吃掉用户配置。
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// 跨生态与生态内的次序表。这是"混合项目默认走 Node"的唯一来源。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalPluginConfig {
    /// 生态之间的次序：**跨生态同分时**裁决，越靠前越优先。
    /// 未列出的位于所有列出的之后，按名字典序 —— 不认识的生态也能被裁决。
    pub family_priority: Vec<String>,

    /// 同一生态内插件的次序：**同分时**裁决。
    pub priority: Vec<String>,
}

impl Default for GlobalPluginConfig {
    fn default() -> Self {
        Self {
            // 混合项目（Rust + 前端）默认走 Node —— 靠这个数组，而不是某处写死的规则。
            family_priority: vec![
                "node".into(),
                "rust".into(),
                "python".into(),
                "go".into(),
                "jvm".into(),
                "dotnet".into(),
                "php".into(),
                "ruby".into(),
            ],
            // 只有一个 package.json 时（四个 Node 后端都 10 分）默认选 pnpm。
            priority: vec![
                "pnpm".into(),
                "npm".into(),
                "yarn".into(),
                "bun".into(),
                "cargo".into(),
            ],
        }
    }
}

/// `[discovery]`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscoveryConfig {
    /// 要不要向上找项目根。`false` = 只看起点目录。
    pub walk_up: bool,
    /// 最多检查几个目录（含起点）。
    pub max_depth: usize,
    /// 遇到 `.git` 就停（仓库根再往上不属于本项目）。
    pub stop_at_git: bool,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            walk_up: true,
            max_depth: 8,
            stop_at_git: true,
        }
    }
}

/// `[plugin_store]`：插件库的位置与安装偏好。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginStoreConfig {
    /// 覆盖插件目录。留空 = [`default_data_dir`]。
    pub data_dir: Option<PathBuf>,
    /// 有 prebuilt 就下载，失败自动回落 build-host。
    pub prefer_prebuilt: Option<bool>,
}

impl PluginStoreConfig {
    /// 生效的数据目录。
    pub fn effective_data_dir(&self) -> Result<PathBuf> {
        match &self.data_dir {
            Some(p) => Ok(expand_tilde(&p.to_string_lossy())),
            None => default_data_dir(),
        }
    }

    /// 生效的 prebuilt 偏好。
    pub fn effective_prefer_prebuilt(&self) -> bool {
        self.prefer_prebuilt.unwrap_or(true)
    }
}

impl GlobalConfig {
    /// 读全局配置。**文件不存在 = 全默认，不是错误。**
    pub fn load() -> Result<Self> {
        Self::load_from(&global_config_path()?)
    }

    /// 从指定路径读。测试用。
    pub fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .with_context(|| format!("解析全局配置失败：{}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("读全局配置失败：{}", path.display())),
        }
    }
}

// ---- 项目配置 -------------------------------------------------------------

/// 一份 `.pmpx.toml`。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    /// `[plugin] <family> = "<name>"`：键是 **family 名字**而不是插件名。
    /// 用 `BTreeMap<String, _>` 是为了让第三方插件带来的新生态不等发版就能写进来。
    pub plugin: BTreeMap<String, String>,

    /// `[scripts] name = "run something"`：语义还没定义，只解析并原样保留 ——
    /// `plugin set/unset` 的读-改-写不能吃掉它。
    pub scripts: BTreeMap<String, String>,

    /// 不认识的键原样保留，理由同 [`GlobalConfig::extra`]。
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl ProjectConfig {
    /// 读一份。文件不存在返回 `None`（调用方据此知道"这里没有配置"）。
    pub fn load_from(path: &Path) -> Result<Option<Self>> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let cfg: Self = toml::from_str(&text)
                    .with_context(|| format!("解析项目配置失败：{}", path.display()))?;
                Ok(Some(cfg))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("读项目配置失败：{}", path.display())),
        }
    }

    /// 把 `other` 叠在 `self` 上，`other` 赢（它就是"更近的那一层"）。
    pub fn overlay(&mut self, other: ProjectConfig) {
        // 顺序很重要：先 extend 未知键，再让已知键覆盖，否则同名的未知键会顶掉已知键。
        for (k, v) in other.extra {
            self.extra.insert(k, v);
        }
        self.plugin.extend(other.plugin);
        self.scripts.extend(other.scripts);
    }
}

/// 从近到远收集到的所有 `.pmpx.toml` 合并结果。
#[derive(Debug, Clone, Default)]
pub struct MergedProjectConfig {
    /// 生效的 `[plugin]` 固化项：family → 插件名。
    pub plugin: BTreeMap<String, String>,
    /// 生效的 `[scripts]`。暂时没有读取方，但不能删：少了它，之后任何读取方
    /// 都看不到用户手写的脚本。
    #[allow(dead_code)]
    pub scripts: BTreeMap<String, String>,
    /// 实际读到的文件，**从近到远**（`pmpx info` 用它说清值是从哪几份配置来的）。
    pub sources: Vec<PathBuf>,
}

impl MergedProjectConfig {
    /// 按"近者优先"合并；`paths` 必须从近到远（调用方逐层上溯收集）。
    pub fn from_paths_near_to_far(paths: &[PathBuf]) -> Result<Self> {
        let mut merged = ProjectConfig::default();
        let mut found = Vec::new();

        // 从最远的开始铺，越近的越后写 —— 后者自然覆盖前者。
        for path in paths.iter().rev() {
            if let Some(cfg) = ProjectConfig::load_from(path)? {
                merged.overlay(cfg);
                found.push(path.clone());
            }
        }

        // `found` 此时是从远到近，翻过来变成从近到远，与入参口径一致。
        found.reverse();

        Ok(Self {
            plugin: merged.plugin,
            scripts: merged.scripts,
            sources: found,
        })
    }

    /// 某个生态被固化成了哪个插件。
    pub fn pinned_plugin(&self, family: &str) -> Option<&str> {
        self.plugin.get(family).map(String::as_str)
    }

    /// 被 pin 过的所有 family。
    pub fn pinned_families(&self) -> Vec<&str> {
        self.plugin.keys().map(String::as_str).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn missing_global_config_is_all_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = GlobalConfig::load_from(&tmp.path().join("nope.toml")).unwrap();

        assert_eq!(cfg.plugin.family_priority[0], "node");
        assert_eq!(cfg.plugin.priority[0], "pnpm");
        assert!(cfg.discovery.walk_up);
        assert_eq!(cfg.discovery.max_depth, 8);
        assert!(cfg.discovery.stop_at_git);
        assert!(cfg.plugin_store.effective_prefer_prebuilt());
    }

    #[test]
    fn unknown_keys_in_the_global_config_are_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(
            tmp.path(),
            "config.toml",
            r#"
[plugin]
family_priority = ["rust", "node"]

# 用户自己加的，pmpx 不认识
[my_own_thing]
keep = "me"
"#,
        );

        let cfg = GlobalConfig::load_from(&path).unwrap();
        assert_eq!(cfg.plugin.family_priority, vec!["rust", "node"]);
        assert_eq!(cfg.plugin.priority[0], "pnpm");
        // 不认识的段必须活下来 —— `pmpx config set` 读-改-写时全靠它原样带回文件里
        assert!(cfg.extra.contains_key("my_own_thing"));
    }

    #[test]
    fn a_broken_global_config_is_an_error_not_a_silent_default() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(tmp.path(), "config.toml", "this is not toml = = =");

        let err = GlobalConfig::load_from(&path).unwrap_err();
        assert!(err.to_string().contains("解析全局配置失败"), "{err}");
    }

    #[test]
    fn project_config_missing_is_none_not_default() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(ProjectConfig::load_from(&tmp.path().join("nope.toml"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn near_config_wins_over_far_one() {
        let tmp = tempfile::tempdir().unwrap();
        let far = write(
            tmp.path(),
            "far.toml",
            "[plugin]\nnode = \"npm\"\nrust = \"cargo\"\n",
        );
        let near = write(tmp.path(), "near.toml", "[plugin]\nnode = \"pnpm\"\n");

        let merged =
            MergedProjectConfig::from_paths_near_to_far(&[near.clone(), far.clone()]).unwrap();

        assert_eq!(merged.pinned_plugin("node"), Some("pnpm"), "近的赢");
        assert_eq!(merged.pinned_plugin("rust"), Some("cargo"), "远的没被顶掉");
        assert_eq!(merged.pinned_plugin("python"), None);
    }

    #[test]
    fn overlaying_unknown_keys_does_not_clobber_known_ones() {
        let mut base = ProjectConfig::default();
        base.plugin.insert("node".into(), "npm".into());

        let mut other = ProjectConfig::default();
        other.plugin.insert("node".into(), "pnpm".into());
        base.overlay(other);

        assert_eq!(base.plugin.get("node").map(String::as_str), Some("pnpm"));
    }

    #[test]
    fn scripts_merge_across_layers() {
        let tmp = tempfile::tempdir().unwrap();
        let far = write(
            tmp.path(),
            "far.toml",
            "[scripts]\nfmt = \"run format\"\nlint = \"run lint\"\n",
        );
        let near = write(tmp.path(), "near.toml", "[scripts]\nfmt = \"run f\"\n");

        let merged = MergedProjectConfig::from_paths_near_to_far(&[near, far]).unwrap();
        assert_eq!(merged.scripts.get("fmt").map(String::as_str), Some("run f"));
        assert_eq!(
            merged.scripts.get("lint").map(String::as_str),
            Some("run lint")
        );
    }

    #[test]
    fn sources_are_recorded_near_to_far() {
        let tmp = tempfile::tempdir().unwrap();
        let far = write(tmp.path(), "far.toml", "[plugin]\nrust = \"cargo\"\n");
        let near = write(tmp.path(), "near.toml", "[plugin]\nnode = \"pnpm\"\n");

        let merged =
            MergedProjectConfig::from_paths_near_to_far(&[near.clone(), far.clone()]).unwrap();

        assert_eq!(merged.sources, vec![near, far], "从近到远");
    }

    #[test]
    fn a_config_file_that_vanished_mid_walk_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let real = write(tmp.path(), "real.toml", "[plugin]\nrust = \"cargo\"\n");
        let ghost = tmp.path().join("ghost.toml");

        let merged = MergedProjectConfig::from_paths_near_to_far(&[ghost, real]).unwrap();
        assert_eq!(merged.pinned_plugin("rust"), Some("cargo"));
        assert_eq!(merged.sources.len(), 1);
    }

    #[test]
    fn expand_tilde_expands_to_home() {
        let home = directories::UserDirs::new()
            .unwrap()
            .home_dir()
            .to_path_buf();

        assert_eq!(expand_tilde("~/x/y"), home.join("x").join("y"));
        assert_eq!(expand_tilde("~"), home);
        // 不处理 `~user`，也不该把普通路径改坏
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(
            expand_tilde("relative/path"),
            PathBuf::from("relative/path")
        );
    }

    #[test]
    fn pinned_families_lists_every_pin() {
        let tmp = tempfile::tempdir().unwrap();
        let a = write(
            tmp.path(),
            "a.toml",
            "[plugin]\nnode = \"pnpm\"\nrust = \"cargo\"\n",
        );

        let merged = MergedProjectConfig::from_paths_near_to_far(&[a]).unwrap();
        let mut fams = merged.pinned_families();
        fams.sort_unstable();
        assert_eq!(fams, vec!["node", "rust"]);
    }

    // ---- 环境变量覆盖 -----------------------------------------------------

    /// 两个覆盖变量必须真的生效 —— 否则 e2e 测试只能去动用户真实的配置目录。
    ///
    /// 它改的是进程级环境变量：只在同一个测试里设、存原值、断言前还原，
    /// 且只碰本模块读的 `PMPX_` 前缀变量。
    #[test]
    fn env_overrides_take_effect() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("cfg");
        let data_dir = tmp.path().join("data");

        let old_cfg = std::env::var_os(ENV_CONFIG_DIR);
        let old_data = std::env::var_os(ENV_DATA_DIR);

        // SAFETY: 见上面关于作用域的说明。
        unsafe {
            std::env::set_var(ENV_CONFIG_DIR, &cfg_dir);
            std::env::set_var(ENV_DATA_DIR, &data_dir);
        }

        let got_cfg = global_config_path().unwrap();
        let got_data = default_data_dir().unwrap();

        // 还原要在断言之前 —— 断言失败也不能把环境弄脏
        unsafe {
            match old_cfg {
                Some(v) => std::env::set_var(ENV_CONFIG_DIR, v),
                None => std::env::remove_var(ENV_CONFIG_DIR),
            }
            match old_data {
                Some(v) => std::env::set_var(ENV_DATA_DIR, v),
                None => std::env::remove_var(ENV_DATA_DIR),
            }
        }

        assert_eq!(got_cfg, cfg_dir.join("config.toml"));
        assert_eq!(got_data, data_dir);
    }

    /// 没有覆盖变量时，走的是平台正确的位置。
    #[test]
    fn without_overrides_the_platform_paths_are_used() {
        if std::env::var_os(ENV_CONFIG_DIR).is_none() {
            let p = global_config_path().unwrap();
            assert!(p.ends_with("config.toml"), "{p:?}");
            assert!(
                p.to_string_lossy().contains("pmpx"),
                "路径里应当有 pmpx：{p:?}"
            );
        }
        if std::env::var_os(ENV_DATA_DIR).is_none() {
            let p = default_data_dir().unwrap();
            assert_eq!(p.file_name().unwrap().to_string_lossy(), ".pmpx");
        }
    }
}
