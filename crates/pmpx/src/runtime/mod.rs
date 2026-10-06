//! What the engine reports, as this host presents it.
//!
//! The engine owns everything that talks to a plugin -- loading it, calling it, reading the files it
//! declared, and the hooks it logs through -- and prints nothing. This module is the other half: the
//! plugin's lines with its id, the notes worth showing, and trace detail. The declared-files provider and
//! the logging levels come straight from the engine, because their *policy* (which files, how loud) is
//! the engine's, not a rendering decision.

mod json;
mod render;

pub use render::render;

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether output is JSON.
///
/// Set once from argv, before any command runs. A process-wide switch rather than a parameter because
/// the alternative is threading a mode through ten `app::` helpers that only pass it on -- and this
/// process runs one command and exits.
static JSON: AtomicBool = AtomicBool::new(false);

/// Turn JSON output on. Called once, right after argv is parsed.
pub fn set_json(on: bool) {
    JSON.store(on, Ordering::Relaxed);
}

/// Whether the JSON mode is on.
pub fn is_json() -> bool {
    JSON.load(Ordering::Relaxed)
}

/// One event, in whichever form the mode asks for.
pub fn render_event(plugin: &str, event: pmpx_engine::Event) {
    if is_json() {
        anstream::println!("{}", json::event(plugin, &event));
    } else {
        render(plugin, event);
    }
}
