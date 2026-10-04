//! 已安装插件的清单。
//!
//! 这个模块只把 `~/.pmpx/plugins/*/pmpx-plugin.toml` 读成结构体，**不加载任何动态库** ——
//! 检测必须在"插件是坏的 / ABI 不匹配 / 是别的平台编的"情况下照样给出答案，
//! 加载是选中之后才发生的事（见 [`crate::runtime`]）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use crate_plugin_kit::{CratePluginKit, PluginInfo, PluginManifest};
use pmpx_plugin::abi::PmpxPluginV1;
use pmpx_plugin::Family;

/// 一个已安装插件的清单信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPlugin {
    /// 插件自报名（manifest 的 `plugin.name`，例如 `pnpm`）。
    ///
    /// manifest 是这个名字的权威来源；加载之后 `PackageManager::name()` 必须与它相等，
    /// 否则拒绝加载。
    pub name: String,

    /// 完整 crate 名（`pmpx-plugin-pnpm`）。
    pub crate_name: String,

    /// 版本。
    pub version: String,

    /// 生态。`None` 表示 manifest 没声明 —— 见 [`InstalledPlugin::problem`]。
    pub family: Option<Family>,

    /// manifest 声明的 ABI 版本。
    pub abi: Option<u32>,

    /// 安装目录。
    pub dir: PathBuf,

    /// 强证据（100 分/项）：证明这个后端确实被用过。
    pub strong: Vec<String>,

    /// 弱证据（10 分/项）：只能证明属于这个生态。
    pub weak: Vec<String>,
}

impl InstalledPlugin {
    /// 这个插件能不能参与检测与裁决。
    ///
    /// `plugin ls` 会显示不能的原因 —— "装了但没生效"是最难自己诊断的一类问题。
    pub fn is_usable(&self) -> bool {
        self.problem().is_none()
    }

    /// 它为什么不能参与裁决。
    pub fn problem(&self) -> Option<&'static str> {
        match self.family {
            // family 决定 `plugin ls` 分组、`plugin set` 作用域与 `.pmpx.toml` 的键名，
            // 缺了它插件无法被任何一层选中 —— 与其猜一个生态，不如明确说缺了。
            None => Some("manifest 没有声明 family，无法参与生态裁决"),
            Some(_) if self.strong.is_empty() && self.weak.is_empty() => {
                Some("manifest 的 [detect] 段是空的，永远不会被检测命中")
            }
            Some(_) => None,
        }
    }

    /// 这个插件声明的全部特征文件。
    pub fn detect_names(&self) -> impl Iterator<Item = &str> {
        self.strong
            .iter()
            .chain(self.weak.iter())
            .map(String::as_str)
    }
}

/// 本机已安装的全部插件。
#[derive(Debug, Clone, Default)]
pub struct PluginSet {
    /// 按 crate 名排序，保证输出稳定。
    pub plugins: Vec<InstalledPlugin>,
}

impl PluginSet {
    /// 从插件库里读一遍（只读 manifest）。
    pub fn load(kit: &CratePluginKit<PmpxPluginV1>) -> Result<Self> {
        let infos = kit.list().context("扫描插件目录失败")?;

        let mut plugins = Vec::with_capacity(infos.len());
        for info in infos {
            plugins.push(read_one(kit, &info)?);
        }
        plugins.sort_by(|a, b| a.crate_name.cmp(&b.crate_name));

        Ok(Self { plugins })
    }

    /// 参与裁决的那些。
    pub fn usable(&self) -> impl Iterator<Item = &InstalledPlugin> {
        self.plugins.iter().filter(|p| p.is_usable())
    }

    /// 按自报名找。名字取自 manifest，与 `pmpx -p <name>` 的口径一致。
    pub fn by_name(&self, name: &str) -> Option<&InstalledPlugin> {
        self.plugins.iter().find(|p| p.name == name)
    }

    /// 按 crate 名找。
    pub fn by_crate_name(&self, crate_name: &str) -> Option<&InstalledPlugin> {
        self.plugins.iter().find(|p| p.crate_name == crate_name)
    }

    /// 所有插件声明的特征文件名，去重。
    ///
    /// 一个目录里有其中任何一个，它看起来就是个项目根。这里不分 strong / weak ——
    /// 项目根是结构判断，`package.json` 与 `pnpm-lock.yaml` 在这件事上同样有效。
    pub fn detect_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.usable().flat_map(|p| p.detect_names()).collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// 这个目录看起来是不是一个项目根。
    pub fn marks_root(&self, dir: &Path) -> bool {
        // `.pmpx.toml` 本身就是"我在这里声明这是个项目"—— 它比任何清单文件都强。
        if dir.join(".pmpx.toml").is_file() {
            return true;
        }
        self.detect_names()
            .iter()
            .any(|name| dir.join(name).exists())
    }
}

