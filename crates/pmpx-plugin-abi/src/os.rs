//! Conversions between `OsStr` and the raw bytes that cross the boundary.
//!
//! Behind the `std` feature, and shared by both sides on purpose: the plugin turns the host's bytes
//! back into an `OsString` (arguments, paths) and the loader turns an `OsStr` into bytes to send.
//! Two separate platform `cfg`s -- one per side -- would be duplication that inevitably drifts, and
//! the platform notes below are exactly the kind of thing that must be written down once.
//!
//! Lossless on Unix. On other platforms `OsString` is WTF-8 underneath, so a value that is not valid
//! UTF-8 degrades to U+FFFD -- that is the platform's boundary, not a choice made here.

use std::ffi::{OsStr, OsString};
use std::string::String;
use std::vec::Vec;

/// Turn raw bytes back into an `OsString`.
///
/// Strictly paired with [`os_to_bytes`]: anything this side produces, that side reads back.
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

/// Turn an `OsStr` into the raw bytes to send.
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
    fn a_round_trip_is_stable() {
        let text = OsStr::new("some/path --with-args");
        assert_eq!(bytes_to_os(&os_to_bytes(text)), text);
    }

    /// On Unix a path or argument is arbitrary bytes; the round trip must not touch them.
    #[cfg(unix)]
    #[test]
    fn unix_keeps_non_utf8_bytes_verbatim() {
        let raw = [b'/', b'x', 0xff, b'y'];
        let os = bytes_to_os(&raw);
        assert_eq!(os_to_bytes(&os), raw);
    }

    /// On the other platforms the conversion goes through UTF-8 and degrades instead of failing.
    #[cfg(not(unix))]
    #[test]
    fn non_unix_degrades_non_utf8_instead_of_failing() {
        let os = bytes_to_os(&[0xff]);
        assert!(
            !os.is_empty(),
            "the value survives as a replacement character"
        );
    }
}
