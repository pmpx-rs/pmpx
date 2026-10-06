//! The shell `export!` generates, driven by the host side of the ABI.
//!
//! There is no hand-rolled host role here: `pmpx_loader::Tables` is the same code the real host uses,
//! and this test only exists to check what *this* crate is responsible for -- the generated shims, the
//! panic containment, the ownership of what a plugin hands back, and the v3 answers for verbs and
//! logging.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use pmpx_loader::{CallError, ContextSource, NoFiles, Tables};
use pmpx_plugin::{CommandSpec, Context, Family, PackageManager, PluginError, Verb};

// The switch the `name` test flips. Thread-local on purpose: one test must not be able to make
// another test's plugin panic while the suite runs in parallel.
thread_local! {
    static PANIC_IN_NAME: Cell<bool> = const { Cell::new(false) };
}

/// A fake plugin to be driven.
struct Toy;

impl PackageManager for Toy {
    fn name(&self) -> &str {
        // The shell has to contain this: a panic crossing `extern "C"` aborts the whole process.
        if PANIC_IN_NAME.with(Cell::get) {
            panic!("the toy plugin was told to panic in name()");
        }
        "toy"
    }

    fn family(&self) -> Family {
        Family::NODE
    }

    fn command(
        &self,
        ctx: &Context,
        verb: Verb,
        args: &[OsString],
    ) -> Result<CommandSpec, PluginError> {
        match verb {
            // What the host said has to arrive, and what the plugin answers has to come back.
            Verb::Install => {
                let mut spec = CommandSpec::new("toy-bin")
                    .arg("add")
                    .args(args.iter())
                    .arg(format!("matched={}", ctx.matched.join("|")))
                    .arg(format!("root={}", ctx.project_root.display()))
                    .arg(format!("start={}", ctx.start_dir.display()))
                    .arg(format!("reason={}", ctx.reason))
                    .arg(format!("score={}", ctx.score));
                if ctx.has_matched(".yarnrc.yml") {
                    spec = spec.arg("--berry");
                }
                Ok(spec)
            }

            // A declared file is read on demand, and an undeclared one is not there.
            Verb::Run => {
                let declared = ctx
                    .file_str("package.json")
                    .unwrap_or_else(|| "none".into());
                Ok(CommandSpec::new("toy-bin").arg(format!("declared={declared}")))
            }

            // Explicitly unsupported, so the host can degrade `exec`.
            Verb::Exec => Err(PluginError::unsupported_verb(verb)),

            // Deliberately panic, to verify the guard.
            Verb::Test => panic!("this panic must be caught by guard"),

            other => Err(PluginError::other(format!(
                "{other} is not implemented yet"
            ))),
        }
    }
}

fn create() -> Box<dyn PackageManager> {
    Box::new(Toy)
}

pmpx_plugin::export!(create);

/// The host's view of the plugin that was just exported into this test binary.
fn tables() -> Tables {
    // SAFETY: the entry symbol belongs to this binary and its tables are `'static`.
    unsafe { Tables::from_root(pmpx_plugin_entry_v3(), Path::new("<in process>")) }
        .expect("the exported plugin should negotiate")
}

/// A call with the given arguments, against a minimal context.
fn call(verb: Verb, args: &[&str]) -> Result<pmpx_loader::Command, CallError> {
    let root = PathBuf::from("/work/project");
    let matched = vec!["pnpm-lock.yaml".to_string()];
    let pins = BTreeMap::new();
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();

    let source = ContextSource {
        root: &root,
        start_dir: &root,
        matched: &matched,
        config_files: &[],
        pins: &pins,
        args: &args,
        verb: verb.to_abi(),
        reason: pmpx_plugin_abi::PMPX_REASON_SCORED,
        score: 110,
        files: &NoFiles,
    };

    tables().call(&source)
}

