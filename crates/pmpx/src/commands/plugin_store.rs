//! `plugin add / rm / update / search / info`: the plugin store and crates.io.
//!
//! Everything here goes through [`crate::app::Session::kit`], which owns the install lock,
//! the build-host path and the download path. This file is the reporting layer around it.

use pmpx_plugin::Family;

use crate::app;
use crate::cli::Cli;
use crate::error::{PmpxError, EXIT_OK};
use crate::style;

/// `plugin add`
pub(super) fn plugin_add(
    args: &Cli,
    names: &[String],
    version: Option<&str>,
) -> crate::error::Result<u8> {
    let session = app::session(args)?;

    if version.is_some() && names.len() > 1 {
        return Err(PmpxError::Usage(
            "--version can only be used with a single plugin name".to_string(),
        ));
    }

    // Installing several names one by one can stop halfway, and the ones before the failure stay
    // installed -- so they are named when that happens, instead of leaving the user to guess what
    // state their plugin directory is in.
    let mut installed_so_far: Vec<String> = Vec::new();

    for name in names {
        anstream::println!("Installing {}...", style::paint(style::PM, name));

        match session.kit.install(name, version) {
            Ok(installed) => {
                anstream::println!(
                    "  {} v{} ({}) -> {}",
                    installed.crate_name,
                    style::paint(style::DIM, installed.version),
                    style::paint(style::DIM, describe_source(installed.source)),
                    style::paint(style::DIM, installed.dir.display())
                );
                installed_so_far.push(installed.crate_name);
            }

            Err(e) => {
                if !installed_so_far.is_empty() {
                    crate::error::note_line(format!(
                        "{name} was not installed; installed before it: {}",
                        installed_so_far.join(", ")
                    ));
                }
                return Err(e.into());
            }
        }
    }

    Ok(EXIT_OK)
}

/// `plugin rm`
pub(super) fn plugin_rm(args: &Cli, names: &[String]) -> crate::error::Result<u8> {
    let session = app::session(args)?;

    for name in names {
        session.kit.uninstall(name)?;
        anstream::println!("Removed {}", style::paint(style::PM, name));
    }

    Ok(EXIT_OK)
}

/// `plugin update`
pub(super) fn plugin_update(
    args: &Cli,
    names: &[String],
    version: Option<&str>,
) -> crate::error::Result<u8> {
    let session = app::session(args)?;

    if version.is_some() && names.len() > 1 {
        return Err(PmpxError::Usage(
            "--version can only be used with a single plugin name".to_string(),
        ));
    }

    // No names = update all of them
    let targets: Vec<String> = if names.is_empty() {
        session.plugins.usable().map(|p| p.name.clone()).collect()
    } else {
        names.to_vec()
    };

    if targets.is_empty() {
        anstream::println!("No plugins to update.");
        return Ok(EXIT_OK);
    }

    for name in targets {
        let installed = session.kit.update(&name, version)?;
        anstream::println!(
            "{} -> v{}",
            installed.crate_name,
            style::paint(style::DIM, installed.version)
        );
    }

    Ok(EXIT_OK)
}

/// `plugin search` -- search crates.io.
pub(super) fn plugin_search(args: &Cli, keyword: &str, limit: usize) -> crate::error::Result<u8> {
    let session = app::session(args)?;

    // The user types the short name (`cargo`) while crates.io has `pmpx-plugin-cargo`. Search
    // the short name directly: crates.io search is full text, so the `pmpx-plugin-` prefix is
    // not a keyword.
    let results = session.kit.search(keyword, limit)?;

    if results.is_empty() {
        anstream::println!("crates.io has no crate matching {keyword:?}.");
        return Ok(EXIT_OK);
    }

    for r in results {
        anstream::println!(
            "{:<32} v{} ↓{}",
            r.name,
            style::padded(style::DIM, 10, &r.version),
            r.downloads
        );
        if let Some(d) = r.description {
            anstream::println!("  {}", style::paint(style::DIM, d));
        }
    }

    Ok(EXIT_OK)
}

