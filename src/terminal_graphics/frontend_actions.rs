//! Negotiated desktop actions on the attaching computer. Never execute a host
//! command, inherit terminal fds, or replay a request after reconnection.
use serde::{Deserialize, Serialize};
use std::{
    io,
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

const LIMIT: usize = 8;
pub const MAX_URL_BYTES: usize = 8192;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToClient {
    /// IDs are nonzero and increase within an attachment. No request is replayed.
    OpenUrl { id: u64, url: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Error {
    Unsupported,
    InvalidUrl,
    Busy,
    LaunchFailed,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "Opening links is unavailable in this terminal",
            Self::InvalidUrl => "Only HTTP and HTTPS links can be opened",
            Self::Busy => "The local browser opener is busy",
            Self::LaunchFailed => "Could not open the link on this computer",
        })
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToHost {
    /// Success means dispatch to the local opener, not proof of a loaded page.
    Result { id: u64, error: Option<Error> },
}

/// Validate at both ends of the transport. A URL becomes exactly one argv
/// element, with a fixed scheme; paths, shell commands and terminal controls
/// are never accepted. Unicode hosts use the URL parser's IDNA normalization.
pub fn web_url(raw: &str) -> Option<String> {
    if raw.is_empty() || raw.len() > MAX_URL_BYTES || raw.chars().any(char::is_control) {
        return None;
    }
    let parsed = url::Url::parse(raw).ok()?;
    (matches!(parsed.scheme(), "http" | "https")
        && parsed.has_host()
        && parsed.as_str().len() <= MAX_URL_BYTES)
        .then(|| parsed.into())
}

/// Host-side request tracking. Apps enqueue only in response to user actions,
/// drain through `Application::effects`, and show returned failures in their UI.
/// Disconnect clears queued work; IDs continue increasing across attachments.
#[derive(Default)]
pub struct Requests {
    available: bool,
    next: u64,
    pending: std::collections::BTreeSet<u64>,
    queued: std::collections::VecDeque<ToClient>,
}
impl Requests {
    pub fn available(&self) -> bool {
        self.available
    }
    pub fn set_available(&mut self, available: bool) {
        self.available = available;
        if !available {
            self.pending.clear();
            self.queued.clear();
        }
    }
    pub fn open_url(&mut self, raw: &str) -> Result<u64, Error> {
        if !self.available {
            return Err(Error::Unsupported);
        }
        let url = web_url(raw).ok_or(Error::InvalidUrl)?;
        if self.pending.len() >= LIMIT {
            return Err(Error::Busy);
        }
        let id = self.next.checked_add(1).ok_or(Error::Busy)?;
        self.next = id;
        self.pending.insert(id);
        self.queued.push_back(ToClient::OpenUrl { id, url });
        Ok(id)
    }
    pub fn drain(&mut self) -> impl Iterator<Item = ToClient> + '_ {
        self.queued.drain(..)
    }
    /// Ignore unknown/duplicate replies and replies cleared by a disconnect.
    pub fn receive(&mut self, reply: ToHost) -> Option<Result<(), Error>> {
        let ToHost::Result { id, error } = reply;
        if !self.pending.remove(&id) {
            return None;
        }
        if error == Some(Error::Unsupported) {
            self.set_available(false);
        }
        Some(match error {
            Some(error) => Err(error),
            None => Ok(()),
        })
    }
}

pub(super) fn available() -> bool {
    cfg!(target_os = "macos")
        || (cfg!(target_os = "linux")
            && ["DISPLAY", "WAYLAND_DISPLAY"]
                .iter()
                .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty())))
}

type Launcher = Arc<dyn Fn(&str) -> io::Result<Child> + Send + Sync>;
struct Launch {
    generation: u64,
    id: u64,
    worker: crate::worker::Worker<Child>,
}
struct Running {
    generation: u64,
    id: u64,
    child: Child,
    started: Instant,
    reported: bool,
}

