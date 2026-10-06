//! The vtable itself: what `export!` reports before any verb is called.

use pmpx_plugin::abi;

use crate::support::{entry, read};

#[test]
fn the_vtable_reports_a_compatible_abi_version() {
    assert_eq!(entry().abi_version, abi::ABI_VERSION);
}

#[test]
fn build_info_is_visible_for_diagnostics() {
    let e = entry();
    let rustc = unsafe { read(e.rustc_version) };
    let target = unsafe { read(e.target) };

    assert!(rustc.contains("rustc"), "rustc_version = {rustc:?}");
    assert!(target.contains('-'), "target = {target:?}");
}

#[test]
fn name_and_family_come_back_and_are_freed() {
    let e = entry();

    // SAFETY: the returned memory belongs to the plugin; free it with free_str after reading.
    unsafe {
        let name = (e.name)();
        assert_eq!(read(name), "toy");
        (e.free_str)(name);

        let family = (e.family)();
        assert_eq!(read(family), "node");
        (e.free_str)(family);
    }
}

/// `name` runs plugin code (the factory, then the trait method), so it needs the same protection
/// `command` has: without it this test would not fail, it would abort the whole test process.
///
/// The host turns the marker into its own message -- see `pmpx`'s `Backend::load`.
#[test]
fn a_panicking_name_is_contained() {
    let e = entry();

    crate::set_panic_in_name(true);
    let name = unsafe { (e.name)() };
    crate::set_panic_in_name(false);

    // SAFETY: the marker is this side's own leaked string, freed like any other answer.
    unsafe {
        assert_eq!(read(name), abi::PANIC_MARKER);
        (e.free_str)(name);
    }
}
