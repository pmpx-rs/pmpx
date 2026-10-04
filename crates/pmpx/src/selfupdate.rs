//! `pmpx self update` -- replacing this binary with one from the project's releases.
//!
//! The design lives in `docs/proposal.md` §13. The rules that shape this file:
//!
//! - **Exactly one file is written**, and only after the download has been verified: the
//!   running executable. Everything else is a scratch file beside it.
//! - **A cargo installation does not self-update.** `cargo install` keeps its own record
//!   of what it installed; replacing the file behind its back would make
//!   `cargo install --list` disagree with reality. Those installations are told to run
//!   `cargo install pmpx --force` instead.
//! - **Nothing is checked automatically.** No background check, no "a new version is
//!   available" note on startup. The only thing that happens on startup is deleting the
//!   `.old` binary a previous update had to leave behind.
//! - **The new binary is staged in the same directory** as the old one, because the swap
//!   is a `rename` and that is only atomic within one filesystem.
//!
//! The asset names it fetches are produced by `.github/workflows/assets.yaml` and
//! `scripts/package-release.*`, which is where the other half of this contract lives.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context as _};

use crate::error::{PmpxError, EXIT_OK};
use crate::style;

/// Where the releases come from.
///
/// Taken from `Cargo.toml` so that it cannot drift away from the manifest the crate is
/// published with.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// The largest thing this will download. The archives are a couple of megabytes; this only
/// guards against a URL that somehow points somewhere enormous.
const MAX_ASSET_BYTES: u64 = 128 * 1024 * 1024;

/// What the command line asked for.
pub struct Request {
    /// Report what is available and change nothing.
    pub check: bool,
    /// Install this version instead of the newest one. This is also how a rollback is done.
    pub version: Option<String>,
    /// Reinstall even when the version is already current.
    pub force: bool,
}

/// Runs the update and returns the process exit code.
pub fn run(request: &Request) -> crate::error::Result<u8> {
    // A version that does not parse is a typo on the command line, not a broken
    // environment, so it keeps the usage exit code.
    let requested = match &request.version {
        Some(text) => Some(Version::parse(text).ok_or_else(|| {
            PmpxError::Usage(format!("`--version {text}` is not a version like 0.2.0"))
        })?),
        None => None,
    };

    // `{:#}` flattens anyhow's cause chain into the message. `PmpxError::Other` is printed
    // transparently, so without this the reason for a failure ("no release has been
    // published yet") would be swallowed and only the context around it ("cannot find out
    // what the latest release is") would reach the person reading it.
    run_validated(request, requested).map_err(|error| PmpxError::Other(anyhow!("{error:#}")))
}

fn run_validated(request: &Request, requested: Option<Version>) -> anyhow::Result<u8> {
    let exe = std::env::current_exe().context("cannot tell which binary is running")?;
    let kind = install_kind(&exe, cargo_bin_dir().as_deref());
    let current = Version::parse(env!("CARGO_PKG_VERSION"))
        .ok_or_else(|| anyhow!("the compiled-in version is not a version"))?;

    // `--check` answers a question rather than doing something, so it works for every
    // installation -- including one that could not install the answer.
    if request.check {
        let latest = latest_version().context("cannot find out what the latest release is")?;
        report_check(&current, &latest, kind);
        return Ok(EXIT_OK);
    }

    if kind == InstallKind::Cargo {
        return Err(anyhow!(
            "this pmpx was installed by cargo, which keeps its own record of what it put \
             there -- replacing the file behind its back would make `cargo install --list` \
             disagree with reality.\n\
             Use `cargo install pmpx --force` instead."
        ));
    }

    // Only ask the API when the version was not spelled out: an explicit version has to
    // work even while the API is unreachable.
    let wanted = match requested {
        Some(version) => version,
        None => latest_version().context("cannot find out what the latest release is")?,
    };

    if wanted == current && !request.force {
        anstream::println!(
            "pmpx {current} is already installed. Use {} to reinstall it.",
            style::paint(style::PM, "--force")
        );
        return Ok(EXIT_OK);
    }

    update(&exe, &wanted)?;

    anstream::println!("{} {current} -> {wanted}", style::paint(style::PM, "pmpx"));
    anstream::println!(
        "{}",
        style::paint(
            style::DIM,
            "The new binary takes effect next time you run pmpx."
        )
    );
    Ok(EXIT_OK)
}