pub(super) struct Frontend {
    enabled: bool,
    generation: u64,
    last: u64,
    launcher: Launcher,
    launches: Vec<Launch>,
    children: Vec<Running>,
}
impl Frontend {
    pub fn new(enabled: bool) -> Self {
        Self::with_launcher(
            enabled,
            Arc::new(|url| {
                Command::new(if cfg!(target_os = "macos") {
                    "open"
                } else {
                    "xdg-open"
                })
                .arg(url)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            }),
        )
    }
    fn with_launcher(enabled: bool, launcher: Launcher) -> Self {
        Self {
            enabled,
            generation: 1,
            last: 0,
            launcher,
            launches: vec![],
            children: vec![],
        }
    }
    pub fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.last = 0;
        for launch in &self.launches {
            launch.worker.cancel();
        }
        // Keep reaping already launched processes. Never close a user's browser.
    }
    pub fn receive(&mut self, request: ToClient) -> Option<ToHost> {
        let ToClient::OpenUrl { id, url } = request;
        if id == 0 || id <= self.last {
            return None;
        }
        self.last = id;
        let failure = |error| {
            Some(ToHost::Result {
                id,
                error: Some(error),
            })
        };
        if !self.enabled {
            return failure(Error::Unsupported);
        }
        let Some(url) = web_url(&url) else {
            return failure(Error::InvalidUrl);
        };
        if self.launches.len() + self.children.len() >= LIMIT {
            return failure(Error::Busy);
        }
        let launcher = self.launcher.clone();
        let worker = crate::worker::Worker::spawn("kit-url-opener", move |context| {
            if context.cancelled() {
                return Err("Cancelled".into());
            }
            launcher(&url).map_err(|error| {
                tracing::warn!(%error, "Local URL opener could not start");
                "URL opener failed".into()
            })
        });
        match worker {
            Ok(worker) => {
                self.launches.push(Launch {
                    generation: self.generation,
                    id,
                    worker,
                });
                None
            }
            Err(_) => failure(Error::LaunchFailed),
        }
    }
    pub fn poll(&mut self) -> Vec<ToHost> {
        let mut replies = vec![];
        self.launches.retain_mut(|launch| {
            match launch.worker.result() {
                None => return true,
                Some(Ok(child)) => self.children.push(Running {
                    generation: launch.generation,
                    id: launch.id,
                    child,
                    started: Instant::now(),
                    reported: false,
                }),
                Some(Err(_)) if launch.generation == self.generation => {
                    replies.push(ToHost::Result {
                        id: launch.id,
                        error: Some(Error::LaunchFailed),
                    })
                }
                _ => {}
            }
            false
        });
        self.children.retain_mut(|running| {
            let status = running.child.try_wait();
            let finished = !matches!(status, Ok(None));
            // Some openers remain alive for the browser's lifetime. A bounded
            // reaper retains them after dispatch, without blocking UI or quit.
            if !running.reported
                && (finished || running.started.elapsed() >= Duration::from_secs(2))
            {
                if running.generation == self.generation {
                    let error = match status {
                        Ok(Some(status)) if !status.success() => Some(Error::LaunchFailed),
                        Err(_) => Some(Error::LaunchFailed),
                        _ => None,
                    };
                    replies.push(ToHost::Result {
                        id: running.id,
                        error,
                    });
                }
                running.reported = true;
            }
            !finished
        });
        replies
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn request(id: u64, url: &str) -> ToClient {
        ToClient::OpenUrl {
            id,
            url: url.into(),
        }
    }
    fn reply(id: u64, error: Error) -> Option<ToHost> {
        Some(ToHost::Result {
            id,
            error: Some(error),
        })
    }
    #[test]
    fn host_requests_bound_pending_work_and_discard_stale_replies_and_effects() {
        let mut requests = Requests::default();
        assert_eq!(
            requests.open_url("https://example.org"),
            Err(Error::Unsupported)
        );
        requests.set_available(true);
        assert_eq!(
            requests.open_url("file:///etc/passwd"),
            Err(Error::InvalidUrl)
        );
        for id in 1..=LIMIT as u64 {
            assert_eq!(requests.open_url("https://example.org"), Ok(id));
        }
        assert_eq!(requests.open_url("https://example.org"), Err(Error::Busy));
        assert_eq!(requests.drain().count(), LIMIT);
        assert_eq!(
            requests.receive(ToHost::Result { id: 1, error: None }),
            Some(Ok(()))
        );
        assert_eq!(
            requests.receive(ToHost::Result { id: 1, error: None }),
            None
        );
        requests.set_available(false);
        assert_eq!(
            requests.receive(ToHost::Result { id: 2, error: None }),
            None
        );
        requests.set_available(true);
        assert_eq!(requests.open_url("https://example.org"), Ok(9));
        requests.set_available(false);
        assert_eq!(
            requests.drain().count(),
            0,
            "detach must not replay queued actions"
        );
        requests.set_available(true);
        let id = requests.open_url("https://example.org").unwrap();
        assert_eq!(
            requests.receive(ToHost::Result {
                id,
                error: Some(Error::Unsupported)
            }),
            Some(Err(Error::Unsupported))
        );
        assert!(!requests.available());
    }
    #[test]
    fn web_urls_preserve_single_argument_content_and_reject_non_web_targets() {
        let raw = "https://example.org/a?q=;$(echo+bad)#fragment";
        assert_eq!(web_url(raw).as_deref(), Some(raw));
        assert_eq!(
            web_url("https://日本語.jp/a b").as_deref(),
            Some("https://xn--wgv71a119e.jp/a%20b")
        );
        for raw in [
            "",
            "file:///etc/passwd",
            "/tmp/image.png",
            "javascript:alert(1)",
            "data:text/html,test",
            "mailto:a@example.org",
            "https://",
            "https://host/a\x1b[0m",
            "https://host/a\n",
        ] {
            assert!(web_url(raw).is_none(), "accepted {raw:?}");
        }
        assert!(web_url(&format!(
            "https://example.org/{}",
            "x".repeat(MAX_URL_BYTES)
        ))
        .is_none());
    }
    #[test]
    fn disabled_and_invalid_requests_never_reach_the_opener_or_replay_ids() {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let mut frontend = Frontend::with_launcher(
            false,
            Arc::new(move |_| {
                count.fetch_add(1, Ordering::SeqCst);
                Err(io::Error::other("must not be called"))
            }),
        );
        assert_eq!(
            frontend.receive(request(1, "https://example.org")),
            reply(1, Error::Unsupported)
        );
        frontend.enabled = true;
        assert!(frontend
            .receive(request(1, "https://example.org"))
            .is_none());
        assert!(frontend
            .receive(request(0, "https://example.org"))
            .is_none());
        assert_eq!(
            frontend.receive(request(2, "file:///etc/passwd")),
            reply(2, Error::InvalidUrl)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn launch_workers_are_bounded_nonblocking_and_reset_discards_old_replies() {
        let (release, gate) = crossbeam_channel::bounded::<()>(LIMIT);
        let mut frontend = Frontend::with_launcher(
            true,
            Arc::new(move |_| {
                gate.recv().unwrap();
                Err(io::Error::other("fixture failure"))
            }),
        );
        let start = Instant::now();
        for id in 1..=LIMIT as u64 {
            assert!(frontend
                .receive(request(id, "https://example.org"))
                .is_none());
        }
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "launching blocked UI"
        );
        assert_eq!(
            frontend.receive(request(9, "https://example.org")),
            reply(9, Error::Busy)
        );
        frontend.reset();
        for _ in 0..LIMIT {
            release.send(()).unwrap();
        }
        while !frontend.launches.is_empty() {
            assert!(frontend.poll().is_empty(), "old attachment reply leaked");
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::yield_now();
        }
        assert!(frontend
            .receive(request(1, "https://example.org"))
            .is_none());
        release.send(()).unwrap();
        loop {
            if let Some(message) = frontend.poll().pop() {
                assert_eq!(Some(message), reply(1, Error::LaunchFailed));
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::yield_now();
        }
    }
    #[cfg(unix)]
    #[test]
    fn opener_child_is_reaped_and_receives_the_exact_url() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let path = file.path().to_path_buf();
        let mut frontend = Frontend::with_launcher(
            true,
            Arc::new(move |url| {
                Command::new("sh")
                    .args(["-c", "printf '%s' \"$1\" > \"$2\"", "opener-fixture"])
                    .arg(url)
                    .arg(&path)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
            }),
        );
        let url = "https://example.org/?literal=;$(not-a-command)";
        frontend.receive(request(1, url));
        let start = Instant::now();
        loop {
            if let Some(message) = frontend.poll().pop() {
                assert_eq!(message, ToHost::Result { id: 1, error: None });
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::yield_now();
        }
        assert!(frontend.children.is_empty());
        assert_eq!(std::fs::read_to_string(file.path()).unwrap(), url);
    }
}
