//! Release versions, and the order they come in.
//!
//! A whole semver implementation is not needed: what arrives is a tag such as `v0.2.0`, and
//! what is asked of it is "is this newer than what is running".

/// A release version: `major.minor.patch`, with an optional pre-release suffix such as
/// `0.2.0-rc.1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Option<String>,
}

impl Version {
    /// Parses `0.2.0`, `v0.2.0`, `0.2.0-rc.1`. Anything else is `None`.
    pub(super) fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);

        // Build metadata is ignored, as semver says it should be.
        let text = text.split('+').next().unwrap_or(text);

        let (numbers, pre) = match text.split_once('-') {
            Some((numbers, pre)) => (numbers, Some(pre.to_string())),
            None => (text, None),
        };

        if pre.as_deref() == Some("") {
            return None;
        }

        let mut parts = numbers.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }

        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre) = &self.pre {
            write!(f, "-{pre}")?;
        }
        Ok(())
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;

        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                // A pre-release sorts before the release it leads up to: `0.2.0-rc.1` is
                // older than `0.2.0`.
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (Some(a), Some(b)) => compare_prerelease(a, b),
                (None, None) => Ordering::Equal,
            })
    }
}

/// Compare two pre-release strings the way semver does: identifier by identifier.
///
/// Plain string comparison gets this wrong in a way that matters -- `rc.10` sorts *below* `rc.2`,
/// because the text `1` is less than `2` -- and `--check` would then offer an upgrade that is
/// actually a downgrade.
fn compare_prerelease(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let mut left = a.split('.');
    let mut right = b.split('.');

    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            // Fewer identifiers, with everything before them equal, is the lower version:
            // `1.0.0-rc` < `1.0.0-rc.1`.
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let ordering = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(nx), Ok(ny)) => nx.cmp(&ny),
                    // Numeric identifiers sort below alphanumeric ones.
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };

                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

// Checksums

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_parses_with_or_without_the_tag_prefix() {
        let want = Version {
            major: 0,
            minor: 2,
            patch: 0,
            pre: None,
        };

        assert_eq!(Version::parse("0.2.0"), Some(want.clone()));
        assert_eq!(Version::parse("v0.2.0"), Some(want.clone()));
        assert_eq!(Version::parse("0.2.0+build.5"), Some(want));
    }

    #[test]
    fn a_prerelease_parses() {
        let v = Version::parse("v0.2.0-rc.1").expect("should parse");
        assert_eq!(v.to_string(), "0.2.0-rc.1");
        assert_eq!(v.pre.as_deref(), Some("rc.1"));
    }

    #[test]
    fn nonsense_is_not_a_version() {
        for text in [
            "",
            "v",
            "0.2",
            "0.2.0.1",
            "one.two.three",
            "0.2.0-",
            "0.2.x",
        ] {
            assert_eq!(Version::parse(text), None, "{text} should not parse");
        }
    }

    #[test]
    fn versions_order_numerically() {
        let v = |s: &str| Version::parse(s).unwrap();

        assert!(v("0.2.0") > v("0.1.9"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert!(v("0.10.0") > v("0.9.0"));
        assert_eq!(v("0.2.0"), v("0.2.0"));
    }

    /// `0.2.0-rc.1` is what leads up to `0.2.0`, so a user on the release candidate is
    /// offered the release.
    #[test]
    fn a_prerelease_sorts_before_its_release() {
        let v = |s: &str| Version::parse(s).unwrap();

        assert!(v("0.2.0-rc.1") < v("0.2.0"));
        assert!(v("0.2.0-rc.1") > v("0.1.9"));
        assert_eq!(v("0.2.0-rc.1").to_string(), "0.2.0-rc.1");
    }

    /// Pre-release identifiers compare one by one, not as text: `rc.10` is *after* `rc.2`, so
    /// `--check` must not offer it as an upgrade to someone already on `rc.10`.
    #[test]
    fn prerelease_identifiers_compare_numerically() {
        let v = |s: &str| Version::parse(s).unwrap();

        assert!(v("0.2.0-rc.2") < v("0.2.0-rc.10"));
        assert!(v("0.2.0-rc.9") < v("0.2.0-rc.10"));
        assert!(v("0.2.0-alpha") < v("0.2.0-beta"));
        // Numeric identifiers sort below alphanumeric ones.
        assert!(v("0.2.0-1") < v("0.2.0-alpha"));
        // Fewer identifiers is the lower version when the rest match.
        assert!(v("0.2.0-rc") < v("0.2.0-rc.1"));
        assert_eq!(v("0.2.0-rc.1"), v("0.2.0-rc.1"));
    }
}
