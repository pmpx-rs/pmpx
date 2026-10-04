//! 检测与裁决。两件事，按顺序：
//!
//! 1. **打分** —— 每个已装插件在项目根里命中几个特征文件（强证据 100 分，弱证据 10 分）；
//! 2. **裁决** —— 先选生态（family），再在那个生态里选插件。
//!
//! 没有锁文件时判不出生态：库 crate 常把 `Cargo.lock` gitignore 掉，此时 `Cargo.toml`（10）
//! 与 `package.json`（10）同分，会按 `family_priority` 判成 node。出口是在 `.pmpx.toml`
//! 里 pin（地板 50），或者调整 `family_priority`。

use std::collections::BTreeMap;
use std::path::Path;

use pmpx_plugin::Family;

use crate::config::{GlobalConfig, MergedProjectConfig};
use crate::plugins::{InstalledPlugin, PluginSet};

/// 命中一项**强证据**得多少分。
pub const STRONG_SCORE: u32 = 100;
/// 命中一项**弱证据**得多少分；它永远压不过一个锁文件（100 分），
/// 插件作者不必纠结某个文件该放哪一档。
pub const WEAK_SCORE: u32 = 10;
/// `.pmpx.toml` pin 了某个 family 时，该 family 的**得分地板**。
///
/// 它比 10 分的清单文件强（能救回没有锁文件的库 crate），比 100 分的锁文件弱
/// （真有锁文件时不越权）。
pub const PIN_FLOOR: u32 = 50;

// 地板必须落在弱证据与强证据之间。
const _: () = assert!(PIN_FLOOR > WEAK_SCORE);
const _: () = assert!(PIN_FLOOR < STRONG_SCORE);

// ---- 打分 -----------------------------------------------------------------

/// 一个插件在某个目录里的得分明细。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoredPlugin {
    /// crate 名。
    pub crate_name: String,
    /// 插件自报名。
    pub name: String,
    /// 命中的强证据文件。
    pub strong_hits: Vec<String>,
    /// 命中的弱证据文件。
    pub weak_hits: Vec<String>,
    /// `100 × strong_hits + 10 × weak_hits`
    pub score: u32,
}

impl ScoredPlugin {
    /// 在 `dir` 里给一个插件打分。
    pub fn score(plugin: &InstalledPlugin, dir: &Path) -> Self {
        let strong_hits = hits(dir, &plugin.strong);
        let weak_hits = hits(dir, &plugin.weak);

        let score = STRONG_SCORE * strong_hits.len() as u32 + WEAK_SCORE * weak_hits.len() as u32;

        Self {
            crate_name: plugin.crate_name.clone(),
            name: plugin.name.clone(),
            strong_hits,
            weak_hits,
            score,
        }
    }

    /// 命中的全部文件（强 + 弱），给 `info` 显示用。
    pub fn all_hits(&self) -> impl Iterator<Item = &str> {
        self.strong_hits
            .iter()
            .chain(self.weak_hits.iter())
            .map(String::as_str)
    }
}

/// 一个生态的得分与它辖下的插件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FamilyScore {
    /// 生态。
    pub family: Family,
    /// `max(辖下插件最高分, pin ? 50 : 0)`
    pub score: u32,
    /// `.pmpx.toml` 是否 pin 了这个生态。
    pub pinned: bool,
    /// 辖下的已装插件（含 0 分的 —— `info` 要把它们显示出来）。
    pub plugins: Vec<ScoredPlugin>,
}

fn hits(dir: &Path, names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = names
        .iter()
        .filter(|n| dir.join(n.as_str()).exists())
        .cloned()
        .collect();
    out.sort();
    out
}

