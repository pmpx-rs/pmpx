//! Turning bytes into `OsString`s and handing memory across the boundary.
//!
//! The two sides share no allocator, so memory is always freed by the side that allocated it:
//! inputs passed in by the host are read-only, and the strings a plugin leaks are handed back with
//! `free_str`. Every string flowing out of the plugin uses the same allocation (`Box<[u8]>`), so
//! freeing has exactly one path. A `free_*` must never use the host's `Box::from_raw` to adopt
//! memory that came from the other side.

use std::ffi::{OsStr, OsString};

use super::types::{PmpxStr, PMPX_ERR_INVALID_ARGS};

/// Leak a byte range into a [`PmpxStr`] for the other side of the boundary to read.
/// Every string flowing out of the plugin uses the same allocation (`Box<[u8]>`), so [`free_str`]
/// has exactly one path and cannot end up freeing a `Box<[u8]>` as a `Box<str>`.
pub fn leak_bytes(bytes: &[u8]) -> PmpxStr {
    let boxed: Box<[u8]> = bytes.to_vec().into_boxed_slice();
    let out = PmpxStr {
        ptr: boxed.as_ptr(),
        len: boxed.len(),
    };
    std::mem::forget(boxed);
    out
}

/// The `&str` version of [`leak_bytes`].
pub fn leak_str(s: &str) -> PmpxStr {
    leak_bytes(s.as_bytes())
}

/// Free a [`PmpxStr`] produced by this side's [`leak_bytes`] / [`leak_str`].
/// A null pointer returns immediately (that is how [`PmpxStr::EMPTY`] is used); length 0 with a
/// non-null pointer is a legitimate allocation and goes through `Box::from_raw` normally.
///
/// # Safety
/// - `s` must come from this side's `leak_*`, never from an input the host passed in;
/// - it may be freed only once.
pub unsafe fn free_str(s: PmpxStr) {
    if s.ptr.is_null() {
        return;
    }
    let raw = std::ptr::slice_from_raw_parts_mut(s.ptr as *mut u8, s.len);
    // Strictly paired with the Box<[u8]> in leak_bytes.
    drop(unsafe { Box::from_raw(raw) });
}

/// Read the bytes the host passed in as an `OsString`.
/// On Unix, paths and command-line arguments need not be valid UTF-8, and a `String` can only
/// convert lossily, which would silently corrupt calls like
/// `pmpx exec some-tool /latin1/path`; `OsString` keeps the raw bytes losslessly.
/// On Windows, `OsString` is WTF-8 underneath and unpaired surrogates degrade to lossy
/// replacement -- that is the platform's boundary.
///
/// # Safety
/// `s` must describe read-only memory that is valid for the duration of this call, or `len == 0`.
pub unsafe fn read_os(s: PmpxStr) -> OsString {
    if s.len == 0 {
        return OsString::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(s.ptr, s.len) };
    bytes_to_os(bytes)
}

/// Read the bytes the host passed in as a `&str`, checking UTF-8; a failure returns
/// [`PMPX_ERR_INVALID_ARGS`], and never `from_utf8_unchecked` -- that would assume the host is
/// always correct, and the whole job of this ABI is not to make that assumption.
///
/// The reference borrows memory the host owns, and [`PmpxStr`] carries no lifetime, so `'a` is
/// chosen by the caller: it must not outlive the call, and it is never `'static`. Callers that need
/// to keep the text have to copy it ([`read_os`] does, by building an owned `OsString`).
/// # Safety
/// Same as [`read_os`].
pub unsafe fn read_str<'a>(s: PmpxStr) -> Result<&'a str, u32> {
    if s.len == 0 {
        return Ok("");
    }
    let bytes = unsafe { std::slice::from_raw_parts(s.ptr, s.len) };
    std::str::from_utf8(bytes).map_err(|_| PMPX_ERR_INVALID_ARGS)
}

