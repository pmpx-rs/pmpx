//! The manifest of installed plugins.
//!
//! This module only reads `~/.pmpx/plugins/*/pmpx-plugin.toml` into structs and **loads no dynamic
//! library** — detection must still give an answer when a plugin is broken / ABI-mismatched /
//! built for another platform. Loading happens only after a plugin has been selected
//! (see [`crate::runtime`]).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use crate_plugin_kit::{CratePluginKit, PluginInfo, PluginManifest};
use pmpx_plugin::abi::PmpxPluginV1;
use pmpx_plugin::Family;

/// Manifest information of one installed plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPlugin {
    /// The plugin's self-reported name (the manifest's `plugin.name`, e.g. `pnpm`).
    ///
    /// The manifest is the authority for this name; after loading, `PackageManager::name()` must
    /// equal it, otherwise loading is refused.
    pub name: String,

    /// Full crate name (`pmpx-plugin-pnpm`).
    pub crate_name: String,

    /// Version.
    pub version: String,

    /// Family. `None` means the manifest did not declare one — see [`InstalledPlugin::problem`].
    pub family: Option<Family>,

    /// The ABI version declared by the manifest.
    pub abi: Option<u32>,

    /// Install directory.
    pub dir: PathBuf,

    /// Strong evidence (100 points each): proves this backend has really been used.
    pub strong: Vec<String>,

    /// Weak evidence (10 points each): only proves the project belongs to this family.
    pub weak: Vec<String>,
}

impl InstalledPlugin {
    /// Whether this plugin can take part in detection and resolution.
    ///
    /// `plugin ls` shows the reason when it cannot — "installed but not in effect" is the hardest
    /// class of problem to diagnose on your own.
    pub fn is_usable(&self) -> bool {
        self.problem().is_none()
    }

    /// Why it cannot take part in resolution.
    pub fn problem(&self) -> Option<&'static str> {
        match self.family {
            // family decides `plugin ls` grouping, the scope of `plugin set`, and the key names in
            // `.pmpx.toml`; without it the plugin cannot be selected by any layer — better to say
            // it is missing than to guess a family.
            None => Some("the manifest does not declare a family, so it cannot take part in family resolution"),
            Some(_) if self.strong.is_empty() && self.weak.is_empty() => {
                Some("the manifest's [detect] section is empty, so detection will never match it")
            }
            Some(_) => None,
        }
    }

    /// Every detect file this plugin declares.
    pub fn detect_names(&self) -> impl Iterator<Item = &str> {
        self.strong
            .iter()
            .chain(self.weak.iter())
            .map(String::as_str)
    }
}

/// Every plugin installed on this machine.
#[derive(Debug, Clone, Default)]
pub struct PluginSet {
    /// Sorted by crate name to keep output stable.
    pub plugins: Vec<InstalledPlugin>,
}

impl PluginSet {
    /// Read once from the plugin store (manifest only).
    pub fn load(kit: &CratePluginKit<PmpxPluginV1>) -> Result<Self> {
        let infos = kit.list().context("failed to scan the plugin directory")?;

        let mut plugins = Vec::with_capacity(infos.len());
        for info in infos {
            plugins.push(read_one(kit, &info)?);
        }
        plugins.sort_by(|a, b| a.crate_name.cmp(&b.crate_name));

        Ok(Self { plugins })
    }

    /// Those that take part in resolution.
    pub fn usable(&self) -> impl Iterator<Item = &InstalledPlugin> {
        self.plugins.iter().filter(|p| p.is_usable())
    }

    /// Find by self-reported name. The name comes from the manifest, matching `pmpx -p <name>`.
    pub fn by_name(&self, name: &str) -> Option<&InstalledPlugin> {
        self.plugins.iter().find(|p| p.name == name)
    }

    /// Find by crate name.
    pub fn by_crate_name(&self, crate_name: &str) -> Option<&InstalledPlugin> {
        self.plugins.iter().find(|p| p.crate_name == crate_name)
    }