/// 给所有已装插件打分，再按生态汇总。
///
/// 返回的 map 里**也会包含**"被 pin 了但一个插件都没装"的生态（得分 = 地板）——
/// 那种情况必须能被选中，才能报出"缺少能处理该生态的插件"，而不是含糊的
/// "检测不到项目类型"。
pub fn score_all(
    set: &PluginSet,
    root: &Path,
    merged: &MergedProjectConfig,
) -> BTreeMap<Family, FamilyScore> {
    let mut families: BTreeMap<Family, FamilyScore> = BTreeMap::new();

    for plugin in set.usable() {
        // `usable()` 保证 family 是 Some
        let Some(family) = plugin.family.clone() else {
            continue;
        };

        let scored = ScoredPlugin::score(plugin, root);
        let entry = families
            .entry(family.clone())
            .or_insert_with(|| FamilyScore {
                family,
                score: 0,
                pinned: false,
                plugins: Vec::new(),
            });

        // 生态分 = 辖下插件的**最高**分，不是求和 —— 四个 Node 后端各命中
        // `package.json` 只是同一份弱证据被数了四遍。
        entry.score = entry.score.max(scored.score);
        entry.plugins.push(scored);
    }

    // pin 地板在插件分算完之后应用，所以是 max 而不是覆盖。
    for (family, fs) in families.iter_mut() {
        if merged.pinned_plugin(family.as_str()).is_some() {
            fs.pinned = true;
            fs.score = fs.score.max(PIN_FLOOR);
        }
    }

    // 被 pin 但没装插件的生态也要进场。
    for name in merged.pinned_families() {
        let family = Family::new(name.to_string());
        families
            .entry(family.clone())
            .or_insert_with(|| FamilyScore {
                family,
                score: PIN_FLOOR,
                pinned: true,
                plugins: Vec::new(),
            });
    }

    for fs in families.values_mut() {
        fs.plugins.sort_by(|a, b| a.crate_name.cmp(&b.crate_name));
    }

    families
}

// ---- 裁决 -----------------------------------------------------------------

/// 裁决结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// 胜出插件的 crate 名（加载时用它定位目录）。
    pub crate_name: String,
    /// 胜出插件自报名。
    pub name: String,
    /// 胜出生态。
    pub family: Family,
    /// 胜出插件的得分（用于 `info`）。
    pub score: u32,
    /// 给用户看的提示；`--quiet` 会关掉它们。
    pub notes: Vec<String>,
}

/// 选不出来时的原因。
///
/// 每一个变体都对应一句**能指导下一步**的话。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectFailure {
    /// 所有候选都是 0 分，而且没有任何 pin。
    NothingDetected,
    /// `.pmpx.toml` pin 了一个没装的插件。
    PinnedNotInstalled {
        /// 生态名。
        family: String,
        /// pin 的插件名。
        name: String,
    },
    /// `.pmpx.toml` pin 的插件**装了**，但它不能参与裁决。
    ///
    /// 与 [`DetectFailure::PinnedNotInstalled`] 分开是因为下一步动作完全不同：
    /// "没装"要去装，"装了但坏了"要去看 `pmpx plugin ls` 里标出来的原因。
    PluginUnusable {
        /// 插件名。
        name: String,
        /// 它为什么不能用。
        problem: String,
    },
    /// `-p` 指定的插件不存在（或不能参与裁决）。
    UnknownPlugin {
        /// 用户给的名字。
        name: String,
        /// 可选项，已经排好序。
        available: Vec<String>,
    },
    /// 胜出的生态里一个可用的插件都没有。
    ///
    /// **防御性分支**：正常路径下走不到 —— 生态分要么来自插件（那就有插件），
    /// 要么来自 pin（那就会先被上面两个变体拦下）。给一句有用的话比 `unreachable!()` 好。
    FamilyWithoutPlugin {
        /// 生态名。
        family: String,
        /// 该生态的得分。
        score: u32,
    },
}

/// 按 `order` 表给 `name` 排名。**不在表里的排最后**。
fn rank(order: &[String], name: &str) -> usize {
    order.iter().position(|x| x == name).unwrap_or(order.len())
}

