//! The error codes the host distinguishes, and the panic guard.

use pmpx_plugin::abi::{self, PmpxStr};
use pmpx_plugin::Verb;

use crate::support::{call_command, entry};

#[test]
fn unsupported_verb_has_its_own_code() {
    let code = call_command("/proj", &[], Some(Verb::Exec), &[]).unwrap_err();
    assert_eq!(code, abi::PMPX_ERR_UNSUPPORTED_VERB);

    // It must be distinguishable from "other errors" -- the host relies on that to decide whether
    // to degrade
    let other = call_command("/proj", &[], Some(Verb::Build), &[]).unwrap_err();
    assert_eq!(other, abi::PMPX_ERR_INTERNAL);
}

#[test]
fn an_unknown_verb_number_is_rejected() {
    let code = call_command("/proj", &[], None, &[]).unwrap_err();
    assert_eq!(code, abi::PMPX_ERR_INVALID_ARGS);
}

#[test]
fn a_null_out_pointer_is_rejected() {
    let e = entry();
    let root = PmpxStr {
        ptr: "/proj".as_ptr(),
        len: 5,
    };
    let empty: Vec<PmpxStr> = Vec::new();

    // SAFETY: null is passed on purpose here, precisely so that it gets rejected.
    let code = unsafe {
        (e.command)(
            root,
            empty.as_ptr(),
            0,
            Verb::Install.to_abi(),
            empty.as_ptr(),
            0,
            std::ptr::null_mut(),
        )
    };
    assert_eq!(code, abi::PMPX_ERR_INVALID_ARGS);
}

/// A panic must be caught by `guard` and turned into an error code -- it must never cross
/// `extern "C"`. Since Rust 1.81 letting a panic through aborts the process and then this test
/// process would vanish entirely, so "the test finishes" is the conclusion by itself.
///
/// Note: the default panic hook prints the message to stderr, so the line
/// "this panic must be caught by guard" in the test output is expected, not a failure.
#[test]
fn a_panicking_plugin_does_not_take_the_host_down() {
    let code = call_command("/proj", &[], Some(Verb::Install), &["panic"]).unwrap_err();
    assert_eq!(code, abi::PMPX_ERR_INTERNAL);
}
