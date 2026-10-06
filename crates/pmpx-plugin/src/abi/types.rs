//! `#[repr(C)]` data and the integers that describe it.
//!
//! Everything that crosses the `dlopen` boundary is here: the string, command and vtable
//! structures, the ABI version, the error codes, the verb numbers and the build diagnostics. No
//! type in this file may have a layout that depends on a dependency version. The marshalling
//! helpers live in the sibling `marshal` module, and the output side in `dispatch`.

// ---- Version ----

/// Version of the cross-boundary layout, an independent integer, fully decoupled from the crate
/// version. Bump it by one only when the shape of [`PmpxPluginV1`] / [`PmpxContextV1`] /
/// [`PmpxCommand`] / [`PmpxStr`], the verb numbering, or the error-code semantics really change.
/// The host uses it as its only hard check and refuses to load when it does not match.
///
/// **2**: the plugin is handed a [`PmpxContextV1`] instead of four loose arrays, and a host-hooks
/// pointer ([`PmpxHostV1`]) it can log through.
pub const ABI_VERSION: u32 = 2;

// ---- Message levels ----
//
// What a plugin's `debug!` / `info!` / `warn!` / `error!` arrive as on the host side. The numbers
// are ordered, so "print this level and everything louder" is a comparison.

/// Something went wrong.
pub const PMPX_LEVEL_ERROR: u32 = 0;
/// Something is off, but the command still runs.
pub const PMPX_LEVEL_WARN: u32 = 1;
/// An ordinary note about what the plugin decided.
pub const PMPX_LEVEL_INFO: u32 = 2;
/// Detail for someone debugging.
pub const PMPX_LEVEL_DEBUG: u32 = 3;

// ---- Why this plugin was selected ----

/// It won on evidence (`score`).
pub const PMPX_REASON_SCORED: u32 = 0;
/// `.pmpx.toml` pins its family to it.
pub const PMPX_REASON_PINNED: u32 = 1;
/// `-p/--plugin` named it.
pub const PMPX_REASON_EXPLICIT: u32 = 2;

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

/// The exact shape of [`PmpxPluginV1::command`].
///
/// Spelled out once so the vtable field, the `export!` shell and the dispatcher cannot drift
/// apart. A mismatch would mean the host calling a function of a different shape -- undefined
/// behaviour that `ABI_VERSION` cannot catch, because the version would not change.
///
/// Everything the host has to say arrives in the [`PmpxContextV1`]: one pointer, so a future field
/// is a change to that struct alone and never to this signature again.
pub type CommandFn =
    unsafe extern "C" fn(context: *const PmpxContextV1, out: *mut PmpxCommand) -> u32;

/// The host's hooks: what a plugin may call back into.
///
/// The plugin never owns this struct -- the host keeps one `'static` and hands over a pointer that
/// stays valid for the life of the process.
#[repr(C)]
pub struct PmpxHostV1 {
    /// Size of this struct as the **host** built it. A plugin must not read past it.
    pub size: usize,

    /// Print one message. The host adds the prefix, the plugin's id, and decides whether to print
    /// at all -- a plugin does not know its own id as the host sees it, and must not have to.
    ///
    /// # Safety
    /// `message` must be valid for the duration of this call; the host may not keep it. Nothing is
    /// handed back, so there is no ownership to settle.
    pub log: unsafe extern "C" fn(level: u32, message: PmpxStr),

    /// The loudest level the host will actually print. A plugin checks this **before** formatting
    /// anything, which is what makes a silent host cost nothing.
    pub max_level: u32,
}

impl PmpxHostV1 {
    /// The hooks one host installs, together with the level it is willing to print.
    pub const fn new(log: unsafe extern "C" fn(u32, PmpxStr), max_level: u32) -> Self {
        Self {
            size: std::mem::size_of::<Self>(),
            log,
            max_level,
        }
    }
}

/// One `family = "plugin"` pin from a `.pmpx.toml`.
#[repr(C)]
pub struct PmpxPin {
    /// Family name, the same key `.pmpx.toml` uses.
    pub family: PmpxStr,
    /// The plugin that family is pinned to.
    pub plugin: PmpxStr,
}

/// One `key = "value"` pair -- the project config's `[scripts]`, which the host parses and does not
/// interpret.
#[repr(C)]
pub struct PmpxKeyValue {
    /// Key.
    pub key: PmpxStr,
    /// Value.
    pub value: PmpxStr,
}

/// The contents of one file a plugin asked for in its manifest's `[context] files`.
#[repr(C)]
pub struct PmpxFile {
    /// Path relative to the project root, as the manifest declared it.
    pub name: PmpxStr,
    /// The bytes, which need not be UTF-8: how to read them is the plugin's business.
    pub contents: PmpxStr,
    /// Non-zero when the file was larger than the host's limit, so `contents` is only its
    /// beginning.
    pub truncated: u32,
}

