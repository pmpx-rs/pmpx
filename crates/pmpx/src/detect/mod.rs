//! Detection and resolution. Two steps, in order:
//!
//! 1. **Scoring** — how many detect files each installed plugin matches in the project root
//!    (strong evidence 100 points, weak evidence 10 points);
//! 2. **Resolution** — pick a family first, then a plugin inside that family.
//!
//! Without a lockfile the family is undecidable: library crates often gitignore `Cargo.lock`, and
//! then `Cargo.toml` (10) and `package.json` (10) tie, so `family_priority` resolves them to node.
//! The escape hatches are pinning in `.pmpx.toml` (floor 50) or reordering `family_priority`.
//!
//! This file is step 1 and the constants both steps share; [`resolve`] is step 2, and [`failure`]
//! holds the reasons step 2 can fail.

use std::collections::BTreeMap;
use std::path::Path;

use pmpx_plugin::Family;

use crate::config::MergedProjectConfig;
use crate::plugins::{InstalledPlugin, PluginSet};

mod failure;
mod resolve;

pub use failure::DetectFailure;
pub use resolve::{select, select_from_scores, Selection};

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

#[cfg(test)]
mod tests;
