//! Detection and resolution. Two steps, in order:
//!
//! 1. **Scoring** — how many detect files each installed plugin matches in the project root
//!    (strong evidence 100 points, weak evidence 10 points);
//! 2. **Resolution** — pick a family first, then a plugin inside that family.
//!
//! Without a lockfile the family is undecidable: library crates often gitignore `Cargo.lock`, and
//! then `Cargo.toml` (10) and `package.json` (10) tie, so `family_priority` resolves them to node.
//! The escape hatches are pinning in `.pmpx.toml` (floor 50) or reordering `family_priority`.

use std::collections::BTreeMap;
use std::path::Path;

use pmpx_plugin::Family;

use crate::config::{GlobalConfig, MergedProjectConfig};
use crate::plugins::{InstalledPlugin, PluginSet};

/// Points for matching one piece of **strong evidence**.
pub const STRONG_SCORE: u32 = 100;
/// Points for matching one piece of **weak evidence**; it can never outweigh a lockfile
/// (100 points), so plugin authors need not agonize over which bucket a file belongs in.
pub const WEAK_SCORE: u32 = 10;
/// **Score floor** for a family pinned in `.pmpx.toml`.
///
/// Stronger than a 10-point manifest file (it rescues a lockless library crate) and weaker than a
/// 100-point lockfile (it does not overstep when a real lockfile exists).
pub const PIN_FLOOR: u32 = 50;

// The floor must sit between weak and strong evidence.
const _: () = assert!(PIN_FLOOR > WEAK_SCORE);
const _: () = assert!(PIN_FLOOR < STRONG_SCORE);

// ---- Scoring ---------------------------------------------------------------

/// One plugin's score breakdown in one directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoredPlugin {
    /// Crate name.
    pub crate_name: String,
    /// The plugin's self-reported name.
    pub name: String,
    /// Matched strong-evidence files.
    pub strong_hits: Vec<String>,
    /// Matched weak-evidence files.
    pub weak_hits: Vec<String>,
    /// `100 × strong_hits + 10 × weak_hits`
    pub score: u32,
}

impl ScoredPlugin {
    /// Score one plugin inside `dir`.
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

    /// All matched files (strong + weak), for `info` to display.
    pub fn all_hits(&self) -> impl Iterator<Item = &str> {
        self.strong_hits
            .iter()
            .chain(self.weak_hits.iter())
            .map(String::as_str)
    }
}

/// One family's score and the plugins under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FamilyScore {
    /// Family.
    pub family: Family,
    /// `max(highest score among its plugins, pin ? 50 : 0)`
    pub score: u32,
    /// Whether `.pmpx.toml` pins this family.
    pub pinned: bool,
    /// Installed plugins under it (including 0-point ones — `info` must show them).
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

/// Score every installed plugin, then aggregate by family.
///
/// The returned map **also contains** families that are "pinned but have no plugin installed"
/// (score = floor) — such a case must be selectable, so that pmpx can report "no plugin can handle
/// this family" instead of a vague "no project type detected".
pub fn score_all(
    set: &PluginSet,
    root: &Path,
    merged: &MergedProjectConfig,
) -> BTreeMap<Family, FamilyScore> {
    let mut families: BTreeMap<Family, FamilyScore> = BTreeMap::new();

    for plugin in set.usable() {
        // `usable()` guarantees family is Some
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

        // Family score = the **highest** score among its plugins, not the sum — four Node backends
        // each matching `package.json` is just the same weak evidence counted four times.
        entry.score = entry.score.max(scored.score);
        entry.plugins.push(scored);
    }

    // The pin floor is applied after plugin scores are computed, hence max and not an overwrite.
    for (family, fs) in families.iter_mut() {
        if merged.pinned_plugin(family.as_str()).is_some() {
            fs.pinned = true;
            fs.score = fs.score.max(PIN_FLOOR);
        }
    }

    // Families that are pinned but have no plugin installed must be present too.
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

// ---- Resolution ------------------------------------------------------------

/// The resolution result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// Crate name of the winning plugin (used to locate its directory when loading).
    pub crate_name: String,
    /// The winning plugin's self-reported name.
    pub name: String,
    /// The winning family.
    pub family: Family,
    /// The winning plugin's score (for `info`).
    pub score: u32,
    /// Notes shown to the user; `--quiet` turns them off.
    pub notes: Vec<String>,
}

