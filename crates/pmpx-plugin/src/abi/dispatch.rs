//! The output side: writing a command into the boundary, the dispatch entry point, and the panic
//! guard around it.
//!
//! This logic lives here rather than inside the `export!` macro so that it can be tested directly.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::context::{ContextFile, SelectionReason};
use crate::{CommandSpec, Context, PackageManager, Verb};

use super::marshal::{free_str, leak_bytes, leak_str, os_to_bytes, read_bytes, read_os, read_str};
use super::types::{
    PmpxCommand, PmpxContextV1, PmpxStr, PMPX_ERR_INTERNAL, PMPX_ERR_INVALID_ARGS, PMPX_OK,
};

/// What `name` / `family` answer when the plugin panicked before it could answer at all.
///
/// A marker rather than an empty string: the host refuses to load a plugin whose name disagrees
/// with its manifest, and this makes that refusal say what happened instead of showing an empty
/// pair of quotes.
pub const PANIC_MARKER: &str = "<the plugin panicked>";

/// The most elements any one context array may hold.
///
/// A length is the host's word and this shell does not trust it: `usize::MAX` would abort inside
/// `Vec::with_capacity` before the first element was read, which is a crash the plugin could not
/// explain away. A real context holds a handful of entries; this is the same ceiling the host puts
/// on the arguments a plugin hands back.
const MAX_CONTEXT_ITEMS: usize = 4096;

/// Write a [`CommandSpec`] in its cross-boundary form, with the memory allocated by this side.
/// # Safety
/// `out` must point at a writable [`PmpxCommand`].
pub unsafe fn write_command(out: *mut PmpxCommand, spec: CommandSpec) {
    let program = leak_bytes(&os_to_bytes(&spec.program));

    let args: Vec<PmpxStr> = spec
        .args
        .iter()
        .map(|a| leak_bytes(&os_to_bytes(a)))
        .collect();
    let args_boxed: Box<[PmpxStr]> = args.into_boxed_slice();
    let args_len = args_boxed.len();
    let args_ptr = args_boxed.as_ptr();
    std::mem::forget(args_boxed);

    let cwd = match &spec.cwd {
        Some(p) => leak_bytes(&os_to_bytes(p.as_os_str())),
        None => PmpxStr::EMPTY,
    };

    unsafe {
        *out = PmpxCommand {
            program,
            args: args_ptr,
            args_len,
            cwd,
        };
    }
}

/// Free the contents of a [`PmpxCommand`] filled in by this side's [`write_command`], without
/// freeing `c` itself (that struct lives on the host side, usually on the stack).
/// # Safety
/// `c` must come from one successful `command` call on this side and may be freed only once.
pub unsafe fn free_command(c: *mut PmpxCommand) {
    if c.is_null() {
        return;
    }
    let cmd = unsafe { &*c };

    unsafe { free_str(cmd.program) };
    unsafe { free_str(cmd.cwd) };

    if !cmd.args.is_null() && cmd.args_len > 0 {
        // Strictly paired with the Box<[PmpxStr]> in write_command.
        let raw = std::ptr::slice_from_raw_parts_mut(cmd.args as *mut PmpxStr, cmd.args_len);
        let args = unsafe { Box::from_raw(raw) };
        for s in args.iter() {
            unsafe { free_str(*s) };
        }
    }
}

