//! Why walking up stopped.
//!
//! Every stop reason is one sentence `pmpx info` shows: "why was no project found" is most often
//! answered by having hit one of these, so each one has to say something the user can act on.

/// Why walking up stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Reached the filesystem root.
    FilesystemRoot,
    /// Hit `$HOME` — anything above it is no longer the user's project.
    Home,
    /// Hit `.git`.
    GitRoot,
    /// Reached the `max_depth` limit.
    MaxDepth,
    /// The caller explicitly asked not to walk up (`--no-walk-up` or `[discovery] walk_up = false`).
    WalkUpDisabled,
}

impl StopReason {
    /// Human-readable explanation, for `pmpx info`.
    pub fn describe(self, max_depth: usize) -> String {
        match self {
            StopReason::FilesystemRoot => "reached the filesystem root".into(),
            StopReason::Home => "reached $HOME".into(),
            StopReason::GitRoot => "reached .git".into(),
            StopReason::MaxDepth => format!("reached the max_depth limit ({})", dirs(max_depth)),
            StopReason::WalkUpDisabled => "--no-walk-up / [discovery] walk_up = false".into(),
        }
    }
}

/// `1 directory` / `6 directories`.
///
/// Every count around here is routinely 1, and "Walked up 1 directories" is exactly the kind of
/// thing users report as a bug.
pub fn dirs(n: usize) -> String {
    format!("{n} director{}", if n == 1 { "y" } else { "ies" })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1 must not come out as "1 directories" -- these counts are routinely 1, and it is exactly
    /// the kind of thing users report as a bug.
    #[test]
    fn a_single_directory_is_not_plural() {
        assert_eq!(dirs(1), "1 directory");
        assert_eq!(dirs(0), "0 directories");
        assert_eq!(dirs(6), "6 directories");
        assert_eq!(
            StopReason::MaxDepth.describe(1),
            "reached the max_depth limit (1 directory)"
        );
    }

    #[test]
    fn every_stop_reason_has_a_description() {
        for (reason, max) in [
            (StopReason::FilesystemRoot, 8),
            (StopReason::Home, 8),
            (StopReason::GitRoot, 8),
            (StopReason::MaxDepth, 8),
            (StopReason::WalkUpDisabled, 8),
        ] {
            assert!(!reason.describe(max).is_empty());
        }
    }
}
