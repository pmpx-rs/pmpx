//! `#[repr(C)]` data and the integers that describe it.
//!
//! Everything that crosses the `dlopen` boundary is here: the string, command and vtable
//! structures, the ABI version, the error codes, the verb numbers and the build diagnostics. No
//! type in this file may have a layout that depends on a dependency version. The marshalling
//! helpers live in the sibling `marshal` module, and the output side in `dispatch`.

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

/// Invalid input -- an unknown verb number, a null `out`, a length without an array in `matched`
/// or `args`, or non-UTF-8 in `matched`.
pub const PMPX_ERR_INVALID_ARGS: u32 = 2;

/// The plugin failed internally, or it panicked (a panic is caught by [`guard`](crate::abi::guard)
/// and mapped here, with the details on stderr).
pub const PMPX_ERR_INTERNAL: u32 = 3;

// ---- Verb numbers ----

/// Number of [`Verb::Install`](crate::Verb::Install).
pub const VERB_INSTALL: u32 = 0;
/// Number of [`Verb::Remove`](crate::Verb::Remove).
pub const VERB_REMOVE: u32 = 1;
/// Number of [`Verb::Run`](crate::Verb::Run).
pub const VERB_RUN: u32 = 2;
/// Number of [`Verb::Build`](crate::Verb::Build).
pub const VERB_BUILD: u32 = 3;
/// Number of [`Verb::Test`](crate::Verb::Test).
pub const VERB_TEST: u32 = 4;
/// Number of [`Verb::Update`](crate::Verb::Update).
pub const VERB_UPDATE: u32 = 5;
/// Number of [`Verb::Exec`](crate::Verb::Exec).
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
/// plugin ([`free_command`](crate::abi::free_command)); the host only reads it.
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

    /// Plugin name. The memory belongs to the plugin; the host frees it with
    /// [`free_str`](crate::abi::free_str) after reading, and compares it against the name declared
    /// in the manifest -- a mismatch means the wrong thing was installed.
    pub name: unsafe extern "C" fn() -> PmpxStr,

    /// Ecosystem family. The memory belongs to the plugin; the host frees it with
    /// [`free_str`](crate::abi::free_str) after reading.
    pub family: unsafe extern "C" fn() -> PmpxStr,

    /// Translate "verb + arguments" into one command. When it returns [`PMPX_OK`], `out` has been
    /// filled in and the host calls [`free_command`](crate::abi::free_command) when done; otherwise
    /// it returns `PMPX_ERR_*` and `out` is untouched.
    ///
    /// # Safety
    /// - `project_root` / `matched` / `args` must be allocated by the host, valid and read-only
    ///   for the duration of the call;
    /// - `matched_len` / `args_len` must be the real lengths of those arrays -- a length larger
    ///   than the array cannot be detected on this side, unlike a null pointer with a length,
    ///   which is rejected with [`PMPX_ERR_INVALID_ARGS`];
    /// - `out` must point at a writable [`PmpxCommand`];
    /// - a panic must not cross this boundary: since Rust 1.81, unwinding across `extern "C"`
    ///   aborts the process and the host's `catch_unwind` cannot save it, so `export!` wraps every
    ///   shim -- this one and the `name` / `family` ones -- in `catch_unwind`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Verb;

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
