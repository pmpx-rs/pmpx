//! Answering the key-value questions a plugin asks during a call.
//!
//! The host builds one [`PmpxContext`] per call, points its three accessors here, and keeps a
//! [`State`] alive for as long as the call lasts. Every value a plugin asks for is either borrowed
//! from the source or copied once into a buffer owned by that state, so what the accessors hand out
//! stays valid until the plugin returns.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use pmpx_plugin_abi::{
    PmpxContext, PmpxStr, PMPX_KEY_ARGS, PMPX_KEY_CONFIG_PIN, PMPX_KEY_FILE_PREFIX,
    PMPX_KEY_PROJECT_CONFIG_FILES, PMPX_KEY_PROJECT_MATCHED, PMPX_KEY_PROJECT_ROOT,
    PMPX_KEY_PROJECT_START_DIR,
};

/// Where the contents of a declared file come from.
///
/// The loader does no file I/O of its own: a plugin's manifest lists the files it wants to see, and
/// whoever owns the project (the engine) decides how to read them, with what limits, and what a
/// failure means. This trait is that seam -- and the reason a plugin can never reach past the names
/// it declared, because this is the only way it can ask.
pub trait Files {
    /// The contents of one file, or `None` when there is nothing to hand over.
    ///
    /// Called at most once per name per call.
    fn contents(&self, name: &str) -> Option<Vec<u8>>;
}

/// A provider with nothing to offer: what a host that does not implement `[context] files` uses.
pub struct NoFiles;

impl Files for NoFiles {
    fn contents(&self, _name: &str) -> Option<Vec<u8>> {
        None
    }
}

/// Everything the host can answer for one call.
pub struct ContextSource<'a> {
    /// The project root, absolute.
    pub root: &'a Path,
    /// Where the person ran pmpx from; equal to `root` when there was no walk-up.
    pub start_dir: &'a Path,
    /// The files this plugin's detection matched.
    pub matched: &'a [String],
    /// The project config files that were read, nearest first.
    pub config_files: &'a [PathBuf],
    /// `[plugin]` pins: family to plugin name.
    pub pins: &'a BTreeMap<String, String>,
    /// The arguments the user typed.
    pub args: &'a [OsString],
    /// The verb, as `PMPX_VERB_*`.
    pub verb: u32,
    /// Why this plugin was selected, as `PMPX_REASON_*`.
    pub reason: u32,
    /// The evidence score it won with.
    pub score: u32,
    /// Reads the files the plugin declared.
    pub files: &'a dyn Files,
}

impl ContextSource<'_> {
    /// Build the context, run `f` while it is valid, and let it go afterwards.
    ///
    /// Nothing the plugin receives may be kept past `f`: the buffers live in the state created here.
    pub(crate) fn with_context<T>(&self, f: impl FnOnce(*const PmpxContext) -> T) -> T {
        let state = State {
            root: self.root,
            start_dir: self.start_dir,
            matched: self.matched,
            config_files: self.config_files,
            pins: self.pins,
            args: self.args,
            files: self.files,
            buffers: RefCell::new(Vec::new()),
            contents: RefCell::new(BTreeMap::new()),
        };

        let context = PmpxContext {
            size: std::mem::size_of::<PmpxContext>(),
            verb: self.verb,
            reason: self.reason,
            score: self.score,
            count: context_count,
            get: context_get,
            name: context_name,
            // The accessors find this again through the context they were handed.
            opaque: (&state as *const State<'_>).cast(),
        };

        f(&context)
    }
}

/// The state an accessor answers from, for the duration of one call.
struct State<'a> {
    root: &'a Path,
    start_dir: &'a Path,
    matched: &'a [String],
    config_files: &'a [PathBuf],
    pins: &'a BTreeMap<String, String>,
    args: &'a [OsString],
    files: &'a dyn Files,
    /// Bytes produced during the call (arguments, paths, names). Keeping the `Vec`s alive is what
    /// makes the pointers handed to the plugin valid; pushing may move the outer `Vec`, never the
    /// buffers inside it.
    buffers: RefCell<Vec<Vec<u8>>>,
    /// File contents, fetched once per name. `None` records "the host has nothing", so a second
    /// question does not ask the provider again.
    contents: RefCell<BTreeMap<String, Option<Vec<u8>>>>,
}