/// What `--check` prints.
fn report_check(current: &Version, latest: &Version, kind: InstallKind) {
    if latest > current {
        anstream::println!(
            "pmpx {latest} is available {}",
            style::paint(style::DIM, format!("(you are on {current})"))
        );
    } else {
        anstream::println!("pmpx {current} is the latest release.");
    }

    if kind == InstallKind::Cargo {
        anstream::println!(
            "{}",
            style::paint(
                style::DIM,
                "This installation was made by cargo, so `pmpx self update` will not \
                 replace it; use `cargo install pmpx --force`."
            )
        );
    }
}

// ---------------------------------------------------------------------------
// The update itself
// ---------------------------------------------------------------------------

/// Downloads, verifies, and swaps in the release for `version`.
fn update(exe: &Path, version: &Version) -> anyhow::Result<()> {
    let (owner, repo) = repository_parts()?;
    let archive_name = format!("{}.{}", asset_stem(), archive_ext(std::env::consts::OS));
    let base = format!("https://github.com/{owner}/{repo}/releases/download/v{version}");

    // 1. The checksum file first: without it there is nothing to verify against, and
    //    installing unverified bytes is the one thing this command must never do.
    let sums_url = format!("{base}/SHA256SUMS");
    let sums = download(&sums_url)
        .with_context(|| format!("cannot download {sums_url}"))?
        .ok_or_else(|| {
            anyhow!(
                "release v{version} has no SHA256SUMS, so there is nothing to verify the \
                 download against -- refusing to install it"
            )
        })?;

    let sums = String::from_utf8(sums).context("SHA256SUMS is not valid UTF-8")?;
    let wanted = parse_sha256sums(&sums, &archive_name).ok_or_else(|| {
        anyhow!("SHA256SUMS does not list {archive_name}, so this release has nothing for this platform")
    })?;

    // 2. The archive, then its hash. A mismatch stops here: nothing has been written yet.
    let archive_url = format!("{base}/{archive_name}");
    let archive = download(&archive_url)
        .with_context(|| format!("cannot download {archive_url}"))?
        .ok_or_else(|| anyhow!("release v{version} has no {archive_name}"))?;

    let got = sha256_hex(&archive);
    if got != wanted {
        return Err(anyhow!(
            "the downloaded {archive_name} does not match SHA256SUMS\n  expected {wanted}\n  got      {got}\n\
             Nothing was changed."
        ));
    }

    // 3. Stage it beside the current binary and swap. Same directory on purpose: the swap
    //    is a rename, and a rename across filesystems is a copy that can be interrupted
    //    halfway.
    let directory = exe.parent().ok_or_else(|| {
        anyhow!(
            "cannot tell which directory the binary is in: {}",
            exe.display()
        )
    })?;

    let scratch = directory.join(format!(".pmpx-update-{}", std::process::id()));
    remove_dir_if_exists(&scratch);
    fs::create_dir_all(&scratch).with_context(|| {
        format!(
            "cannot create {} (is the directory writable?)",
            scratch.display()
        )
    })?;
    let scratch = Scratch::new(scratch);

    let extracted = extract(archive_name.ends_with(".zip"), &archive, &scratch.path)?;

    let staged = append_to_name(exe, ".new");
    fs::rename(&extracted, &staged)
        .with_context(|| format!("cannot stage the new binary at {}", staged.display()))?;

    replace(exe, &staged)?;

    anstream::println!(
        "{}",
        style::paint(style::DIM, format!("replaced {}", exe.display()))
    );
    Ok(())
}

/// Removes a scratch directory when it goes out of scope.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn remove_dir_if_exists(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

/// Extracts the binary out of a release archive into `dir`, returning its path.
fn extract(is_zip: bool, archive: &[u8], dir: &Path) -> anyhow::Result<PathBuf> {
    let member = binary_name(std::env::consts::OS);

    let extracted = if is_zip {
        extract_from_zip(archive, member, dir)?
    } else {
        extract_from_tar_gz(archive, member, dir)?
    };

    // The archive is built from a compiled binary, so it carries the executable bit -- but
    // this is the file that is about to become `pmpx`, and a mode bit lost in transit
    // would leave an installation that cannot be started. Set it explicitly rather than
    // trusting the archive.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&extracted, fs::Permissions::from_mode(0o755))
            .context("cannot make the new binary executable")?;
    }

    Ok(extracted)
}

