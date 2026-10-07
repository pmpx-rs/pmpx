//! The CLI's view of one run: the engine's session, plus the one thing the engine deliberately does not
//! do -- deciding what a person sees.
//!
//! Everything with a decision in it lives in `pmpx-engine`: reading the configuration, walking up, listing
//! the plugins, choosing one, loading it, asking it, running the answer. What stays here is the *sink*:
//! the engine reports [`Event`]s, and this module turns them into stderr through
//! [`crate::runtime::render`] and [`crate::debug`]. That is also why every wrapper below exists -- it is
//! the place where an event stream becomes output.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

use pmpx_engine::store::InstalledPlugin;
use pmpx_engine::Event;
pub use pmpx_engine::{Backend, Options, Session};
use pmpx_plugin::Verb;

use crate::cli::Cli;
use crate::debug;
use crate::detect_types::{FamilyScore, Selection};
use crate::error::PmpxError;

/// What the command line said, as the engine needs it.
pub fn options(args: &Cli) -> Options {
    Options {
        start_dir: args.dir.clone(),
        no_walk_up: args.no_walk_up,
        wanted_plugin: args.plugin.clone(),
        quiet: args.quiet,
        trace: debug::enabled(),
        // --json keeps stdout a JSON stream, so the backend writes to stderr instead.
        child_output_on_stderr: args.json,
        // The workspace carries a single version, so this is also the contract's version.
        contract_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Open a session, showing the setup phases in the trace.
pub fn session(args: &Cli) -> crate::error::Result<Session> {
    let mut sink = Sink::new(args.quiet);
    Session::open(options(args), &mut |event| sink.handle(event)).map_err(PmpxError::from)
}

/// The engine's events, as this host shows them.
///
/// It remembers which plugin the run selected, because the plugin's own lines arrive without a name and
/// the name is what makes them readable. The `--quiet` flag lives here too: the announcement at the
/// start of a run is the host's own presentation, and quiet is the host's own decision to keep it.
pub struct Sink {
    plugin: String,
    quiet: bool,
}

impl Sink {
    /// A sink that does not know the plugin yet. `quiet` is forwarded to the rendering rules:
    /// `--quiet` suppresses the `pmpx -> <command>` announcement and the selection's notes.
    pub fn new(quiet: bool) -> Self {
        Self {
            plugin: String::new(),
            quiet,
        }
    }

    /// Show one event.
    pub fn handle(&mut self, event: Event) {
        if let Event::Notes { plugin, .. } = &event {
            self.plugin.clone_from(plugin);
        }

        // The "pmpx -> <command>" announcement is the host's own framing, not part of the
        // engine's event rendering. JSON mode keeps stdout for machine-readable events, so
        // this only reaches the terminal in the human path. `--quiet` turns it off: it is the
        // "resolved command" the cli docs say quiet suppresses.
        if let Event::Starting { plan } = &event {
            if !self.quiet && !crate::runtime::is_json() {
                anstream::eprintln!("{}", crate::style::announce_starting(plan));
            }
        }

        // The selection's notes (ambiguous detection, uninstalled candidates) belong on stderr,
        // and `--quiet` is what hides them. The JSON path is unaffected: `notes` is its own
        // event and a script that wants them gets them either way.
        if self.quiet && matches!(event, Event::Notes { .. }) {
            return;
        }

        crate::runtime::render_event(&self.plugin, event);
    }
}

/// Resolve which plugin to use, showing the decision's phases.
pub fn select(
    session: &Session,
    root: &Path,
) -> std::result::Result<Selection, crate::error::PmpxError> {
    let mut sink = Sink::new(session.quiet);
    session
        .select(root, &mut |event| sink.handle(event))
        .map_err(|failure| PmpxError::from(pmpx_engine::EngineError::Detect(failure)))
}

/// Resolve from scores the caller already computed.
pub fn select_from(
    session: &Session,
    root: &Path,
    families: &BTreeMap<String, FamilyScore>,
) -> std::result::Result<Selection, crate::error::PmpxError> {
    let mut sink = Sink::new(session.quiet);
    session
        .select_from(root, families, &mut |event| sink.handle(event))
        .map_err(|failure| PmpxError::from(pmpx_engine::EngineError::Detect(failure)))
}

/// Score every installed plugin, for a command that wants the whole table (`info`, `plugin ls`).
pub fn score_all(session: &Session, root: &Path) -> BTreeMap<String, FamilyScore> {
    let mut sink = Sink::new(session.quiet);
    pmpx_engine::detect::score_all(&session.plugins, root, &session.project, &mut |event| {
        sink.handle(event)
    })
}

/// Load the selected plugin.
pub fn load_backend(session: &Session, selection: &Selection) -> crate::error::Result<Backend> {
    let mut sink = Sink::new(session.quiet);
    session
        .load_backend(selection, &mut |event| sink.handle(event))
        .map_err(PmpxError::from)
}

/// Load one installed plugin, for a command that inspects plugins rather than running one.
pub fn load_plugin(session: &Session, plugin: &InstalledPlugin) -> crate::error::Result<Backend> {
    let mut sink = Sink::new(session.quiet);
    session
        .load_plugin(plugin, &mut |event| sink.handle(event))
        .map_err(PmpxError::from)
}

/// Show the notes a resolution produced. `--quiet` turns them off.
pub fn emit_notes(session: &Session, selection: &Selection) {
    let mut sink = Sink::new(session.quiet);
    sink.handle(Event::Notes {
        plugin: selection.name.clone(),
        notes: selection.notes.clone(),
    });
}

/// Run one verb all the way.
pub fn run_verb(
    session: &Session,
    verb: Verb,
    args: &[OsString],
    allow_exec_fallback: bool,
) -> crate::error::Result<u8> {
    let mut sink = Sink::new(session.quiet);
    pmpx_engine::run_verb(session, verb, args, allow_exec_fallback, &mut |event| {
        sink.handle(event)
    })
    .map_err(PmpxError::from)
}

/// Run one of the project's own named commands, if it defines one.
pub fn run_script(
    session: &Session,
    name: &str,
    extra: &[OsString],
) -> Option<crate::error::Result<u8>> {
    let mut sink = Sink::new(session.quiet);
    pmpx_engine::run_script(session, name, extra, &mut |event| sink.handle(event))
        .map(|result| result.map_err(PmpxError::from))
}

/// `--explain`: what the decision was based on, and nothing run.
///
/// The interesting part of pmpx is invisible: which markers an installed plugin declares, which one
/// cannot take part and why, what each family scored, and what the decision did with all of that. This
/// prints it. With `--json` the same thing goes out as one object, because a program reads it too.
pub fn explain(args: &Cli) -> crate::error::Result<u8> {
    // The setup phases are a separate concern: unless `--debug` asked for them, they stay out of the
    // report, so `--explain --json` is exactly one object.
    let trace = debug::enabled();
    let mut sink = Sink::new(args.quiet);
    let session = Session::open(options(args), &mut |event| {
        if trace {
            sink.handle(event);
        }
    })
    .map_err(PmpxError::from)?;

    let root = session
        .project_root()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| session.start_dir.clone());
    // The engine is asked directly, with the same "only when tracing" rule as the setup above: the report
    // is the output, and a phase line is not part of it.
    let families =
        pmpx_engine::detect::score_all(&session.plugins, &root, &session.project, &mut |event| {
            if trace {
                sink.handle(event);
            }
        });

    // A refusal is a result here, not an error: "why is nothing selected" is the question this command
    // exists to answer.
    let outcome = session.project_root().map(|_| {
        session
            .select_from(&root, &families, &mut |event| {
                if trace {
                    sink.handle(event);
                }
            })
            .map_err(|failure| PmpxError::from(pmpx_engine::EngineError::Detect(failure)))
    });

    if crate::runtime::is_json() {
        let installed: Vec<serde_json::Value> = session
            .plugins
            .plugins
            .iter()
            .map(|plugin| {
                serde_json::json!({
                    "name": plugin.name(),
                    "crate_name": plugin.crate_name(),
                    "version": plugin.version(),
                    "family": plugin.family.as_ref().map(|f| f.as_str().to_string()),
                    "abi": plugin.abi(),
                    "problem": plugin.problem(),
                    "markers": plugin.detect_names().collect::<Vec<_>>(),
                })
            })
            .collect();
        let scores: Vec<serde_json::Value> = families
            .values()
            .map(|family| {
                serde_json::json!({
                    "family": family.family,
                    "score": family.score,
                    "pinned": family.pinned,
                    "plugins": family.plugins.len(),
                })
            })
            .collect();
        let chosen = match &outcome {
            Some(Ok(selection)) => serde_json::json!({
                "plugin": selection.name,
                "crate_name": selection.crate_name,
                "family": selection.family,
                "score": selection.score,
                "reason": format!("{:?}", selection.reason),
            }),
            Some(Err(error)) => serde_json::json!({ "refused": error.to_string() }),
            None => serde_json::Value::Null,
        };

        anstream::println!(
            "{}",
            serde_json::json!({
                "start_dir": session.start_dir.to_string_lossy(),
                "project_root": session.project_root().map(|p| p.to_string_lossy().into_owned()),
                "walked": session.walk.dirs.len(),
                "installed": installed,
                "scores": scores,
                "chosen": chosen,
            })
        );

        return Ok(crate::error::EXIT_OK);
    }

    anstream::println!(
        "{}",
        crate::style::paint(crate::style::DIM, "explain: nothing was run")
    );
    anstream::println!("{:<18}{}", "start directory", session.start_dir.display());
    anstream::println!(
        "{:<18}{}",
        "project root",
        session
            .project_root()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(none)".to_string())
    );
    anstream::println!("{:<18}{} directories", "walk-up", session.walk.dirs.len());

    anstream::println!("\ninstalled");
    if session.plugins.plugins.is_empty() {
        anstream::println!("  (none)");
    }
    for plugin in &session.plugins.plugins {
        anstream::println!(
            "  {} {} family={} abi={}{}",
            plugin.name(),
            crate::style::paint(crate::style::DIM, format!("v{}", plugin.version())),
            plugin.family.as_ref().map(|f| f.as_str()).unwrap_or("-"),
            plugin
                .abi()
                .map(|abi| abi.to_string())
                .unwrap_or_else(|| "-".to_string()),
            match plugin.problem() {
                Some(problem) => format!("  [{problem}]"),
                None => String::new(),
            }
        );
        let markers: Vec<&str> = plugin.detect_names().collect();
        if !markers.is_empty() {
            anstream::println!("      markers: {}", markers.join(", "));
        }
    }

    anstream::println!("\nscores");
    if families.is_empty() {
        anstream::println!("  (nothing scored)");
    }
    for family in families.values() {
        anstream::println!(
            "  {:<12}{}{}",
            family.family,
            family.score,
            if family.pinned { "  (pinned)" } else { "" }
        );
    }

    match &outcome {
        Some(Ok(selection)) => anstream::println!(
            "\nchosen             {} ({}) score {} -- {:?}",
            selection.name,
            selection.family,
            selection.score,
            selection.reason
        ),
        Some(Err(error)) => anstream::println!("\nchosen             (none) {error}"),
        None => anstream::println!("\nchosen             (none) no project root"),
    }

    Ok(crate::error::EXIT_OK)
}
