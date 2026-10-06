//! Saying something to the person running pmpx.
//!
//! A plugin cannot print usefully on its own: it does not know the id the host shows for it,
//! whether the person asked for detail, or where those lines should go. So it calls the macros
//! here -- `debug!`, `info!`, `warn!`, `error!` -- and the **host** decides: it adds the prefix and
//! the id, and drops anything louder than the level it was asked for.
//!
//! Two consequences worth knowing:
//!
//! - **Nothing is formatted when the host would not print it.** Every macro asks
//!   [`wants`] first, so a run without `--debug` pays nothing for the debug lines a plugin writes.
//! - **`--debug` is not an input.** The plugin always calls the same way; only the host's printing
//!   changes, so a debug run executes byte-for-byte the same command as any other.
//!
//! Without a host -- a plugin's own `cargo test` -- the messages go to stderr directly, at every
//! level, which is what an author wants while writing a plugin.

use std::cell::RefCell;
use std::fmt;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::OnceLock;

use crate::abi::{PmpxHostV1, PmpxStr};
use crate::{Context, Verb};

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
            Level::Error => crate::abi::PMPX_LEVEL_ERROR,
            Level::Warn => crate::abi::PMPX_LEVEL_WARN,
            Level::Info => crate::abi::PMPX_LEVEL_INFO,
            Level::Debug => crate::abi::PMPX_LEVEL_DEBUG,
        }
    }
}

/// The host's hooks, or null when there is no host (the plugin is under its own tests).
static HOST: AtomicPtr<PmpxHostV1> = AtomicPtr::new(std::ptr::null_mut());

/// What the plugin calls itself, for the no-host case only -- the host knows its own id for it.
static NAME: OnceLock<String> = OnceLock::new();

// The call in progress on this thread, held here rather than passed around so that `context()` and
// the macros can report what the host said without every plugin method threading it through. It is
// set for exactly as long as `PackageManager::command` runs.
thread_local! {
    static CURRENT: RefCell<Option<Call>> = const { RefCell::new(None) };
}

/// What the shell knows about the call in progress.
pub(crate) struct Call {
    pub(crate) context: Context,
    pub(crate) verb: Verb,
    pub(crate) args_len: usize,
}

/// Install the host's hooks. Called by the `export!` shell; a plugin never calls this.
pub fn set_host(host: *const PmpxHostV1) {
    HOST.store(host as *mut PmpxHostV1, Ordering::Release);
}

/// Remember the plugin's own name, for the no-host fallback. Called by the `export!` shell.
pub fn remember_name(name: &str) {
    let _ = NAME.set(name.to_string());
}

/// Whether the host would print a message at this level.
///
/// Cheap on purpose: the macros call it *before* formatting, so a silent host costs one atomic
/// load. With no host the answer is "yes", because that is a plugin's own test run.
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

/// Print the whole context of the call in progress.
///
/// One line, once: what the host said it was given. A plugin that wants to know its situation does
/// not have to reassemble it from the parameters.
pub fn context() {
    if !wants(Level::Debug) {
        return;
    }

    let line = CURRENT.with(|current| current.borrow().as_ref().map(describe));
    if let Some(line) = line {
        send(Level::Debug, &line);
    }
}

/// Run `f` with `call` installed as the current one, restoring whatever was there before.
pub(crate) fn with_call<T>(call: Call, f: impl FnOnce() -> T) -> T {
    let previous = CURRENT.with(|current| current.borrow_mut().replace(call));
    let out = f();
    CURRENT.with(|current| *current.borrow_mut() = previous);
    out
}

/// One line describing a call, as `context()` prints it.
fn describe(call: &Call) -> String {
    format!(
        "context: root={} matched=[{}] verb={} args={}",
        call.context.project_root.display(),
        call.context.matched.join(" "),
        call.verb,
        call.args_len
    )
}

/// Hand one message to the host, or print it here when there is none.
fn send(level: Level, message: &str) {
    match host() {
        Some(host) => {
            let borrowed = PmpxStr {
                ptr: message.as_ptr(),
                len: message.len(),
            };
            // SAFETY: the host installed this pointer and keeps it alive for the process, and
            // `message` outlives the call -- which is exactly what the contract asks for.
            unsafe { (host.log)(level.to_abi(), borrowed) };
        }
        None => eprintln!("[{}] {message}", name()),
    }
}

/// The hooks, if a host installed any.
fn host() -> Option<&'static PmpxHostV1> {
    let raw = HOST.load(Ordering::Acquire);
    if raw.is_null() {
        return None;
    }

    // SAFETY: `set_host` is only called by the shell with a pointer the host promises to keep
    // alive and unchanged for the life of the process.
    Some(unsafe { &*raw })
}

/// The name to use when there is no host to ask.
fn name() -> &'static str {
    NAME.get().map(String::as_str).unwrap_or("plugin")
}

/// Say something that went wrong.
///
/// Printed by the host in its error style; the plugin's own error return is still what decides the
/// exit code, so this is for the detail behind it.
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
/// With `--debug` off the host drops it, and this costs an atomic load and no formatting at all.
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        $crate::debug::emit($crate::debug::Level::Debug, format_args!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn call() -> Call {
        Call {
            context: Context {
                project_root: PathBuf::from("/work/project"),
                matched: vec!["package.json".to_string(), "pnpm-lock.yaml".to_string()],
            },
            verb: Verb::Install,
            args_len: 2,
        }
    }

    /// The line a plugin gets from `context()` has to carry what the host actually said, not a
    /// summary the plugin guessed at.
    #[test]
    fn the_context_line_names_root_matched_verb_and_args() {
        let line = describe(&call());

        assert!(line.contains("/work/project"), "{line}");
        assert!(line.contains("package.json pnpm-lock.yaml"), "{line}");
        assert!(line.contains("install"), "{line}");
        assert!(line.contains("args=2"), "{line}");
    }

    /// Without a host -- a plugin's own test run -- everything is wanted, so an author sees their
    /// own lines.
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
}
