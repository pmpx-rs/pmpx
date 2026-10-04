//! Shared fixtures for the detection tests.
//!
//! Both halves of detection need the same scenario -- "a store with these plugins installed, plus a
//! project directory containing these files" -- so it is built once here and the per-subject test
//! modules live beside this file.

use std::path::PathBuf;
use std::time::Duration;

use crate_plugin_kit::CratePluginKit;
use pmpx_plugin::abi::PmpxPluginV1;

use crate::config::{GlobalConfig, MergedProjectConfig};
use crate::detect::{select, DetectFailure, Selection};
use crate::plugins::PluginSet;

mod failure;
mod scoring;
mod selection;

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
