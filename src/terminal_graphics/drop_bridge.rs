//! Local Linux file capabilities for graphical SSH drops. The OS gesture ends
//! after its URI list is captured; a bounded worker serves only those paths.
//! No remote request can supply a path, and Move cleanup is acknowledged.
use base64::Engine as _;
use crossbeam_channel::{bounded, Receiver, Sender};
use std::collections::HashMap;
use std::io;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

const META_LIMIT: usize = 8 * 1024 * 1024;
const CHUNK: usize = 48 * 1024;

#[derive(Clone, Debug)]
struct Message<'a> {
    fields: HashMap<&'a str, &'a str>,
    payload: &'a str,
}
impl<'a> Message<'a> {
    fn parse(raw: &'a str) -> Option<Self> {
        if raw.len() > META_LIMIT * 2 {
            return None;
        }
        let (header, payload) = raw.split_once(';').unwrap_or((raw, ""));
        let mut fields = HashMap::new();
        for part in header.split(':') {
            let (k, v) = part.split_once('=')?;
            if k.is_empty() || v.contains(['\x1b', '\x07']) || fields.insert(k, v).is_some() {
                return None;
            }
        }
        Some(Self { fields, payload })
    }
    fn get(&self, k: &str) -> Option<&str> {
        self.fields.get(k).copied()
    }
    fn n(&self, k: &str) -> u64 {
        self.get(k).and_then(|v| v.parse().ok()).unwrap_or(0)
    }
}

pub(super) struct Output {
    pub epoch: u64,
    pub event: Event,
}
pub(super) enum Event {
    Ready { client: u64 },
    Wire(String),
}
#[derive(Clone)]
struct OutputSender {
    tx: Sender<Output>,
    epoch: u64,
}
struct Pending {
    mime: u64,
    client: u64,
    allowed: u64,
    bytes: Vec<u8>,
    remote: bool,
    own_source: bool,
}
struct Transfer {
    requests: Sender<String>,
    cancelled: Arc<AtomicBool>,
    finishing: bool,
}
impl Drop for Transfer {
    fn drop(&mut self) {
        if !self.finishing {
            self.cancelled.store(true, Ordering::Release);
        }
    }
}

