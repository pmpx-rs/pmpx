//! The error codes the host distinguishes, and the panic guard.

use pmpx_plugin::abi;
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
    let ctx = crate::support::context("/proj", &[], Verb::Install.to_abi(), &[]);

    // SAFETY: null is passed on purpose here, precisely so that it gets rejected.
    let code = unsafe { (e.command)(ctx.ptr(), std::ptr::null_mut()) };
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

/// A length with no array behind it is rejected rather than dereferenced.
///
/// Defence in depth -- `pmpx` itself always passes real pointers -- but the shell exists precisely
/// so the two sides need not trust each other, and a null pointer with a non-zero length is
/// undefined behaviour rather than an empty input.
#[test]
fn a_null_array_with_a_length_is_rejected() {
    let e = entry();
    let mut out = std::mem::MaybeUninit::<pmpx_plugin::abi::PmpxCommand>::uninit();

    for (matched_len, args_len) in [(0, 1), (1, 0)] {
        let mut ctx = crate::support::context("/proj", &[], Verb::Install.to_abi(), &[]);
        let raw = ctx.raw_mut();
        raw.matched = std::ptr::null();
        raw.matched_len = matched_len;
        raw.args = std::ptr::null();
        raw.args_len = args_len;

        // SAFETY: `out` is writable, and the null arrays are exactly what is being rejected --
        // nothing on this side reads them.
        let code = unsafe { (e.command)(ctx.ptr(), out.as_mut_ptr()) };

        assert_eq!(
            code,
            abi::PMPX_ERR_INVALID_ARGS,
            "matched_len {matched_len}, args_len {args_len} should be rejected"
        );
    }
}

/// A context the host built smaller than this plugin knows is refused rather than read past its
/// end -- the `size` field is exactly what makes that decidable.
#[test]
fn a_context_smaller_than_this_build_is_rejected() {
    let e = entry();
    let mut ctx = crate::support::context("/proj", &[], Verb::Install.to_abi(), &[]);
    ctx.raw_mut().size -= 1;
    let mut out = std::mem::MaybeUninit::<pmpx_plugin::abi::PmpxCommand>::uninit();

    // SAFETY: the context is valid apart from the size it claims, which is the point.
    let code = unsafe { (e.command)(ctx.ptr(), out.as_mut_ptr()) };

    assert_eq!(code, abi::PMPX_ERR_INVALID_ARGS);
}
