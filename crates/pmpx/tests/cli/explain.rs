//! `--explain`: the decision, reported, and nothing run.

use crate::support::{sandbox_with_plugin, stderr_of, stdout_of};

/// The report names the installed plugin, its markers and the winner -- and no backend ran.
#[test]
fn explain_reports_without_running_anything() {
    let (sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    let out = sb.run(&["--explain", "build"]);

    assert!(out.status.success(), "{}", stderr_of(&out));
    let text = stdout_of(&out);
    assert!(text.contains("nothing was run"), "{text}");
    assert!(text.contains("installed"), "{text}");
    assert!(text.contains("markers"), "{text}");
    assert!(text.contains("fakepm.lock"), "{text}");
    assert!(text.contains("chosen"), "{text}");
    assert!(
        !text.contains("pmpx-probe"),
        "the backend must not have been started: {text}"
    );
}

/// With `--json` the same report is one object a program reads.
#[test]
fn explain_can_be_read_by_a_program() {
    let (sb, _lib) = sandbox_with_plugin();
    sb.file("fakepm.lock");

    let out = sb.run(&["--explain", "--json", "build"]);

    assert!(out.status.success(), "{}", stderr_of(&out));
    let text = stdout_of(&out);
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(lines.len(), 1, "one object, not a stream: {text}");
    assert!(
        lines[0].starts_with('{') && lines[0].ends_with('}'),
        "{text}"
    );
    assert!(text.contains("\"installed\""), "{text}");
    assert!(text.contains("\"chosen\""), "{text}");
    assert!(!text.contains("pmpx-probe"), "{text}");
}
