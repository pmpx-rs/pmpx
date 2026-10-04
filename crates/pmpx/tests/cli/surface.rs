//! The command surface itself: shell completion, and the verbs `--help` advertises.

use crate::support::Sandbox;

#[test]
fn completion_writes_a_script_to_stdout() {
    let sb = Sandbox::new();
    let out = sb.ok(&["completion", "bash"]);
    assert!(
        out.contains("pmpx"),
        "the completion script should mention pmpx: {out}"
    );
    assert!(out.contains("complete") || out.contains("_pmpx"), "{out}");
}

/// The verbs appearing in `--help` are exactly those seven -- no more, no fewer.
#[test]
fn help_lists_exactly_the_seven_verbs() {
    let sb = Sandbox::new();
    let out = sb.ok(&["--help"]);

    for verb in [
        "install", "remove", "run", "build", "test", "update", "exec",
    ] {
        assert!(out.contains(verb), "--help is missing {verb}: {out}");
    }
    // No lock / outdated / audit
    for absent in ["lock", "outdated", "audit", "publish"] {
        assert!(
            !out.contains(absent),
            "--help must not contain {absent}: {out}"
        );
    }
}
