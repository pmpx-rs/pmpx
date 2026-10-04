//! `plugin ls / current`: which plugin answers.
//!
//! Listing and scoring read manifests only -- no plugin code is loaded to decide anything
//! here. Pinning the answer (`plugin set / unset`) is the read-modify-write of `.pmpx.toml`,
//! and lives in the sibling `plugin_pin` module.

use std::collections::BTreeMap;
use std::path::PathBuf;

use pmpx_plugin::Family;

use crate::app::Session;
use crate::cli::{Cli, PluginCommand};
use crate::detect::{self, ScoredPlugin};
use crate::error::{PmpxError, EXIT_OK};
use crate::plugins::InstalledPlugin;
use crate::style;

use super::plugin_pin::{plugin_set, plugin_unset};
use super::plugin_store::{plugin_add, plugin_info, plugin_rm, plugin_search, plugin_update};

pub(super) fn plugin_cmd(args: &Cli, cmd: &PluginCommand) -> crate::error::Result<u8> {
    match cmd {
        PluginCommand::Ls { flat } => plugin_ls(args, *flat),
        PluginCommand::Current => plugin_current(args),
        PluginCommand::Set { name } => plugin_set(args, name),
        PluginCommand::Unset { family, yes } => plugin_unset(args, family.as_deref(), *yes),
        PluginCommand::Add { names, version } => plugin_add(args, names, version.as_deref()),
        PluginCommand::Rm { names } => plugin_rm(args, names),
        PluginCommand::Update { names, version } => plugin_update(args, names, version.as_deref()),
        PluginCommand::Search { keyword, limit } => plugin_search(args, keyword, *limit),
        PluginCommand::Info { name } => plugin_info(args, name),
    }
}

/// `plugin ls`: list grouped by family.
pub(super) fn plugin_ls(args: &Cli, flat: bool) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;

    if session.plugins.plugins.is_empty() {
        anstream::println!("No plugins are installed.");
        anstream::println!();
        anstream::println!(
            "{}",
            style::paint(
                style::DIM,
                "Install one with `pmpx plugin add <name>`, for example `pmpx plugin add cargo`."
            )
        );
        return Ok(EXIT_OK);
    }

    if flat {
        for p in &session.plugins.plugins {
            print_plugin_row(p);
        }
        return Ok(EXIT_OK);
    }

    // Group in one pass. `BTreeMap` supplies the family name order, and the bucket for plugins that
    // declare no family is printed last rather than wherever `Option`'s ordering would put it.
    let mut grouped: BTreeMap<Option<&Family>, Vec<&InstalledPlugin>> = BTreeMap::new();
    for p in &session.plugins.plugins {
        grouped.entry(p.family.as_ref()).or_default().push(p);
    }

    for (family, plugins) in &grouped {
        if let Some(family) = family {
            print_family_group(family.display(), plugins);
        }
    }
    if let Some(plugins) = grouped.get(&None) {
        print_family_group("(no family declared)", plugins);
    }

    Ok(EXIT_OK)
}

/// One `plugin ls` section: a header and its rows, indented by one level.
pub(super) fn print_family_group(title: &str, plugins: &[&InstalledPlugin]) {
    anstream::println!("{}", style::paint(style::LABEL, title));
    for p in plugins {
        anstream::print!("  ");
        print_plugin_row(p);
    }
}

pub(super) fn print_plugin_row(p: &InstalledPlugin) {
    match p.problem() {
        None => anstream::println!(
            "{} v{} {:<28} {}",
            style::padded(style::PM, 10, &p.name),
            style::padded(style::DIM, 10, &p.version),
            p.crate_name,
            style::paint(style::DIM, p.dir.display())
        ),
        Some(why) => anstream::println!(
            "{} ⚠ {}",
            style::padded(style::PM, 10, &p.name),
            style::paint(style::DIM, why)
        ),
    }
}

/// `plugin current`: the current plugin per family, plus candidates and scores.
pub(super) fn plugin_current(args: &Cli) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;
    let Some(root) = session.project_root().map(PathBuf::from) else {
        return Err(session.no_project_error());
    };

    let families = detect::score_all(&session.plugins, &root, &session.project);
    if families.is_empty() {
        anstream::println!("No plugin can take part in the resolution.");
        return Ok(EXIT_OK);
    }

    let selection = session.select_from(&root, &families).ok();

    for (family, fs) in &families {
        let current = selection
            .as_ref()
            .filter(|s| &s.family == family)
            .map(|s| s.name.as_str());

        anstream::println!("{}", style::paint(style::LABEL, family.display()));
        let mut ranked: Vec<&ScoredPlugin> = fs.plugins.iter().collect();
        ranked.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.name.cmp(&b.name)));

        for p in ranked {
            let mark = if Some(p.name.as_str()) == current {
                "←"
            } else {
                " "
            };
            anstream::println!(
                "  {} {} score {:>4}",
                mark,
                style::padded(style::PM, 8, &p.name),
                p.score
            );
        }
        if let Some(pinned) = session.project.pinned_plugin(family.as_str()) {
            anstream::println!(
                "  {} {}",
                style::paint(style::LABEL, "pinned:"),
                style::paint(style::PM, pinned)
            );
        }
        anstream::println!();
    }

    Ok(EXIT_OK)
}