/// Everything the host knows about this call.
///
/// `size` comes first so a plugin built against an older, smaller context can tell how much of it
/// is really there -- and so a future host can grow the struct without this becoming a question
/// the plugin has to guess at.
///
/// **A field the host has nothing for is null (or zero).** That is "absent", not an error: a
/// different host may not know about pins, and a plugin must cope.
#[repr(C)]
pub struct PmpxContextV1 {
    /// Size of this struct as the **host** built it. Read it before anything past the fields this
    /// plugin was compiled with.
    pub size: usize,

    /// Project root: where the command will run, unless the answer names a `cwd` of its own.
    pub root: PmpxStr,

    /// The directory the person ran pmpx from (its `-C`, or the process's own directory). It
    /// differs from `root` whenever the project root was found by walking up, which is the only
    /// way to tell which package of a monorepo this is.
    ///
    /// **Not** where the command will run: that is the `cwd` of the answer, and it defaults to
    /// `root`. Calling this field `cwd` would invite exactly that confusion.
    pub start_dir: PmpxStr,

    /// The files this plugin's detection matched, relative to `root`.
    pub matched: *const PmpxStr,
    /// Number of elements in `matched`.
    pub matched_len: usize,

    /// The verb, numbered as [`VERB_INSTALL`] and friends.
    pub verb: u32,

    /// Why this plugin was selected: [`PMPX_REASON_SCORED`] and friends.
    pub reason: u32,

    /// The evidence score it won with (0 when it was pinned or named outright).
    pub score: u32,

    /// The arguments, as the person typed them.
    pub args: *const PmpxStr,
    /// Number of elements in `args`.
    pub args_len: usize,

    /// `[plugin]` pins from the project config, nearest layer wins.
    pub pins: *const PmpxPin,
    /// Number of elements in `pins`.
    pub pins_len: usize,

    /// The `.pmpx.toml` files that were read, nearest first.
    pub config_paths: *const PmpxStr,
    /// Number of elements in `config_paths`.
    pub config_paths_len: usize,

    /// `[scripts]` from those files.
    pub scripts: *const PmpxKeyValue,
    /// Number of elements in `scripts`.
    pub scripts_len: usize,

    /// What this plugin asked to see in its manifest's `[context] files`.
    pub files: *const PmpxFile,
    /// Number of elements in `files`.
    pub files_len: usize,
}

impl PmpxContextV1 {
    /// A context with every field absent, sized for this build.
    ///
    /// The host fills in what it has; whatever it leaves alone reads as "the host does not know
    /// this", which is exactly what a null field means.
    pub const fn empty() -> Self {
        Self {
            size: std::mem::size_of::<Self>(),
            root: PmpxStr::EMPTY,
            start_dir: PmpxStr::EMPTY,
            matched: std::ptr::null(),
            matched_len: 0,
            verb: 0,
            reason: PMPX_REASON_SCORED,
            score: 0,
            args: std::ptr::null(),
            args_len: 0,
            pins: std::ptr::null(),
            pins_len: 0,
            config_paths: std::ptr::null(),
            config_paths_len: 0,
            scripts: std::ptr::null(),
            scripts_len: 0,
            files: std::ptr::null(),
            files_len: 0,
        }
    }
}

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

    /// Translate one call into one command. When it returns [`PMPX_OK`], `out` has been filled in
    /// and the host calls [`free_command`](crate::abi::free_command) when done; otherwise it returns
    /// `PMPX_ERR_*` and `out` is untouched.
    ///
    /// # Safety
    /// - `context` must point at a [`PmpxContextV1`] the host built, with `size` set to the size it
    ///   built, valid and read-only for the duration of the call;
    /// - every array the context points at must have at least the length it declares -- a length
    ///   larger than the array cannot be detected on this side, unlike a null pointer with a
    ///   length, which is rejected with [`PMPX_ERR_INVALID_ARGS`];
    /// - `out` must point at a writable [`PmpxCommand`];
    /// - a panic must not cross this boundary: since Rust 1.81, unwinding across `extern "C"`
    ///   aborts the process and the host's `catch_unwind` cannot save it, so `export!` wraps every
    ///   shim -- this one and the `name` / `family` ones -- in `catch_unwind`.
    pub command: CommandFn,

    /// Hand the plugin the host's hooks.
    ///
    /// Called once after loading, before any other call except the ones that read `name` /
    /// `family`. Passing null detaches them, and the plugin then falls back to writing its own
    /// messages to stderr.
    ///
    /// # Safety
    /// `host` must point at a [`PmpxHostV1`] that stays alive, and at functions that stay callable,
    /// for as long as the plugin may call them -- normally the life of the host process.
    pub set_host: unsafe extern "C" fn(*const PmpxHostV1),

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
        let rustc = std::str::from_utf8(rustc).unwrap();
        let target = std::str::from_utf8(target).unwrap();

        // `build.rs` writes `unknown` when it could not ask, on purpose: this is diagnostics, not
        // a hard requirement. The assertion is therefore "filled in", not "a version".
        assert!(
            rustc == "unknown" || rustc.contains("rustc"),
            "rustc_version should look like `rustc 1.x.y (...)`: {rustc:?}"
        );
        assert!(
            target == "unknown" || target.contains('-'),
            "target should be a triple: {target:?}"
        );
    }
}
