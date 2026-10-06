//! Where the config files live, and how a configured path is interpreted.
//!
//! Every location here is platform-derived rather than hard-coded: `~/.config` is neither the
//! macOS nor the Windows answer. The two environment overrides exist so that a test (or a second
//! environment) can point pmpx somewhere harmless instead of the user's real config directory.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Environment variable overriding the global config directory (for tests and multi-environment use).
pub const ENV_CONFIG_DIR: &str = "PMPX_CONFIG_DIR";

/// Environment variable overriding the plugin data dir. Same reason.
pub const ENV_DATA_DIR: &str = "PMPX_DATA_DIR";

/// Path of the global config file (the config directory differs per platform; `directories` decides).
/// [`ENV_CONFIG_DIR`] overrides it wholesale, and that is a **directory** rather than a file.
///
/// Where that lands, in full:
///
/// | Platform | File |
/// | --- | --- |
/// | Linux | `$XDG_CONFIG_HOME/pmpx/config.toml` (`~/.config/pmpx/config.toml` by default) |
/// | macOS | `~/Library/Application Support/pmpx/config.toml` |
/// | Windows | `%APPDATA%\pmpx\config\config.toml` |
///
/// The doubled `config` on Windows comes from `ProjectDirs`' layout (`RoamingAppData\<app>\config`),
/// and it is left as it is on purpose: released versions of pmpx read that file, so tidying it up
/// would silently move every Windows user's configuration.
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
