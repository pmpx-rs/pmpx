//! Terminal styling.
//!
//! Colour is only ever decoration: every message has to read correctly with the codes
//! stripped, so nothing here changes wording. `anstream` decides whether the codes reach the
//! terminal at all -- a pipe, `NO_COLOR`, or a dumb terminal and they are gone.
//!
//! Escape codes are written here and nowhere else.

use std::fmt;

use anstyle::{AnsiColor, Style};
use pmpx_engine::Plan;

/// An error word that has to stand out, such as the `pmpx:` prefix.
pub const ERROR: Style = AnsiColor::Red.on_default().bold();
/// The body of an error message.
pub const ERROR_BODY: Style = AnsiColor::Red.on_default();
/// A package manager name: `cargo`, `pnpm`, ...
pub const PM: Style = AnsiColor::Green.on_default().bold();
/// A field label.
pub const LABEL: Style = Style::new().bold();
/// Hints, suggestions and other secondary information.
pub const DIM: Style = Style::new().dimmed();

/// Paint `text`, for use inside a format string.
pub fn paint<T: fmt::Display>(style: Style, text: T) -> Painted<T> {
    Painted { style, text }
}

/// Paint `text` after padding it to `width`.
///
/// The padding has to happen first: the width in a format spec counts the escape codes as
/// columns, so padding an already painted string would break the alignment it exists for.
pub fn padded(style: Style, width: usize, text: impl fmt::Display) -> Painted<String> {
    paint(style, format!("{text:<width$}"))
}

/// A bold field label, padded to `width`.
pub fn label(width: usize, text: &str) -> Painted<String> {
    padded(LABEL, width, text)
}

/// The `pmpx -> <program> <args>` line shown at the start of every run.
///
/// Three colours so the shape reads at a glance: `pmpx ->` is dim (it is the host's own
/// framing), the program is the green package-manager style (it is the verb's addressee), and
/// the arguments are left at the terminal's default (they belong to the backend, not to pmpx).
///
/// Pure, so the shape can be asserted without touching a terminal.
pub fn announce_starting(plan: &Plan) -> String {
    let program = plan.program.to_string_lossy();
    // Build the arguments with their leading spaces, so an empty `args` produces no trailing
    // gap -- `cargo` alone is still readable, not `cargo `.
    let args: String = plan
        .args
        .iter()
        .map(|arg| format!(" {}", arg.to_string_lossy()))
        .collect();
    format!("{}{}{}", paint(DIM, "pmpx -> "), paint(PM, program), args,)
}

/// The `Display` adapter [`paint`] returns.
pub struct Painted<T> {
    /// The style to wrap the text in.
    style: Style,
    /// The text itself.
    text: T,
}

impl<T: fmt::Display> fmt::Display for Painted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{}{}",
            self.style.render(),
            self.text,
            self.style.render_reset()
        )
    }
}

/// The heading a family is grouped under in `plugin ls`, and shown as in `info`.
///
/// This is the host's presentation, which is exactly why it does **not** live in the plugin contract:
/// what an ecosystem is called in someone's terminal is not something a plugin needs to agree with.
/// An ecosystem the host has never heard of is shown by its own name, which is the honest answer.
pub(crate) fn family_label(family: &str) -> &str {
    match family {
        "node" => "Node / frontend",
        "rust" => "Rust",
        "python" => "Python",
        "go" => "Go",
        "jvm" => "JVM",
        "dotnet" => ".NET",
        "php" => "PHP",
        "ruby" => "Ruby",
        other => other,
    }
}
#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    #[test]
    fn paint_keeps_the_text() {
        assert!(paint(DIM, "x").to_string().contains('x'));
        assert!(paint(ERROR, "boom").to_string().contains("boom"));
    }

    #[test]
    fn padded_keeps_the_text_and_the_padding() {
        assert!(padded(PM, 6, "pnpm").to_string().contains("pnpm  "));
        assert!(label(12, "Start").to_string().contains("Start       "));
    }

    /// Every part of the announcement survives: the dim prefix, the green program, the default
    /// arguments. `contains` ignores the ANSI codes that wrap each piece, so the test reads the
    /// announcement the way a `--no-color` terminal would.
    #[test]
    fn announce_starting_keeps_every_part() {
        let plan = Plan::new(OsString::from("cargo"))
            .arg(OsString::from("test"))
            .arg(OsString::from("--release"));
        let line = announce_starting(&plan);

        assert!(line.contains("pmpx -> "), "the dim prefix: {line}");
        assert!(line.contains("cargo"), "the program: {line}");
        assert!(line.contains("test"), "the first arg: {line}");
        assert!(line.contains("--release"), "the second arg: {line}");
    }

    /// A plan with no arguments announces just the program: no trailing space, no stray
    /// argument. The bug to catch is a stray space at the end -- the iteration over an empty
    /// `args` list must yield an empty `String`, not `" "` or similar. The check strips the
    /// ANSI codes so the assertion reads what a `--no-color` terminal would.
    #[test]
    fn announce_starting_with_no_args_omits_the_gap() {
        let plan = Plan::new(OsString::from("cargo"));
        let line = announce_starting(&plan);

        let stripped = strip_ansi(&line);
        assert_eq!(stripped, "pmpx -> cargo", "{stripped:?}");
    }

    /// Drop every CSI escape sequence the helper emits: `\x1b[<params><final>` where the final
    /// byte is in `0x40..=0x7e`. Good enough for the announcements, which use plain SGR codes.
    fn strip_ansi(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(idx) = rest.find('\x1b') {
            out.push_str(&rest[..idx]);
            let after = &rest[idx + 1..];
            // CSI: ESC [ ... <final byte in 0x40..=0x7E>
            if let Some(after_stripped) = after.strip_prefix('[') {
                let end = after_stripped
                    .find(|ch: char| {
                        let code = ch as u32;
                        (0x40..=0x7e).contains(&code)
                    })
                    .map(|idx| idx + 1)
                    .unwrap_or(after_stripped.len());
                rest = &after_stripped[end..];
                continue;
            }
            // Unknown escape: drop just the ESC and continue, so we never loop forever.
            rest = after;
        }
        out.push_str(rest);
        out
    }
}
