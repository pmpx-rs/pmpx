//! `.pmpx.toml` and the layered merge.
//!
//! One project can carry several `.pmpx.toml` files -- one per directory walked up from the cwd --
//! and their merge rule is **nearest wins**. Writing a pin is a read-modify-write of the nearest
//! one, so everything a layer carries (including keys pmpx does not know) has to be preserved.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// One `.pmpx.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    /// `[plugin] <family> = "<name>"`: the key is a **family name**, not a plugin name.
    /// `BTreeMap<String, _>` so that new families brought by third-party plugins can be written
    /// without waiting for a release.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub plugin: BTreeMap<String, String>,

    /// `[scripts] name = "run something"`: the semantics are not defined yet; it is only parsed and
    /// kept verbatim — the read-modify-write of `plugin set/unset` must not eat it.
    ///
    /// Empty tables are not written back, so pinning something does not add a `[scripts]` line to a
    /// file that never had one.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub scripts: BTreeMap<String, String>,

    /// Unrecognized keys are kept as they are; same reason as [`GlobalConfig::extra`].
    ///
    /// [`GlobalConfig::extra`]: super::GlobalConfig::extra
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl ProjectConfig {
    /// Read one. A missing file returns `None` (so the caller knows "there is no config here").
    pub fn load_from(path: &Path) -> Result<Option<Self>> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let cfg: Self = toml::from_str(&text).with_context(|| {
                    format!("failed to parse the project config: {}", path.display())
                })?;
                Ok(Some(cfg))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e)
                .with_context(|| format!("failed to read the project config: {}", path.display())),
        }
    }

    /// Overlay `other` on top of `self`; `other` wins (it is "the nearer layer").
    pub fn overlay(&mut self, other: ProjectConfig) {
        // Order matters: extend with unknown keys first, then let known keys overwrite, otherwise an
        // unknown key with the same name would clobber a known one.
        for (k, v) in other.extra {
            self.extra.insert(k, v);
        }
        self.plugin.extend(other.plugin);
        self.scripts.extend(other.scripts);
    }
}

/// The merge result of every `.pmpx.toml` collected from near to far.
#[derive(Debug, Clone, Default)]
pub struct MergedProjectConfig {
    /// Effective `[plugin]` pins: family → plugin name.
    pub plugin: BTreeMap<String, String>,
    /// Effective `[scripts]`. No reader yet, but it cannot be dropped: without it no future reader
    /// would see user-written scripts.
    #[allow(dead_code)]
    pub scripts: BTreeMap<String, String>,
    /// The files actually read, **near to far** (`pmpx info` uses them to say which configs a value
    /// came from).
    pub sources: Vec<PathBuf>,
}

impl MergedProjectConfig {
    /// Merge by "nearest wins"; `paths` must be near to far (the caller collects them walking up).
    pub fn from_paths_near_to_far(paths: &[PathBuf]) -> Result<Self> {
        let mut merged = ProjectConfig::default();
        let mut found = Vec::new();

        // Lay from the farthest first, the nearer written later — the latter naturally overrides the
        // former.
        for path in paths.iter().rev() {
            if let Some(cfg) = ProjectConfig::load_from(path)? {
                merged.overlay(cfg);
                found.push(path.clone());
            }
        }

        // `found` is far-to-near here; reverse it to near-to-far, matching the input convention.
        found.reverse();

        Ok(Self {
            plugin: merged.plugin,
            scripts: merged.scripts,
            sources: found,
        })
    }

    /// Which plugin a family is pinned to.
    pub fn pinned_plugin(&self, family: &str) -> Option<&str> {
        self.plugin.get(family).map(String::as_str)
    }
}
