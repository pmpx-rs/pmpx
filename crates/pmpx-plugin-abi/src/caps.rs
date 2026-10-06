//! The capabilities each side may provide, and how they are named.
//!
//! A capability is a name plus a pointer to a size-prefixed table of function pointers. The lookup
//! replaces a fixed vtable, which is what makes adding one additive: an old host never asks for a
//! name it does not know, and an old plugin answers null for one it does not have.

/// The plugin's identity: `name`, `family`, `free_str`.
pub const PMPX_CAP_IDENTITY: &str = "identity";

/// The plugin's one job: map a call to a command.
pub const PMPX_CAP_COMMAND: &str = "command";

/// Optional: the plugin accepts the host's hooks.
pub const PMPX_CAP_ATTACH: &str = "attach";

/// The host's logging channel.
pub const PMPX_CAP_LOG: &str = "log";

/// The capabilities a host requires of a plugin before it can be used at all.
///
/// A plugin missing one of these is refused with a message naming it. Everything else is optional,
/// and a plugin offering more than this is fine: an old host ignores what it does not ask for.
pub const PMPX_REQUIRED_CAPS: &[&str] = &[PMPX_CAP_COMMAND, PMPX_CAP_IDENTITY];

/// Every capability name of this major version, in the order a rendering of the ABI lists them.
pub const PMPX_CAPS: &[&str] = &[
    PMPX_CAP_ATTACH,
    PMPX_CAP_COMMAND,
    PMPX_CAP_IDENTITY,
    PMPX_CAP_LOG,
];
