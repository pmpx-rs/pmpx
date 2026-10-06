//! What happens when the host offers a logging table smaller than this build knows how to read.
//!
//! Its own test binary on purpose: the logging table is process-global, so "a short table is treated
//! as no hooks at all" can only be observed in a process where nothing else has installed hooks.

use std::sync::atomic::{AtomicU32, Ordering};

use pmpx_loader::{ContextSource, NoFiles, Tables};
use pmpx_plugin::{CommandSpec, Context, Family, PackageManager, PluginError, Verb};

static WRITTEN: AtomicU32 = AtomicU32::new(0);

struct Quiet;

impl PackageManager for Quiet {
    fn name(&self) -> &str {
        "quiet"
    }

    fn family(&self) -> Family {
        Family::NODE
    }

    fn command(
        &self,
        _ctx: &Context,
        _verb: Verb,
        _args: &[std::ffi::OsString],
    ) -> Result<CommandSpec, PluginError> {
        Ok(CommandSpec::new("quiet-bin"))
    }
}

fn create() -> Box<dyn PackageManager> {
    Box::new(Quiet)
}

pmpx_plugin::export!(create);

unsafe extern "C" fn write(_level: u32, _message: pmpx_plugin_abi::PmpxStr) {
    WRITTEN.fetch_add(1, Ordering::SeqCst);
}

/// A host table that stops before `max_level`: everything past the write pointer is not there.
#[repr(C)]
struct Short {
    size: usize,
    write: unsafe extern "C" fn(u32, pmpx_plugin_abi::PmpxStr),
}

static SHORT: Short = Short {
    size: std::mem::size_of::<pmpx_plugin_abi::PmpxLog>() - 8,
    write,
};

unsafe extern "C" fn lookup(name: pmpx_plugin_abi::PmpxStr) -> *const std::ffi::c_void {
    let bytes = unsafe { name.as_bytes() }.unwrap_or(&[]);
    match bytes {
        b"log" => std::ptr::from_ref(&SHORT).cast(),
        _ => std::ptr::null(),
    }
}

static HOST: pmpx_plugin_abi::PmpxHost = pmpx_plugin_abi::PmpxHost {
    abi_major: pmpx_plugin_abi::PMPX_ABI_MAJOR,
    size: std::mem::size_of::<pmpx_plugin_abi::PmpxHost>(),
    capability: lookup,
};

#[test]
fn a_short_log_table_is_treated_as_no_hooks_at_all() {
    // SAFETY: the entry symbol belongs to this binary and its tables are `'static`.
    let tables =
        unsafe { Tables::from_root(pmpx_plugin_entry_v3(), std::path::Path::new("<in process>")) }
            .expect("the exported plugin should negotiate");

    tables.attach(&HOST);

    pmpx_plugin::debug!("this must never reach a table that stops before max_level");
    pmpx_plugin::warn!("neither must this");

    assert_eq!(
        WRITTEN.load(Ordering::SeqCst),
        0,
        "a table too small to read `max_level` from is not usable, so the plugin falls back to its \
         own stderr instead of guessing"
    );

    // And the plugin is still callable: the hooks were the only thing refused.
    let tables =
        unsafe { Tables::from_root(pmpx_plugin_entry_v3(), std::path::Path::new("<in process>")) }
            .expect("the exported plugin should negotiate");

    let root = std::path::PathBuf::from("/work");
    let pins = std::collections::BTreeMap::new();
    let source = ContextSource {
        root: &root,
        start_dir: &root,
        matched: &[],
        config_files: &[],
        pins: &pins,
        args: &[],
        verb: Verb::Install.to_abi(),
        reason: pmpx_plugin_abi::PMPX_REASON_SCORED,
        score: 0,
        files: &NoFiles,
    };

    assert_eq!(
        tables
            .call(&source)
            .expect("no hooks does not mean no calls")
            .program,
        std::ffi::OsString::from("quiet-bin")
    );
}