/// Why no selection could be made.
///
/// Every variant corresponds to one sentence that **tells the user what to do next**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectFailure {
    /// Every candidate scored 0 and nothing is pinned.
    NothingDetected,
    /// `.pmpx.toml` pins a plugin that is not installed.
    PinnedNotInstalled {
        /// Family name.
        family: String,
        /// The pinned plugin name.
        name: String,
    },
    /// The plugin pinned in `.pmpx.toml` **is** installed, but cannot take part in resolution.
    ///
    /// Kept separate from [`DetectFailure::PinnedNotInstalled`] because the next step is completely
    /// different: "not installed" means go install it, "installed but broken" means look at the
    /// reason shown in `pmpx plugin ls`.
    PluginUnusable {
        /// Plugin name.
        name: String,
        /// Why it cannot be used.
        problem: String,
    },
    /// The plugin named by `-p` does not exist (or cannot take part in resolution).
    UnknownPlugin {
        /// The name the user gave.
        name: String,
        /// The available choices, already sorted.
        available: Vec<String>,
    },
    /// The winning family has no usable plugin at all.
    ///
    /// **Defensive branch**: unreachable on the normal path — a family score either comes from a
    /// plugin (then it has one) or from a pin (then it is caught by the two variants above first).
    /// A useful sentence beats `unreachable!()`.
    FamilyWithoutPlugin {
        /// Family name.
        family: String,
        /// That family's score.
        score: u32,
    },
}

/// Rank `name` against the `order` table. **Names not in the table rank last**.
fn rank(order: &[String], name: &str) -> usize {
    order.iter().position(|x| x == name).unwrap_or(order.len())
}

/// The full resolution.
///
/// `explicit` is the value of `-p/--plugin`: **it overrides everything, including `.pmpx.toml`** —
/// `-p` is a one-off temporary override and `.pmpx.toml` is a durable pin, so the temporary one
/// should win.
pub fn select(
    set: &PluginSet,
    root: &Path,
    merged: &MergedProjectConfig,
    global: &GlobalConfig,
    explicit: Option<&str>,
) -> Result<Selection, DetectFailure> {
    // Layer 0: -p names it directly. Every later resolution step is skipped.
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
            // `-p` is what the user explicitly asked for; there is no ambiguity to flag
            notes: Vec::new(),
        });
    }

    // Layer 1: pick the family
    let families = score_all(set, root, merged);

    let mut candidates: Vec<FamilyScore> = families.into_values().filter(|f| f.score > 0).collect();

    if candidates.is_empty() {
        return Err(DetectFailure::NothingDetected);
    }

    // Sort keys: score descending → family_priority rank ascending → name lexicographic.
    // The last one gives unlisted families a deterministic order too.
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

    // A tie is reported — not because the result is uncertain, but because the user may want the
    // other one.
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
            "Multiple candidates detected ({} tied at {} points), selected {}",
            tied.join(" / "),
            winner.score,
            winner.family
        ));
    }

    // Layer 2: pick a plugin within that family
    let family_name = winner.family.as_str().to_string();

    // 2-a: `.pmpx.toml`'s `[plugin] <family> = "<name>"`
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

        // Not in this family's usable list — there are two very different reasons, which must be
        // reported separately.
        if let Some(existing) = set.by_name(pinned) {
            // It is installed but unusable itself (missing family / empty detect section), or the
            // family it declares does not match the line written in `.pmpx.toml`.
            return Err(DetectFailure::PluginUnusable {
                name: pinned.to_string(),
                problem: existing
                    .problem()
                    .unwrap_or("its declared family does not match the one written in .pmpx.toml")
                    .to_string(),
            });
        }

        return Err(DetectFailure::PinnedNotInstalled {
            family: family_name,
            name: pinned.to_string(),
        });
    }

    // 2-b: the highest score. 0-point entries do not take part.
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
            "Multiple candidates detected ({} tied at {} points), selected {}",
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
    /// The full explanation shown to the user; it must include what to do next.
    pub fn message(&self) -> String {
        match self {
            DetectFailure::NothingDetected => "Cannot detect the project type.\n\
                 pmpx decides the project type from the detect files declared by installed plugins, so:\n\
                   - with no plugin installed, no project can be detected (`pmpx plugin add <name>`)\n\
                   - you can also declare it explicitly with a .pmpx.toml in the current directory, for example:\n\
                     [plugin]\n\
                     rust = \"cargo\""
                .to_string(),
            DetectFailure::FamilyWithoutPlugin { family, score } => format!(
                "The winning family is {family} ({score} points), but no plugin belonging to it is installed.\n\
                 pmpx only knows which tool to invoke once one is installed."
            ),
            DetectFailure::PinnedNotInstalled { family, name } => format!(
                ".pmpx.toml pins {family} to {name}, but it is not installed.\n\
                 Either install it (`pmpx plugin add {name}`) or change that pin."
            ),
            DetectFailure::PluginUnusable { name, problem } => format!(
                "{name} is installed but cannot take part in resolution: {problem}.\n\
                 Use `pmpx plugin ls` to see every plugin and its own problem."
            ),
            DetectFailure::UnknownPlugin { name, available } => {
                let mut msg = format!("Plugin {name} not found.");
                if available.is_empty() {
                    msg.push_str("\nNo plugin is installed at all (`pmpx plugin add <name>`).");
                } else {
                    msg.push_str("\nInstalled: ");
                    msg.push_str(&available.join(", "));
                }
                msg
            }
        }
    }
}

