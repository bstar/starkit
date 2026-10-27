//! Bounded, copy-only staging of regular files from Kitty's remote OSC 72 drops.
use anyhow::{ensure, Context, Result};
use base64::Engine as _;
use std::{
    collections::HashMap,
    fs::File,
    io::Write,
    path::{Component, PathBuf},
};
const MAX_BYTES: usize = 256 * 1024 * 1024;
pub fn machine_identity() -> Option<String> {
    super::drop_bridge::machine_id()
}
pub struct Incoming {
    stage: tempfile::TempDir,
    paths: Vec<PathBuf>,
    mime: u64,
    index: usize,
    file: Option<File>,
    bytes: usize,
}
pub enum Progress {
    Chunk,
    Next(String),
    Complete,
}
impl Incoming {
    pub fn new(uris: &str, mime: u64) -> Result<Self> {
        ensure!(
            uris.len() <= 1024 * 1024 && mime > 0,
            "Invalid remote drop metadata"
        );
        let stage = tempfile::Builder::new().prefix("starmux-drop-").tempdir()?;
        let mut paths = vec![];
        for uri in uris
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.starts_with('#'))
        {
            ensure!(paths.len() < 64, "Too many dropped files (maximum 64)");
            let uri = uri
                .strip_prefix("file://")
                .context("Drop is not a file URI")?;
            let raw = if uri.starts_with('/') {
                uri
            } else {
                &uri[uri.find('/').context("Invalid file URI")?..]
            };
            let mut bytes = Vec::with_capacity(raw.len());
            let encoded = raw.as_bytes();
            let mut index = 0;
            while index < encoded.len() {
                if encoded[index] == b'%' {
                    ensure!(index + 2 < encoded.len(), "Invalid URI escape");
                    bytes.push(u8::from_str_radix(
                        std::str::from_utf8(&encoded[index + 1..index + 3])?,
                        16,
                    )?);
                    index += 3;
                } else {
                    bytes.push(encoded[index]);
                    index += 1;
                }
            }
            ensure!(!bytes.contains(&0), "Invalid dropped filename");
            #[cfg(unix)]
            let path = {
                use std::os::unix::ffi::OsStringExt;
                PathBuf::from(std::ffi::OsString::from_vec(bytes))
            };
            #[cfg(not(unix))]
            let path = PathBuf::from(String::from_utf8(bytes)?);
            let name = path.file_name().context("Dropped file has no name")?;
            ensure!(
                path.components()
                    .all(|c| !matches!(c, Component::ParentDir)),
                "Invalid dropped file path"
            );
            let target = stage.path().join(name);
            ensure!(
                !paths.contains(&target),
                "Dropped files have duplicate names"
            );
            paths.push(target);
        }
        ensure!(!paths.is_empty(), "No dropped files");
        Ok(Self {
            stage,
            paths,
            mime,
            index: 0,
            file: None,
            bytes: 0,
        })
    }
    pub fn request(&self) -> String {
        format!("t=r:x={}:y={}:i=1", self.mime, self.index + 1)
    }
    pub fn receive(&mut self, raw: &str) -> Result<Progress> {
        ensure!(raw.len() <= 768 * 1024, "Drop chunk exceeds limit");
        let (header, payload) = raw.split_once(';').unwrap_or((raw, ""));
        let mut fields = HashMap::new();
        for field in header.split(':') {
            let (key, value) = field.split_once('=').context("Invalid drop message")?;
            ensure!(fields.insert(key, value).is_none(), "Duplicate drop field");
        }
        let number = |key| {
            fields
                .get(key)
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)
        };
        ensure!(
            fields.get("t") == Some(&"r")
                && number("x") == self.mime
                && number("y") == self.index as u64 + 1
                && number("Y") == 0,
            "Unexpected remote drop response"
        );
        ensure!(
            number("X") == 0,
            "Drop regular image/files directly; folders and symlinks are not supported"
        );
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(payload))?;
        ensure!(
            self.bytes.saturating_add(bytes.len()) <= MAX_BYTES,
            "Dropped files exceed 256 MiB"
        );
        if self.file.is_none() {
            let path = self
                .paths
                .get(self.index)
                .context("Drop already completed")?;
            self.file = Some(File::options().create_new(true).write(true).open(path)?);
        }
        self.file.as_mut().unwrap().write_all(&bytes)?;
        self.bytes += bytes.len();
        // A nonempty m=0 chunk still precedes Kitty's explicit empty terminator.
        if !payload.is_empty() || number("m") != 0 {
            return Ok(Progress::Chunk);
        }
        self.file = None;
        self.index += 1;
        if self.index == self.paths.len() {
            Ok(Progress::Complete)
        } else {
            Ok(Progress::Next(self.request()))
        }
    }
    pub fn into_files(self) -> (tempfile::TempDir, Vec<PathBuf>) {
        (self.stage, self.paths)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_bytes_are_staged_before_delivery_and_partial_files_are_removed() {
        let mut r = Incoming::new("file://mac/Users/test/screen%20shot.png\r\n", 2).unwrap();
        assert_eq!(r.request(), "t=r:x=2:y=1:i=1");
        assert!(r.receive("t=r:x=2:y=2;YQ").is_err());
        assert!(matches!(
            r.receive("t=r:x=2:y=1:X=0:m=0;iVBORw0KGgo=").unwrap(),
            Progress::Chunk
        ));
        assert!(matches!(
            r.receive("t=r:x=2:y=1:m=0;").unwrap(),
            Progress::Complete
        ));
        let (stage, paths) = r.into_files();
        assert_eq!(paths[0].file_name().unwrap(), "screen shot.png");
        assert_eq!(std::fs::read(&paths[0]).unwrap(), b"\x89PNG\r\n\x1a\n");
        let path = paths[0].clone();
        drop(stage);
        assert!(!path.exists());
        let mut r = Incoming::new("file:///tmp/partial.png", 1).unwrap();
        r.receive("t=r:x=1:y=1;YQ").unwrap();
        let path = r.paths[0].clone();
        drop(r);
        assert!(!path.exists());
    }
    #[test]
    fn refuses_traversal_links_duplicates_and_replayed_data() {
        assert!(Incoming::new("file:///tmp/../bad.png", 1).is_err());
        assert!(Incoming::new("file:///a/a.png\nfile:///b/a.png", 1).is_err());
        let mut r = Incoming::new("file:///tmp/a.png", 1).unwrap();
        assert!(r.receive("t=r:x=1:y=1:X=1;YQ").is_err());
        r.receive("t=r:x=1:y=1;").unwrap();
        assert!(r.receive("t=r:x=1:y=1;YQ").is_err());
    }
}