/// The `pmpx` member of a `.tar.gz`, written next to nothing else at the archive root.
fn extract_from_tar_gz(archive: &[u8], member: &str, dir: &Path) -> anyhow::Result<PathBuf> {
    let decoder = flate2::read::GzDecoder::new(archive);
    let mut tar = tar::Archive::new(decoder);

    for entry in tar
        .entries()
        .context("the release archive is not a tar file")?
    {
        let mut entry = entry.context("the release archive is damaged")?;
        let path = entry
            .path()
            .context("an entry in the archive has no name")?;

        if path.file_name().and_then(|name| name.to_str()) != Some(member) {
            continue;
        }

        let dest = dir.join(member);
        entry
            .unpack(&dest)
            .with_context(|| format!("cannot write {}", dest.display()))?;
        return Ok(dest);
    }

    Err(anyhow!("the release archive does not contain {member}"))
}

/// The `pmpx.exe` member of a `.zip`.
fn extract_from_zip(archive: &[u8], member: &str, dir: &Path) -> anyhow::Result<PathBuf> {
    let reader = std::io::Cursor::new(archive);
    let mut zip = zip::ZipArchive::new(reader).context("the release archive is not a zip file")?;

    for index in 0..zip.len() {
        let mut file = zip
            .by_index(index)
            .context("the release archive is damaged")?;
        if !file.is_file() {
            continue;
        }

        let name = file.name().to_string();
        if Path::new(&name).file_name().and_then(|name| name.to_str()) != Some(member) {
            continue;
        }

        let dest = dir.join(member);
        let mut out =
            fs::File::create(&dest).with_context(|| format!("cannot write {}", dest.display()))?;
        std::io::copy(&mut file, &mut out).context("cannot read the archive entry")?;
        return Ok(dest);
    }

    Err(anyhow!("the release archive does not contain {member}"))
}

/// Puts `staged` in place of `exe`.
#[cfg(unix)]
fn replace(exe: &Path, staged: &Path) -> anyhow::Result<()> {
    // On Unix a running executable can simply be replaced: the old inode stays alive
    // until this process exits.
    fs::rename(staged, exe).with_context(|| format!("cannot replace {}", exe.display()))
}

/// Puts `staged` in place of `exe`.
#[cfg(windows)]
fn replace(exe: &Path, staged: &Path) -> anyhow::Result<()> {
    // Windows will not let a running executable be overwritten or deleted, but it will let
    // it be *renamed*. So: move the old one aside, move the new one in, and try to delete
    // the old one -- which usually fails while this process is still running it, hence
    // `cleanup_stale_old` on the next start.
    let old = append_to_name(exe, ".old");
    let _ = fs::remove_file(&old);

    fs::rename(exe, &old).with_context(|| {
        format!(
            "cannot move the running binary aside to {} -- is that directory writable by you?",
            old.display()
        )
    })?;

    if let Err(error) = fs::rename(staged, exe) {
        // Put the old binary back rather than leaving the user with no pmpx at all.
        let _ = fs::rename(&old, exe);
        let _ = fs::remove_file(staged);
        return Err(error)
            .with_context(|| format!("cannot install the new binary at {}", exe.display()));
    }

    let _ = fs::remove_file(&old);
    Ok(())
}

/// Deletes the `.old` binary a previous Windows update had to leave behind.
///
/// It cannot be deleted at update time -- that process is still running it -- so every
/// start tries once and says nothing: a failure only means "next time". On other platforms
/// an update never leaves one, so this costs nothing at all.
#[cfg(windows)]
pub fn cleanup_stale_old() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = fs::remove_file(append_to_name(&exe, ".old"));
    }
}

/// Deletes the `.old` binary a previous Windows update had to leave behind.
#[cfg(not(windows))]
pub fn cleanup_stale_old() {}

