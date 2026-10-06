//! Saying something to the person running pmpx.
//!
//! A plugin cannot print usefully on its own: it does not know the id the host shows for it, whether
//! the person asked for detail, or where those lines should go. So it calls the macros here --
//! [`debug!`](macro@crate::debug), [`info!`](macro@crate::info), [`warn!`](macro@crate::warn), [`error!`](macro@crate::error)
//! -- and the **host** decides: it adds the prefix and the id, and drops anything louder than the
//! level it was asked for.
//!
//! Two consequences worth knowing:
//!
//! - **Nothing is formatted when the host would not print it.** Every macro asks [`wants`] first, so
//!   a run without a trace pays nothing for the lines a plugin writes.
//! - **The trace is not an input.** The plugin always calls the same way; only the host's printing
//!   changes, so a traced run executes byte-for-byte the same command as any other.
//!
//! Without a host -- a plugin's own `cargo test` -- the messages go to stderr directly, at every
//! level, which is what an author wants while writing a plugin.

use std::cell::RefCell;
use std::fmt;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::OnceLock;

use pmpx_plugin_abi::{PmpxLog, PmpxStr};

/// How loud one message is.
///
/// Ordered: a host that prints [`Level::Warn`] prints everything up to and including it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Something went wrong.
    Error,
    /// Something is off, but the command still runs.
    Warn,
    /// An ordinary note about what the plugin decided.
    Info,
    /// Detail for someone debugging.
    Debug,
}

impl Level {
    /// The number this level crosses the boundary as.
    pub const fn to_abi(self) -> u32 {
        match self {
            Level::Error => pmpx_plugin_abi::PMPX_LEVEL_ERROR,
            Level::Warn => pmpx_plugin_abi::PMPX_LEVEL_WARN,
            Level::Info => pmpx_plugin_abi::PMPX_LEVEL_INFO,
            Level::Debug => pmpx_plugin_abi::PMPX_LEVEL_DEBUG,
        }
    }
}

/// The host's logging table, or null when there is no host (the plugin is under its own tests).
///
/// It is only stored after its `size` was checked against what this build knows how to read, which
/// is what makes reading `max_level` below safe.
static LOG: AtomicPtr<PmpxLog> = AtomicPtr::new(std::ptr::null_mut());

/// What the plugin calls itself, for the no-host case only -- the host knows its own id for it.
static NAME: OnceLock<String> = OnceLock::new();

// The call in progress on this thread, held here rather than passed around so that `context()` and
// the macros can report what the host said without every plugin method threading it through. The
// description is rendered by the shell, because only it has the raw context.
thread_local! {
    static CURRENT: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Install the host's logging table. Called by the `export!` shell; a plugin never calls this.
///
/// # Safety
/// `table` must be the host's, with at least `size_of::<PmpxLog>()` bytes, and must stay valid for
/// the life of the process.
pub(crate) unsafe fn set_log_table(table: *const PmpxLog) {
    LOG.store(table as *mut PmpxLog, Ordering::Release);
}

/// Remember the plugin's own name, for the no-host fallback. Called by the `export!` shell.
pub(crate) fn remember_name(name: &str) {
    let _ = NAME.set(name.to_string());
}

/// Whether the host would print a message at this level.
///
/// Cheap on purpose: the macros call it *before* formatting, so a silent host costs one atomic load.
/// With no host the answer is "yes", because that is a plugin's own test run.
pub fn wants(level: Level) -> bool {
    match host() {
        Some(host) => level.to_abi() <= host.max_level,
        None => true,
    }
}

/// Send one already-formatted message. The macros go through here.
pub fn emit(level: Level, args: fmt::Arguments<'_>) {
    if !wants(level) {
        return;
    }
    send(level, &args.to_string());
}

/// Print the whole context of the call in progress, as one line.
///
/// The line is rendered by the shell that made the call, so it carries what the host actually said
/// rather than a summary this side guessed at. Outside a call there is nothing to print.
pub fn context() {
    if !wants(Level::Debug) {
        return;
    }

    if let Some(line) = CURRENT.with(|current| current.borrow().clone()) {
        send(Level::Debug, &line);
    }
}

/// Run `f` with `description` installed as the current call, restoring whatever was there before.
pub(crate) fn with_call<T>(description: String, f: impl FnOnce() -> T) -> T {
    let previous = CURRENT.with(|current| current.borrow_mut().replace(description));
    let out = f();
    CURRENT.with(|current| *current.borrow_mut() = previous);
    out
}

/// Hand one message to the host, or print it here when there is none.
fn send(level: Level, message: &str) {
    match host() {
        Some(host) => {
            let borrowed = PmpxStr::new(message.as_ptr(), message.len());
            // SAFETY: the table was checked when it was installed, and the host keeps it alive for
            // the process; `message` outlives the call, which is what the contract asks for.
            unsafe { (host.write)(level.to_abi(), borrowed) };
        }
        None => eprintln!("[{}] {message}", name()),
    }
}

/// The logging table, if a host installed one.
fn host() -> Option<&'static PmpxLog> {
    let raw = LOG.load(Ordering::Acquire);
    if raw.is_null() {
        return None;
    }

    // SAFETY: only the shell calls `set_log_table`, with a pointer the host promises to keep alive
    // and unchanged, and only after checking that the table is large enough to read.
    Some(unsafe { &*raw })
}

/// The name to use when there is no host to ask.
fn name() -> &'static str {
    NAME.get().map(String::as_str).unwrap_or("plugin")
}

/// Say something that went wrong.
///
/// Printed by the host in its error style. This is also the channel for the text a
/// [`PluginError`](crate::PluginError) carries: that text stays inside the plugin and is never sent
/// across the boundary, so `error!` is how it reaches a person.
#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        $crate::debug::emit($crate::debug::Level::Error, format_args!($($arg)*))
    };
}

/// Say that something is off, but the command still runs.
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        $crate::debug::emit($crate::debug::Level::Warn, format_args!($($arg)*))
    };
}

/// Say what the plugin decided, in one line.
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        $crate::debug::emit($crate::debug::Level::Info, format_args!($($arg)*))
    };
}

/// Say something for whoever is debugging.
///
/// With no trace asked for the host drops it, and this costs an atomic load and no formatting.
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        $crate::debug::emit($crate::debug::Level::Debug, format_args!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without a host -- a plugin's own test run -- everything is wanted, so an author sees their own
    /// lines.
    #[test]
    fn without_a_host_every_level_is_wanted() {
        assert!(wants(Level::Error));
        assert!(wants(Level::Warn));
        assert!(wants(Level::Info));
        assert!(wants(Level::Debug));
    }

    #[test]
    fn levels_are_ordered_so_a_host_can_say_how_loud_it_wants_them() {
        assert!(Level::Error < Level::Warn);
        assert!(Level::Warn < Level::Info);
        assert!(Level::Info < Level::Debug);
    }

    /// The description is only current inside a call: a plugin logging outside one prints nothing
    /// from `context()`.
    #[test]
    fn the_call_description_is_scoped() {
        context();

        with_call("context: inside".to_string(), || {
            let seen = CURRENT.with(|current| current.borrow().clone());
            assert_eq!(seen.as_deref(), Some("context: inside"));
        });

        let after = CURRENT.with(|current| current.borrow().clone());
        assert!(after.is_none(), "the description must not outlive the call");
    }
}
