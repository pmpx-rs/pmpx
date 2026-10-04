//! 零插件时的提示表。
//!
//! 它只在"一个插件都没装"时给出一句"这看起来是个 rust 项目"，**不参与任何裁决**：
//! 装上插件之后，检测完全由各插件 manifest 的 `detect` 段决定，这张表连读都不读。
//! 匹配到了也不推荐装哪个插件。

use std::path::Path;

/// 生态 → 典型的特征文件。
///
/// 值里可以出现 `*.ext` 形式，它会被当成"目录里有任意这个扩展名的文件" ——
/// `*.csproj` 那种文件名是项目名的清单没法列全。
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

/// 一次探测的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    /// 生态名。与 `Family` 的键名同口径。
    pub family: &'static str,
    /// 实际命中的文件（按 [`ECOSYSTEM_HINTS`] 里的声明顺序）。
    pub matched: Vec<String>,
}

/// 在 `dir` 里找线索，只看这一层，不递归。
pub fn probe(dir: &Path) -> Vec<Hint> {
    let mut out = Vec::new();

    for (family, patterns) in ECOSYSTEM_HINTS {
        let matched: Vec<String> = patterns
            .iter()
            .filter(|p| hits(dir, p))
            .map(|p| (*p).to_string())
            .collect();

        if !matched.is_empty() {
            out.push(Hint { family, matched });
        }
    }

    out
}

/// 某一条声明在 `dir` 里命中了没有。
fn hits(dir: &Path, pattern: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(ext) => {
            let suffix = format!(".{ext}");
            std::fs::read_dir(dir)
                .map(|entries| {
                    entries.filter_map(Result::ok).any(|e| {
                        let name = e.file_name();
                        let name = name.to_string_lossy();
                        name.len() > suffix.len() && name.ends_with(&suffix)
                    })
                })
                .unwrap_or(false)
        }
        // 存在就算命中，**目录也算** —— 只回答"有没有这个东西"，不回答"它是什么"。
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
        touch(tmp.path(), "csproj"); // 没有点
        touch(tmp.path(), "csproj.bak"); // 后缀不对

        assert!(probe(tmp.path()).is_empty());
    }

    #[test]
    fn wildcard_matches_any_extension_in_the_list() {
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "App.fsproj");

        let hints = probe(tmp.path());
        assert_eq!(hints[0].matched, vec!["*.fsproj"]);
    }

    #[test]
    fn probe_does_not_recurse() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("nested")).unwrap();
        touch(&tmp.path().join("nested"), "Cargo.toml");

        assert!(probe(tmp.path()).is_empty(), "只看这一层");
    }

    /// 提示文案里显示的生态名必须是纯小写 ASCII。
    #[test]
    fn every_hint_family_is_lowercase_ascii() {
        for (family, patterns) in ECOSYSTEM_HINTS {
            assert!(
                family.chars().all(|c| c.is_ascii_lowercase()),
                "生态名应当是纯小写 ASCII：{family}"
            );
            assert!(!patterns.is_empty(), "{family} 没有任何特征文件");
        }
    }

    #[test]
    fn no_pattern_is_declared_twice_within_a_family() {
        for (family, patterns) in ECOSYSTEM_HINTS {
            let mut sorted = patterns.to_vec();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted.len(), patterns.len(), "{family} 里有重复项");
        }
    }
}
