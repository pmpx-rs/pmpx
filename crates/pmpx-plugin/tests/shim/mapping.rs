//! Translation: verb plus arguments in, one command out.

use std::ffi::OsString;

use pmpx_plugin::Verb;

use crate::support::call_command;

#[test]
fn install_maps_and_forwards_args() {
    let spec = call_command("/proj", &[], Some(Verb::Install), &["serde", "anyhow"])
        .expect("install should succeed");

    assert_eq!(spec.program, OsString::from("toy-bin"));
    assert_eq!(
        spec.args,
        vec![OsString::from("add"), "serde".into(), "anyhow".into()]
    );
    assert_eq!(spec.cwd, None, "cwd should be None when it is not set");
}

#[test]
fn install_with_no_args_still_maps() {
    let spec = call_command("/proj", &[], Some(Verb::Install), &[]).expect("should succeed");
    assert_eq!(spec.args, vec![OsString::from("add")]);
}

/// This is why `Context::matched` exists: the plugin branches on shape with it, without reading
/// any file.
#[test]
fn matched_drives_a_shape_branch() {
    let classic = call_command("/proj", &["yarn.lock"], Some(Verb::Run), &[]).unwrap();
    assert_eq!(classic.args, vec![OsString::from("classic")]);

    let berry = call_command("/proj", &["yarn.lock", ".yarnrc.yml"], Some(Verb::Run), &[]).unwrap();
    assert_eq!(berry.args, vec![OsString::from("berry")]);
}

#[test]
fn empty_matched_and_args_are_accepted() {
    let spec = call_command("", &[], Some(Verb::Run), &[]).expect("empty input should work too");
    assert_eq!(spec.args, vec![OsString::from("classic")]);
}
