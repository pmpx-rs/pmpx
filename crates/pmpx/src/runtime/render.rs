//! Presenting what the engine reports.
//!
//! The engine says what happened as [`Event`]s and prints nothing; this is the half that decides what a
//! person sees -- the plugin's own lines with its id and the right style, a warning the host noticed, and
//! trace detail behind `--debug`.

use pmpx_engine::Event;
use pmpx_plugin::abi::{PMPX_LEVEL_DEBUG, PMPX_LEVEL_ERROR, PMPX_LEVEL_INFO, PMPX_LEVEL_WARN};

use crate::debug;
use crate::error::{error_line, note_line};

/// Show one event, knowing which plugin it came from.
pub fn render(plugin: &str, event: Event) -> Rendered {
    match event {
        // A plugin's own line: the id is added here because the plugin does not know it.
        Event::PluginMessage { level, text } => {
            let line = prefixed(plugin, level, &text);
            match level {
                // A plugin saying something went wrong is worth the error style; it is reporting, not
                // deciding -- the plugin's own error code is what sets the exit status.
                PMPX_LEVEL_ERROR => error_line(line),
                // Warnings show up in a normal run, dimmed like the rest of pmpx's secondary output.
                PMPX_LEVEL_WARN => note_line(line),
                // Notes and detail belong to the trace.
                _ => debug::note(line),
            }
            Rendered::Shown
        }

        // Something the host noticed that the person should see.
        Event::Warning(text) => {
            note_line(text);
            Rendered::Shown
        }

        Event::Error(text) => {
            error_line(text);
            Rendered::Shown
        }

        // Trace detail.
        Event::Note(text) => {
            debug::note(text);
            Rendered::Shown
        }

        // The execution events are rendered where the run is, by `spawn::run`'s own sink.
        Event::Resolved { .. } | Event::Starting { .. } | Event::Finished { .. } => {
            Rendered::NotShown
        }
    }
}

/// Whether anything was printed, which is what a caller needs to know to keep its own trace honest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rendered {
    /// It reached the person.
    Shown,
    /// It was an execution event, rendered elsewhere.
    NotShown,
}

/// The line a plugin's message becomes: `[pnpm] warn: no lockfile`.
///
/// The id is bracketed so it reads as a label rather than as part of the sentence -- and so it matches
/// what the plugin prints for itself when no host is installed.
///
/// Pure, so the shape can be asserted without capturing stderr.
pub fn prefixed(plugin: &str, level: u32, message: &str) -> String {
    match level {
        PMPX_LEVEL_ERROR => format!("[{plugin}] error: {message}"),
        PMPX_LEVEL_WARN => format!("[{plugin}] warn: {message}"),
        // Info and debug only ever arrive with a trace asked for, where "which plugin" is the only thing
        // missing from the line.
        PMPX_LEVEL_INFO | PMPX_LEVEL_DEBUG => format!("[{plugin}] {message}"),
        // A level from a plugin built against a newer contract: detail is the harmless direction.
        _ => format!("[{plugin}] {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_level_has_its_own_shape() {
        assert_eq!(
            prefixed("pnpm", PMPX_LEVEL_ERROR, "boom"),
            "[pnpm] error: boom"
        );
        assert_eq!(prefixed("pnpm", PMPX_LEVEL_WARN, "hmm"), "[pnpm] warn: hmm");
        assert_eq!(
            prefixed("pnpm", PMPX_LEVEL_INFO, "chose x"),
            "[pnpm] chose x"
        );
        assert_eq!(
            prefixed("pnpm", PMPX_LEVEL_DEBUG, "detail"),
            "[pnpm] detail"
        );
    }

    /// A plugin's line is attributed to it; the execution events belong to the run and are rendered
    /// there, so this module says so rather than staying silent about them.
    #[test]
    fn only_the_lines_this_module_owns_are_rendered_here() {
        assert_eq!(
            render(
                "pnpm",
                Event::PluginMessage {
                    level: PMPX_LEVEL_DEBUG,
                    text: "detail".to_string()
                }
            ),
            Rendered::Shown
        );
        assert_eq!(
            render("pnpm", Event::Finished { code: 0 }),
            Rendered::NotShown
        );
    }
}
