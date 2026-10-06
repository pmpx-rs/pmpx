//! Detection, wired to this host.
//!
//! The decision itself -- scoring, family choice, plugin choice, and every way that can fail -- lives
//! in `pmpx-detect`, which takes data and returns data. This module is the wire between it and this
//! host: the installed plugins as [`Candidate`]s, the project directory as a [`Presence`](pmpx_detect::Presence), the pins and
//! orderings from the two config layers, and the reason mapped onto the contract's wire form (the
//! plugin is told why it was picked).

use std::collections::BTreeMap;
use std::path::Path;

use pmpx_detect::{Candidate, Pins, Preferences};
use pmpx_plugin::{Family, SelectionReason};

use crate::config::{GlobalConfig, MergedProjectConfig};
use crate::plugins::PluginSet;

pub use pmpx_detect::{DetectFailure, FamilyScore, Reason, ScoredPlugin, Selection};

/// A real directory, answering the decision's one question.
struct Dir<'a> {
    root: &'a Path,
}

impl pmpx_detect::Presence for Dir<'_> {
    fn has(&self, relative: &str) -> bool {
        self.root.join(relative).exists()
    }
}

/// The project, as the decision sees it.
pub fn presence(root: &Path) -> impl pmpx_detect::Presence + '_ {
    Dir { root }
}

/// Every installed plugin, as data.
///
/// The unusable ones are included and carry their problem as text: a pin or `-p` naming one of them has
/// to produce "installed but broken", not "not installed".
pub fn candidates(set: &PluginSet) -> Vec<Candidate> {
    set.plugins
        .iter()
        .map(|p| Candidate {
            crate_name: p.crate_name.clone(),
            name: p.name.clone(),
            family: p
                .family
                .as_ref()
                .map(Family::as_str)
                .unwrap_or("")
                .to_string(),
            strong: p.strong.clone(),
            weak: p.weak.clone(),
            problem: p.problem().map(str::to_string),
        })
        .collect()
}

/// The `[plugin]` pins from the merged project config.
pub fn pins(merged: &MergedProjectConfig) -> Pins {
    merged.plugin.clone()
}

/// The orderings the user configured.
pub fn preferences(global: &GlobalConfig) -> Preferences {
    Preferences {
        family_priority: global.plugin.family_priority.clone(),
        priority: global.plugin.priority.clone(),
    }
}

/// The contract's word for a decision's reason: this is what crosses to the plugin, which is why the
/// wire form belongs to the contract and the plain one to the decision.
pub fn reason_of(reason: Reason) -> SelectionReason {
    match reason {
        Reason::Scored => SelectionReason::Scored,
        Reason::Pinned => SelectionReason::Pinned,
        Reason::Explicit => SelectionReason::Explicit,
    }
}

/// Score every installed plugin against the project root, aggregated by family.
pub fn score_all(
    set: &PluginSet,
    root: &Path,
    merged: &MergedProjectConfig,
) -> BTreeMap<String, FamilyScore> {
    let t = crate::debug::now();
    let families = pmpx_detect::score_all(&candidates(set), &presence(root), &pins(merged));

    // The marker count is the size of the job: one `exists()` per plugin per declared file, and every
    // Node backend claims `package.json` again.
    crate::debug::done("detect.score", t, || {
        format!(
            "{} plugins, {} marker names",
            set.usable().count(),
            set.detect_names().len()
        )
    });

    families
}

/// Resolve which plugin the project belongs to.
pub fn select(
    set: &PluginSet,
    root: &Path,
    merged: &MergedProjectConfig,
    global: &GlobalConfig,
    explicit: Option<&str>,
) -> Result<Selection, DetectFailure> {
    // Scoring goes through this host's own wrapper even on the one-call path, so the `--debug` trace
    // names the work that is actually being done (and the marker count is the size of that work).
    // Scoring once, then deciding, is also what the old two-step shape did.
    let families = score_all(set, root, merged);

    let t = crate::debug::now();
    let result = pmpx_detect::select_from_scores(
        &candidates(set),
        &presence(root),
        &pins(merged),
        &preferences(global),
        explicit,
        &families,
    );
    trace(t, &result);
    result
}

/// Resolve from scores the caller already computed, instead of scoring the project a second time.
pub fn select_from_scores(
    set: &PluginSet,
    root: &Path,
    families: &BTreeMap<String, FamilyScore>,
    merged: &MergedProjectConfig,
    global: &GlobalConfig,
    explicit: Option<&str>,
) -> Result<Selection, DetectFailure> {
    let t = crate::debug::now();
    let result = pmpx_detect::select_from_scores(
        &candidates(set),
        &presence(root),
        &pins(merged),
        &preferences(global),
        explicit,
        families,
    );
    trace(t, &result);
    result
}

/// Nothing selected is **exit code 3**: pmpx itself is fine, the environment is missing something.
///
/// This lives here rather than in the decision crate because it is about *this* program's exit codes,
/// and the decision has no opinion about processes.
impl From<DetectFailure> for crate::error::PmpxError {
    fn from(failure: DetectFailure) -> Self {
        crate::error::PmpxError::NotFound(failure.message())
    }
}

/// Both outcomes are reported: "nothing was selected, and it took 4ms to find that out" is as much
/// part of the answer as the winner is.
fn trace(t: std::time::Instant, result: &Result<Selection, DetectFailure>) {
    crate::debug::done("detect.decide", t, || match result {
        Ok(s) => format!("{} ({}, {} pts)", s.name, s.family, s.score),
        Err(f) => format!("nothing selected: {}", f.message()),
    });
}