/// 完整的裁决。
///
/// `explicit` 是 `-p/--plugin` 的值：**它压过包括 `.pmpx.toml` 在内的一切** ——
/// `-p` 是一次性临时覆盖，`.pmpx.toml` 是持久固化，临时的那次应当赢。
pub fn select(
    set: &PluginSet,
    root: &Path,
    merged: &MergedProjectConfig,
    global: &GlobalConfig,
    explicit: Option<&str>,
) -> Result<Selection, DetectFailure> {
    // 第 0 层：-p 直接指定。跳过后面全部裁决。
    if let Some(name) = explicit {
        let Some(plugin) = set.by_name(name) else {
            return Err(DetectFailure::UnknownPlugin {
                name: name.to_string(),
                available: set.usable().map(|p| p.name.clone()).collect(),
            });
        };
        let Some(family) = plugin.family.clone() else {
            return Err(DetectFailure::UnknownPlugin {
                name: name.to_string(),
                available: set.usable().map(|p| p.name.clone()).collect(),
            });
        };

        return Ok(Selection {
            crate_name: plugin.crate_name.clone(),
            name: plugin.name.clone(),
            family,
            score: 0,
            // `-p` 是用户明确要的，没有任何歧义要提示
            notes: Vec::new(),
        });
    }

    // 第 1 层：选 family
    let families = score_all(set, root, merged);

    let mut candidates: Vec<FamilyScore> = families.into_values().filter(|f| f.score > 0).collect();

    if candidates.is_empty() {
        return Err(DetectFailure::NothingDetected);
    }

    // 排序键：得分降序 → family_priority 名次升序 → 名字典序。
    // 最后一条保证未列出的生态也拿到一个确定的次序。
    candidates.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| {
                rank(&global.plugin.family_priority, a.family.as_str())
                    .cmp(&rank(&global.plugin.family_priority, b.family.as_str()))
            })
            .then_with(|| a.family.as_str().cmp(b.family.as_str()))
    });

    let winner = candidates.remove(0);
    let mut notes = Vec::new();

    // 同分就提示 —— 不是因为结果不确定，而是因为用户可能想要另一个。
    if candidates.first().is_some_and(|r| r.score == winner.score) {
        let tied: Vec<&str> = std::iter::once(winner.family.as_str())
            .chain(
                candidates
                    .iter()
                    .filter(|c| c.score == winner.score)
                    .map(|c| c.family.as_str()),
            )
            .collect();
        notes.push(format!(
            "检测到多个候选（{} 同为 {} 分），已选 {}",
            tied.join(" / "),
            winner.score,
            winner.family
        ));
    }

    // 第 2 层：在该 family 内选插件
    let family_name = winner.family.as_str().to_string();

    // 2-a：`.pmpx.toml` 的 `[plugin] <family> = "<name>"`
    if let Some(pinned) = merged.pinned_plugin(&family_name) {
        if let Some(p) = winner.plugins.iter().find(|p| p.name == pinned) {
            return Ok(Selection {
                crate_name: p.crate_name.clone(),
                name: p.name.clone(),
                family: winner.family,
                score: p.score,
                notes,
            });
        }

        // 不在这个生态的可用列表里 —— 有两种截然不同的原因，必须分开报。
        if let Some(existing) = set.by_name(pinned) {
            // 装了，但它自己不可用（缺 family / detect 段是空的），
            // 或者它声明的生态与 `.pmpx.toml` 里写的那一行对不上。
            return Err(DetectFailure::PluginUnusable {
                name: pinned.to_string(),
                problem: existing
                    .problem()
                    .unwrap_or("它声明的 family 与 .pmpx.toml 里写的不一致")
                    .to_string(),
            });
        }

        return Err(DetectFailure::PinnedNotInstalled {
            family: family_name,
            name: pinned.to_string(),
        });
    }

    // 2-b：得分最高者。0 分的不参与。
    let mut ranked: Vec<&ScoredPlugin> = winner.plugins.iter().filter(|p| p.score > 0).collect();

    if ranked.is_empty() {
        return Err(DetectFailure::FamilyWithoutPlugin {
            family: family_name,
            score: winner.score,
        });
    }

    ranked.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| {
                rank(&global.plugin.priority, &a.name).cmp(&rank(&global.plugin.priority, &b.name))
            })
            .then_with(|| a.name.cmp(&b.name))
    });

    if ranked.len() > 1 && ranked[1].score == ranked[0].score {
        let tied: Vec<&str> = ranked
            .iter()
            .filter(|p| p.score == ranked[0].score)
            .map(|p| p.name.as_str())
            .collect();
        notes.push(format!(
            "检测到多个候选（{} 同为 {} 分），已选 {}",
            tied.join(" / "),
            ranked[0].score,
            ranked[0].name
        ));
    }

    let best = ranked[0];
    Ok(Selection {
        crate_name: best.crate_name.clone(),
        name: best.name.clone(),
        family: winner.family,
        score: best.score,
        notes,
    })
}

