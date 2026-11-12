//! Opt-in executable replacement for idle persistent controllers. The frontend
//! reconnects to the same session; no Closed message or new window is sent.
#[cfg(unix)]
use std::{
    os::unix::fs::MetadataExt,
    path::PathBuf,
    time::{Duration, Instant},
};
#[cfg(unix)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Identity(u64, u64);
#[cfg(unix)]
fn identity(path: &std::path::Path) -> std::io::Result<Identity> {
    let m = std::fs::metadata(path)?;
    Ok(Identity(m.dev(), m.ino()))
}
#[cfg(unix)]
pub(super) struct Probe {
    path: PathBuf,
    loaded: Identity,
    attempted: Option<Identity>,
    checked: Instant,
}
#[cfg(unix)]
impl Probe {
    pub fn new() -> Option<Self> {
        if !std::env::args_os().any(|a| a == "--serve") {
            return None;
        }
        let path = std::env::current_exe().ok()?;
        let loaded = identity(&path).ok()?;
        Some(Self {
            path,
            loaded,
            attempted: None,
            checked: Instant::now(),
        })
    }
    pub fn replacement(&mut self) -> Option<PathBuf> {
        if self.checked.elapsed() < Duration::from_secs(2) {
            return None;
        }
        self.checked = Instant::now();
        let found = identity(&self.path).ok()?;
        if found == self.loaded || self.attempted == Some(found) {
            return None;
        }
        self.attempted = Some(found);
        Some(self.path.clone())
    }
}
#[cfg(unix)]
pub(super) fn replace(path: &std::path::Path) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    std::process::Command::new(path)
        .args(std::env::args_os().skip(1))
        .exec()
}
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn executable_updates_are_identity_based_and_attempted_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app");
        std::fs::write(&path, b"old").unwrap();
        let mut probe = Probe {
            loaded: identity(&path).unwrap(),
            path: path.clone(),
            attempted: None,
            checked: Instant::now() - Duration::from_secs(3),
        };
        assert!(probe.replacement().is_none());
        let next = dir.path().join("next");
        std::fs::write(&next, b"new").unwrap();
        std::fs::rename(next, &path).unwrap();
        probe.checked = Instant::now() - Duration::from_secs(3);
        assert_eq!(probe.replacement(), Some(path));
        probe.checked = Instant::now() - Duration::from_secs(3);
        assert!(probe.replacement().is_none());
    }
}
