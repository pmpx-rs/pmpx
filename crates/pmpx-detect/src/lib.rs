//! Which plugin does this project belong to?
//!
//! Two steps, and both of them work on **data**:
//!
//! 1. **Scoring** ([`score_all`]) -- how many of each plugin's detect files are present (strong
//!    evidence 100 points, weak evidence 10 points);
//! 2. **Resolution** ([`select`]) -- pick a family first, then a plugin inside that family.
//!
//! Without a lockfile the family is undecidable: library crates often gitignore `Cargo.lock`, and then
//! `Cargo.toml` (10) and `package.json` (10) tie, so `family_priority` resolves them to node. The
//! escape hatches are pinning in `.pmpx.toml` (floor 50) or reordering `family_priority`.
//!
//! # Why there is no filesystem here
//!
//! A decision that reads the filesystem can only be tested against one, which means every test needs a
//! directory shaped like a project, and the interesting cases ("this family is pinned but the plugin
//! is not installed", "two families tie") get tangled up with disk state. So the caller passes:
//!
//! - the installed plugins, as [`Candidate`] data,
//! - the project's files through a [`Presence`] -- one call, one answer,
//! - the pins and the ordering tables, as plain values.
//!
//! The host wraps a real directory in a `Presence`; a test wraps a literal set of names. The decision
//! cannot tell the difference, which is the point.
//!
//! This crate never prints and never exits: every outcome is a value, and [`DetectFailure::message`] is
//! a string the caller decides what to do with.

use std::collections::{BTreeMap, BTreeSet};

mod failure;

pub use failure::DetectFailure;

/// Points for matching one piece of **strong evidence**.
pub const STRONG_SCORE: u32 = 100;
/// Points for matching one piece of **weak evidence**; it can never outweigh a lockfile (100 points),
/// so plugin authors need not agonize over which bucket a file belongs to.
pub const WEAK_SCORE: u32 = 10;
/// **Score floor** for a family pinned in `.pmpx.toml`.
///
/// Stronger than a 10-point manifest file (it rescues a lockless library crate) and weaker than a
/// 100-point lockfile (it does not overstep when a real lockfile exists).
pub const PIN_FLOOR: u32 = 50;

// The floor must sit between weak and strong evidence.
const _: () = assert!(PIN_FLOOR > WEAK_SCORE);
const _: () = assert!(PIN_FLOOR < STRONG_SCORE);

/// Whether one file, named relative to the project root, is there.
///
/// The whole filesystem surface of this crate. `BTreeSet<String>` answers for a directory that was
/// listed, and a closure answers for anything else:
///
/// ```
/// use pmpx_detect::Presence;
///
/// let listed: std::collections::BTreeSet<String> = ["package.json".to_string()].into();
/// assert!(listed.has("package.json"));
/// assert!(!listed.has("Cargo.toml"));
///
/// let from_disk = |name: &str| std::path::Path::new("/tmp/x").join(name).exists();
/// let _ = from_disk.has("package.json");
/// ```
pub trait Presence {
    /// Is this file there?
    fn has(&self, relative: &str) -> bool;
}

impl Presence for BTreeSet<String> {
    fn has(&self, relative: &str) -> bool {
        self.contains(relative)
    }
}

impl<F> Presence for F
where
    F: Fn(&str) -> bool,
{
    fn has(&self, relative: &str) -> bool {
        self(relative)
    }
}

/// One installed plugin, as this crate sees it: pure data, no store, no directory, no version.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Candidate {
    /// Full crate name (`pmpx-plugin-pnpm`) -- what the caller loads.
    pub crate_name: String,
    /// The name the plugin reports (`pnpm`) -- what a pin and `-p` say.
    pub name: String,
    /// The family it belongs to. Empty means the manifest did not declare one, so it cannot take part
    /// in resolution at all.
    pub family: String,
    /// Strong evidence (100 points each): proves this backend has really been used.
    pub strong: Vec<String>,
    /// Weak evidence (10 points each): only proves the project belongs to this family.
    pub weak: Vec<String>,
    /// Why it cannot take part, if it cannot (`pmpx plugin ls` shows the same sentence). `None` means
    /// it is usable.
    pub problem: Option<String>,
}

