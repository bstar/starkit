//! Local management for HTTP/HLS services. No bearer tokens enter snapshots.
use crate::session_control::{Endpoint, Operation, VERSION};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Settings {
    paused: bool,
    bandwidth_kbps: u32,
    max_transfers: u32,
    hls_enabled: bool,
    originals_enabled: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            paused: false,
            bandwidth_kbps: 0,
            max_transfers: 8,
            hls_enabled: true,
            originals_enabled: true,
        }
    }
}
impl Settings {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.bandwidth_kbps == 0 || (256..=1_000_000).contains(&self.bandwidth_kbps),
            "Bandwidth must be 0 (unlimited) or 256–1000000 kbps"
        );
        ensure!(
            (1..=8).contains(&self.max_transfers),
            "Concurrent transfers must be between 1 and 8"
        );
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Patch {
    expected_revision: Option<u64>,
    expected_settings: Option<Value>,
    transport: Settings,
}
struct Inner {
    settings: Settings,
    revision: u64,
    active: BTreeMap<u64, Value>,
    events: Vec<Value>,
    next_send: Instant,
    movies: BTreeMap<String, Value>,
}
pub(super) struct State {
    inner: Mutex<Inner>,
    next: AtomicU64,
    cancelled: AtomicU64,
    pub bytes: AtomicU64,
    pub requests: AtomicU64,
    pub failures: AtomicU64,
    began: Instant,
    epoch: String,
    port: u16,
}
impl State {
    pub fn new(port: u16) -> Self {
        Self {
            inner: Mutex::new(Inner {
                settings: Settings::default(),
                revision: 0,
                active: BTreeMap::new(),
                events: vec![],
                next_send: Instant::now(),
                movies: BTreeMap::new(),
            }),
            next: AtomicU64::new(1),
            cancelled: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            requests: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            began: Instant::now(),
            epoch: format!(
                "{}-{port}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ),
            port,
        }
    }
    pub fn begin(self: &Arc<Self>, peer: String) -> Option<Transfer> {
        let mut inner = self.inner.lock().ok()?;
        if inner.active.len() >= inner.settings.max_transfers as usize {
            return None;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        inner.active.insert(id,json!({"id":id,"peer":peer,"state":"receiving request","uptime_seconds":self.began.elapsed().as_secs_f64()}));
        self.requests.fetch_add(1, Ordering::Relaxed);
        Some(Transfer {
            state: self.clone(),
            id,
            epoch: self.cancelled.load(Ordering::Acquire),
        })
    }
    pub fn allowed(&self, hls: bool) -> bool {
        let inner = self.inner.lock().unwrap();
        if hls {
            inner.settings.hls_enabled
        } else {
            inner.settings.originals_enabled
        }
    }
    pub fn flow(&self, bytes: usize, stop: &AtomicBool, generation: u64) -> bool {
        loop {
            if stop.load(Ordering::Acquire) || self.cancelled.load(Ordering::Acquire) != generation
            {
                return false;
            }
            if !self.inner.lock().unwrap().settings.paused {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let until = {
            let mut inner = self.inner.lock().unwrap();
            if inner.settings.bandwidth_kbps == 0 {
                Instant::now()
            } else {
                let duration = Duration::from_secs_f64(
                    bytes as f64 * 8. / (f64::from(inner.settings.bandwidth_kbps) * 1000.),
                );
                let until = inner.next_send.max(Instant::now());
                inner.next_send = until + duration;
                until
            }
        };
        while Instant::now() < until {
            if stop.load(Ordering::Acquire) || self.cancelled.load(Ordering::Acquire) != generation
            {
                return false;
            }
            std::thread::sleep(
                until
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(20)),
            );
        }
        true
    }
    fn settings(&self) -> Value {
        json!({"transport":self.inner.lock().unwrap().settings})
    }
    pub fn snapshot(&self, hls: Value) -> Value {
        let inner = self.inner.lock().unwrap();
        let active = inner.active.values().cloned().collect::<Vec<_>>();
        let now = self.began.elapsed().as_secs_f64();
        let movies = inner
            .movies
            .values()
            .filter_map(|movie| {
                let age = now - movie["last_seen"].as_f64().unwrap_or(0.);
                let transfers = active
                    .iter()
                    .filter(|t| t["movie_id"] == movie["id"])
                    .count();
                if age > 90. && transfers == 0 {
                    return None;
                }
                let mut movie = movie.clone();
                movie["last_activity_seconds"] = json!(age);
                movie["active_transfers"] = json!(transfers);
                Some(movie)
            })
            .collect::<Vec<_>>();
        json!({"ok":true,"version":VERSION,"epoch":self.epoch,"session":format!("http-{}",self.port),"pid":std::process::id(),"client":format!("{} active HTTP transfers",active.len()),"connected":!active.is_empty(),"connection_count":active.len(),"transport":"http / hls",
            "metrics":{"uptime_seconds":self.began.elapsed().as_secs_f64(),"requests":self.requests.load(Ordering::Relaxed),"video_bytes":self.bytes.load(Ordering::Relaxed),"errors":self.failures.load(Ordering::Relaxed),"events":inner.events},
            "application":{"kind":"media-service","name":"STAR media service","project":"HTTP/HLS streaming service","status":format!("localhost:{} · HTTP byte ranges and adaptive HLS",self.port),"controls_supported":true,"revision":inner.revision,"settings":{"transport":inner.settings},"transfers":active,"movies":movies,"hls":hls,
                "schema":[{"title":"Live transport","fields":[
                    {"key":"transport.paused","label":"Pause outgoing media","type":"boolean"},
                    {"key":"transport.bandwidth_kbps","label":"Total bandwidth kbps · 0 = unlimited","type":"integer","min":0,"max":1000000,"step":1000},
                    {"key":"transport.max_transfers","label":"Maximum concurrent transfers","type":"integer","min":1,"max":8,"step":1},
                    {"key":"transport.hls_enabled","label":"Allow adaptive HLS requests","type":"boolean"},
                    {"key":"transport.originals_enabled","label":"Allow original-file requests","type":"boolean"}
                ]}],"actions":[{"id":"disconnect","label":"Disconnect active transfers","enabled":!active.is_empty()},{"id":"clear-cache","label":"Clear idle segment caches","enabled":true}]}})
    }
    fn manage(&self, operation: Operation, clear: impl FnOnce() -> Value) -> Result<Value> {
        match operation {
            Operation::Apply { settings } => {
                let patch: Patch = serde_json::from_value(settings)?;
                patch.transport.validate()?;
                let baseline = self.settings();
                let mut inner = self.inner.lock().unwrap();
                ensure!(
                    patch.expected_revision.is_none_or(|r| r == inner.revision),
                    "Settings changed; reload before applying"
                );
                ensure!(
                    patch.expected_settings.is_none_or(|s| s == baseline),
                    "Settings changed; reload before applying"
                );
                inner.settings = patch.transport;
                inner.revision += 1;
                inner.next_send = Instant::now();
            }
            Operation::Command { action } => match action.as_str() {
                "disconnect" => {
                    self.cancelled.fetch_add(1, Ordering::Release);
                }
                "clear-cache" => return Ok(clear()),
                _ => anyhow::bail!("Unknown media-service control"),
            },
        }
        Ok(Value::Null)
    }
}
pub(super) struct Transfer {
    state: Arc<State>,
    id: u64,
    pub epoch: u64,
}
impl Transfer {
    pub fn movie(
        &self,
        id: &str,
        details: &Value,
        mode: &str,
        rendition: &str,
        range: Option<&str>,
    ) {
        let mut inner = self.state.inner.lock().unwrap();
        if let Some(active) = inner.active.get_mut(&self.id) {
            active["movie_id"] = json!(id);
            active["mode"] = json!(mode);
        }
        let now = self.state.began.elapsed().as_secs_f64();
        if !inner.movies.contains_key(id) && inner.movies.len() >= 32 {
            let oldest = inner
                .movies
                .iter()
                .min_by(|a, b| {
                    a.1["last_seen"]
                        .as_f64()
                        .unwrap_or(0.)
                        .total_cmp(&b.1["last_seen"].as_f64().unwrap_or(0.))
                })
                .map(|(id, _)| id.clone());
            if let Some(id) = oldest {
                inner.movies.remove(&id);
            }
        }
        let movie = inner.movies.entry(id.to_string()).or_insert_with(
            || json!({"id":id,"details":details,"requests":0,"bytes":0,"started":now}),
        );
        movie["requests"] = json!(movie["requests"].as_u64().unwrap_or(0) + 1);
        movie["last_seen"] = json!(now);
        movie["mode"] = json!(mode);
        if rendition != "master.m3u8" {
            movie["rendition"] = json!(rendition);
        }
        movie["last_range"] = json!(range);
    }
    pub fn sent(&self, bytes: u64) {
        self.state.bytes.fetch_add(bytes, Ordering::Relaxed);
        let mut inner = self.state.inner.lock().unwrap();
        let movie_id = inner
            .active
            .get(&self.id)
            .and_then(|t| t["movie_id"].as_str())
            .map(String::from);
        if let Some(id) = movie_id {
            if let Some(movie) = inner.movies.get_mut(&id) {
                movie["bytes"] = json!(movie["bytes"].as_u64().unwrap_or(0) + bytes);
                movie["last_seen"] = json!(self.state.began.elapsed().as_secs_f64());
            }
        }
    }
    pub fn describe(&self, route: &str) {
        if let Some(value) = self.state.inner.lock().unwrap().active.get_mut(&self.id) {
            value["state"] = json!(route.chars().take(160).collect::<String>());
        }
    }
}
impl Drop for Transfer {
    fn drop(&mut self) {
        self.state.inner.lock().unwrap().active.remove(&self.id);
    }
}
pub(super) struct Management {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Management {
    pub fn start(state: Arc<State>, hls: Arc<super::hls::Hls>) -> Result<Self> {
        use std::os::unix::fs::PermissionsExt;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::env::temp_dir()
                    .join(format!("starkit-{}", rustix::process::geteuid().as_raw()))
            });
        let root = runtime.join("starkit-streaming");
        if !root.exists() {
            std::fs::create_dir_all(&root)?;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
        }
        let mut endpoint =
            Endpoint::bind(&root.join(format!("http-{}-{}.sock", std::process::id(), state.port)))?;
        endpoint.publish(state.snapshot(hls.diagnostics()));
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker=std::thread::Builder::new().name("star-http-management".into()).spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                for request in endpoint.requests.try_iter().take(8).collect::<Vec<_>>() {
                    let result=(||->Result<Value>{
                        ensure!(request.epoch==state.epoch,"Session changed; refresh before applying");
                        state.manage(request.operation,||hls.clear_idle())?;
                        Ok(state.snapshot(hls.diagnostics())["application"].clone())
                    })();
                    let response=match result {
                        Ok(value)=>json!({"ok":true,"version":VERSION,"epoch":state.epoch,"result":value}),
                        Err(error)=>json!({"ok":false,"version":VERSION,"error":format!("{error:#}")}),
                    };
                    let mut inner=state.inner.lock().unwrap();
                    inner.events.push(json!({"kind":if response["ok"]==true {"control"}else{"error"},"detail":if response["ok"]==true {"Media-service control applied"}else{"Media-service control rejected"},"uptime_seconds":state.began.elapsed().as_secs_f64()}));
                    if inner.events.len()>32 {inner.events.remove(0);}
                    drop(inner);
                    endpoint.publish(state.snapshot(hls.diagnostics()));
                    let _=request.reply.try_send(response);
                }
                if endpoint.due(){endpoint.publish(state.snapshot(hls.diagnostics()));}
                std::thread::sleep(Duration::from_millis(20));
            }
        }).context("Start HTTP management")?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}
