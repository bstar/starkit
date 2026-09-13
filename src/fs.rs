//! Writing a file so that an interrupted write cannot be read back.
//!
//! Everything either application saves while it is running -- the session, the
//! config, the credentials -- is written over its own previous version. A
//! plain `write` truncates first, so a crash, a full disk or a pulled power
//! cable between the truncate and the last byte leaves a file that parses as
//! far as it got and then loses whatever came after. That is worse than no
//! file at all, because nothing notices.
//!
//! A temporary file in the same directory, then a rename. `rename(2)` within a
//! filesystem is atomic: a reader sees either the old contents or the new ones
//! and never a mixture. The same directory matters -- across a mount point it
//! is a copy, not a rename, and the atomicity is gone.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Write `bytes` to `path`, through a temporary file in the same directory.
///
/// The parent directory is created if it is missing, because every caller was
/// doing that first anyway.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = prepare(path)?;
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

/// The same, for a file nobody else on the machine may read.
///
/// The mode is set when the temporary file is *created* rather than afterwards,
/// so the contents are never briefly world-readable -- which is the whole point
/// for a file holding a token. It is applied again to the destination because a
/// rename over an existing file keeps the source's mode, and an install that
/// predates this would otherwise keep whatever mode it was created with.
///
/// `sync_all` before the rename: a credential the user just entered and a
/// crash ten seconds later should not produce an empty file where a working
/// one used to be.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = prepare(path)?;

    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(bytes)
            .with_context(|| format!("writing {}", tmp.display()))?;
        f.sync_all()
            .with_context(|| format!("flushing {}", tmp.display()))?;
    }
    // Windows has no mode to set. The file still goes through the temporary
    // and the rename, so it is no less atomic -- only no less readable.
    #[cfg(not(unix))]
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;

    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("restricting {}", path.display()))?;
    }

    Ok(())
}

/// Make sure the directory exists, and name the temporary file.
///
/// Named for this process. A fixed name is shared by every instance writing
/// into the same directory, and two of them saving at once interleave into one
/// file which they then both rename over the destination.
fn prepare(path: &Path) -> Result<PathBuf> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let suffix = match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{ext}.{}", std::process::id()),
        None => format!("{}", std::process::id()),
    };
    Ok(path.with_extension(suffix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_write_replaces_what_was_there() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.toml");
        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
    }

    #[test]
    fn the_temporary_file_does_not_survive() {
        // A stray `session.toml.1234` beside the real one would be read by
        // nothing and cleaned up by nobody.
        let dir = tempfile::tempdir().unwrap();
        write_atomic(&dir.path().join("session.toml"), b"x").unwrap();
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["session.toml".to_string()]);
    }

    #[test]
    fn a_missing_parent_directory_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a").join("b").join("config.toml");
        write_atomic(&path, b"x").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"x");
    }

    #[test]
    fn a_file_with_no_extension_still_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        write_atomic(&path, b"x").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"x");
    }

    #[cfg(unix)]
    #[test]
    fn a_private_write_is_readable_by_nobody_else() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.toml");
        write_private(&path, b"token = \"secret\"").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "got {:o}", mode & 0o777);
    }

    #[cfg(unix)]
    #[test]
    fn a_private_write_tightens_a_file_that_was_already_loose() {
        // An install that predates this wrote the file at 0644 and would keep
        // that mode forever, because a rename carries the source's.
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.toml");
        std::fs::write(&path, b"old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        write_private(&path, b"new").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "got {:o}", mode & 0o777);
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }
}
