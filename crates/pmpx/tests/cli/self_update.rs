//! `self update` -- only the parts that do not talk to GitHub.

use crate::support::{stderr_of, Sandbox};

#[test]
fn self_help_describes_the_update() {
    let sb = Sandbox::new();

    // The subcommand is listed...
    let self_help = sb.ok(&["self", "--help"]);
    assert!(self_help.contains("update"), "{self_help}");

    // ...and it documents the three flags, which live one level further down.
    let update_help = sb.ok(&["self", "update", "--help"]);
    for flag in ["--check", "--version", "--force"] {
        assert!(
            update_help.contains(flag),
            "self update should document {flag}: {update_help}"
        );
    }
}

/// A version that is not one is a typo on the command line, so it is a usage error -- and
/// it has to be caught before anything is asked of the network.
#[test]
fn self_update_rejects_a_version_that_is_not_one() {
    let sb = Sandbox::new();
    let out = sb.run(&["self", "update", "--version", "not-a-version"]);

    assert_eq!(
        out.status.code(),
        Some(2),
        "a bad --version is a usage error: {}",
        stderr_of(&out)
    );
    assert!(
        stderr_of(&out).contains("not a version"),
        "{}",
        stderr_of(&out)
    );
}