/// `pmpx` -> `pmpx.new` (the extension is appended, not replaced, so `pmpx.exe` becomes
/// `pmpx.exe.new` rather than `pmpx.new`).
fn append_to_name(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Whether this binary was put here by `cargo install`.
///
/// Two signals, because either one alone has a hole: the cargo bin directory (which
/// `CARGO_HOME` can move) and the `.crates.toml` cargo writes next to the binaries it
/// manages.
fn install_kind(exe: &Path, cargo_bin: Option<&Path>) -> InstallKind {
    if let Some(bin) = cargo_bin {
        if exe.parent() == Some(bin) {
            return InstallKind::Cargo;
        }
    }

    let managed = exe
        .parent()
        .map(|dir| dir.join(".crates.toml").is_file())
        .unwrap_or(false);

    if managed {
        InstallKind::Cargo
    } else {
        InstallKind::Standalone
    }
}

/// `$CARGO_HOME/bin`, or `~/.cargo/bin` when that is not set.
fn cargo_bin_dir() -> Option<PathBuf> {
    let home = match std::env::var_os("CARGO_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => directories::UserDirs::new()?.home_dir().join(".cargo"),
    };
    Some(home.join("bin"))
}

/// `(owner, repo)` of the releases, out of the manifest's repository URL.
fn repository_parts() -> anyhow::Result<(String, String)> {
    crate_plugin_kit::install::prebuilt::parse_github_repo(REPOSITORY).ok_or_else(|| {
        anyhow!("`pmpx self update` only knows how to read GitHub releases, and {REPOSITORY} is not a GitHub repository")
    })
}

/// `pmpx-<target>`, the part of the asset name that is the same on every platform.
fn asset_stem() -> String {
    format!("pmpx-{}", crate_plugin_kit::TARGET_TRIPLE)
}

/// The archive a platform publishes: a zip on Windows, a tarball everywhere else.
fn archive_ext(os: &str) -> &'static str {
    if os == "windows" {
        "zip"
    } else {
        "tar.gz"
    }
}

/// The binary inside the archive.
fn binary_name(os: &str) -> &'static str {
    if os == "windows" {
        "pmpx.exe"
    } else {
        "pmpx"
    }
}

// ---------------------------------------------------------------------------
// Talking to GitHub
// ---------------------------------------------------------------------------

/// The newest release's version.
fn latest_version() -> anyhow::Result<Version> {
    let (owner, repo) = repository_parts()?;
    let url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");

    let body = match download(&url) {
        Ok(Some(body)) => body,
        // No release has been published for this repository yet. That is a state, not a
        // failure of the request.
        Ok(None) => return Err(anyhow!("no release has been published yet")),
        Err(error) => {
            let mut error = error;
            if error.to_string().contains("403") {
                error = error.context(
                    "GitHub rate-limits requests that are not authenticated; trying again later usually works",
                );
            }
            return Err(error.context(format!("cannot ask {url}")));
        }
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

/// Downloads one URL. `Ok(None)` is a 404, which for a release asset means "not published".
fn download(url: &str) -> anyhow::Result<Option<Vec<u8>>> {
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
        Err(error) => Err(anyhow!("{error} ({url})")),
    }
}

fn user_agent() -> String {
    format!("pmpx/{} (+{REPOSITORY})", env!("CARGO_PKG_VERSION"))
}

// ---------------------------------------------------------------------------
// Versions
// ---------------------------------------------------------------------------

/// A release version: `major.minor.patch`, with an optional pre-release suffix such as
/// `0.2.0-rc.1`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Option<String>,
}

impl Version {
    /// Parses `0.2.0`, `v0.2.0`, `0.2.0-rc.1`. Anything else is `None`.
    fn parse(text: &str) -> Option<Self> {
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

// ---------------------------------------------------------------------------
// Checksums
// ---------------------------------------------------------------------------

/// The SHA-256 of `bytes`, as lowercase hex.
fn sha256_hex(bytes: &[u8]) -> String {
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
fn parse_sha256sums(text: &str, file: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let (hash, rest) = line.split_once(char::is_whitespace)?;
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

/// Where this installation came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallKind {
    /// `cargo install` put it there, and keeps a record of doing so.
    Cargo,
    /// Anything else: the install script, a release archive, a build in a checkout.
    Standalone,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- versions ---------------------------------------------------------

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

    // ---- asset names ------------------------------------------------------

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
        assert_eq!(
            asset_stem(),
            format!("pmpx-{}", crate_plugin_kit::TARGET_TRIPLE)
        );
    }

    // ---- checksums --------------------------------------------------------

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

    #[test]
    fn sha256_matches_the_published_vector() {
        // The classic one: SHA-256("abc").
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    // ---- install kind -----------------------------------------------------

    #[test]
    fn a_binary_in_the_cargo_bin_directory_is_a_cargo_install() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join(binary_name("linux"));

        assert_eq!(install_kind(&exe, Some(&bin)), InstallKind::Cargo);
    }

    /// `CARGO_HOME` can point anywhere, and the record cargo writes is the second signal.
    #[test]
    fn a_crates_toml_next_to_the_binary_is_a_cargo_install() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".crates.toml"), "[v1]\n").unwrap();
        let exe = tmp.path().join(binary_name("linux"));

