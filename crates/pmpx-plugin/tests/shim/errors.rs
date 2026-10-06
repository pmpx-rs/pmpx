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

/// A length with no array behind it is rejected rather than dereferenced -- **for every array in
/// the context**, not just the two that existed first.
///
/// Defence in depth: `pmpx` itself always passes real pointers, but the shell exists precisely so
/// the two sides need not trust each other, and a null pointer with a non-zero length is undefined
/// behaviour rather than an empty input. `files`, `pins`, `scripts` and `config_paths` were added
/// to the context later, which is exactly how one of them came to be missing from this check.
#[test]
fn a_null_array_with_a_length_is_rejected() {
    /// Break one array of the context the way a buggy host would.
    type Break = fn(&mut abi::PmpxContextV1);

    let breaks: [(&str, Break); 6] = [
        ("matched", |c| {
            c.matched = std::ptr::null();
            c.matched_len = 1;
        }),
        ("args", |c| {
            c.args = std::ptr::null();
            c.args_len = 1;
        }),
        ("pins", |c| {
            c.pins = std::ptr::null();
            c.pins_len = 1;
        }),
        ("scripts", |c| {
            c.scripts = std::ptr::null();
            c.scripts_len = 1;
        }),
        ("config_paths", |c| {
            c.config_paths = std::ptr::null();
            c.config_paths_len = 1;
        }),
        ("files", |c| {
            c.files = std::ptr::null();
            c.files_len = 1;
        }),
    ];

    for (name, broken) in breaks {
        let answer = call_with(broken);

        assert_eq!(
            answer,
            abi::PMPX_ERR_INVALID_ARGS,
            "a null {name} array with a length should be rejected"
        );
    }
}

/// The same for a length no real context could have: reading it is not the risk (the pointer may be
/// real) -- allocating for it is, and `Vec::with_capacity(usize::MAX)` aborts.
#[test]
fn an_implausible_length_is_rejected() {
    type Break = fn(&mut abi::PmpxContextV1);

    let breaks: [(&str, Break); 6] = [
        ("matched", |c| c.matched_len = usize::MAX),
        ("args", |c| c.args_len = usize::MAX),
        ("pins", |c| c.pins_len = usize::MAX),
        ("scripts", |c| c.scripts_len = usize::MAX),
        ("config_paths", |c| c.config_paths_len = usize::MAX),
        ("files", |c| c.files_len = usize::MAX),
    ];

    for (name, broken) in breaks {
        let answer = call_with(broken);

        assert_eq!(
            answer,
            abi::PMPX_ERR_INVALID_ARGS,
            "an implausible {name} length should be rejected"
        );
    }
}

/// Call `command` once with a context that `break_it` has damaged.
fn call_with(break_it: fn(&mut abi::PmpxContextV1)) -> u32 {
    let e = entry();
    let mut ctx = crate::support::context("/proj", &[], Verb::Install.to_abi(), &[]);
    break_it(ctx.raw_mut());
    let mut out = std::mem::MaybeUninit::<pmpx_plugin::abi::PmpxCommand>::uninit();

    // SAFETY: `out` is writable, and whatever was broken is what the call has to reject: the shell
    // answers before dereferencing any of it.
    unsafe { (e.command)(ctx.ptr(), out.as_mut_ptr()) }
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
