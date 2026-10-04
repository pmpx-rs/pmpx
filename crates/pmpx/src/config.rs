//! Configuration: one global file, plus any number of layers inside a project.
//!
//! Global is `<pmpx-config-dir>/config.toml` (a single file: personal preferences, not committed,
//! written by `pmpx config set`); project is `.pmpx.toml` in each directory (possibly several,
//! collected level by level upward from the cwd, meant to be committed, written by
//! `pmpx plugin set/unset`). Merge rule: **nearest wins**.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ---- Paths ----------------------------------------------------------------

/// Environment variable overriding the global config directory (for tests and multi-environment use).
pub const ENV_CONFIG_DIR: &str = "PMPX_CONFIG_DIR";

/// Environment variable overriding the plugin data dir. Same reason.
pub const ENV_DATA_DIR: &str = "PMPX_DATA_DIR";

/// Path of the global config file (the config directory differs per platform; `directories` decides).
/// [`ENV_CONFIG_DIR`] overrides it wholesale, and that is a **directory** rather than a file.
pub fn global_config_path() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os(ENV_CONFIG_DIR) {
        return Ok(PathBuf::from(dir).join("config.toml"));
    }
    let dirs = directories::ProjectDirs::from("", "", "pmpx")
        .context("cannot get the config directory (neither HOME nor APPDATA set?)")?;
    Ok(dirs.config_dir().join("config.toml"))
}

/// Plugin data dir: `~/.pmpx`, the same location on all three platforms (not `directories`'s
/// data_dir), because the plugin directory should be within the user's reach.
///
/// It is handed explicitly to `crate-plugin-kit`'s `KitConfig::with_data_dir` — otherwise "the
/// location pmpx shows" and "the location the kit actually uses" would be two implementations.
/// Can be overridden with [`ENV_DATA_DIR`].
pub fn default_data_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os(ENV_DATA_DIR) {
        return Ok(PathBuf::from(dir));
    }
    let dirs = directories::UserDirs::new()
        .context("cannot get the user directory (HOME / USERPROFILE both unset?)")?;
    Ok(dirs.home_dir().join(".pmpx"))
}

/// Expand a leading `~` into the user's home directory (does not handle `~user`).
pub fn expand_tilde(raw: &str) -> PathBuf {
    let rest = raw
        .strip_prefix("~/")
        .or_else(|| raw.strip_prefix("~\\"))
        .or_else(|| if raw == "~" { Some("") } else { None });

    match rest {
        Some(rest) => match directories::UserDirs::new() {
            Some(dirs) => dirs.home_dir().join(rest),
            None => PathBuf::from(raw),
        },
        None => PathBuf::from(raw),
    }
}

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

// ---- Project config -------------------------------------------------------

