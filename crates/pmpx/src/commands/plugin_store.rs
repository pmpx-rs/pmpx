//! `plugin add / rm / update / search / info`: the plugin store and crates.io.
//!
//! Everything here goes through [`crate::app::Session::kit`], which owns the install lock,
//! the build-host path and the download path. This file is the reporting layer around it.

use pmpx_plugin::Family;

use std::path::{Path, PathBuf};

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
        // A path is installed the path way: `plugin add .` in a plugin checkout builds and installs
        // what is there, which is the development loop. Anything else is a plugin *name*, looked up
        // on crates.io.
        if let Some(dir) = checkout_of(name, session.manifest_name()) {
            anstream::println!("Installing {}...", style::paint(style::PM, name));
            match install_checkout(&session, &dir) {
                Ok(installed) => {
                    anstream::println!(
                        "  {} v{} ({}) -> {}",
                        installed.crate_name,
                        style::paint(style::DIM, installed.version),
                        style::paint(style::DIM, describe_source(&installed.source)),
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
                    return Err(e);
                }
            }
            continue;
        }

        anstream::println!("Installing {}...", style::paint(style::PM, name));

        match session.install(name, version) {
            Ok(installed) => {
                anstream::println!(
                    "  {} v{} ({}) -> {}",
                    installed.crate_name,
                    style::paint(style::DIM, installed.version),
                    style::paint(style::DIM, describe_source(&installed.source)),
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

/// The checkout a `plugin add` argument names, if it names one.
///
/// The rule is what the argument *is*, not how it is spelled: something that resolves to a
/// directory containing the manifest is a checkout, and everything else is a plugin name. That keeps
/// `plugin add pnpm` meaning what it always meant, while `plugin add .` and an absolute path in a
/// plugin repository do the obvious thing.
///
/// A path *spelling* that does not resolve is still a path, so the error can say "there is no such
/// directory" instead of "no such crate on crates.io".
fn checkout_of(name: &str, manifest: &str) -> Option<PathBuf> {
    let path = Path::new(name);

    if path.join(manifest).is_file() {
        return Some(path.to_path_buf());
    }

    let spelled_like_a_path = name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || path.is_absolute();

    if spelled_like_a_path && path.is_dir() {
        // A directory, but not a checkout: let the caller report that it is missing the manifest.
        return Some(path.to_path_buf());
    }

    None
}

/// Install one checkout, after checking it is something this host can use.
fn install_checkout(
    session: &app::Session,
    dir: &Path,
) -> crate::error::Result<pmpx_engine::store::Installed> {
    if !dir.is_dir() {
        return Err(PmpxError::Usage(format!(
            "{} is not a directory, so there is no plugin to install from it",
            dir.display()
        )));
    }

    vet_checkout(session, dir)?;

    Ok(session.install_from_path(dir)?)
}

/// What the store cannot check: whether *this host* will be able to use the checkout.
///
/// The reading is [`pmpx_engine::store::vet_checkout`]'s -- one place that knows the manifest, and it
/// never mentions the plugin system's own vocabulary. What a report *means* is here, because the words
/// are presentation: what cannot work is refused, and what will silently do nothing is said out loud,
/// since those are the failures an author would otherwise chase after installing.
fn vet_checkout(session: &app::Session, dir: &Path) -> crate::error::Result<()> {
    let report = pmpx_engine::store::vet_checkout(dir, session.manifest_name())?;

    // The one thing that cannot work: a plugin built against another contract version is refused at
    // `dlopen`, so installing it would only move the failure somewhere less obvious.
    match report.abi {
        Some(abi) if abi == pmpx_plugin::abi::PMPX_ABI_MAJOR => {}
        Some(abi) => {
            return Err(PmpxError::Usage(format!(
                "{} declares ABI {abi}, but this pmpx speaks ABI {}.\n\
                 Rebuild the plugin against pmpx-plugin {}.",
                dir.display(),
                pmpx_plugin::abi::PMPX_ABI_MAJOR,
                env!("CARGO_PKG_VERSION")
            )))
        }
        None => {
            return Err(PmpxError::Usage(format!(
                "{} does not declare an abi, so pmpx cannot tell whether it can load it",
                dir.display()
            )))
        }
    }

    // Everything below makes the plugin quietly do nothing. Saying so now is cheaper than finding
    // out why it never runs.
    if report.family.as_deref().unwrap_or("").is_empty() {
        crate::error::note_line(format!(
            "{} declares no family, so no project can ever select it",
            dir.display()
        ));
    }

    if report.markers.is_empty() {
        crate::error::note_line(format!(
            "{} declares no [detect] markers, so it will never match a project",
            dir.display()
        ));
    }

    Ok(())
}

/// `plugin rm`
pub(super) fn plugin_rm(args: &Cli, names: &[String]) -> crate::error::Result<u8> {
    let session = app::session(args)?;

    for name in names {
        session.uninstall(name)?;
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
        session
            .plugins
            .usable()
            .map(|p| p.name().to_string())
            .collect()
    } else {
        names.to_vec()
    };

    if targets.is_empty() {
        anstream::println!("No plugins to update.");
        return Ok(EXIT_OK);
    }

    for name in targets {
        let installed = session.update(&name, version)?;
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
    let results = session.search(keyword, limit)?;

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
            style::paint(style::PM, &p.name())
        );
        anstream::println!("  {} {}", style::label(15, "crate"), p.crate_name());
        anstream::println!(
            "  {} {}",
            style::label(15, "version"),
            style::paint(style::DIM, &p.version())
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
            p.abi()
                .map(|a| a.to_string())
                .unwrap_or_else(|| "(not declared)".into())
        );
        anstream::println!(
            "  {} {}",
            style::label(15, "directory"),
            style::paint(style::DIM, p.dir().display())
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
    let crate_name = session.normalize_crate_name(name);
    match session.view(&crate_name) {
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

/// How a plugin got here, in the words `plugin add` shows.
///
/// A local install names the directory it was built from: that is the thing the person
/// will want to know when they wonder why a plugin does not match what is published.
pub(super) fn describe_source(source: &pmpx_engine::store::InstallSource) -> String {
    use pmpx_engine::store::InstallSource;

    match source {
        InstallSource::Prebuilt => "prebuilt".to_string(),
        InstallSource::BuildHost => "built locally".to_string(),
        InstallSource::Local { path } => format!("built from {}", path.display()),
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
