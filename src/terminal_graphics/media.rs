//! Negotiated media transport. Opaque sessions prevent remote path access locally.
use super::protocol::{ClientMessage, ServerMessage};
use crate::media::{self, Quality};
use base64::Engine as _;
use crossbeam_channel::{bounded, Receiver, Sender};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToClient {
    AudioOpen {
        session: u64,
        epoch: u64,
    },
    AudioChunk {
        session: u64,
        epoch: u64,
        samples: Vec<i16>,
    },
    AudioClose {
        session: u64,
    },
    Open {
        session: u64,
        generation: u64,
        image: String,
        local: Option<PathBuf>,
        start: f64,
        quality: Quality,
    },
    Chunk {
        session: u64,
        generation: u64,
        data: String,
    },
    Eof {
        session: u64,
        generation: u64,
    },
    Control {
        session: u64,
        generation: u64,
        paused: bool,
        volume: u8,
    },
    Close {
        session: u64,
        generation: u64,
    },
    Error {
        session: u64,
        generation: u64,
        message: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToHost {
    AudioCredit {
        session: u64,
        epoch: u64,
        blocks: usize,
    },
    AudioError {
        session: u64,
        epoch: u64,
        message: String,
    },
    Credit {
        session: u64,
        generation: u64,
        bytes: usize,
    },
    Status {
        session: u64,
        generation: u64,
        position: f64,
        buffering: bool,
        finished: bool,
        warning: Option<String>,
        #[serde(default)]
        bandwidth_bps: u64,
    },
}
struct Writer {
    pending: Vec<u8>,
    tx: Sender<ToClient>,
    cancel: Arc<AtomicBool>,
    credit: Arc<Mutex<usize>>,
    session: u64,
    generation: u64,
}
impl Writer {
    fn send(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        let n = bytes.len();
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(std::io::ErrorKind::ConnectionAborted.into());
            }
            let mut credit = self.credit.lock().unwrap();
            if *credit >= n {
                *credit -= n;
                break;
            }
            drop(credit);
            std::thread::sleep(Duration::from_millis(5));
        }
        let msg = ToClient::Chunk {
            session: self.session,
            generation: self.generation,
            data: base64::engine::general_purpose::STANDARD.encode(&bytes[..n]),
        };
        let mut msg = msg;
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(std::io::ErrorKind::ConnectionAborted.into());
            }
            match self.tx.send_timeout(msg, Duration::from_millis(20)) {
                Ok(()) => return Ok(()),
                Err(crossbeam_channel::SendTimeoutError::Timeout(m)) => msg = m,
                Err(_) => return Err(std::io::ErrorKind::BrokenPipe.into()),
            }
        }
    }
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let n = bytes.len().min(media::CHUNK - self.pending.len());
        self.pending.extend_from_slice(&bytes[..n]);
        if self.pending.len() == media::CHUNK {
            let chunk = std::mem::replace(&mut self.pending, Vec::with_capacity(media::CHUNK));
            self.send(&chunk)?;
        }
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let chunk = std::mem::take(&mut self.pending);
        if !chunk.is_empty() {
            self.send(&chunk)?;
        }
        Ok(())
    }
}
pub struct Host {
    pub session: u64,
    pub generation: u64,
    pub position: f64,
    pub paused: bool,
    pub volume: u8,
    pub quality: Quality,
    pub buffering: bool,
    pub finished: bool,
    pub warning: Option<String>,
    path: PathBuf,
    image: String,
    local: bool,
    bounds: (u32, u32),
    cancel: Arc<AtomicBool>,
    credit: Arc<Mutex<usize>>,
    queue: Receiver<ToClient>,
    control: Vec<ToClient>,
    stable: Instant,
    stalls: u8,
    last_buffering: bool,
}
impl Drop for Host {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Host {
    pub fn new(path: PathBuf, image: String, session: u64, local: bool) -> Self {
        let (_, queue) = bounded(1);
        let mut h = Self {
            session,
            generation: 0,
            position: 0.0,
            paused: false,
            volume: 0,
            quality: Quality::Balanced,
            buffering: true,
            finished: false,
            warning: None,
            path,
            image,
            local,
            bounds: (1280, 720),
            cancel: Arc::new(AtomicBool::new(false)),
            credit: Arc::new(Mutex::new(0)),
            queue,
            control: vec![],
            stable: Instant::now(),
            stalls: 0,
            last_buffering: true,
        };
        h.restart(0.0);
        h
    }
    pub fn local_playback(&self) -> bool {
        self.local
    }
    pub fn set_bounds(&mut self, width: u32, height: u32) {
        let bounds = (width.max(2), height.max(2));
        if self.bounds != bounds {
            self.bounds = bounds;
            if !self.local {
                self.control.clear();
                self.restart(self.position);
            }
        }
    }
    pub fn restart(&mut self, position: f64) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.credit = Arc::new(Mutex::new(0));
        self.generation += 1;
        self.image = format!("video-{}-{}", self.session, self.generation);
        self.position = position.max(0.0);
        self.buffering = true;
        self.finished = false;
        self.warning = None;
        self.stable = Instant::now();
        self.control.push(ToClient::Open {
            session: self.session,
            generation: self.generation,
            image: self.image.clone(),
            local: self.local.then(|| self.path.clone()),
            start: self.position,
            quality: self.quality,
        });
        self.control.push(ToClient::Control {
            session: self.session,
            generation: self.generation,
            paused: self.paused,
            volume: self.volume,
        });
        let (tx, rx) = bounded(8);
        self.queue = rx;
        if self.local {
            return;
        }
        let bounds = self.bounds;
        let path = self.path.clone();
        let quality = self.quality;
        let start = self.position;
        let cancel = self.cancel.clone();
        let credit = self.credit.clone();
        let session = self.session;
        let generation = self.generation;
        let worker = std::thread::Builder::new()
            .name("star-video-proxy".into())
            .spawn(move || {
                let writer = Writer {
                    pending: Vec::with_capacity(media::CHUNK),
                    tx: tx.clone(),
                    cancel: cancel.clone(),
                    credit,
                    session,
                    generation,
                };
                let result = media::proxy::encode_to_fit(
                    &path,
                    start,
                    quality,
                    bounds,
                    writer,
                    cancel.clone(),
                );
                if !cancel.load(Ordering::Relaxed) {
                    let message = match result {
                        Ok(()) => ToClient::Eof {
                            session,
                            generation,
                        },
                        Err(e) => ToClient::Error {
                            session,
                            generation,
                            message: format!("Video preview: {e:#}"),
                        },
                    };
                    let _ = tx.send(message);
                }
            });
        if let Err(e) = worker {
            self.warning = Some(e.to_string());
        }
    }
    pub fn set_control(&mut self, paused: bool, volume: u8) {
        self.paused = paused;
        self.volume = volume.min(100);
        self.control.push(ToClient::Control {
            session: self.session,
            generation: self.generation,
            paused,
            volume: self.volume,
        });
    }
    pub fn receive(&mut self, msg: ToHost) {
        match msg {
            ToHost::Credit {
                session,
                generation,
                bytes,
            } if session == self.session && generation == self.generation => {
                let mut credit = self.credit.lock().unwrap();
                *credit = credit.saturating_add(bytes).min(media::WINDOW);
            }
            ToHost::Status {
                session,
                generation,
                position,
                buffering,
                finished,
                warning,
                bandwidth_bps,
            } if session == self.session && generation == self.generation => {
                if position.is_finite() {
                    self.position = position.max(0.0);
                }
                self.buffering = buffering;
                self.finished = finished;
                if warning.is_some() {
                    self.warning = warning;
                }
                if !self.local && !self.paused && !finished {
                    if buffering && !self.last_buffering {
                        self.stalls += 1;
                        self.stable = Instant::now();
                    }
                    if self.stalls >= 3 && self.quality != Quality::Low {
                        self.quality = self.quality.down();
                        self.stalls = 0;
                        self.restart(self.position);
                    }
                    // Upgrades need a long period without a buffer stall.
                    else if !buffering
                        && self.stable.elapsed() > Duration::from_secs(20)
                        && self.quality != Quality::High
                        && bandwidth_bps > u64::from(self.quality.up().parameters().2) * 3 / 2
                    {
                        self.quality = self.quality.up();
                        self.restart(self.position);
                    }
                }
                self.last_buffering = buffering;
            }
            _ => {}
        }
    }
    pub fn effects(&mut self) -> Vec<ServerMessage> {
        std::mem::take(&mut self.control)
            .into_iter()
            .map(|message| ServerMessage::Media { message })
            .collect()
    }
    pub fn chunks(&mut self) -> Vec<ServerMessage> {
        self.queue
            .try_iter()
            .take(2)
            .map(|message| ServerMessage::Media { message })
            .collect()
    }
    pub fn close(&self) -> ServerMessage {
        ServerMessage::Media {
            message: ToClient::Close {
                session: self.session,
                generation: self.generation,
            },
        }
    }
}

