//! The wire format across the `dlopen` boundary.
//!
//! The host and the plugin are two separately compiled worlds, so data crossing this line can
//! only be `#[repr(C)]` POD and plain integers: there is no guarantee about which allocator owns
//! the memory of `String` / `Vec` / `Box`, the layout of `toml::Value` / `anyhow::Error` changes
//! with dependency patch versions, and nothing fixes which side a trait object's vtable belongs
//! to. The price is that the two sides share no allocator -- memory is always freed by the side
//! that allocated it: inputs passed in by the host are read-only, and outputs produced by the
//! plugin (`PmpxCommand` and the strings inside it, `name` / `family`) are handed back by the
//! host with [`free_command`] / [`free_str`]. So `free_*` must never use the host's
//! `Box::from_raw` to adopt memory that came from the plugin.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use crate::{CommandSpec, Context, PackageManager, Verb};

// ---- Version ----

/// Version of the cross-boundary layout, an independent integer, fully decoupled from the crate
/// version. Bump it by one only when the shape of [`PmpxPluginV1`] / [`PmpxCommand`] / [`PmpxStr`],
/// the verb numbering, or the error-code semantics really change. The host uses it as its only
/// hard check and refuses to load when it does not match.
pub const ABI_VERSION: u32 = 1;

// ---- Error codes ----

/// Success.
pub const PMPX_OK: u32 = 0;

/// This backend does not support that verb.
/// The host treats it specially: `pmpx exec` degrades to passing through verbatim when it sees
/// this code, while the other verbs report the error as-is, so it must stay separate from
/// [`PMPX_ERR_INTERNAL`].
pub const PMPX_ERR_UNSUPPORTED_VERB: u32 = 1;

/// Invalid input -- an unknown verb number, a null `out`, or non-UTF-8 in `matched`.
pub const PMPX_ERR_INVALID_ARGS: u32 = 2;

/// The plugin failed internally, or it panicked (a panic is caught by [`guard`] and mapped here,
/// with the details on stderr).
pub const PMPX_ERR_INTERNAL: u32 = 3;

// ---- Verb numbers ----

/// Number of [`Verb::Install`].
pub const VERB_INSTALL: u32 = 0;
/// Number of [`Verb::Remove`].
pub const VERB_REMOVE: u32 = 1;
/// Number of [`Verb::Run`].
pub const VERB_RUN: u32 = 2;
/// Number of [`Verb::Build`].
pub const VERB_BUILD: u32 = 3;
/// Number of [`Verb::Test`].
pub const VERB_TEST: u32 = 4;
/// Number of [`Verb::Update`].
pub const VERB_UPDATE: u32 = 5;
/// Number of [`Verb::Exec`].
pub const VERB_EXEC: u32 = 6;

// ---- Data structures ----

/// A cross-boundary string: pointer + length, with no NUL terminator required.
/// `ptr` / `len` describe raw bytes (possibly a path or a command-line argument) which on Unix
/// need not be valid UTF-8; UTF-8 is checked only where the data is explicitly required to be
/// text, and a failure returns [`PMPX_ERR_INVALID_ARGS`] rather than UB.
/// It carries a length instead of relying on NUL because the pointer returned by `str::as_ptr()`
/// is not guaranteed to be followed by a NUL.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct PmpxStr {
    /// Start address. May be null when `len == 0`.
    pub ptr: *const u8,
    /// Length in bytes.
    pub len: usize,
}

// SAFETY: `PmpxStr` is "a read-only byte range plus a length"; the only unsafe part of sharing it
// across threads is that the memory `ptr` points at must still be valid. Both origins of this
// struct are well defined:
//   - one passed in by the host: valid for the whole call;
//   - one produced by the plugin: points at a `Box<[u8]>` the plugin leaked, valid until
//     `free_str`.
// Neither is freed or written while it is shared. So marking it `Sync` holds.
unsafe impl Sync for PmpxStr {}