/// All the wiring of one `command` call: read the context, call
/// [`crate::PackageManager::command`], write the output.
/// This logic lives here rather than in the `export!` macro so that it can be tested directly.
/// # Safety
/// See the Safety section of [`PmpxPluginV1::command`](crate::abi::PmpxPluginV1::command). In
/// addition, `plugin` must be a valid instance in this process.
pub unsafe fn dispatch_command(
    plugin: &dyn PackageManager,
    context: *const PmpxContextV1,
    out: *mut PmpxCommand,
) -> u32 {
    if out.is_null() || context.is_null() {
        return PMPX_ERR_INVALID_ARGS;
    }

    // SAFETY: non-null, and the contract has the host keeping it read-only for this call.
    let context = unsafe { &*context };

    // The host says how much of the struct it built. A shorter one means the fields past its end
    // are not there to read; a longer one is a newer host whose prefix is this one, which is fine.
    if context.size < std::mem::size_of::<PmpxContextV1>() {
        return PMPX_ERR_INVALID_ARGS;
    }

    let Some(verb) = Verb::from_abi(context.verb) else {
        return PMPX_ERR_INVALID_ARGS;
    };

    // A length without an array is not "empty": reading it would be undefined behaviour, and a
    // bogus length would allocate before the first element is even touched. `pmpx` always passes a
    // real pointer -- it builds every array from a `Vec` -- but the whole point of this shell is not
    // to assume that. (A length *larger* than the caller's actual array cannot be detected here,
    // which is why the `command` contract puts that on the caller; what *can* be caught is a length
    // no real context would have, which would otherwise abort inside `Vec::with_capacity` -- a crash
    // the plugin could not explain.)
    let arrays = [
        (context.matched_len, context.matched.cast::<()>()),
        (context.args_len, context.args.cast::<()>()),
        (context.pins_len, context.pins.cast::<()>()),
        (context.scripts_len, context.scripts.cast::<()>()),
        (context.config_paths_len, context.config_paths.cast::<()>()),
        (context.files_len, context.files.cast::<()>()),
    ];
    if arrays
        .iter()
        .any(|(len, ptr)| *len > 0 && (ptr.is_null() || *len > MAX_CONTEXT_ITEMS))
    {
        return PMPX_ERR_INVALID_ARGS;
    }

    let project_root = PathBuf::from(unsafe { read_os(context.root) });

    // A path, so not necessarily UTF-8: read as bytes and build the `PathBuf` directly.
    let start_dir = PathBuf::from(unsafe { read_os(context.start_dir) });

    // `matched` is text (file names declared in the manifest), so UTF-8 is checked here.
    let mut matched_names = Vec::with_capacity(context.matched_len);
    for i in 0..context.matched_len {
        let raw = unsafe { *context.matched.add(i) };
        match unsafe { read_str(raw) } {
            Ok(s) => matched_names.push(s.to_string()),
            Err(code) => return code,
        }
    }

    // `args` are arguments and may be arbitrary bytes -- converted to OsString as-is, losslessly.
    let mut arg_list = Vec::with_capacity(context.args_len);
    for i in 0..context.args_len {
        let raw = unsafe { *context.args.add(i) };
        arg_list.push(unsafe { read_os(raw) });
    }

    // The config's own vocabulary is text: a family, a plugin name, a script name and its body.
    let mut pins = BTreeMap::new();
    for i in 0..context.pins_len {
        let raw = unsafe { *context.pins.add(i) };
        match (unsafe { read_str(raw.family) }, unsafe {
            read_str(raw.plugin)
        }) {
            (Ok(family), Ok(plugin)) => {
                pins.insert(family.to_string(), plugin.to_string());
            }
            _ => return PMPX_ERR_INVALID_ARGS,
        }
    }

    let mut scripts = BTreeMap::new();
    for i in 0..context.scripts_len {
        let raw = unsafe { *context.scripts.add(i) };
        match (unsafe { read_str(raw.key) }, unsafe { read_str(raw.value) }) {
            (Ok(key), Ok(value)) => {
                scripts.insert(key.to_string(), value.to_string());
            }
            _ => return PMPX_ERR_INVALID_ARGS,
        }
    }

    let mut config_files = Vec::with_capacity(context.config_paths_len);
    for i in 0..context.config_paths_len {
        let raw = unsafe { *context.config_paths.add(i) };
        config_files.push(PathBuf::from(unsafe { read_os(raw) }));
    }

    // What the plugin asked to see. The name is text -- the plugin wrote it in its own manifest --
    // so a broken one is a contract error; the contents are bytes and go through untouched.
    let mut files = Vec::with_capacity(context.files_len);
    for i in 0..context.files_len {
        let raw = unsafe { *context.files.add(i) };
        match unsafe { read_str(raw.name) } {
            Ok(name) => files.push(ContextFile {
                name: name.to_string(),
                bytes: unsafe { read_bytes(raw.contents) },
                truncated: raw.truncated != 0,
            }),
            Err(code) => return code,
        }
    }

    let context = Context {
        project_root,
        start_dir,
        matched: matched_names,
        reason: SelectionReason::from_abi(context.reason),
        score: context.score,
        pins,
        scripts,
        config_files,
        files,
    };

    // Bracketed so that anything the plugin logs can say what it was working with -- and so that a
    // plugin logging outside a call finds nothing rather than a stale one.
    let answer = crate::debug::with_call(
        crate::debug::Call {
            context: context.clone(),
            verb,
            args_len: arg_list.len(),
        },
        || plugin.command(&context, verb, &arg_list),
    );

    match answer {
        Ok(spec) => {
            unsafe { write_command(out, spec) };
            PMPX_OK
        }
        Err(e) => e.code(),
    }
}