    /// Every detect file name declared by any plugin, deduplicated.
    ///
    /// A directory containing any one of them looks like a project root. strong / weak is not
    /// distinguished here — the project root is a structural judgement, and `package.json` and
    /// `pnpm-lock.yaml` are equally valid for it.
    pub fn detect_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.usable().flat_map(|p| p.detect_names()).collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Whether this directory looks like a project root.
    pub fn marks_root(&self, dir: &Path) -> bool {
        // `.pmpx.toml` itself is "I declare this is a project here" — stronger than any manifest
        // file.
        if dir.join(".pmpx.toml").is_file() {
            return true;
        }
        self.detect_names()
            .iter()
            .any(|name| dir.join(name).exists())
    }
}

/// Complete a [`PluginInfo`] into an [`InstalledPlugin`] (reading the manifest again for `[detect]`).
fn read_one(kit: &CratePluginKit<PmpxPluginV1>, info: &PluginInfo) -> Result<InstalledPlugin> {
    // `list()` just read this manifest successfully, so a failure here can only mean the file
    // disappeared between the two reads. Fall back to an empty detect then — the plugin is still
    // listed, it just does not take part in detection.
    let manifest = kit.manifest_of(&info.crate_name).ok();
    let (strong, weak) = manifest.as_ref().map(detect_patterns).unwrap_or_default();

    Ok(InstalledPlugin {
        name: info.name.clone(),
        crate_name: info.crate_name.clone(),
        version: info.version.clone(),
        family: info.family.clone().map(Family::new),
        abi: info.abi,
        dir: info.dir.clone(),
        strong,
        weak,
    })
}

/// Dig `[detect]` out of the manifest's unrecognized fields.
///
/// `crate-plugin-kit` does not know it, so it is left as-is in `extra`.
fn detect_patterns(manifest: &PluginManifest) -> (Vec<String>, Vec<String>) {
    let Some(detect) = manifest.extra.get("detect") else {
        return (Vec::new(), Vec::new());
    };

    (
        str_array(detect.get("strong")),
        str_array(detect.get("weak")),
    )
}

/// Read a TOML value as an array of strings; when it is not an array, or an element is not a
/// string, skip it instead of erroring — a malformed `detect` section must not make the whole
/// plugin disappear from the manifest.
fn str_array(value: Option<&toml::Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|x| x.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
    fn store(
        entries: &[(&str, &str)],
    ) -> (tempfile::TempDir, CratePluginKit<PmpxPluginV1>, PathBuf) {
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
        let kit = CratePluginKit::<PmpxPluginV1>::new(cfg).unwrap();

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
        assert_eq!(p.name, "pnpm");
        assert_eq!(p.crate_name, "pmpx-plugin-pnpm");
        assert_eq!(p.version, "0.1.0");
        assert_eq!(p.family.as_ref().map(Family::as_str), Some("node"));
        assert_eq!(p.abi, Some(1));
        assert_eq!(p.strong, vec!["pnpm-lock.yaml", "pnpm-workspace.yaml"]);
        assert_eq!(p.weak, vec!["package.json"]);
        assert!(p.is_usable());
    }

    #[test]
    fn results_are_sorted_by_crate_name() {
        let (_t, kit, _) = store(&[("pmpx-plugin-pnpm", PNPM), ("pmpx-plugin-cargo", CARGO)]);
        let set = PluginSet::load(&kit).unwrap();

        let names: Vec<_> = set.plugins.iter().map(|p| p.crate_name.as_str()).collect();
        assert_eq!(names, vec!["pmpx-plugin-cargo", "pmpx-plugin-pnpm"]);
    }

    #[test]
    fn detect_names_are_deduped_across_plugins() {
        let (_t, kit, _) = store(&[("pmpx-plugin-pnpm", PNPM), ("pmpx-plugin-cargo", CARGO)]);
        let set = PluginSet::load(&kit).unwrap();

        let mut names = set.detect_names();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before, "duplicates: {names:?}");

        assert!(names.contains(&"Cargo.toml"));
        assert!(names.contains(&"pnpm-lock.yaml"));
        assert!(!names.contains(&"package.json.lock"));
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
            set.by_name("pnpm").map(|p| &p.crate_name),
            Some(&"pmpx-plugin-pnpm".to_string())
        );
        assert_eq!(
            set.by_crate_name("pmpx-plugin-pnpm").map(|p| &p.name),
            Some(&"pnpm".to_string())
        );
        assert!(set.by_name("nope").is_none());
    }
}
