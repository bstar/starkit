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
    OriginalOpen {
        session: u64,
        generation: u64,
        image: String,
        size: u64,
        name: String,
        start: f64,
        options: media::tracks::PlaybackOptions,
    },
    Range {
        session: u64,
        generation: u64,
        offset: u64,
        data: String,
    },
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
        #[serde(default)]
        options: media::tracks::PlaybackOptions,
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
    Read {
        session: u64,
        generation: u64,
        offset: u64,
    },
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
        #[serde(default)]
        buffered_bytes: u64,
        #[serde(default)]
        dropped_frames: u64,
        #[serde(default)]
        source_bitrate: u64,
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
fn send_original(
    tx: &Sender<ToClient>,
    mut message: ToClient,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    loop {
        anyhow::ensure!(!cancel.load(Ordering::Relaxed), "Original media cancelled");
        match tx.send_timeout(message, Duration::from_millis(20)) {
            Ok(()) => return Ok(()),
            Err(crossbeam_channel::SendTimeoutError::Timeout(value)) => message = value,
            Err(_) => anyhow::bail!("Original media disconnected"),
        }
    }
}
pub struct Host {
    pub session: u64,
    pub generation: u64,
    pub position: f64,
    pub paused: bool,
    pub volume: u8,
    pub quality: Quality,
    pub original: bool,
    pub buffered_bytes: u64,
    pub dropped_frames: u64,
    pub source_bitrate: u64,
    reads: Option<Sender<u64>>,
    pub buffering: bool,
    pub finished: bool,
    pub warning: Option<String>,
    pub tracks: media::tracks::Tracks,
    pub options: media::tracks::PlaybackOptions,
    discovery: Receiver<Result<media::tracks::Tracks, String>>,
    discovery_cancel: Arc<AtomicBool>,
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
        self.discovery_cancel.store(true, Ordering::Relaxed);
    }
}
impl Host {
    pub fn new(path: PathBuf, image: String, session: u64, local: bool) -> Self {
        Self::new_with_original(path, image, session, local, false)
    }
    pub fn new_with_original(
        path: PathBuf,
        image: String,
        session: u64,
        local: bool,
        original: bool,
    ) -> Self {
        let (_, queue) = bounded(1);
        let (tx, discovery) = bounded(1);
        let discovery_cancel = Arc::new(AtomicBool::new(false));
        let stop = discovery_cancel.clone();
        let source = path.clone();
        let _ = std::thread::Builder::new()
            .name("star-video-tracks".into())
            .spawn(move || {
                let _ = tx.send(
                    media::tracks::discover(&source, stop)
                        .map_err(|e| format!("Track discovery: {e:#}")),
                );
            });
        let mut h = Self {
            session,
            generation: 0,
            position: 0.0,
            paused: false,
            volume: 0,
            quality: Quality::Balanced,
            original: original && !local,
            buffered_bytes: 0,
            dropped_frames: 0,
            source_bitrate: 0,
            reads: None,
            buffering: true,
            finished: false,
            warning: None,
            tracks: Default::default(),
            options: Default::default(),
            discovery,
            discovery_cancel,
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
    pub fn set_original(&mut self, original: bool) {
        let original = original && !self.local;
        if self.original != original {
            self.original = original;
            self.control.clear();
            self.restart(self.position);
        }
    }
    pub fn local_playback(&self) -> bool {
        self.local
    }
    /// Replace queued startup messages with the saved playback state before
    /// the frontend opens a decoder, avoiding a brief play from the beginning.
    pub fn restore_playback(
        &mut self,
        position: f64,
        options: media::tracks::PlaybackOptions,
        paused: bool,
        volume: u8,
    ) {
        self.control.clear();
        self.options = options;
        self.paused = paused;
        self.volume = volume.min(100);
        self.restart(position);
    }
    pub fn set_bounds(&mut self, width: u32, height: u32) {
        let bounds = (width.max(2), height.max(2));
        if self.bounds != bounds {
            self.bounds = bounds;
            if !self.local && !self.original {
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
        self.last_buffering = true;
        self.stalls = 0;
        self.finished = false;
        self.warning = None;
        self.stable = Instant::now();
        self.reads = None;
        self.buffered_bytes = 0;
        self.dropped_frames = 0;
        if !self.original {
            self.control.push(ToClient::Open {
                session: self.session,
                generation: self.generation,
                image: self.image.clone(),
                local: self.local.then(|| self.path.clone()),
                start: self.position,
                quality: self.quality,
                options: self.options.clone(),
            });
        }
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
        let options = self.options.clone();
        if self.original {
            let (requests, incoming) = bounded(8);
            self.reads = Some(requests);
            let image = self.image.clone();
            let _ = std::thread::Builder::new()
                .name("star-original-media".into())
                .spawn(move || {
                    use std::io::{Read, Seek, SeekFrom};
                    let result = (|| -> anyhow::Result<()> {
                        let mut file = std::fs::File::open(&path)?;
                        anyhow::ensure!(
                            file.metadata()?.is_file(),
                            "Original media must be a regular file"
                        );
                        let size = file.metadata()?.len();
                        let name = format!(
                            "movie.{}",
                            path.extension()
                                .and_then(|v| v.to_str())
                                .filter(|v| v.len() < 16
                                    && v.chars().all(|c| c.is_ascii_alphanumeric()))
                                .unwrap_or("mkv")
                        );
                        send_original(
                            &tx,
                            ToClient::OriginalOpen {
                                session,
                                generation,
                                image,
                                size,
                                name,
                                start,
                                options,
                            },
                            &cancel,
                        )?;
                        while !cancel.load(Ordering::Relaxed) {
                            let offset = match incoming.recv_timeout(Duration::from_millis(20)) {
                                Ok(offset) => offset,
                                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                                Err(_) => break,
                            };
                            anyhow::ensure!(
                                offset < size
                                    && offset.is_multiple_of(super::original::BLOCK as u64),
                                "Invalid original media offset"
                            );
                            file.seek(SeekFrom::Start(offset))?;
                            let mut bytes = vec![
                                0;
                                (size - offset).min(super::original::BLOCK as u64)
                                    as usize
                            ];
                            file.read_exact(&mut bytes)?;
                            send_original(
                                &tx,
                                ToClient::Range {
                                    session,
                                    generation,
                                    offset,
                                    data: base64::engine::general_purpose::STANDARD.encode(bytes),
                                },
                                &cancel,
                            )?;
                        }
                        Ok(())
                    })();
                    if let Err(error) = result {
                        if !cancel.load(Ordering::Relaxed) {
                            let _ = send_original(
                                &tx,
                                ToClient::Error {
                                    session,
                                    generation,
                                    message: format!("Original media: {error:#}"),
                                },
                                &cancel,
                            );
                        }
                    }
                });
            return;
        }
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
                let result = media::proxy::encode_selected_to_fit(
                    &path,
                    start,
                    quality,
                    bounds,
                    writer,
                    cancel.clone(),
                    options,
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
            ToHost::Read {
                session,
                generation,
                offset,
            } if session == self.session && generation == self.generation && self.original => {
                if let Some(reads) = &self.reads {
                    let _ = reads.try_send(offset);
                }
            }
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
                buffered_bytes,
                dropped_frames,
                source_bitrate,
            } if session == self.session && generation == self.generation => {
                if position.is_finite() {
                    self.position = position.max(0.0);
                }
                self.buffered_bytes = buffered_bytes;
                self.dropped_frames = dropped_frames;
                self.source_bitrate = source_bitrate;
                self.buffering = buffering;
                self.finished = finished;
                if warning.is_some() {
                    self.warning = warning;
                }
                if !self.local && !self.original && !self.paused && !finished {
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
    pub fn load_subtitle(&mut self, path: PathBuf) -> anyhow::Result<()> {
        let path = if path.is_absolute() {
            path
        } else {
            self.path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join(path)
        };
        anyhow::ensure!(
            path.extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| matches!(
                    s.to_ascii_lowercase().as_str(),
                    "srt" | "ass" | "ssa" | "vtt"
                )),
            "Choose an SRT, ASS, SSA, or WebVTT subtitle file"
        );
        let selection = media::tracks::Selection::External(path.clone());
        self.tracks.subtitles.push(media::tracks::Track {
            selection: selection.clone(),
            label: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            default: false,
            forced: false,
        });
        self.select(false, selection)
    }
    pub fn select(
        &mut self,
        audio: bool,
        selection: media::tracks::Selection,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            if audio {
                self.tracks.contains_audio(&selection)
            } else {
                self.tracks.contains_subtitle(&selection)
            },
            "Track unavailable"
        );
        if audio {
            self.options.audio = selection;
        } else {
            self.options.subtitle = selection;
        }
        self.restart(self.position);
        Ok(())
    }
    pub fn effects(&mut self) -> Vec<ServerMessage> {
        if let Ok(result) = self.discovery.try_recv() {
            match result {
                Ok(tracks) => self.tracks = tracks,
                Err(error) => self.warning = Some(error),
            }
        }
        std::mem::take(&mut self.control)
            .into_iter()
            .map(|message| ServerMessage::Media { message })
            .collect()
    }
    pub fn chunks(&mut self) -> Vec<ServerMessage> {
        self.queue
            .try_iter()
            .take(if self.original { 8 } else { 2 })
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
    original: Option<Arc<super::original::Source>>,
    player: media::playback::Player,
    input: Option<Sender<Option<Vec<u8>>>>,
    consumed: Receiver<usize>,
    last: Instant,
    pending_credit: usize,
    arrival: Instant,
    arrival_bytes: usize,
    bandwidth_bps: u64,
    eof: bool,
    audio_underruns: u64,
}
pub struct Frontend {
    active: Option<Active>,
    local: bool,
    pending_control: Option<(u64, u64, bool, u8)>,
    audio: super::audio::Frontend,
}
impl Frontend {
    pub fn video_playing(&self) -> bool {
        self.active.as_ref().is_some_and(|a| {
            !a.player.controls.paused.load(Ordering::Relaxed)
                && !a.player.controls.finished.load(Ordering::Relaxed)
        })
    }
    pub fn reset(&mut self) {
        self.active = None;
        self.audio.reset();
        self.pending_control = None;
    }
    pub fn new(local: bool) -> Self {
        Self {
            active: None,
            local,
            pending_control: None,
            audio: super::audio::Frontend::default(),
        }
    }
    pub fn receive(&mut self, msg: ToClient, out: &Sender<ClientMessage>) -> anyhow::Result<()> {
        if self.audio.receive(&msg)? {
            return Ok(());
        }
        match msg {
            ToClient::OriginalOpen {
                session,
                generation,
                image,
                size,
                name,
                start,
                options,
            } => {
                anyhow::ensure!(
                    size > 0
                        && name.len() < 128
                        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
                        && start.is_finite()
                        && start >= 0.0
                        && image.len() < 128,
                    "Invalid original media session"
                );
                self.active = None;
                let source = super::original::Source::new(size, session, generation, out.clone());
                let controls = media::playback::Controls::new();
                if let Some((s, g, paused, volume)) = self
                    .pending_control
                    .filter(|(s, g, _, _)| *s == session && *g == generation)
                {
                    let _ = (s, g);
                    controls.paused.store(paused, Ordering::Relaxed);
                    controls.volume.store(u32::from(volume), Ordering::Relaxed);
                }
                let reader = source.clone();
                let open: media::playback::InputFactory = Arc::new(move |controls| {
                    let io = ffmpeg_next::format::context::StreamIo::from_read_seek(
                        reader.reader(controls.clone()),
                    )?;
                    ffmpeg_next::format::input_from_stream_with_interrupt(
                        io,
                        Some(&name),
                        None,
                        move || controls.cancelled.load(Ordering::Relaxed),
                    )
                });
                let player = media::playback::Player::remote(open, start, options, controls)?;
                let (_, consumed) = bounded(1);
                self.active = Some(Active {
                    session,
                    generation,
                    image,
                    original: Some(source),
                    player,
                    input: None,
                    consumed,
                    last: Instant::now(),
                    pending_credit: 0,
                    arrival: Instant::now(),
                    arrival_bytes: 0,
                    bandwidth_bps: 0,
                    eof: false,
                    audio_underruns: 0,
                });
            }
            ToClient::Range {
                session,
                generation,
                offset,
                data,
            } => {
                if let Some(active) = self
                    .active
                    .as_mut()
                    .filter(|a| a.session == session && a.generation == generation)
                {
                    anyhow::ensure!(
                        data.len() <= super::original::BLOCK.div_ceil(3) * 4,
                        "Original range exceeds limit"
                    );
                    if let Some(source) = &active.original {
                        source.accept(
                            offset,
                            base64::engine::general_purpose::STANDARD.decode(data)?,
                        )?;
                    }
                }
            }
            ToClient::AudioOpen { .. }
            | ToClient::AudioChunk { .. }
            | ToClient::AudioClose { .. } => unreachable!(),
            ToClient::Open {
                session,
                generation,
                image,
                local,
                start,
                options,
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
                    media::playback::Player::file_with_options(path, start, options)?
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
                    original: None,
                    player,
                    input: Some(tx),
                    consumed,
                    last: Instant::now(),
                    pending_credit: 0,
                    arrival: Instant::now(),
                    arrival_bytes: 0,
                    bandwidth_bps: 0,
                    eof: false,
                    audio_underruns: 0,
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
                self.pending_control = Some((session, generation, paused, volume.min(100)));
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
                if self.active.as_ref().map_or_else(
                    || {
                        self.pending_control
                            .is_some_and(|(s, g, _, _)| s == session && g == generation)
                    },
                    |a| a.session == session && a.generation == generation,
                ) {
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
                            buffered_bytes: 0,
                            dropped_frames: 0,
                            source_bitrate: 0,
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
            let underruns = a.player.controls.underruns.load(Ordering::Relaxed);
            if underruns != a.audio_underruns {
                tracing::warn!(
                    session = a.session,
                    generation = a.generation,
                    underruns,
                    "Movie audio buffer underrun"
                );
                a.audio_underruns = underruns;
            }
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
                buffered_bytes: a.original.as_ref().map_or(0, |s| s.buffered_bytes()),
                dropped_frames: a.player.controls.dropped.load(Ordering::Relaxed),
                source_bitrate: a.player.controls.source_bitrate.load(Ordering::Relaxed),
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
    fn restored_playback_opens_only_at_saved_position_and_control_state() {
        let mut host = Host::new(PathBuf::new(), "resume".into(), 1, true);
        let options = media::tracks::PlaybackOptions {
            audio: media::tracks::Selection::Stream(2),
            subtitle: media::tracks::Selection::Off,
        };
        host.restore_playback(37.125, options.clone(), true, 65);
        let effects = host.effects();
        let opens: Vec<_> = effects
            .iter()
            .filter_map(|effect| match effect {
                ServerMessage::Media {
                    message: ToClient::Open { start, options, .. },
                } => Some((*start, options)),
                _ => None,
            })
            .collect();
        assert_eq!(opens, vec![(37.125, &options)]);
        assert!(effects.iter().any(|effect| matches!(
            effect,
            ServerMessage::Media {
                message: ToClient::Control {
                    paused: true,
                    volume: 65,
                    ..
                }
            }
        )));
    }
    #[test]
    fn original_open_failure_is_reported_before_decoder_exists() {
        let mut frontend = Frontend::new(false);
        let (send, receive) = bounded(8);
        frontend
            .receive(
                ToClient::Control {
                    session: 4,
                    generation: 2,
                    paused: false,
                    volume: 80,
                },
                &send,
            )
            .unwrap();
        frontend
            .receive(
                ToClient::Error {
                    session: 4,
                    generation: 1,
                    message: "stale".into(),
                },
                &send,
            )
            .unwrap();
        assert!(receive.is_empty());
        frontend
            .receive(
                ToClient::Error {
                    session: 4,
                    generation: 2,
                    message: "Original media: permission denied".into(),
                },
                &send,
            )
            .unwrap();
        assert!(matches!(
            receive.try_recv().unwrap(),
            ClientMessage::Media {
                message: ToHost::Status {
                    finished: true,
                    generation: 2,
                    warning: Some(_),
                    ..
                }
            }
        ));
    }
    #[test]
    fn seek_startup_does_not_carry_stalls_or_reduce_quality() {
        let mut h = Host::new(PathBuf::new(), "test".into(), 1, true);
        h.stalls = 2;
        h.last_buffering = false;
        h.restart(120.0);
        h.local = false;
        h.receive(ToHost::Status {
            session: h.session,
            generation: h.generation,
            position: 120.0,
            buffering: true,
            finished: false,
            warning: None,
            bandwidth_bps: 0,
            buffered_bytes: 0,
            dropped_frames: 0,
            source_bitrate: 0,
        });
        assert_eq!(h.stalls, 0);
        assert!(h.last_buffering);
        assert_eq!(h.quality, Quality::Balanced);
    }
    #[test]
    fn original_stream_never_restarts_for_proxy_quality_adjustments() {
        let mut h = Host::new(PathBuf::new(), "test".into(), 1, true);
        h.local = false;
        h.original = true;
        let generation = h.generation;
        for buffering in [false, true, false, true, false, true, false] {
            h.stable = Instant::now() - Duration::from_secs(30);
            h.receive(ToHost::Status {
                session: h.session,
                generation: h.generation,
                position: 120.0,
                buffering,
                finished: false,
                warning: None,
                bandwidth_bps: 100_000_000,
                buffered_bytes: 8_000_000,
                dropped_frames: 0,
                source_bitrate: 18_000_000,
            });
            assert_eq!(h.generation, generation);
            assert_eq!(h.quality, Quality::Balanced);
        }
    }
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
            original: false,
            buffered_bytes: 0,
            dropped_frames: 0,
            source_bitrate: 0,
            reads: None,
            buffering: false,
            finished: false,
            warning: None,
            tracks: Default::default(),
            options: Default::default(),
            discovery: bounded(1).1,
            discovery_cancel: Arc::new(AtomicBool::new(false)),
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