pub(super) struct Bridge {
    identity: Option<String>,
    offered_source: bool,
    source_finished: bool,
    pending: Option<Pending>,
    transfer: Option<Transfer>,
    responses: Receiver<Output>,
    output: OutputSender,
}
impl Bridge {
    pub fn new(ssh: bool) -> Self {
        let (output, responses) = bounded(8);
        Self {
            identity: ssh.then(machine_id).flatten(),
            offered_source: false,
            source_finished: false,
            pending: None,
            transfer: None,
            output: OutputSender {
                tx: output,
                epoch: 0,
            },
            responses,
        }
    }
    pub fn identity(&self, meta: &str) -> Option<&str> {
        let m = Message::parse(meta)?;
        (m.get("t") == Some("a") && m.n("x") == 1)
            .then_some(self.identity.as_deref())
            .flatten()
    }
    /// Consume remote requests only after an authentic local URI drop.
    pub fn request(&mut self, meta: &str) -> io::Result<bool> {
        let Some(m) = Message::parse(meta) else {
            return Ok(false);
        };
        if m.get("t") == Some("o") && m.get("x").is_none() {
            self.offered_source = true;
            self.source_finished = false;
        }
        if m.get("t") != Some("r") {
            return Ok(false);
        }
        if m.n("x") == 0 && m.n("y") == 0 && m.n("Y") == 0 {
            self.pending = None;
        }
        if let Some(t) = self.transfer.as_mut() {
            if m.n("x") == 0 && m.n("y") == 0 && m.n("Y") == 0 && m.n("o") == 0 {
                self.reset();
                return Ok(true);
            }
            t.requests.try_send(meta.to_owned()).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Local drop request queue is full",
                )
            })?;
            if m.n("x") == 0 && m.n("y") == 0 && m.n("Y") == 0 {
                t.finishing = true;
            }
            return Ok(true);
        }
        Ok(false)
    }
    /// Return false when the terminal reply is buffered instead of forwarded.
    pub fn terminal(&mut self, text: &str) -> io::Result<bool> {
        if self.identity.is_none() {
            return Ok(true);
        }
        let Some(m) = Message::parse(text) else {
            return Ok(true);
        };
        if m.get("t") == Some("e") && m.n("x") == 4 {
            // Kitty can cancel a same-window source just before emitting M.
            // Keep provenance until that drop, or a fresh drag enters.
            self.source_finished = true;
        }
        if m.get("t") == Some("m") && self.source_finished && !m.payload.is_empty() {
            self.offered_source = false;
            self.source_finished = false;
        }
        if m.get("t") == Some("M") {
            self.reset();
            self.pending = m
                .payload
                .split_whitespace()
                .position(|mime| mime == "text/uri-list")
                .map(|i| Pending {
                    mime: i as u64 + 1,
                    client: m.n("i"),
                    allowed: m.n("o"),
                    bytes: Vec::new(),
                    remote: false,
                    own_source: self.offered_source,
                });
            self.offered_source = false;
            self.source_finished = false;
            return Ok(true);
        }
        let Some(p) = &mut self.pending else {
            return Ok(true);
        };
        if m.get("t") != Some("r")
            || m.n("x") != p.mime
            || m.n("y") != 0
            || m.n("Y") != 0
            || m.n("i") != p.client
        {
            return Ok(true);
        }
        p.remote |= m.n("X") == 1;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(m.payload)
            .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(m.payload))
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid dropped URI list"))?;
        if p.bytes.len().saturating_add(bytes.len()) > META_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Dropped URI list too large",
            ));
        }
        p.bytes.extend(bytes);
        if !m.payload.is_empty() || m.n("m") != 0 {
            return Ok(false);
        }
        let p = self.pending.take().unwrap();
        #[cfg(target_os = "linux")]
        if !p.remote && !p.own_source {
            if let Ok(paths) = local_paths(&p.bytes) {
                if !paths.iter().any(|path| {
                    path.components()
                        .any(|c| c.as_os_str().to_string_lossy().starts_with("dnd-drag-"))
                }) {
                    let (requests, rx) = bounded(8);
                    let cancelled = Arc::new(AtomicBool::new(false));
                    let cancel = Arc::clone(&cancelled);
                    let output = self.output.clone();
                    std::thread::spawn(move || worker::run(paths, p, rx, output, cancel));
                    self.transfer = Some(Transfer {
                        requests,
                        cancelled,
                        finishing: false,
                    });
                    return Ok(false);
                }
            }
        }
        // Remote terminals/promises keep the original Kitty transport. Even
        // when the client ID is local, the application host is still remote.
        let output = self.output.clone();
        std::thread::spawn(move || {
            let _ = emit_data(
                &output,
                &format!("t=r:x={}:X=1:i={}", p.mime, p.client),
                &p.bytes,
                &AtomicBool::new(false),
            );
        });
        Ok(false)
    }
    pub fn responses(&self) -> &Receiver<Output> {
        &self.responses
    }
    pub fn reset(&mut self) {
        self.transfer = None;
        self.pending = None;
        self.output.epoch += 1;
    }
    pub fn current(&self, epoch: u64) -> bool {
        epoch == self.output.epoch
    }
}

