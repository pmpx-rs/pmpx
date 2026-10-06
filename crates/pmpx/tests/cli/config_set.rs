//! `config set` refuses a value the reader would choke on.
//!
//! The global config is read by every command, and its typed reader is strict about types: a typo
//! written into the file would otherwise make pmpx unusable until someone edited TOML by hand.

use crate::support::{stderr_of, stdout_of, Sandbox};

/// A value of the wrong type is refused, nothing is written, and the next command still works.
#[test]
fn a_value_the_reader_cannot_parse_is_refused_before_it_is_written() {
    let sb = Sandbox::new();

    let out = sb.run(&["config", "set", "discovery.walk_up", "maybe"]);

    assert_eq!(out.status.code(), Some(2), "it is a usage error");
    let err = stderr_of(&out);
    assert!(err.contains("not a valid value"), "{err}");
    assert!(err.contains("Nothing was written"), "{err}");
    assert!(
        !sb.config_dir.join("config.toml").exists(),
        "the file must not have been created"
    );

    // And the installation is still usable: this is the whole point.
    let after = sb.run(&["config", "get", "discovery.walk_up"]);
    assert_ne!(
        after.status.code(),
        Some(1),
        "a refused value must not leave a config nobody can read: {}",
        stderr_of(&after)
    );
}

/// The right type goes through, and reads back as the same value.
#[test]
fn a_value_of_the_right_type_is_written_and_read_back() {
    let sb = Sandbox::new();

    let out = sb.run(&["config", "set", "discovery.walk_up", "false"]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert!(stdout_of(&out).contains("Wrote"), "{}", stdout_of(&out));

    let got = sb.run(&["config", "get", "discovery.walk_up"]);
    assert_eq!(stdout_of(&got).trim(), "false");
}

/// An unknown key is allowed on purpose: the config keeps whatever a future version, or a person,
/// puts there.
#[test]
fn an_unknown_key_is_still_accepted() {
    let sb = Sandbox::new();

    let out = sb.run(&["config", "set", "my.own.key", "hello"]);

    assert!(out.status.success(), "{}", stderr_of(&out));
    assert!(stdout_of(&out).contains("Wrote"), "{}", stdout_of(&out));
}
