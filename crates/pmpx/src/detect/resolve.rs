//! Step 2 of detection: which family, and which plugin inside it.
//!
//! The decision layer works on **scores that were already computed** ([`super::score_all`]), so one
//! run scores the project once: [`select`] scores and decides in one call, while a caller that has
//! already scored (`pmpx info`, `plugin current`) hands its own map to [`select_from_scores`]
//! instead of walking the same files again.
//!
//! The layers, in order: `-p/--plugin` overrides everything, then the family is picked by score,
//! then a plugin inside it is picked by the `.pmpx.toml` pin or, failing that, by score.

use std::collections::BTreeMap;
use std::path::Path;

use pmpx_plugin::Family;

use super::failure::DetectFailure;
use super::{score_all, FamilyScore, ScoredPlugin};
use crate::config::{GlobalConfig, MergedProjectConfig};
use crate::plugins::PluginSet;

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
    /// The files matching the winning plugin, relative to the project root — exactly what is
    /// handed to it as [`pmpx_plugin::Context::matched`].
    ///
    /// It is carried out of the resolution rather than recomputed by the caller so that **the
    /// evidence that chose a plugin and the evidence the plugin is given are the same value**,
    /// produced by the same scoring pass.
    pub matched: Vec<String>,
    /// Notes shown to the user; `--quiet` turns them off.
    pub notes: Vec<String>,
}

/// Rank `name` against the `order` table. **Names not in the table rank last**.
fn rank(order: &[String], name: &str) -> usize {
    order.iter().position(|x| x == name).unwrap_or(order.len())
}

/// The files that matched, in the order a plugin sees them: strong evidence first, then weak, each
/// already sorted. This is [`Selection::matched`] and nothing else.
fn matched_files(scored: &ScoredPlugin) -> Vec<String> {
    scored.all_hits().map(str::to_string).collect()
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
    // Layer 0: `-p` names it directly, so the project does not need to be scored at all.
    if explicit.is_some() {
        return select_from_scores(set, root, &BTreeMap::new(), merged, global, explicit);
    }

    let families = score_all(set, root, merged);
    select_from_scores(set, root, &families, merged, global, explicit)
}

/// The decision layer: pick a family, then a plugin inside it, from **scores that were already
/// computed**.
///
/// Scoring is separated from deciding so that one run scores the project once: [`select`] does both
/// in one call, while a caller that has already scored (`pmpx info`, `plugin current`) hands its
/// own map in rather than walking the same files again.
///
/// `families` is only consulted when `explicit` is `None`, so a caller going straight to `-p` may
/// pass an empty map. The `-p` path still scores **the one named plugin**, because
/// [`Selection::matched`] has to be filled there too.
pub fn select_from_scores(
    set: &PluginSet,
    root: &Path,
    families: &BTreeMap<Family, FamilyScore>,
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
            matched: matched_files(&ScoredPlugin::score(plugin, root)),
            // `-p` is what the user explicitly asked for; there is no ambiguity to flag
            notes: Vec::new(),
        });
    }

    // Layer 1: pick the family
    let mut candidates: Vec<&FamilyScore> = families.values().filter(|f| f.score > 0).collect();

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

    let winner = candidates[0];
    let mut notes = Vec::new();

    // A tie is reported — not because the result is uncertain, but because the user may want the
    // other one.
    if candidates.get(1).is_some_and(|r| r.score == winner.score) {
        let tied: Vec<&str> = candidates
            .iter()
            .filter(|c| c.score == winner.score)
            .map(|c| c.family.as_str())
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
                family: winner.family.clone(),
                score: p.score,
                matched: matched_files(p),
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
        family: winner.family.clone(),
        score: best.score,
        matched: matched_files(best),
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranking_puts_listed_before_unlisted() {
        let order = vec!["node".to_string(), "rust".to_string()];
        assert_eq!(rank(&order, "node"), 0);
        assert_eq!(rank(&order, "rust"), 1);
        assert_eq!(rank(&order, "python"), 2, "unlisted ones rank last");
    }
}
