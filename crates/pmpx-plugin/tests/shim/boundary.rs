//! Bytes crossing the boundary verbatim.

// The one case here is Unix-specific: elsewhere `OsString` is WTF-8 underneath, so there is
// nothing to keep verbatim.
#![cfg(unix)]

use pmpx_plugin::abi::{self, PmpxCommand, PmpxStr};
use pmpx_plugin::Verb;

use crate::support::entry;

/// A non-UTF-8 path or argument must cross the boundary verbatim (on Unix).
#[test]
fn non_utf8_input_survives_the_boundary() {
    use std::os::unix::ffi::OsStrExt;

    // 0xFF is not valid UTF-8, but it is a legitimate path byte
    let raw = [b'/', b'x', 0xFF];
    let root = PmpxStr {
        ptr: raw.as_ptr(),
        len: raw.len(),
    };
    let e = entry();
    let empty: Vec<PmpxStr> = Vec::new();
    let mut out = std::mem::MaybeUninit::<PmpxCommand>::uninit();

    // SAFETY: called per the ABI, with the inputs valid for the whole call.
    let code = unsafe {
        (e.command)(
            root,
            empty.as_ptr(),
            0,
            Verb::Run.to_abi(),
            empty.as_ptr(),
            0,
            out.as_mut_ptr(),
        )
    };
    assert_eq!(code, abi::PMPX_OK);

    // This fake plugin does not look at project_root, so this only proves "it gets in without
    // blowing up"; the real lossless round trip is in the abi unit test
    // (read_os_keeps_arbitrary_bytes).
    let mut cmd = unsafe { out.assume_init() };
    unsafe { (e.free_command)(&mut cmd as *mut _) };

    let os = std::ffi::OsStr::from_bytes(&raw);
    assert_eq!(os.as_bytes(), raw);
}