impl Drop for Management {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transfer_lifecycle_limits_and_stale_controls_are_real() {
        let state = Arc::new(State::new(1234));
        let first = state.begin("127.0.0.1:1000".into()).unwrap();
        first.describe("/v1/media/opaque-id");
        assert_eq!(state.snapshot(Value::Null)["connection_count"], 1);
        let mut settings = Settings {
            max_transfers: 1,
            bandwidth_kbps: 4000,
            ..Default::default()
        };
        state
            .manage(
                Operation::Apply {
                    settings: json!({"expected_revision":0,"transport":settings}),
                },
                || Value::Null,
            )
            .unwrap();
        assert!(state.begin("second".into()).is_none());
        assert!(state
            .manage(
                Operation::Apply {
                    settings: json!({"expected_revision":0,"transport":settings})
                },
                || Value::Null
            )
            .is_err());
        settings.max_transfers = 9;
        assert!(state
            .manage(
                Operation::Apply {
                    settings: json!({"transport":settings})
                },
                || Value::Null
            )
            .is_err());
        assert_eq!(
            state.snapshot(Value::Null)["application"]["settings"]["transport"]["max_transfers"],
            1
        );
        state
            .manage(
                Operation::Command {
                    action: "disconnect".into(),
                },
                || Value::Null,
            )
            .unwrap();
        assert!(!state.flow(1, &AtomicBool::new(false), first.epoch));
        drop(first);
        assert_eq!(state.snapshot(Value::Null)["connection_count"], 0);
        assert!(state.begin("new".into()).is_some());
    }
    #[test]
    fn snapshots_never_include_credentials_and_pause_is_cancellable() {
        let state = Arc::new(State::new(1234));
        let settings = Settings {
            paused: true,
            hls_enabled: false,
            ..Default::default()
        };
        state
            .manage(
                Operation::Apply {
                    settings: json!({"transport":settings}),
                },
                || Value::Null,
            )
            .unwrap();
        assert!(!state.allowed(true));
        assert!(state.allowed(false));
        assert!(!state.flow(64, &AtomicBool::new(true), 0));
        let snapshot = state.snapshot(Value::Null).to_string();
        assert!(!snapshot.contains("token"));
        assert!(!snapshot.contains("Authorization"));
        assert!(state
            .manage(
                Operation::Command {
                    action: "arbitrary shell".into()
                },
                || Value::Null
            )
            .is_err());
    }
}

#[cfg(test)]
mod movie_tests {
    use super::*;
    #[test]
    fn movies_remain_visible_between_segment_requests_and_transfer_bytes_are_exact() {
        let state = Arc::new(State::new(1234));
        {
            let transfer = state.begin("127.0.0.1:1000".into()).unwrap();
            transfer.movie(
                "movie-id",
                &json!({"title":"A movie","width":1920,"height":1080}),
                "HLS",
                "1080",
                None,
            );
            transfer.sent(1234);
            let snapshot = state.snapshot(Value::Null);
            assert_eq!(snapshot["application"]["movies"][0]["active_transfers"], 1);
        }
        let snapshot = state.snapshot(Value::Null);
        let movie = &snapshot["application"]["movies"][0];
        assert_eq!(movie["details"]["title"], "A movie");
        assert_eq!(movie["bytes"], 1234);
        assert_eq!(movie["active_transfers"], 0);
        assert_eq!(snapshot["connection_count"], 0);
        assert_eq!(snapshot["metrics"]["video_bytes"], 1234);
    }
}
