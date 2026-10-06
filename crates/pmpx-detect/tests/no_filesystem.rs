//! Detection without a filesystem.
//!
//! This is what the crate is for: a decision that reads the disk can only be tested against a disk, and
//! then every interesting case needs a directory shaped like a project. Here the project is a set of
//! names, the plugins are values, and the whole decision runs in memory -- including the part that
//! matters most, the exact list of files the winner is handed.

use std::collections::{BTreeMap, BTreeSet};

use pmpx_detect::{
    score_all, select, Candidate, DetectFailure, Preferences, Presence, Reason, PIN_FLOOR,
    WEAK_SCORE,
};

fn plugin(crate_name: &str, name: &str, family: &str, strong: &[&str], weak: &[&str]) -> Candidate {
    Candidate {
        crate_name: crate_name.to_string(),
        name: name.to_string(),
        family: family.to_string(),
        strong: strong.iter().map(|s| s.to_string()).collect(),
        weak: weak.iter().map(|s| s.to_string()).collect(),
        problem: None,
    }
}

/// A project, as a list of names.
fn project(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

/// `cargo init` in a library: no lockfile is committed, so only the manifests are evidence.
fn two_manifests() -> Vec<Candidate> {
    vec![
        plugin(
            "pmpx-plugin-pnpm",
            "pnpm",
            "node",
            &["pnpm-lock.yaml"],
            &["package.json"],
        ),
        plugin(
            "pmpx-plugin-cargo",
            "cargo",
            "rust",
            &["Cargo.lock"],
            &["Cargo.toml"],
        ),
    ]
}

#[test]
fn the_decision_runs_on_data_alone() {
    let selection = select(
        &two_manifests(),
        &project(&["package.json", "Cargo.toml"]),
        &BTreeMap::new(),
        &Preferences {
            family_priority: vec!["rust".to_string()],
            priority: Vec::new(),
        },
        None,
    )
    .expect("the user's ordering breaks the tie");

    assert_eq!(selection.name, "cargo");
    assert_eq!(selection.reason, Reason::Scored);
    assert_eq!(selection.score, WEAK_SCORE);
    assert_eq!(
        selection.matched,
        vec!["Cargo.toml".to_string()],
        "the evidence that decided is the evidence the plugin is handed"
    );
}

/// The files the decision asks about are exactly the declared ones -- nothing else is ever looked at.
#[test]
fn only_declared_marker_names_are_asked_about() {
    let asked = std::cell::RefCell::new(Vec::new());
    let presence = |name: &str| {
        asked.borrow_mut().push(name.to_string());
        false
    };

    let failure = select(
        &two_manifests(),
        &presence,
        &BTreeMap::new(),
        &Preferences::default(),
        None,
    )
    .expect_err("nothing is there");

    assert_eq!(failure, DetectFailure::NothingDetected);

    let mut asked = asked.into_inner();
    asked.sort();
    assert_eq!(
        asked,
        vec![
            "Cargo.lock".to_string(),
            "Cargo.toml".to_string(),
            "package.json".to_string(),
            "pnpm-lock.yaml".to_string(),
        ],
        "the decision asks about declared markers only"
    );
}

/// A missing plugin and a broken plugin are different failures with different next steps, and both are
/// decided here rather than by reading the store.
#[test]
fn a_pin_is_resolved_against_the_candidate_list() {
    let mut pins = BTreeMap::new();
    pins.insert("rust".to_string(), "cargo".to_string());

    // Not installed at all.
    let failure = select(
        &[plugin(
            "pmpx-plugin-pnpm",
            "pnpm",
            "node",
            &[],
            &["package.json"],
        )],
        &project(&["package.json"]),
        &pins,
        &Preferences::default(),
        None,
    )
    .expect_err("cargo is not installed");
    assert!(
        matches!(failure, DetectFailure::PinnedNotInstalled { .. }),
        "{failure:?}"
    );

    // Installed, but it cannot take part.
    let broken = Candidate {
        problem: Some("the manifest's [detect] section is empty".to_string()),
        ..plugin("pmpx-plugin-cargo", "cargo", "rust", &[], &[])
    };
    let failure = select(
        &[broken],
        &project(&[]),
        &pins,
        &Preferences::default(),
        None,
    )
    .expect_err("cargo is broken");
    assert!(
        matches!(failure, DetectFailure::PluginUnusable { .. }),
        "{failure:?}"
    );
}

/// The pin floor is the one number a user can rely on: above a manifest, below a lockfile.
#[test]
fn the_pin_floor_sits_between_weak_and_strong_evidence() {
    let mut pins = BTreeMap::new();
    pins.insert("rust".to_string(), "cargo".to_string());

    let families = score_all(
        &[plugin("pmpx-plugin-cargo", "cargo", "rust", &[], &[])],
        &project(&[]),
        &pins,
    );

    assert_eq!(families["rust"].score, PIN_FLOOR);
    assert!(families["rust"].pinned);
    assert!(families["rust"].score > WEAK_SCORE);
}

/// `Presence` is one method, so a listing is enough -- and this is what the host wraps a directory in.
#[test]
fn a_listing_answers_presence() {
    let listing = project(&["package.json"]);
    assert!(Presence::has(&listing, "package.json"));
    assert!(!Presence::has(&listing, "Cargo.toml"));
}