/// One `.pmpx.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    /// `[plugin] <family> = "<name>"`: the key is a **family name**, not a plugin name.
    /// `BTreeMap<String, _>` so that new families brought by third-party plugins can be written
    /// without waiting for a release.
    pub plugin: BTreeMap<String, String>,

    /// `[scripts] name = "run something"`: the semantics are not defined yet; it is only parsed and
    /// kept verbatim — the read-modify-write of `plugin set/unset` must not eat it.
    pub scripts: BTreeMap<String, String>,

    /// Unrecognized keys are kept as they are; same reason as [`GlobalConfig::extra`].
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

    /// Every family that has been pinned.
    pub fn pinned_families(&self) -> Vec<&str> {
        self.plugin.keys().map(String::as_str).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn missing_global_config_is_all_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = GlobalConfig::load_from(&tmp.path().join("nope.toml")).unwrap();

        assert_eq!(cfg.plugin.family_priority[0], "node");
        assert_eq!(cfg.plugin.priority[0], "pnpm");
        assert!(cfg.discovery.walk_up);
        assert_eq!(cfg.discovery.max_depth, 8);
        assert!(cfg.discovery.stop_at_git);
        assert!(cfg.plugin_store.effective_prefer_prebuilt());
    }

    #[test]
    fn unknown_keys_in_the_global_config_are_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(
            tmp.path(),
            "config.toml",
            r#"
[plugin]
family_priority = ["rust", "node"]

# added by the user, pmpx does not know it
[my_own_thing]
keep = "me"
"#,
        );

        let cfg = GlobalConfig::load_from(&path).unwrap();
        assert_eq!(cfg.plugin.family_priority, vec!["rust", "node"]);
        assert_eq!(cfg.plugin.priority[0], "pnpm");
        // Unrecognized sections must survive — `pmpx config set` relies on this to carry them back
        // into the file verbatim
        assert!(cfg.extra.contains_key("my_own_thing"));
    }

    #[test]
    fn a_broken_global_config_is_an_error_not_a_silent_default() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(tmp.path(), "config.toml", "this is not toml = = =");

        let err = GlobalConfig::load_from(&path).unwrap_err();
        assert!(
            err.to_string()
                .contains("failed to parse the global config"),
            "{err}"
        );
    }

    #[test]
    fn project_config_missing_is_none_not_default() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(ProjectConfig::load_from(&tmp.path().join("nope.toml"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn near_config_wins_over_far_one() {
        let tmp = tempfile::tempdir().unwrap();
        let far = write(
            tmp.path(),
            "far.toml",
            "[plugin]\nnode = \"npm\"\nrust = \"cargo\"\n",
        );
        let near = write(tmp.path(), "near.toml", "[plugin]\nnode = \"pnpm\"\n");

        let merged =
            MergedProjectConfig::from_paths_near_to_far(&[near.clone(), far.clone()]).unwrap();

        assert_eq!(
            merged.pinned_plugin("node"),
            Some("pnpm"),
            "the nearer one wins"
        );
        assert_eq!(
            merged.pinned_plugin("rust"),
            Some("cargo"),
            "the farther one is not clobbered"
        );
        assert_eq!(merged.pinned_plugin("python"), None);
    }

    #[test]
    fn overlaying_unknown_keys_does_not_clobber_known_ones() {
        let mut base = ProjectConfig::default();
        base.plugin.insert("node".into(), "npm".into());

        let mut other = ProjectConfig::default();
        other.plugin.insert("node".into(), "pnpm".into());
        base.overlay(other);

        assert_eq!(base.plugin.get("node").map(String::as_str), Some("pnpm"));
    }

    #[test]
    fn scripts_merge_across_layers() {
        let tmp = tempfile::tempdir().unwrap();
        let far = write(
            tmp.path(),
            "far.toml",
            "[scripts]\nfmt = \"run format\"\nlint = \"run lint\"\n",
        );
        let near = write(tmp.path(), "near.toml", "[scripts]\nfmt = \"run f\"\n");

        let merged = MergedProjectConfig::from_paths_near_to_far(&[near, far]).unwrap();
        assert_eq!(merged.scripts.get("fmt").map(String::as_str), Some("run f"));
        assert_eq!(
            merged.scripts.get("lint").map(String::as_str),
            Some("run lint")
        );
    }

    #[test]
    fn sources_are_recorded_near_to_far() {
        let tmp = tempfile::tempdir().unwrap();
        let far = write(tmp.path(), "far.toml", "[plugin]\nrust = \"cargo\"\n");
        let near = write(tmp.path(), "near.toml", "[plugin]\nnode = \"pnpm\"\n");

        let merged =
            MergedProjectConfig::from_paths_near_to_far(&[near.clone(), far.clone()]).unwrap();

        assert_eq!(merged.sources, vec![near, far], "near to far");
    }

    #[test]
    fn a_config_file_that_vanished_mid_walk_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let real = write(tmp.path(), "real.toml", "[plugin]\nrust = \"cargo\"\n");
        let ghost = tmp.path().join("ghost.toml");

        let merged = MergedProjectConfig::from_paths_near_to_far(&[ghost, real]).unwrap();
        assert_eq!(merged.pinned_plugin("rust"), Some("cargo"));
        assert_eq!(merged.sources.len(), 1);
    }

    #[test]
    fn expand_tilde_expands_to_home() {
        let home = directories::UserDirs::new()
            .unwrap()
            .home_dir()
            .to_path_buf();

        assert_eq!(expand_tilde("~/x/y"), home.join("x").join("y"));
        assert_eq!(expand_tilde("~"), home);
        // `~user` is not handled, and ordinary paths must not be mangled
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(
            expand_tilde("relative/path"),
            PathBuf::from("relative/path")
        );
    }

    #[test]
    fn pinned_families_lists_every_pin() {
        let tmp = tempfile::tempdir().unwrap();
        let a = write(
            tmp.path(),
            "a.toml",
            "[plugin]\nnode = \"pnpm\"\nrust = \"cargo\"\n",
        );

        let merged = MergedProjectConfig::from_paths_near_to_far(&[a]).unwrap();
        let mut fams = merged.pinned_families();
        fams.sort_unstable();
        assert_eq!(fams, vec!["node", "rust"]);
    }

    // ---- Environment variable overrides -----------------------------------

    /// Both override variables must actually take effect — otherwise e2e tests could only touch the
    /// user's real config directory.
    ///
    /// It mutates process-level environment variables: set inside one test only, original values
    /// saved, restored before asserting, and only the `PMPX_`-prefixed variables read by this module
    /// are touched.
    #[test]
    fn env_overrides_take_effect() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("cfg");
        let data_dir = tmp.path().join("data");

        let old_cfg = std::env::var_os(ENV_CONFIG_DIR);
        let old_data = std::env::var_os(ENV_DATA_DIR);

        // SAFETY: see the note above about scope.
        unsafe {
            std::env::set_var(ENV_CONFIG_DIR, &cfg_dir);
            std::env::set_var(ENV_DATA_DIR, &data_dir);
        }

        let got_cfg = global_config_path().unwrap();
        let got_data = default_data_dir().unwrap();

        // Restore before asserting — a failing assertion must not leave the environment dirty
        unsafe {
            match old_cfg {
                Some(v) => std::env::set_var(ENV_CONFIG_DIR, v),
                None => std::env::remove_var(ENV_CONFIG_DIR),
            }
            match old_data {
                Some(v) => std::env::set_var(ENV_DATA_DIR, v),
                None => std::env::remove_var(ENV_DATA_DIR),
            }
        }

        assert_eq!(got_cfg, cfg_dir.join("config.toml"));
        assert_eq!(got_data, data_dir);
    }

    /// Without overrides the platform-correct locations are used.
    #[test]
    fn without_overrides_the_platform_paths_are_used() {
        if std::env::var_os(ENV_CONFIG_DIR).is_none() {
            let p = global_config_path().unwrap();
            assert!(p.ends_with("config.toml"), "{p:?}");
            assert!(
                p.to_string_lossy().contains("pmpx"),
                "the path should contain pmpx: {p:?}"
            );
        }
        if std::env::var_os(ENV_DATA_DIR).is_none() {
            let p = default_data_dir().unwrap();
            assert_eq!(p.file_name().unwrap().to_string_lossy(), ".pmpx");
        }
    }
}