/// 把一条 [`PluginInfo`] 补全成 [`InstalledPlugin`]（再读一次 manifest 拿 `[detect]`）。
fn read_one(kit: &CratePluginKit<PmpxPluginV1>, info: &PluginInfo) -> Result<InstalledPlugin> {
    // `list()` 刚成功读过这份 manifest，这里失败只可能是文件在两次读之间消失了。
    // 此时退回空 detect —— 插件仍然会被列出来，只是不参与检测。
    let manifest = kit.manifest_of(&info.crate_name).ok();
    let (strong, weak) = manifest.as_ref().map(detect_patterns).unwrap_or_default();

    Ok(InstalledPlugin {
        name: info.name.clone(),
        crate_name: info.crate_name.clone(),
        version: info.version.clone(),
        family: info.family.clone().map(Family::new),
        abi: info.abi,
        dir: info.dir.clone(),
        strong,
        weak,
    })
}

/// 从 manifest 的不认识字段里挖出 `[detect]`。
///
/// `crate-plugin-kit` 不认识它，所以它被原样留在 `extra` 里。
fn detect_patterns(manifest: &PluginManifest) -> (Vec<String>, Vec<String>) {
    let Some(detect) = manifest.extra.get("detect") else {
        return (Vec::new(), Vec::new());
    };

    (
        str_array(detect.get("strong")),
        str_array(detect.get("weak")),
    )
}

