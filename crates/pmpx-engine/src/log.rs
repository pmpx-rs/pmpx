#![allow(unsafe_code)] // the two C callbacks the host exports: this module is an ABI boundary
//! The host's logging hooks: the one channel that goes **into** a plugin.
//!
//! A plugin cannot print usefully by itself -- it does not know the id pmpx shows for it, or whether
//! the person asked for detail -- so it calls the hooks here and the host decides. What it decides is
//! an [`Event`]: the text and the level, with nothing about colour, prefixes or destinations. That
//! belongs to whoever is showing it.
//!
//! # Why the events are deferred
//!
//! The callback is a plain C function pointer: it gets a level and a string, and no way to reach a
//! caller's closure. So messages are queued on the calling thread and drained into the event sink once
//! the call returns -- which also means the queue needs no `unsafe` pointer to a borrowed sink.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};

use pmpx_plugin::abi::{
    PmpxHost, PmpxLog, PmpxStr, PMPX_LEVEL_DEBUG, PMPX_LEVEL_ERROR, PMPX_LEVEL_WARN,
};

use crate::Event;

thread_local! {
    /// Messages a plugin produced during the call on this thread.
    static QUEUED: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
}

/// The level this run installed, so the host's capability lookup can answer with the matching table.
static LEVEL: AtomicU32 = AtomicU32::new(PMPX_LEVEL_WARN);

/// What a plugin's `debug!` reaches, when the trace is on: everything, including `debug!`.
static LOG_TRACE: PmpxLog = PmpxLog {
    size: std::mem::size_of::<PmpxLog>(),
    write: log,
    max_level: PMPX_LEVEL_DEBUG,
};

/// A normal run: warnings and errors, no notes.
static LOG_NORMAL: PmpxLog = PmpxLog {
    size: std::mem::size_of::<PmpxLog>(),
    write: log,
    max_level: PMPX_LEVEL_WARN,
};

/// `--quiet`: only errors.
static LOG_QUIET: PmpxLog = PmpxLog {
    size: std::mem::size_of::<PmpxLog>(),
    write: log,
    max_level: PMPX_LEVEL_ERROR,
};

/// How much of a plugin's output a run wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Levels {
    /// Only errors.
    pub quiet: bool,
    /// Everything, including notes and detail.
    pub trace: bool,
}

impl Levels {
    /// The level this combination installs.
    ///
    /// `trace` wins over `quiet`: asking for a trace is the more explicit of the two requests, and a
    /// plugin's lines are part of that trace. `quiet` on its own still silences a plugin's notes and
    /// warnings, leaving only its errors.
    pub const fn max_level(self) -> u32 {
        if self.trace {
            PMPX_LEVEL_DEBUG
        } else if self.quiet {
            PMPX_LEVEL_ERROR
        } else {
            PMPX_LEVEL_WARN
        }
    }
}

/// The host's capability lookup: the one thing a plugin may ask for.
///
/// # Safety
/// `name` must be valid for the duration of the call; the returned pointer is to one of the `'static`
/// tables above.
unsafe extern "C" fn capability(name: PmpxStr) -> *const std::ffi::c_void {
    // SAFETY: the plugin passes a borrow that outlives the call.
    let bytes = unsafe { name.as_bytes() }.unwrap_or(&[]);
    if bytes != pmpx_plugin::abi::PMPX_CAP_LOG.as_bytes() {
        // A capability this host does not have is "not here", which keeps an unknown one from being an
        // error.
        return std::ptr::null();
    }

    let table = match LEVEL.load(Ordering::Acquire) {
        PMPX_LEVEL_DEBUG => &LOG_TRACE,
        PMPX_LEVEL_ERROR => &LOG_QUIET,
        _ => &LOG_NORMAL,
    };
    std::ptr::from_ref(table).cast()
}

/// The host's table, handed to a plugin once per load.
static HOST: PmpxHost = PmpxHost {
    abi_major: pmpx_plugin::abi::PMPX_ABI_MAJOR,
    size: std::mem::size_of::<PmpxHost>(),
    capability,
};