fn emit(output: &OutputSender, item: Event, cancel: &AtomicBool) -> io::Result<()> {
    let mut item = Output {
        epoch: output.epoch,
        event: item,
    };
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "Drop cancelled"));
        }
        match output
            .tx
            .send_timeout(item, std::time::Duration::from_millis(20))
        {
            Ok(()) => return Ok(()),
            Err(crossbeam_channel::SendTimeoutError::Timeout(old)) => item = old,
            Err(_) => return Err(io::Error::new(io::ErrorKind::BrokenPipe, "Drop detached")),
        }
    }
}
fn emit_data(
    output: &OutputSender,
    meta: &str,
    data: &[u8],
    cancel: &AtomicBool,
) -> io::Result<()> {
    for bytes in data.chunks(CHUNK) {
        emit(
            output,
            Event::Wire(format!(
                "{meta}:m=1;{}",
                base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes)
            )),
            cancel,
        )?;
    }
    emit(output, Event::Wire(format!("{meta}:m=0;")), cancel)
}
#[cfg(target_os = "linux")]
fn machine_id() -> Option<String> {
    use sha2::{Digest, Sha256};
    let raw = std::fs::read_to_string("/etc/machine-id").ok()?;
    let key = b"tty-dnd-protocol-machine-id";
    let mut inner = [0x36u8; 64];
    let mut outer = [0x5cu8; 64];
    for (i, b) in key.iter().enumerate() {
        inner[i] ^= b;
        outer[i] ^= b;
    }
    let mut h = Sha256::new();
    h.update(inner);
    h.update(raw.trim().as_bytes());
    let digest = h.finalize();
    let mut h = Sha256::new();
    h.update(outer);
    h.update(digest);
    Some(format!(
        "1:{}",
        h.finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    ))
}
#[cfg(not(target_os = "linux"))]
fn machine_id() -> Option<String> {
    None
}

#[cfg(target_os = "linux")]
fn local_paths(bytes: &[u8]) -> io::Result<Vec<PathBuf>> {
    use std::os::unix::ffi::OsStringExt;
    let text = std::str::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid URI text"))?;
    let mut paths = Vec::new();
    for uri in text
        .lines()
        .map(str::trim_end)
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
    {
        let value = uri
            .strip_prefix("file://")
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Unsupported drop URI"))?;
        let path = value.strip_prefix("localhost").unwrap_or(value);
        if !path.starts_with('/') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Non-local URI authority",
            ));
        }
        let mut out = Vec::new();
        let mut i = 0;
        let raw = path.as_bytes();
        while i < raw.len() {
            if raw[i] == b'%' {
                let a = raw.get(i + 1).and_then(|b| (*b as char).to_digit(16));
                let b = raw.get(i + 2).and_then(|b| (*b as char).to_digit(16));
                let (Some(a), Some(b)) = (a, b) else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Invalid URI escape",
                    ));
                };
                out.push((a * 16 + b) as u8);
                i += 3;
            } else {
                out.push(raw[i]);
                i += 1;
            }
        }
        if out.contains(&0) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "NUL in URI"));
        }
        let path = PathBuf::from(std::ffi::OsString::from_vec(out));
        if path.file_name().is_none()
            || path
                .components()
                .any(|p| matches!(p, std::path::Component::ParentDir))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unsafe drop root",
            ));
        }
        paths.push(path);
    }
    // Validate overlapping roots without quadratic work on marked sets;
    // preserve original URI order because transfer requests use those indices.
    let mut sorted = paths.iter().collect::<Vec<_>>();
    sorted.sort_unstable();
    if sorted.windows(2).any(|pair| pair[1].starts_with(pair[0])) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Overlapping drop roots",
        ));
    }
    if paths.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Empty drop"));
    }
    Ok(paths)
}

