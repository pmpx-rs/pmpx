//! Where the release is, and whether the bytes are the right ones.
//!
//! The names built here are an interface shared with `.github/workflows/assets.yaml` and
//! `scripts/package-release.*`: change one side alone and every update fails at the
//! download, or worse, quietly finds a different release.

use anyhow::{anyhow, Context as _};

use super::version::Version;

/// Where the releases come from.
///
/// Taken from `Cargo.toml` so that it cannot drift away from the manifest the crate is
/// published with.
pub(super) const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// The largest thing this will download. The archives are a couple of megabytes; this only
/// guards against a URL that somehow points somewhere enormous.
pub(super) const MAX_ASSET_BYTES: u64 = 128 * 1024 * 1024;

/// `(owner, repo)` of the releases, out of the manifest's repository URL.
///
/// Eight lines rather than a dependency: this is the whole of what `self update` needs to know about
/// GitHub, and a URL it cannot read is reported rather than guessed at.
pub(super) fn repository_parts() -> anyhow::Result<(String, String)> {
    let not_github = || {
        anyhow!(
            "`pmpx self update` only knows how to read GitHub releases, and {REPOSITORY} is not a \
             GitHub repository"
        )
    };

    let rest = REPOSITORY
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .strip_prefix("https://github.com/")
        .or_else(|| REPOSITORY.strip_prefix("http://github.com/"))
        .ok_or_else(not_github)?;

    let (owner, repo) = rest.split_once('/').ok_or_else(not_github)?;

    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return Err(not_github());
    }

    Ok((owner.to_string(), repo.to_string()))
}

/// `pmpx-<target>`, the part of the asset name that is the same on every platform.
pub(super) fn asset_stem() -> String {
    format!("pmpx-{}", env!("PMPX_TARGET"))
}

/// The archive a platform publishes: a zip on Windows, a tarball everywhere else.
pub(super) fn archive_ext(os: &str) -> &'static str {
    if os == "windows" {
        "zip"
    } else {
        "tar.gz"
    }
}

/// The binary inside the archive.
pub(super) fn binary_name(os: &str) -> &'static str {
    if os == "windows" {
        "pmpx.exe"
    } else {
        "pmpx"
    }
}

// Talking to GitHub

/// The newest release's version.
pub(super) fn latest_version() -> anyhow::Result<Version> {
    let (owner, repo) = repository_parts()?;
    let url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");

    let body = match download(&url) {
        Ok(Some(body)) => body,
        // No release has been published for this repository yet. That is a state, not a
        // failure of the request.
        Ok(None) => return Err(anyhow!("no release has been published yet")),
        Err(error) => return Err(error.context(format!("cannot ask {url}"))),
    };

    let json: serde_json::Value =
        serde_json::from_slice(&body).context("the releases API did not return JSON")?;

    let tag = json
        .get("tag_name")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow!("the releases API returned no tag_name"))?;

    Version::parse(tag)
        .ok_or_else(|| anyhow!("the latest release is tagged {tag}, which is not a version"))
}

/// How to read a request GitHub refused.
///
/// 403 and 429 are what GitHub answers for unauthenticated rate limiting -- and a repository that
/// is not public answers 403 as well, so both causes have to be named. This used to be one caller
/// matching on the text `"403"` inside the error, which any change to the message would have
/// silently broken.
fn refused_message(code: u16, url: &str) -> String {
    format!(
        "GitHub refused the request ({code}): it rate-limits unauthenticated requests, and a \
         repository that is not public answers the same way ({url})"
    )
}

/// Downloads one URL. `Ok(None)` is a 404, which for a release asset means "not published".
pub(super) fn download(url: &str) -> anyhow::Result<Option<Vec<u8>>> {
    let request = ureq::get(url).header("User-Agent", &user_agent());

    match request.call() {
        Ok(mut response) => {
            let body = response
                .body_mut()
                .read_to_vec()
                .with_context(|| format!("cannot read the response from {url}"))?;

            if body.len() as u64 > MAX_ASSET_BYTES {
                return Err(anyhow!(
                    "{url} is larger than the {MAX_ASSET_BYTES} byte limit ({} bytes)",
                    body.len()
                ));
            }

            Ok(Some(body))
        }
        Err(ureq::Error::StatusCode(404)) => Ok(None),
        Err(ureq::Error::StatusCode(code)) if code == 403 || code == 429 => {
            Err(anyhow!("{}", refused_message(code, url)))
        }
        Err(error) => Err(anyhow!("{error} ({url})")),
    }
}

pub(super) fn user_agent() -> String {
    format!("pmpx/{} (+{REPOSITORY})", env!("CARGO_PKG_VERSION"))
}

// Versions

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_archive_matches_the_platform() {
        assert_eq!(archive_ext("windows"), "zip");
        assert_eq!(archive_ext("linux"), "tar.gz");
        assert_eq!(archive_ext("macos"), "tar.gz");

        assert_eq!(binary_name("windows"), "pmpx.exe");
        assert_eq!(binary_name("linux"), "pmpx");
        assert_eq!(binary_name("macos"), "pmpx");
    }

    #[test]
    fn the_asset_stem_carries_the_target() {
        assert_eq!(asset_stem(), format!("pmpx-{}", env!("PMPX_TARGET")));
    }

    /// A refused request has two possible causes and the message has to name both, because there
    /// is no way to tell them apart from the status code alone.
    #[test]
    fn a_refused_request_explains_both_causes() {
        let message = refused_message(403, "https://example.invalid/x");

        assert!(message.contains("403"), "{message}");
        assert!(message.contains("rate-limits"), "{message}");
        assert!(message.contains("not public"), "{message}");
        assert!(message.contains("https://example.invalid/x"), "{message}");
    }
}
