//! Getting the binary out of a release archive.
//!
//! Two shapes exist, one per platform family: a `.tar.gz` on Unix and a `.zip` on Windows.
//! Both put the binary and `LICENSE` at the archive root.
//!
//! The destination is chosen by this code, never by the archive: whatever an entry calls
//! itself, the binary lands where the caller asked for it.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context as _};

use super::release::binary_name;

/// The largest the extracted binary may be.
///
/// A release archive is a couple of megabytes; the *download* limit says nothing about what comes
/// out of a decompressor, so an archive cannot be allowed to decide how much lands in the install
/// directory.
const MAX_EXTRACTED_BYTES: u64 = 256 * 1024 * 1024;

/// Copy one archive member into a file this code creates, refusing anything absurdly large.
///
/// Both archive shapes go through here so that the limit, the flush and the error wording cannot
/// drift apart. `declared` is what the archive claims; the copy itself is limited as well, because
/// that claim is only the archive's word.
fn write_member<R: std::io::Read>(
    dest: &Path,
    declared: u64,
    reader: R,
    limit: u64,
) -> anyhow::Result<()> {
    if declared > limit {
        return Err(anyhow!(
            "the archive's {} is {declared} bytes, which is too large to be the pmpx binary",
            binary_name(std::env::consts::OS)
        ));
    }

    let mut out =
        fs::File::create(dest).with_context(|| format!("cannot write {}", dest.display()))?;

    // The reader is taken by value and limited: the declared size is the archive's word, not a
    // guarantee about how many bytes the decompressor will produce.
    let mut limited = reader.take(limit + 1);
    let copied = std::io::copy(&mut limited, &mut out).context("cannot read the archive entry")?;
    if copied > limit {
        drop(out);
        let _ = fs::remove_file(dest);
        return Err(anyhow!(
            "the entry for {} is larger than the {limit} byte limit",
            dest.display()
        ));
    }

    // The rename that installs this file must not be able to publish a half-written one: without
    // this, a crash can leave a truncated binary in place of a working pmpx.
    out.sync_all()
        .with_context(|| format!("cannot flush {}", dest.display()))?;

    Ok(())
}
/// Extracts the binary out of a release archive into `dir`, returning its path.
pub(super) fn extract(is_zip: bool, archive: &[u8], dir: &Path) -> anyhow::Result<PathBuf> {
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
pub(super) fn extract_from_tar_gz(
    archive: &[u8],
    member: &str,
    dir: &Path,
) -> anyhow::Result<PathBuf> {
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

        // The archive does not get to choose *what kind* of thing lands here. A symlink or hard
        // link entry named `pmpx` would become a link to somewhere else -- and the
        // `set_permissions` below would then chmod whatever it points at -- and a directory entry
        // would reach the swap as a directory. Writing the bytes into a file this code creates
        // makes the entry type irrelevant. (The zip branch below guards the same way.)
        if !entry.header().entry_type().is_file() {
            continue;
        }

        let dest = dir.join(member);
        write_member(
            &dest,
            entry.header().size().unwrap_or(0),
            &mut entry,
            MAX_EXTRACTED_BYTES,
        )?;
        return Ok(dest);
    }

    Err(anyhow!("the release archive does not contain {member}"))
}

/// The `pmpx.exe` member of a `.zip`.
pub(super) fn extract_from_zip(
    archive: &[u8],
    member: &str,
    dir: &Path,
) -> anyhow::Result<PathBuf> {
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
        write_member(&dest, file.size(), &mut file, MAX_EXTRACTED_BYTES)?;
        return Ok(dest);
    }

    Err(anyhow!("the release archive does not contain {member}"))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// A `.tar.gz` whose `pmpx` member is a **link** rather than a file.
    fn tar_gz_with_a_link_named(member: &str) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_link_name("/etc/passwd").unwrap();
        header.set_cksum();
        builder
            .append_data(&mut header, member, std::io::empty())
            .unwrap();
        let tar = builder.into_inner().unwrap();

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &tar).unwrap();
        encoder.finish().unwrap()
    }

    /// The archive does not get to choose the entry *type*: a link named like the binary must not
    /// be extracted, or the installation would become a link to somewhere else -- and the
    /// executable-bit chmod after extraction would follow it.
    #[test]
    fn a_link_entry_named_like_the_binary_is_refused() {
        let archive = tar_gz_with_a_link_named("pmpx");
        let tmp = tempfile::tempdir().unwrap();

        let error = extract_from_tar_gz(&archive, "pmpx", tmp.path()).unwrap_err();

        assert!(
            error.to_string().contains("does not contain pmpx"),
            "{error}"
        );
        assert!(
            !tmp.path().join("pmpx").exists(),
            "a link entry must not create anything at the destination"
        );
    }

    /// The same rule on the zip side, where the member is a directory.
    #[test]
    fn a_directory_entry_named_like_the_binary_is_refused() {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer.add_directory("pmpx.exe", options).unwrap();
        let archive = writer.finish().unwrap().into_inner();

        let tmp = tempfile::tempdir().unwrap();
        let error = extract_from_zip(&archive, "pmpx.exe", tmp.path()).unwrap_err();

        assert!(
            error.to_string().contains("does not contain pmpx.exe"),
            "{error}"
        );
        assert!(!tmp.path().join("pmpx.exe").is_file());
    }

    /// What the archive *claims* is checked first, before a byte is written.
    #[test]
    fn a_member_that_claims_to_be_enormous_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("pmpx");

        let error = write_member(&dest, 100, &b"short"[..], 16).unwrap_err();

        assert!(error.to_string().contains("too large"), "{error}");
        assert!(!dest.exists(), "nothing should have been created");
    }

    /// And the claim is not trusted: a member that lies about its size still stops at the limit,
    /// and the partial file is removed rather than left looking like an extracted binary.
    #[test]
    fn a_member_larger_than_it_claims_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("pmpx");
        let ten_bytes = b"0123456789";

        let error = write_member(&dest, 1, &ten_bytes[..], 4).unwrap_err();

        assert!(error.to_string().contains("larger than"), "{error}");
        assert!(!dest.exists(), "the partial file must not be left behind");
    }

    /// The ordinary case still works, through the same helper.
    #[test]
    fn a_member_within_the_limit_is_written() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("pmpx");

        write_member(&dest, 5, &b"hello"[..], 16).unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), b"hello");
    }
}
