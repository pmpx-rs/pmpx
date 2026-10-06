//! `--debug`: debug output for one run.
//!
//! Every phase of a run reports one line, in the order it ran: what it was, and how long it took.
//! Measuring is the only thing `main` does for it.
//!
//! Three rules:
//!
//! - **Off by default, and free when off.** Nothing here prints unless `--debug` was given, and
//!   the detail text is passed as a closure so that not even a `format!` runs on a normal run.
//! - **stderr, dim.** The backend inherits stdout, which the caller may be reading through a
//!   pipe, so no trace line may go there. `--quiet` does not silence the trace: asking for a
//!   trace is a more explicit request than asking for fewer hints.
//! - **The clock starts at the first statement of `main`.** [`total`] therefore includes argv
//!   parsing; the phases are where that time went, not a second total.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::style;

/// What every trace line starts with — the same `pmpx` marker real messages use, so that a
/// trace line and a message can be told apart at a glance.
const PREFIX: &str = "pmpx debug: ";

/// Whether `--debug` was given.
///
/// A process-wide flag rather than a field threaded down through every call: the trace is a
/// cross-cutting concern, and one run of pmpx is one process with one flow.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// When `main` started. Written once, never reset — a second write would silently move the
/// baseline of every duration reported afterwards.
static START: OnceLock<Instant> = OnceLock::new();

/// Start the clock: **the first statement of `main`**.
pub fn mark_start() {
    let _ = START.set(Instant::now());
}

/// Turn the trace on.
///
/// Separate from [`mark_start`] because the value of `--debug` only exists after the arguments
/// have been parsed — the clock has to start before that, the flag cannot be known before it.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// Whether the trace is on.
///
/// Private: [`done`], [`note`] and [`total`] check it themselves, so no caller has to ask, and
/// the flag stays the only piece of global state in here.
fn is_on() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Whether the trace is on.
///
/// For a caller that has to *decide* something because of it -- which logging level to install into
/// a plugin, say -- rather than for the trace itself, which checks [`is_on`] as it goes.
pub fn enabled() -> bool {
    is_on()
}

/// A timestamp to hand back to [`done`].
pub fn now() -> Instant {
    Instant::now()
}

/// Report one phase that has finished, measured from `since`.
///
/// `detail` is a closure rather than a value so that the text costs nothing on a normal run:
/// when the trace is off, it is never called.
pub fn done<D: fmt::Display>(phase: &str, since: Instant, detail: impl FnOnce() -> D) {
    if !is_on() {
        return;
    }
    emit(phase, since.elapsed(), detail());
}

/// Report one phase the *engine* measured, in the same shape as this module's own phases.
pub fn phase(name: &str, micros: u128, detail: impl fmt::Display) {
    if !is_on() {
        return;
    }
    emit(name, Duration::from_micros(micros as u64), detail);
}

/// A free-form line, for the header.
pub fn note(text: impl fmt::Display) {
    if !is_on() {
        return;
    }
    anstream::eprintln!("{}", style::paint(style::DIM, format!("{PREFIX}{text}")));
}

/// The header: what is running, and where.
///
/// Printed after `--debug` is known, so it cannot be the first thing on the clock — [`total`]
/// is what says how long the start-up before this line took.
pub fn header() {
    note(format_args!(
        "pmpx {} ({}) in {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        cwd()
    ));
}

/// The last line: the whole run, and the exit code on its way back to the shell.
pub fn total(code: u8) {
    if !is_on() {
        return;
    }
    emit("total", elapsed(), format!("exit {code}"));
}

/// Time since [`mark_start`].
fn elapsed() -> Duration {
    START.get().map(Instant::elapsed).unwrap_or_default()
}

/// The working directory, for the header — a failure here is cosmetic, so it cannot fail the run.
fn cwd() -> String {
    std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "?".to_string())
}

/// One phase line on stderr.
fn emit(phase: &str, took: Duration, detail: impl fmt::Display) {
    anstream::eprintln!("{}", style::paint(style::DIM, line(phase, took, detail)));
}

/// The text of one line, with no colour.
///
/// Separate from printing so that the shape — the part that makes a column of durations
/// skimmable — can be asserted without capturing stderr.
fn line(phase: &str, took: Duration, detail: impl fmt::Display) -> String {
    // Both columns are fixed width: `{:>8.2}` so that a phase a hundred times slower than the
    // others does not shift the phase names out of line, `{:<20}` so that the details do not
    // shift either.
    format!("{PREFIX}{:>8.2}ms  {phase:<20} {detail}", millis(took))
}

/// Milliseconds with a fraction — at this scale, the fraction is the interesting part.
fn millis(took: Duration) -> f64 {
    took.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_carries_the_marker_the_phase_and_the_detail() {
        let text = line("detect.score", Duration::from_micros(240), "2 plugins");

        assert!(text.starts_with(PREFIX), "{text}");
        assert!(text.contains("0.24ms"), "{text}");
        assert!(text.contains("detect.score"), "{text}");
        assert!(text.contains("2 plugins"), "{text}");
    }

    /// The trace is read as a column of durations; a line that shifts the columns turns it back
    /// into prose.
    #[test]
    fn the_columns_hold_still_across_phases_of_very_different_sizes() {
        let fast = line("a", Duration::from_millis(1), "x");
        let slow = line("detect.decide", Duration::from_secs(1), "y");

        assert_eq!(
            fast.find(" x"),
            slow.find(" y"),
            "the detail column moved:\n{fast}\n{slow}"
        );
    }

    /// `mark_start` is called before `--debug` is known and must therefore be idempotent: a
    /// second call must not move the baseline the totals are measured from.
    #[test]
    fn elapsed_is_measured_from_the_first_mark_start() {
        mark_start();
        let first = elapsed();

        mark_start();
        let second = elapsed();

        assert!(second >= first);
        assert!(second < Duration::from_secs(60), "a clock, not a stopwatch");
    }
}