impl DetectFailure {
    /// 给用户看的完整说明，必须包含下一步该做什么。
    pub fn message(&self) -> String {
        match self {
            DetectFailure::NothingDetected => "检测不到项目类型。\n\
                 pmpx 靠**已安装插件**声明的特征文件来判断项目类型，所以：\n\
                   · 还没装插件时，任何项目都检测不出来（`pmpx plugin add <name>`）\n\
                   · 也可以在当前目录写一个 .pmpx.toml 显式声明，例如：\n\
                     [plugin]\n\
                     rust = \"cargo\""
                .to_string(),
            DetectFailure::FamilyWithoutPlugin { family, score } => format!(
                "胜出的生态是 {family}（{score} 分），但没有安装任何属于它的插件。\n\
                 装上之后 pmpx 才知道该调用哪个工具。"
            ),
            DetectFailure::PinnedNotInstalled { family, name } => format!(
                ".pmpx.toml 里把 {family} 固化成了 {name}，但它没有安装。\n\
                 要么装它（`pmpx plugin add {name}`），要么改掉那条固化。"
            ),
            DetectFailure::PluginUnusable { name, problem } => format!(
                "{name} 装了，但不能参与裁决：{problem}。\n\
                 用 `pmpx plugin ls` 看全部插件与它们各自的问题。"
            ),
            DetectFailure::UnknownPlugin { name, available } => {
                let mut msg = format!("找不到插件 {name}。");
                if available.is_empty() {
                    msg.push_str("\n当前一个插件都没装（`pmpx plugin add <name>`）。");
                } else {
                    msg.push_str("\n已装的是：");
                    msg.push_str(&available.join(", "));
                }
                msg
            }
        }
    }
}

/// 选不出来 = **退出码 3**：pmpx 本身没问题，是环境里缺东西。
/// 用户的脚本该能靠这个码把"pmpx 坏了"（1）和"你还没装东西"（3）分开。
impl From<DetectFailure> for crate::error::PmpxError {
    fn from(f: DetectFailure) -> Self {
        crate::error::PmpxError::NotFound(f.message())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GlobalConfig;
    use crate_plugin_kit::CratePluginKit;
    use pmpx_plugin::abi::PmpxPluginV1;
    use std::path::PathBuf;
    use std::time::Duration;

    struct Fixture {
        _tmp: tempfile::TempDir,
        project: PathBuf,
        set: PluginSet,
        global: GlobalConfig,
    }

    fn plugin_manifest(name: &str, family: &str, strong: &[&str], weak: &[&str]) -> String {
        let s: Vec<String> = strong.iter().map(|x| format!("\"{x}\"")).collect();
        let w: Vec<String> = weak.iter().map(|x| format!("\"{x}\"")).collect();
        format!(
            "[plugin]\nname = \"{name}\"\nversion = \"0.1.0\"\nabi = 1\nfamily = \"{family}\"\n\n\
             [detect]\nstrong = [{}]\nweak = [{}]\n",
            s.join(", "),
            w.join(", ")
        )
    }

    /// 建一个场景：装若干插件 + 在项目目录里放若干文件（`files` 相对项目根）。
    fn fixture(plugins: &[(&str, &str, &[&str], &[&str])], files: &[&str]) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join("store");
        for (crate_name, family, strong, weak) in plugins {
            let dir = store.join("plugins").join(crate_name);
            std::fs::create_dir_all(&dir).unwrap();
            let name = crate_name.trim_start_matches("pmpx-plugin-");
            std::fs::write(
                dir.join("pmpx-plugin.toml"),
                plugin_manifest(name, family, strong, weak),
            )
            .unwrap();
        }

        let cfg = crate_plugin_kit::KitConfig::new("pmpx")
            .with_data_dir(&store)
            .with_lock_timeout(Duration::from_millis(500));
        let kit = CratePluginKit::<PmpxPluginV1>::new(cfg).unwrap();
        let set = PluginSet::load(&kit).unwrap();

        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        for f in files {
            let p = project.join(f);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(p, "").unwrap();
        }

        Fixture {
            _tmp: tmp,
            project,
            set,
            global: GlobalConfig::default(),
        }
    }