        assert_eq!(
            install_kind(&exe, Some(Path::new("/elsewhere"))),
            InstallKind::Cargo
        );
    }

    #[test]
    fn a_binary_somewhere_else_is_standalone() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join(binary_name("linux"));

        assert_eq!(
            install_kind(&exe, Some(Path::new("/elsewhere"))),
            InstallKind::Standalone
        );
        assert_eq!(install_kind(&exe, None), InstallKind::Standalone);
    }

    // ---- archives ---------------------------------------------------------

    /// The Unix shape, built here rather than downloaded: a `.tar.gz` whose root holds the
    /// binary and the licence.
    fn tar_gz_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, body) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, name, *body).unwrap();
        }
        let tar = builder.into_inner().unwrap();

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &tar).unwrap();
        encoder.finish().unwrap()
    }

    /// The Windows shape.
    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write as _;

        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        for (name, body) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(body).unwrap();
        }

        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn the_binary_comes_out_of_a_tarball() {
        let archive = tar_gz_with(&[("LICENSE", b"licence"), ("pmpx", b"the new pmpx")]);
        let tmp = tempfile::tempdir().unwrap();

        let got = extract_from_tar_gz(&archive, "pmpx", tmp.path()).expect("should extract");

        assert_eq!(std::fs::read(&got).unwrap(), b"the new pmpx");
        assert_eq!(got.file_name().unwrap(), "pmpx");
    }

    #[test]
    fn the_binary_comes_out_of_a_zip() {
        let archive = zip_with(&[("LICENSE", b"licence"), ("pmpx.exe", b"the new pmpx.exe")]);
        let tmp = tempfile::tempdir().unwrap();

        let got = extract_from_zip(&archive, "pmpx.exe", tmp.path()).expect("should extract");

        assert_eq!(std::fs::read(&got).unwrap(), b"the new pmpx.exe");
        assert_eq!(got.file_name().unwrap(), "pmpx.exe");
    }

    #[test]
    fn an_archive_without_the_binary_is_refused() {
        let archive = tar_gz_with(&[("LICENSE", b"licence")]);
        let tmp = tempfile::tempdir().unwrap();

        let error = extract_from_tar_gz(&archive, "pmpx", tmp.path()).unwrap_err();
        assert!(
            error.to_string().contains("does not contain pmpx"),
            "{error}"
        );
    }

    #[test]
    fn something_that_is_not_an_archive_is_refused() {
        let tmp = tempfile::tempdir().unwrap();

        assert!(extract_from_tar_gz(b"not a tarball", "pmpx", tmp.path()).is_err());
        assert!(extract_from_zip(b"not a zip", "pmpx.exe", tmp.path()).is_err());
    }

    /// The destination is chosen by this code, never by the archive: whatever the entry is
    /// called, the binary lands in the directory the caller named.
    #[test]
    fn the_destination_does_not_follow_the_archive_entry_name() {
        let archive = tar_gz_with(&[("nested/dir/pmpx", b"somewhere else")]);
        let tmp = tempfile::tempdir().unwrap();

        let got = extract_from_tar_gz(&archive, "pmpx", tmp.path()).expect("should extract");

        assert_eq!(got.parent().unwrap(), tmp.path());
        assert_eq!(std::fs::read(&got).unwrap(), b"somewhere else");
    }

    // ---- the swap ---------------------------------------------------------

    #[test]
    fn replacing_puts_the_new_binary_in_place_of_the_old() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join(binary_name(std::env::consts::OS));
        let staged = append_to_name(&exe, ".new");

        std::fs::write(&exe, b"old").unwrap();
        std::fs::write(&staged, b"new").unwrap();

        replace(&exe, &staged).expect("should replace");

        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert!(!staged.exists(), "the staged file should have been moved");
    }

    /// The name the leftover gets: appended, not substituted, so `pmpx.exe` does not turn
    /// into `pmpx.old`.
    #[test]
    fn the_backup_name_keeps_the_extension() {
        assert_eq!(
            append_to_name(Path::new("/usr/local/bin/pmpx"), ".new"),
            PathBuf::from("/usr/local/bin/pmpx.new")
        );
        assert_eq!(
            append_to_name(Path::new(r"C:\bin\pmpx.exe"), ".old"),
            PathBuf::from(r"C:\bin\pmpx.exe.old")
        );
    }
}