#[cfg(target_os = "linux")]
mod worker {
    use super::*;
    use rustix::fd::OwnedFd;
    use rustix::fs::{AtFlags, FileType, Mode, OFlags};
    use std::ffi::OsString;
    use std::fs::{self, File};
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    struct Node {
        parent: Arc<OwnedFd>,
        name: OsString,
        fd: Option<Arc<OwnedFd>>,
        stat: rustix::fs::Stat,
        children: Option<Vec<usize>>,
        served: bool,
        depth: usize,
    }
    impl Node {
        fn capture(parent: Arc<OwnedFd>, name: OsString, depth: usize) -> io::Result<Self> {
            let stat = rustix::fs::statat(&*parent, &name, AtFlags::SYMLINK_NOFOLLOW)?;
            if !matches!(
                FileType::from_raw_mode(stat.st_mode),
                FileType::RegularFile | FileType::Directory | FileType::Symlink
            ) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Drop contains a special file",
                ));
            }
            Ok(Self {
                parent,
                name,
                fd: None,
                stat,
                children: None,
                served: false,
                depth,
            })
        }
        fn unchanged(&self) -> io::Result<()> {
            let current = rustix::fs::statat(&*self.parent, &self.name, AtFlags::SYMLINK_NOFOLLOW)?;
            if current.st_dev != self.stat.st_dev
                || current.st_ino != self.stat.st_ino
                || current.st_mode != self.stat.st_mode
                || current.st_mtime != self.stat.st_mtime
                || current.st_mtime_nsec != self.stat.st_mtime_nsec
                || current.st_ctime != self.stat.st_ctime
                || current.st_ctime_nsec != self.stat.st_ctime_nsec
                || current.st_size != self.stat.st_size
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Source changed: {}", self.name.to_string_lossy()),
                ));
            }
            Ok(())
        }
    }
    struct Files {
        nodes: Vec<Node>,
        roots: Vec<usize>,
        handles: HashMap<u64, usize>,
        next_handle: u64,
    }
    impl Files {
        fn capture(paths: &[PathBuf]) -> io::Result<Self> {
            let mut nodes: Vec<Node> = Vec::new();
            let mut parents = HashMap::new();
            let mut identities = std::collections::HashSet::new();
            for path in paths {
                let parent_path = path.parent().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "Missing source parent")
                })?;
                let parent = if let Some(parent) = parents.get(parent_path) {
                    Arc::clone(parent)
                } else {
                    let parent = Arc::new(rustix::fs::open(
                        parent_path,
                        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
                        Mode::empty(),
                    )?);
                    parents.insert(parent_path.to_owned(), Arc::clone(&parent));
                    parent
                };
                let node = Node::capture(parent, path.file_name().unwrap().to_owned(), 0)?;
                if !identities.insert((node.stat.st_dev, node.stat.st_ino)) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Aliased drop roots",
                    ));
                }
                nodes.push(node);
            }
            Ok(Self {
                roots: (0..nodes.len()).collect(),
                nodes,
                handles: HashMap::new(),
                next_handle: 2,
            })
        }
        fn read(
            &mut self,
            request: &Message<'_>,
            mime: u64,
            client: u64,
            out: &OutputSender,
            cancel: &AtomicBool,
        ) -> io::Result<()> {
            if request.n("i") != client {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Wrong drop client",
                ));
            }
            let (index, meta) = if request.n("Y") != 0 {
                let handle = request.n("Y");
                let parent = *self.handles.get(&handle).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "Unknown drop directory handle")
                })?;
                if request.n("x") == 0 {
                    self.handles.remove(&handle);
                    return Ok(());
                }
                let id = request
                    .n("x")
                    .checked_sub(1)
                    .and_then(|i| self.nodes[parent].children.as_ref()?.get(i as usize))
                    .copied()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Unknown drop entry"))?;
                (
                    id,
                    format!("t=r:Y={handle}:x={}:i={client}", request.n("x")),
                )
            } else {
                if request.n("x") != mime {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "Wrong drop MIME index",
                    ));
                }
                let id = request
                    .n("y")
                    .checked_sub(1)
                    .and_then(|i| self.roots.get(i as usize))
                    .copied()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Unknown drop root"))?;
                (id, format!("t=r:x={mime}:y={}:i={client}", request.n("y")))
            };
            if self.nodes[index].served {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Repeated drop entry request",
                ));
            }
            self.nodes[index].unchanged()?;
            if self.nodes[index].fd.is_none() {
                let node = &self.nodes[index];
                let fd = rustix::fs::openat(
                    &*node.parent,
                    &node.name,
                    OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?;
                let actual = rustix::fs::fstat(&fd)?;
                if actual.st_dev != node.stat.st_dev || actual.st_ino != node.stat.st_ino {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Drop source was replaced",
                    ));
                }
                self.nodes[index].fd = Some(Arc::new(fd));
            }
            let node = &self.nodes[index];
            let fd = node.fd.as_ref().unwrap();
            match FileType::from_raw_mode(node.stat.st_mode) {
                FileType::RegularFile => {
                    let mut file = File::open(format!("/proc/self/fd/{}", fd.as_raw_fd()))?;
                    let mut buffer = vec![0u8; CHUNK];
                    loop {
                        if cancel.load(Ordering::Acquire) {
                            return Err(io::Error::new(
                                io::ErrorKind::Interrupted,
                                "Drop cancelled",
                            ));
                        }
                        let n = file.read(&mut buffer)?;
                        if n == 0 {
                            break;
                        }
                        emit(
                            out,
                            Event::Wire(format!(
                                "{meta}:X=0:m=1;{}",
                                base64::engine::general_purpose::STANDARD_NO_PAD
                                    .encode(&buffer[..n])
                            )),
                            cancel,
                        )?;
                    }
                    node.unchanged()?;
                    emit(out, Event::Wire(format!("{meta}:X=0:m=0;")), cancel)?;
                }
                FileType::Symlink => {
                    let target = rustix::fs::readlinkat(&**fd, "", Vec::new())?;
                    emit_data(out, &format!("{meta}:X=1"), target.as_bytes(), cancel)?;
                }
                FileType::Directory => {
                    if node.depth >= 256 {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Drop tree too deep",
                        ));
                    }
                    let mut names = fs::read_dir(format!("/proc/self/fd/{}", fd.as_raw_fd()))?
                        .map(|e| e.map(|e| e.file_name()))
                        .collect::<io::Result<Vec<_>>>()?;
                    names.sort();
                    let mut data = Vec::new();
                    let mut children = Vec::new();
                    let parent = Arc::clone(fd);
                    let depth = node.depth + 1;
                    for name in names {
                        if cancel.load(Ordering::Acquire) {
                            return Err(io::Error::new(
                                io::ErrorKind::Interrupted,
                                "Drop cancelled",
                            ));
                        }
                        if data.len().saturating_add(name.as_bytes().len() + 1) > META_LIMIT
                            || self.nodes.len() >= 100_000
                        {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "Drop directory too large",
                            ));
                        }
                        data.extend_from_slice(name.as_bytes());
                        data.push(0);
                        let child = Node::capture(Arc::clone(&parent), name, depth)?;
                        children.push(self.nodes.len());
                        self.nodes.push(child);
                    }
                    self.nodes[index].unchanged()?;
                    self.nodes[index].children = Some(children);
                    let handle = self.next_handle;
                    self.next_handle += 1;
                    self.handles.insert(handle, index);
                    emit_data(out, &format!("{meta}:X={handle}"), &data, cancel)?;
                }
                _ => unreachable!(),
            }
            self.nodes[index].served = true;
            if FileType::from_raw_mode(self.nodes[index].stat.st_mode) != FileType::Directory {
                self.nodes[index].fd = None;
            }
            Ok(())
        }
        fn remove_sources(&self) -> io::Result<()> {
            // Validate the entire sent manifest before deleting anything. A
            // replaced file, unrequested entry or new child keeps its source.
            for node in &self.nodes {
                if !node.served {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Move source was not fully transferred",
                    ));
                }
                node.unchanged()?;
            }
            for node in self.nodes.iter().rev() {
                let flags = if FileType::from_raw_mode(node.stat.st_mode) == FileType::Directory {
                    AtFlags::REMOVEDIR
                } else {
                    AtFlags::empty()
                };
                // Directory timestamps change as our own children are removed;
                // retain the device/inode check at the actual unlink boundary.
                let now = rustix::fs::statat(&*node.parent, &node.name, AtFlags::SYMLINK_NOFOLLOW)?;
                if now.st_dev != node.stat.st_dev || now.st_ino != node.stat.st_ino {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Move source was replaced",
                    ));
                }
                rustix::fs::unlinkat(&*node.parent, &node.name, flags)?;
            }
            Ok(())
        }
    }
    fn error(
        out: &OutputSender,
        client: u64,
        reason: impl std::fmt::Display,
        cancel: &AtomicBool,
        completion: Option<u64>,
    ) {
        let reason = reason.to_string();
        tracing::warn!(%reason, client, operation = ?completion, "Local SSH drop failed");
        let description: String = reason
            .chars()
            .filter(|c| !c.is_control())
            .take(2048)
            .collect();
        let meta = completion.map_or_else(
            || format!("t=R:i={client}"),
            |op| format!("t=L:C={op}:o=0:i={client}"),
        );
        let _ = emit(
            out,
            Event::Wire(format!("{meta};EIO:{description}")),
            cancel,
        );
    }
    pub(super) fn run(
        paths: Vec<PathBuf>,
        pending: Pending,
        requests: Receiver<String>,
        output: OutputSender,
        cancel: Arc<AtomicBool>,
    ) {
        if emit(
            &output,
            Event::Ready {
                client: pending.client,
            },
            &cancel,
        )
        .is_err()
        {
            return;
        }
        if emit_data(
            &output,
            &format!("t=r:x={}:X=1:B=1:i={}", pending.mime, pending.client),
            &pending.bytes,
            &cancel,
        )
        .is_err()
        {
            return;
        }
        let mut files = match Files::capture(&paths) {
            Ok(files) => files,
            Err(e) => {
                error(&output, pending.client, e, &cancel, None);
                return;
            }
        };
        for raw in requests {
            if cancel.load(Ordering::Acquire) {
                return;
            }
            let Some(request) = Message::parse(&raw) else {
                error(
                    &output,
                    pending.client,
                    "Invalid drop request",
                    &cancel,
                    None,
                );
                return;
            };
            let completion = request.n("x") == 0 && request.n("y") == 0 && request.n("Y") == 0;
            if completion {
                let op = request.n("o");
                if op == 2 {
                    if pending.allowed & 2 == 0 {
                        error(
                            &output,
                            pending.client,
                            "Source did not permit Move",
                            &cancel,
                            Some(request.n("C")),
                        );
                        return;
                    }
                    if let Err(e) = files.remove_sources() {
                        error(&output, pending.client, e, &cancel, Some(request.n("C")));
                        return;
                    }
                }
                let _ = emit(
                    &output,
                    Event::Wire(format!(
                        "t=L:C={}:o=1:i={};",
                        request.n("C"),
                        pending.client
                    )),
                    &cancel,
                );
                return;
            }
            if let Err(e) = files.read(&request, pending.mime, pending.client, &output, &cancel) {
                error(&output, pending.client, e, &cancel, None);
                return;
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn replaced_or_modified_sources_survive_move_cleanup() {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("file");
            fs::write(&path, b"old").unwrap();
            let mut files = Files::capture(std::slice::from_ref(&path)).unwrap();
            files.nodes[0].served = true;
            fs::remove_file(&path).unwrap();
            fs::write(&path, b"new").unwrap();
            assert!(files.remove_sources().is_err());
            assert_eq!(fs::read(&path).unwrap(), b"new");
            let mut files = Files::capture(std::slice::from_ref(&path)).unwrap();
            files.nodes[0].served = true;
            fs::write(&path, b"changed size").unwrap();
            assert!(files.remove_sources().is_err());
            assert!(path.exists());
        }
        #[test]
        fn unread_sources_survive_forged_move_completion() {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("file");
            fs::write(&path, b"safe").unwrap();
            let files = Files::capture(std::slice::from_ref(&path)).unwrap();
            assert!(files.remove_sources().is_err());
            assert!(path.exists());
        }
        #[test]
        fn nested_directory_handles_deliver_all_entries_before_move_removal() {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("tree");
            fs::create_dir_all(root.join("nested")).unwrap();
            fs::write(root.join("nested/file"), b"bytes").unwrap();
            std::os::unix::fs::symlink("nested/file", root.join("link")).unwrap();
            let mut files = Files::capture(std::slice::from_ref(&root)).unwrap();
            let (tx, rx) = bounded(32);
            let output = OutputSender { tx, epoch: 0 };
            let cancel = AtomicBool::new(false);
            let mut todo = std::collections::VecDeque::from(["t=r:x=1:y=1:i=1".to_string()]);
            let mut delivered = Vec::new();
            while let Some(request) = todo.pop_front() {
                files
                    .read(&Message::parse(&request).unwrap(), 1, 1, &output, &cancel)
                    .unwrap();
                let mut bytes = Vec::new();
                let kind;
                loop {
                    let Event::Wire(raw) = rx.recv().unwrap().event else {
                        panic!()
                    };
                    let m = Message::parse(&raw).unwrap();
                    if m.payload.is_empty() {
                        kind = m.n("X");
                        break;
                    }
                    bytes.extend(
                        base64::engine::general_purpose::STANDARD_NO_PAD
                            .decode(m.payload)
                            .unwrap(),
                    );
                }
                if kind >= 2 {
                    for (i, _) in bytes
                        .split(|b| *b == 0)
                        .filter(|name| !name.is_empty())
                        .enumerate()
                    {
                        todo.push_back(format!("t=r:Y={kind}:x={}:i=1", i + 1));
                    }
                } else {
                    delivered.push(bytes);
                }
            }
            assert!(delivered.contains(&b"bytes".to_vec()));
            assert!(delivered.contains(&b"nested/file".to_vec()));
            files.remove_sources().unwrap();
            assert!(!root.exists());
        }
        #[test]
        fn symlink_drop_reads_target_text_without_following_it() {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("link");
            std::os::unix::fs::symlink("/etc/shadow", &path).unwrap();
            let mut files = Files::capture(&[path]).unwrap();
            let (tx, rx) = bounded(8);
            let tx = OutputSender { tx, epoch: 0 };
            files
                .read(
                    &Message::parse("t=r:x=1:y=1:i=1").unwrap(),
                    1,
                    1,
                    &tx,
                    &AtomicBool::new(false),
                )
                .unwrap();
            let Event::Wire(text) = rx.recv().unwrap().event else {
                panic!()
            };
            assert_eq!(
                base64::engine::general_purpose::STANDARD_NO_PAD
                    .decode(Message::parse(&text).unwrap().payload)
                    .unwrap(),
                b"/etc/shadow"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_rejects_duplicate_fields() {
        assert!(Message::parse("t=r:x=1:x=2").is_none());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn uris_preserve_non_utf8_and_reject_other_hosts_or_traversal() {
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(
            local_paths(b"file://localhost/tmp/a%FF%20b\r\n").unwrap()[0]
                .as_os_str()
                .as_bytes(),
            b"/tmp/a\xff b"
        );
        for uri in [
            "file://evil/tmp/file",
            "file:///tmp/../etc/shadow",
            "file:///",
            "file:///tmp/%00",
            "file:///tmp/%G0",
        ] {
            assert!(local_paths(uri.as_bytes()).is_err(), "{uri}");
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn ssh_drop_releases_desktop_before_data_and_removes_only_after_move() {
        for operation in [0, 1, 2] {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("file");
            std::fs::write(&source, vec![0x5au8; 100_000]).unwrap();
            let mut bridge = Bridge::new(true);
            bridge.identity = Some("test".into());
            assert!(bridge
                .terminal("t=M:x=5:y=5:o=3:i=1;text/uri-list")
                .unwrap());
            let uri = format!("file://{}\r\n", source.display());
            let raw = format!(
                "t=r:x=1:i=1:m=1;{}",
                base64::engine::general_purpose::STANDARD_NO_PAD.encode(uri)
            );
            assert!(!bridge.terminal(&raw).unwrap());
            assert!(!bridge.terminal("t=r:x=1:i=1:m=0;").unwrap());
            let ready = bridge
                .responses
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            assert!(matches!(ready.event, Event::Ready { client: 1 }));
            let metadata = bridge.responses.recv().unwrap();
            let Event::Wire(metadata) = metadata.event else {
                panic!()
            };
            assert!(metadata.starts_with("t=r:x=1:X=1:B=1:i=1"));
            assert!(matches!(
                bridge.responses.recv().unwrap().event,
                Event::Wire(_)
            ));
            assert!(source.exists(), "metadata is not permission to delete");
            if operation != 0 {
                assert!(bridge.request("t=r:x=1:y=1:i=1").unwrap());
                let mut bytes = Vec::new();
                loop {
                    let Event::Wire(raw) = bridge.responses.recv().unwrap().event else {
                        panic!()
                    };
                    let m = Message::parse(&raw).unwrap();
                    if m.payload.is_empty() {
                        break;
                    }
                    bytes.extend(
                        base64::engine::general_purpose::STANDARD_NO_PAD
                            .decode(m.payload)
                            .unwrap(),
                    );
                }
                assert_eq!(bytes, vec![0x5au8; 100_000]);
                assert!(source.exists(), "sending data must not delete");
            }
            assert!(bridge
                .request(&format!("t=r:o={operation}:C=42:i=1"))
                .unwrap());
            if operation != 0 {
                let Event::Wire(ack) = bridge
                    .responses
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap()
                    .event
                else {
                    panic!()
                };
                assert_eq!(ack, "t=L:C=42:o=1:i=1;");
            }
            assert_eq!(source.exists(), operation != 2);
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn new_drop_discards_old_queued_file_chunks() {
        let mut bridge = Bridge::new(true);
        bridge.identity = Some("test".into());
        bridge.terminal("t=M:o=3:i=1;text/uri-list").unwrap();
        let old = bridge.output.epoch;
        bridge.terminal("t=M:o=3:i=1;text/uri-list").unwrap();
        assert!(!bridge.current(old));
        assert!(bridge.current(bridge.output.epoch));
    }
    #[test]
    fn same_window_source_never_authorizes_local_file_access() {
        let mut bridge = Bridge::new(true);
        bridge.identity = Some("test".into());
        assert!(!bridge.request("t=o:o=1:i=1").unwrap());
        bridge.terminal("t=e:x=4:y=1:i=1").unwrap();
        bridge.terminal("t=M:o=3:i=1;text/uri-list").unwrap();
        let bytes =
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(b"file:///etc/shadow\r\n");
        bridge
            .terminal(&format!("t=r:x=1:i=1:m=1;{bytes}"))
            .unwrap();
        bridge.terminal("t=r:x=1:i=1:m=0;").unwrap();
        let response = bridge
            .responses
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(
            matches!(response.event, Event::Wire(_)),
            "self drops must use Kitty's guarded transfer"
        );
        assert!(bridge.transfer.is_none());
    }
    #[test]
    fn a_new_external_drag_clears_cancelled_source_provenance() {
        let mut bridge = Bridge::new(true);
        bridge.identity = Some("test".into());
        bridge.request("t=o:o=1:i=1").unwrap();
        bridge.terminal("t=e:x=4:y=1:i=1").unwrap();
        bridge.terminal("t=m:x=-1:y=-1:i=1").unwrap();
        assert!(bridge.offered_source);
        bridge.terminal("t=m:x=4:y=4:i=1;text/uri-list").unwrap();
        bridge.terminal("t=M:o=3:i=1;text/uri-list").unwrap();
        assert!(!bridge.pending.as_ref().unwrap().own_source);
    }
    proptest::proptest! {
        #[test]
        fn foreign_metadata_never_panics(raw in ".{0,4096}") { let _ = Message::parse(&raw); }
        #[cfg(target_os = "linux")]
        #[test]
        fn foreign_uris_never_panics(raw in proptest::collection::vec(proptest::prelude::any::<u8>(),0..4096)) { let _ = local_paths(&raw); }
    }
}
