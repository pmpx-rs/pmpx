//! Configuration: one global file, plus any number of layers inside a project.
//!
//! Global is `<pmpx-config-dir>/config.toml` (a single file: personal preferences, not committed,
//! written by `pmpx config set`); project is `.pmpx.toml` in each directory (possibly several,
//! collected level by level upward from the cwd, meant to be committed, written by
//! `pmpx plugin set/unset`). Merge rule: **nearest wins**.
//!
//! This file is the global file and the types it is made of; [`paths`] says where the files live,
//! [`project`] holds `.pmpx.toml` and its layered merge, and [`write`] is how either of them is
//! written back.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

mod paths;
mod project;
mod write;

pub use paths::{default_data_dir, expand_tilde, global_config_path};
pub use project::{MergedProjectConfig, ProjectConfig};
pub use write::atomic_write;

// ---- Global config --------------------------------------------------------

/// `<pmpx-config-dir>/config.toml`, read once at startup — `[discovery]` affects every command.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalConfig {
    /// Ordering tables for plugins.
    pub plugin: GlobalPluginConfig,
    /// Project root discovery behaviour.
    pub discovery: DiscoveryConfig,
    /// Plugin store location and install preferences.
    pub plugin_store: PluginStoreConfig,

    /// Unrecognized keys are kept as they are — `pmpx config set` is a read-modify-write, so losing
    /// them would silently eat the user's config.
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// Ordering tables across families and within a family. This is the only source of "mixed projects
/// default to Node".
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalPluginConfig {
    /// Order between families: resolves **ties across families**; the earlier, the more preferred.
    /// Unlisted families come after all listed ones, in name order — so even unknown families can
    /// be resolved.
    pub family_priority: Vec<String>,

    /// Order of plugins within one family: resolves **ties**.
    pub priority: Vec<String>,
}

impl Default for GlobalPluginConfig {
    fn default() -> Self {
        Self {
            // Mixed projects (Rust + frontend) default to Node — through this array, not a rule
            // hard-coded somewhere.
            family_priority: vec![
                "node".into(),
                "rust".into(),
                "python".into(),
                "go".into(),
                "jvm".into(),
                "dotnet".into(),
                "php".into(),
                "ruby".into(),
            ],
            // With only a package.json (all four Node backends at 10 points) pnpm is the default.
            priority: vec![
                "pnpm".into(),
                "npm".into(),
                "yarn".into(),
                "bun".into(),
                "cargo".into(),
            ],
        }
    }
}

/// `[discovery]`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscoveryConfig {
    /// Whether to look upward for the project root. `false` = only the start directory.
    pub walk_up: bool,
    /// How many directories may be checked at most (including the start).
    pub max_depth: usize,
    /// Stop at `.git` (anything above a repository root is not part of this project).
    pub stop_at_git: bool,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            walk_up: true,
            max_depth: 8,
            stop_at_git: true,
        }
    }
}

/// `[plugin_store]`: plugin store location and install preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginStoreConfig {
    /// Override the plugin directory. Empty = [`default_data_dir`].
    pub data_dir: Option<PathBuf>,
    /// Download a prebuilt when available, falling back to build-host on failure.
    pub prefer_prebuilt: Option<bool>,
}

impl PluginStoreConfig {
    /// The data dir in effect.
    pub fn effective_data_dir(&self) -> Result<PathBuf> {
        match &self.data_dir {
            Some(p) => Ok(expand_tilde(&p.to_string_lossy())),
            None => default_data_dir(),
        }
    }

    /// The prebuilt preference in effect.
    pub fn effective_prefer_prebuilt(&self) -> bool {
        self.prefer_prebuilt.unwrap_or(true)
    }
}

impl GlobalConfig {
    /// Read the global config. **A missing file means all defaults, not an error.**
    pub fn load() -> Result<Self> {
        Self::load_from(&global_config_path()?)
    }

    /// Read from a given path. For tests.
    pub fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .with_context(|| format!("failed to parse the global config: {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e)
                .with_context(|| format!("failed to read the global config: {}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests;
