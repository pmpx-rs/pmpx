//! `plugin set / unset`: pinning which plugin answers, in the nearest `.pmpx.toml`.
//!
//! Writing a pin is a read-modify-write of the nearest `.pmpx.toml`, which is the one project file
//! pmpx ever writes, and only when asked. Every **key** the file already carries — other pins, the
//! `[scripts]` section, keys pmpx does not know — is read back and written out again.
//!
//! What it does not keep is the *text*: the file is parsed into a document and serialised again, so
//! comments and the exact layout are lost, and tables are written in a fixed order. Keys and values
//! are what survives; a hand-written comment next to a pin does not.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::app;
use crate::cli::Cli;
use crate::config::ProjectConfig;
use crate::error::{PmpxError, EXIT_OK};
use crate::style;

/// `plugin set <name>`: pin it in the nearest layer of `.pmpx.toml`.
pub(super) fn plugin_set(args: &Cli, name: &str) -> crate::error::Result<u8> {
    let session = app::session(args)?;

    let Some(plugin) = session.plugins.by_name(name) else {
        return Err(PmpxError::not_found(format!(
            "no plugin named {name}. Installed: {}",
            session
                .plugins
                .usable()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    };

    let Some(family) = plugin.family.clone() else {
        return Err(PmpxError::Usage(format!(
            "{name}'s manifest declares no family, so it cannot be pinned."
        )));
    };

    let path = target_config_path(&session);
    let current = ProjectConfig::load_from(&path)
        .map_err(PmpxError::Other)?
        .unwrap_or_default();
    edit_project_config(&path, current, |cfg| {
        cfg.plugin
            .insert(family.as_str().to_string(), plugin.name.clone());
    })
    .map_err(PmpxError::Other)?;

    anstream::println!(
        "Pinned {} = \"{}\" in {}",
        family.as_str(),
        style::paint(style::PM, &plugin.name),
        style::paint(style::DIM, path.display())
    );
    Ok(EXIT_OK)
}

/// `plugin unset [family] [--yes]`
pub(super) fn plugin_unset(
    args: &Cli,
    family: Option<&str>,
    yes: bool,
) -> crate::error::Result<u8> {
    let session = app::session(args)?;
    let path = target_config_path(&session);

    let existing = ProjectConfig::load_from(&path)
        .map_err(PmpxError::Other)?
        .unwrap_or_default();

    match family {
        Some(f) => {
            if !existing.plugin.contains_key(f) {
                return Err(PmpxError::Usage(format!(
                    "{} has no pin for {f}",
                    path.display()
                )));
            }
            edit_project_config(&path, existing, |cfg| {
                cfg.plugin.remove(f);
            })
            .map_err(PmpxError::Other)?;
            anstream::println!(
                "Removed the pin for {f} from {}",
                style::paint(style::DIM, path.display())
            );
        }

        None => {
            if existing.plugin.is_empty() {
                return Err(PmpxError::Usage(format!(
                    "{} has no pins at all",
                    path.display()
                )));
            }

            // With the family omitted and more than one pin present, `--yes` is required;
            // otherwise list what would be deleted and exit with code 2.
            if existing.plugin.len() > 1 && !yes {
                let mut msg = format!(
                    "{} has {} pins; deleting all of them needs `--yes`:",
                    path.display(),
                    existing.plugin.len()
                );
                for (f, p) in &existing.plugin {
                    msg.push_str(&format!("\n  {f} = \"{p}\""));
                }
                return Err(PmpxError::Usage(msg));
            }

            let removed: Vec<String> = existing.plugin.keys().cloned().collect();
            edit_project_config(&path, existing, |cfg| {
                cfg.plugin.clear();
            })
            .map_err(PmpxError::Other)?;
            anstream::println!(
                "Removed the pins for {} from {}",
                removed.join(", "),
                style::paint(style::DIM, path.display())
            );
        }
    }

    Ok(EXIT_OK)
}

/// Which `.pmpx.toml` to write.
///
/// The nearest layer wins: an existing file is the layer the user chose; when there is none,
/// write at the project root.
///
/// `Session::open` already collected exactly that list -- `project.sources` holds every
/// `.pmpx.toml` the walk found, nearest first -- so this asks it instead of walking the same
/// directories a second time.
fn target_config_path(session: &pmpx_engine::Session) -> PathBuf {
    if let Some(nearest) = session.project.sources.first() {
        return nearest.clone();
    }

    let base = session
        .project_root()
        .map(PathBuf::from)
        .unwrap_or_else(|| session.start_dir.clone());
    base.join(".pmpx.toml")
}

/// Read-modify-write one `.pmpx.toml`, keeping keys it does not recognise.
///
/// `current` is the content the caller already read: passing it in keeps "what was inspected" and
/// "what is edited" the same document, and saves parsing the same file twice.
fn edit_project_config(
    path: &Path,
    current: ProjectConfig,
    edit: impl FnOnce(&mut ProjectConfig),
) -> Result<()> {
    let mut cfg = current;
    edit(&mut cfg);

    // Do not leave an empty file behind for an empty config
    if cfg.plugin.is_empty() && cfg.scripts.is_empty() && cfg.extra.is_empty() {
        if path.is_file() {
            std::fs::remove_file(path).with_context(|| {
                format!("failed to delete the empty config file: {}", path.display())
            })?;
        }
        return Ok(());
    }

    let text = toml::to_string_pretty(&cfg).context("failed to serialise the project config")?;
    crate::config::atomic_write(path, &text)
}
