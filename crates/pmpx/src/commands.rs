//! Subcommand handling.
//!
//! This layer only does "read the context -> do the work -> print"; the decision logic lives
//! in [`crate::app`] and below.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use clap::CommandFactory;
use pmpx_plugin::Family;

use crate::app::{self, Session};
use crate::cli::{Cli, Command, ConfigCommand, PluginCommand};
use crate::config::ProjectConfig;
use crate::detect::{self, FamilyScore, ScoredPlugin};
use crate::error::{PmpxError, EXIT_OK};

/// Dispatch by argv.
pub fn dispatch(args: &Cli) -> crate::error::Result<u8> {
    let Some(command) = &args.command else {
        return show_detection(args);
    };

    match command {
        Command::Install { packages } => {
            run_verb(args, pmpx_plugin::Verb::Install, packages.clone(), false)
        }
        Command::Remove { packages } => {
            run_verb(args, pmpx_plugin::Verb::Remove, packages.clone(), false)
        }
        Command::Update { packages } => {
            run_verb(args, pmpx_plugin::Verb::Update, packages.clone(), false)
        }
        Command::Build { args: rest } => {
            run_verb(args, pmpx_plugin::Verb::Build, rest.clone(), false)
        }
        Command::Test { args: rest } => {
            run_verb(args, pmpx_plugin::Verb::Test, rest.clone(), false)
        }

        Command::Run { target, args: rest } => {
            // `pmpx run <target> -- <args>`: the target and what follows `--` go to the
            // plugin together; how to arrange them is up to the plugin (cargo, for example,
            // uses `cargo run -- ...`).
            let argv: Vec<OsString> = target.iter().cloned().chain(rest.iter().cloned()).collect();
            run_verb(args, pmpx_plugin::Verb::Run, argv, false)
        }

        // `exec` is the only verb allowed to degrade
        Command::Exec { command } => run_verb(args, pmpx_plugin::Verb::Exec, command.clone(), true),

        Command::Info => info(args),

        Command::Plugin(sub) => plugin_cmd(args, sub),
        Command::Config(sub) => config_cmd(sub),
        Command::Completion { shell } => completion(*shell),
    }
}

/// The shared verb entry point: open a session and hand over to [`app::run_verb`].
fn run_verb(
    args: &Cli,
    verb: pmpx_plugin::Verb,
    argv: Vec<OsString>,
    allow_exec_fallback: bool,
) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;
    app::run_verb(&session, verb, &argv, allow_exec_fallback)
}

// ---------------------------------------------------------------------------
// No subcommand: show the detection result
// ---------------------------------------------------------------------------

/// Bare `pmpx`: state what this directory is and which backend will run.
fn show_detection(args: &Cli) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;

    let Some(root) = session.project_root().map(PathBuf::from) else {
        return Err(session.no_project_error());
    };

    let selection = session.select(&root).map_err(PmpxError::from)?;

    session.emit_notes(&selection);

    println!("{:<12}  {}", "Project root", root.display());
    println!("{:<12}  {}", "Family", selection.family.display());
    println!(
        "{:<12}  {} (score {})",
        "Plugin", selection.name, selection.score
    );
    println!();
    println!("Use `pmpx info` to see every candidate and score.");

    Ok(EXIT_OK)
}

// ---------------------------------------------------------------------------
// info
// ---------------------------------------------------------------------------

