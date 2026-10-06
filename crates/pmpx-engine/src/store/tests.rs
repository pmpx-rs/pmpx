//! Tests for reading manifests and for the installed-plugin set those manifests add up to.

use std::path::PathBuf;
use std::time::Duration;

use super::*;

const PNPM: &str = r#"
[plugin]
name    = "pnpm"
version = "0.1.0"
abi     = 1
family  = "node"

[detect]
strong = ["pnpm-lock.yaml", "pnpm-workspace.yaml"]
weak   = ["package.json"]
"#;

const CARGO: &str = r#"
[plugin]
name    = "cargo"
version = "0.2.0"
abi     = 1
family  = "rust"

[detect]
strong = ["Cargo.lock"]
weak   = ["Cargo.toml"]
"#;

/// Build a plugin store, returning (keep-alive, kit, data dir).
fn store(entries: &[(&str, &str)]) -> (tempfile::TempDir, CratePluginKit<PmpxPlugin>, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("store");

    for (crate_name, manifest) in entries {
        let dir = root.join("plugins").join(crate_name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pmpx-plugin.toml"), manifest).unwrap();
    }

    let cfg = crate_plugin_kit::KitConfig::new("pmpx")
        .with_data_dir(&root)
        .with_lock_timeout(Duration::from_millis(500));
    let kit = CratePluginKit::<PmpxPlugin>::new(cfg).unwrap();

    (tmp, kit, root)
}

#[test]
fn an_empty_store_lists_nothing() {
    let (_t, kit, _) = store(&[]);
    let set = PluginSet::load(&kit).unwrap();
    assert!(set.plugins.is_empty());
    assert!(set.detect_names().is_empty());
}

#[test]
fn reads_names_family_and_detect_patterns() {
    let (_t, kit, _) = store(&[("pmpx-plugin-pnpm", PNPM)]);
    let set = PluginSet::load(&kit).unwrap();

    assert_eq!(set.plugins.len(), 1);
    let p = &set.plugins[0];
    assert_eq!(p.name(), "pnpm");
    assert_eq!(p.crate_name(), "pmpx-plugin-pnpm");
    assert_eq!(p.version(), "0.1.0");
    assert_eq!(p.family.as_ref().map(Family::as_str), Some("node"));
    assert_eq!(p.abi(), Some(1));
    assert_eq!(p.strong, vec!["pnpm-lock.yaml", "pnpm-workspace.yaml"]);
    assert_eq!(p.weak, vec!["package.json"]);
    assert!(p.is_usable());
}

#[test]
fn results_are_sorted_by_crate_name() {
    let (_t, kit, _) = store(&[("pmpx-plugin-pnpm", PNPM), ("pmpx-plugin-cargo", CARGO)]);
    let set = PluginSet::load(&kit).unwrap();

    let names: Vec<_> = set.plugins.iter().map(|p| p.crate_name()).collect();
    assert_eq!(names, vec!["pmpx-plugin-cargo", "pmpx-plugin-pnpm"]);
}

#[test]
fn detect_names_are_deduped_across_plugins() {
    let (_t, kit, _) = store(&[("pmpx-plugin-pnpm", PNPM), ("pmpx-plugin-cargo", CARGO)]);
    let set = PluginSet::load(&kit).unwrap();

    let names = set.detect_names();
    // The list is a frozen, sorted, deduplicated set -- it is read once per walked directory,
    // so it must not need cleaning up at the call site.
    assert!(
        names.windows(2).all(|w| w[0] < w[1]),
        "sorted and without duplicates: {names:?}"
    );

    assert!(names.contains(&"Cargo.toml".to_string()));
    assert!(names.contains(&"pnpm-lock.yaml".to_string()));
    assert!(!names.contains(&"package.json.lock".to_string()));
}

#[test]
fn marks_root_on_a_detect_file_or_a_pmpx_toml() {
    let (_t, kit, _) = store(&[("pmpx-plugin-pnpm", PNPM)]);
    let set = PluginSet::load(&kit).unwrap();

    let tmp = tempfile::tempdir().unwrap();
    assert!(!set.marks_root(tmp.path()));

    std::fs::write(tmp.path().join("package.json"), "{}").unwrap();
    assert!(
        set.marks_root(tmp.path()),
        "weak evidence also marks a root"
    );

    let other = tempfile::tempdir().unwrap();
    std::fs::write(other.path().join(".pmpx.toml"), "").unwrap();
    assert!(
        set.marks_root(other.path()),
        ".pmpx.toml is itself a root marker"
    );
}

#[test]
fn marks_root_on_a_strong_evidence_file_too() {
    let (_t, kit, _) = store(&[("pmpx-plugin-cargo", CARGO)]);
    let set = PluginSet::load(&kit).unwrap();

    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("Cargo.lock"), "").unwrap();
    assert!(set.marks_root(tmp.path()));
}

