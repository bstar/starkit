//! Where an application keeps its files.
//!
//! Everything lives under one directory -- `~/.local/<app>` by default --
//! rather than being scattered across the three XDG roots. Config, databases,
//! themes and cache in one place means the whole of a setup can be backed up,
//! moved between machines, or deleted by moving one folder.
//!
//! `$<APP>_DIR` overrides the location entirely. `$<APP>_CONFIG_DIR` is
//! honoured as well, for anyone who wants the config somewhere else.
//!
//! The application names its own variables, because `starcord` reading
//! `STARAMP_DIR` would be a surprise nobody asked for:
//!
//! ```
//! use starkit::paths::Paths;
//! pub const PATHS: Paths = Paths::new("staramp", "STARAMP_DIR", "STARAMP_CONFIG_DIR");
//! ```
//!
//! Only the locations both applications share are here. A path that means
//! something to one of them -- an index, a playlist directory, a control
//! socket -- is built from [`Paths::data_dir`] or [`Paths::runtime_dir`] in the
//! application that has it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// One application's directories.
///
/// Const-constructible so it can be a `const` beside the rest of an
/// application's constants rather than something passed down from `main`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Paths {
    app: &'static str,
    dir_env: &'static str,
    config_dir_env: &'static str,
}

impl Paths {
    pub const fn new(
        app: &'static str,
        dir_env: &'static str,
        config_dir_env: &'static str,
    ) -> Self {
        Self {
            app,
            dir_env,
            config_dir_env,
        }
    }