    /// 全部官方插件的 detect 声明。
    fn official() -> Vec<(
        &'static str,
        &'static str,
        &'static [&'static str],
        &'static [&'static str],
    )> {
        vec![
            (
                "pmpx-plugin-cargo",
                "rust",
                &["Cargo.lock"],
                &["Cargo.toml"],
            ),
            (
                "pmpx-plugin-npm",
                "node",
                &["package-lock.json", "npm-shrinkwrap.json"],
                &["package.json"],
            ),
            (
                "pmpx-plugin-pnpm",
                "node",
                &["pnpm-lock.yaml", "pnpm-workspace.yaml"],
                &["package.json"],
            ),
            (
                "pmpx-plugin-yarn",
                "node",
                &["yarn.lock", ".yarnrc.yml"],
                &["package.json"],
            ),
            (
                "pmpx-plugin-bun",
                "node",
                &["bun.lock", "bun.lockb"],
                &["package.json"],
            ),
        ]
    }

    fn merged(pins: &[(&str, &str)]) -> MergedProjectConfig {
        let mut m = MergedProjectConfig::default();
        for (f, p) in pins {
            m.plugin.insert((*f).to_string(), (*p).to_string());
        }
        m
    }

    fn pick(fx: &Fixture, pins: &[(&str, &str)], explicit: Option<&str>) -> Selection {
        select(&fx.set, &fx.project, &merged(pins), &fx.global, explicit)
            .unwrap_or_else(|e| panic!("应当选得出来：{}", e.message()))
    }

    fn fail(fx: &Fixture, pins: &[(&str, &str)], explicit: Option<&str>) -> DetectFailure {
        select(&fx.set, &fx.project, &merged(pins), &fx.global, explicit).unwrap_err()
    }

    // ---- 打分 -------------------------------------------------------------

    #[test]
    fn scoring_is_one_hundred_per_strong_and_ten_per_weak() {
        let fx = fixture(&official(), &["Cargo.toml", "Cargo.lock"]);
        let cargo = fx.set.by_name("cargo").unwrap();
        let scored = ScoredPlugin::score(cargo, &fx.project);

        assert_eq!(scored.strong_hits, vec!["Cargo.lock"]);
        assert_eq!(scored.weak_hits, vec!["Cargo.toml"]);
        assert_eq!(scored.score, 110);
    }

    #[test]
    fn a_weak_only_plugin_scores_ten() {
        let fx = fixture(&official(), &["package.json"]);
        let pnpm = fx.set.by_name("pnpm").unwrap();
        assert_eq!(ScoredPlugin::score(pnpm, &fx.project).score, 10);
    }

    #[test]
    fn a_plugin_with_no_hits_scores_zero() {
        let fx = fixture(&official(), &[]);
        let cargo = fx.set.by_name("cargo").unwrap();
        assert_eq!(ScoredPlugin::score(cargo, &fx.project).score, 0);
    }

    // ---- 典型场景对照 ------------------------------------------------------

    #[test]
    fn scenario_cargo_toml_only_picks_cargo() {
        let fx = fixture(&official(), &["Cargo.toml"]);
        let s = pick(&fx, &[], None);
        assert_eq!(s.name, "cargo");
        assert_eq!(s.family, Family::RUST);
        assert_eq!(s.score, 10);
    }

    #[test]
    fn scenario_cargo_lock_picks_cargo() {
        let fx = fixture(&official(), &["Cargo.toml", "Cargo.lock"]);
        let s = pick(&fx, &[], None);
        assert_eq!(s.name, "cargo");
        assert_eq!(s.score, 110);
    }

    /// 只有一个 `package.json` 时四个后端都是 10 分 → 按 `priority` 选 pnpm。
    #[test]
    fn scenario_package_json_alone_falls_back_to_pnpm() {
        let fx = fixture(&official(), &["package.json"]);
        let s = pick(&fx, &[], None);
        assert_eq!(s.name, "pnpm");
        assert_eq!(s.score, 10);
        assert!(!s.notes.is_empty(), "同分必须有提示（5.4）");
    }

    #[test]
    fn scenario_pnpm_lock_picks_pnpm() {
        let fx = fixture(&official(), &["package.json", "pnpm-lock.yaml"]);
        let s = pick(&fx, &[], None);
        assert_eq!(s.name, "pnpm");
        assert_eq!(s.score, 110);
    }

    #[test]
    fn scenario_package_lock_picks_npm() {
        let fx = fixture(&official(), &["package.json", "package-lock.json"]);
        let s = pick(&fx, &[], None);
        assert_eq!(s.name, "npm");
    }

    /// 混合项目：10 vs 10 → 默认 `family_priority` 把 node 排在前面 → 走 node。
    /// 这不是硬编码规则，是那个数组的自然结果。
    #[test]
    fn scenario_mixed_at_ten_ten_goes_to_node() {
        let fx = fixture(&official(), &["Cargo.toml", "package.json"]);
        let s = pick(&fx, &[], None);
        assert_eq!(s.family, Family::NODE);
        assert_eq!(s.name, "pnpm");
    }

    /// 把 rust 提到 family_priority 首位，同一个项目就走 cargo。
    #[test]
    fn scenario_mixed_follows_family_priority() {
        let mut fx = fixture(&official(), &["Cargo.toml", "package.json"]);
        fx.global.plugin.family_priority = vec!["rust".into(), "node".into()];

        let s = pick(&fx, &[], None);
        assert_eq!(
            s.family,
            Family::RUST,
            "改一个数组就能改默认行为 —— 这正是它不该被硬编码的理由"
        );
    }

    #[test]
    fn scenario_both_locked_at_one_ten_follows_family_priority() {
        let fx = fixture(
            &official(),
            &["Cargo.toml", "Cargo.lock", "package.json", "pnpm-lock.yaml"],
        );
        let s = pick(&fx, &[], None);
        assert_eq!(s.family, Family::NODE);
        assert_eq!(s.name, "pnpm");
    }

    // ---- pin 地板 ----------------------------------------------------------

    /// 库 crate 误判的出口：pin 50 > 10。
    #[test]
    fn scenario_pin_rescues_a_lockless_library_crate() {
        let fx = fixture(&official(), &["Cargo.toml", "package.json"]);

        assert_eq!(pick(&fx, &[], None).family, Family::NODE);

        let s = pick(&fx, &[("rust", "cargo")], None);
        assert_eq!(s.family, Family::RUST);
        assert_eq!(s.name, "cargo");
    }

    /// 但 pin **压不过锁文件** —— 真有锁文件时它不越权。
    #[test]
    fn a_pin_does_not_beat_a_lockfile() {
        let fx = fixture(&official(), &["Cargo.toml", "Cargo.lock", "package.json"]);
        let s = pick(&fx, &[("node", "pnpm")], None);

        assert_eq!(s.family, Family::RUST, "110 分压过 50 分的地板");
        assert_eq!(s.score, 110);
    }

    #[test]
    fn a_pin_works_even_with_no_files_at_all() {
        let fx = fixture(&official(), &[]);
        let s = pick(&fx, &[("node", "pnpm")], None);
        assert_eq!(s.family, Family::NODE);
    }

    #[test]
    fn pin_floor_is_fifty() {
        let fx = fixture(&official(), &[]);
        let families = score_all(&fx.set, &fx.project, &merged(&[("node", "pnpm")]));
        let node = families.get(&Family::NODE).unwrap();

        assert_eq!(node.score, PIN_FLOOR);
        assert!(node.pinned);
        // 地板由文件顶部那两条编译期断言钉住
    }

    // ---- 第 0 层：-p -------------------------------------------------------

    #[test]
    fn explicit_plugin_overrides_the_pin() {
        let fx = fixture(&official(), &["Cargo.toml", "Cargo.lock"]);
        // 项目明明是 rust，但 -p 说了算
        let s = pick(&fx, &[], Some("npm"));
        assert_eq!(s.name, "npm");
        assert_eq!(s.family, Family::NODE);
        assert!(s.notes.is_empty(), "-p 是明确的，不该有歧义提示");
    }

    #[test]
    fn explicit_plugin_overrides_detection_in_a_mixed_project() {
        let fx = fixture(&official(), &["Cargo.toml", "Cargo.lock", "package.json"]);
        assert_eq!(pick(&fx, &[], None).name, "cargo");
        assert_eq!(pick(&fx, &[], Some("yarn")).name, "yarn");
    }

    #[test]
    fn an_unknown_explicit_plugin_lists_what_is_available() {
        let fx = fixture(&official(), &["Cargo.toml"]);
        match fail(&fx, &[], Some("nope")) {
            DetectFailure::UnknownPlugin { name, available } => {
                assert_eq!(name, "nope");
                assert!(available.contains(&"cargo".to_string()));
                assert!(available.contains(&"pnpm".to_string()));
            }
            other => panic!("期望 UnknownPlugin，得到 {other:?}"),
        }
    }

    // ---- 失败路径 ----------------------------------------------------------

    #[test]
    fn no_files_and_no_pin_detects_nothing() {
        let fx = fixture(&official(), &[]);
        assert_eq!(fail(&fx, &[], None), DetectFailure::NothingDetected);
    }

    #[test]
    fn zero_plugins_detects_nothing_even_with_files_present() {
        let fx = fixture(&[], &["Cargo.toml", "package.json"]);
        assert_eq!(
            fail(&fx, &[], None),
            DetectFailure::NothingDetected,
            "零插件时检测必须一无所获 —— ECOSYSTEM_HINTS 不参与裁决"
        );
    }

    /// pin 的生态一个插件都没装 → 报"没装"，不是含糊的"检测不到"。
    #[test]
    fn a_pinned_family_with_no_plugins_names_the_plugin_it_wants() {
        let fx = fixture(&official(), &[]);
        match fail(&fx, &[("python", "poetry")], None) {
            DetectFailure::PinnedNotInstalled { family, name } => {
                assert_eq!(family, "python");
                assert_eq!(name, "poetry");
            }
            other => panic!("期望 PinnedNotInstalled，得到 {other:?}"),
        }
    }

    /// pin 的插件**装了但坏了**时，不能报成"没装" —— 下一步动作完全不同。
    #[test]
    fn pinning_a_broken_plugin_says_it_is_broken_not_missing() {
        let fx = fixture(
            &[(
                "pmpx-plugin-pnpm",
                "node",
                &["pnpm-lock.yaml"],
                &["package.json"],
            )],
            &["package.json"],
        );
        // 手工把那份 manifest 改成缺 family 的坏插件
        let bad = fx.set.by_name("pnpm").unwrap().dir.join("pmpx-plugin.toml");
        std::fs::write(
            &bad,
            "[plugin]\nname = \"pnpm\"\nversion = \"0.1.0\"\n\n[detect]\nstrong = [\"pnpm-lock.yaml\"]\n",
        )
        .unwrap();

        // 重新读一遍清单
        let cfg = crate_plugin_kit::KitConfig::new("pmpx")
            .with_data_dir(
                fx.set
                    .by_name("pnpm")
                    .unwrap()
                    .dir
                    .parent()
                    .unwrap()
                    .parent()
                    .unwrap(),
            )
            .with_lock_timeout(Duration::from_millis(500));
        let kit = CratePluginKit::<PmpxPluginV1>::new(cfg).unwrap();
        let set = PluginSet::load(&kit).unwrap();

        match select(
            &set,
            &fx.project,
            &merged(&[("node", "pnpm")]),
            &fx.global,
            None,
        ) {
            Err(DetectFailure::PluginUnusable { name, problem }) => {
                assert_eq!(name, "pnpm");
                assert!(problem.contains("family"), "{problem}");
            }
            other => panic!("期望 PluginUnusable，得到 {other:?}"),
        }
    }

    #[test]
    fn pinning_a_plugin_that_is_not_installed_is_reported() {
        // 只装 pnpm，却 pin 成 npm
        let fx = fixture(
            &[(
                "pmpx-plugin-pnpm",
                "node",
                &["pnpm-lock.yaml"],
                &["package.json"],
            )],
            &["package.json"],
        );
        match fail(&fx, &[("node", "npm")], None) {
            DetectFailure::PinnedNotInstalled { family, name } => {
                assert_eq!(family, "node");
                assert_eq!(name, "npm");
            }
            other => panic!("期望 PinnedNotInstalled，得到 {other:?}"),
        }
    }

    // ---- 排序的确定性 ------------------------------------------------------

    /// 两个都没列进 `family_priority` 的生态同分时，靠名字典序拿到确定的次序。
    #[test]
    fn unlisted_families_fall_back_to_name_order() {
        let fx = fixture(
            &[
                ("pmpx-plugin-zed", "zebra", &["z.lock"], &[]),
                ("pmpx-plugin-aaa", "alpha", &["a.lock"], &[]),
            ],
            &["a.lock", "z.lock"],
        );

        let s = pick(&fx, &[], None);
        assert_eq!(s.family.as_str(), "alpha", "未列出的按名字典序");
        assert!(!s.notes.is_empty(), "同分仍要提示");
    }

    #[test]
    fn ranking_puts_listed_before_unlisted() {
        let order = vec!["node".to_string(), "rust".to_string()];
        assert_eq!(rank(&order, "node"), 0);
        assert_eq!(rank(&order, "rust"), 1);
        assert_eq!(rank(&order, "python"), 2, "未列出的排最后");
    }

    #[test]
    fn an_unlisted_plugin_loses_to_a_listed_one_at_the_same_score() {
        let fx = fixture(
            &[
                ("pmpx-plugin-aaa", "node", &[], &["package.json"]),
                ("pmpx-plugin-pnpm", "node", &[], &["package.json"]),
            ],
            &["package.json"],
        );

        let s = pick(&fx, &[], None);
        assert_eq!(s.name, "pnpm", "priority 表里的赢过不在表里的");
    }

    /// 每个失败原因都得给出可执行的下一步。
    #[test]
    fn every_failure_message_is_actionable() {
        let cases = [
            DetectFailure::NothingDetected,
            DetectFailure::FamilyWithoutPlugin {
                family: "python".into(),
                score: 50,
            },
            DetectFailure::PinnedNotInstalled {
                family: "node".into(),
                name: "bun".into(),
            },
            DetectFailure::PluginUnusable {
                name: "pnpm".into(),
                problem: "manifest 没有声明 family".into(),
            },
            DetectFailure::UnknownPlugin {
                name: "x".into(),
                available: vec!["pnpm".into()],
            },
        ];

        for c in cases {
            let msg = c.message();
            assert!(!msg.is_empty());
            assert!(
                msg.contains("pmpx plugin")
                    || msg.contains(".pmpx.toml")
                    || msg.contains("没装")
                    || msg.contains("装上")
                    || msg.contains("已装"),
                "这条消息没告诉用户下一步做什么：{msg}"
            );
        }
    }

    // ---- 其它 --------------------------------------------------------------

    #[test]
    fn score_all_includes_zero_score_plugins_for_display() {
        let fx = fixture(&official(), &["Cargo.toml"]);
        let families = score_all(&fx.set, &fx.project, &merged(&[]));

        let node = families.get(&Family::NODE).unwrap();
        assert_eq!(node.score, 0, "node 这边一分没有");
        assert_eq!(node.plugins.len(), 4, "但四个插件仍要列出来给 info 看");
    }

    #[test]
    fn selection_reports_the_winning_score() {
        let fx = fixture(&official(), &["package.json", "pnpm-lock.yaml"]);
        assert_eq!(pick(&fx, &[], None).score, 110);
    }
}
