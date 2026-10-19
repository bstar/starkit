//! Private, non-attaching management for session observers and desktop controls.
//! Read requests use a cached snapshot; mutations run on the controller thread.
use anyhow::{bail, ensure, Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub const VERSION: u32 = 1;
const REQUEST_LIMIT: u64 = 32 * 1024;
const RESPONSE_LIMIT: usize = 1024 * 1024;

/// Application-specific operations. Session identity is checked by KIT first.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Apply { settings: Value },
    Command { action: String },
}
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Status {
        version: u32,
    },
    Apply {
        version: u32,
        epoch: String,
        settings: Value,
    },
    Command {
        version: u32,
        epoch: String,
        action: String,
    },
}
pub(crate) struct Pending {
    pub epoch: String,
    pub operation: Operation,
    pub reply: Sender<Value>,
}
pub(crate) struct Endpoint {
    pub requests: Receiver<Pending>,
    snapshot: Arc<Mutex<Value>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    path: PathBuf,
    published: Instant,
}
impl Endpoint {
    pub fn bind(scene_socket: &Path) -> Result<Self> {
        let path = scene_socket.with_extension("ctl");
        let root = path.parent().context("Management root missing")?;
        let metadata = fs::symlink_metadata(root)?;
        ensure!(
            metadata.is_dir()
                && metadata.uid() == rustix::process::geteuid().as_raw()
                && metadata.permissions().mode() & 0o077 == 0,
            "Management root must be owned by this user and mode 0700"
        );
        if path.exists() {
            ensure!(
                UnixStream::connect(&path).is_err(),
                "Management endpoint is already active"
            );
            let metadata = fs::symlink_metadata(&path)?;
            ensure!(
                metadata.file_type().is_socket()
                    && metadata.uid() == rustix::process::geteuid().as_raw(),
                "Refusing to replace an unrelated management path"
            );
            fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let snapshot = Arc::new(Mutex::new(
            json!({"ok":false,"error":"Session is starting","version":VERSION}),
        ));
        let (tx, requests) = bounded(8);
        let stopped = stop.clone();
        let shared = snapshot.clone();
        let worker = std::thread::Builder::new().name("star-session-management".into()).spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let result = respond(&mut stream, &shared, &tx, &stopped);
                        if let Err(error) = result {
                            let _ = write_response(&mut stream, &json!({"ok":false,"version":VERSION,"error":format!("{error:#}")}));
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
                    Err(_) => break,
                }
            }
        })?;
        Ok(Self {
            requests,
            snapshot,
            stop,
            worker: Some(worker),
            path,
            published: Instant::now() - Duration::from_secs(1),
        })
    }
    pub fn invalidate(&mut self) {
        self.published = Instant::now() - Duration::from_secs(1);
    }
    pub fn due(&self) -> bool {
        self.published.elapsed() >= Duration::from_millis(250)
    }
    pub fn publish(&mut self, value: Value) {
        *self.snapshot.lock().expect("management snapshot") = value;
        self.published = Instant::now();
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let _ = fs::remove_file(&self.path);
    }
}
fn write_response(stream: &mut UnixStream, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(
        bytes.len() <= RESPONSE_LIMIT,
        "Management response exceeds limit"
    );
    stream.write_all(&bytes)?;
    stream.write_all(b"\n")?;
    Ok(())
}
fn respond(
    stream: &mut UnixStream,
    snapshot: &Mutex<Value>,
    requests: &Sender<Pending>,
    stop: &AtomicBool,
) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    stream.set_write_timeout(Some(Duration::from_millis(250)))?;
    let mut bytes = vec![];
    BufReader::new(stream.try_clone()?)
        .take(REQUEST_LIMIT + 1)
        .read_until(b'\n', &mut bytes)?;
    ensure!(
        bytes.len() as u64 <= REQUEST_LIMIT && bytes.last() == Some(&b'\n'),
        "Management request exceeds limit or lacks newline"
    );
    let request: Request = serde_json::from_slice(&bytes)?;
    let (version, epoch, operation) = match request {
        Request::Status { version } => {
            ensure!(version == VERSION, "Unsupported management protocol");
            return write_response(stream, &snapshot.lock().expect("management snapshot"));
        }
        Request::Apply {
            version,
            epoch,
            settings,
        } => (version, epoch, Operation::Apply { settings }),
        Request::Command {
            version,
            epoch,
            action,
        } => {
            ensure!(action.len() <= 128, "Action exceeds limit");
            (version, epoch, Operation::Command { action })
        }
    };
    ensure!(version == VERSION, "Unsupported management protocol");
    ensure!(epoch.len() <= 128, "Invalid epoch");
    let (reply, result) = bounded(1);
    requests
        .try_send(Pending {
            epoch,
            operation,
            reply,
        })
        .context("Management command queue is full")?;
    let began = Instant::now();
    while !stop.load(Ordering::Acquire) && began.elapsed() < Duration::from_secs(2) {
        match result.recv_timeout(Duration::from_millis(20)) {
            Ok(value) => return write_response(stream, &value),
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                bail!("Controller stopped before acknowledging the command")
            }
            Err(_) => {}
        }
    }
    bail!("Controller did not acknowledge the command; refresh status before retrying")
}
