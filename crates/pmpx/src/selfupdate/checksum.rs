//! SHA-256: computing it, and reading the file that publishes it.

use std::path::Path;

/// The SHA-256 of `bytes`, as lowercase hex.
pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);

    let mut out = String::with_capacity(64);
    for byte in digest.as_ref() {
        // Writing into a String cannot fail.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The hash `SHA256SUMS` lists for `file`.
///
/// GNU `sha256sum` writes `hash  name` in text mode and `hash *name` in binary mode, and
/// which one it writes depends on the platform that produced the file -- so both are
/// accepted. A release's `SHA256SUMS` is produced on Linux, but a locally reproduced one
/// may not be.
pub(super) fn parse_sha256sums(text: &str, file: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // A line with no whitespace at all is just another unparseable line: skip it and keep
        // looking, exactly like the two cases below. (`?` here used to end the whole search, so a
        // single stray line made pmpx report "this release has nothing for your platform".)
        let Some((hash, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }

        let name = rest.trim_start().trim_start_matches('*').trim();
        let same =
            name == file || Path::new(name).file_name().and_then(|n| n.to_str()) == Some(file);

        if same {
            return Some(hash.to_ascii_lowercase());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_accepts_text_and_binary_mode_lines() {
        let text = "\
b546e4e4c085b5f3699220f8f1dee442bd65d389f0930e376f37ac5effa99df0  pmpx-x86_64-unknown-linux-gnu.tar.gz
814c3e352f5c0a651a23b03f3f379135b314b625ed4422b2d27c7081126eac7a *pmpx-x86_64-pc-windows-msvc.zip
";

        assert_eq!(
            parse_sha256sums(text, "pmpx-x86_64-unknown-linux-gnu.tar.gz").as_deref(),
            Some("b546e4e4c085b5f3699220f8f1dee442bd65d389f0930e376f37ac5effa99df0")
        );
        assert_eq!(
            parse_sha256sums(text, "pmpx-x86_64-pc-windows-msvc.zip").as_deref(),
            Some("814c3e352f5c0a651a23b03f3f379135b314b625ed4422b2d27c7081126eac7a")
        );
    }

    #[test]
    fn sums_ignores_crlf_and_an_unknown_name() {
        let text =
            "b546e4e4c085b5f3699220f8f1dee442bd65d389f0930e376f37ac5effa99df0  other.tar.gz\r\n";

        assert_eq!(parse_sha256sums(text, "pmpx-other.tar.gz"), None);
        assert_eq!(
            parse_sha256sums(text, "other.tar.gz").as_deref(),
            Some("b546e4e4c085b5f3699220f8f1dee442bd65d389f0930e376f37ac5effa99df0")
        );
    }

    #[test]
    fn sums_rejects_a_line_that_is_not_a_hash() {
        let text = "not-a-hash  pmpx-x86_64-unknown-linux-gnu.tar.gz\n";

        assert_eq!(
            parse_sha256sums(text, "pmpx-x86_64-unknown-linux-gnu.tar.gz"),
            None
        );
    }

    /// A stray line with no whitespace at all must not end the search: the wanted entry is still
    /// further down the file, and stopping early would report "this release has nothing for your
    /// platform" for a release that has it.
    #[test]
    fn sums_keeps_looking_past_a_line_without_whitespace() {
        let text = "\
binary
b546e4e4c085b5f3699220f8f1dee442bd65d389f0930e376f37ac5effa99df0  pmpx-x86_64-unknown-linux-gnu.tar.gz
";
        assert_eq!(
            parse_sha256sums(text, "pmpx-x86_64-unknown-linux-gnu.tar.gz").as_deref(),
            Some("b546e4e4c085b5f3699220f8f1dee442bd65d389f0930e376f37ac5effa99df0")
        );
    }

    #[test]
    fn sums_of_only_stray_lines_finds_nothing() {
        assert_eq!(parse_sha256sums("binary\nlines\n", "anything"), None);
    }

    #[test]
    fn sha256_matches_the_published_vector() {
        // The classic one: SHA-256("abc").
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
