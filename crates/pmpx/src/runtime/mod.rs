//! What the engine reports, as this host presents it.
//!
//! The engine owns everything that talks to a plugin -- loading it, calling it, reading the files it
//! declared, and the hooks it logs through -- and prints nothing. This module is the other half: the
//! plugin's lines with its id, the notes worth showing, and trace detail. The declared-files provider and
//! the logging levels come straight from the engine, because their *policy* (which files, how loud) is
//! the engine's, not a rendering decision.

mod render;

pub use pmpx_engine::{Backend, Call, Declared, Levels, PluginIdentity};
pub use render::render;
