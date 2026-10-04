//! Reading the memory the plugin hands back, and giving it back afterwards.

use pmpx_plugin::abi::PmpxStr;

/// Call one vtable function that returns a [`PmpxStr`], read it as a Rust string, and
/// **release it as agreed**.
///
/// # Safety
///
/// `f` and `free` must come from the same still-valid vtable.
pub(super) unsafe fn read_plugin_str(
    f: unsafe extern "C" fn() -> PmpxStr,
    free: unsafe extern "C" fn(PmpxStr),
) -> String {
    let s = unsafe { f() };
    let out = unsafe { read_bytes(s) };
    // The memory belongs to the plugin and must be given back after reading -- the host
    // never frees what it did not allocate.
    unsafe { free(s) };
    out
}

/// Read a [`PmpxStr`] without releasing it.
///
/// # Safety
///
/// `s` must describe read-only memory that is valid for the duration of the call.
pub(super) unsafe fn read_bytes(s: PmpxStr) -> String {
    if s.ptr.is_null() || s.len == 0 {
        return String::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(s.ptr, s.len) };
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_an_empty_string_is_safe() {
        assert_eq!(unsafe { read_bytes(PmpxStr::EMPTY) }, "");
    }

    #[test]
    fn reading_a_null_pointer_is_safe() {
        let s = PmpxStr {
            ptr: std::ptr::null(),
            len: 99,
        };
        assert_eq!(
            unsafe { read_bytes(s) },
            "",
            "a null pointer must not be dereferenced"
        );
    }

    #[test]
    fn reading_non_utf8_bytes_does_not_panic() {
        let bytes = [0xff, 0xfe, 0xfd];
        let s = PmpxStr {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        };
        // Diagnostics only, so a lossy conversion is enough -- but it must not panic
        assert!(!unsafe { read_bytes(s) }.is_empty());
    }
}