impl PmpxStr {
    /// Empty. `len == 0` with a null pointer -- in `cwd` this means "no override".
    pub const EMPTY: PmpxStr = PmpxStr {
        ptr: std::ptr::null(),
        len: 0,
    };

    /// Build from a `'static` string (`const` so that the `static` vtable of `export!` can be
    /// filled in at compile time).
    pub const fn from_static(s: &'static str) -> Self {
        Self {
            ptr: s.as_ptr(),
            len: s.len(),
        }
    }

    /// Whether it is empty.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// A command description that crosses the boundary. Filled in by the plugin and freed by the
/// plugin ([`free_command`]); the host only reads it.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct PmpxCommand {
    /// Executable.
    pub program: PmpxStr,
    /// Argument array, with `args_len` elements.
    pub args: *const PmpxStr,
    /// Number of elements in `args`.
    pub args_len: usize,
    /// Working-directory override. `len == 0` means use the project root given by the host.
    pub cwd: PmpxStr,
}

// SAFETY: Same as `PmpxStr` -- this struct is just "references to read-only bytes plus an array
// length".
unsafe impl Sync for PmpxCommand {}

/// The only struct a plugin exports, and it is that table of function pointers; once the host has
/// obtained it, every interaction goes through these pointers, with no trait object and none of
/// the UB that comes from converting between vtables.
#[repr(C)]
pub struct PmpxPluginV1 {
    /// Must equal [`ABI_VERSION`]. This is the first field the host compares.
    pub abi_version: u32,

    /// The rustc version that built this plugin, injected by `pmpx-plugin`'s build.rs.
    /// Diagnostics only, never a hard check -- plugins built by different rustcs can be loaded
    /// safely under this C ABI.
    pub rustc_version: PmpxStr,

    /// The target triple that built this plugin. Also diagnostics only.
    pub target: PmpxStr,

    /// Plugin name. The memory belongs to the plugin; the host frees it with [`free_str`] after
    /// reading, and compares it against the name declared in the manifest -- a mismatch means the
    /// wrong thing was installed.
    pub name: unsafe extern "C" fn() -> PmpxStr,

    /// Ecosystem family. The memory belongs to the plugin; the host frees it with [`free_str`]
    /// after reading.
    pub family: unsafe extern "C" fn() -> PmpxStr,

    /// Translate "verb + arguments" into one command. When it returns [`PMPX_OK`], `out` has been
    /// filled in and the host calls [`free_command`] when done; otherwise it returns `PMPX_ERR_*`
    /// and `out` is untouched.
    ///
    /// # Safety
    /// - `project_root` / `matched` / `args` must be allocated by the host, valid and read-only
    ///   for the duration of the call;
    /// - `out` must point at a writable [`PmpxCommand`];
    /// - a panic must not cross this boundary: since Rust 1.81, unwinding across `extern "C"`
    ///   aborts the process and the host's `catch_unwind` cannot save it, so `export!` wraps
    ///   everything in `catch_unwind`.
    pub command: unsafe extern "C" fn(
        project_root: PmpxStr,
        matched: *const PmpxStr,
        matched_len: usize,
        verb: u32,
        args: *const PmpxStr,
        args_len: usize,
        out: *mut PmpxCommand,
    ) -> u32,

    /// Free the memory held by the values returned from [`PmpxPluginV1::name`] /
    /// [`PmpxPluginV1::family`].
    ///
    /// # Safety
    /// `s` must come from the same plugin and may be freed only once.
    pub free_str: unsafe extern "C" fn(PmpxStr),

    /// Free the memory filled in by [`PmpxPluginV1::command`]'s [`PmpxCommand`], without freeing
    /// the struct itself (that struct lives on the host side).
    ///
    /// # Safety
    /// `c` must come from one successful `command` call on the same plugin and may be freed only
    /// once.
    pub free_command: unsafe extern "C" fn(*mut PmpxCommand),
}

