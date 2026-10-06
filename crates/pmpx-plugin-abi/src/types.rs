//! The `#[repr(C)]` data that crosses the boundary, and the numbers that go with it.
//!
//! Lifetime rules, in one place, because everything here depends on them:
//!
//! - Every borrowed view (`PmpxStr`) is valid for the duration of the call it arrived with, and
//!   must not be kept.
//! - Memory is freed by the side that allocated it. The host frees what it hands over; a plugin
//!   frees its own output through [`PmpxCommandCap::free_command`] and its own strings through
//!   [`PmpxIdentity::free_str`].
//! - A panic must not cross: the plugin's shell wraps every entry point in `catch_unwind`.

use core::ffi::c_void;

/// The semantic major version: the revision of the *keys and capabilities* both sides were built
/// against.
///
/// It changes only when the meaning of something that already exists changes -- a key's meaning, a
/// field's layout, a verb number, an error code. Adding a key, a capability or a field at the end of
/// a table does **not** change it; that is the whole point of the key-value context and of the
/// capability lookup.
pub const PMPX_ABI_MAJOR: u32 = 3;

/// The most elements any one context array or map may have.
///
/// A length is one side's word and the other side does not trust it: a length this size is refused
/// with [`PMPX_ERR_INVALID_ARGS`], because `usize::MAX` would otherwise abort inside an allocation
/// before the first element was read.
pub const PMPX_MAX_ITEMS: usize = 4096;

// ---- Verbs ----

/// Install dependencies. An empty argument list means "the whole tree".
pub const PMPX_VERB_INSTALL: u32 = 0;
/// Remove dependencies.
pub const PMPX_VERB_REMOVE: u32 = 1;
/// Update dependencies.
pub const PMPX_VERB_UPDATE: u32 = 2;
/// Run a script or target.
pub const PMPX_VERB_RUN: u32 = 3;
/// Build.
pub const PMPX_VERB_BUILD: u32 = 4;
/// Test.
pub const PMPX_VERB_TEST: u32 = 5;
/// Run a command the user typed, with no interpretation.
pub const PMPX_VERB_EXEC: u32 = 6;

// ---- Why a plugin was selected ----

/// It won on evidence.
pub const PMPX_REASON_SCORED: u32 = 0;
/// The project config pins its family to it.
pub const PMPX_REASON_PINNED: u32 = 1;
/// The user named it on the command line.
pub const PMPX_REASON_EXPLICIT: u32 = 2;

// ---- Return codes ----

/// Success: the output struct has been filled in.
pub const PMPX_OK: u32 = 0;
/// This plugin does not do that verb.
///
/// **A plugin must answer this for a verb number it does not recognise**, rather than
/// [`PMPX_ERR_INVALID_ARGS`]: that is what keeps a new verb additive, because it leaves the host's
/// degradation path (running the user's command verbatim for `exec`) available.
pub const PMPX_ERR_UNSUPPORTED_VERB: u32 = 1;
/// The call itself was malformed: a null output pointer, a length without an array, a length beyond
/// [`PMPX_MAX_ITEMS`], a context smaller than this side knows, or text that is not UTF-8.
pub const PMPX_ERR_INVALID_ARGS: u32 = 2;
/// Anything else, including a panic that was contained.
///
/// **A host must treat a code it does not know as this one.**
pub const PMPX_ERR_INTERNAL: u32 = 3;

// ---- Message levels ----

/// Something went wrong.
pub const PMPX_LEVEL_ERROR: u32 = 0;
/// Something is off, but the command still runs.
pub const PMPX_LEVEL_WARN: u32 = 1;
/// An ordinary note about what the plugin decided.
pub const PMPX_LEVEL_INFO: u32 = 2;
/// Detail for whoever is debugging.
pub const PMPX_LEVEL_DEBUG: u32 = 3;

/// A borrowed byte string.
///
/// Two different "nothing" values, and the difference matters:
///
/// - **absent** (`ptr == null`): the host has nothing under this key. A plugin must treat it as
///   "the host does not know this", never as an error.
/// - **empty** (`len == 0`, `ptr != null`): the value is there and is empty.
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PmpxStr {
    /// Start of the bytes; null means the value is absent.
    pub ptr: *const u8,
    /// Length in bytes.
    pub len: usize,
}

impl PmpxStr {
    /// The absent value.
    pub const EMPTY: Self = Self {
        ptr: core::ptr::null(),
        len: 0,
    };

