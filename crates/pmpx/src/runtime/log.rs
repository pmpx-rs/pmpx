//! What a plugin's `debug!` reaches: the one channel that goes **into** a plugin.
//!
//! A plugin cannot print usefully by itself -- it does not know the id pmpx shows for it, or whether
//! the person asked for detail -- so it calls the hooks here and pmpx decides. The host half of the
//! pair lives in `pmpx-plugin`'s `debug` module, which is what a plugin author writes against.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};

use pmpx_plugin::abi::{
    PmpxHost, PmpxLog, PmpxStr, PMPX_LEVEL_DEBUG, PMPX_LEVEL_ERROR, PMPX_LEVEL_INFO,
    PMPX_LEVEL_WARN,
};

thread_local! {
    // Which plugin is being called on this thread. Set once per load (a run loads one backend), and
    // read by the callback below so that every line carries the id without the plugin knowing it.
    static CURRENT_PLUGIN: RefCell<Option<String>> = const { RefCell::new(None) };
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

/// The host's capability lookup: the one thing a plugin may ask for.
///
/// # Safety
/// `name` must be valid for the duration of the call; the returned pointer is to one of the `'static`
/// tables above.
unsafe extern "C" fn capability(name: PmpxStr) -> *const std::ffi::c_void {
    // SAFETY: the plugin passes a borrow that outlives the call.
    let bytes = unsafe { name.as_bytes() }.unwrap_or(&[]);
    if bytes != pmpx_plugin::abi::PMPX_CAP_LOG.as_bytes() {
        // A capability this host does not have is "not here", which is what keeps an unknown one
        // from being an error.
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
///
/// `--debug` wins over `--quiet`: asking for a trace is the more explicit of the two requests, and a
/// plugin's lines are part of that trace. `--quiet` on its own still silences a plugin's notes and
/// warnings, leaving only its errors.
pub(crate) fn hooks(quiet: bool, trace: bool) -> &'static PmpxHost {
    let level = if trace {
        PMPX_LEVEL_DEBUG
    } else if quiet {
        PMPX_LEVEL_ERROR
    } else {
        PMPX_LEVEL_WARN
    };
    LEVEL.store(level, Ordering::Release);
    &HOST
}

/// Remember which plugin the host is calling, so the id can be added to its lines.
pub(crate) fn set_current_plugin(name: &str) {
    CURRENT_PLUGIN.with(|current| *current.borrow_mut() = Some(name.to_string()));
}

/// The line a plugin's message becomes, before any styling: `[pnpm] warn: no lockfile`.
///
/// The id is bracketed so it reads as a label rather than as part of the sentence -- and so it matches
/// what the plugin prints for itself when no host is installed.
///
/// Pure, so the shape can be asserted without capturing stderr.
fn render(plugin: &str, level: u32, message: &str) -> String {
    match level {
        PMPX_LEVEL_ERROR => format!("[{plugin}] error: {message}"),
        PMPX_LEVEL_WARN => format!("[{plugin}] warn: {message}"),
        // Info and debug only ever arrive with a trace asked for, where "which plugin" is the only
        // thing missing from the line.
        PMPX_LEVEL_INFO | PMPX_LEVEL_DEBUG => format!("[{plugin}] {message}"),
        // A level from a plugin built against a newer contract: detail is the harmless direction.
        _ => format!("[{plugin}] {message}"),
    }
}

/// The callback a plugin calls through.
///
/// # Safety
/// `message` must be valid for the duration of this call, which is what the contract promises.
unsafe extern "C" fn log(level: u32, message: PmpxStr) {
    // SAFETY: the plugin passes a borrowed view of a `String` that outlives this call.
    let bytes = unsafe { message.as_bytes() }.unwrap_or(&[]);
    let message = String::from_utf8_lossy(bytes);

    let plugin = CURRENT_PLUGIN
        .with(|current| current.borrow().clone())
        .unwrap_or_else(|| "plugin".to_string());
    let line = render(&plugin, level, &message);

    match level {
        // A plugin saying something went wrong is worth the error style; it is reporting, not
        // deciding -- the plugin's own error code is what sets the exit status.
        PMPX_LEVEL_ERROR => crate::error::error_line(line),
        // Warnings show up in a normal run, dimmed like the rest of pmpx's secondary output.
        PMPX_LEVEL_WARN => crate::error::note_line(line),
        // Notes and detail belong to the trace.
        _ => crate::debug::note(line),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_level_has_its_own_shape() {
        assert_eq!(
            render("pnpm", PMPX_LEVEL_ERROR, "boom"),
            "[pnpm] error: boom"
        );
        assert_eq!(render("pnpm", PMPX_LEVEL_WARN, "hmm"), "[pnpm] warn: hmm");
        assert_eq!(render("pnpm", PMPX_LEVEL_INFO, "chose x"), "[pnpm] chose x");
        assert_eq!(render("pnpm", PMPX_LEVEL_DEBUG, "detail"), "[pnpm] detail");
    }

    /// The level a run installs is what decides how much of a plugin's output is ever formatted.
    #[test]
    fn the_installed_level_follows_the_flags() {
        let max = |quiet: bool, trace: bool| {
            let hooks = hooks(quiet, trace);
            let table = unsafe {
                (hooks.capability)(PmpxStr::new(
                    pmpx_plugin::abi::PMPX_CAP_LOG.as_ptr(),
                    pmpx_plugin::abi::PMPX_CAP_LOG.len(),
                ))
            };
            assert!(!table.is_null(), "the log capability is always offered");
            // SAFETY: the lookup above answered with one of this module's own tables.
            unsafe { (*(table as *const PmpxLog)).max_level }
        };

        assert_eq!(max(false, false), PMPX_LEVEL_WARN);
        assert_eq!(max(true, false), PMPX_LEVEL_ERROR);
        assert_eq!(max(false, true), PMPX_LEVEL_DEBUG);
        // `--debug` wins over `--quiet`, the same way it does for pmpx's own trace: a plugin's lines
        // are part of the trace, and asking for one is the more explicit request.
        assert_eq!(max(true, true), PMPX_LEVEL_DEBUG);
    }

    /// A capability this host does not have is "not here", not an error.
    #[test]
    fn an_unknown_capability_answers_nothing() {
        let name = "something.else";
        let answer = unsafe { capability(PmpxStr::new(name.as_ptr(), name.len())) };
        assert!(answer.is_null());
    }

    /// The pointer handed to a plugin must stay valid for its whole life, so it is one of three
    /// `'static`s rather than something built per call.
    #[test]
    fn the_hooks_are_static() {
        let first = hooks(false, true) as *const PmpxHost;
        let second = hooks(false, true) as *const PmpxHost;
        assert_eq!(first, second);
        assert_eq!(
            unsafe { &*first }.size,
            std::mem::size_of::<PmpxHost>(),
            "the plugin is told how much of the table it may read"
        );
    }
}