// SAFETY: This struct is a read-only table filled in from compile-time constants: a few integers,
// two `'static` byte ranges, and five function pointers. It is never modified after that.
// Function pointers are `Sync` themselves.
unsafe impl Sync for PmpxPluginV1 {}

/// Name of the single entry symbol.
/// The symbol itself is defined inside the plugin by `pmpx_plugin::export!`, not here -- this
/// crate gets linked into every plugin, and defining the same `#[no_mangle]` symbol itself would
/// collide with the one `export!` generates. Its shape is
/// `extern "C" fn() -> *const PmpxPluginV1`.
pub const ENTRY_SYMBOL: &str = "pmpx_plugin_entry_v1";

// ---- Build info (diagnostics) ----

/// rustc version injected at compile time. `const fn` is deliberate: the `static` vtable that
/// `export!` generates has to be evaluated at compile time.
pub const fn build_rustc() -> PmpxStr {
    PmpxStr::from_static(env!("PMPX_BUILD_RUSTC"))
}

/// Target triple injected at compile time.
pub const fn build_target() -> PmpxStr {
    PmpxStr::from_static(env!("PMPX_BUILD_TARGET"))
}

// ---- Memory: allocation and freeing ----

/// Leak a byte range into a [`PmpxStr`] for the other side of the boundary to read.
/// Every string flowing out of the plugin uses the same allocation (`Box<[u8]>`), so [`free_str`]
/// has exactly one path and cannot end up freeing a `Box<[u8]>` as a `Box<str>`.
pub fn leak_bytes(bytes: &[u8]) -> PmpxStr {
    let boxed: Box<[u8]> = bytes.to_vec().into_boxed_slice();
    let out = PmpxStr {
        ptr: boxed.as_ptr(),
        len: boxed.len(),
    };
    std::mem::forget(boxed);
    out
}

/// The `&str` version of [`leak_bytes`].
pub fn leak_str(s: &str) -> PmpxStr {
    leak_bytes(s.as_bytes())
}