    /// A view over `len` bytes at `ptr`.
    pub const fn new(ptr: *const u8, len: usize) -> Self {
        Self { ptr, len }
    }

    /// Whether the host had nothing to say.
    ///
    /// Not a const fn: pointer::is_null only became const-stable in 1.84, and this crate's MSRV
    /// is the floor for plugin authors.
    pub fn is_absent(&self) -> bool {
        self.ptr.is_null()
    }

    /// Whether the value is present and empty.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The bytes, or `None` when the value is absent.
    ///
    /// # Safety
    /// `ptr` and `len` must describe memory that is valid for reads for as long as the returned
    /// slice is used -- which, per the ABI, is the duration of the call this value arrived with.
    pub unsafe fn as_bytes(&self) -> Option<&[u8]> {
        if self.ptr.is_null() {
            return None;
        }
        // SAFETY: the caller promises the range is valid; null was handled above.
        Some(unsafe { core::slice::from_raw_parts(self.ptr, self.len) })
    }
}

/// A borrowed array: a pointer and a length, as two fields rather than a Rust slice, because a
/// slice's layout is not part of the C ABI.
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PmpxSlice<T> {
    /// First element; null means the array is absent.
    pub ptr: *const T,
    /// Number of elements.
    pub len: usize,
}

impl<T> PmpxSlice<T> {
    /// The absent array.
    pub const EMPTY: Self = Self {
        ptr: core::ptr::null(),
        len: 0,
    };

    /// An array of `len` elements at `ptr`.
    pub const fn new(ptr: *const T, len: usize) -> Self {
        Self { ptr, len }
    }

    /// Whether there is no array.
    pub fn is_absent(&self) -> bool {
        self.ptr.is_null()
    }

    /// The elements, or `None` when the array is absent.
    ///
    /// # Safety
    /// `ptr` and `len` must describe memory that is valid for reads for as long as the returned
    /// slice is used -- the duration of the call, per the ABI.
    pub unsafe fn as_slice(&self) -> Option<&[T]> {
        if self.ptr.is_null() {
            return None;
        }
        // SAFETY: the caller promises the range is valid; null was handled above.
        Some(unsafe { core::slice::from_raw_parts(self.ptr, self.len) })
    }
}

/// Everything the host knows about this call.
///
/// The scalars are fields because they are always there and cheap; everything else is reached
/// through the three accessors, which is what makes a new piece of context additive instead of a
/// layout change. See [`crate::keys`] for the keys of major version 3.
///
/// A field the host has nothing for answers `0` (or an absent [`PmpxStr`]). That is not an error:
/// another host may simply not know about it.
#[repr(C)]
pub struct PmpxContext {
    /// Size of this struct as the **host** built it. Read no further than this.
    pub size: usize,
    /// The verb, as `PMPX_VERB_*`.
    pub verb: u32,
    /// Why this plugin was selected, as `PMPX_REASON_*`.
    pub reason: u32,
    /// The evidence score it won with.
    pub score: u32,

    /// How many values `key` has: a scalar the host has is `1`, anything else is `0`.
    ///
    /// # Safety
    /// `context` must be the pointer this table was reached through, and `key` must be valid for
    /// the duration of the call.
    pub count: unsafe extern "C" fn(context: *const PmpxContext, key: PmpxStr) -> usize,

    /// The `index`-th value of `key`, or an absent [`PmpxStr`].
    ///
    /// # Safety
    /// As [`PmpxContext::count`].
    pub get:
        unsafe extern "C" fn(context: *const PmpxContext, key: PmpxStr, index: usize) -> PmpxStr,

    /// The `index`-th *name* of `key`: a map key (a pin's family), not a value.
    ///
    /// # Safety
    /// As [`PmpxContext::count`].
    pub name:
        unsafe extern "C" fn(context: *const PmpxContext, key: PmpxStr, index: usize) -> PmpxStr,
}

/// The host, as the plugin sees it: a lookup for the host's capabilities.
#[repr(C)]
pub struct PmpxHost {
    /// The host's semantic major version.
    pub abi_major: u32,
    /// Size of this struct as the host built it.
    pub size: usize,
    /// Returns the table for `name`, or null when the host has no such capability.
    ///
    /// # Safety
    /// `name` must be valid for the duration of the call. The returned pointer must be checked
    /// against the capability's own `size` before any field past it is read.
    pub capability: unsafe extern "C" fn(name: PmpxStr) -> *const c_void,
}

