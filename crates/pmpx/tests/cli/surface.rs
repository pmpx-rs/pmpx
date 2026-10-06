//! The command surface itself: shell completion, and the verbs `--help` advertises.

use crate::support::{stderr_of, stdout_of, Sandbox};

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

/// The trace lives on stderr only, flag included: the whole point of `--debug` is that the
/// command's own output can still go through a pipe.
#[test]
fn debug_writes_the_trace_to_stderr_and_leaves_stdout_alone() {
    let sb = Sandbox::new();

    // `completion` opens no session, so this pins the flag, not the detection phases.
    let out = sb.run(&["--debug", "completion", "bash"]);
    assert!(out.status.success(), "{}", stderr_of(&out));

    let err = stderr_of(&out);
    assert!(err.contains("pmpx debug:"), "{err}");
    assert!(err.contains("cli.parse"), "{err}");
    assert!(err.contains("total"), "{err}");

    let stdout = stdout_of(&out);
    assert!(!stdout.contains("pmpx debug:"), "{stdout}");
    assert!(
        stdout.contains("complete") || stdout.contains("_pmpx"),
        "{stdout}"
    );
}

/// Without the flag there is no trace at all -- not even a header.
#[test]
fn no_debug_flag_means_no_trace() {
    let sb = Sandbox::new();
    let out = sb.run(&["completion", "bash"]);

    assert!(stderr_of(&out).is_empty(), "{}", stderr_of(&out));
}
