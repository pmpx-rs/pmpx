//! Resolution: which family and which plugin a given project layout selects.

use pmpx_plugin::Family;

use crate::detect::DetectFailure;

use super::{fail, fixture, official, pick};

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

// ---- Misc --------------------------------------------------------------

#[test]
fn selection_reports_the_winning_score() {
    let fx = fixture(&official(), &["package.json", "pnpm-lock.yaml"]);
    assert_eq!(pick(&fx, &[], None).score, 110);
}

/// The files handed to the plugin are the ones the scoring actually saw -- strong evidence
/// first, then weak. Nothing re-reads the directory afterwards.
#[test]
fn selection_carries_the_evidence_it_was_chosen_on() {
    let fx = fixture(&official(), &["Cargo.toml", "Cargo.lock"]);
    assert_eq!(
        pick(&fx, &[], None).matched,
        vec!["Cargo.lock", "Cargo.toml"]
    );
}

/// The `-p` path skips family resolution but still reports that plugin's own evidence.
#[test]
fn an_explicit_plugin_carries_its_own_evidence() {
    let fx = fixture(&official(), &["package.json", "pnpm-lock.yaml"]);
    assert_eq!(
        pick(&fx, &[], Some("pnpm")).matched,
        vec!["pnpm-lock.yaml", "package.json"]
    );
}
