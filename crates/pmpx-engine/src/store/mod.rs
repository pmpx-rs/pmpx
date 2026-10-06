//! The manifest of installed plugins.
//!
//! This module only reads `~/.pmpx/plugins/*/pmpx-plugin.toml` into structs and **loads no dynamic
//! library** — detection must still give an answer when a plugin is broken / ABI-mismatched /
//! built for another platform. Loading happens only after a plugin has been selected
//! (see [`crate::Backend`]).
//!
//! This file is the installed set and what one entry means; [`manifest`](self) is the one place that
//! turns an entry's manifest into those fields.

use std::path::Path;

use anyhow::{Context, Result};
use crate_plugin_kit::{CratePluginKit, PluginInfo};
use pmpx_plugin::abi::PmpxPlugin;
use pmpx_plugin::Family;

mod manifest;

use self::manifest::read_one;

/// One installed plugin, as this host sees it.
///
/// The kit's [`PluginInfo`] is kept whole rather than copied field by field: it is what the manifest
/// said, host sections (`[detect]`, `[context]`, and anything a later version adds) included — so a new
/// section needs a field here only when this host actually reads it. `name` and the rest are read
/// through [`InstalledPlugin::name`] and its neighbours.
///
/// No `PartialEq`: the kit's [`PluginInfo`] has none, and comparing two plugins is not something this
/// host does.
#[derive(Debug, Clone)]
pub struct InstalledPlugin {
    /// What the kit read from the manifest, in full.
    pub info: PluginInfo,

    /// Family. `None` means the manifest did not declare one — see [`InstalledPlugin::problem`].
    pub family: Option<Family>,

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
    /// The plugin's self-reported name.
    pub fn name(&self) -> &str {
        &self.info.name
    }

    /// Full crate name (`pmpx-plugin-pnpm`).
    pub fn crate_name(&self) -> &str {
        &self.info.crate_name
    }

    /// Version.
    pub fn version(&self) -> &str {
        &self.info.version
    }

    /// The ABI version the manifest declares. Diagnostics only.
    pub fn abi(&self) -> Option<u32> {
        self.info.abi
    }

    /// Install directory.
    pub fn dir(&self) -> &std::path::Path {
        &self.info.dir
    }

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
            plugins.push(read_one(&info));
        }
        plugins.sort_by(|a, b| a.crate_name().cmp(b.crate_name()));

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
        self.plugins.iter().find(|p| p.name() == name)
    }

    /// Find by crate name.
    pub fn by_crate_name(&self, crate_name: &str) -> Option<&InstalledPlugin> {
        self.plugins.iter().find(|p| p.crate_name() == crate_name)
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

// The kit's own types, named here so that the host's commands never mention the kit: a command prints
// an `InstallSource` or shows an `Installed`, and where those come from is this module's business.
pub use crate_plugin_kit::cache::InstallSource;
pub use crate_plugin_kit::install::Installed;
pub use crate_plugin_kit::{CrateInfo, CrateSummary};

use crate::EngineError;

/// What a plugin checkout declares, read out of its manifest.
///
/// The kit installs whatever compiles; whether *this host* can use it is this crate's business, and this
/// is the whole answer to that question. Turning it into refusals and warnings -- the words -- belongs to
/// the command, because the words are presentation.
#[derive(Debug, Clone)]
pub struct VetReport {
    /// The ABI version the checkout declares.
    pub abi: Option<u32>,

    /// Its family, if it declares one.
    pub family: Option<String>,

    /// Every `[detect]` marker it would be selected by, strong and weak together.
    pub markers: Vec<String>,
}

/// Read the manifest of a plugin checkout.
///
/// A directory without the manifest is not a checkout, which is a usage error: the person named a place
/// that has nothing to install from.
pub fn vet_checkout(dir: &Path, manifest_name: &str) -> Result<VetReport, EngineError> {
    let path = dir.join(manifest_name);

    if !path.is_file() {
        return Err(EngineError::Usage(format!(
            "{} has no {manifest_name}, so it is not a plugin checkout",
            dir.display()
        )));
    }

    let manifest = crate_plugin_kit::PluginManifest::read(&path)
        .map_err(|error| EngineError::Usage(format!("cannot read {}: {error}", path.display())))?;

    Ok(VetReport {
        abi: manifest.plugin.abi,
        family: manifest.plugin.family.clone(),
        markers: markers(&manifest),
    })
}

/// Every `[detect]` marker the manifest declares.
fn markers(manifest: &crate_plugin_kit::PluginManifest) -> Vec<String> {
    let mut out = str_array(manifest, "detect", "strong");
    out.extend(str_array(manifest, "detect", "weak"));
    out
}

/// One `[section] key = [...]` out of the host's own sections.
fn str_array(manifest: &crate_plugin_kit::PluginManifest, section: &str, key: &str) -> Vec<String> {
    manifest
        .extra
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
#[cfg(test)]
mod tests;