/// The hooks to install for this run.
///
/// Three tables rather than one mutable: the level is decided once, at startup, and the table a plugin
/// reads `max_level` from has to be right when it is attached.
pub fn hooks(levels: Levels) -> &'static PmpxHost {
    LEVEL.store(levels.max_level(), Ordering::Release);
    &HOST
}

/// Hand the queued plugin messages to `sink`, oldest first.
pub fn drain(sink: &mut dyn FnMut(Event)) {
    QUEUED.with(|queued| {
        let mut queued = queued.borrow_mut();
        for event in queued.drain(..) {
            sink(event);
        }
    });
}

/// Forget anything queued on this thread.
///
/// Called before a call, so that a plugin's messages can never be attributed to the next one.
pub fn clear() {
    QUEUED.with(|queued| queued.borrow_mut().clear());
}

/// The callback a plugin calls through.
///
/// # Safety
/// `message` must be valid for the duration of this call, which is what the contract promises.
unsafe extern "C" fn log(level: u32, message: PmpxStr) {
    // SAFETY: the plugin passes a borrowed view of a `String` that outlives this call.
    let bytes = unsafe { message.as_bytes() }.unwrap_or(&[]);
    let text = String::from_utf8_lossy(bytes).into_owned();

    QUEUED.with(|queued| {
        queued
            .borrow_mut()
            .push(Event::PluginMessage { level, text })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The level a run installs is what decides how much of a plugin's output is ever formatted.
    #[test]
    fn the_installed_level_follows_the_flags() {
        let max = |levels: Levels| {
            let hooks = hooks(levels);
            // SAFETY: the host's own lookup, with a key borrowed from a literal.
            let table = unsafe {
                (hooks.capability)(PmpxStr::new(
                    pmpx_plugin::abi::PMPX_CAP_LOG.as_ptr(),
                    pmpx_plugin::abi::PMPX_CAP_LOG.len(),
                ))
            };
            assert!(!table.is_null(), "the log capability is always offered");
            // SAFETY: the lookup answered with one of this module's own tables.
            unsafe { (*(table as *const PmpxLog)).max_level }
        };

        assert_eq!(max(Levels::default()), PMPX_LEVEL_WARN);
        assert_eq!(
            max(Levels {
                quiet: true,
                trace: false
            }),
            PMPX_LEVEL_ERROR
        );
        assert_eq!(
            max(Levels {
                quiet: false,
                trace: true
            }),
            PMPX_LEVEL_DEBUG
        );
        assert_eq!(
            max(Levels {
                quiet: true,
                trace: true
            }),
            PMPX_LEVEL_DEBUG,
            "a trace is the more explicit request of the two"
        );
    }

    /// A capability this host does not have is "not here", not an error.
    #[test]
    fn an_unknown_capability_answers_nothing() {
        let name = "something.else";
        // SAFETY: a key borrowed from a literal.
        let answer = unsafe { capability(PmpxStr::new(name.as_ptr(), name.len())) };
        assert!(answer.is_null());
    }

    /// The plugin's messages reach the sink as data, and only once.
    #[test]
    fn messages_are_queued_and_drained_once() {
        clear();
        // SAFETY: a borrowed view of a local `String` that outlives the call.
        unsafe {
            log(
                PMPX_LEVEL_WARN,
                PmpxStr::new("careful".as_ptr(), "careful".len()),
            )
        };

        let mut seen = Vec::new();
        drain(&mut |event| seen.push(event));
        assert_eq!(
            seen,
            vec![Event::PluginMessage {
                level: PMPX_LEVEL_WARN,
                text: "careful".to_string()
            }]
        );

        let mut again = Vec::new();
        drain(&mut |event| again.push(event));
        assert!(again.is_empty(), "a message is delivered once");
    }

    /// The pointer handed to a plugin must stay valid for its whole life, so it is one `'static`.
    #[test]
    fn the_hooks_are_static() {
        let first = hooks(Levels::default()) as *const PmpxHost;
        let second = hooks(Levels::default()) as *const PmpxHost;
        assert_eq!(first, second);
        // SAFETY: the pointer is to this module's own table.
        assert_eq!(unsafe { &*first }.size, std::mem::size_of::<PmpxHost>());
    }
}
