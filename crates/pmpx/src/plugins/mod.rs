//! The manifest of installed plugins.
//!
//! This module only reads `~/.pmpx/plugins/*/pmpx-plugin.toml` into structs and **loads no dynamic
//! library** — detection must still give an answer when a plugin is broken / ABI-mismatched /
//! built for another platform. Loading happens only after a plugin has been selected
//! (see [`crate::runtime`]).
//!
//! This file is the installed set and what one entry means; [`manifest`] is the one place that
//! turns an entry's manifest into those fields.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use crate_plugin_kit::CratePluginKit;
use pmpx_plugin::abi::PmpxPlugin;
use pmpx_plugin::Family;

mod manifest;

use self::manifest::read_one;

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

    /// The files this plugin asked to see the contents of, from its manifest's `[context] files`.
    ///
    /// Read from the project root and handed over with the call; the host does not interpret them.
    pub wanted: Vec<String>,
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

    /// Every detect file name declared by a usable plugin, deduplicated and sorted.
    ///
    /// Derived from `plugins` and then frozen: it is read once per walked directory (see
    /// [`PluginSet::marks_root`]), and rebuilding it there would re-collect and re-sort the same
    /// names for every step of the walk.
    detect_names: Vec<String>,
}

impl PluginSet {
    /// Read once from the plugin store (manifest only).
    pub fn load(kit: &CratePluginKit<PmpxPlugin>) -> Result<Self> {
        let infos = kit.list().context("failed to scan the plugin directory")?;

        let mut plugins = Vec::with_capacity(infos.len());
        for info in infos {
            plugins.push(read_one(kit, &info)?);
        }
        plugins.sort_by(|a, b| a.crate_name.cmp(&b.crate_name));

        let mut detect_names: Vec<String> = plugins
            .iter()
            .filter(|p| p.is_usable())
            .flat_map(|p| p.detect_names())
            .map(str::to_string)
            .collect();
        detect_names.sort_unstable();
        detect_names.dedup();

        Ok(Self {
            plugins,
            detect_names,
        })
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

    /// Every detect file name declared by any plugin, deduplicated and sorted.
    ///
    /// A directory containing any one of them looks like a project root. strong / weak is not
    /// distinguished here — the project root is a structural judgement, and `package.json` and
    /// `pnpm-lock.yaml` are equally valid for it.
    pub fn detect_names(&self) -> &[String] {
        &self.detect_names
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

#[cfg(test)]
mod tests;