/// Free a [`PmpxStr`] produced by this side's [`leak_bytes`] / [`leak_str`].
/// A null pointer returns immediately (that is how [`PmpxStr::EMPTY`] is used); length 0 with a
/// non-null pointer is a legitimate allocation and goes through `Box::from_raw` normally.
///
/// # Safety
/// - `s` must come from this side's `leak_*`, never from an input the host passed in;
/// - it may be freed only once.
pub unsafe fn free_str(s: PmpxStr) {
    if s.ptr.is_null() {
        return;
    }
    let raw = std::ptr::slice_from_raw_parts_mut(s.ptr as *mut u8, s.len);
    // Strictly paired with the Box<[u8]> in leak_bytes.
    drop(unsafe { Box::from_raw(raw) });
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

// ---- Input direction: bytes <-> OsString ----

/// Read the bytes the host passed in as an `OsString`.
/// On Unix, paths and command-line arguments need not be valid UTF-8, and a `String` can only
/// convert lossily, which would silently corrupt calls like
/// `pmpx exec some-tool /latin1/path`; `OsString` keeps the raw bytes losslessly.
/// On Windows, `OsString` is WTF-8 underneath and unpaired surrogates degrade to lossy
/// replacement -- that is the platform's boundary.
///
/// # Safety
/// `s` must describe read-only memory that is valid for the duration of this call, or `len == 0`.
pub unsafe fn read_os(s: PmpxStr) -> OsString {
    if s.len == 0 {
        return OsString::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(s.ptr, s.len) };
    bytes_to_os(bytes)
}

/// Read the bytes the host passed in as a `&str`, checking UTF-8; a failure returns
/// [`PMPX_ERR_INVALID_ARGS`], and never `from_utf8_unchecked` -- that would assume the host is
/// always correct, and the whole job of this ABI is not to make that assumption.
/// # Safety
/// Same as [`read_os`].
pub unsafe fn read_str<'a>(s: PmpxStr) -> Result<&'a str, u32> {
    if s.len == 0 {
        return Ok("");
    }
    let bytes = unsafe { std::slice::from_raw_parts(s.ptr, s.len) };
    std::str::from_utf8(bytes).map_err(|_| PMPX_ERR_INVALID_ARGS)
}

/// Convert raw bytes into an `OsString`.
/// Public because the host side does the same thing (turning `project_root` and `args` into bytes
/// to send across the boundary), and a separate platform `cfg` on each side would be duplication
/// that inevitably drifts. Lossless on Unix; on other platforms `OsString` is WTF-8 underneath,
/// so non-UTF-8 degrades to U+FFFD.
#[cfg(unix)]
pub fn bytes_to_os(bytes: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(bytes.to_vec())
}

/// See [`bytes_to_os`] for the platform notes.
#[cfg(not(unix))]
pub fn bytes_to_os(bytes: &[u8]) -> OsString {
    String::from_utf8_lossy(bytes).into_owned().into()
}

/// Convert an `OsStr` into raw bytes. Strictly paired with [`bytes_to_os`]: lossless on Unix, on
/// the other platforms it goes through `to_string_lossy` and non-UTF-8 degrades to U+FFFD.
#[cfg(unix)]
pub fn os_to_bytes(s: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    s.as_bytes().to_vec()
}

/// See [`os_to_bytes`] for the platform notes.
#[cfg(not(unix))]
pub fn os_to_bytes(s: &OsStr) -> Vec<u8> {
    s.to_string_lossy().into_owned().into_bytes()
}

// ---- Output direction ----

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

// ---- Dispatch ----

/// All the wiring of one `command` call: read the inputs, call
/// [`crate::PackageManager::command`], write the output.
/// This logic lives here rather than in the `export!` macro so that it can be tested directly.
/// # Safety
/// See the Safety section of [`PmpxPluginV1::command`]. In addition, `plugin` must be a valid
/// instance in this process.
#[allow(clippy::too_many_arguments)]
pub unsafe fn dispatch_command(
    plugin: &dyn PackageManager,
    project_root: PmpxStr,
    matched: *const PmpxStr,
    matched_len: usize,
    verb: u32,
    args: *const PmpxStr,
    args_len: usize,
    out: *mut PmpxCommand,
) -> u32 {
    if out.is_null() {
        return PMPX_ERR_INVALID_ARGS;
    }

    let Some(verb) = Verb::from_abi(verb) else {
        return PMPX_ERR_INVALID_ARGS;
    };

    let project_root = PathBuf::from(unsafe { read_os(project_root) });

    // `matched` is text (file names declared in the manifest), so UTF-8 is checked here.
    let mut matched_names = Vec::with_capacity(matched_len);
    for i in 0..matched_len {
        let raw = unsafe { *matched.add(i) };
        match unsafe { read_str(raw) } {
            Ok(s) => matched_names.push(s.to_string()),
            Err(code) => return code,
        }
    }

    // `args` are arguments and may be arbitrary bytes -- converted to OsString as-is, losslessly.
    let mut arg_list = Vec::with_capacity(args_len);
    for i in 0..args_len {
        let raw = unsafe { *args.add(i) };
        arg_list.push(unsafe { read_os(raw) });
    }

    let ctx = Context {
        project_root,
        matched: matched_names,
    };

    match plugin.command(&ctx, verb, &arg_list) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verb_numbers_match_the_public_enum() {
        // Once the numbering slips, the host and the plugin disagree about "install" and nothing
        // fails to compile.
        assert_eq!(Verb::Install.to_abi(), VERB_INSTALL);
        assert_eq!(Verb::Remove.to_abi(), VERB_REMOVE);
        assert_eq!(Verb::Run.to_abi(), VERB_RUN);
        assert_eq!(Verb::Build.to_abi(), VERB_BUILD);
        assert_eq!(Verb::Test.to_abi(), VERB_TEST);
        assert_eq!(Verb::Update.to_abi(), VERB_UPDATE);
        assert_eq!(Verb::Exec.to_abi(), VERB_EXEC);
    }

    #[test]
    fn verb_round_trips() {
        for v in Verb::ALL {
            assert_eq!(Verb::from_abi(v.to_abi()), Some(*v));
        }
        assert_eq!(Verb::from_abi(99), None);
    }

    #[test]
    fn empty_str_reads_as_empty() {
        assert_eq!(unsafe { read_os(PmpxStr::EMPTY) }, OsString::new());
        assert_eq!(unsafe { read_str(PmpxStr::EMPTY) }.unwrap(), "");
    }

    #[test]
    fn leak_and_free_round_trip() {
        let s = leak_str("hello");
        assert_eq!(s.len, 5);
        assert_eq!(
            unsafe { std::slice::from_raw_parts(s.ptr, s.len) },
            b"hello"
        );
        unsafe { free_str(s) };
    }

    #[test]
    fn free_str_tolerates_null() {
        // EMPTY is passed to free_str unconditionally by free_command
        unsafe { free_str(PmpxStr::EMPTY) };
    }

    #[test]
    fn leak_and_free_an_empty_string() {
        // The Box<[u8]> of an empty string is a dangling pointer (non-null, len 0) and must still
        // free cleanly
        let s = leak_str("");
        assert_eq!(s.len, 0);
        assert!(!s.ptr.is_null(), "an empty Box dangles but is not null");
        unsafe { free_str(s) };
    }

    #[test]
    fn read_str_rejects_invalid_utf8() {
        let bytes = [0xff, 0xfe];
        let s = PmpxStr {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        };
        assert_eq!(unsafe { read_str(s) }, Err(PMPX_ERR_INVALID_ARGS));
    }

    #[test]
    fn read_os_round_trips_valid_utf8() {
        let bytes = "/tmp/projéct/ünïcode".as_bytes();
        let s = PmpxStr {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        };
        let got = unsafe { read_os(s) };
        assert_eq!(os_to_bytes(&got), bytes);
    }

    /// On Unix a path or an argument may be arbitrary bytes (0xFF is not valid UTF-8, but it is a
    /// legitimate path byte) and `OsString` must keep them losslessly.
    #[cfg(unix)]
    #[test]
    fn read_os_keeps_arbitrary_bytes_on_unix() {
        let bytes = [0x2f, 0x62, 0x61, 0x64, 0xff];
        let s = PmpxStr {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        };
        let got = unsafe { read_os(s) };
        assert_eq!(os_to_bytes(&got), bytes, "must be lossless on Unix");
    }

    /// Off Unix, `OsString` is WTF-8 underneath, so non-UTF-8 degrades to U+FFFD -- a platform
    /// boundary that the test pins down as known behaviour instead of pretending otherwise.
    #[cfg(not(unix))]
    #[test]
    fn read_os_replaces_invalid_utf8_off_unix() {
        let bytes = [0x2f, 0x62, 0xff];
        let s = PmpxStr {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        };
        let got = unsafe { read_os(s) };
        let expected = String::from_utf8_lossy(&bytes).into_owned().into_bytes();
        assert_eq!(os_to_bytes(&got), expected);
        assert_ne!(os_to_bytes(&got), bytes, "off Unix it really is lossy");
    }

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

    #[test]
    fn build_info_is_populated() {
        let rustc = build_rustc();
        let target = build_target();
        assert!(rustc.len > 0);
        assert!(target.len > 0);

        let rustc = unsafe { std::slice::from_raw_parts(rustc.ptr, rustc.len) };
        let target = unsafe { std::slice::from_raw_parts(target.ptr, target.len) };
        assert!(
            std::str::from_utf8(rustc).unwrap().contains("rustc"),
            "rustc_version should look like `rustc 1.x.y (...)`"
        );
        assert!(std::str::from_utf8(target).unwrap().contains('-'));
    }
}
