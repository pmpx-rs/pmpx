//! The decision's types, as the CLI names them.
//!
//! The decision itself lives in `pmpx-detect`, and the adapter that feeds it the installed plugins lives
//! in `pmpx-engine`. This module exists so that the CLI's own code says `detect_types::Selection` rather
//! than reaching through two crates for a type it shows on screen.

pub use pmpx_detect::{FamilyScore, Selection};
