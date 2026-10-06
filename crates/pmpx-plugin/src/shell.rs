//! The plugin side of the ABI: everything between `export!`'s shims and the trait a plugin writes.
//!
//! This is the only module in the crate that touches raw memory. It owns three jobs:
//!
//! - **Answer the host's questions** through the accessors (`dispatch` is what `export!` points the
//!   `command` capability at),
//! - **Build the typed [`Context`]** out of them, so a plugin never sees a key or a pointer,
//! - **Hand the answer back** in the ABI's shape, with the memory this side owns.
//!
//! Everything here runs *inside* the plugin, on data the host lent it. The rules it obeys:
//!
//! - Nothing lent may be kept past the call.
//! - Memory is freed by the side that allocated it: what this module leases out for the answer is
//!   reclaimed by `free_command` / `free_str`, never by the host.
//! - A panic must not cross: the shims in `export!` wrap every entry point in `catch_unwind`.

use std::ffi::{OsStr, OsString};

use pmpx_plugin_abi::{
    PmpxCommand, PmpxContext, PmpxStr, PMPX_ERR_INTERNAL, PMPX_ERR_INVALID_ARGS,
    PMPX_ERR_UNSUPPORTED_VERB, PMPX_MAX_ITEMS, PMPX_OK,
};

use crate::{CommandSpec, Context, PackageManager, Verb};

/// What `name` / `family` answer when the plugin panicked before it could answer at all.
///
/// A marker rather than an empty string: the host refuses to load a plugin whose name disagrees with
/// its manifest, and this makes that refusal say what happened instead of showing an empty pair of
/// quotes.
pub const PANIC_MARKER: &str = "<the plugin panicked>";

/// Write a [`CommandSpec`] in its cross-boundary form, with the memory allocated by this side.
///
/// # Safety
/// `out` must point at writable memory for a [`PmpxCommand`].
pub unsafe fn write_command(out: *mut PmpxCommand, spec: CommandSpec) {
    let program = leak_bytes(&os_to_bytes(&spec.program));

    let args: Vec<PmpxStr> = spec
        .args
        .iter()
        .map(|arg| leak_bytes(&os_to_bytes(arg)))
        .collect();
    let args = args.into_boxed_slice();
    let args_len = args.len();
    let args = Box::into_raw(args) as *const PmpxStr;

    let cwd = match &spec.cwd {
        Some(dir) => leak_bytes(&os_to_bytes(dir.as_os_str())),
        None => PmpxStr::EMPTY,
    };

    // SAFETY: the caller promises writable memory; every pointer above belongs to this side and is
    // reclaimed by `free_command`.
    unsafe {
        (*out).size = std::mem::size_of::<PmpxCommand>();
        (*out).program = program;
        (*out).args = pmpx_plugin_abi::PmpxSlice::new(args, args_len);
        (*out).cwd = cwd;
    }
}

/// Release everything [`write_command`] allocated, without touching the struct itself (it lives on
/// the host's stack).
///
/// # Safety
/// `command` must come from one successful [`write_command`] on this side, and must be released
/// once.
pub unsafe fn free_command(command: *mut PmpxCommand) {
    if command.is_null() {
        return;
    }

    // SAFETY: the caller vouches for the pointer.
    let command = unsafe { &*command };

    free_str(command.program);
    free_str(command.cwd);

    if !command.args.is_absent() {
        // SAFETY: `write_command` leaked exactly this box.
        let args = unsafe {
            Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                command.args.ptr as *mut PmpxStr,
                command.args.len,
            ))
        };
        for arg in args.iter() {
            free_str(*arg);
        }
    }
}

/// Give one string back to this side's allocator.
///
/// # Safety
/// `s` must come from [`leak_bytes`] or [`leak_str`], and must be released once.
pub unsafe fn free_str(s: PmpxStr) {
    if s.ptr.is_null() {
        return;
    }
    // SAFETY: the caller vouches for the provenance and the single release.
    let bytes =
        unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(s.ptr as *mut u8, s.len)) };
    drop(bytes);
}

/// Lease out bytes for the host to read.
pub fn leak_bytes(bytes: &[u8]) -> PmpxStr {
    let boxed = bytes.to_vec().into_boxed_slice();
    let len = boxed.len();
    PmpxStr::new(Box::into_raw(boxed) as *const u8, len)
}

/// Lease out text for the host to read.
pub fn leak_str(s: &str) -> PmpxStr {
    leak_bytes(s.as_bytes())
}

/// Read a borrowed C string as text, checking UTF-8.
///
/// # Safety
/// `s` must be valid for the duration of the call.
pub unsafe fn read_str(s: PmpxStr) -> Option<&'static str> {
    // SAFETY: the caller vouches for the memory; the lifetime is deliberately tied to the call site
    // by making the caller keep the value inside it.
    let bytes = unsafe { s.as_bytes() }?;
    std::str::from_utf8(bytes).ok().map(|text| {
        // The bytes belong to the host and are valid for this call; saying `'static` here is what
        // lets the shells read a key without copying it. Every caller uses the value immediately,
        // inside the same call.
        unsafe { std::mem::transmute::<&str, &'static str>(text) }
    })
}

/// Read a borrowed C string as an owned `OsString`, losslessly where the platform allows it.
///
/// # Safety
/// As [`read_str`].
pub unsafe fn read_os(s: PmpxStr) -> OsString {
    // SAFETY: the caller vouches for the memory.
    let bytes = unsafe { s.as_bytes() }.unwrap_or(&[]);
    pmpx_plugin_abi::bytes_to_os(bytes)
}