/// `plugin info <name>`: local install state plus crates.io information.
pub(super) fn plugin_info(args: &Cli, name: &str) -> crate::error::Result<u8> {
    let session = app::session(args)?;

    let mut found = false;

    if let Some(p) = session.plugins.by_name(name) {
        found = true;
        anstream::println!("Installed");
        anstream::println!(
            "  {} {}",
            style::label(15, "reported name"),
            style::paint(style::PM, &p.name)
        );
        anstream::println!("  {} {}", style::label(15, "crate"), p.crate_name);
        anstream::println!(
            "  {} {}",
            style::label(15, "version"),
            style::paint(style::DIM, &p.version)
        );
        anstream::println!(
            "  {} {}",
            style::label(15, "family"),
            p.family
                .as_ref()
                .map(Family::as_str)
                .unwrap_or("(not declared)")
        );
        anstream::println!(
            "  {} {}",
            style::label(15, "ABI"),
            p.abi
                .map(|a| a.to_string())
                .unwrap_or_else(|| "(not declared)".into())
        );
        anstream::println!(
            "  {} {}",
            style::label(15, "directory"),
            style::paint(style::DIM, p.dir.display())
        );
        anstream::println!(
            "  {} {}",
            style::label(15, "strong evidence"),
            style::paint(style::DIM, join_or_dash(&p.strong))
        );
        anstream::println!(
            "  {} {}",
            style::label(15, "weak evidence"),
            style::paint(style::DIM, join_or_dash(&p.weak))
        );
        if let Some(why) = p.problem() {
            anstream::println!("  {}", style::paint(style::DIM, format!("⚠ {why}")));
        }
        anstream::println!();

        // The plugin record we already have is enough to load it -- no need to dress it up as a
        // detection result.
        if let Ok(backend) = app::load_plugin(&session, p) {
            let (rustc_version, target) = backend.build_info();
            anstream::println!("Plugin reports");
            anstream::println!(
                "  {} {}",
                style::label(15, "name"),
                style::paint(style::PM, backend.name())
            );
            anstream::println!("  {} {}", style::label(15, "family"), backend.family());
            anstream::println!(
                "  {} {}",
                style::label(15, "compiled with"),
                style::paint(style::DIM, rustc_version)
            );
            anstream::println!(
                "  {} {}",
                style::label(15, "target"),
                style::paint(style::DIM, target)
            );
            anstream::println!();
        }
    }

    // The crates.io side
    let crate_name = session.kit.config().normalize_crate_name(name);
    match session.kit.view(&crate_name) {
        Ok(Some(info)) => {
            found = true;
            anstream::println!("crates.io");
            anstream::println!("  {} {}", style::label(15, "crate"), info.name);
            anstream::println!(
                "  {} {}",
                style::label(15, "latest"),
                style::paint(style::DIM, info.version)
            );
            if let Some(d) = info.description {
                anstream::println!(
                    "  {} {}",
                    style::label(15, "description"),
                    style::paint(style::DIM, d)
                );
            }
            if let Some(r) = info.repository {
                anstream::println!(
                    "  {} {}",
                    style::label(15, "repository"),
                    style::paint(style::DIM, r)
                );
            }
        }
        Ok(None) => {
            if !found {
                anstream::println!("crates.io has no {crate_name}.");
            }
        }
        Err(e) => {
            // Being offline or having no network must not hide the local information
            crate::error::error_line(format!("crates.io lookup failed: {e}"));
        }
    }

    if !found {
        return Err(PmpxError::not_found(format!("no plugin named {name}.")));
    }

    Ok(EXIT_OK)
}

pub(super) fn join_or_dash(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".to_string()
    } else {
        items.join(", ")
    }
}

pub(super) fn describe_source(source: crate_plugin_kit::cache::InstallSource) -> &'static str {
    match source {
        crate_plugin_kit::cache::InstallSource::Prebuilt => "prebuilt",
        crate_plugin_kit::cache::InstallSource::BuildHost => "built locally",
    }
}

// config

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_or_dash_handles_the_empty_case() {
        assert_eq!(join_or_dash(&[]), "(none)");
        assert_eq!(join_or_dash(&["a".into(), "b".into()]), "a, b");
    }
}
