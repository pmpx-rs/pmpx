//! `pmpx self update` -- replacing this binary with one from the project's releases.
//!
//! The design lives in `docs/proposal.md` §13. The rules that shape this module:
//!
//! - **Exactly one file is written**, and only after the download has been verified: the
//!   running executable. Everything else is a scratch file beside it.
//! - **A cargo installation does not self-update.** `cargo install` keeps its own record of
//!   what it installed; replacing the file behind its back would make `cargo install --list`
//!   disagree with reality. Those installations are told to run `cargo install pmpx --force`
//!   instead.
//! - **Nothing is checked automatically.** No background check, no "a new version is
//!   available" note on startup. The only thing that happens on startup is deleting the
//!   `.old` binary a previous update had to leave behind.
//! - **The new binary is staged in the same directory** as the old one, because the swap is
//!   a `rename` and that is only atomic within one filesystem.
//!
//! The work is split by what it talks to, because each half fails in its own way:
//!
//! | Module | Responsibility |
//! | --- | --- |
//! | [`release`] | where the release is, and whether the bytes are the right ones |
//! | [`checksum`] | the SHA-256 side of that question |
//! | [`archive`] | getting the binary out of a `.tar.gz` or a `.zip` |
//! | [`swap`] | putting it in place of the running binary |
//! | [`ledger`] | deciding whether cargo owns this binary (and so may not be replaced) |
//! | [`version`] | comparing versions |
//!
//! The asset names it fetches are produced by `.github/workflows/assets.yaml` and
//! `scripts/package-release.*`, which is where the other half of this contract lives.

use std::fs;
use std::path::Path;

use anyhow::{anyhow, Context as _};

use crate::error::{PmpxError, EXIT_OK};
use crate::style;

mod archive;
mod checksum;
mod ledger;
mod release;
mod swap;
mod version;

use self::archive::extract;
use self::checksum::{parse_sha256sums, sha256_hex};
use self::ledger::{install_kind, InstallKind};
use self::release::{archive_ext, asset_stem, download, latest_version, repository_parts};
use self::swap::{append_to_name, remove_dir_if_exists, replace, Scratch};
use self::version::Version;

pub use self::swap::cleanup_stale_old;

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
    let kind = install_kind(&exe);
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

// The update itself

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