struct Active {
    session: u64,
    generation: u64,
    image: String,
    player: media::playback::Player,
    input: Option<Sender<Option<Vec<u8>>>>,
    consumed: Receiver<usize>,
    last: Instant,
    pending_credit: usize,
    arrival: Instant,
    arrival_bytes: usize,
    bandwidth_bps: u64,
    eof: bool,
}
pub struct Frontend {
    active: Option<Active>,
    local: bool,
    audio: super::audio::Frontend,
}
impl Frontend {
    pub fn reset(&mut self) {
        self.active = None;
        self.audio.reset();
    }
    pub fn new(local: bool) -> Self {
        Self {
            active: None,
            local,
            audio: super::audio::Frontend::default(),
        }
    }
    pub fn receive(&mut self, msg: ToClient, out: &Sender<ClientMessage>) -> anyhow::Result<()> {
        if self.audio.receive(&msg)? {
            return Ok(());
        }
        match msg {
            ToClient::AudioOpen { .. }
            | ToClient::AudioChunk { .. }
            | ToClient::AudioClose { .. } => unreachable!(),
            ToClient::Open {
                session,
                generation,
                image,
                local,
                start,
                ..
            } => {
                anyhow::ensure!(
                    start.is_finite() && start >= 0.0 && image.len() < 128,
                    "Invalid media session"
                );
                self.active = None;
                let (tx, rx) = bounded(8);
                let (credits, consumed) = bounded(16);
                let player = if let Some(path) = local {
                    anyhow::ensure!(self.local, "Remote media cannot open local paths");
                    media::playback::Player::file(path, start)?
                } else {
                    let controls = media::playback::Controls::new();
                    // The reader token is cancelled together with the Player below.
                    media::playback::Player::stream_with_controls(
                        media::playback::StreamReader {
                            receiver: rx,
                            cancel: controls.clone(),
                            consumed: credits,
                            current: std::io::Cursor::new(vec![]),
                        },
                        start,
                        controls,
                    )?
                };
                self.active = Some(Active {
                    session,
                    generation,
                    image,
                    player,
                    input: Some(tx),
                    consumed,
                    last: Instant::now(),
                    pending_credit: 0,
                    arrival: Instant::now(),
                    arrival_bytes: 0,
                    bandwidth_bps: 0,
                    eof: false,
                });
                let _ = out.try_send(ClientMessage::Media {
                    message: ToHost::Credit {
                        session,
                        generation,
                        bytes: media::WINDOW,
                    },
                });
            }
            ToClient::Chunk {
                session,
                generation,
                data,
            } => {
                if let Some(a) = self
                    .active
                    .as_mut()
                    .filter(|a| a.session == session && a.generation == generation)
                {
                    anyhow::ensure!(
                        data.len() <= media::CHUNK.div_ceil(3) * 4,
                        "Media chunk exceeds limit"
                    );
                    let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
                    anyhow::ensure!(bytes.len() <= media::CHUNK, "Media chunk exceeds limit");
                    a.arrival_bytes += bytes.len();
                    let elapsed = a.arrival.elapsed();
                    if elapsed >= Duration::from_millis(20) {
                        let measured =
                            (a.arrival_bytes as f64 * 8.0 / elapsed.as_secs_f64()) as u64;
                        a.bandwidth_bps = if a.bandwidth_bps == 0 {
                            measured
                        } else {
                            (a.bandwidth_bps * 3 + measured) / 4
                        };
                        a.arrival_bytes = 0;
                        a.arrival = Instant::now();
                    }
                    if let Some(tx) = &a.input {
                        tx.try_send(Some(bytes)).map_err(|_| {
                            anyhow::anyhow!("Media receiver exceeded its credit window")
                        })?;
                    }
                }
            }
            ToClient::Eof {
                session,
                generation,
            } => {
                if let Some(a) = self
                    .active
                    .as_mut()
                    .filter(|a| a.session == session && a.generation == generation)
                {
                    a.eof = true;
                    a.input.take();
                }
            }
            ToClient::Control {
                session,
                generation,
                paused,
                volume,
            } => {
                if let Some(a) = self
                    .active
                    .as_mut()
                    .filter(|a| a.session == session && a.generation == generation)
                {
                    a.player.controls.paused.store(paused, Ordering::Relaxed);
                    a.player
                        .controls
                        .volume
                        .store(u32::from(volume.min(100)), Ordering::Relaxed);
                }
            }
            ToClient::Close {
                session,
                generation,
            } => {
                if self
                    .active
                    .as_ref()
                    .is_some_and(|a| a.session == session && a.generation <= generation)
                {
                    self.active = None;
                }
            }
            ToClient::Error {
                session,
                generation,
                message,
            } => {
                if self
                    .active
                    .as_ref()
                    .is_some_and(|a| a.session == session && a.generation == generation)
                {
                    self.active = None;
                    let _ = out.try_send(ClientMessage::Media {
                        message: ToHost::Status {
                            session,
                            generation,
                            position: 0.0,
                            buffering: false,
                            finished: true,
                            warning: Some(message),
                            bandwidth_bps: 0,
                        },
                    });
                }
            }
        }
        Ok(())
    }
    pub fn tick(
        &mut self,
        out: &Sender<ClientMessage>,
    ) -> Option<(String, Arc<crate::image::RgbaImage>)> {
        self.audio.tick(out);
        let a = self.active.as_mut()?;
        a.pending_credit += a.consumed.try_iter().sum::<usize>();
        if a.pending_credit > 0
            && out
                .try_send(ClientMessage::Media {
                    message: ToHost::Credit {
                        session: a.session,
                        generation: a.generation,
                        bytes: a.pending_credit,
                    },
                })
                .is_ok()
        {
            a.pending_credit = 0;
        }
        let frame = a.player.frames.try_iter().last();
        if a.last.elapsed() > Duration::from_millis(250) {
            let warning = a
                .player
                .notices
                .try_iter()
                .filter(|n| n != "Playback finished")
                .last();
            let finished = a.player.controls.finished.load(Ordering::Relaxed);
            let position = a.player.controls.position_ms.load(Ordering::Relaxed) as f64 / 1000.0;
            let message = ToHost::Status {
                session: a.session,
                generation: a.generation,
                position,
                buffering: a.player.controls.buffering.load(Ordering::Relaxed) && !a.eof,
                finished,
                warning,
                bandwidth_bps: a.bandwidth_bps,
            };
            if out.try_send(ClientMessage::Media { message }).is_ok() {
                a.last = Instant::now();
            }
        }
        frame.map(|f| (a.image.clone(), f.pixels))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_credits_cannot_resume_new_seek() {
        let (_, rx) = bounded(1);
        let mut h = Host {
            session: 1,
            generation: 2,
            position: 0.,
            paused: false,
            volume: 0,
            quality: Quality::Balanced,
            buffering: false,
            finished: false,
            warning: None,
            path: PathBuf::new(),
            image: String::new(),
            local: true,
            bounds: (1280, 720),
            cancel: Arc::new(AtomicBool::new(false)),
            credit: Arc::new(Mutex::new(0)),
            queue: rx,
            control: vec![],
            stable: Instant::now(),
            stalls: 0,
            last_buffering: false,
        };
        h.receive(ToHost::Credit {
            session: 1,
            generation: 1,
            bytes: usize::MAX,
        });
        assert_eq!(*h.credit.lock().unwrap(), 0);
        h.receive(ToHost::Credit {
            session: 1,
            generation: 2,
            bytes: usize::MAX,
        });
        assert_eq!(*h.credit.lock().unwrap(), media::WINDOW);
    }
}