impl Candidate {
    /// Can this plugin take part in resolution at all?
    pub fn is_usable(&self) -> bool {
        self.problem.is_none() && !self.family.is_empty()
    }
}

/// The project's `[plugin]` pins: family → plugin name.
pub type Pins = BTreeMap<String, String>;

/// The orderings the user asked for, as data.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preferences {
    /// Family order, used to break a tie between families.
    pub family_priority: Vec<String>,
    /// Plugin order inside a family, used to break a tie between plugins.
    pub priority: Vec<String>,
}

/// One plugin's score breakdown in the project.
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
    /// Score one plugin against the project.
    pub fn score(plugin: &Candidate, presence: &dyn Presence) -> Self {
        let strong_hits = hits(presence, &plugin.strong);
        let weak_hits = hits(presence, &plugin.weak);

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
    /// Family name.
    pub family: String,
    /// `max(highest score among its plugins, pin ? 50 : 0)`
    pub score: u32,
    /// Whether `.pmpx.toml` pins this family.
    pub pinned: bool,
    /// Installed plugins under it (including 0-point ones -- `info` must show them).
    pub plugins: Vec<ScoredPlugin>,
}

/// The resolution result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// Crate name of the winning plugin (used to locate its directory when loading).
    pub crate_name: String,
    /// The winning plugin's self-reported name.
    pub name: String,
    /// The winning family.
    pub family: String,
    /// The winning plugin's score (for `info`).
    pub score: u32,

    /// How this plugin got picked. The contract has the same three reasons plus an "unknown" arm; the
    /// caller maps one to the other, because the wire form belongs to the contract and this form
    /// belongs to the decision.
    pub reason: Reason,
    /// The files matching the winning plugin, relative to the project root -- exactly what is handed
    /// to it as the contract's `matched`.
    ///
    /// It is carried out of the resolution rather than recomputed by the caller so that **the evidence
    /// that chose a plugin and the evidence the plugin is given are the same value**, produced by the
    /// same scoring pass.
    pub matched: Vec<String>,
    /// Notes shown to the user; `--quiet` turns them off.
    pub notes: Vec<String>,
}

/// How a plugin got picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// It won on evidence: the highest score in the winning family.
    Scored,
    /// `.pmpx.toml` pins its family to it.
    Pinned,
    /// The user named it with `-p/--plugin`.
    Explicit,
}

