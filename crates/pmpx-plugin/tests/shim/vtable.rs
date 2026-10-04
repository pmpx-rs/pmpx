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
