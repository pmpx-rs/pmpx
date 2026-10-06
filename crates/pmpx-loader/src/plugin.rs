//! Loading one plugin and calling it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use libloading::{Library, Symbol};
use pmpx_plugin_abi::{
    PmpxCommand, PmpxCommandCap, PmpxHost, PmpxIdentity, PmpxPlugin, PmpxStr, PMPX_ABI_MAJOR,
    PMPX_CAP_ATTACH, PMPX_CAP_COMMAND, PMPX_CAP_IDENTITY, PMPX_ENTRY_SYMBOL, PMPX_ERR_INTERNAL,
    PMPX_ERR_INVALID_ARGS, PMPX_ERR_UNSUPPORTED_VERB, PMPX_OK,
};

use crate::context::ContextSource;
use crate::error::{CallError, LoadError};

/// One command a plugin answered with.
///
/// The plugin produced it; this crate copied every byte out of the plugin's memory before the plugin
/// released it, so what is here belongs to the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// The program to run. Empty means the plugin answered success without naming one, which the
    /// engine reports -- this layer only copies what is there.
    pub program: OsString,
    /// Its arguments, verbatim.
    pub args: Vec<OsString>,
    /// Where to run it, or `None` for "the project root".
    pub cwd: Option<PathBuf>,
}

/// A loaded plugin: the library stays open, and every function pointer used below belongs to it.
pub struct Plugin {
    /// Held so the library is not unloaded while its function pointers are in use.
    _library: Library,
    root: *const PmpxPlugin,
    command: *const PmpxCommandCap,
    name: String,
    family: String,
}

