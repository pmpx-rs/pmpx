//! The raw ABI between pmpx and a plugin.
//!
//! This crate is the *wire format*: `#[repr(C)]` data, the numbers that go with it, the names of
//! the keys a host may answer, and the names of the capabilities each side may provide. It has no
//! dependencies, no `std`, and no behaviour -- the safe Rust surface a plugin author writes against
//! lives in `pmpx-plugin`, and the host side lives in `pmpx-loader`.
//!
//! Three properties are what make it worth a crate of its own:
//!
//! - **It is the only unsafe island besides the loader.** Everything else in the workspace is
//!   `#![forbid(unsafe_code)]`.
//! - **It is hand-translatable.** [`surface`] can render this ABI as text and as a C header, both
//!   committed and checked by tests, so a plugin in C, Zig or Go is a supported target rather than a
//!   thought experiment.
//! - **Adding to it is supposed to be additive.** New keys and new capabilities do not change any
//!   layout and do not bump [`PMPX_ABI_MAJOR`]; see `docs/refactor.md` §3.3 for what does.
//!
//! # The shape of a call
//!
//! The host asks the plugin for the `identity` and `command` capabilities, hands over a
//! [`PmpxContext`] it can query for everything else, and reads back a [`PmpxCommand`]:
//!
//! ```text
//! host                                    plugin
//!  │  plugin.capability("identity")  ──────▶  PmpxIdentity
//!  │  plugin.capability("command")   ──────▶  PmpxCommandCap
//!  │  plugin.capability("attach")    ──────▶  PmpxAttach  (optional)
//!  │  command_cap.run(context, out)  ──────▶
//!  │        context.get("args", 0)   ◀──────  asks for what it needs, lazily
//!  │        context.get("file.package.json", 0)
//!  │  ◀──────  PMPX_OK, out filled in
//!  │  command_cap.free_command(out)
//! ```
#![no_std]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

// The crate is `no_std`; the `std` feature is what opts a *crate that links it* into the two OS
// conversions, without pulling `std` into the wire format itself.
#[cfg(feature = "std")]
extern crate std;

pub mod caps;
pub mod keys;
#[cfg(feature = "std")]
mod os;
pub mod surface;
mod types;

#[cfg(feature = "std")]
pub use os::{bytes_to_os, os_to_bytes};

// A flat namespace on purpose: a consumer (the loader, a plugin shell, a test fixture) should be
// able to write `pmpx_plugin_abi::PMPX_KEY_ARGS` without knowing which module it lives in.
pub use caps::{
    PMPX_CAPS, PMPX_CAP_ATTACH, PMPX_CAP_COMMAND, PMPX_CAP_IDENTITY, PMPX_CAP_LOG,
    PMPX_REQUIRED_CAPS,
};
pub use keys::{
    PMPX_KEYS, PMPX_KEY_ARGS, PMPX_KEY_CONFIG_PIN, PMPX_KEY_FILE_PREFIX,
    PMPX_KEY_PROJECT_CONFIG_FILES, PMPX_KEY_PROJECT_MATCHED, PMPX_KEY_PROJECT_ROOT,
    PMPX_KEY_PROJECT_START_DIR,
};
pub use types::{
    PmpxAttach, PmpxCommand, PmpxCommandCap, PmpxContext, PmpxHost, PmpxIdentity, PmpxLog,
    PmpxPlugin, PmpxSlice, PmpxStr, PMPX_ABI_MAJOR, PMPX_ENTRY_SYMBOL, PMPX_ERR_INTERNAL,
    PMPX_ERR_INVALID_ARGS, PMPX_ERR_UNSUPPORTED_VERB, PMPX_LEVEL_DEBUG, PMPX_LEVEL_ERROR,
    PMPX_LEVEL_INFO, PMPX_LEVEL_WARN, PMPX_MAX_ITEMS, PMPX_OK, PMPX_REASON_EXPLICIT,
    PMPX_REASON_PINNED, PMPX_REASON_SCORED, PMPX_VERB_BUILD, PMPX_VERB_EXEC, PMPX_VERB_INSTALL,
    PMPX_VERB_REMOVE, PMPX_VERB_RUN, PMPX_VERB_TEST, PMPX_VERB_UPDATE,
};