/// `pmpx info`: lay out the whole detection process.
///
/// Like `plugin current`, it always lists every candidate and score -- that is what makes
/// ambiguity visible.
fn info(args: &Cli) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;

    println!("{:<12}  {}", "Start", session.start_dir.display());
    match session.project_root() {
        Some(root) => println!("{:<12}  {}", "Project root", root.display()),
        None => println!("{:<12}  (not found)", "Project root"),
    }
    println!(
        "{:<12}  {}, stopped because: {}",
        "Walk-up",
        crate::discovery::dirs(session.walk.dirs.len()),
        session
            .walk
            .stopped
            .describe(session.global.discovery.max_depth)
    );

    if session.project.sources.is_empty() {
        println!("{:<12}  (none)", "Project config");
    } else {
        println!("{:<12}  (nearest first, nearest wins)", "Project config");
        for p in &session.project.sources {
            println!("              {}", p.display());
        }
    }
    for (family, plugin) in &session.project.plugin {
        println!("  {:<10}  {family} = \"{plugin}\"", "pinned");
    }

    println!();

    if session.plugins.plugins.is_empty() {
        println!("{:<12}  (none)", "Installed plugins");
        println!();
        println!("{}", session.no_project_error());
        return Ok(EXIT_OK);
    }

    println!("Installed plugins");
    for p in &session.plugins.plugins {
        match p.problem() {
            None => println!(
                "  {:<10} {:<8} v{}",
                p.name,
                p.family.as_ref().map(Family::as_str).unwrap_or("?"),
                p.version
            ),
            Some(why) => println!("  {:<10} ⚠ {why}", p.name),
        }
    }
    println!();

    let Some(root) = session.project_root().map(PathBuf::from) else {
        println!("(no project root, cannot score)");
        return Ok(EXIT_OK);
    };

    let families = detect::score_all(&session.plugins, &root, &session.project);

    if families.is_empty() {
        println!(
            "{:<12}  (no plugin can take part in the resolution)",
            "Candidates"
        );
        return Ok(EXIT_OK);
    }

    println!("Candidates and scores");
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
        println!("  {}  score {}{pin}", fs.family.display(), fs.score);

        for p in &fs.plugins {
            let hits = p.all_hits().collect::<Vec<_>>().join(", ");
            let detail = if hits.is_empty() {
                "(no match)".to_string()
            } else {
                hits
            };
            println!("    {:<8} score {:>4}   {detail}", p.name, p.score);
        }
    }
    println!();

    // The resolution result
    match session.select(&root) {
        Ok(selection) => {
            session.emit_notes(&selection);
            println!(
                "{:<12}  {} ({})",
                "Selected", selection.name, selection.crate_name
            );

            match session.load_backend(&selection) {
                Ok(backend) => {
                    let d = backend.diagnostics();
                    println!("  {:<15} {}", "reported name", d.name);
                    println!("  {:<15} {}", "reported family", d.family);
                    println!("  {:<15} {}", "compiled with", d.rustc_version);
                    println!("  {:<15} {}", "target", d.target);

                    if d.family != selection.family.as_str() {
                        println!(
                            "  ⚠ the manifest says it is {}, it says it is {} -- was the \
                             manifest edited?",
                            selection.family.as_str(),
                            d.family
                        );
                    }
                }
                Err(e) => println!("  ⚠ failed to load: {e}"),
            }
        }
        Err(failure) => println!(
            "{:<12}  (nothing selected)\n{}",
            "Selected",
            failure.message()
        ),
    }

    Ok(EXIT_OK)
}

// ---------------------------------------------------------------------------
// plugin
// ---------------------------------------------------------------------------

fn plugin_cmd(args: &Cli, cmd: &PluginCommand) -> crate::error::Result<u8> {
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
fn plugin_ls(args: &Cli, flat: bool) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;

    if session.plugins.plugins.is_empty() {
        println!("No plugins are installed.");
        println!();
        println!("Install one with `pmpx plugin add <name>`, for example `pmpx plugin add cargo`.");
        return Ok(EXIT_OK);
    }

    if flat {
        for p in &session.plugins.plugins {
            print_plugin_row(p);
        }
        return Ok(EXIT_OK);
    }

    let mut families: Vec<Option<Family>> = session
        .plugins
        .plugins
        .iter()
        .map(|p| p.family.clone())
        .collect();
    families.sort_by(|a, b| match (a, b) {
        (Some(x), Some(y)) => x.as_str().cmp(y.as_str()),
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, None) => std::cmp::Ordering::Equal,
    });
    families.dedup();

    for family in families {
        let title = family
            .as_ref()
            .map(Family::display)
            .unwrap_or("(no family declared)");
        println!("{title}");
        for p in session
            .plugins
            .plugins
            .iter()
            .filter(|p| p.family == family)
        {
            print!("  ");
            print_plugin_row(p);
        }
    }

    Ok(EXIT_OK)
}

