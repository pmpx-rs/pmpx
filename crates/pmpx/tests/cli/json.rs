//! `--json`: what a program reads instead of the human text.

use crate::support::{sandbox_with_plugin, stderr_of, stdout_of};

/// Every line of stdout is one JSON object, and the outcome is in it.
///
/// The backend's own output is the interesting part: it is a child process, and it must not land in
/// this stream -- a caller is parsing it.
#[test]
fn json_prints_one_object_per_line() {
    let (sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    let out = sb.run(&["--json", "build"]);

    assert!(out.status.success(), "{}", stderr_of(&out));
    let stdout = stdout_of(&out);
    assert!(!stdout.trim().is_empty(), "{}", stderr_of(&out));

    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        assert!(
            line.starts_with('{') && line.ends_with('}'),
            "not one object per line: {line}"
        );
    }

    assert!(stdout.contains("\"event\":\"resolved\""), "{stdout}");
    assert!(stdout.contains("\"event\":\"starting\""), "{stdout}");
    assert!(stdout.contains("\"event\":\"finished\""), "{stdout}");
    assert!(stdout.contains("\"code\":0"), "{stdout}");
}

/// The backend's output goes to stderr, so it stays visible without joining the JSON stream.
#[test]
fn the_backends_own_output_goes_to_stderr() {
    let (sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    let out = sb.run(&["--json", "build"]);

    let stderr = stderr_of(&out);
    assert!(
        stderr.contains("pmpx-probe"),
        "the backend's line should be on stderr: {stderr}"
    );
    // Not a substring check: the probe text also appears *inside* the `starting` event, as one of the
    // arguments. What must not be on stdout is the backend's own line.
    let raw = stdout_of(&out);
    let leaked = raw
        .lines()
        .any(|line| line.trim_start().starts_with("pmpx-probe"));
    assert!(
        !leaked,
        "the backend's raw line must not be on stdout: {raw}"
    );
}

/// A command whose result is a human table refuses the flag instead of mixing prose into the stream.
#[test]
fn json_is_refused_where_it_would_mix_prose_in() {
    let (sb, _lib) = sandbox_with_plugin();

    let out = sb.run(&["--json", "plugin", "ls"]);

    assert_eq!(out.status.code(), Some(2), "{}", stderr_of(&out));
    assert!(
        stderr_of(&out).contains("not supported"),
        "{}",
        stderr_of(&out)
    );
}