/// Wrap one cross-boundary call in `catch_unwind`.
/// Since Rust 1.81, letting a panic cross an `extern "C"` boundary aborts the process outright,
/// and the host's `catch_unwind` cannot help at all, so the plugin has to catch it itself. The
/// host side wraps one more layer, for the cases where "the plugin forgot to wrap" or "the plugin
/// was built with `panic=abort`".
pub fn guard(f: impl FnOnce() -> u32) -> u32 {
    // `AssertUnwindSafe`: once the caller has PMPX_ERR_INTERNAL it aborts the operation and never
    // touches the caught state again.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or(PMPX_ERR_INTERNAL)
}

/// [`guard`] for the shims that answer with a [`PmpxStr`] instead of an error code: `name` and
/// `family` run plugin code too (the factory and the trait method), so they need exactly the same
/// protection -- a panic there aborts the host just as surely as one inside `command`.
///
/// The answer on a panic is [`PANIC_MARKER`], leaked like any other answer so the host frees it
/// the same way.
pub fn guard_str(f: impl FnOnce() -> PmpxStr) -> PmpxStr {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .unwrap_or_else(|_| leak_str(PANIC_MARKER))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_frees_a_command() {
        let spec = CommandSpec::new("cargo")
            .arg("add")
            .arg("serde")
            .cwd("/tmp/project");

        let mut out = std::mem::MaybeUninit::<PmpxCommand>::uninit();
        unsafe { write_command(out.as_mut_ptr(), spec) };
        let mut cmd = unsafe { out.assume_init() };

        assert_eq!(cmd.args_len, 2);
        let program = unsafe { std::slice::from_raw_parts(cmd.program.ptr, cmd.program.len) };
        assert_eq!(program, b"cargo");

        let arg0 = unsafe { *cmd.args.add(0) };
        let a0 = unsafe { std::slice::from_raw_parts(arg0.ptr, arg0.len) };
        assert_eq!(a0, b"add");

        let cwd = unsafe { std::slice::from_raw_parts(cmd.cwd.ptr, cmd.cwd.len) };
        assert_eq!(cwd, b"/tmp/project");

        unsafe { free_command(&mut cmd as *mut _) };
    }

    #[test]
    fn writes_a_command_with_no_args_and_no_cwd() {
        let spec = CommandSpec::new("cargo");

        let mut out = std::mem::MaybeUninit::<PmpxCommand>::uninit();
        unsafe { write_command(out.as_mut_ptr(), spec) };
        let mut cmd = unsafe { out.assume_init() };

        assert_eq!(cmd.args_len, 0);
        assert!(
            cmd.cwd.is_empty(),
            "cwd without an override should be EMPTY"
        );

        unsafe { free_command(&mut cmd as *mut _) };
    }

    #[test]
    fn free_command_tolerates_null() {
        unsafe { free_command(std::ptr::null_mut()) };
    }

    #[test]
    fn guard_turns_a_panic_into_internal_error() {
        assert_eq!(guard(|| PMPX_OK), PMPX_OK);
        assert_eq!(guard(|| panic!("the plugin blew up")), PMPX_ERR_INTERNAL);
    }
}