fn print_plugin_row(p: &crate::plugins::InstalledPlugin) {
    match p.problem() {
        None => println!(
            "{:<10} v{:<10} {:<28} {}",
            p.name,
            p.version,
            p.crate_name,
            p.dir.display()
        ),
        Some(why) => println!("{:<10} ⚠ {why}", p.name),
    }
}

/// `plugin current`: the current plugin per family, plus candidates and scores.
fn plugin_current(args: &Cli) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;
    let Some(root) = session.project_root().map(PathBuf::from) else {
        return Err(session.no_project_error());
    };

    let families = detect::score_all(&session.plugins, &root, &session.project);
    if families.is_empty() {
        println!("No plugin can take part in the resolution.");
        return Ok(EXIT_OK);
    }

    let selection = session.select(&root).ok();

    for (family, fs) in &families {
        let current = selection
            .as_ref()
            .filter(|s| &s.family == family)
            .map(|s| s.name.as_str());

        println!("{}", family.display());
        let mut ranked: Vec<&ScoredPlugin> = fs.plugins.iter().collect();
        ranked.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.name.cmp(&b.name)));

        for p in ranked {
            let mark = if Some(p.name.as_str()) == current {
                "←"
            } else {
                " "
            };
            println!("  {} {:<8} score {:>4}", mark, p.name, p.score);
        }
        if let Some(pinned) = session.project.pinned_plugin(family.as_str()) {
            println!("  pinned: {pinned}");
        }
        println!();
    }

    Ok(EXIT_OK)
}

/// `plugin set <name>`: pin it in the nearest layer of `.pmpx.toml`.
fn plugin_set(args: &Cli, name: &str) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;

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
    edit_project_config(&path, |cfg| {
        cfg.plugin
            .insert(family.as_str().to_string(), plugin.name.clone());
    })
    .map_err(PmpxError::Other)?;

    println!(
        "Pinned {} = \"{}\" in {}",
        family.as_str(),
        plugin.name,
        path.display()
    );
    Ok(EXIT_OK)
}

/// `plugin unset [family] [--yes]`
fn plugin_unset(args: &Cli, family: Option<&str>, yes: bool) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;
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
            edit_project_config(&path, |cfg| {
                cfg.plugin.remove(f);
            })
            .map_err(PmpxError::Other)?;
            println!("Removed the pin for {f} from {}", path.display());
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
            edit_project_config(&path, |cfg| {
                cfg.plugin.clear();
            })
            .map_err(PmpxError::Other)?;
            println!(
                "Removed the pins for {} from {}",
                removed.join(", "),
                path.display()
            );
        }
    }

    Ok(EXIT_OK)
}

/// Which `.pmpx.toml` to write.
///
/// The nearest layer wins: an existing file is the layer the user chose; when there is none,
/// write at the project root.
fn target_config_path(session: &Session) -> PathBuf {
    // `walk.dirs` goes from nearest to farthest
    for dir in &session.walk.dirs {
        let p = dir.join(".pmpx.toml");
        if p.is_file() {
            return p;
        }
    }

    let base = session
        .project_root()
        .map(PathBuf::from)
        .unwrap_or_else(|| session.start_dir.clone());
    base.join(".pmpx.toml")
}

/// Read-modify-write one `.pmpx.toml`, keeping keys it does not recognise.
fn edit_project_config(path: &Path, edit: impl FnOnce(&mut ProjectConfig)) -> Result<()> {
    let mut cfg = ProjectConfig::load_from(path)?.unwrap_or_default();
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
    std::fs::write(path, text)
        .with_context(|| format!("failed to write the project config: {}", path.display()))
}

/// `plugin add`
fn plugin_add(args: &Cli, names: &[String], version: Option<&str>) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;

    if version.is_some() && names.len() > 1 {
        return Err(PmpxError::Usage(
            "--version can only be used with a single plugin name".to_string(),
        ));
    }

    for name in names {
        println!("Installing {name}...");
        let installed = session
            .kit
            .install(name, version)
            .map_err(|e| PmpxError::Other(anyhow::anyhow!("{e}")))?;

        println!(
            "  {} v{} ({}) -> {}",
            installed.crate_name,
            installed.version,
            describe_source(installed.source),
            installed.dir.display()
        );
    }

    Ok(EXIT_OK)
}

