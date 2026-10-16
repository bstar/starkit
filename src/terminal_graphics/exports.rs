//! Bounded, opt-in file delivery to a frontend-owned directory. The remote
//! controller supplies only a basename; partial files disappear on disconnect.
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};
const CHUNK: usize = 64 * 1024;
const WINDOW: usize = 4;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToClient {
    Offer { id: u64, name: String, size: u64 },
    Chunk { id: u64, offset: u64, data: String },
    Finish { id: u64, sha256: String },
    Cancel { id: u64 },
}
impl ToClient {
    fn id(&self) -> u64 {
        match self {
            Self::Offer { id, .. }
            | Self::Chunk { id, .. }
            | Self::Finish { id, .. }
            | Self::Cancel { id } => *id,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToHost {
    Ready {
        id: u64,
    },
    Ack {
        id: u64,
        offset: u64,
    },
    Result {
        id: u64,
        path: Option<String>,
        error: Option<String>,
    },
}
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 240
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
}
pub struct Sender {
    id: u64,
    name: String,
    file: File,
    size: u64,
    sent: u64,
    acknowledged: u64,
    offered: bool,
    ready: bool,
    credit: usize,
    finished: bool,
    hash: Sha256,
    result: Option<std::result::Result<String, String>>,
}
impl Sender {
    pub fn open(id: u64, path: &Path, name: String) -> Result<Self> {
        ensure!(valid_name(&name), "Export needs a plain filename");
        let file = File::open(path)?;
        ensure!(file.metadata()?.is_file(), "Export source is not a file");
        let size = file.metadata()?.len();
        Ok(Self {
            id,
            name,
            file,
            size,
            sent: 0,
            acknowledged: 0,
            offered: false,
            ready: false,
            credit: 0,
            finished: false,
            hash: Sha256::new(),
            result: None,
        })
    }
    pub fn progress(&self) -> (u64, u64) {
        (self.acknowledged, self.size)
    }
    pub fn result(&mut self) -> Option<std::result::Result<String, String>> {
        self.result.take()
    }
    pub fn cancel(&mut self) -> ToClient {
        self.finished = true;
        self.ready = false;
        ToClient::Cancel { id: self.id }
    }
    pub fn receive(&mut self, message: ToHost) {
        match message {
            ToHost::Ready { id } if id == self.id && !self.ready => {
                self.ready = true;
                self.credit = WINDOW;
            }
            ToHost::Ack { id, offset }
                if id == self.id && offset > self.acknowledged && offset <= self.sent =>
            {
                self.acknowledged = offset;
                self.credit = (self.credit + 1).min(WINDOW);
            }
            ToHost::Result { id, path, error } if id == self.id => {
                self.result = Some(match (path, error) {
                    (Some(path), None) if self.finished => Ok(path),
                    (_, Some(error)) => Err(error),
                    _ => Err("Invalid export completion".into()),
                });
                self.ready = false;
                self.finished = true;
            }
            _ => {}
        }
    }
    pub fn next_message(&mut self) -> Result<Option<ToClient>> {
        if !self.offered {
            self.offered = true;
            return Ok(Some(ToClient::Offer {
                id: self.id,
                name: self.name.clone(),
                size: self.size,
            }));
        }
        if !self.ready || self.finished {
            return Ok(None);
        }
        if self.sent == self.size {
            if self.acknowledged != self.size {
                return Ok(None);
            }
            self.finished = true;
            return Ok(Some(ToClient::Finish {
                id: self.id,
                sha256: format!("{:x}", self.hash.clone().finalize()),
            }));
        }
        if self.credit == 0 {
            return Ok(None);
        }
        let mut data = vec![0; CHUNK.min((self.size - self.sent) as usize)];
        self.file
            .read_exact(&mut data)
            .context("Read exported video")?;
        let offset = self.sent;
        self.sent += data.len() as u64;
        self.credit -= 1;
        self.hash.update(&data);
        Ok(Some(ToClient::Chunk {
            id: self.id,
            offset,
            data: STANDARD.encode(data),
        }))
    }
}
struct Partial {
    id: u64,
    file: tempfile::NamedTempFile,
    path: PathBuf,
    size: u64,
    received: u64,
    hash: Sha256,
}
pub struct Receiver {
    directory: PathBuf,
    pending: Option<Partial>,
}
impl Receiver {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            pending: None,
        }
    }
    pub fn reset(&mut self) {
        self.pending = None;
    }
    pub fn receive(&mut self, message: ToClient) -> ToHost {
        let id = message.id();
        match self.apply(message) {
            Ok(reply) => reply,
            Err(error) => {
                self.pending = None;
                ToHost::Result {
                    id,
                    path: None,
                    error: Some(format!("{error:#}")),
                }
            }
        }
    }
    fn apply(&mut self, message: ToClient) -> Result<ToHost> {
        let id = message.id();
        match message {
            ToClient::Offer { name, size, .. } => {
                ensure!(valid_name(&name), "Invalid export filename");
                ensure!(self.pending.is_none(), "Another export is being received");
                std::fs::create_dir_all(&self.directory)?;
                let mut path = self.directory.join(&name);
                if path.exists() {
                    let original = Path::new(&name);
                    let stem = original
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("export");
                    let extension = original
                        .extension()
                        .and_then(|s| s.to_str())
                        .map(|s| format!(".{s}"))
                        .unwrap_or_default();
                    path = self.directory.join(format!("{stem}-{id:x}{extension}"));
                }
                let file = tempfile::Builder::new()
                    .prefix(".starscreen-export-")
                    .tempfile_in(&self.directory)?;
                self.pending = Some(Partial {
                    id,
                    file,
                    path,
                    size,
                    received: 0,
                    hash: Sha256::new(),
                });
                Ok(ToHost::Ready { id })
            }
            ToClient::Chunk { offset, data, .. } => {
                ensure!(
                    data.len() <= CHUNK.div_ceil(3) * 4,
                    "Oversized export chunk"
                );
                let pending = self.pending.as_mut().context("No export in progress")?;
                ensure!(
                    id == pending.id && offset == pending.received,
                    "Export chunk is out of order"
                );
                let bytes = STANDARD.decode(data)?;
                ensure!(
                    !bytes.is_empty()
                        && bytes.len() <= CHUNK
                        && offset + bytes.len() as u64 <= pending.size,
                    "Invalid export chunk size"
                );
                pending.file.write_all(&bytes)?;
                pending.hash.update(&bytes);
                pending.received += bytes.len() as u64;
                Ok(ToHost::Ack {
                    id,
                    offset: pending.received,
                })
            }
            ToClient::Finish { sha256, .. } => {
                let mut pending = self.pending.take().context("No export in progress")?;
                ensure!(
                    id == pending.id && pending.received == pending.size,
                    "Incomplete export"
                );
                ensure!(
                    sha256 == format!("{:x}", pending.hash.finalize()),
                    "Export checksum mismatch"
                );
                pending.file.flush()?;
                pending.file.as_file().sync_all()?;
                pending
                    .file
                    .persist_noclobber(&pending.path)
                    .map_err(|e| e.error)?;
                Ok(ToHost::Result {
                    id,
                    path: Some(pending.path.to_string_lossy().into()),
                    error: None,
                })
            }
            ToClient::Cancel { .. } => {
                if self.pending.as_ref().is_some_and(|p| p.id == id) {
                    self.reset();
                }
                bail!("Export transfer cancelled")
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_is_credit_bounded_and_never_clobbers_existing_video() {
        let host = tempfile::tempdir().unwrap();
        let client = tempfile::tempdir().unwrap();
        let bytes: Vec<u8> = (0..CHUNK * 7 + 37).map(|i| (i % 251) as u8).collect();
        let source = host.path().join("take.mp4");
        std::fs::write(&source, &bytes).unwrap();
        std::fs::write(client.path().join("take.mp4"), b"keep").unwrap();
        let mut tx = Sender::open(12, &source, "take.mp4".into()).unwrap();
        let mut rx = Receiver::new(client.path().into());
        let offer = tx.next_message().unwrap().unwrap();
        assert!(tx.next_message().unwrap().is_none());
        tx.receive(rx.receive(offer));
        let mut waiting = Vec::new();
        for _ in 0..WINDOW {
            waiting.push(tx.next_message().unwrap().unwrap());
        }
        assert!(tx.next_message().unwrap().is_none());
        for msg in waiting {
            tx.receive(rx.receive(msg));
        }
        for _ in 0..20 {
            if let Some(msg) = tx.next_message().unwrap() {
                tx.receive(rx.receive(msg));
            }
        }
        let result = tx.result().unwrap().unwrap();
        assert_eq!(std::fs::read(result).unwrap(), bytes);
        assert_eq!(
            std::fs::read(client.path().join("take.mp4")).unwrap(),
            b"keep"
        );
        assert!(source.exists());
    }
    #[test]
    fn malformed_chunks_checksums_and_disconnect_leave_no_partial_files() {
        for name in ["../take.mp4", "/tmp/file", "a\\b", "bad\nname", ""] {
            assert!(!valid_name(name));
        }
        let dir = tempfile::tempdir().unwrap();
        let mut rx = Receiver::new(dir.path().into());
        for failure in 0..3 {
            rx.receive(ToClient::Offer {
                id: 1,
                name: "take.mp4".into(),
                size: 1,
            });
            if failure == 0 {
                rx.receive(ToClient::Chunk {
                    id: 1,
                    offset: 10,
                    data: STANDARD.encode([1]),
                });
            } else if failure == 1 {
                rx.receive(ToClient::Chunk {
                    id: 1,
                    offset: 0,
                    data: STANDARD.encode([1]),
                });
                rx.receive(ToClient::Finish {
                    id: 1,
                    sha256: "bad".into(),
                });
            } else {
                rx.reset();
            }
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        }
    }
}
