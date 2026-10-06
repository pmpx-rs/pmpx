//! What to say when a backend executable cannot be found.
//!
//! A bare "pnpm not found" leaves the user with nothing to act on, so a failed lookup lists the
//! near-matching names in PATH instead. This is why a resolution failure is more than a boolean.

use std::ffi::OsStr;
use std::path::PathBuf;

#[cfg(test)]
use std::path::Path;

use crate::error::PmpxError;

/// When nothing is found, say something that guides the next step: list the near-matching
/// files in PATH -- that is far more useful than a bare "pnpm not found".
pub(super) fn not_found_error(program: &OsStr) -> PmpxError {
    let wanted = program.to_string_lossy().to_ascii_lowercase();
    let near = near_misses(&wanted);

    let mut msg = format!("cannot find executable {}", program.to_string_lossy());

    if near.is_empty() {
        msg.push_str(
            "\nNothing in PATH has a similar name -- it is probably not installed at all.",
        );
    } else {
        msg.push_str("\nSimilar names in PATH:");
        for p in &near {
            msg.push_str(&format!("\n  - {}", p.display()));
        }
    }

    // The most common Windows case gets its own note
    #[cfg(windows)]
    if matches!(near.first().and_then(|p| p.extension()), Some(e) if e.eq_ignore_ascii_case("cmd"))
    {
        msg.push_str(
            "\n\nNote: on Windows these package managers are .cmd scripts and only PATHEXT \
             resolution finds them; pmpx already does that, so seeing this means that \
             directory really is not on PATH.",
        );
    }

    PmpxError::not_found(msg)
}

/// Whether a file name in PATH counts as a "near candidate" for `wanted`.
///
/// Prefix matching rather than edit distance: edit distance would also call `pnpm` and `npm`
/// similar, and those are two unrelated tools.
fn name_matches(wanted_lower: &str, candidate: &str) -> bool {
    let lower = candidate.to_ascii_lowercase();
    lower.starts_with(wanted_lower) && lower != wanted_lower
}

/// Find near-matching files in PATH.
fn near_misses(wanted_lower: &str) -> Vec<PathBuf> {
    // `var_os` rather than `var`: a PATH that is not valid UTF-8 is legal (any byte except NUL),
    // and `var` would report it as absent -- degrading the whole hint to "probably not installed
    // at all", which is the opposite of what this module is for.
    let Some(path_var) = std::env::var_os("PATH") else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for dir in std::env::split_paths(&path_var) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(std::result::Result::ok) {
            if name_matches(wanted_lower, &entry.file_name().to_string_lossy()) {
                out.push(entry.path());
            }
        }
        if out.len() >= 5 {
            break;
        }
    }

    out.sort();
    out.truncate(5);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spawn::resolve::resolve;

    // ---- near-miss classification --------------------------------------------

    #[test]
    fn a_cmd_shim_is_a_near_miss_for_its_bare_name() {
        assert!(name_matches("pnpm", "pnpm.cmd"));
        assert!(name_matches("pnpm", "PNPM.CMD"), "case-insensitive");
        assert!(name_matches("pnpm", "pnpm.cmd.old"));
    }

    /// Two names differing by one letter are not near candidates -- this is the reason for
    /// prefix matching instead of edit distance.
    #[test]
    fn a_different_tool_is_not_a_near_miss() {
        assert!(!name_matches("npm", "pnpm.cmd"));
        assert!(!name_matches("pnpm", "npmy"));
    }

    /// The known cost of prefix matching: `bun` also drags in `bunzip2`, `npm` also drags in
    /// `npmx`. Accepted rather than fixed -- edit distance would only be worse.
    #[test]
    fn prefix_matching_accepts_some_false_positives() {
        assert!(name_matches("bun", "bunzip2"));
        assert!(name_matches("npm", "npmx"));
    }

    #[test]
    fn an_exact_name_is_not_a_near_miss() {
        // An exact match means it would have been found anyway, so it does not belong in
        // "near candidates"
        assert!(!name_matches("cargo", "cargo"));
        assert!(!name_matches("cargo", "CARGO"));
    }

    // ---- what the user is told -----------------------------------------------

    /// The message for "not found" has to carry information.
    #[test]
    fn a_missing_program_says_something_useful() {
        let err = resolve(
            OsStr::new("pmpx-definitely-not-a-real-program-xyz"),
            Path::new("."),
        )
        .unwrap_err();
        let msg = err.to_string();

        assert!(
            msg.contains("pmpx-definitely-not-a-real-program-xyz"),
            "{msg}"
        );
        assert!(
            msg.contains("not installed") || msg.contains("Similar names"),
            "either say it is probably not installed, or list candidates: {msg}"
        );
    }
}