#[test]
fn marks_root_is_false_with_no_plugins_installed() {
    let (_t, kit, _) = store(&[]);
    let set = PluginSet::load(&kit).unwrap();

    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("Cargo.toml"), "").unwrap();

    // With zero plugins **nothing is guessed** — this assertion is the concrete form of "the
    // hint table takes no part in resolution"
    assert!(!set.marks_root(tmp.path()));
}

#[test]
fn a_manifest_without_family_is_listed_but_unusable() {
    let (_t, kit, _) = store(&[(
        "pmpx-plugin-weird",
        "[plugin]\nname = \"weird\"\nversion = \"0.1.0\"\n\n[detect]\nstrong = [\"x\"]\n",
    )]);
    let set = PluginSet::load(&kit).unwrap();

    assert_eq!(set.plugins.len(), 1, "still listed");
    let p = &set.plugins[0];
    assert!(!p.is_usable());
    assert!(p.problem().unwrap().contains("family"));
    assert_eq!(set.usable().count(), 0);
    assert!(
        set.detect_names().is_empty(),
        "unusable plugins take no part in detection"
    );
}

#[test]
fn a_manifest_with_an_empty_detect_section_is_unusable() {
    let (_t, kit, _) = store(&[(
        "pmpx-plugin-blank",
        "[plugin]\nname = \"blank\"\nversion = \"0.1.0\"\nfamily = \"node\"\n\n[detect]\nstrong = []\nweak = []\n",
    )]);
    let set = PluginSet::load(&kit).unwrap();

    let p = &set.plugins[0];
    assert!(!p.is_usable());
    assert!(p.problem().unwrap().contains("detect"));
}

#[test]
fn a_malformed_detect_value_is_skipped_not_fatal() {
    let (_t, kit, _) = store(&[(
        "pmpx-plugin-odd",
        "[plugin]\nname = \"odd\"\nversion = \"0.1.0\"\nfamily = \"node\"\n\n[detect]\nstrong = \"not-an-array\"\nweak = [1, 2, \"package.json\"]\n",
    )]);
    let set = PluginSet::load(&kit).unwrap();

    let p = &set.plugins[0];
    assert!(p.strong.is_empty(), "a non-array is treated as empty");
    assert_eq!(
        p.weak,
        vec!["package.json"],
        "non-string elements are skipped"
    );
    assert!(
        p.is_usable(),
        "one usable piece of evidence is left, the plugin is still usable"
    );
}

#[test]
fn by_name_and_by_crate_name_find_the_same_plugin() {
    let (_t, kit, _) = store(&[("pmpx-plugin-pnpm", PNPM)]);
    let set = PluginSet::load(&kit).unwrap();

    assert_eq!(
        set.by_name("pnpm").map(|p| p.crate_name()),
        Some("pmpx-plugin-pnpm")
    );
    assert_eq!(
        set.by_crate_name("pmpx-plugin-pnpm").map(|p| p.name()),
        Some("pnpm")
    );
    assert!(set.by_name("nope").is_none());
}