/// Score every candidate, then aggregate by family.
///
/// The returned map **also contains** families that are "pinned but have no plugin installed"
/// (score = floor) -- such a case must be selectable, so that the caller can report "no plugin can
/// handle this family" instead of a vague "no project type detected".
pub fn score_all(
    candidates: &[Candidate],
    presence: &dyn Presence,
    pins: &Pins,
) -> BTreeMap<String, FamilyScore> {
    let mut families: BTreeMap<String, FamilyScore> = BTreeMap::new();

    for plugin in candidates.iter().filter(|c| c.is_usable()) {
        let scored = ScoredPlugin::score(plugin, presence);
        let entry = families
            .entry(plugin.family.clone())
            .or_insert_with(|| FamilyScore {
                family: plugin.family.clone(),
                score: 0,
                pinned: false,
                plugins: Vec::new(),
            });

        // Family score = the **highest** score among its plugins, not the sum -- four Node backends
        // each matching `package.json` is just the same weak evidence counted four times.
        entry.score = entry.score.max(scored.score);
        entry.plugins.push(scored);
    }

    // The pin floor is applied after plugin scores are computed, hence max and not an overwrite.
    for (family, fs) in families.iter_mut() {
        if pins.contains_key(family) {
            fs.pinned = true;
            fs.score = fs.score.max(PIN_FLOOR);
        }
    }

    // Families that are pinned but have no plugin installed must be present too.
    for name in pins.keys() {
        families.entry(name.clone()).or_insert_with(|| FamilyScore {
            family: name.clone(),
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

/// The full resolution: score, then decide.
///
/// `explicit` is the value of `-p/--plugin`: **it overrides everything, including `.pmpx.toml`** --
/// `-p` is a one-off temporary override and `.pmpx.toml` is a durable pin, so the temporary one should
/// win.
pub fn select(
    candidates: &[Candidate],
    presence: &dyn Presence,
    pins: &Pins,
    preferences: &Preferences,
    explicit: Option<&str>,
) -> Result<Selection, DetectFailure> {
    // Layer 0: `-p` names it directly, so the project does not need to be scored at all.
    if explicit.is_some() {
        return select_from_scores(
            candidates,
            presence,
            pins,
            preferences,
            explicit,
            &BTreeMap::new(),
        );
    }

    let families = score_all(candidates, presence, pins);
    select_from_scores(candidates, presence, pins, preferences, explicit, &families)
}

/// The decision layer: pick a family, then a plugin inside it, from **scores that were already
/// computed**.
///
/// Scoring is separated from deciding so that one run scores the project once: [`select`] does both in
/// one call, while a caller that has already scored (`pmpx info`, `plugin current`) hands its own map
/// in rather than walking the same files again.
///
/// `families` is only consulted when `explicit` is `None`, so a caller going straight to `-p` may pass
/// an empty map. The `-p` path still scores **the one named plugin**, because [`Selection::matched`]
/// has to be filled there too.
pub fn select_from_scores(
    candidates: &[Candidate],
    presence: &dyn Presence,
    pins: &Pins,
    preferences: &Preferences,
    explicit: Option<&str>,
    families: &BTreeMap<String, FamilyScore>,
) -> Result<Selection, DetectFailure> {
    // Layer 0: -p names it directly. Every later resolution step is skipped.
    if let Some(name) = explicit {
        let available = || {
            candidates
                .iter()
                .filter(|c| c.is_usable())
                .map(|c| c.name.clone())
                .collect::<Vec<_>>()
        };

        let Some(plugin) = candidates.iter().find(|c| c.name == name) else {
            return Err(DetectFailure::UnknownPlugin {
                name: name.to_string(),
                available: available(),
            });
        };
        if !plugin.is_usable() {
            return Err(DetectFailure::UnknownPlugin {
                name: name.to_string(),
                available: available(),
            });
        }

        return Ok(Selection {
            crate_name: plugin.crate_name.clone(),
            name: plugin.name.clone(),
            family: plugin.family.clone(),
            score: 0,
            reason: Reason::Explicit,
            matched: matched_files(&ScoredPlugin::score(plugin, presence)),
            // `-p` is what the user explicitly asked for; there is no ambiguity to flag
            notes: Vec::new(),
        });
    }

    // Layer 1: pick the family
    let mut ranked_families: Vec<&FamilyScore> =
        families.values().filter(|f| f.score > 0).collect();

    if ranked_families.is_empty() {
        return Err(DetectFailure::NothingDetected);
    }

    // Sort keys: score descending → family_priority rank ascending → name lexicographic. The last one
    // gives unlisted families a deterministic order too.
    ranked_families.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| {
                rank(&preferences.family_priority, &a.family)
                    .cmp(&rank(&preferences.family_priority, &b.family))
            })
            .then_with(|| a.family.cmp(&b.family))
    });

    let winner = ranked_families[0];
    let mut notes = Vec::new();

    // A tie is reported -- not because the result is uncertain, but because the user may want the
    // other one.
    if ranked_families
        .get(1)
        .is_some_and(|r| r.score == winner.score)
    {
        let tied: Vec<&str> = ranked_families
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

    // 2-a: `.pmpx.toml`'s `[plugin] <family> = "<name>"`
    if let Some(pinned) = pins.get(&winner.family) {
        if let Some(p) = winner.plugins.iter().find(|p| &p.name == pinned) {
            return Ok(Selection {
                crate_name: p.crate_name.clone(),
                name: p.name.clone(),
                family: winner.family.clone(),
                score: p.score,
                reason: Reason::Pinned,
                matched: matched_files(p),
                notes,
            });
        }

        // Not in this family's usable list -- there are two very different reasons, which must be
        // reported separately.
        if let Some(existing) = candidates.iter().find(|c| &c.name == pinned) {
            // It is installed but unusable itself (missing family / empty detect section), or the
            // family it declares does not match the line written in `.pmpx.toml`.
            return Err(DetectFailure::PluginUnusable {
                name: pinned.to_string(),
                problem: existing.problem.clone().unwrap_or_else(|| {
                    "its declared family does not match the one written in .pmpx.toml".to_string()
                }),
            });
        }

        return Err(DetectFailure::PinnedNotInstalled {
            family: winner.family.clone(),
            name: pinned.to_string(),
        });
    }

    // 2-b: the highest score. 0-point entries do not take part.
    let mut ranked: Vec<&ScoredPlugin> = winner.plugins.iter().filter(|p| p.score > 0).collect();

    if ranked.is_empty() {
        return Err(DetectFailure::FamilyWithoutPlugin {
            family: winner.family.clone(),
            score: winner.score,
        });
    }

    ranked.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| {
                rank(&preferences.priority, &a.name).cmp(&rank(&preferences.priority, &b.name))
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
        reason: Reason::Scored,
        matched: matched_files(best),
        notes,
    })
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

fn hits(presence: &dyn Presence, names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = names
        .iter()
        .filter(|n| presence.has(n.as_str()))
        .cloned()
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(
        crate_name: &str,
        name: &str,
        family: &str,
        strong: &[&str],
        weak: &[&str],
    ) -> Candidate {
        Candidate {
            crate_name: crate_name.to_string(),
            name: name.to_string(),
            family: family.to_string(),
            strong: strong.iter().map(|s| s.to_string()).collect(),
            weak: weak.iter().map(|s| s.to_string()).collect(),
            problem: None,
        }
    }

    fn project(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn ranking_puts_listed_before_unlisted() {
        let order = vec!["node".to_string(), "rust".to_string()];
        assert_eq!(rank(&order, "node"), 0);
        assert_eq!(rank(&order, "rust"), 1);
        assert_eq!(rank(&order, "python"), 2, "unlisted ones rank last");
    }

    /// Strong evidence is worth ten weak ones, and the family score is the best plugin in it -- not
    /// the sum, because four Node backends matching `package.json` is one piece of evidence.
    #[test]
    fn a_lockfile_outweighs_a_manifest() {
        let node = candidate(
            "pmpx-plugin-pnpm",
            "pnpm",
            "node",
            &["pnpm-lock.yaml"],
            &["package.json"],
        );
        let rust = candidate("pmpx-plugin-cargo", "cargo", "rust", &[], &["Cargo.toml"]);
        let presence = project(&["pnpm-lock.yaml", "package.json", "Cargo.toml"]);

        let families = score_all(&[node, rust], &presence, &Pins::new());

        assert_eq!(families["node"].score, STRONG_SCORE + WEAK_SCORE);
        assert_eq!(families["rust"].score, WEAK_SCORE);
    }

    #[test]
    fn two_plugins_of_one_family_do_not_add_up() {
        let a = candidate("pmpx-plugin-a", "a", "node", &[], &["package.json"]);
        let b = candidate("pmpx-plugin-b", "b", "node", &[], &["package.json"]);
        let presence = project(&["package.json"]);

        let families = score_all(&[a, b], &presence, &Pins::new());

        assert_eq!(families["node"].score, WEAK_SCORE, "max, not sum");
        assert_eq!(families["node"].plugins.len(), 2, "both are still listed");
    }

    /// A pin lifts its family over a mere manifest file, which is how a library crate with a
    /// gitignored lockfile is rescued.
    #[test]
    fn a_pin_rescues_a_family_whose_only_evidence_is_weak() {
        let node = candidate("pmpx-plugin-pnpm", "pnpm", "node", &[], &["package.json"]);
        let rust = candidate("pmpx-plugin-cargo", "cargo", "rust", &[], &["Cargo.toml"]);
        let presence = project(&["package.json", "Cargo.toml"]);
        let mut pins = Pins::new();
        pins.insert("rust".to_string(), "cargo".to_string());

        let selection = select(
            &[node, rust],
            &presence,
            &pins,
            &Preferences::default(),
            None,
        )
        .expect("the pin decides");

        assert_eq!(selection.name, "cargo");
        assert_eq!(selection.reason, Reason::Pinned);
    }

    /// A pin is not allowed to overstep a real lockfile.
    #[test]
    fn a_pin_does_not_beat_a_lockfile() {
        let node = candidate("pmpx-plugin-pnpm", "pnpm", "node", &["pnpm-lock.yaml"], &[]);
        let rust = candidate("pmpx-plugin-cargo", "cargo", "rust", &[], &["Cargo.toml"]);
        let presence = project(&["pnpm-lock.yaml", "Cargo.toml"]);
        let mut pins = Pins::new();
        pins.insert("rust".to_string(), "cargo".to_string());

        let selection = select(
            &[node, rust],
            &presence,
            &pins,
            &Preferences::default(),
            None,
        )
        .expect("node still wins");

        assert_eq!(selection.name, "pnpm");
        assert_eq!(selection.reason, Reason::Scored);
    }

    /// `-p` overrides a pin, and the evidence still travels with the answer.
    #[test]
    fn an_explicit_name_overrides_a_pin() {
        let node = candidate("pmpx-plugin-pnpm", "pnpm", "node", &["pnpm-lock.yaml"], &[]);
        let rust = candidate("pmpx-plugin-cargo", "cargo", "rust", &["Cargo.lock"], &[]);
        let presence = project(&["pnpm-lock.yaml", "Cargo.lock", "package.json"]);
        let mut pins = Pins::new();
        pins.insert("node".to_string(), "pnpm".to_string());

        let selection = select(
            &[node, rust],
            &presence,
            &pins,
            &Preferences::default(),
            Some("cargo"),
        )
        .expect("-p decides");

        assert_eq!(selection.name, "cargo");
        assert_eq!(selection.reason, Reason::Explicit);
        assert_eq!(
            selection.matched,
            vec!["Cargo.lock".to_string()],
            "the named plugin is still scored, because its evidence is handed to it"
        );
        assert_eq!(selection.score, 0, "`-p` needed no evidence to decide");
    }

    /// A tie is resolved by the user's ordering, and said out loud.
    #[test]
    fn a_tie_is_broken_by_priority_and_reported() {
        let node = candidate("pmpx-plugin-pnpm", "pnpm", "node", &[], &["package.json"]);
        let rust = candidate("pmpx-plugin-cargo", "cargo", "rust", &[], &["Cargo.toml"]);
        let presence = project(&["package.json", "Cargo.toml"]);

        let without = select(
            &[node.clone(), rust.clone()],
            &presence,
            &Pins::new(),
            &Preferences::default(),
            None,
        )
        .expect("something wins");
        assert_eq!(
            without.family, "node",
            "alphabetical, with no ordering given"
        );
        assert_eq!(without.notes.len(), 1, "the tie is reported");

        let preferences = Preferences {
            family_priority: vec!["rust".to_string()],
            priority: Vec::new(),
        };
        let with = select(&[node, rust], &presence, &Pins::new(), &preferences, None)
            .expect("something wins");

        assert_eq!(with.family, "rust", "the user's order decides the tie");
    }

    /// A family that is pinned but has no plugin installed is still a family: the failure says which
    /// plugin to install, not "nothing detected".
    #[test]
    fn a_pin_for_an_uninstalled_plugin_says_so() {
        let node = candidate("pmpx-plugin-pnpm", "pnpm", "node", &[], &["package.json"]);
        let presence = project(&["package.json"]);
        let mut pins = Pins::new();
        pins.insert("rust".to_string(), "cargo".to_string());

        let failure = select(&[node], &presence, &pins, &Preferences::default(), None)
            .expect_err("nothing can answer for rust");

        // The pin floor (50) beats the weak match (10), so the pinned family wins and then has no
        // plugin -- which is exactly the case this message exists for.
        assert_eq!(
            failure,
            DetectFailure::PinnedNotInstalled {
                family: "rust".to_string(),
                name: "cargo".to_string(),
            }
        );
    }

    /// A pin whose plugin is installed but broken reports the plugin's own problem, not "not
    /// installed".
    #[test]
    fn a_pin_for_a_broken_plugin_reports_its_problem() {
        let broken = Candidate {
            crate_name: "pmpx-plugin-broken".to_string(),
            name: "broken".to_string(),
            family: "rust".to_string(),
            strong: Vec::new(),
            weak: Vec::new(),
            problem: Some("the manifest's [detect] section is empty".to_string()),
        };
        let presence = project(&[]);
        let mut pins = Pins::new();
        pins.insert("rust".to_string(), "broken".to_string());

        let failure = select(&[broken], &presence, &pins, &Preferences::default(), None)
            .expect_err("a broken plugin cannot answer");

        assert!(
            matches!(
                &failure,
                DetectFailure::PluginUnusable { name, problem }
                    if name == "broken" && problem.contains("[detect]")
            ),
            "{failure:?}"
        );
    }

    /// Nothing to go on at all.
    #[test]
    fn an_empty_project_detects_nothing() {
        let node = candidate("pmpx-plugin-pnpm", "pnpm", "node", &[], &["package.json"]);

        let failure = select(
            &[node],
            &project(&[]),
            &Pins::new(),
            &Preferences::default(),
            None,
        )
        .expect_err("no evidence, no answer");

        assert_eq!(failure, DetectFailure::NothingDetected);
        assert!(failure.message().contains("pmpx plugin add"));
    }

    /// `-p` with a name nobody installed lists what is there.
    #[test]
    fn an_unknown_explicit_name_lists_the_alternatives() {
        let node = candidate("pmpx-plugin-pnpm", "pnpm", "node", &[], &["package.json"]);

        let failure = select(
            &[node],
            &project(&["package.json"]),
            &Pins::new(),
            &Preferences::default(),
            Some("nope"),
        )
        .expect_err("nobody by that name");

        assert_eq!(
            failure,
            DetectFailure::UnknownPlugin {
                name: "nope".to_string(),
                available: vec!["pnpm".to_string()],
            }
        );
        assert!(failure.message().contains("pnpm"), "{}", failure.message());
    }

    /// A plugin inside the winning family is picked by the plugin ordering, and a tie is reported.
    #[test]
    fn a_plugin_tie_inside_a_family_is_broken_by_priority() {
        let a = candidate("pmpx-plugin-a", "a", "node", &[], &["package.json"]);
        let b = candidate("pmpx-plugin-b", "b", "node", &[], &["package.json"]);
        let presence = project(&["package.json"]);

        let without = select(
            &[a.clone(), b.clone()],
            &presence,
            &Pins::new(),
            &Preferences::default(),
            None,
        )
        .expect("one of them wins");
        assert_eq!(without.name, "a", "alphabetical with no ordering given");

        let preferences = Preferences {
            family_priority: Vec::new(),
            priority: vec!["b".to_string()],
        };
        let with =
            select(&[a, b], &presence, &Pins::new(), &preferences, None).expect("one of them wins");

        assert_eq!(with.name, "b");
        assert!(!with.notes.is_empty(), "the tie is reported");
    }

    /// The evidence that decided and the evidence the plugin is handed are the same value.
    #[test]
    fn the_matched_files_are_what_the_winner_scored() {
        let pnpm = candidate(
            "pmpx-plugin-pnpm",
            "pnpm",
            "node",
            &["pnpm-lock.yaml"],
            &["package.json", "pnpm-workspace.yaml"],
        );
        let presence = project(&["pnpm-lock.yaml", "pnpm-workspace.yaml"]);

        let selection = select(
            &[pnpm],
            &presence,
            &Pins::new(),
            &Preferences::default(),
            None,
        )
        .expect("pnpm wins");

        assert_eq!(
            selection.matched,
            vec![
                "pnpm-lock.yaml".to_string(),
                "pnpm-workspace.yaml".to_string()
            ],
            "strong first, then weak, each sorted"
        );
    }
}
