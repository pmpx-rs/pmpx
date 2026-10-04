//! Building the command line handed to `cmd.exe`.
//!
//! A `.cmd` / `.bat` cannot be started directly with arguments: `cmd /c` has its own quoting
//! rules, so the whole command line is built here as one string and delivered through
//! `raw_arg`. `quote_arg` follows the Windows `CommandLineToArgvW` rules, and it is kept in
//! the test build on every platform because those rules are exactly where mistakes happen.
//!
//! (`quote_arg` is a code span rather than a link on purpose: it only exists under
//! `cfg(any(windows, test))`, so a link to it would be unresolved in the Unix documentation
//! build, which is a `-D warnings` error there.)

#[cfg(windows)]
use std::ffi::OsString;
#[cfg(windows)]
use std::path::Path;

/// Build the complete raw command line handed to `cmd.exe` (including `/d /s /c` and the
/// quote pair for `/s` to strip).
///
/// It is split out for testability: after going through `raw_arg`, `Command::get_args()` is
/// empty and cannot be introspected, and the shape of this command line is exactly where
/// mistakes happen.
#[cfg(windows)]
pub(super) fn cmd_raw_command_line(program: &Path, args: &[OsString]) -> String {
    format!("/d /s /c \"{}\"", build_cmd_line(program, args))
}

/// Build one command line for `cmd /d /s /c`.
#[cfg(windows)]
fn build_cmd_line(program: &Path, args: &[OsString]) -> String {
    let mut line = quote_arg(&program.to_string_lossy());
    for a in args {
        line.push(' ');
        line.push_str(&quote_arg(&a.to_string_lossy()));
    }
    line
}

/// Quote one argument by the Windows command-line (`CommandLineToArgvW`) rules.
///
/// Both rules are counter-intuitive: `"` inside quotes has to be written `\"`; and
/// **backslashes are only special in front of a quote** -- the n backslashes before a quote
/// must be written as `2n+1`, and the n trailing backslashes must be doubled to `2n`
/// (otherwise they would swallow the closing quote).
///
/// It is really only called on Windows; the test build keeps it too so the quoting rules run
/// on all three platforms.
#[cfg(any(windows, test))]
fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
        return arg.to_string();
    }

    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');

    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => {
                backslashes += 1;
                out.push('\\');
            }
            '"' => {
                // The backslashes before the quote are doubled, plus one to escape the quote
                // itself
                for _ in 0..=backslashes {
                    out.push('\\');
                }
                out.push('"');
                backslashes = 0;
            }
            _ => {
                backslashes = 0;
                out.push(c);
            }
        }
    }

    // The backslashes before the closing quote are doubled as well
    for _ in 0..backslashes {
        out.push('\\');
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- quoting rules -------------------------------------------------------

    #[test]
    fn simple_args_are_not_quoted() {
        assert_eq!(quote_arg("add"), "add");
        assert_eq!(quote_arg("--noEmit"), "--noEmit");
        assert_eq!(quote_arg("C:/x/y"), "C:/x/y");
    }

    #[test]
    fn args_with_spaces_are_quoted() {
        assert_eq!(quote_arg("C:/Program Files/x"), "\"C:/Program Files/x\"");
    }

    #[test]
    fn empty_arg_becomes_empty_quotes() {
        // Without this an empty argument would simply vanish from the command line
        assert_eq!(quote_arg(""), "\"\"");
    }

    #[test]
    fn inner_quotes_are_escaped() {
        assert_eq!(quote_arg("say \"hi\""), "\"say \\\"hi\\\"\"");
    }

    /// Backslashes are only special before a quote -- so no spaces means no quotes are added,
    /// and there is no trailing-backslash problem either.
    #[test]
    fn an_arg_without_spaces_is_left_alone_even_with_backslashes() {
        assert_eq!(quote_arg("C:\\dir\\"), "C:\\dir\\");
        assert_eq!(quote_arg("C:\\x\\y"), "C:\\x\\y");
    }

    /// But once quotes are needed, the trailing backslashes must be doubled, otherwise they
    /// would escape the closing quote.
    #[test]
    fn trailing_backslashes_are_doubled_when_quoting() {
        assert_eq!(quote_arg("a b\\"), "\"a b\\\\\"");
        assert_eq!(quote_arg("C:\\a b\\"), "\"C:\\a b\\\\\"");
    }

    #[test]
    fn backslashes_before_a_quote_are_doubled_and_the_quote_escaped() {
        // `a\"` -> one backslash + a quote -> `a\\\"`
        assert_eq!(quote_arg("a\\\"b"), "\"a\\\\\\\"b\"");
    }

    #[test]
    fn a_plain_backslash_is_untouched_inside_quotes() {
        assert_eq!(quote_arg("C:\\a b\\c"), "\"C:\\a b\\c\"");
    }

    /// On Windows `.cmd` must be wrapped in `cmd /d /s /c`, with one quote pair at each end
    /// for `/s` to strip.
    #[cfg(windows)]
    #[test]
    fn a_cmd_shim_is_wrapped_in_cmd_exe() {
        let shim = Path::new("C:\\path with space\\pnpm.cmd");
        let args = vec![OsString::from("add"), OsString::from("serde")];

        let line = cmd_raw_command_line(shim, &args);

        assert!(line.starts_with("/d /s /c "), "{line}");
        // The outermost quote pair is there for `/s` to strip
        assert!(line.ends_with('"'), "{line}");
        assert_eq!(
            line.matches('"').count(),
            4,
            "one pair for the path + one outer pair: {line}"
        );
        assert!(line.contains("\"C:\\path with space\\pnpm.cmd\""), "{line}");
        assert!(line.ends_with("add serde\""), "{line}");
    }

    /// A path without spaces needs no quotes, but the outer pair must still be there --
    /// `/s`'s behaviour depends on it.
    #[cfg(windows)]
    #[test]
    fn the_outer_quote_pair_is_always_present() {
        let line = cmd_raw_command_line(Path::new("C:\\x\\pnpm.cmd"), &[]);
        assert_eq!(line, "/d /s /c \"C:\\x\\pnpm.cmd\"");
    }
}