/// The plugin-side entry point `export!` exposes as the `command` capability.
///
/// # Safety
/// See the safety notes of the ABI's `command` table: the context must be the host's, valid and
/// read-only for this call, and `out` must be writable.
pub unsafe fn dispatch(
    plugin: &dyn PackageManager,
    context: *const PmpxContext,
    out: *mut PmpxCommand,
) -> u32 {
    if out.is_null() || context.is_null() {
        return PMPX_ERR_INVALID_ARGS;
    }

    // SAFETY: non-null, and the host keeps it read-only for the call.
    let raw = unsafe { &*context };

    // The host says how much of the struct it built; reading past that would be reading whatever
    // happens to be there.
    if raw.size < std::mem::size_of::<PmpxContext>() {
        return PMPX_ERR_INVALID_ARGS;
    }

    // A verb this build does not know is answered as *unsupported*, not as invalid arguments: that
    // is what keeps a new verb additive, because it leaves the host's degradation path open.
    let Some(verb) = Verb::from_abi(raw.verb) else {
        return PMPX_ERR_UNSUPPORTED_VERB;
    };

    let Ok(args) = (unsafe { read_args(context) }) else {
        return PMPX_ERR_INVALID_ARGS;
    };

    // SAFETY: the context is the host's, valid for this call; the typed context borrows it only for
    // as long as this function runs.
    let context = unsafe { Context::from_host(context) };

    // Bracketed so that anything the plugin logs can say what it was working with, and so that a
    // plugin logging outside a call finds nothing rather than a stale one.
    let description = context.describe(verb, args.len());
    let answer = crate::debug::with_call(description, || plugin.command(&context, verb, &args));

    match answer {
        Ok(spec) => {
            // SAFETY: `out` was checked non-null, and the host promises it is writable.
            unsafe { write_command(out, spec) };
            PMPX_OK
        }
        Err(error) => error.code(),
    }
}

/// Read the arguments the host sent.
///
/// # Safety
/// `context` must be a valid host context.
unsafe fn read_args(context: *const PmpxContext) -> Result<Vec<OsString>, ()> {
    let raw = unsafe { &*context };
    let key = PmpxStr::new(
        pmpx_plugin_abi::PMPX_KEY_ARGS.as_ptr(),
        pmpx_plugin_abi::PMPX_KEY_ARGS.len(),
    );

    let count = unsafe { (raw.count)(context, key) };
    // A host that answers an absurd count is refused rather than allocated for.
    if count > PMPX_MAX_ITEMS {
        return Err(());
    }

    let mut args = Vec::with_capacity(count);
    for index in 0..count {
        let value = unsafe { (raw.get)(context, key, index) };
        if value.is_absent() {
            return Err(());
        }
        args.push(unsafe { read_os(value) });
    }
    Ok(args)
}

/// The `free_str` shim's implementation.
///
/// # Safety
/// As [`free_str`].
pub unsafe fn dispatch_free_str(s: PmpxStr) {
    // SAFETY: the caller (the host) only passes back what this side leased out.
    unsafe { free_str(s) };
}

/// The `free_command` shim's implementation.
///
/// # Safety
/// As [`free_command`].
pub unsafe fn dispatch_free_command(command: *mut PmpxCommand) {
    // SAFETY: as above.
    unsafe { free_command(command) };
}

/// Wrap one cross-boundary call in `catch_unwind`.
///
/// Since Rust 1.81, letting a panic cross an `extern "C"` boundary aborts the process outright, and
/// the host cannot help at all -- so the plugin has to catch it itself, here.
pub fn guard(f: impl FnOnce() -> u32) -> u32 {
    // `AssertUnwindSafe`: once the caller sees an error code it aborts the operation and never looks
    // at the borrowed state again, so there is nothing for the unwind-safety check to protect.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or(PMPX_ERR_INTERNAL)
}

/// The same, for one of the two `-> PmpxStr` shims.
pub fn guard_str(f: impl FnOnce() -> PmpxStr) -> PmpxStr {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| {
        let marker = leak_str(PANIC_MARKER);
        // The host frees what it is handed, including this marker.
        marker
    })
}

/// Read the arguments a command table was given, converting paths and arguments both ways.
///
/// Re-exported shape helpers: they exist so the shells and the tests do not each write their own
/// platform `cfg`.
pub(crate) fn os_to_bytes(s: &OsStr) -> Vec<u8> {
    pmpx_plugin_abi::os_to_bytes(s)
}

/// The rustc version this crate was built with, injected by `build.rs`. Diagnostics only.
pub const BUILD_RUSTC: &str = env!("PMPX_BUILD_RUSTC");

/// The target this crate was built for, injected by `build.rs`. Diagnostics only.
pub const BUILD_TARGET: &str = env!("PMPX_BUILD_TARGET");

/// Remember the plugin's own name, for the no-host fallback.
pub fn remember_name(name: &str) {
    crate::debug::remember_name(name);
}

/// Install the host's logging table, after the caller has checked its size.
///
/// # Safety
/// As [`debug`](mod@crate::debug)'s own note: the table must be the host's, large enough, and alive for the
/// process.
pub unsafe fn install_log_table(table: *const pmpx_plugin_abi::PmpxLog) {
    // SAFETY: the caller checked the size and the lifetime.
    unsafe { crate::debug::set_log_table(table) };
}
