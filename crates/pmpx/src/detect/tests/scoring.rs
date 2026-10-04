//! Scoring: what each plugin scores, and how that aggregates into a family score.

use pmpx_plugin::Family;

use crate::detect::{score_all, ScoredPlugin, PIN_FLOOR};

use super::{fixture, merged, official};

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

#[test]
fn pin_floor_is_fifty() {
    let fx = fixture(&official(), &[]);
    let families = score_all(&fx.set, &fx.project, &merged(&[("node", "pnpm")]));
    let node = families.get(&Family::NODE).unwrap();

    assert_eq!(node.score, PIN_FLOOR);
    assert!(node.pinned);
    // The floor is nailed down by the two compile-time assertions at the top of the module
}

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
