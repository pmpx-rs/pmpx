//! A plugin written straight against the raw ABI -- the shape a C plugin would have.
//!
//! It answers `identity`, `command` and `attach`, and nothing else. The command it produces encodes
//! what it saw, so the host's test can tell whether the context, the verb and the arguments really
//! crossed the boundary:
//!
//! ```text
//! program = "raw-<verb>"
//! args    = ["<first argument the host sent, or (none)>"]
//! ```
//!
//! Every string it hands back is a `Box<[u8]>` it leaked, and `free_command` reclaims exactly those
//! -- the ownership rule of the ABI, with no library to help.

use std::ffi::c_void;

use pmpx_plugin_abi::{
    PmpxAttach, PmpxCommand, PmpxCommandCap, PmpxContext, PmpxHost, PmpxIdentity, PmpxPlugin,
    PmpxSlice, PmpxStr, PMPX_ABI_MAJOR, PMPX_OK,
};

static NAME: &[u8] = b"raw";
static FAMILY: &[u8] = b"rawfam";

unsafe extern "C" fn name() -> PmpxStr {
    PmpxStr::new(NAME.as_ptr(), NAME.len())
}

unsafe extern "C" fn family() -> PmpxStr {
    PmpxStr::new(FAMILY.as_ptr(), FAMILY.len())
}

unsafe extern "C" fn free_str(_s: PmpxStr) {
    // The two strings above are `'static`; there is nothing to release.
}

static IDENTITY: PmpxIdentity = PmpxIdentity {
    size: std::mem::size_of::<PmpxIdentity>(),
    name,
    family,
    free_str,
};

/// How many times the host attached: the test reads this through a second exported symbol.
static ATTACHED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

unsafe extern "C" fn attach(_host: *const PmpxHost) {
    ATTACHED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

static ATTACH: PmpxAttach = PmpxAttach {
    size: std::mem::size_of::<PmpxAttach>(),
    attach,
};

/// Read the first argument the host sent, through the context's accessors -- the same question a real
/// plugin asks.
unsafe fn first_argument(context: *const PmpxContext) -> Vec<u8> {
    let key = PmpxStr::new(b"args".as_ptr(), 4);

    // SAFETY: the host promises a context that stays valid for this call.
    let count = unsafe { (*context).count };
    let get = unsafe { (*context).get };
    if unsafe { count(context, key) } == 0 {
        return b"(none)".to_vec();
    }

    let value = unsafe { get(context, key, 0) };
    unsafe { value.as_bytes() }.unwrap_or(b"(absent)").to_vec()
}

unsafe extern "C" fn run(context: *const PmpxContext, out: *mut PmpxCommand) -> u32 {
    // SAFETY: the host promises a valid context and a writable command.
    let verb = unsafe { (*context).verb };
    let first = unsafe { first_argument(context) };

    let program = format!("raw-{verb}").into_bytes().into_boxed_slice();
    let argument = first.into_boxed_slice();

    // Both boxes have to be leaked: the host frees what the plugin hands over, through
    // `free_command`, and it reclaims each string from the `PmpxStr` inside the array. Dropping
    // `argument` here would leave the array pointing at freed memory -- and the host would then free
    // it a second time.
    let program_len = program.len();
    let program_ptr = Box::into_raw(program) as *const u8;
    let argument_len = argument.len();
    let argument_ptr = Box::into_raw(argument) as *const u8;

    let args = vec![PmpxStr::new(argument_ptr, argument_len)].into_boxed_slice();
    let args_len = args.len();
    let args_ptr = Box::into_raw(args) as *const PmpxStr;

    // SAFETY: writable memory the host owns for the duration of the call.
    unsafe {
        (*out).size = std::mem::size_of::<PmpxCommand>();
        (*out).program = PmpxStr::new(program_ptr, program_len);
        (*out).args = PmpxSlice::new(args_ptr, args_len);
        (*out).cwd = PmpxStr::EMPTY;
    }

    PMPX_OK
}

unsafe extern "C" fn free_command(command: *mut PmpxCommand) {
    // SAFETY: only a command this library produced reaches here, and it is released once.
    let command = unsafe { &*command };

    if !command.program.is_absent() {
        let raw = std::ptr::slice_from_raw_parts_mut(
            command.program.ptr as *mut u8,
            command.program.len,
        );
        drop(unsafe { Box::from_raw(raw) });
    }

    if let Some(args) = unsafe { command.args.as_slice() } {
        for arg in args {
            if !arg.is_absent() {
                let raw = std::ptr::slice_from_raw_parts_mut(arg.ptr as *mut u8, arg.len);
                drop(unsafe { Box::from_raw(raw) });
            }
        }
        let raw =
            std::ptr::slice_from_raw_parts_mut(command.args.ptr as *mut PmpxStr, command.args.len);
        drop(unsafe { Box::from_raw(raw) });
    }
}

static COMMAND: PmpxCommandCap = PmpxCommandCap {
    size: std::mem::size_of::<PmpxCommandCap>(),
    run,
    free_command,
};

unsafe extern "C" fn capability(name: PmpxStr) -> *const c_void {
    // SAFETY: the host passes a borrow that outlives this call.
    let bytes = unsafe { name.as_bytes() }.unwrap_or(&[]);

    match bytes {
        b"identity" => std::ptr::from_ref(&IDENTITY).cast(),
        b"command" => std::ptr::from_ref(&COMMAND).cast(),
        b"attach" => std::ptr::from_ref(&ATTACH).cast(),
        _ => std::ptr::null(),
    }
}

static RUSTC: &[u8] = b"the fixture's rustc";
static TARGET: &[u8] = b"the fixture's target";

static PLUGIN: PmpxPlugin = PmpxPlugin {
    abi_major: PMPX_ABI_MAJOR,
    rustc_version: PmpxStr::new(RUSTC.as_ptr(), RUSTC.len()),
    target: PmpxStr::new(TARGET.as_ptr(), TARGET.len()),
    capability,
};

/// The one symbol a host looks for.
#[unsafe(no_mangle)]
pub extern "C" fn pmpx_plugin_entry_v3() -> *const PmpxPlugin {
    &PLUGIN
}

/// How many times the host attached. Not part of the ABI: this exists so the host's test can see
/// that `attach` really happened.
#[unsafe(no_mangle)]
pub extern "C" fn pmpx_raw_plugin_attached() -> u32 {
    ATTACHED.load(std::sync::atomic::Ordering::SeqCst)
}
