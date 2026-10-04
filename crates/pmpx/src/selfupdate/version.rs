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
                (Some(a), Some(b)) => a.cmp(b),
                (None, None) => Ordering::Equal,
            })
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
}
