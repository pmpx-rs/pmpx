#![allow(unsafe_code)] // the fake host is an ABI boundary: it installs a table a plugin calls through
//! A fake host, so a plugin's own tests can see what it says.
//!
//! A plugin cannot print usefully: its lines go to the host, and without one they fall back to its own
//! stderr -- which a test cannot assert on. So this module installs the smallest possible host: one
//! [`PmpxLog`] table whose callback records into a list, at the level the test asks for.
//!
//! The list is process-wide, exactly like the real hooks, so a test that installs one should keep the
//! [`Captured`] handle around and assert on its own lines rather than on the absence of others.

use std::sync::Mutex;

use pmpx_plugin::abi::{
    PmpxLog, PmpxStr, PMPX_LEVEL_DEBUG, PMPX_LEVEL_ERROR, PMPX_LEVEL_INFO, PMPX_LEVEL_WARN,
};

/// The lines the plugin wrote, oldest first.
static LINES: Mutex<Vec<(u32, String)>> = Mutex::new(Vec::new());

/// The tables, one per level.
///
/// A table carries its `max_level` as a *value*, and the shell reads it once, when the table is installed
/// -- so a level is a different table, not a field to change. That is why the real host has three of them
/// too.
static TRACE: PmpxLog = table(PMPX_LEVEL_DEBUG);
static INFO: PmpxLog = table(PMPX_LEVEL_INFO);
static NORMAL: PmpxLog = table(PMPX_LEVEL_WARN);
static QUIET: PmpxLog = table(PMPX_LEVEL_ERROR);

/// One table at one level.
const fn table(max_level: u32) -> PmpxLog {
    PmpxLog {
        size: std::mem::size_of::<PmpxLog>(),
        write: record,
        max_level,
    }
}

/// Record one line. The callback a plugin calls through.
///
/// # Safety
/// `message` must be valid for the duration of this call, which is what the contract promises.
unsafe extern "C" fn record(level: u32, message: PmpxStr) {
    // SAFETY: the plugin passes a borrowed view of a `String` that outlives this call.
    let bytes = unsafe { message.as_bytes() }.unwrap_or(&[]);
    let text = String::from_utf8_lossy(bytes).into_owned();

    if let Ok(mut lines) = LINES.lock() {
        lines.push((level, text));
    }
}

/// A handle on the fake host: install it, run the plugin, assert on the lines.
pub struct Captured;

impl Captured {
    /// Everything the plugin wrote since the handle was created, oldest first.
    ///
    /// Draining by taking it: a test that asserts twice does not see the first assertion's lines again.
    pub fn take(&self) -> Vec<String> {
        let mut lines = LINES.lock().expect("the fake host's list is not poisoned");
        lines.drain(..).map(|(_, text)| text).collect()
    }

    /// The same, with the contract's level numbers.
    pub fn take_levels(&self) -> Vec<(u32, String)> {
        let mut lines = LINES.lock().expect("the fake host's list is not poisoned");
        lines.drain(..).collect()
    }

    /// How loud this host is: installing the table for that level is what tells the plugin.
    pub fn level(self, level: u32) -> Self {
        let table = match level {
            PMPX_LEVEL_DEBUG => &TRACE,
            PMPX_LEVEL_INFO => &INFO,
            PMPX_LEVEL_ERROR => &QUIET,
            _ => &NORMAL,
        };
        // SAFETY: this module's own `'static` table, large enough for the shell's check.
        unsafe { pmpx_plugin::shell::install_log_table(table) };
        self
    }

    /// Everything: notes and detail included.
    pub fn trace(self) -> Self {
        self.level(PMPX_LEVEL_DEBUG)
    }

    /// Only failures.
    pub fn quiet(self) -> Self {
        self.level(PMPX_LEVEL_ERROR)
    }

    /// A plain run: warnings and errors.
    pub fn normal(self) -> Self {
        self.level(PMPX_LEVEL_WARN)
    }

    /// Notes and above.
    pub fn info(self) -> Self {
        self.level(PMPX_LEVEL_INFO)
    }
}

/// Install the fake host and hand back a handle on it.
///
/// The level starts at `Warn`, like a plain run; [`Captured::trace`] and friends change it. Whatever was
/// recorded before is dropped, so a test starts from an empty list.
pub fn capture() -> Captured {
    if let Ok(mut lines) = LINES.lock() {
        lines.clear();
    }

    Captured.level(PMPX_LEVEL_WARN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pmpx_plugin::{CommandSpec, Context, Family, PackageManager, PluginError, Verb};

    /// The hooks are process-wide, so these three tests take turns: one installs a level, runs the plugin,
    /// and asserts -- with the lock held, so no other test can install a different level in between. A
    /// plugin author writing one test does not have to think about this; this module's own tests do.
    static SERIAL: Mutex<()> = Mutex::new(());

    /// A plugin that says one thing per level, to prove the capture works the way an author would use it.
    struct Talker;

    impl PackageManager for Talker {
        fn name(&self) -> &str {
            "talker"
        }

        fn family(&self) -> Family {
            Family::NODE
        }

        fn command(
            &self,
            _ctx: &Context,
            _verb: Verb,
            _args: &[std::ffi::OsString],
        ) -> Result<CommandSpec, PluginError> {
            pmpx_plugin::debug!("choosing a version");
            pmpx_plugin::warn!("no lockfile, guessing");
            pmpx_plugin::error!("this will not work");
            Ok(CommandSpec::new("tool"))
        }
    }

    /// With a trace on, every line is recorded, in order, at the level it was written.
    #[test]
    fn a_trace_captures_every_level() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let host = capture().trace();

        let _ = Talker.command(&crate::context().build(), Verb::Install, &[]);

        let lines = host.take_levels();
        for level in [PMPX_LEVEL_DEBUG, PMPX_LEVEL_WARN, PMPX_LEVEL_ERROR] {
            assert!(
                lines.iter().any(|(got, _)| *got == level),
                "level {level} is missing from {lines:?}"
            );
        }
        assert!(
            lines.iter().any(|(_, text)| text == "choosing a version"),
            "{lines:?}"
        );
    }

    /// After a normal run, only what a normal run shows is recorded -- the "nothing is formatted when
    /// nobody asked" promise, from a test's side.
    #[test]
    fn a_normal_capture_drops_notes_and_detail() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let host = capture().normal();

        let _ = Talker.command(&crate::context().build(), Verb::Install, &[]);

        let lines = host.take_levels();
        assert!(
            lines
                .iter()
                .any(|(_, text)| text == "no lockfile, guessing"),
            "a warning is shown by a normal run: {lines:?}"
        );
        assert!(
            !lines.iter().any(|(level, _)| *level == PMPX_LEVEL_DEBUG),
            "detail is not even formatted: {lines:?}"
        );
        assert!(host.take().is_empty(), "the list is drained by taking it");
    }

    /// A quiet host keeps only the failures.
    #[test]
    fn a_quiet_capture_keeps_only_errors() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let host = capture().quiet();

        let _ = Talker.command(&crate::context().build(), Verb::Install, &[]);

        let lines = host.take_levels();
        assert!(
            lines.iter().any(|(_, text)| text == "this will not work"),
            "{lines:?}"
        );
        assert!(
            lines.iter().all(|(level, _)| *level == PMPX_LEVEL_ERROR),
            "a quiet host keeps only failures: {lines:?}"
        );
    }
}
