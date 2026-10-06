//! What pmpx sees in this directory: nothing but a report, and `info`.
//!
//! Neither of these spawns anything. `info` is the one command that loads the winning plugin
//! without running it, because the diagnostics it prints (the version and target the plugin
//! was built with) only exist inside the loaded library.

use std::path::PathBuf;

use pmpx_plugin::Family;

use crate::app;
use crate::cli::Cli;
use crate::detect_types::FamilyScore;
use crate::error::EXIT_OK;
use crate::style;

/// Bare `pmpx`: state what this directory is and which backend will run.
pub(super) fn show_detection(args: &Cli) -> crate::error::Result<u8> {
    let session = app::session(args)?;

    let Some(root) = session.project_root().map(PathBuf::from) else {
        return Err(session.no_project_error().into());
    };

    let selection = app::select(&session, &root)?;

    app::emit_notes(&session, &selection);

    anstream::println!(
        "{}  {}",
        style::label(12, "Project root"),
        style::paint(style::DIM, root.display())
    );
    anstream::println!(
        "{}  {}",
        style::label(12, "Family"),
        style::family_label(selection.family.as_str())
    );
    anstream::println!(
        "{}  {} (score {})",
        style::label(12, "Plugin"),
        style::paint(style::PM, &selection.name),
        selection.score
    );
    anstream::println!();
    anstream::println!(
        "{}",
        style::paint(
            style::DIM,
            "Use `pmpx info` to see every candidate and score."
        )
    );

    Ok(EXIT_OK)
}

// info

/// `pmpx info`: lay out the whole detection process.
///
/// Like `plugin current`, it always lists every candidate and score -- that is what makes
/// ambiguity visible.
pub(super) fn show_info(args: &Cli) -> crate::error::Result<u8> {
    let session = app::session(args)?;

    anstream::println!(
        "{}  {}",
        style::label(12, "Start"),
        style::paint(style::DIM, session.start_dir.display())
    );
    match session.project_root() {
        Some(root) => anstream::println!(
            "{}  {}",
            style::label(12, "Project root"),
            style::paint(style::DIM, root.display())
        ),
        None => anstream::println!("{}  (not found)", style::label(12, "Project root")),
    }
    anstream::println!(
        "{}  {}, stopped because: {}",
        style::label(12, "Walk-up"),
        pmpx_engine::discovery::dirs(session.walk.dirs.len()),
        session
            .walk
            .stopped
            .describe(session.global.discovery.max_depth)
    );

    if session.project.sources.is_empty() {
        anstream::println!("{}  (none)", style::label(12, "Project config"));
    } else {
        anstream::println!(
            "{}  (nearest first, nearest wins)",
            style::label(12, "Project config")
        );
        for p in &session.project.sources {
            anstream::println!("              {}", style::paint(style::DIM, p.display()));
        }
    }
    for (family, plugin) in &session.project.plugin {
        anstream::println!(
            "  {}  {family} = \"{}\"",
            style::label(10, "pinned"),
            style::paint(style::PM, plugin)
        );
    }

    anstream::println!();

    if session.plugins.plugins.is_empty() {
        anstream::println!("{}  (none)", style::label(12, "Installed plugins"));
        anstream::println!();
        anstream::println!(
            "{}",
            style::paint(style::ERROR_BODY, session.no_project_error())
        );
        return Ok(EXIT_OK);
    }

    anstream::println!("Installed plugins");
    for p in &session.plugins.plugins {
        match p.problem() {
            None => anstream::println!(
                "  {} {:<8} v{}",
                style::padded(style::PM, 10, p.name()),
                p.family.as_ref().map(Family::as_str).unwrap_or("?"),
                style::paint(style::DIM, &p.version())
            ),
            Some(why) => anstream::println!(
                "  {} ⚠ {}",
                style::padded(style::PM, 10, p.name()),
                style::paint(style::DIM, why)
            ),
        }
    }
    anstream::println!();

    let Some(root) = session.project_root().map(PathBuf::from) else {
        anstream::println!(
            "{}",
            style::paint(style::DIM, "(no project root, cannot score)")
        );
        return Ok(EXIT_OK);
    };

    let families = app::score_all(&session, &root);

    if families.is_empty() {
        anstream::println!(
            "{}  (no plugin can take part in the resolution)",
            style::label(12, "Candidates")
        );
        return Ok(EXIT_OK);
    }

    anstream::println!("Candidates and scores");
    let mut rows: Vec<&FamilyScore> = families.values().collect();
    rows.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.family.as_str().cmp(b.family.as_str()))
    });

    for fs in rows {
        let pin = if fs.pinned {
            "  [pinned in .pmpx.toml]"
        } else {
            ""
        };
        anstream::println!(
            "  {}  score {}{pin}",
            style::family_label(fs.family.as_str()),
            fs.score
        );

        for p in &fs.plugins {
            let hits = p.all_hits().collect::<Vec<_>>().join(", ");
            let detail = if hits.is_empty() {
                "(no match)".to_string()
            } else {
                hits
            };
            anstream::println!(
                "    {} score {:>4}   {}",
                style::padded(style::PM, 8, &p.name),
                p.score,
                style::paint(style::DIM, detail)
            );
        }
    }
    anstream::println!();

    // The resolution result. The scores below are the ones this command already computed, so the
    // decision costs no further filesystem work.
    match app::select_from(&session, &root, &families) {
        Ok(selection) => {
            app::emit_notes(&session, &selection);
            anstream::println!(
                "{}  {} ({})",
                style::label(12, "Selected"),
                style::paint(style::PM, &selection.name),
                selection.crate_name
            );

            match app::load_backend(&session, &selection) {
                Ok(backend) => {
                    let (rustc_version, target) = backend.build_info();
                    anstream::println!(
                        "  {} {}",
                        style::label(15, "reported name"),
                        style::paint(style::PM, backend.name())
                    );
                    anstream::println!(
                        "  {} {}",
                        style::label(15, "reported family"),
                        backend.family()
                    );
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

                    if backend.family() != selection.family.as_str() {
                        anstream::println!(
                            "  {}",
                            style::paint(
                                style::DIM,
                                format!(
                                    "⚠ the manifest says it is {}, it says it is {} -- was the \
                                     manifest edited?",
                                    selection.family.as_str(),
                                    backend.family()
                                )
                            )
                        );
                    }
                }
                Err(e) => anstream::println!(
                    "  {}",
                    style::paint(style::DIM, format!("⚠ failed to load: {e}"))
                ),
            }
        }
        Err(failure) => anstream::println!(
            "{}  (nothing selected)\n{}",
            style::label(12, "Selected"),
            style::paint(style::ERROR_BODY, failure)
        ),
    }

    Ok(EXIT_OK)
}

// plugin
