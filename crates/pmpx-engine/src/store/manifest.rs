//! One [`PluginInfo`] into one [`InstalledPlugin`]: the host's own sections, read from the
//! manifest the kit already read.
//!
//! `[detect]` and `[context]` are pmpx's vocabulary, not the kit's — the kit carries every
//! section it does not interpret through [`PluginInfo::extra`] and never looks inside. That
//! is what makes this a pure conversion: no file is opened here, and a manifest cannot be
//! read twice and disagree with itself.

use crate_plugin_kit::PluginInfo;
use pmpx_plugin::Family;

use super::InstalledPlugin;

/// Complete a [`PluginInfo`] into an [`InstalledPlugin`].
pub(super) fn read_one(info: &PluginInfo) -> InstalledPlugin {
    let (strong, weak) = detect_patterns(info);

    InstalledPlugin {
        info: info.clone(),
        family: info.family.clone().map(Family::new),
        strong,
        weak,
        wanted: wanted_files(info),
    }
}

/// Read `[context] files` out of the host's own sections.
///
/// This is the plugin saying which files it wants to see the contents of. The host reads exactly
/// these and nothing else -- the declarative, auditable allowlist stays the plugin's own
/// declaration, and pmpx still knows nothing about what any of them mean.
fn wanted_files(info: &PluginInfo) -> Vec<String> {
    strings(info, "context", "files")
}

/// One `[section] key = [...]` out of the host's own sections, as a list of strings.
///
/// A malformed entry is skipped rather than fatal: a typo in a manifest must not make the plugin
/// disappear from `plugin ls`, and a plugin with nothing to match simply never wins.
fn strings(info: &PluginInfo, section: &str, key: &str) -> Vec<String> {
    info.extra
        .get(section)
        .and_then(|section| section.get(key))
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Read `[detect]`.
///
/// A malformed section should not make the plugin vanish from the list: whatever cannot be read
/// as an array of strings is skipped, and a plugin with nothing to match simply never wins.
fn detect_patterns(info: &PluginInfo) -> (Vec<String>, Vec<String>) {
    (
        strings(info, "detect", "strong"),
        strings(info, "detect", "weak"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate_plugin_kit::{CratePluginKit, KitConfig};
    use pmpx_plugin::abi::PmpxPlugin;

    /// A manifest with both of the host's sections, one of them deliberately malformed.
    const MANIFEST: &str = r#"
[plugin]
name    = "toy"
version = "0.1.0"
abi     = 3
family  = "node"

[detect]
strong = ["toy.lock"]
weak   = ["toy.json", 7]

[context]
files = ["toy.json"]
"#;

    /// A store directory with one plugin in it, read the way the host reads it.
    fn listed(manifest: &str) -> PluginInfo {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = KitConfig::new("pmpx");
        cfg.crate_prefix = "pmpx-plugin-".to_string();
        let kit = CratePluginKit::<PmpxPlugin>::new(cfg.with_data_dir(tmp.path())).unwrap();

        let dir = tmp.path().join("plugins").join("pmpx-plugin-toy");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pmpx-plugin.toml"), manifest).unwrap();

        kit.list().unwrap().remove(0)
    }

    #[test]
    fn the_hosts_sections_become_detect_and_context() {
        let plugin = read_one(&listed(MANIFEST));

        assert_eq!(plugin.strong, vec!["toy.lock".to_string()]);
        assert_eq!(
            plugin.weak,
            vec!["toy.json".to_string()],
            "a non-string element is skipped rather than fatal"
        );
        assert_eq!(plugin.wanted, vec!["toy.json".to_string()]);
        assert_eq!(plugin.name(), "toy");
        assert_eq!(plugin.crate_name(), "pmpx-plugin-toy");
        assert_eq!(plugin.abi(), Some(3));
        assert_eq!(plugin.family, Some(pmpx_plugin::Family::NODE));
    }

    #[test]
    fn a_plugin_without_those_sections_has_nothing_to_match() {
        let bare = "[plugin]\nname = \"toy\"\nversion = \"0.1.0\"\n";

        let plugin = read_one(&listed(bare));

        assert!(plugin.strong.is_empty());
        assert!(plugin.weak.is_empty());
        assert!(plugin.wanted.is_empty());
    }
}
