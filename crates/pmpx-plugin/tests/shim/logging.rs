//! What a plugin's `debug!` reaches, and what the host's level keeps it from doing.
//!
//! The host's role here is played by a callback that records what it was handed, which is the only
//! way to see that a message crossed the boundary at all.

use std::sync::Mutex;

use pmpx_plugin::abi::{self, PmpxHostV1, PmpxStr, PMPX_LEVEL_DEBUG, PMPX_LEVEL_WARN};
use pmpx_plugin::Verb;

use crate::support::{
    call_command, entry, read, TEST_CONFIG, TEST_FILE_CONTENT, TEST_SCORE, TEST_START_DIR,
};

/// What the "host" was told. `level:message`, so the level is asserted too.
static LOGGED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Host state: one test must not be swapping the hooks while another is calling through them.
static SERIAL: Mutex<()> = Mutex::new(());

unsafe extern "C" fn capture(level: u32, message: PmpxStr) {
    // SAFETY: the plugin keeps the message alive for the duration of the call.
    let text = unsafe { read(message) };
    LOGGED.lock().unwrap().push(format!("{level}:{text}"));
}

/// A host that wants everything.
static TALKATIVE: PmpxHostV1 = PmpxHostV1::new(capture, PMPX_LEVEL_DEBUG);

/// A host that only wants to hear about trouble.
static GRUMPY: PmpxHostV1 = PmpxHostV1::new(capture, PMPX_LEVEL_WARN);

fn install(hooks: &'static PmpxHostV1) -> std::sync::MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    LOGGED.lock().unwrap().clear();
    // SAFETY: the hooks are `'static` and outlive every call below.
    unsafe { (entry().set_host)(hooks as *const _) };
    guard
}

/// A message a plugin writes arrives with its level, in one piece.
#[test]
fn a_plugin_message_reaches_the_host() {
    let _serial = install(&TALKATIVE);

    call_command("/proj", &[], Some(Verb::Run), &["log"]).expect("run should map");

    let seen = LOGGED.lock().unwrap().clone();
    assert_eq!(seen, vec![format!("{PMPX_LEVEL_DEBUG}:toy logged for run")]);
}

/// The level is what makes `--debug` free when it is off: a host that does not want the message
/// never sees it, and the plugin never formatted it.
#[test]
fn a_host_that_only_wants_warnings_hears_nothing() {
    let _serial = install(&GRUMPY);

    call_command("/proj", &[], Some(Verb::Run), &["log"]).expect("run should map");

    assert!(
        LOGGED.lock().unwrap().is_empty(),
        "a debug message must not reach a host that asked for warnings only"
    );
}

/// `debug::context()` is the plugin saying "here is my situation", so the line has to carry what
/// the host actually handed over -- not a summary the plugin assembled from nothing.
#[test]
fn the_context_can_be_logged_in_one_line() {
    let _serial = install(&TALKATIVE);

    call_command("/proj", &["package.json"], Some(Verb::Run), &["context"])
        .expect("run should map");

    let seen = LOGGED.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "context() prints exactly once: {seen:?}");
    let line = &seen[0];

    assert!(line.contains("root=/proj"), "{line}");
    assert!(line.contains("package.json"), "{line}");
    assert!(line.contains("verb=run"), "{line}");

    // And everything the host filled in beyond the root and the matched files: the invocation
    // directory, the reason it picked this plugin, the score, the pins, the scripts, and the
    // configs that were read.
    assert!(line.contains(&format!("start={TEST_START_DIR}")), "{line}");
    assert!(line.contains("reason=pinned"), "{line}");
    assert!(line.contains(&format!("score={TEST_SCORE}")), "{line}");
    assert!(line.contains("node=fakepm"), "{line}");
    assert!(line.contains("scripts=[build]"), "{line}");
    assert!(line.contains(TEST_CONFIG), "{line}");
    assert!(line.contains("files=[package.json]"), "{line}");
}

/// The bytes of a declared file cross the boundary, not just its name -- which is the point of the
/// channel: a plugin learns what a lockfile pins without reading anything itself.
#[test]
fn the_contents_of_a_declared_file_reach_the_plugin() {
    let _serial = install(&TALKATIVE);

    call_command("/proj", &[], Some(Verb::Run), &["file"]).expect("run should map");

    let seen = LOGGED.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![format!(
            "{PMPX_LEVEL_DEBUG}:file: Some({TEST_FILE_CONTENT:?})"
        )],
        "the declared file's contents should reach the plugin verbatim"
    );
}

/// Detaching the hooks puts the plugin back on its own stderr, which is what a plugin's own test
/// run does -- and what a plugin that was loaded by another host would see.
#[test]
fn detaching_the_host_puts_the_plugin_back_on_its_own() {
    let _serial = install(&TALKATIVE);

    // SAFETY: null is the documented way to detach.
    unsafe { (entry().set_host)(std::ptr::null()) };

    // Nothing panics, and nothing reaches the old hooks: with no host the message goes to the
    // plugin's stderr instead, which is the case a plugin author sees under `cargo test`.
    call_command("/proj", &[], Some(Verb::Run), &["log"]).expect("run should map");
    assert!(LOGGED.lock().unwrap().is_empty());

    // Put the hooks back for whatever runs next.
    unsafe { (entry().set_host)(&TALKATIVE as *const _) };
}

/// The host declares how much of its struct the plugin may read, and the plugin trusts that number
/// rather than its own idea of the size.
#[test]
fn the_host_says_how_much_of_itself_to_read() {
    assert_eq!(
        TALKATIVE.size,
        std::mem::size_of::<PmpxHostV1>(),
        "the size the plugin is told must be the one it was built against"
    );
    assert_eq!(TALKATIVE.max_level, abi::PMPX_LEVEL_DEBUG);
    assert_eq!(GRUMPY.max_level, abi::PMPX_LEVEL_WARN);
}
