//! Running a plugin: the host's half of the ABI.
//!
//! Everything here is about *this* host: which library to open, what the plugin says about itself,
//! when to hand over the logging hooks, and how to read the files the plugin declared. The wire format
//! itself lives in `pmpx-plugin-abi`, and the code that speaks it lives in `pmpx-loader`.

mod backend;
mod files;
mod log;

pub use backend::{Backend, Invocation};
pub(crate) use files::Declared;
pub(crate) use log::{hooks, set_current_plugin};