impl Plugin {
    /// Open a library, check what it speaks, and read its identity.
    ///
    /// # Safety
    ///
    /// Loading a dynamic library runs whatever code is inside it, including its initializers. The
    /// caller must trust the file -- which in practice means it came from the plugin store, not from
    /// an untrusted download.
    pub unsafe fn open(path: &Path) -> Result<Self, LoadError> {
        // SAFETY: the caller vouches for the library.
        let library = unsafe { Library::new(path) }.map_err(|error| LoadError::Open {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;

        // The symbol carries the root structure's layout, so an old host looking for its own name
        // finds nothing instead of reading fields that moved.
        let root = {
            let mut symbol_name = PMPX_ENTRY_SYMBOL.as_bytes().to_vec();
            symbol_name.push(0);

            let entry: Symbol<unsafe extern "C" fn() -> *const PmpxPlugin> =
                // SAFETY: the symbol name is NUL-terminated and the type is the ABI's entry point.
                unsafe { library.get(symbol_name.as_slice()) }.map_err(|_| LoadError::NoEntrySymbol {
                    path: path.to_path_buf(),
                    symbol: PMPX_ENTRY_SYMBOL,
                })?;

            // SAFETY: the entry point returns the plugin's `'static` table.
            unsafe { entry() }
        };

        let negotiated = unsafe { negotiate(root, path) }?;

        Ok(Self {
            _library: library,
            root,
            command: negotiated.command,
            name: negotiated.name,
            family: negotiated.family,
        })
    }

    /// The name the plugin reports for itself. The caller still has to check it against the
    /// manifest: a mismatch means the wrong thing was installed.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The ecosystem family the plugin reports for itself.
    pub fn family(&self) -> &str {
        &self.family
    }

    /// Offer the host's hooks to the plugin, if it can take them.
    ///
    /// Called once after loading, before any call. A plugin without the `attach` capability simply
    /// does not get them: logging is optional, being called is not.
    pub fn attach(&self, host: &'static PmpxHost) {
        let Some(table) = (unsafe { capability(self.root, PMPX_CAP_ATTACH) }) else {
            return;
        };
        let attach: *const pmpx_plugin_abi::PmpxAttach = table.cast();
        // SAFETY: a non-null table from `capability`; the plugin promises it stays valid.
        unsafe { ((*attach).attach)(host) };
    }

    /// Ask the plugin to map one call to one command.
    pub fn call(&self, source: &ContextSource<'_>) -> Result<Command, CallError> {
        call_table(self.command, source)
    }
}

/// Call one `command` table.
///
/// Separate from [`Plugin`] because the call itself does not need the library handle -- only the
/// table -- and that is what lets the whole path be tested without a `dlopen`.
fn call_table(
    table: *const PmpxCommandCap,
    source: &ContextSource<'_>,
) -> Result<Command, CallError> {
    source.with_context(|context| {
        let mut out = PmpxCommand::empty();

        // SAFETY: `context` was built by `with_context` and is valid for this call; `out` is
        // writable local memory; the table was checked when the plugin was loaded.
        let code = unsafe { ((*table).run)(context, &mut out) };

        if code != PMPX_OK {
            return Err(code_to_error(code));
        }

        // Read everything out of the plugin's memory first, then let the plugin release it.
        let answer = unsafe { read_command(&out) };
        // SAFETY: `out` was filled in by one successful call to this plugin's `run`.
        unsafe { ((*table).free_command)(&mut out) };
        answer
    })
}

/// What `negotiate` found: the table this host calls, and the identity it read out of the plugin.
#[derive(Debug)]
pub(crate) struct Negotiated {
    pub(crate) command: *const PmpxCommandCap,
    pub(crate) name: String,
    pub(crate) family: String,
}

/// Check that a plugin speaks this ABI, and fetch the two tables a host cannot work without.
///
/// # Safety
///
/// `root` must point at a valid [`PmpxPlugin`] whose tables and functions stay valid for as long as
/// they are used.
pub(crate) unsafe fn negotiate(
    root: *const PmpxPlugin,
    path: &Path,
) -> Result<Negotiated, LoadError> {
    if root.is_null() {
        return Err(LoadError::NoEntrySymbol {
            path: path.to_path_buf(),
            symbol: PMPX_ENTRY_SYMBOL,
        });
    }

    // SAFETY: non-null, and the caller vouches for the table.
    let found = unsafe { (*root).abi_major };
    if found != PMPX_ABI_MAJOR {
        return Err(LoadError::Major {
            path: path.to_path_buf(),
            found,
            expected: PMPX_ABI_MAJOR,
        });
    }

    let identity = unsafe { required_table::<PmpxIdentity>(root, PMPX_CAP_IDENTITY, path) }?;
    let command = unsafe { required_table::<PmpxCommandCap>(root, PMPX_CAP_COMMAND, path) }?;

    // The identity's two strings belong to the plugin and are released straight after copying.
    // SAFETY: the table is valid, and `free_str` is the plugin's own release function.
    let name = unsafe { read_owned_str((*identity).name, (*identity).free_str)? };
    // SAFETY: as above.
    let family = unsafe { read_owned_str((*identity).family, (*identity).free_str)? };

    Ok(Negotiated {
        command,
        name,
        family,
    })
}

/// The table for one required capability, checked for presence and size.
///
/// # Safety
///
/// See [`negotiate`].
unsafe fn required_table<T>(
    root: *const PmpxPlugin,
    capability_name: &'static str,
    path: &Path,
) -> Result<*const T, LoadError> {
    let Some(table) = (unsafe { capability(root, capability_name) }) else {
        return Err(LoadError::MissingCapability {
            path: path.to_path_buf(),
            capability: capability_name,
        });
    };

    let table: *const T = table.cast();
    // Every table starts with its own size, so a plugin built against an *older*, smaller table is
    // refused instead of being read past its end.
    let found = unsafe { *table.cast::<usize>() };
    let expected = std::mem::size_of::<T>();
    if found < expected {
        return Err(LoadError::ShortTable {
            path: path.to_path_buf(),
            capability: capability_name,
            found,
            expected,
        });
    }

    Ok(table)
}

/// Ask a plugin for one capability table.
///
/// # Safety
///
/// `root` must be a valid plugin table.
unsafe fn capability(root: *const PmpxPlugin, name: &str) -> Option<*const std::ffi::c_void> {
    let lookup = unsafe { (*root).capability };
    let key = PmpxStr::new(name.as_ptr(), name.len());
    // SAFETY: the plugin's lookup is called with a borrow that outlives the call, and it returns
    // either null or a pointer to one of its own tables.
    let table = unsafe { lookup(key) };
    if table.is_null() {
        None
    } else {
        Some(table)
    }
}

/// Call one `-> PmpxStr` function and copy the answer before releasing it.
///
/// # Safety
///
/// `f` and `free` must come from the same plugin, and `free` must be that plugin's release function.
unsafe fn read_owned_str(
    f: unsafe extern "C" fn() -> PmpxStr,
    free: unsafe extern "C" fn(PmpxStr),
) -> Result<String, LoadError> {
    // SAFETY: the caller vouches for the function.
    let value = unsafe { f() };
    let bytes = unsafe { value.as_bytes() }.unwrap_or(&[]);
    let text = String::from_utf8_lossy(bytes).into_owned();
    // The memory belongs to the plugin: it is copied above and released here, once.
    unsafe { free(value) };
    Ok(text)
}

/// Copy one command out of the plugin's memory.
///
/// # Safety
///
/// `command` must have been filled in by a successful `run`, and its strings must be valid.
unsafe fn read_command(command: &PmpxCommand) -> Result<Command, CallError> {
    let expected = std::mem::size_of::<PmpxCommand>();
    if command.size < expected {
        return Err(CallError::ShortCommand {
            found: command.size,
            expected,
        });
    }

    // SAFETY: the fields are past the size the plugin declared, and the plugin filled them in.
    let program = unsafe { command.program.as_bytes() }.unwrap_or(&[]);
    // SAFETY: as above; a null array means "no arguments".
    let args = unsafe { command.args.as_slice() }.unwrap_or(&[]);
    // SAFETY: as above.
    let cwd = unsafe { command.cwd.as_bytes() };

    Ok(Command {
        program: pmpx_plugin_abi::bytes_to_os(program),
        args: args
            .iter()
            .map(|arg| {
                // SAFETY: every element is a view the plugin filled in.
                let bytes = unsafe { arg.as_bytes() }.unwrap_or(&[]);
                pmpx_plugin_abi::bytes_to_os(bytes)
            })
            .collect(),
        cwd: cwd.map(|bytes| PathBuf::from(pmpx_plugin_abi::bytes_to_os(bytes))),
    })
}

/// Translate one answer code.
///
/// An unknown code is kept as [`CallError::Unknown`] rather than guessed: the plugin may be newer
/// than this host, and the engine decides how loud to be about it.
fn code_to_error(code: u32) -> CallError {
    match code {
        PMPX_ERR_UNSUPPORTED_VERB => CallError::UnsupportedVerb,
        PMPX_ERR_INVALID_ARGS => CallError::InvalidArgs,
        PMPX_ERR_INTERNAL => CallError::Internal,
        other => CallError::Unknown(other),
    }
}

#[cfg(test)]
mod tests {
    use pmpx_plugin_abi::PmpxContext;

    use super::*;

    /// An identity table that answers with borrowed `'static` strings.
    static NAME: &[u8] = b"toy";
    static FAMILY: &[u8] = b"node";

    unsafe extern "C" fn toy_name() -> PmpxStr {
        PmpxStr::new(NAME.as_ptr(), NAME.len())
    }

    unsafe extern "C" fn toy_family() -> PmpxStr {
        PmpxStr::new(FAMILY.as_ptr(), FAMILY.len())
    }

    unsafe extern "C" fn toy_free_str(_s: PmpxStr) {
        // The fixture's strings are `'static`: nothing to release.
    }

    /// A command table whose `run` fills the out struct with plugin-owned memory.
    static COMMAND_TEXT: &[u8] = b"pnpm";
    static ARG_TEXT: &[u8] = b"install";

    unsafe extern "C" fn toy_run(_context: *const PmpxContext, out: *mut PmpxCommand) -> u32 {
        let args = vec![PmpxStr::new(ARG_TEXT.as_ptr(), ARG_TEXT.len())].into_boxed_slice();
        let len = args.len();
        let ptr = Box::into_raw(args) as *const PmpxStr;

        // SAFETY: the host promises a writable command.
        unsafe {
            (*out).program = PmpxStr::new(COMMAND_TEXT.as_ptr(), COMMAND_TEXT.len());
            (*out).args = pmpx_plugin_abi::PmpxSlice::new(ptr, len);
            (*out).cwd = PmpxStr::EMPTY;
        }
        PMPX_OK
    }

    unsafe extern "C" fn toy_free_command(command: *mut PmpxCommand) {
        // SAFETY: `run` leaked exactly this box, and the host frees once.
        let args = unsafe { &*command }.args;
        if !args.is_absent() {
            let raw = std::ptr::slice_from_raw_parts_mut(args.ptr as *mut PmpxStr, args.len);
            drop(unsafe { Box::from_raw(raw) });
        }
    }

    static IDENTITY: PmpxIdentity = PmpxIdentity {
        size: std::mem::size_of::<PmpxIdentity>(),
        name: toy_name,
        family: toy_family,
        free_str: toy_free_str,
    };

    static COMMAND: PmpxCommandCap = PmpxCommandCap {
        size: std::mem::size_of::<PmpxCommandCap>(),
        run: toy_run,
        free_command: toy_free_command,
    };

    unsafe extern "C" fn lookup(name: PmpxStr) -> *const std::ffi::c_void {
        let bytes = unsafe { name.as_bytes() }.unwrap_or(&[]);
        match bytes {
            b"identity" => std::ptr::from_ref(&IDENTITY).cast(),
            b"command" => std::ptr::from_ref(&COMMAND).cast(),
            _ => std::ptr::null(),
        }
    }

    fn plugin_table(major: u32) -> PmpxPlugin {
        PmpxPlugin {
            abi_major: major,
            rustc_version: PmpxStr::EMPTY,
            target: PmpxStr::EMPTY,
            capability: lookup,
        }
    }

    #[test]
    fn a_plugin_that_speaks_this_abi_is_accepted() {
        let table = plugin_table(PMPX_ABI_MAJOR);
        let negotiated =
            unsafe { negotiate(&table, Path::new("/x.so")) }.expect("should negotiate");

        assert_eq!(negotiated.name, "toy");
        assert_eq!(negotiated.family, "node");
    }

    #[test]
    fn a_plugin_from_another_major_is_refused() {
        let table = plugin_table(PMPX_ABI_MAJOR + 1);
        let error = unsafe { negotiate(&table, Path::new("/x.so")) }.unwrap_err();

        match error {
            LoadError::Major {
                found, expected, ..
            } => {
                assert_eq!(found, PMPX_ABI_MAJOR + 1);
                assert_eq!(expected, PMPX_ABI_MAJOR);
            }
            other => panic!("expected a major-version refusal, got {other:?}"),
        }
    }

    /// A capability the host needs but the plugin does not have: refused, and named.
    #[test]
    fn a_missing_required_capability_is_refused_by_name() {
        unsafe extern "C" fn only_identity(name: PmpxStr) -> *const std::ffi::c_void {
            let bytes = unsafe { name.as_bytes() }.unwrap_or(&[]);
            match bytes {
                b"identity" => std::ptr::from_ref(&IDENTITY).cast(),
                _ => std::ptr::null(),
            }
        }

        // A local, not a `static`: a table of raw pointers is not `Sync`, and there is no reason for
        // it to outlive this test.
        let table = PmpxPlugin {
            abi_major: PMPX_ABI_MAJOR,
            rustc_version: PmpxStr::EMPTY,
            target: PmpxStr::EMPTY,
            capability: only_identity,
        };

        let error = unsafe { negotiate(&table, Path::new("/x.so")) }.unwrap_err();
        match error {
            LoadError::MissingCapability { capability, .. } => assert_eq!(capability, "command"),
            other => panic!("expected a missing capability, got {other:?}"),
        }
    }

    /// A table smaller than this host knows how to read: refused rather than read past its end.
    #[test]
    fn a_table_smaller_than_this_host_expects_is_refused() {
        #[repr(C)]
        struct ShortTable {
            size: usize,
            name: unsafe extern "C" fn() -> PmpxStr,
        }

        unsafe extern "C" fn short_name() -> PmpxStr {
            PmpxStr::EMPTY
        }

        static SHORT: ShortTable = ShortTable {
            size: std::mem::size_of::<PmpxIdentity>() - 8,
            name: short_name,
        };

        unsafe extern "C" fn short_lookup(name: PmpxStr) -> *const std::ffi::c_void {
            let bytes = unsafe { name.as_bytes() }.unwrap_or(&[]);
            match bytes {
                b"identity" => std::ptr::from_ref(&SHORT).cast(),
                b"command" => std::ptr::from_ref(&COMMAND).cast(),
                _ => std::ptr::null(),
            }
        }

        let table = PmpxPlugin {
            abi_major: PMPX_ABI_MAJOR,
            rustc_version: PmpxStr::EMPTY,
            target: PmpxStr::EMPTY,
            capability: short_lookup,
        };

        let error = unsafe { negotiate(&table, Path::new("/x.so")) }.unwrap_err();
        match error {
            LoadError::ShortTable {
                capability,
                found,
                expected,
                ..
            } => {
                assert_eq!(capability, "identity");
                assert!(found < expected);
            }
            other => panic!("expected a short-table refusal, got {other:?}"),
        }
    }

    /// A call goes out, comes back as Rust values, and the plugin's memory is released exactly once.
    ///
    /// No library is involved: the call needs the table, not the handle, which is exactly why the
    /// two are separate functions.
    #[test]
    fn a_call_copies_the_command_and_releases_the_plugins_memory() {
        let root = PathBuf::from("/work/project");
        let matched = vec!["package.json".to_string()];
        let pins = std::collections::BTreeMap::new();
        let args = vec![OsString::from("left-pad")];
        let source = ContextSource {
            root: &root,
            start_dir: &root,
            matched: &matched,
            config_files: &[],
            pins: &pins,
            args: &args,
            verb: pmpx_plugin_abi::PMPX_VERB_INSTALL,
            reason: pmpx_plugin_abi::PMPX_REASON_SCORED,
            score: 100,
            files: &crate::context::NoFiles,
        };

        let command = call_table(&COMMAND, &source).expect("the fixture always answers ok");

        assert_eq!(command.program, OsString::from("pnpm"));
        assert_eq!(command.args, vec![OsString::from("install")]);
        assert_eq!(command.cwd, None);
    }

    #[test]
    fn every_answer_code_has_an_error() {
        assert_eq!(
            code_to_error(PMPX_ERR_UNSUPPORTED_VERB),
            CallError::UnsupportedVerb
        );
        assert_eq!(code_to_error(PMPX_ERR_INVALID_ARGS), CallError::InvalidArgs);
        assert_eq!(code_to_error(PMPX_ERR_INTERNAL), CallError::Internal);
        assert_eq!(code_to_error(99), CallError::Unknown(99));
        assert_eq!(code_to_error(99).code(), Some(99));
    }

    #[test]
    fn errors_say_what_is_wrong() {
        let error = LoadError::Major {
            path: PathBuf::from("/x.so"),
            found: 2,
            expected: 3,
        };
        let text = error.to_string();
        assert!(text.contains("/x.so"), "{text}");
        assert!(text.contains('3'), "{text}");

        assert!(CallError::UnsupportedVerb
            .to_string()
            .contains("does not support"));
    }
}
