//! Negotiated source-format audio. Legacy fixed stereo audio remains independent.
use super::{
    media::{ToClient, ToHost},
    protocol::ClientMessage,
};
use crate::pcm::{Spec, MAX_BLOCK_BYTES, WINDOW_BLOCKS};
use anyhow::{ensure, Result};
use base64::Engine as _;
use crossbeam_channel::{bounded, Receiver, Sender};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;

struct Active {
    cache: Arc<super::pcm_cache::Cache>,
    paused: Arc<AtomicBool>,
    cache_mode: Arc<AtomicBool>,
    buffered_reported: u64,
    spool_worker: Option<std::thread::JoinHandle<()>>,
    played: Arc<AtomicU64>,
    reported: u64,
    session: u64,
    epoch: u64,
    spec: Spec,
    blocks: Sender<Option<(crate::pcm::SampleFormat, bool, Vec<u8>)>>,
    credits: Receiver<()>,
    events: Receiver<Result<ToHost, String>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    pending: usize,
    pending_event: Option<ToHost>,
}
impl Drop for Active {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.spool_worker.take() {
            let _ = worker.join();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[derive(Default)]
pub struct Frontend {
    active: Option<Active>,
}
impl Frontend {
    pub fn reset(&mut self) {
        self.active = None;
    }
    pub fn receive(&mut self, msg: &ToClient) -> Result<bool> {
        match msg {
            ToClient::PcmOpen {
                session,
                epoch,
                spec,
                compatibility,
            } => {
                spec.validate()?;
                self.active = None;
                let (tx, rx) = bounded(WINDOW_BLOCKS);
                let cache = Arc::new(super::pcm_cache::Cache::new()?);
                let cache_mode = Arc::new(AtomicBool::new(false));
                let output_cache = cache.clone();
                let output_mode = cache_mode.clone();
                let paused = Arc::new(AtomicBool::new(false));
                let output_paused = paused.clone();
                let (credit, credits) = bounded(WINDOW_BLOCKS);
                let (events, receive) = bounded(2);
                let played = Arc::new(AtomicU64::new(0));
                let callback_played = played.clone();
                let stop = Arc::new(AtomicBool::new(false));
                let cancel = stop.clone();
                let source = *spec;
                let compatibility = *compatibility;
                let sid = *session;
                let eid = *epoch;
                let spool_cache = cache.clone();
                let spool_mode = cache_mode.clone();
                let spool_cancel = stop.clone();
                let spool_events = events.clone();
                let spool_credit = credit.clone();
                let spool_worker = std::thread::Builder::new()
                    .name("star-pcm-cache".into())
                    .spawn(move || {
                        let result = spool(
                            rx,
                            spool_cache,
                            spool_mode,
                            spool_credit,
                            &spool_cancel,
                            source.channels,
                        );
                        if let Err(error) = result {
                            let _ =
                                spool_events.try_send(Err(format!("Local audio cache: {error:#}")));
                            spool_cancel.store(true, Ordering::Release);
                        }
                    })?;
                let worker = std::thread::Builder::new()
                    .name("star-pcm-output".into())
                    .spawn(move || {
                        if let Err(e) = play(
                            output_cache,
                            output_mode,
                            output_paused,
                            credit,
                            events.clone(),
                            cancel.clone(),
                            source,
                            compatibility,
                            callback_played,
                            sid,
                            eid,
                        ) {
                            if !cancel.load(Ordering::Relaxed) {
                                let _ = events.try_send(Err(format!("Local audio: {e:#}")));
                                cancel.store(true, Ordering::Relaxed);
                            }
                        }
                    })?;
                self.active = Some(Active {
                    cache,
                    paused,
                    cache_mode,
                    buffered_reported: 0,
                    spool_worker: Some(spool_worker),
                    played,
                    reported: 0,
                    session: *session,
                    epoch: *epoch,
                    spec: *spec,
                    blocks: tx,
                    credits,
                    events: receive,
                    stop,
                    worker: Some(worker),
                    pending: 0,
                    pending_event: None,
                });
            }
            ToClient::PcmPause {
                session,
                epoch,
                paused,
            } => {
                if let Some(a) = self
                    .active
                    .as_mut()
                    .filter(|a| a.session == *session && a.epoch == *epoch)
                {
                    a.paused.store(*paused, Ordering::Release);
                }
            }
            ToClient::PcmCache { session, epoch } => {
                if let Some(a) = self
                    .active
                    .as_mut()
                    .filter(|a| a.session == *session && a.epoch == *epoch)
                {
                    a.cache_mode.store(true, Ordering::Release);
                }
            }
            ToClient::PcmChunk {
                session,
                epoch,
                format,
                processed,
                data,
            } => {
                if let Some(a) = self
                    .active
                    .as_mut()
                    .filter(|a| a.session == *session && a.epoch == *epoch)
                {
                    ensure!(
                        data.len() <= MAX_BLOCK_BYTES.div_ceil(3) * 4,
                        "PCM base64 exceeds limit"
                    );
                    let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
                    crate::pcm::Spec {
                        format: *format,
                        ..a.spec
                    }
                    .validate_bytes(bytes.len())?;
                    if !a.stop.load(Ordering::Relaxed) {
                        ensure!(
                            a.blocks
                                .try_send(Some((*format, *processed, bytes)))
                                .is_ok(),
                            "PCM sender exceeded credit window"
                        );
                    }
                }
            }
            ToClient::PcmEnd { session, epoch } => {
                if let Some(a) = self
                    .active
                    .as_mut()
                    .filter(|a| a.session == *session && a.epoch == *epoch)
                {
                    ensure!(
                        a.blocks.try_send(None).is_ok(),
                        "PCM end exceeds credit window"
                    );
                }
            }
            ToClient::PcmClose { session, epoch } => {
                if self
                    .active
                    .as_ref()
                    .is_some_and(|a| a.session == *session && a.epoch == *epoch)
                {
                    self.active = None;
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    pub fn tick(&mut self, out: &Sender<ClientMessage>) {
        let Some(a) = self.active.as_mut() else {
            return;
        };
        let buffered = a.cache.frames.load(Ordering::Acquire);
        if a.cache_mode.load(Ordering::Acquire)
            && buffered != a.buffered_reported
            && out
                .try_send(ClientMessage::Media {
                    message: ToHost::PcmBuffered {
                        session: a.session,
                        epoch: a.epoch,
                        frames: buffered,
                    },
                })
                .is_ok()
        {
            a.buffered_reported = buffered;
        }
        let played = a.played.load(Ordering::Acquire);
        if played != a.reported
            && out
                .try_send(ClientMessage::Media {
                    message: ToHost::PcmProgress {
                        session: a.session,
                        epoch: a.epoch,
                        frames: played,
                    },
                })
                .is_ok()
        {
            a.reported = played;
        }
        if a.pending_event.is_none() {
            if let Ok(event) = a.events.try_recv() {
                a.pending_event = Some(match event {
                    Ok(event) => {
                        if matches!(event, ToHost::PcmReady { .. }) {
                            a.pending = WINDOW_BLOCKS;
                        }
                        event
                    }
                    Err(message) => ToHost::AudioError {
                        session: a.session,
                        epoch: a.epoch,
                        message,
                    },
                });
            }
        }
        if let Some(event) = a.pending_event.take() {
            if let Err(error) = out.try_send(ClientMessage::Media { message: event }) {
                if let ClientMessage::Media { message } = error.into_inner() {
                    a.pending_event = Some(message);
                }
            }
        }
        if a.stop.load(Ordering::Relaxed) {
            return;
        }
        a.pending += a.credits.try_iter().count();
        if a.pending > 0
            && out
                .try_send(ClientMessage::Media {
                    message: ToHost::AudioCredit {
                        session: a.session,
                        epoch: a.epoch,
                        blocks: a.pending.min(WINDOW_BLOCKS),
                    },
                })
                .is_ok()
        {
            a.pending = 0;
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn play(
    cache: Arc<super::pcm_cache::Cache>,
    cache_mode: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    credit: Sender<()>,
    events: Sender<Result<ToHost, String>>,
    stop: Arc<AtomicBool>,
    source: Spec,
    compatibility: bool,
    played: Arc<AtomicU64>,
    session: u64,
    epoch: u64,
) -> Result<()> {
    use cpal::traits::StreamTrait;
    let plan = crate::pcm_output::Plan::select(source, compatibility)?;
    let rate = plan.config.sample_rate.0;
    let channels = plan.config.channels;
    let format = format!("{:?}", plan.format);
    let converted = plan.converted;
    // A half-second device reserve absorbs brief scheduling stalls while the
    // independent worker feeds the whole-song disk cache into this bounded ring.
    let (mut producer, consumer) =
        rtrb::RingBuffer::new((rate as usize * channels as usize / 2).max(1024));
    let (errors, error) = bounded(1);
    let mut device_frames = 0u64;
    let began = std::time::Instant::now();
    let playback_deadline = Arc::new(AtomicU64::new(0));
    let callback_deadline = playback_deadline.clone();
    let consumed = Arc::new(AtomicU64::new(0));
    let callback_consumed = consumed.clone();
    let stream =
        plan.open_with_timing(consumer, source, paused, errors, move |values, _, delay| {
            device_frames += values.len() as u64 / u64::from(channels);
            if !values.is_empty() {
                callback_deadline.store(
                    (began.elapsed() + delay)
                        .as_micros()
                        .min(u128::from(u64::MAX)) as u64,
                    Ordering::Release,
                );
            }
            played.store(
                device_frames.saturating_mul(u64::from(source.sample_rate)) / u64::from(rate),
                Ordering::Release,
            );
            callback_consumed.store(device_frames, Ordering::Release);
        })?;
    stream.play()?;
    let mut converter = if rate != source.sample_rate || channels != source.channels || converted {
        Some(crate::pcm_output::Converter::new(source, channels, rate)?)
    } else {
        None
    };
    let _ = events.send(Ok(ToHost::PcmReady {
        session,
        epoch,
        sample_rate: rate,
        channels,
        format,
        converted,
    }));
    let mut queued_frames = 0u64;
    while !stop.load(Ordering::Relaxed) {
        if let Ok(e) = error.try_recv() {
            anyhow::bail!("{e}");
        }
        let (format, processed, bytes) = match cache.next()? {
            super::pcm_cache::Next::Block(b) => b,
            super::pcm_cache::Next::End => {
                if let Some(c) = converter.as_mut() {
                    let tail = c.finish()?;
                    queued_frames += tail.len() as u64 / u64::from(channels);
                    enqueue(tail, true, &mut producer, &stop, &error)?;
                }
                while (producer.slots() < producer.buffer().capacity()
                    || consumed.load(Ordering::Acquire) < queued_frames
                    || began.elapsed().as_micros()
                        < u128::from(playback_deadline.load(Ordering::Acquire)))
                    && !stop.load(Ordering::Relaxed)
                {
                    if let Ok(e) = error.try_recv() {
                        anyhow::bail!("{e}");
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                if !stop.load(Ordering::Relaxed) {
                    let _ = events.send(Ok(ToHost::PcmFinished { session, epoch }));
                }
                return Ok(());
            }
            super::pcm_cache::Next::Waiting => {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
        };
        let samples = crate::pcm::Spec { format, ..source }.decode(&bytes)?;
        let samples = if let Some(c) = converter.as_mut() {
            c.convert(&samples)?
        } else {
            samples
        };
        queued_frames += samples.len() as u64 / u64::from(channels);
        enqueue(
            samples,
            converted || processed,
            &mut producer,
            &stop,
            &error,
        )?;
        if !cache_mode.load(Ordering::Acquire) {
            let _ = credit.try_send(());
        }
    }
    Ok(())
}

fn spool(
    blocks: Receiver<Option<super::pcm_cache::Block>>,
    cache: Arc<super::pcm_cache::Cache>,
    cache_mode: Arc<AtomicBool>,
    credit: Sender<()>,
    stop: &AtomicBool,
    channels: u16,
) -> Result<()> {
    while !stop.load(Ordering::Acquire) {
        match blocks.recv_timeout(Duration::from_millis(5)) {
            Ok(Some(block)) => {
                cache.append(block, channels)?;
                if cache_mode.load(Ordering::Acquire) {
                    let _ = credit.try_send(());
                }
            }
            Ok(None) => {
                cache.finish();
                return Ok(());
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(_) => return Ok(()),
        }
    }
    Ok(())
}

fn enqueue(
    samples: Vec<f64>,
    processed: bool,
    producer: &mut rtrb::Producer<crate::pcm::Sample>,
    stop: &AtomicBool,
    error: &Receiver<String>,
) -> Result<()> {
    for value in samples {
        let mut value = crate::pcm::Sample { value, processed };
        loop {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            if let Ok(e) = error.try_recv() {
                anyhow::bail!("{e}");
            }
            match producer.push(value) {
                Ok(()) => break,
                Err(rtrb::PushError::Full(v)) => {
                    value = v;
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    type Blocks = Receiver<Option<(crate::pcm::SampleFormat, bool, Vec<u8>)>>;
    fn spec() -> Spec {
        Spec {
            sample_rate: 48000,
            channels: 2,
            channel_mask: 3,
            format: crate::pcm::SampleFormat::I32,
            valid_bits: Some(32),
        }
    }
    fn fixture() -> (Frontend, Blocks) {
        let (blocks, receive) = bounded(WINDOW_BLOCKS);
        let (_, credits) = bounded(WINDOW_BLOCKS);
        let (_, events) = bounded(2);
        (
            Frontend {
                active: Some(Active {
                    cache: Arc::new(super::super::pcm_cache::Cache::new().unwrap()),
                    paused: Arc::new(AtomicBool::new(false)),
                    cache_mode: Arc::new(AtomicBool::new(false)),
                    buffered_reported: 0,
                    spool_worker: None,
                    played: Arc::new(AtomicU64::new(0)),
                    reported: 0,
                    session: 11,
                    epoch: 3,
                    spec: spec(),
                    blocks,
                    credits,
                    events,
                    stop: Arc::new(AtomicBool::new(false)),
                    worker: None,
                    pending: 0,
                    pending_event: None,
                }),
            },
            receive,
        )
    }
    #[test]
    fn disk_credits_download_a_song_without_a_playback_consumer() {
        let (blocks, receive) = bounded(WINDOW_BLOCKS);
        let (credit, credits) = bounded(WINDOW_BLOCKS);
        let cache = Arc::new(super::super::pcm_cache::Cache::new().unwrap());
        let stored = cache.clone();
        let worker = std::thread::spawn(move || {
            spool(
                receive,
                stored,
                Arc::new(AtomicBool::new(true)),
                credit,
                &AtomicBool::new(false),
                2,
            )
            .unwrap()
        });
        for _ in 0..300 {
            blocks
                .send(Some((
                    crate::pcm::SampleFormat::I32,
                    false,
                    vec![0; 960 * 8],
                )))
                .unwrap();
            credits.recv_timeout(Duration::from_secs(1)).unwrap();
        }
        blocks.send(None).unwrap();
        worker.join().unwrap();
        assert_eq!(cache.frames.load(Ordering::Acquire), 288000);
        for _ in 0..300 {
            assert!(matches!(
                cache.next().unwrap(),
                super::super::pcm_cache::Next::Block(_)
            ));
        }
        assert!(matches!(
            cache.next().unwrap(),
            super::super::pcm_cache::Next::End
        ));
    }
    #[test]
    fn stale_cache_and_pause_do_not_change_active_audio() {
        let (mut frontend, _) = fixture();
        frontend
            .receive(&ToClient::PcmCache {
                session: 11,
                epoch: 2,
            })
            .unwrap();
        frontend
            .receive(&ToClient::PcmPause {
                session: 11,
                epoch: 2,
                paused: true,
            })
            .unwrap();
        let active = frontend.active.as_ref().unwrap();
        assert!(!active.cache_mode.load(Ordering::Acquire));
        assert!(!active.paused.load(Ordering::Acquire));
        frontend
            .receive(&ToClient::PcmCache {
                session: 11,
                epoch: 3,
            })
            .unwrap();
        frontend
            .receive(&ToClient::PcmPause {
                session: 11,
                epoch: 3,
                paused: true,
            })
            .unwrap();
        let active = frontend.active.as_ref().unwrap();
        assert!(active.cache_mode.load(Ordering::Acquire));
        assert!(active.paused.load(Ordering::Acquire));
    }
    #[test]
    fn stale_close_end_and_chunks_cannot_affect_a_new_epoch() {
        let (mut frontend, receive) = fixture();
        frontend
            .receive(&ToClient::PcmClose {
                session: 11,
                epoch: 2,
            })
            .unwrap();
        frontend
            .receive(&ToClient::PcmEnd {
                session: 11,
                epoch: 2,
            })
            .unwrap();
        frontend
            .receive(&ToClient::PcmChunk {
                session: 11,
                epoch: 2,
                format: crate::pcm::SampleFormat::I32,
                processed: false,
                data: "malformed".into(),
            })
            .unwrap();
        assert!(frontend.active.is_some());
        assert!(receive.is_empty());
    }
    #[test]
    fn rejects_malformed_and_over_window_blocks() {
        let (mut frontend, _receive) = fixture();
        let mut block = ToClient::PcmChunk {
            session: 11,
            epoch: 3,
            format: crate::pcm::SampleFormat::I32,
            processed: false,
            data: "malformed".into(),
        };
        assert!(frontend.receive(&block).is_err());
        if let ToClient::PcmChunk { data, .. } = &mut block {
            *data = base64::engine::general_purpose::STANDARD.encode([0; 8]);
        }
        for _ in 0..WINDOW_BLOCKS {
            frontend.receive(&block).unwrap();
        }
        assert!(frontend.receive(&block).is_err());
        assert!(frontend
            .receive(&ToClient::PcmEnd {
                session: 11,
                epoch: 3
            })
            .is_err());
    }
    /// Run with an isolated ALSA null configuration; never targets a desktop sink.
    #[test]
    #[ignore]
    fn local_device_drains_and_reports_progress() {
        assert!(
            std::env::var_os("STAR_PCM_DEVICE_TEST").is_some(),
            "Use an isolated test audio device"
        );
        let mut frontend = Frontend::default();
        frontend
            .receive(&ToClient::PcmOpen {
                session: 11,
                epoch: 3,
                spec: spec(),
                compatibility: false,
            })
            .unwrap();
        let (out, messages) = bounded(32);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut ready = false;
        while !ready {
            frontend.tick(&out);
            for event in messages.try_iter() {
                if let ClientMessage::Media { message } = event {
                    match message {
                        ToHost::PcmReady { .. } => ready = true,
                        ToHost::AudioError { message, .. } => panic!("{message}"),
                        _ => {}
                    }
                }
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        frontend
            .receive(&ToClient::PcmChunk {
                session: 11,
                epoch: 3,
                format: crate::pcm::SampleFormat::I32,
                processed: false,
                data: base64::engine::general_purpose::STANDARD
                    .encode(spec().encode(&vec![0.25; 1920]).unwrap()),
            })
            .unwrap();
        frontend
            .receive(&ToClient::PcmEnd {
                session: 11,
                epoch: 3,
            })
            .unwrap();
        let mut finished = false;
        let mut progress = 0;
        while !finished {
            frontend.tick(&out);
            for event in messages.try_iter() {
                if let ClientMessage::Media { message } = event {
                    match message {
                        ToHost::PcmProgress { frames, .. } => progress = frames,
                        ToHost::PcmFinished { .. } => finished = true,
                        ToHost::AudioError { message, .. } => panic!("{message}"),
                        _ => {}
                    }
                }
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(progress, 960);
    }
}