/// Nothing selected = **exit code 3**: pmpx itself is fine, the environment is missing something.
/// User scripts should be able to use this code to tell "pmpx is broken" (1) from "you have not
/// installed anything yet" (3).
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

    /// Build a scenario: install several plugins and place several files in the project dir
    /// (`files` are relative to the project root).
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

    /// The detect declarations of all official plugins.
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
            .unwrap_or_else(|e| panic!("should have selected something: {}", e.message()))
    }

    fn fail(fx: &Fixture, pins: &[(&str, &str)], explicit: Option<&str>) -> DetectFailure {
        select(&fx.set, &fx.project, &merged(pins), &fx.global, explicit).unwrap_err()
    }

    // ---- Scoring -------------------------------------------------------------

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

    // ---- Typical scenario comparisons --------------------------------------

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

    /// With only a `package.json` all four backends score 10 → `priority` picks pnpm.
    #[test]
    fn scenario_package_json_alone_falls_back_to_pnpm() {
        let fx = fixture(&official(), &["package.json"]);
        let s = pick(&fx, &[], None);
        assert_eq!(s.name, "pnpm");
        assert_eq!(s.score, 10);
        assert!(!s.notes.is_empty(), "a tie must produce a note");
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

    /// Mixed project: 10 vs 10 → the default `family_priority` puts node first → node is used.
    /// This is not a hard-coded rule, it is the natural result of that array.
    #[test]
    fn scenario_mixed_at_ten_ten_goes_to_node() {
        let fx = fixture(&official(), &["Cargo.toml", "package.json"]);
        let s = pick(&fx, &[], None);
        assert_eq!(s.family, Family::NODE);
        assert_eq!(s.name, "pnpm");
    }

    /// Move rust to the front of family_priority and the same project goes to cargo.
    #[test]
    fn scenario_mixed_follows_family_priority() {
        let mut fx = fixture(&official(), &["Cargo.toml", "package.json"]);
        fx.global.plugin.family_priority = vec!["rust".into(), "node".into()];

        let s = pick(&fx, &[], None);
        assert_eq!(
            s.family,
            Family::RUST,
            "changing one array changes the default behaviour"
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

    // ---- The pin floor ------------------------------------------------------

    /// The escape hatch for a misdetected library crate: pin 50 > 10.
    #[test]
    fn scenario_pin_rescues_a_lockless_library_crate() {
        let fx = fixture(&official(), &["Cargo.toml", "package.json"]);

        assert_eq!(pick(&fx, &[], None).family, Family::NODE);

        let s = pick(&fx, &[("rust", "cargo")], None);
        assert_eq!(s.family, Family::RUST);
        assert_eq!(s.name, "cargo");
    }

    /// But a pin **does not beat a lockfile** — with a real lockfile it does not overstep.
    #[test]
    fn a_pin_does_not_beat_a_lockfile() {
        let fx = fixture(&official(), &["Cargo.toml", "Cargo.lock", "package.json"]);
        let s = pick(&fx, &[("node", "pnpm")], None);

        assert_eq!(
            s.family,
            Family::RUST,
            "110 points beats the 50-point floor"
        );
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
        // The floor is nailed down by the two compile-time assertions at the top of the file
    }

    // ---- Layer 0: -p -------------------------------------------------------

    #[test]
    fn explicit_plugin_overrides_the_pin() {
        let fx = fixture(&official(), &["Cargo.toml", "Cargo.lock"]);
        // The project is clearly rust, but -p has the final say
        let s = pick(&fx, &[], Some("npm"));
        assert_eq!(s.name, "npm");
        assert_eq!(s.family, Family::NODE);
        assert!(
            s.notes.is_empty(),
            "-p is explicit, there should be no ambiguity note"
        );
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
            other => panic!("expected UnknownPlugin, got {other:?}"),
        }
    }

    // ---- Failure paths ------------------------------------------------------

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
            "with zero plugins detection must find nothing — ECOSYSTEM_HINTS takes no part in resolution"
        );
    }

    /// A pinned family with no plugin installed → reports "not installed", not a vague
    /// "not detected".
    #[test]
    fn a_pinned_family_with_no_plugins_names_the_plugin_it_wants() {
        let fx = fixture(&official(), &[]);
        match fail(&fx, &[("python", "poetry")], None) {
            DetectFailure::PinnedNotInstalled { family, name } => {
                assert_eq!(family, "python");
                assert_eq!(name, "poetry");
            }
            other => panic!("expected PinnedNotInstalled, got {other:?}"),
        }
    }

    /// When the pinned plugin **is installed but broken**, it must not be reported as "not
    /// installed" — the next step is completely different.
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
        // Hand-edit that manifest into a broken plugin missing family
        let bad = fx.set.by_name("pnpm").unwrap().dir.join("pmpx-plugin.toml");
        std::fs::write(
            &bad,
            "[plugin]\nname = \"pnpm\"\nversion = \"0.1.0\"\n\n[detect]\nstrong = [\"pnpm-lock.yaml\"]\n",
        )
        .unwrap();

        // Read the manifest again
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
            other => panic!("expected PluginUnusable, got {other:?}"),
        }
    }

    #[test]
    fn pinning_a_plugin_that_is_not_installed_is_reported() {
        // Only pnpm is installed, but it is pinned to npm
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
            other => panic!("expected PinnedNotInstalled, got {other:?}"),
        }
    }

    // ---- Determinism of ordering --------------------------------------------

    /// When two families absent from `family_priority` tie, name order gives a deterministic
    /// result.
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
        assert_eq!(
            s.family.as_str(),
            "alpha",
            "unlisted ones follow name order"
        );
        assert!(!s.notes.is_empty(), "a tie still produces a note");
    }

    #[test]
    fn ranking_puts_listed_before_unlisted() {
        let order = vec!["node".to_string(), "rust".to_string()];
        assert_eq!(rank(&order, "node"), 0);
        assert_eq!(rank(&order, "rust"), 1);
        assert_eq!(rank(&order, "python"), 2, "unlisted ones rank last");
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
        assert_eq!(
            s.name, "pnpm",
            "the one in the priority table beats the one that is not"
        );
    }

    /// Every failure reason must give an actionable next step.
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
                problem: "manifest does not declare a family".into(),
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
                    || msg.to_lowercase().contains("install"),
                "this message does not tell the user what to do next: {msg}"
            );
        }
    }

    // ---- Misc --------------------------------------------------------------

    #[test]
    fn score_all_includes_zero_score_plugins_for_display() {
        let fx = fixture(&official(), &["Cargo.toml"]);
        let families = score_all(&fx.set, &fx.project, &merged(&[]));

        let node = families.get(&Family::NODE).unwrap();
        assert_eq!(node.score, 0, "node has no points at all");
        assert_eq!(
            node.plugins.len(),
            4,
            "but all four plugins are still listed for info"
        );
    }

    #[test]
    fn selection_reports_the_winning_score() {
        let fx = fixture(&official(), &["package.json", "pnpm-lock.yaml"]);
        assert_eq!(pick(&fx, &[], None).score, 110);
    }
}
