//! Terminal styling.
//!
//! Colour is only ever decoration: every message has to read correctly with the codes
//! stripped, so nothing here changes wording. `anstream` decides whether the codes reach the
//! terminal at all -- a pipe, `NO_COLOR`, or a dumb terminal and they are gone.
//!
//! Escape codes are written here and nowhere else.

use std::fmt;

use anstyle::{AnsiColor, Style};

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
}