/// What the host sees, as plain strings.
fn args_of(command: &pmpx_loader::Command) -> Vec<String> {
    command
        .args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn the_identity_crosses() {
    let tables = tables();

    assert_eq!(tables.name(), "toy");
    assert_eq!(tables.family(), "node");

    let (rustc, target) = tables.build_info();
    assert!(!rustc.is_empty(), "the build info is filled in");
    assert!(!target.is_empty());
}

/// The entry symbol is the one the ABI declares: it tracks the root table's layout, so a host that
/// finds no symbol says "another contract" instead of reading fields that moved.
#[test]
fn the_entry_symbol_is_the_declared_one() {
    assert_eq!(pmpx_plugin_abi::PMPX_ENTRY_SYMBOL, "pmpx_plugin_entry_v3");
}

#[test]
fn what_the_host_says_arrives_and_what_the_plugin_answers_comes_back() {
    let command = call(Verb::Install, &["left-pad", "--save-dev"]).expect("install maps");

    assert_eq!(command.program, OsStr::new("toy-bin"));
    let args = args_of(&command);
    assert_eq!(args[0], "add");
    assert_eq!(args[1], "left-pad", "the arguments arrive");
    assert_eq!(args[2], "--save-dev");
    assert!(
        args.contains(&"matched=pnpm-lock.yaml".to_string()),
        "the matched files arrive: {args:?}"
    );
    assert!(
        args.contains(&"root=/work/project".to_string()),
        "the root arrives: {args:?}"
    );
    assert!(
        args.contains(&"start=/work/project".to_string()),
        "so does the invocation directory: {args:?}"
    );
    assert!(args.contains(&"reason=scored".to_string()), "{args:?}");
    assert!(args.contains(&"score=110".to_string()), "{args:?}");
}

/// A shape decision through `has_matched` -- the whole reason a plugin is given evidence at all.
#[test]
fn the_plugin_can_branch_on_the_matched_files() {
    let root = PathBuf::from("/work");
    let matched = vec![".yarnrc.yml".to_string()];
    let pins = BTreeMap::new();
    let source = ContextSource {
        root: &root,
        start_dir: &root,
        matched: &matched,
        config_files: &[],
        pins: &pins,
        args: &[],
        verb: Verb::Install.to_abi(),
        reason: pmpx_plugin_abi::PMPX_REASON_SCORED,
        score: 10,
        files: &NoFiles,
    };

    let command = tables().call(&source).expect("install maps");

    assert!(
        args_of(&command).contains(&"--berry".to_string()),
        "the berry branch has to be taken: {:?}",
        args_of(&command)
    );
}

/// A declared file is read on demand through the host's provider; an undeclared one is absent.
#[test]
fn a_declared_file_reaches_the_plugin() {
    struct One;
    impl pmpx_loader::Files for One {
        fn contents(&self, name: &str) -> Option<Vec<u8>> {
            (name == "package.json").then(|| b"{\"name\":\"toy\"}".to_vec())
        }
    }

    let root = PathBuf::from("/work");
    let pins = BTreeMap::new();
    let source = ContextSource {
        root: &root,
        start_dir: &root,
        matched: &[],
        config_files: &[],
        pins: &pins,
        args: &[],
        verb: Verb::Run.to_abi(),
        reason: pmpx_plugin_abi::PMPX_REASON_SCORED,
        score: 0,
        files: &One,
    };

    let command = tables().call(&source).expect("run maps");
    assert_eq!(args_of(&command), vec!["declared={\"name\":\"toy\"}"]);
}

/// `unsupported` is a *business* answer: the host gets a code it can act on, not a failure.
#[test]
fn an_unsupported_verb_is_reported_as_such() {
    assert_eq!(call(Verb::Exec, &[]), Err(CallError::UnsupportedVerb));
}

/// A verb *number* this build does not know is answered the same way, which is what keeps a new verb
/// additive: the host's `exec` degradation stays available.
#[test]
fn an_unknown_verb_number_is_unsupported_rather_than_invalid() {
    let root = PathBuf::from("/work");
    let pins = BTreeMap::new();
    let source = ContextSource {
        root: &root,
        start_dir: &root,
        matched: &[],
        config_files: &[],
        pins: &pins,
        args: &[],
        verb: 999,
        reason: pmpx_plugin_abi::PMPX_REASON_SCORED,
        score: 0,
        files: &NoFiles,
    };

    assert_eq!(tables().call(&source), Err(CallError::UnsupportedVerb));
}

/// A panic inside `command` is contained by the shell and becomes an error code.
///
/// The default panic hook prints the message, so the line "this panic must be caught by guard" in the
/// test output is expected, not a failure.
#[test]
fn a_panic_in_command_does_not_take_the_host_down() {
    assert_eq!(call(Verb::Test, &[]), Err(CallError::Internal));
}

/// A panic inside `name` becomes the contract's marker, which is what lets the host report it
/// instead of showing an empty pair of quotes.
#[test]
fn a_panic_in_name_is_contained_and_marked() {
    PANIC_IN_NAME.with(|flag| flag.set(true));
    let tables = tables();
    PANIC_IN_NAME.with(|flag| flag.set(false));

    // Negotiation still succeeds -- `family` is read too, and only `name` panics -- and what comes
    // back is the contract's marker, which the host reports as "this plugin panicked".
    assert_eq!(tables.name(), pmpx_plugin::shell::PANIC_MARKER);
}

/// Calling twice is fine: the host frees what it was handed the first time, and the plugin's next
/// answer is its own memory again.
#[test]
fn a_second_call_does_not_reuse_the_first_answers_memory() {
    let first = call(Verb::Install, &["a"]).expect("install maps");
    let second = call(Verb::Install, &["b"]).expect("install maps again");

    assert_eq!(first.args[1], OsString::from("a"));
    assert_eq!(second.args[1], OsString::from("b"));
    assert_eq!(
        first.program, second.program,
        "the same plugin, the same program"
    );
}

/// The host's logging table is accepted, and a table smaller than this build knows how to read is
/// treated as "no hooks" rather than read past its end.
#[test]
fn the_log_table_is_only_used_when_it_is_large_enough() {
    static WRITTEN: AtomicU32 = AtomicU32::new(0);

    unsafe extern "C" fn write(_level: u32, _message: pmpx_plugin_abi::PmpxStr) {
        WRITTEN.fetch_add(1, Ordering::SeqCst);
    }

    static FULL: pmpx_plugin_abi::PmpxLog = pmpx_plugin_abi::PmpxLog {
        size: std::mem::size_of::<pmpx_plugin_abi::PmpxLog>(),
        write,
        max_level: pmpx_plugin_abi::PMPX_LEVEL_DEBUG,
    };

    #[repr(C)]
    struct Short {
        size: usize,
        write: unsafe extern "C" fn(u32, pmpx_plugin_abi::PmpxStr),
    }

    // A table that stops before `max_level` is a table this build cannot read. What happens when one
    // is the *first* thing offered needs its own process (the table is process-global), so that case
    // lives in `tests/short_log_table.rs`; here it only has to be ignored rather than read.
    static SHORT: Short = Short {
        size: std::mem::size_of::<pmpx_plugin_abi::PmpxLog>() - 8,
        write,
    };

    unsafe extern "C" fn host_lookup(name: pmpx_plugin_abi::PmpxStr) -> *const std::ffi::c_void {
        let bytes = unsafe { name.as_bytes() }.unwrap_or(&[]);
        match bytes {
            b"log" => std::ptr::from_ref(&FULL).cast(),
            _ => std::ptr::null(),
        }
    }

    static HOST: pmpx_plugin_abi::PmpxHost = pmpx_plugin_abi::PmpxHost {
        abi_major: pmpx_plugin_abi::PMPX_ABI_MAJOR,
        size: std::mem::size_of::<pmpx_plugin_abi::PmpxHost>(),
        capability: host_lookup,
    };

    tables().attach(&HOST);
    pmpx_plugin::debug!("a line the host should see");
    assert_eq!(WRITTEN.load(Ordering::SeqCst), 1, "the host got the line");

    // The short table is only *offered* here: the hooks already installed stay, and the point is that
    // nothing reads `max_level` out of the short one.
    static SHORT_HOST: pmpx_plugin_abi::PmpxHost = pmpx_plugin_abi::PmpxHost {
        abi_major: pmpx_plugin_abi::PMPX_ABI_MAJOR,
        size: std::mem::size_of::<pmpx_plugin_abi::PmpxHost>(),
        capability: short_lookup,
    };

    unsafe extern "C" fn short_lookup(name: pmpx_plugin_abi::PmpxStr) -> *const std::ffi::c_void {
        let bytes = unsafe { name.as_bytes() }.unwrap_or(&[]);
        match bytes {
            b"log" => std::ptr::from_ref(&SHORT).cast(),
            _ => std::ptr::null(),
        }
    }

    tables().attach(&SHORT_HOST);
    pmpx_plugin::debug!("the hooks that were already installed are still the ones in use");
    assert_eq!(
        WRITTEN.load(Ordering::SeqCst),
        2,
        "an unreadable table is ignored rather than adopted"
    );
}
