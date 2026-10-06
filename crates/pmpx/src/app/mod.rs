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
        // The workspace carries a single version, so this is also the contract's version.
        contract_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Open a session, showing the setup phases in the trace.
pub fn session(args: &Cli) -> crate::error::Result<Session> {
    let mut sink = Sink::new();
    Session::open(options(args), &mut |event| sink.handle(event)).map_err(PmpxError::from)
}

/// The engine's events, as this host shows them.
///
/// It remembers which plugin the run selected, because the plugin's own lines arrive without a name and
/// the name is what makes them readable.
pub struct Sink {
    plugin: String,
}

impl Sink {
    /// A sink that does not know the plugin yet.
    pub fn new() -> Self {
        Self {
            plugin: String::new(),
        }
    }

    /// Show one event.
    pub fn handle(&mut self, event: Event) {
        if let Event::Notes { plugin, .. } = &event {
            self.plugin.clone_from(plugin);
        }
        crate::runtime::render(&self.plugin, event);
    }
}

impl Default for Sink {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve which plugin to use, showing the decision's phases.
pub fn select(
    session: &Session,
    root: &Path,
) -> std::result::Result<Selection, crate::error::PmpxError> {
    let mut sink = Sink::new();
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
    let mut sink = Sink::new();
    session
        .select_from(root, families, &mut |event| sink.handle(event))
        .map_err(|failure| PmpxError::from(pmpx_engine::EngineError::Detect(failure)))
}

/// Score every installed plugin, for a command that wants the whole table (`info`, `plugin ls`).
pub fn score_all(session: &Session, root: &Path) -> BTreeMap<String, FamilyScore> {
    let mut sink = Sink::new();
    pmpx_engine::detect::score_all(&session.plugins, root, &session.project, &mut |event| {
        sink.handle(event)
    })
}

/// Load the selected plugin.
pub fn load_backend(session: &Session, selection: &Selection) -> crate::error::Result<Backend> {
    let mut sink = Sink::new();
    session
        .load_backend(selection, &mut |event| sink.handle(event))
        .map_err(PmpxError::from)
}

/// Load one installed plugin, for a command that inspects plugins rather than running one.
pub fn load_plugin(session: &Session, plugin: &InstalledPlugin) -> crate::error::Result<Backend> {
    let mut sink = Sink::new();
    session
        .load_plugin(plugin, &mut |event| sink.handle(event))
        .map_err(PmpxError::from)
}

/// Show the notes a resolution produced. `--quiet` turns them off.
pub fn emit_notes(session: &Session, selection: &Selection) {
    let mut sink = Sink::new();
    sink.handle(Event::Notes {
        plugin: selection.name.clone(),
        notes: selection.notes.clone(),
    });
    let _ = session;
}

/// Run one verb all the way.
pub fn run_verb(
    session: &Session,
    verb: Verb,
    args: &[OsString],
    allow_exec_fallback: bool,
) -> crate::error::Result<u8> {
    let mut sink = Sink::new();
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
    let mut sink = Sink::new();
    pmpx_engine::run_script(session, name, extra, &mut |event| sink.handle(event))
        .map(|result| result.map_err(PmpxError::from))
}
