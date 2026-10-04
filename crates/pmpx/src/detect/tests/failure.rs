//! The failure paths: every way resolution can come up empty-handed, and what the user is told.

use std::time::Duration;

use crate_plugin_kit::CratePluginKit;
use pmpx_plugin::abi::PmpxPluginV1;

use crate::detect::{select, DetectFailure};
use crate::plugins::PluginSet;

use super::{fail, fixture, merged, official};

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
