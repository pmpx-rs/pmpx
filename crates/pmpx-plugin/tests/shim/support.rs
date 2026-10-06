//! The host role: reading C strings, reaching the vtable, and calling `command` once per the
//! contract.

use std::ffi::OsString;

use pmpx_plugin::abi::{
    self, PmpxCommand, PmpxContextV1, PmpxKeyValue, PmpxPin, PmpxPluginV1, PmpxStr,
};
use pmpx_plugin::{CommandSpec, Verb};

use crate::pmpx_plugin_entry_v1;

/// Read a Rust string back from a C string.
///
/// # Safety
/// `s` must be a valid byte range in this process.
pub(crate) unsafe fn read(s: PmpxStr) -> String {
    if s.is_empty() {
        return String::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(s.ptr, s.len) };
    String::from_utf8(bytes.to_vec()).expect("the tests only pass UTF-8")
}

pub(crate) fn entry() -> &'static PmpxPluginV1 {
    // SAFETY: the entry returns a 'static table.
    unsafe { &*pmpx_plugin_entry_v1() }
}

/// The values [`context`] fills in for the parts a plugin only ever reads.
///
/// Exposed so the assertions and the setup cannot drift apart.
pub(crate) const TEST_START_DIR: &str = "/work/packages/api";
pub(crate) const TEST_PINS: &[(&str, &str)] = &[("node", "fakepm")];
pub(crate) const TEST_SCRIPTS: &[(&str, &str)] = &[("build", "fakepm run build")];
pub(crate) const TEST_CONFIG: &str = "/work/.pmpx.toml";
pub(crate) const TEST_REASON: u32 = abi::PMPX_REASON_PINNED;
pub(crate) const TEST_SCORE: u32 = 110;

/// A context plus the storage its views point at.
///
/// The arrays must outlive the call, so they are owned here rather than being temporaries in
/// [`context`]; everything else points at `'static` literals or at the caller's own strings.
pub(crate) struct TestContext {
    raw: PmpxContextV1,
    _matched: Vec<PmpxStr>,
    _args: Vec<PmpxStr>,
    _pins: Vec<PmpxPin>,
    _scripts: Vec<PmpxKeyValue>,
    _config: Vec<PmpxStr>,
}

impl TestContext {
    /// The pointer to hand to `command`: valid while this value is alive.
    pub(crate) fn ptr(&self) -> *const PmpxContextV1 {
        &self.raw
    }

    /// For the tests that hand the plugin a broken context on purpose.
    pub(crate) fn raw_mut(&mut self) -> &mut PmpxContextV1 {
        &mut self.raw
    }
}

/// Build the context a host would hand over: root, matched files and args borrowed from the caller,
/// and the parts a plugin only reads filled with the `TEST_*` values.
pub(crate) fn context(root: &str, matched: &[&str], verb: u32, args: &[&str]) -> TestContext {
    fn view(s: &str) -> PmpxStr {
        PmpxStr {
            ptr: s.as_ptr(),
            len: s.len(),
        }
    }

    let matched_raw: Vec<PmpxStr> = matched.iter().map(|s| view(s)).collect();
    let args_raw: Vec<PmpxStr> = args.iter().map(|s| view(s)).collect();
    let pins: Vec<PmpxPin> = TEST_PINS
        .iter()
        .map(|(family, plugin)| PmpxPin {
            family: view(family),
            plugin: view(plugin),
        })
        .collect();
    let scripts: Vec<PmpxKeyValue> = TEST_SCRIPTS
        .iter()
        .map(|(key, value)| PmpxKeyValue {
            key: view(key),
            value: view(value),
        })
        .collect();
    let config: Vec<PmpxStr> = std::iter::once(view(TEST_CONFIG)).collect();

    let mut raw = PmpxContextV1::empty();
    raw.root = view(root);
    raw.start_dir = view(TEST_START_DIR);
    raw.matched = matched_raw.as_ptr();
    raw.matched_len = matched_raw.len();
    raw.verb = verb;
    raw.reason = TEST_REASON;
    raw.score = TEST_SCORE;
    raw.args = args_raw.as_ptr();
    raw.args_len = args_raw.len();
    raw.pins = pins.as_ptr();
    raw.pins_len = pins.len();
    raw.scripts = scripts.as_ptr();
    raw.scripts_len = scripts.len();
    raw.config_paths = config.as_ptr();
    raw.config_paths_len = config.len();

    TestContext {
        raw,
        _matched: matched_raw,
        _args: args_raw,
        _pins: pins,
        _scripts: scripts,
        _config: config,
    }
}

/// Call `command` once, copy the result into Rust values, then free the plugin's memory per the
/// contract.
pub(crate) fn call_command(
    root: &str,
    matched: &[&str],
    verb: Option<Verb>,
    args: &[&str],
) -> Result<CommandSpec, u32> {
    let e = entry();

    // The verb is either a real number or a deliberately out-of-range one.
    let verb_code = verb.map(Verb::to_abi).unwrap_or(99);
    let context = context(root, matched, verb_code, args);

    let mut out = std::mem::MaybeUninit::<PmpxCommand>::uninit();

    // SAFETY: the context owns every array it points at and stays alive for the call; out points at
    // local writable memory.
    let code = unsafe { (e.command)(context.ptr(), out.as_mut_ptr()) };

    if code != abi::PMPX_OK {
        return Err(code);
    }

    // SAFETY: when code == PMPX_OK the plugin guarantees out has been filled in.
    let mut cmd = unsafe { out.assume_init() };

    // Copy this memory out before handing it back to the plugin -- the host only ever reads.
    let spec = unsafe {
        let program = read(cmd.program);
        let cwd = if cmd.cwd.is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(read(cmd.cwd)))
        };
        let args: Vec<OsString> = (0..cmd.args_len)
            .map(|i| OsString::from(read(*cmd.args.add(i))))
            .collect();

        CommandSpec {
            program: program.into(),
            args,
            cwd,
        }
    };

    // SAFETY: this struct comes from the successful call above and is freed only once.
    unsafe { (e.free_command)(&mut cmd as *mut _) };

    Ok(spec)
}