/// `plugin rm`
fn plugin_rm(args: &Cli, names: &[String]) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;

    for name in names {
        session
            .kit
            .uninstall(name)
            .map_err(|e| PmpxError::Other(anyhow::anyhow!("{e}")))?;
        println!("Removed {name}");
    }

    Ok(EXIT_OK)
}

/// `plugin update`
fn plugin_update(args: &Cli, names: &[String], version: Option<&str>) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;

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
        println!("No plugins to update.");
        return Ok(EXIT_OK);
    }

    for name in targets {
        let installed = session
            .kit
            .update(&name, version)
            .map_err(|e| PmpxError::Other(anyhow::anyhow!("{e}")))?;
        println!("{} -> v{}", installed.crate_name, installed.version);
    }

    Ok(EXIT_OK)
}

/// `plugin search` -- search crates.io.
fn plugin_search(args: &Cli, keyword: &str, limit: usize) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;

    // The user types the short name (`cargo`) while crates.io has `pmpx-plugin-cargo`. Search
    // the short name directly: crates.io search is full text, so the `pmpx-plugin-` prefix is
    // not a keyword.
    let results = session
        .kit
        .search(keyword, limit)
        .map_err(|e| PmpxError::Other(anyhow::anyhow!("{e}")))?;

    if results.is_empty() {
        println!("crates.io has no crate matching {keyword:?}.");
        return Ok(EXIT_OK);
    }

    for r in results {
        println!("{:<32} v{:<10} ↓{}", r.name, r.version, r.downloads);
        if let Some(d) = r.description {
            println!("  {d}");
        }
    }

    Ok(EXIT_OK)
}

/// `plugin info <name>`: local install state plus crates.io information.
fn plugin_info(args: &Cli, name: &str) -> crate::error::Result<u8> {
    let session = Session::open(args).map_err(PmpxError::Other)?;

    let mut found = false;

    if let Some(p) = session.plugins.by_name(name) {
        found = true;
        println!("Installed");
        println!("  {:<15} {}", "reported name", p.name);
        println!("  {:<15} {}", "crate", p.crate_name);
        println!("  {:<15} {}", "version", p.version);
        println!(
            "  {:<15} {}",
            "family",
            p.family
                .as_ref()
                .map(Family::as_str)
                .unwrap_or("(not declared)")
        );
        println!(
            "  {:<15} {}",
            "ABI",
            p.abi
                .map(|a| a.to_string())
                .unwrap_or_else(|| "(not declared)".into())
        );
        println!("  {:<15} {}", "directory", p.dir.display());
        println!("  {:<15} {}", "strong evidence", join_or_dash(&p.strong));
        println!("  {:<15} {}", "weak evidence", join_or_dash(&p.weak));
        if let Some(why) = p.problem() {
            println!("  ⚠ {why}");
        }
        println!();

        if let Ok(backend) = session.load_backend(&crate::detect::Selection {
            crate_name: p.crate_name.clone(),
            name: p.name.clone(),
            family: p.family.clone().unwrap_or_else(|| Family::new("unknown")),
            score: 0,
            notes: Vec::new(),
        }) {
            let d = backend.diagnostics();
            println!("Plugin reports");
            println!("  {:<15} {}", "name", d.name);
            println!("  {:<15} {}", "family", d.family);
            println!("  {:<15} {}", "compiled with", d.rustc_version);
            println!("  {:<15} {}", "target", d.target);
            println!();
        }
    }

    // The crates.io side
    let crate_name = session.kit.config().normalize_crate_name(name);
    match session.kit.view(&crate_name) {
        Ok(Some(info)) => {
            found = true;
            println!("crates.io");
            println!("  {:<15} {}", "crate", info.name);
            println!("  {:<15} {}", "latest", info.version);
            if let Some(d) = info.description {
                println!("  {:<15} {d}", "description");
            }
            if let Some(r) = info.repository {
                println!("  {:<15} {r}", "repository");
            }
        }
        Ok(None) => {
            if !found {
                println!("crates.io has no {crate_name}.");
            }
        }
        Err(e) => {
            // Being offline or having no network must not hide the local information
            eprintln!("pmpx: crates.io lookup failed: {e}");
        }
    }

    if !found {
        return Err(PmpxError::not_found(format!("no plugin named {name}.")));
    }

    Ok(EXIT_OK)
}

