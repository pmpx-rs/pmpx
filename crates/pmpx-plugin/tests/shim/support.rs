//! The host role: reading C strings, reaching the vtable, and calling `command` once per the
//! contract.

use std::ffi::OsString;

use pmpx_plugin::abi::{self, PmpxCommand, PmpxContextV1, PmpxPluginV1, PmpxStr};
use pmpx_plugin::{CommandSpec, Verb};

use crate::pmpx_plugin_entry_v1;

/// Read a Rust string back from a C string.
///
/// # Safety
/// `s` must be a valid byte range in this process.
pub(crate) unsafe fn read(s: PmpxStr) -> String {
    if s.is_empty() {
        return String::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(s.ptr, s.len) };
    String::from_utf8(bytes.to_vec()).expect("the tests only pass UTF-8")
}

pub(crate) fn entry() -> &'static PmpxPluginV1 {
    // SAFETY: the entry returns a 'static table.
    unsafe { &*pmpx_plugin_entry_v1() }
}

/// Build the context a host would hand over: the arguments as borrowed views, so it stays valid
/// for exactly as long as the borrows do.
pub(crate) fn context<'a>(
    root: &'a str,
    matched: &'a [&'a str],
    verb: u32,
    args: &'a [&'a str],
) -> (PmpxContextV1, Vec<PmpxStr>, Vec<PmpxStr>) {
    let matched_raw: Vec<PmpxStr> = matched
        .iter()
        .map(|s| PmpxStr {
            ptr: s.as_ptr(),
            len: s.len(),
        })
        .collect();
    let args_raw: Vec<PmpxStr> = args
        .iter()
        .map(|s| PmpxStr {
            ptr: s.as_ptr(),
            len: s.len(),
        })
        .collect();

    let mut ctx = PmpxContextV1::empty();
    ctx.root = PmpxStr {
        ptr: root.as_ptr(),
        len: root.len(),
    };
    ctx.matched = matched_raw.as_ptr();
    ctx.matched_len = matched_raw.len();
    ctx.verb = verb;
    ctx.args = args_raw.as_ptr();
    ctx.args_len = args_raw.len();

    (ctx, matched_raw, args_raw)
}

/// Call `command` once, copy the result into Rust values, then free the plugin's memory per the
/// contract.
pub(crate) fn call_command(
    root: &str,
    matched: &[&str],
    verb: Option<Verb>,
    args: &[&str],
) -> Result<CommandSpec, u32> {
    let e = entry();

    // The verb is either a real number or a deliberately out-of-range one.
    let verb_code = verb.map(Verb::to_abi).unwrap_or(99);
    let (ctx, _matched_raw, _args_raw) = context(root, matched, verb_code, args);

    let mut out = std::mem::MaybeUninit::<PmpxCommand>::uninit();

    // SAFETY: the context borrows arrays allocated by this function and stays alive for the call;
    // out points at local writable memory.
    let code = unsafe { (e.command)(&ctx as *const PmpxContextV1, out.as_mut_ptr()) };

    if code != abi::PMPX_OK {
        return Err(code);
    }

    // SAFETY: when code == PMPX_OK the plugin guarantees out has been filled in.
    let mut cmd = unsafe { out.assume_init() };

    // Copy this memory out before handing it back to the plugin -- the host only ever reads.
    let spec = unsafe {
        let program = read(cmd.program);
        let cwd = if cmd.cwd.is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(read(cmd.cwd)))
        };
        let args: Vec<OsString> = (0..cmd.args_len)
            .map(|i| OsString::from(read(*cmd.args.add(i))))
            .collect();

        CommandSpec {
            program: program.into(),
            args,
            cwd,
        }
    };

    // SAFETY: this struct comes from the successful call above and is freed only once.
    unsafe { (e.free_command)(&mut cmd as *mut _) };

    Ok(spec)
}