/// Convert raw bytes into an `OsString`.
/// Public because the host side does the same thing (turning `project_root` and `args` into bytes
/// to send across the boundary), and a separate platform `cfg` on each side would be duplication
/// that inevitably drifts. Lossless on Unix; on other platforms `OsString` is WTF-8 underneath,
/// so non-UTF-8 degrades to U+FFFD.
#[cfg(unix)]
pub fn bytes_to_os(bytes: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(bytes.to_vec())
}

/// See [`bytes_to_os`] for the platform notes.
#[cfg(not(unix))]
pub fn bytes_to_os(bytes: &[u8]) -> OsString {
    String::from_utf8_lossy(bytes).into_owned().into()
}

/// Convert an `OsStr` into raw bytes. Strictly paired with [`bytes_to_os`]: lossless on Unix, on
/// the other platforms it goes through `to_string_lossy` and non-UTF-8 degrades to U+FFFD.
#[cfg(unix)]
pub fn os_to_bytes(s: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    s.as_bytes().to_vec()
}

/// See [`os_to_bytes`] for the platform notes.
#[cfg(not(unix))]
pub fn os_to_bytes(s: &OsStr) -> Vec<u8> {
    s.to_string_lossy().into_owned().into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_str_reads_as_empty() {
        assert_eq!(unsafe { read_os(PmpxStr::EMPTY) }, OsString::new());
        assert_eq!(unsafe { read_str(PmpxStr::EMPTY) }.unwrap(), "");
    }

    #[test]
    fn leak_and_free_round_trip() {
        let s = leak_str("hello");
        assert_eq!(s.len, 5);
        assert_eq!(
            unsafe { std::slice::from_raw_parts(s.ptr, s.len) },
            b"hello"
        );
        unsafe { free_str(s) };
    }

    #[test]
    fn free_str_tolerates_null() {
        // EMPTY is passed to free_str unconditionally by free_command
        unsafe { free_str(PmpxStr::EMPTY) };
    }

    #[test]
    fn leak_and_free_an_empty_string() {
        // The Box<[u8]> of an empty string is a dangling pointer (non-null, len 0) and must still
        // free cleanly
        let s = leak_str("");
        assert_eq!(s.len, 0);
        assert!(!s.ptr.is_null(), "an empty Box dangles but is not null");
        unsafe { free_str(s) };
    }

    #[test]
    fn read_str_rejects_invalid_utf8() {
        let bytes = [0xff, 0xfe];
        let s = PmpxStr {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        };
        assert_eq!(unsafe { read_str(s) }, Err(PMPX_ERR_INVALID_ARGS));
    }

    #[test]
    fn read_os_round_trips_valid_utf8() {
        let bytes = "/tmp/projéct/ünïcode".as_bytes();
        let s = PmpxStr {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        };
        let got = unsafe { read_os(s) };
        assert_eq!(os_to_bytes(&got), bytes);
    }

    /// On Unix a path or an argument may be arbitrary bytes (0xFF is not valid UTF-8, but it is a
    /// legitimate path byte) and `OsString` must keep them losslessly.
    #[cfg(unix)]
    #[test]
    fn read_os_keeps_arbitrary_bytes_on_unix() {
        let bytes = [0x2f, 0x62, 0x61, 0x64, 0xff];
        let s = PmpxStr {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        };
        let got = unsafe { read_os(s) };
        assert_eq!(os_to_bytes(&got), bytes, "must be lossless on Unix");
    }

    /// Off Unix, `OsString` is WTF-8 underneath, so non-UTF-8 degrades to U+FFFD -- a platform
    /// boundary that the test pins down as known behaviour instead of pretending otherwise.
    #[cfg(not(unix))]
    #[test]
    fn read_os_replaces_invalid_utf8_off_unix() {
        let bytes = [0x2f, 0x62, 0xff];
        let s = PmpxStr {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        };
        let got = unsafe { read_os(s) };
        let expected = String::from_utf8_lossy(&bytes).into_owned().into_bytes();
        assert_eq!(os_to_bytes(&got), expected);
        assert_ne!(os_to_bytes(&got), bytes, "off Unix it really is lossy");
    }
}