fn join_or_dash(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".to_string()
    } else {
        items.join(", ")
    }
}

fn describe_source(source: crate_plugin_kit::cache::InstallSource) -> &'static str {
    match source {
        crate_plugin_kit::cache::InstallSource::Prebuilt => "prebuilt",
        crate_plugin_kit::cache::InstallSource::BuildHost => "built locally",
    }
}

// ---------------------------------------------------------------------------
// config
// ---------------------------------------------------------------------------

fn config_cmd(cmd: &ConfigCommand) -> crate::error::Result<u8> {
    let path = crate::config::global_config_path().map_err(PmpxError::Other)?;

    match cmd {
        ConfigCommand::Get { key } => {
            let doc = read_toml_table(&path).map_err(PmpxError::Other)?;
            match lookup_dotted(&doc, key) {
                Some(v) => {
                    println!("{}", render_value(v));
                    Ok(EXIT_OK)
                }
                None => Err(PmpxError::Usage(format!(
                    "{} has no {key} (config file: {})",
                    "global config",
                    path.display()
                ))),
            }
        }

        ConfigCommand::Set { key, value } => {
            let mut doc = read_toml_table(&path).map_err(PmpxError::Other)?;
            insert_dotted(&mut doc, key, parse_value(value))
                .map_err(|e| PmpxError::Usage(e.to_string()))?;

            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| PmpxError::Other(e.into()))?;
            }
            let text = toml::to_string_pretty(&doc).map_err(|e| PmpxError::Other(e.into()))?;
            std::fs::write(&path, text).map_err(|e| PmpxError::Other(e.into()))?;

            println!(
                "Wrote {key} = {} to {}",
                render_value(&parse_value(value)),
                path.display()
            );
            Ok(EXIT_OK)
        }
    }
}

/// Read a TOML file into a table; a missing file = an empty table.
fn read_toml_table(path: &Path) -> Result<toml::Table> {
    match std::fs::read_to_string(path) {
        Ok(text) if !text.trim().is_empty() => {
            toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
        }
        Ok(_) => Ok(toml::Table::new()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(toml::Table::new()),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Look up a value by `a.b.c`.
fn lookup_dotted<'a>(table: &'a toml::Table, key: &str) -> Option<&'a toml::Value> {
    let mut parts = key.split('.');
    let first = parts.next()?;
    let mut current = table.get(first)?;

    for part in parts {
        current = current.as_table()?.get(part)?;
    }
    Some(current)
}

/// Write a value by `a.b.c`; missing intermediate tables are created.
fn insert_dotted(table: &mut toml::Table, key: &str, value: toml::Value) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.iter().any(|p| p.is_empty()) {
        anyhow::bail!("a key must not have empty segments: {key}");
    }

    let mut current = table;
    for part in &parts[..parts.len() - 1] {
        let entry = current
            .entry((*part).to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));

        current = entry
            .as_table_mut()
            .with_context(|| format!("{part} is not a table, cannot write into it"))?;
    }

    current.insert(parts[parts.len() - 1].to_string(), value);
    Ok(())
}

/// Parse a string from the command line into a TOML value.
///
/// The order is deliberately "most specific to most permissive": `true` -> integer -> array
/// -> string. So `pmpx config set x 123` stores a number; to store a string, write `"123"`
/// (with quotes).
fn parse_value(raw: &str) -> toml::Value {
    if raw == "true" {
        return toml::Value::Boolean(true);
    }
    if raw == "false" {
        return toml::Value::Boolean(false);
    }
    if let Ok(n) = raw.parse::<i64>() {
        return toml::Value::Integer(n);
    }

    // Arrays and quoted strings borrow TOML's own parser
    if let Ok(doc) = toml::from_str::<toml::Table>(&format!("v = {raw}")) {
        if let Some(v) = doc.get("v") {
            return v.clone();
        }
    }

    toml::Value::String(raw.to_string())
}