/// The plugin, as the host sees it: one lookup plus whatever the plugin can do.
#[repr(C)]
pub struct PmpxPlugin {
    /// The plugin's semantic major version.
    pub abi_major: u32,
    /// The rustc version that built it. Diagnostics only.
    pub rustc_version: PmpxStr,
    /// The target triple that built it. Diagnostics only.
    pub target: PmpxStr,
    /// Returns the table for `name`, or null when the plugin has no such capability.
    ///
    /// # Safety
    /// `name` must be valid for the duration of the call.
    pub capability: unsafe extern "C" fn(name: PmpxStr) -> *const c_void,
}

/// One command, allocated by the plugin and freed by the plugin.
#[repr(C)]
pub struct PmpxCommand {
    /// Size of this struct as the **plugin** built it.
    pub size: usize,
    /// The program to run. Resolved through `PATH` by the host.
    pub program: PmpxStr,
    /// Its arguments, verbatim.
    pub args: PmpxSlice<PmpxStr>,
    /// Where to run it, or absent for "the project root".
    pub cwd: PmpxStr,
}

impl PmpxCommand {
    /// An empty command, to be filled in by a plugin that has not decided yet.
    pub const fn empty() -> Self {
        Self {
            size: core::mem::size_of::<Self>(),
            program: PmpxStr::EMPTY,
            args: PmpxSlice::EMPTY,
            cwd: PmpxStr::EMPTY,
        }
    }
}

/// The `identity` capability: who the plugin is.
#[repr(C)]
pub struct PmpxIdentity {
    /// Size of this struct as the plugin built it.
    pub size: usize,
    /// The plugin's name, which must equal the one in its manifest.
    ///
    /// # Safety
    /// The returned memory belongs to the plugin and must be released with
    /// [`PmpxIdentity::free_str`].
    pub name: unsafe extern "C" fn() -> PmpxStr,
    /// The plugin's ecosystem family.
    ///
    /// # Safety
    /// As [`PmpxIdentity::name`].
    pub family: unsafe extern "C" fn() -> PmpxStr,
    /// Release a string this plugin returned.
    ///
    /// # Safety
    /// `s` must come from this plugin and must be released only once.
    pub free_str: unsafe extern "C" fn(s: PmpxStr),
}

/// The `command` capability: the plugin's whole job.
#[repr(C)]
pub struct PmpxCommandCap {
    /// Size of this struct as the plugin built it.
    pub size: usize,
    /// Map one call to one command.
    ///
    /// # Safety
    /// - `context` must be a context the host built, with `size` set to what it built, valid and
    ///   read-only for the duration of the call;
    /// - every array the context points at must have at least the length it declares;
    /// - `out` must point at writable memory for a [`PmpxCommand`];
    /// - the call must not panic across the boundary.
    pub run: unsafe extern "C" fn(context: *const PmpxContext, out: *mut PmpxCommand) -> u32,
    /// Release what [`PmpxCommandCap::run`] filled in.
    ///
    /// # Safety
    /// `command` must come from one successful `run` on this plugin and must be released once.
    pub free_command: unsafe extern "C" fn(command: *mut PmpxCommand),
}

/// The optional `attach` capability: how a plugin receives the host's hooks.
#[repr(C)]
pub struct PmpxAttach {
    /// Size of this struct as the plugin built it.
    pub size: usize,
    /// Called once after loading, before any other call but the ones that read `identity`.
    ///
    /// # Safety
    /// `host` must point at a [`PmpxHost`] that stays alive, and at functions that stay callable,
    /// for as long as the plugin may call them -- normally the life of the host process.
    pub attach: unsafe extern "C" fn(host: *const PmpxHost),
}

/// The host's `log` capability.
#[repr(C)]
pub struct PmpxLog {
    /// Size of this struct as the host built it.
    pub size: usize,
    /// Print one message. A plugin never prints for itself: it does not know the id the host shows
    /// for it, or whether anyone asked for detail.
    ///
    /// # Safety
    /// `message` must be valid for the duration of the call; the host must not keep it.
    pub write: unsafe extern "C" fn(level: u32, message: PmpxStr),
    /// The loudest level the host will actually print. A plugin checks this **before** formatting
    /// anything, so a host that is not tracing pays nothing.
    pub max_level: u32,
}
