//! The wire format across the `dlopen` boundary.
//!
//! The host and the plugin are two separately compiled worlds, so data crossing this line can
//! only be `#[repr(C)]` POD and plain integers: there is no guarantee about which allocator owns
//! the memory of `String` / `Vec` / `Box`, the layout of `toml::Value` / `anyhow::Error` changes
//! with dependency patch versions, and nothing fixes which side a trait object's vtable belongs
//! to. The price is that the two sides share no allocator -- memory is always freed by the side
//! that allocated it: inputs passed in by the host are read-only, and outputs produced by the
//! plugin (`PmpxCommand` and the strings inside it, `name` / `family`) are handed back by the
//! host with [`free_command`] / [`free_str`]. So `free_*` must never use the host's
//! `Box::from_raw` to adopt memory that came from the plugin.
//!
//! The parts: `types` is the `#[repr(C)]` data and its constants, `marshal` converts bytes and
//! hands memory over, and `dispatch` is the output side of one `command` call. Every item is
//! re-exported here, so the public paths are all `pmpx_plugin::abi::…`.

mod dispatch;
mod marshal;
mod types;

pub use dispatch::{dispatch_command, free_command, guard, guard_str, write_command, PANIC_MARKER};
pub use marshal::{bytes_to_os, free_str, leak_bytes, leak_str, os_to_bytes, read_os, read_str};
pub use types::{
    build_rustc, build_target, CommandFn, PmpxCommand, PmpxContextV1, PmpxFile, PmpxHostV1,
    PmpxKeyValue, PmpxPin, PmpxPluginV1, PmpxStr, ABI_VERSION, ENTRY_SYMBOL, PMPX_ERR_INTERNAL,
    PMPX_ERR_INVALID_ARGS, PMPX_ERR_UNSUPPORTED_VERB, PMPX_LEVEL_DEBUG, PMPX_LEVEL_ERROR,
    PMPX_LEVEL_INFO, PMPX_LEVEL_WARN, PMPX_OK, PMPX_REASON_EXPLICIT, PMPX_REASON_PINNED,
    PMPX_REASON_SCORED, VERB_BUILD, VERB_EXEC, VERB_INSTALL, VERB_REMOVE, VERB_RUN, VERB_TEST,
    VERB_UPDATE,
};