/// Print a TOML value. Strings get no quotes -- the user wants the value, not the syntax.
fn render_value(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        other => other.to_string().trim().to_string(),
    }
}

// ---------------------------------------------------------------------------
// completion
// ---------------------------------------------------------------------------

fn completion(shell: clap_complete::Shell) -> crate::error::Result<u8> {
    let mut cmd = Cli::command();
    let name = cmd.get_name().to_string();
    // Print to stdout and let the user redirect -- pmpx does not guess which file to write.
    clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
    Ok(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_value_recognises_booleans() {
        assert_eq!(parse_value("true"), toml::Value::Boolean(true));
        assert_eq!(parse_value("false"), toml::Value::Boolean(false));
    }

    #[test]
    fn parse_value_recognises_integers() {
        assert_eq!(parse_value("42"), toml::Value::Integer(42));
        assert_eq!(parse_value("-7"), toml::Value::Integer(-7));
    }

    #[test]
    fn parse_value_recognises_arrays() {
        let v = parse_value("[\"rust\", \"node\"]");
        let arr = v.as_array().expect("should be an array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].as_str(), Some("rust"));
    }

    #[test]
    fn parse_value_falls_back_to_a_string() {
        assert_eq!(parse_value("node"), toml::Value::String("node".to_string()));
        // A number stored as a string has to carry quotes
        assert_eq!(
            parse_value("\"123\""),
            toml::Value::String("123".to_string())
        );
    }

    #[test]
    fn render_value_omits_quotes_for_strings() {
        assert_eq!(render_value(&toml::Value::String("node".into())), "node");
        assert_eq!(render_value(&toml::Value::Integer(3)), "3");
        assert_eq!(render_value(&toml::Value::Boolean(true)), "true");
    }

    #[test]
    fn lookup_dotted_walks_tables() {
        let doc: toml::Table = toml::from_str(
            r#"
[plugin]
family_priority = ["node"]
[deep]
[deep.er]
x = 1
"#,
        )
        .unwrap();

        assert!(lookup_dotted(&doc, "plugin.family_priority").is_some());
        assert_eq!(
            lookup_dotted(&doc, "deep.er.x"),
            Some(&toml::Value::Integer(1))
        );
        assert!(lookup_dotted(&doc, "plugin.nope").is_none());
        assert!(lookup_dotted(&doc, "nope.at.all").is_none());
    }

    #[test]
    fn insert_dotted_creates_missing_tables() {
        let mut doc = toml::Table::new();
        insert_dotted(&mut doc, "a.b.c", toml::Value::Integer(1)).unwrap();

        assert_eq!(lookup_dotted(&doc, "a.b.c"), Some(&toml::Value::Integer(1)));
    }

    #[test]
    fn insert_dotted_refuses_to_clobber_a_non_table() {
        let mut doc: toml::Table = toml::from_str("a = 1\n").unwrap();
        let err = insert_dotted(&mut doc, "a.b", toml::Value::Integer(2)).unwrap_err();
        assert!(err.to_string().contains("not a table"), "{err}");
    }

    #[test]
    fn insert_dotted_rejects_empty_segments() {
        let mut doc = toml::Table::new();
        assert!(insert_dotted(&mut doc, "a..b", toml::Value::Integer(1)).is_err());
        assert!(insert_dotted(&mut doc, "", toml::Value::Integer(1)).is_err());
    }

    #[test]
    fn read_toml_table_treats_a_missing_file_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let t = read_toml_table(&tmp.path().join("nope.toml")).unwrap();
        assert!(t.is_empty());
    }

    #[test]
    fn read_toml_table_treats_an_empty_file_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("empty.toml");
        std::fs::write(&p, "   \n").unwrap();
        assert!(read_toml_table(&p).unwrap().is_empty());
    }

    #[test]
    fn join_or_dash_handles_the_empty_case() {
        assert_eq!(join_or_dash(&[]), "(none)");
        assert_eq!(join_or_dash(&["a".into(), "b".into()]), "a, b");
    }
}