    /// The application's own name, as it appears in file names and log targets.
    pub const fn app(&self) -> &'static str {
        self.app
    }

    /// The one directory everything hangs off.
    pub fn base_dir(&self) -> Result<PathBuf> {
        base_from(std::env::var_os(self.dir_env), home_dir(), self.app)
            .context("cannot determine the home directory")
    }

    /// Config lives at the base, unless pointed elsewhere.
    pub fn config_dir(&self) -> Result<PathBuf> {
        if let Some(dir) = std::env::var_os(self.config_dir_env) {
            return Ok(PathBuf::from(dir));
        }
        self.base_dir()
    }

    pub fn config_file(&self) -> Result<PathBuf> {
        Ok(self.config_dir()?.join("config.toml"))
    }

    /// Databases and anything else that must not be lost.
    pub fn data_dir(&self) -> Result<PathBuf> {
        self.base_dir()
    }

    /// Thumbnails and logs. Safe to delete.
    pub fn cache_dir(&self) -> Result<PathBuf> {
        Ok(self.base_dir()?.join("cache"))
    }

    /// Downloaded pictures: avatars, emoji, attachments, cover art.
    ///
    /// A directory of its own under the cache rather than the cache itself, so
    /// that a user clearing space, or a support answer saying to, can name the
    /// bytes that came off somebody else's server without taking the log and
    /// whatever else lives there with them.
    pub fn media_cache_dir(&self) -> Result<PathBuf> {
        Ok(self.cache_dir()?.join("media"))
    }

    pub fn log_dir(&self) -> Result<PathBuf> {
        self.cache_dir()
    }

    pub fn themes_dir(&self) -> Result<PathBuf> {
        Ok(self.config_dir()?.join("themes"))
    }

    /// Tokens and provider credentials, kept out of the ordinary config so
    /// that pasting a config file into a bug report cannot leak one.
    pub fn credentials_file(&self) -> Result<PathBuf> {
        Ok(self.data_dir()?.join("credentials.toml"))
    }

    /// Where the application was when it was last closed: the last thing
    /// open, unsent drafts, scroll positions.
    ///
    /// Data rather than config, because nobody typed it, and next to the
    /// credentials rather than in the cache, because losing it loses work. It
    /// is not a secret and it is not harmless either -- it names every channel
    /// or playlist the user reads -- so it goes in the directory that is 0700
    /// rather than the one a bug report gets pasted from.
    pub fn session_file(&self) -> Result<PathBuf> {
        Ok(self.data_dir()?.join("session.toml"))
    }

    /// Volatile per-user state: sockets, and nothing that should survive a
    /// reboot.
    ///
    /// `$XDG_RUNTIME_DIR` where there is one, which on Linux is a tmpfs the
    /// session owns. macOS has no such variable, so the cache directory stands
    /// in; it is on disk rather than in memory, which for a socket that is
    /// recreated every run costs nothing.
    pub fn runtime_dir(&self) -> Result<PathBuf> {
        let dir = match std::env::var_os("XDG_RUNTIME_DIR") {
            Some(d) => PathBuf::from(d).join(self.app),
            None => self.cache_dir()?.join("run"),
        };
        // 0700: the XDG runtime directory is already private, but the cache
        // fallback is not.
        own_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(dir)
    }

    /// Create the directories the application keeps its own files in,
    /// privately.
    ///
    /// Called once at startup, before anything opens a database or a log.
    pub fn init_private_dirs(&self) {
        for dir in [self.base_dir(), self.config_dir(), self.cache_dir()]
            .into_iter()
            .flatten()
        {
            if let Err(e) = own_dir(&dir) {
                tracing::debug!("could not prepare {}: {e}", dir.display());
            }
        }
    }

    /// Legacy XDG locations, checked once so an existing install is not
    /// orphaned.
    fn legacy_locations(&self) -> Vec<(PathBuf, PathBuf)> {
        let Some(home) = home_dir() else {
            return Vec::new();
        };
        let old_config = home.join(".config").join(self.app);
        let old_data = home.join(".local").join("share").join(self.app);
        let Ok(base) = self.base_dir() else {
            return Vec::new();
        };
        vec![(old_config, base.clone()), (old_data, base)]
    }

    /// Move anything left in the old XDG directories into the single base dir.
    ///
    /// Runs once at startup and is a no-op afterwards. Silent when there is
    /// nothing to do; existing files at the destination are never overwritten,
    /// so a repeat run cannot clobber newer state.
    pub fn migrate_legacy(&self) -> Vec<String> {
        let mut moved = Vec::new();
        let Ok(base) = self.base_dir() else {
            return moved;
        };

        for (old, new) in self.legacy_locations() {
            if !old.is_dir() || old == new {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&old) else {
                continue;
            };
            if std::fs::create_dir_all(&new).is_err() {
                continue;
            }
            for entry in entries.flatten() {
                let from = entry.path();
                let Some(name) = from.file_name() else {
                    continue;
                };
                let to = new.join(name);
                if to.exists() {
                    continue;
                }
                // Rename first; fall back to a copy when the two are on
                // different filesystems.
                let ok = std::fs::rename(&from, &to).is_ok() || copy_recursive(&from, &to).is_ok();
                if ok {
                    moved.push(format!("{} -> {}", from.display(), to.display()));
                }
            }
            // Only remove the old directory if it emptied out.
            let _ = std::fs::remove_dir(&old);
        }

        let _ = std::fs::create_dir_all(&base);
        moved
    }
}