impl State<'_> {
    /// Copy `bytes` into a buffer that lives until the end of the call, and return a view of it.
    fn keep(&self, bytes: &[u8]) -> PmpxStr {
        let mut buffers = self.buffers.borrow_mut();
        buffers.push(bytes.to_vec());
        let kept = buffers.last().expect("just pushed");
        // A present-but-empty value has a non-null pointer, which is how "empty" is told apart from
        // "absent" on the other side.
        PmpxStr::new(kept.as_ptr(), kept.len())
    }

    /// The contents of a declared file, fetched on first use.
    fn file(&self, name: &str) -> Option<PmpxStr> {
        let mut contents = self.contents.borrow_mut();
        let entry = contents
            .entry(name.to_string())
            .or_insert_with(|| self.files.contents(name));

        entry
            .as_ref()
            .map(|bytes| PmpxStr::new(bytes.as_ptr(), bytes.len()))
    }

    /// The `index`-th pin, as `(family, plugin)`.
    fn pin(&self, index: usize) -> Option<(&str, &str)> {
        self.pins
            .iter()
            .nth(index)
            .map(|(family, plugin)| (family.as_str(), plugin.as_str()))
    }
}

/// Recover the state from the context a plugin was handed.
///
/// # Safety
/// `context` must be the pointer built by [`ContextSource::with_context`], still inside that call.
unsafe fn state<'a>(context: *const PmpxContext) -> Option<&'a State<'a>> {
    if context.is_null() {
        return None;
    }
    let opaque = unsafe { (*context).opaque };
    if opaque.is_null() {
        return None;
    }
    // SAFETY: this is the pointer `with_context` put there, and it outlives every accessor call
    // because the accessors are only reachable through that context.
    Some(unsafe { &*opaque.cast::<State<'a>>() })
}

/// The key a plugin asked about, as text. Invalid UTF-8 is "unknown", not an error.
///
/// Copied rather than borrowed: the bytes belong to the plugin, and the `PmpxStr` that describes
/// them does not carry a lifetime. A key is a handful of bytes and is looked at once per accessor
/// call, so the copy is the honest price of not writing an unsafe lifetime extension here.
fn key_of(key: PmpxStr) -> Option<String> {
    let bytes = unsafe { key.as_bytes() }?;
    std::str::from_utf8(bytes).ok().map(str::to_string)
}

/// How many values one key has.
///
/// # Safety
/// See [`state`].
pub(crate) unsafe extern "C" fn context_count(context: *const PmpxContext, key: PmpxStr) -> usize {
    let Some(state) = (unsafe { state(context) }) else {
        return 0;
    };
    let Some(key) = key_of(key) else {
        return 0;
    };

    match key.as_str() {
        PMPX_KEY_PROJECT_ROOT | PMPX_KEY_PROJECT_START_DIR => 1,
        PMPX_KEY_PROJECT_MATCHED => state.matched.len(),
        PMPX_KEY_PROJECT_CONFIG_FILES => state.config_files.len(),
        PMPX_KEY_ARGS => state.args.len(),
        PMPX_KEY_CONFIG_PIN => state.pins.len(),
        _ => match file_name(&key) {
            Some(name) => usize::from(state.file(name).is_some()),
            None => 0,
        },
    }
}

/// The `index`-th value of one key.
///
/// # Safety
/// See [`state`].
pub(crate) unsafe extern "C" fn context_get(
    context: *const PmpxContext,
    key: PmpxStr,
    index: usize,
) -> PmpxStr {
    let Some(state) = (unsafe { state(context) }) else {
        return PmpxStr::EMPTY;
    };
    let Some(key) = key_of(key) else {
        return PmpxStr::EMPTY;
    };

    match key.as_str() {
        PMPX_KEY_PROJECT_ROOT if index == 0 => {
            state.keep(&pmpx_plugin_abi::os_to_bytes(state.root.as_os_str()))
        }
        PMPX_KEY_PROJECT_START_DIR if index == 0 => {
            state.keep(&pmpx_plugin_abi::os_to_bytes(state.start_dir.as_os_str()))
        }
        PMPX_KEY_PROJECT_MATCHED => state
            .matched
            .get(index)
            .map_or(PmpxStr::EMPTY, |name| state.keep(name.as_bytes())),
        PMPX_KEY_PROJECT_CONFIG_FILES => state
            .config_files
            .get(index)
            .map_or(PmpxStr::EMPTY, |path| {
                state.keep(&pmpx_plugin_abi::os_to_bytes(path.as_os_str()))
            }),
        PMPX_KEY_ARGS => state.args.get(index).map_or(PmpxStr::EMPTY, |arg| {
            state.keep(&pmpx_plugin_abi::os_to_bytes(arg.as_os_str()))
        }),
        PMPX_KEY_CONFIG_PIN => state
            .pin(index)
            .map_or(PmpxStr::EMPTY, |(_, plugin)| state.keep(plugin.as_bytes())),
        _ => match file_name(&key) {
            Some(name) => state.file(name).unwrap_or(PmpxStr::EMPTY),
            None => PmpxStr::EMPTY,
        },
    }
}

/// The `index`-th *name* of one key: map keys, not values.
///
/// # Safety
/// See [`state`].
pub(crate) unsafe extern "C" fn context_name(
    context: *const PmpxContext,
    key: PmpxStr,
    index: usize,
) -> PmpxStr {
    let Some(state) = (unsafe { state(context) }) else {
        return PmpxStr::EMPTY;
    };
    let Some(key) = key_of(key) else {
        return PmpxStr::EMPTY;
    };

    match key.as_str() {
        PMPX_KEY_CONFIG_PIN => state
            .pin(index)
            .map_or(PmpxStr::EMPTY, |(family, _)| state.keep(family.as_bytes())),
        // Every other key is a list or a scalar: names are only a map's business.
        _ => PmpxStr::EMPTY,
    }
}

/// The declared-file name one key asks for, or `None` when the key is not a file key.
fn file_name(key: &str) -> Option<&str> {
    key.strip_prefix(PMPX_KEY_FILE_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider that answers from a fixed table.
    struct Table(Vec<(&'static str, &'static str)>);

    impl Files for Table {
        fn contents(&self, name: &str) -> Option<Vec<u8>> {
            self.0
                .iter()
                .find(|(declared, _)| *declared == name)
                .map(|(_, body)| body.as_bytes().to_vec())
        }
    }

    struct Fixture {
        root: PathBuf,
        start_dir: PathBuf,
        matched: Vec<String>,
        config_files: Vec<PathBuf>,
        pins: BTreeMap<String, String>,
        args: Vec<OsString>,
        files: Table,
    }

    fn fixture() -> Fixture {
        Fixture {
            root: PathBuf::from("/work/project"),
            start_dir: PathBuf::from("/work/project/packages/api"),
            matched: vec!["package.json".into(), "pnpm-lock.yaml".into()],
            config_files: vec![PathBuf::from("/work/project/.pmpx.toml")],
            pins: [("node".to_string(), "pnpm".to_string())].into(),
            args: vec![OsString::from("left-pad"), OsString::from("--save-dev")],
            files: Table(vec![("package.json", "{\"name\":\"x\"}")]),
        }
    }

    /// Run `f` with a context built from the fixture, the way `Plugin::call` does.
    fn with<T>(fixture: &Fixture, f: impl FnOnce(*const PmpxContext) -> T) -> T {
        let source = ContextSource {
            root: &fixture.root,
            start_dir: &fixture.start_dir,
            matched: &fixture.matched,
            config_files: &fixture.config_files,
            pins: &fixture.pins,
            args: &fixture.args,
            verb: pmpx_plugin_abi::PMPX_VERB_INSTALL,
            reason: pmpx_plugin_abi::PMPX_REASON_SCORED,
            score: 110,
            files: &fixture.files,
        };
        source.with_context(f)
    }

    /// Ask a context a question, the way a plugin would.
    fn ask(context: *const PmpxContext, key: &str, index: usize) -> Option<String> {
        let key = PmpxStr::new(key.as_ptr(), key.len());
        let value = unsafe { context_get(context, key, index) };
        let bytes = unsafe { value.as_bytes() }?;
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    fn count(context: *const PmpxContext, key: &str) -> usize {
        let key = PmpxStr::new(key.as_ptr(), key.len());
        unsafe { context_count(context, key) }
    }

    fn name(context: *const PmpxContext, key: &str, index: usize) -> Option<String> {
        let key = PmpxStr::new(key.as_ptr(), key.len());
        let value = unsafe { context_name(context, key, index) };
        let bytes = unsafe { value.as_bytes() }?;
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    #[test]
    fn every_key_answers_what_the_host_knows() {
        let fixture = fixture();
        with(&fixture, |context| {
            assert_eq!(count(context, PMPX_KEY_PROJECT_ROOT), 1);
            assert_eq!(
                ask(context, PMPX_KEY_PROJECT_ROOT, 0).as_deref(),
                Some("/work/project")
            );
            assert_eq!(
                ask(context, PMPX_KEY_PROJECT_START_DIR, 0).as_deref(),
                Some("/work/project/packages/api"),
                "the invocation directory is not the root"
            );
            assert_eq!(count(context, PMPX_KEY_PROJECT_MATCHED), 2);
            assert_eq!(
                ask(context, PMPX_KEY_PROJECT_MATCHED, 1).as_deref(),
                Some("pnpm-lock.yaml")
            );
            assert_eq!(count(context, PMPX_KEY_PROJECT_CONFIG_FILES), 1);
            assert_eq!(count(context, PMPX_KEY_ARGS), 2);
            assert_eq!(ask(context, PMPX_KEY_ARGS, 0).as_deref(), Some("left-pad"));
            assert_eq!(count(context, PMPX_KEY_CONFIG_PIN), 1);
            assert_eq!(
                ask(context, PMPX_KEY_CONFIG_PIN, 0).as_deref(),
                Some("pnpm")
            );
            assert_eq!(
                name(context, PMPX_KEY_CONFIG_PIN, 0).as_deref(),
                Some("node")
            );
        });
    }

    #[test]
    fn the_scalars_are_fields() {
        let fixture = fixture();
        with(&fixture, |context| {
            let context = unsafe { &*context };
            assert_eq!(context.verb, pmpx_plugin_abi::PMPX_VERB_INSTALL);
            assert_eq!(context.reason, pmpx_plugin_abi::PMPX_REASON_SCORED);
            assert_eq!(context.score, 110);
            assert_eq!(context.size, std::mem::size_of::<PmpxContext>());
        });
    }

    /// A declared file is answered on demand, and only that one.
    #[test]
    fn a_declared_file_is_answered_and_an_undeclared_one_is_not() {
        let fixture = fixture();
        with(&fixture, |context| {
            assert_eq!(count(context, "file.package.json"), 1);
            assert_eq!(
                ask(context, "file.package.json", 0).as_deref(),
                Some("{\"name\":\"x\"}")
            );
            assert_eq!(
                count(context, "file.Cargo.toml"),
                0,
                "a file the plugin did not declare may not be read"
            );
            assert!(ask(context, "file.Cargo.toml", 0).is_none());
        });
    }

    /// The provider is asked once per name, not once per question.
    #[test]
    fn a_file_is_fetched_once_per_call() {
        struct Counting(RefCell<usize>);

        impl Files for Counting {
            fn contents(&self, _name: &str) -> Option<Vec<u8>> {
                *self.0.borrow_mut() += 1;
                Some(b"body".to_vec())
            }
        }

        let counting = Counting(RefCell::new(0));
        let fixture = fixture();
        let source = ContextSource {
            root: &fixture.root,
            start_dir: &fixture.start_dir,
            matched: &fixture.matched,
            config_files: &fixture.config_files,
            pins: &fixture.pins,
            args: &fixture.args,
            verb: pmpx_plugin_abi::PMPX_VERB_RUN,
            reason: pmpx_plugin_abi::PMPX_REASON_SCORED,
            score: 0,
            files: &counting,
        };

        source.with_context(|context| {
            for _ in 0..3 {
                assert_eq!(ask(context, "file.any", 0).as_deref(), Some("body"));
            }
        });
        assert_eq!(*counting.0.borrow(), 1);
    }

    /// Unknown keys, absent indexes and garbage are all "absent" -- never a panic, never a wrong
    /// answer. This is the counterpart of "an unknown key is not an error" on the plugin side.
    #[test]
    fn anything_the_host_does_not_have_is_absent() {
        let fixture = fixture();
        with(&fixture, |context| {
            assert_eq!(count(context, "no.such.key"), 0);
            assert!(ask(context, "no.such.key", 0).is_none());
            assert!(ask(context, PMPX_KEY_ARGS, 99).is_none(), "past the end");
            assert!(
                ask(context, PMPX_KEY_PROJECT_ROOT, 1).is_none(),
                "a scalar has one value"
            );
            assert!(
                name(context, PMPX_KEY_ARGS, 0).is_none(),
                "a list has no names"
            );

            // A key that is not UTF-8 at all.
            let bad = PmpxStr::new([0xffu8].as_ptr(), 1);
            assert_eq!(unsafe { context_count(context, bad) }, 0);
            assert!(unsafe { context_get(context, bad, 0) }.is_absent());

            // A context that says nothing about itself.
            assert_eq!(unsafe { context_count(std::ptr::null(), bad) }, 0);
        });
    }

    /// A value stays readable while the call lasts, even after other values were produced: the
    /// buffers live in the state, and pushing more of them never moves an earlier one.
    #[test]
    fn earlier_values_stay_valid_while_the_call_lasts() {
        let fixture = fixture();
        with(&fixture, |context| {
            let first = unsafe { context_get(context, PmpxStr::new("args".as_ptr(), 4), 0) };
            let first_bytes = unsafe { first.as_bytes() }.expect("present");

            // Force the buffers to grow a lot, which is what would move them if they all lived in
            // one flat allocation.
            for _ in 0..200 {
                let _ = ask(context, PMPX_KEY_ARGS, 0);
            }

            assert_eq!(first_bytes, b"left-pad", "an earlier value must not move");
        });
    }

    /// The state is reachable through the context alone: that is what makes the accessors work
    /// without a process-wide global.
    #[test]
    fn the_state_is_found_through_the_context() {
        let fixture = fixture();
        with(&fixture, |context| {
            assert!(
                !unsafe { (*context).opaque }.is_null(),
                "the context has to carry its state, or the accessors cannot find the data"
            );
            assert!(unsafe { state(context) }.is_some());
        });
    }
}
