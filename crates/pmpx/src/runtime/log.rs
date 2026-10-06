//! What a plugin's `debug!` reaches: the one channel that goes **into** a plugin.
//!
//! A plugin cannot print usefully by itself -- it does not know the id pmpx shows for it, or
//! whether the person asked for detail -- so it calls the hooks here and pmpx decides. The host
//! half of the pair lives in `pmpx-plugin`'s `debug` module, which is what a plugin author writes
//! against.

use std::cell::RefCell;

use pmpx_plugin::abi::{
    PmpxHostV1, PmpxStr, PMPX_LEVEL_DEBUG, PMPX_LEVEL_ERROR, PMPX_LEVEL_INFO, PMPX_LEVEL_WARN,
};

thread_local! {
    // Which plugin is being called on this thread. Set once per load (a run loads one backend), and
    // read by the callback below so that every line carries the id without the plugin knowing it.
    static CURRENT_PLUGIN: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// What a plugin's `debug!` reaches, when the trace is on: everything, including `debug!`.
static HOST_TRACE: PmpxHostV1 = PmpxHostV1::new(log, PMPX_LEVEL_DEBUG);

/// A normal run: warnings and errors, no notes.
static HOST_NORMAL: PmpxHostV1 = PmpxHostV1::new(log, PMPX_LEVEL_WARN);

/// `--quiet`: only errors.
static HOST_QUIET: PmpxHostV1 = PmpxHostV1::new(log, PMPX_LEVEL_ERROR);

/// The hooks to install for this run.
///
/// Three `static`s rather than one mutable: the level is decided once, at startup, and a `static`
/// can be built in const context -- no atomics, no allocation, and the pointer the plugin keeps is
/// good for the life of the process.
///
/// `--debug` wins over `--quiet`: asking for a trace is the more explicit of the two requests, and
/// a plugin's lines are part of that trace. `--quiet` on its own still silences a plugin's notes
/// and warnings, leaving only its errors.
pub(crate) fn hooks(quiet: bool, trace: bool) -> &'static PmpxHostV1 {
    if trace {
        &HOST_TRACE
    } else if quiet {
        &HOST_QUIET
    } else {
        &HOST_NORMAL
    }
}

/// Remember which plugin the host is calling, so the id can be added to its lines.
pub(crate) fn set_current_plugin(name: &str) {
    CURRENT_PLUGIN.with(|current| *current.borrow_mut() = Some(name.to_string()));
}

/// The line a plugin's message becomes, before any styling: `[pnpm] warn: no lockfile`.
///
/// The id is bracketed so it reads as a label rather than as part of the sentence -- and so it
/// matches what the plugin prints for itself when no host is installed.
///
/// Pure, so the shape can be asserted without capturing stderr.
fn render(plugin: &str, level: u32, message: &str) -> String {
    match level {
        PMPX_LEVEL_ERROR => format!("[{plugin}] error: {message}"),
        PMPX_LEVEL_WARN => format!("[{plugin}] warn: {message}"),
        // Info and debug only ever arrive with `--debug` on, where "which plugin" is the only
        // thing missing from the trace line.
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
    let message = unsafe { crate::runtime::strings::read_bytes(message) };

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
        assert_eq!(hooks(false, false).max_level, PMPX_LEVEL_WARN);
        assert_eq!(hooks(true, false).max_level, PMPX_LEVEL_ERROR);
        assert_eq!(hooks(false, true).max_level, PMPX_LEVEL_DEBUG);
        // `--debug` wins over `--quiet`, the same way it does for pmpx's own trace: a plugin's
        // lines are part of the trace, and asking for one is the more explicit request.
        assert_eq!(hooks(true, true).max_level, PMPX_LEVEL_DEBUG);
    }

    /// The pointer handed to a plugin must stay valid for its whole life, so it is one of three
    /// `'static`s rather than something built per call.
    #[test]
    fn the_hooks_are_static() {
        let first = hooks(false, true) as *const PmpxHostV1;
        let second = hooks(false, true) as *const PmpxHostV1;
        assert_eq!(first, second);
        assert_eq!(
            unsafe { &*first }.size,
            std::mem::size_of::<PmpxHostV1>(),
            "the plugin is told how much of the struct it may read"
        );
    }
}
