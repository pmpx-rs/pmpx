//! Hint table used when no plugin is installed.
//!
//! It only produces a "this looks like a rust project" line when **no plugin is installed at all**,
//! and it **takes no part in any resolution**: once plugins are installed, detection is decided
//! entirely by the `detect` section of each plugin manifest, and this table is not even read.
//! A match does not recommend which plugin to install either.

use std::path::Path;

/// Family → typical detect files.
///
/// Values may use the `*.ext` form, which is treated as "the directory contains any file with this
/// extension" — a manifest whose file name is the project name, like `*.csproj`, cannot be listed
/// exhaustively.
pub const ECOSYSTEM_HINTS: &[(&str, &[&str])] = &[
    ("rust", &["Cargo.toml", "Cargo.lock"]),
    (
        "node",
        &[
            "package.json",
            "pnpm-lock.yaml",
            "pnpm-workspace.yaml",
            "yarn.lock",
            "package-lock.json",
            "bun.lock",
            "bun.lockb",
        ],
    ),
    ("python", &["pyproject.toml", "requirements.txt", "Pipfile"]),
    ("go", &["go.mod"]),
    ("jvm", &["pom.xml", "build.gradle", "build.gradle.kts"]),
    ("dotnet", &["*.csproj", "*.fsproj", "*.sln"]),
    ("php", &["composer.json"]),
    ("ruby", &["Gemfile"]),
];

/// The result of one probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    /// Family name. Same convention as `Family`'s key name.
    pub family: &'static str,
    /// The files actually matched (in declaration order in [`ECOSYSTEM_HINTS`]).
    pub matched: Vec<String>,
}

/// Look for clues in `dir`; only this level is looked at, no recursion.
pub fn probe(dir: &Path) -> Vec<Hint> {
    // The directory is listed **once** for the whole probe: one listing per `*.ext` pattern would
    // read the same directory again for every wildcard (`*.csproj`, `*.fsproj`, `*.sln`).
    let entries: Option<Vec<String>> = std::fs::read_dir(dir).ok().map(|list| {
        list.filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()
    });

    let mut out = Vec::new();

    for (family, patterns) in ECOSYSTEM_HINTS {
        let matched: Vec<String> = patterns
            .iter()
            .filter(|p| hits(dir, entries.as_deref(), p))
            .map(|p| (*p).to_string())
            .collect();

        if !matched.is_empty() {
            out.push(Hint { family, matched });
        }
    }

    out
}

/// Whether one declaration matched in `dir`.
///
/// `entries` is the listing from [`probe`], or `None` when the directory could not be read: a
/// `*.ext` pattern then cannot match, while the plain-name form still goes through `exists()`.
fn hits(dir: &Path, entries: Option<&[String]>, pattern: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(ext) => {
            let suffix = format!(".{ext}");
            entries
                .map(|names| {
                    names
                        .iter()
                        .any(|name| name.len() > suffix.len() && name.ends_with(&suffix))
                })
                .unwrap_or(false)
        }
        // Existing counts as a match, **a directory too** — this only answers "is this thing
        // there", not "what is it".
        None => dir.join(pattern).exists(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), "").unwrap();
    }

    #[test]
    fn an_empty_directory_yields_no_hints() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(probe(tmp.path()).is_empty());
    }

    #[test]
    fn detects_rust() {
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "Cargo.toml");

        let hints = probe(tmp.path());
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].family, "rust");
        assert_eq!(hints[0].matched, vec!["Cargo.toml"]);
    }

    #[test]
    fn collects_every_match_within_a_family() {
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "Cargo.toml");
        touch(tmp.path(), "Cargo.lock");

        let hints = probe(tmp.path());
        assert_eq!(hints[0].matched, vec!["Cargo.toml", "Cargo.lock"]);
    }

    #[test]
    fn detects_several_families_at_once() {
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "Cargo.toml");
        touch(tmp.path(), "package.json");

        let families: Vec<_> = probe(tmp.path()).into_iter().map(|h| h.family).collect();
        assert!(families.contains(&"rust"));
        assert!(families.contains(&"node"));
    }

    #[test]
    fn wildcard_patterns_scan_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "MyApp.csproj");

        let hints = probe(tmp.path());
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].family, "dotnet");
        assert_eq!(hints[0].matched, vec!["*.csproj"]);
    }

    #[test]
    fn wildcard_does_not_match_a_bare_extension_or_a_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "csproj"); // no dot
        touch(tmp.path(), "csproj.bak"); // wrong suffix

        assert!(probe(tmp.path()).is_empty());
    }

    #[test]
    fn wildcard_matches_any_extension_in_the_list() {
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "App.fsproj");

        let hints = probe(tmp.path());
        assert_eq!(hints[0].matched, vec!["*.fsproj"]);
    }

    /// The one directory listing the probe takes may come back empty (the directory is gone, or
    /// unreadable); a wildcard pattern then simply cannot match, and nothing panics.
    #[test]
    fn a_directory_that_cannot_be_listed_matches_no_wildcard() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(probe(&tmp.path().join("nope")).is_empty());
    }

    #[test]
    fn probe_does_not_recurse() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("nested")).unwrap();
        touch(&tmp.path().join("nested"), "Cargo.toml");

        assert!(probe(tmp.path()).is_empty(), "only this level is looked at");
    }

    /// The family name shown in the hint text must be lowercase ASCII only.
    #[test]
    fn every_hint_family_is_lowercase_ascii() {
        for (family, patterns) in ECOSYSTEM_HINTS {
            assert!(
                family.chars().all(|c| c.is_ascii_lowercase()),
                "the family name should be lowercase ASCII only: {family}"
            );
            assert!(!patterns.is_empty(), "{family} has no detect files");
        }
    }

    #[test]
    fn no_pattern_is_declared_twice_within_a_family() {
        for (family, patterns) in ECOSYSTEM_HINTS {
            let mut sorted = patterns.to_vec();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(
                sorted.len(),
                patterns.len(),
                "duplicate entries in {family}"
            );
        }
    }
}