/// 把一个 TOML 值读成字符串数组；不是数组、或元素不是字符串时跳过而不是报错 ——
/// 写坏的 `detect` 段不该让整个插件从清单里消失。
fn str_array(value: Option<&toml::Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|x| x.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const PNPM: &str = r#"
[plugin]
name    = "pnpm"
version = "0.1.0"
abi     = 1
family  = "node"

[detect]
strong = ["pnpm-lock.yaml", "pnpm-workspace.yaml"]
weak   = ["package.json"]
"#;

    const CARGO: &str = r#"
[plugin]
name    = "cargo"
version = "0.2.0"
abi     = 1
family  = "rust"

[detect]
strong = ["Cargo.lock"]
weak   = ["Cargo.toml"]
"#;

    /// 造一个插件库，返回（保活, kit, 数据目录）。
    fn store(
        entries: &[(&str, &str)],
    ) -> (tempfile::TempDir, CratePluginKit<PmpxPluginV1>, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("store");

        for (crate_name, manifest) in entries {
            let dir = root.join("plugins").join(crate_name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("pmpx-plugin.toml"), manifest).unwrap();
        }

        let cfg = crate_plugin_kit::KitConfig::new("pmpx")
            .with_data_dir(&root)
            .with_lock_timeout(Duration::from_millis(500));
        let kit = CratePluginKit::<PmpxPluginV1>::new(cfg).unwrap();

        (tmp, kit, root)
    }

    #[test]
    fn an_empty_store_lists_nothing() {
        let (_t, kit, _) = store(&[]);
        let set = PluginSet::load(&kit).unwrap();
        assert!(set.plugins.is_empty());
        assert!(set.detect_names().is_empty());
    }

    #[test]
    fn reads_names_family_and_detect_patterns() {
        let (_t, kit, _) = store(&[("pmpx-plugin-pnpm", PNPM)]);
        let set = PluginSet::load(&kit).unwrap();

        assert_eq!(set.plugins.len(), 1);
        let p = &set.plugins[0];
        assert_eq!(p.name, "pnpm");
        assert_eq!(p.crate_name, "pmpx-plugin-pnpm");
        assert_eq!(p.version, "0.1.0");
        assert_eq!(p.family.as_ref().map(Family::as_str), Some("node"));
        assert_eq!(p.abi, Some(1));
        assert_eq!(p.strong, vec!["pnpm-lock.yaml", "pnpm-workspace.yaml"]);
        assert_eq!(p.weak, vec!["package.json"]);
        assert!(p.is_usable());
    }

    #[test]
    fn results_are_sorted_by_crate_name() {
        let (_t, kit, _) = store(&[("pmpx-plugin-pnpm", PNPM), ("pmpx-plugin-cargo", CARGO)]);
        let set = PluginSet::load(&kit).unwrap();

        let names: Vec<_> = set.plugins.iter().map(|p| p.crate_name.as_str()).collect();
        assert_eq!(names, vec!["pmpx-plugin-cargo", "pmpx-plugin-pnpm"]);
    }

    #[test]
    fn detect_names_are_deduped_across_plugins() {
        let (_t, kit, _) = store(&[("pmpx-plugin-pnpm", PNPM), ("pmpx-plugin-cargo", CARGO)]);
        let set = PluginSet::load(&kit).unwrap();

        let mut names = set.detect_names();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before, "有重复：{names:?}");

        assert!(names.contains(&"Cargo.toml"));
        assert!(names.contains(&"pnpm-lock.yaml"));
        assert!(!names.contains(&"package.json.lock"));
    }

    #[test]
    fn marks_root_on_a_detect_file_or_a_pmpx_toml() {
        let (_t, kit, _) = store(&[("pmpx-plugin-pnpm", PNPM)]);
        let set = PluginSet::load(&kit).unwrap();

        let tmp = tempfile::tempdir().unwrap();
        assert!(!set.marks_root(tmp.path()));

        std::fs::write(tmp.path().join("package.json"), "{}").unwrap();
        assert!(set.marks_root(tmp.path()), "弱证据也算项目根");

        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join(".pmpx.toml"), "").unwrap();
        assert!(set.marks_root(other.path()), ".pmpx.toml 本身就是根标记");
    }

    #[test]
    fn marks_root_on_a_strong_evidence_file_too() {
        let (_t, kit, _) = store(&[("pmpx-plugin-cargo", CARGO)]);
        let set = PluginSet::load(&kit).unwrap();

        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.lock"), "").unwrap();
        assert!(set.marks_root(tmp.path()));
    }

    #[test]
    fn marks_root_is_false_with_no_plugins_installed() {
        let (_t, kit, _) = store(&[]);
        let set = PluginSet::load(&kit).unwrap();

        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "").unwrap();

        // 零插件时**不做任何猜测** —— 这条断言就是"提示表不参与裁决"的具体体现
        assert!(!set.marks_root(tmp.path()));
    }

    #[test]
    fn a_manifest_without_family_is_listed_but_unusable() {
        let (_t, kit, _) = store(&[(
            "pmpx-plugin-weird",
            "[plugin]\nname = \"weird\"\nversion = \"0.1.0\"\n\n[detect]\nstrong = [\"x\"]\n",
        )]);
        let set = PluginSet::load(&kit).unwrap();

        assert_eq!(set.plugins.len(), 1, "仍然要列出来");
        let p = &set.plugins[0];
        assert!(!p.is_usable());
        assert!(p.problem().unwrap().contains("family"));
        assert_eq!(set.usable().count(), 0);
        assert!(set.detect_names().is_empty(), "不可用的插件不参与检测");
    }

    #[test]
    fn a_manifest_with_an_empty_detect_section_is_unusable() {
        let (_t, kit, _) = store(&[(
            "pmpx-plugin-blank",
            "[plugin]\nname = \"blank\"\nversion = \"0.1.0\"\nfamily = \"node\"\n\n[detect]\nstrong = []\nweak = []\n",
        )]);
        let set = PluginSet::load(&kit).unwrap();

        let p = &set.plugins[0];
        assert!(!p.is_usable());
        assert!(p.problem().unwrap().contains("detect"));
    }

    #[test]
    fn a_malformed_detect_value_is_skipped_not_fatal() {
        let (_t, kit, _) = store(&[(
            "pmpx-plugin-odd",
            "[plugin]\nname = \"odd\"\nversion = \"0.1.0\"\nfamily = \"node\"\n\n[detect]\nstrong = \"not-an-array\"\nweak = [1, 2, \"package.json\"]\n",
        )]);
        let set = PluginSet::load(&kit).unwrap();

        let p = &set.plugins[0];
        assert!(p.strong.is_empty(), "非数组当空处理");
        assert_eq!(p.weak, vec!["package.json"], "非字符串元素被跳过");
        assert!(p.is_usable(), "还有一条能用的证据，插件仍可用");
    }

    #[test]
    fn by_name_and_by_crate_name_find_the_same_plugin() {
        let (_t, kit, _) = store(&[("pmpx-plugin-pnpm", PNPM)]);
        let set = PluginSet::load(&kit).unwrap();

        assert_eq!(
            set.by_name("pnpm").map(|p| &p.crate_name),
            Some(&"pmpx-plugin-pnpm".to_string())
        );
        assert_eq!(
            set.by_crate_name("pmpx-plugin-pnpm").map(|p| &p.name),
            Some(&"pnpm".to_string())
        );
        assert!(set.by_name("nope").is_none());
    }
}