/// Make a directory the application owns readable by nobody else.
///
/// `create_dir_all` takes the umask, which on most systems means 0755, and what
/// lives under here is not a matter of taste: a library index lists every path
/// on the disk, a message cache is a transcript of private conversations, and
/// the log names whatever is open. On a shared machine that is all readable by
/// every other account.
///
/// Applied to a directory that already exists as well as to a new one, because
/// installs that predate this ran with the old mode and would otherwise keep it
/// forever. Best-effort by design: a directory that cannot be chmodded -- one
/// on a filesystem with no Unix modes, most likely -- is not a reason to refuse
/// to start.
pub fn own_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// The base directory as a pure function of what the environment said.
///
/// Split out so the rule can be tested without setting a variable in a process
/// that other tests are reading from at the same time.
fn base_from(dir_env: Option<OsString>, home: Option<PathBuf>, app: &str) -> Option<PathBuf> {
    if let Some(dir) = dir_env {
        return Some(PathBuf::from(dir));
    }
    Some(home?.join(".local").join(app))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    // `is_dir` and `copy` both follow a link, and `remove_file` then deletes
    // the link rather than what it pointed at. Migrating an old install is a
    // small blast radius, but not a different rule.
    if from.symlink_metadata()?.file_type().is_symlink() {
        return Err(std::io::Error::other(format!(
            "refusing to follow symlink {}",
            from.display()
        )));
    }
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        std::fs::remove_dir(from)?;
    } else {
        std::fs::copy(from, to)?;
        std::fs::remove_file(from)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: Paths = Paths::new("testapp", "TESTAPP_DIR", "TESTAPP_CONFIG_DIR");

    #[test]
    fn the_default_is_one_directory_under_dot_local() {
        let home = PathBuf::from("/home/someone");
        let base = base_from(None, Some(home.clone()), "testapp").unwrap();
        assert_eq!(base, home.join(".local").join("testapp"));
        // Not under .config, which is where the XDG split would have put it and
        // where an earlier version did.
        assert!(!base.to_string_lossy().contains("/.config/"));
    }

    #[test]
    fn the_env_override_relocates_everything_together() {
        let base = base_from(Some(OsString::from("/srv/app")), None, "testapp").unwrap();
        assert_eq!(base, PathBuf::from("/srv/app"));
    }

    #[test]
    fn with_no_home_and_no_override_there_is_no_base() {
        assert_eq!(base_from(None, None, "testapp"), None);
    }

    #[test]
    fn everything_hangs_off_the_base() {
        // Guarded rather than asserted unconditionally: the variables are a
        // legitimate override and the test must not fail because one is set.
        if std::env::var_os("TESTAPP_DIR").is_some()
            || std::env::var_os("TESTAPP_CONFIG_DIR").is_some()
            || home_dir().is_none()
        {
            return;
        }
        let base = P.base_dir().unwrap();
        assert!(base.ends_with("testapp"));
        assert!(P.config_file().unwrap().starts_with(&base));
        assert!(P.themes_dir().unwrap().starts_with(&base));
        assert!(P.cache_dir().unwrap().starts_with(&base));
        assert!(P.log_dir().unwrap().starts_with(&base));
        assert!(P.credentials_file().unwrap().starts_with(&base));
        assert!(P.session_file().unwrap().starts_with(&base));
        assert!(P.media_cache_dir().unwrap().starts_with(&base));
    }

    /// Where the two new ones land, exactly, because both applications had
    /// written the same two paths out by hand before this and a shared one
    /// that pointed somewhere else would orphan what is already on disk.
    #[test]
    fn the_session_is_kept_with_the_data_and_the_pictures_with_the_cache() {
        if home_dir().is_none() {
            return;
        }
        assert_eq!(
            P.session_file().unwrap(),
            P.data_dir().unwrap().join("session.toml")
        );
        assert_eq!(
            P.media_cache_dir().unwrap(),
            P.cache_dir().unwrap().join("media")
        );
        // Not in the cache: clearing space must not lose a draft.
        let session = P.session_file().unwrap();
        assert!(!session.starts_with(P.cache_dir().unwrap()));
    }

    #[test]
    fn the_log_directory_is_the_cache_directory() {
        // Logs are rebuildable in the only sense that matters: deleting the
        // cache must not take anything with it that cannot be regenerated.
        if home_dir().is_none() {
            return;
        }
        assert_eq!(P.log_dir().unwrap(), P.cache_dir().unwrap());
    }

    #[test]
    fn a_directory_this_owns_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("private");
        own_dir(&sub).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&sub).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "got {:o}", mode & 0o777);
        }
    }

    #[cfg(unix)]
    #[test]
    fn migration_does_not_follow_a_symlink_out_of_the_old_directory() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside.txt");
        std::fs::write(&outside, b"not yours").unwrap();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&outside, &link).unwrap();

        assert!(copy_recursive(&link, &dir.path().join("copy.txt")).is_err());
        assert!(
            outside.is_file(),
            "the link was followed and its target moved"
        );
    }
}
