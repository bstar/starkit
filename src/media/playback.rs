//! Local decoding, bounded frame scheduling and audio on the presentation machine.
use super::*;
use av::{codec, format, frame, media};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::{
    io::Read,
    sync::atomic::{AtomicU32, AtomicU64},
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct Controls {
    pub cancelled: Arc<AtomicBool>,
    pub paused: AtomicBool,
    pub volume: AtomicU32,
    pub position_ms: AtomicU64,
    pub buffering: AtomicBool,
    pub finished: AtomicBool,
    pub audio_active: AtomicBool,
    pub audio_samples: AtomicU64,
    pub audio_rate: AtomicU32,
    pub audio_origin_us: AtomicU64,
    pub dropped: AtomicU64,
    pub underruns: AtomicU64,
}
impl Controls {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            volume: AtomicU32::new(0),
            ..Self::default()
        })
    }
}
pub struct Frame {
    pub pixels: Arc<crate::image::RgbaImage>,
    pub seconds: f64,
}
pub struct Player {
    pub frames: Receiver<Frame>,
    pub notices: Receiver<String>,
    pub controls: Arc<Controls>,
}
impl Drop for Player {
    fn drop(&mut self) {
        self.controls.cancelled.store(true, Ordering::Relaxed);
    }
}
impl Player {
    pub fn file(path: std::path::PathBuf, start: f64) -> Result<Self> {
        Self::file_with_options(path, start, tracks::PlaybackOptions::default())
    }
    pub fn file_with_options(
        path: std::path::PathBuf,
        start: f64,
        options: tracks::PlaybackOptions,
    ) -> Result<Self> {
        let source = Some(path.clone());
        Self::spawn(
            move |stop| {
                format::input_with_interrupt(&path, move || stop.cancelled.load(Ordering::Relaxed))
            },
            start,
            Controls::new(),
            source,
            options,
        )
    }
    pub fn stream(reader: impl Read + Send + 'static, start: f64) -> Result<Self> {
        Self::stream_with_controls(reader, start, Controls::new())
    }
    pub fn stream_with_controls(
        reader: impl Read + Send + 'static,
        start: f64,
        controls: Arc<Controls>,
    ) -> Result<Self> {
        Self::spawn(
            move |stop| {
                let io = format::context::StreamIo::from_read(reader)?;
                let mut opts = av::Dictionary::new();
                opts.set("probesize", "32768");
                opts.set("analyzeduration", "100000");
                format::input_from_stream_with_interrupt(
                    io,
                    Some("preview.ts"),
                    Some(opts),
                    move || stop.cancelled.load(Ordering::Relaxed),
                )
            },
            start,
            controls,
            None,
            tracks::PlaybackOptions::default(),
        )
    }
    fn spawn(
        open: impl FnOnce(Arc<Controls>) -> std::result::Result<format::context::Input, av::Error>
            + Send
            + 'static,
        start: f64,
        controls: Arc<Controls>,
        source: Option<std::path::PathBuf>,
        options: tracks::PlaybackOptions,
    ) -> Result<Self> {
        init()?;
        let ctrl = controls.clone();
        let (decoded, queue) = bounded::<Frame>(1);
        let (show, frames) = bounded(1);
        let old = frames.clone();
        let (notice, notices) = bounded(8);
        let c = ctrl.clone();
        let clock_notice = notice.clone();
        std::thread::Builder::new()
            .name("star-video-clock".into())
            .spawn(move || schedule_frames(queue, show, old, c, clock_notice, start))?;
        std::thread::Builder::new()
            .name("star-video-decoder".into())
            .spawn(move || {
                let open_stop = ctrl.clone();
                let result = (|| -> Result<()> {
                    let mut input = open(open_stop)?;
                    // The custom stream also observes cancellation while waiting for data.
                    let s = input
                        .streams()
                        .best(media::Type::Video)
                        .context("No video stream")?;
                    let index = s.index();
                    let time = s.time_base();
                    let mut video = if source.is_some() {
                        hardware::decoder(s.parameters())?
                    } else {
                        video_decoder(s.parameters())?
                    };
                    anyhow::ensure!(
                        u64::from(video.width()) * u64::from(video.height()) <= 32_000_000,
                        "Video exceeds preview limits"
                    );
                    let mut processor = if let Some(path) = &source {
                        Some(processing::Processor::new(
                            path,
                            &input,
                            &options.subtitle,
                            time,
                            start,
                            ctrl.cancelled.clone(),
                        )?)
                    } else {
                        None
                    };
                    let native = source.is_some();
                    let mut scale = None;
                    let mut sound = tracks::audio_index(&input, &options.audio)
                        .and_then(|i| input.stream(i))
                        .and_then(|s| {
                            let mut decoder =
                                codec::context::Context::from_parameters(s.parameters())
                                    .ok()?
                                    .decoder()
                                    .audio()
                                    .ok()?;
                            if decoder.channel_layout().is_empty() {
                                decoder.set_channel_layout(av::ChannelLayout::default(i32::from(
                                    decoder.channels(),
                                )));
                            }
                            match Audio::open(&decoder, ctrl.clone()) {
                                Ok(output) => {
                                    let _ = notice.try_send(format!(
                                        "Audio · {} Hz · {} channels{}",
                                        output.rate,
                                        output.channels,
                                        if output.rate != decoder.rate()
                                            || output.channels != decoder.channels() as usize
                                        {
                                            " · device conversion"
                                        } else {
                                            " · source format"
                                        }
                                    ));
                                    Some((s.index(), s.time_base(), decoder, output))
                                }
                                Err(e) => {
                                    let _ = notice.try_send(format!(
                                        "Audio unavailable; playing silently: {e:#}"
                                    ));
                                    None
                                }
                            }
                        });
                    let local = source.is_some();
                    if start > 0.0 && local {
                        input.seek((start * 1_000_000.0) as i64, ..)?;
                    }
                    let mut receive = |decoder: &mut codec::decoder::Video| -> Result<()> {
                        let mut f = frame::Video::empty();
                        while decoder.receive_frame(&mut f).is_ok() {
                            let f = hardware::download(&f)?;
                            let seconds = f.timestamp().unwrap_or(0) as f64 * f64::from(time);
                            if local && seconds + 0.001 < start {
                                continue;
                            }
                            let started = Instant::now();
                            let processed = if let Some(processor) = &mut processor {
                                processor.process(&f)?
                            } else {
                                hardware::download(&f)?
                            };
                            let bounds = if native {
                                (processed.width(), processed.height())
                            } else {
                                dimensions(processed.width(), processed.height(), 720)
                            };
                            let mut pixels = rgba(&processed, &mut scale, bounds)?;
                            if let Some(processor) = &mut processor {
                                processor.compose(&mut pixels, seconds, (f.width(), f.height()))?;
                            }
                            tracing::trace!(
                                decode_convert_us = started.elapsed().as_micros(),
                                width = bounds.0,
                                height = bounds.1,
                                "Video decoded"
                            );
                            let mut item = Frame {
                                pixels: Arc::new(pixels),
                                seconds,
                            };
                            loop {
                                if ctrl.cancelled.load(Ordering::Relaxed) {
                                    anyhow::bail!("Cancelled");
                                }
                                match decoded.send_timeout(item, Duration::from_millis(20)) {
                                    Ok(()) => break,
                                    Err(crossbeam_channel::SendTimeoutError::Timeout(f)) => {
                                        item = f
                                    }
                                    Err(_) => anyhow::bail!("Playback closed"),
                                }
                            }
                        }
                        Ok(())
                    };
                    for (s, p) in input.packets() {
                        if ctrl.cancelled.load(Ordering::Relaxed) {
                            return Ok(());
                        }
                        if s.index() == index {
                            video.send_packet(&p)?;
                            receive(&mut video)?;
                        } else if let Some((i, audio_time, d, o)) = &mut sound {
                            if s.index() == *i {
                                d.send_packet(&p)?;
                                o.drain(d, if local { start } else { 0.0 }, *audio_time)?;
                            }
                        }
                    }
                    video.send_eof()?;
                    receive(&mut video)?;
                    drop(decoded);
                    if let Some((_, audio_time, decoder, output)) = &mut sound {
                        decoder.send_eof()?;
                        output.drain(decoder, if local { start } else { 0.0 }, *audio_time)?;
                    }
                    while !ctrl.finished.load(Ordering::Relaxed)
                        && !ctrl.cancelled.load(Ordering::Relaxed)
                    {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Ok(())
                })();
                if let Err(e) = result {
                    if !ctrl.cancelled.load(Ordering::Relaxed) {
                        let _ = notice.try_send(format!("Video preview: {e:#}"));
                    }
                }
            })?;
        Ok(Self {
            frames,
            notices,
            controls,
        })
    }
}
struct Audio {
    _stream: cpal::Stream,
    producer: rtrb::Producer<f32>,
    resample: av::software::resampling::Context,
    ctrl: Arc<Controls>,
    rate: u32,
    channels: usize,
}
impl Audio {
    fn open(decoder: &codec::decoder::Audio, ctrl: Arc<Controls>) -> Result<Self> {
        let device = cpal::default_host()
            .default_output_device()
            .context("No audio device")?;
        let preferred = device.default_output_config()?;
        let supported = device
            .supported_output_configs()?
            .filter(|c| c.channels() <= 8)
            .map(|c| {
                let rate = decoder
                    .rate()
                    .clamp(c.min_sample_rate().0, c.max_sample_rate().0);
                let score = (u32::from(c.channels().abs_diff(decoder.channels())) * 100_000_000)
                    + rate.abs_diff(decoder.rate())
                    + if c.sample_format() == cpal::SampleFormat::F32 {
                        0
                    } else {
                        1
                    };
                (score, c.with_sample_rate(cpal::SampleRate(rate)))
            })
            .min_by_key(|(score, _)| *score)
            .map_or(preferred, |(_, c)| c);
        let config = supported.config();
        let channels = config.channels as usize;
        let rate = config.sample_rate.0;
        anyhow::ensure!(channels <= 8, "Unsupported audio device channel count");
        let (producer, consumer) = rtrb::RingBuffer::new(rate as usize * channels / 2);
        let mut consumer = Some(consumer);
        let c = ctrl.clone();
        ctrl.audio_rate.store(rate, Ordering::Relaxed);
        let error = |e| tracing::warn!(%e,"Video audio device error");
        macro_rules! stream {
            ($ty:ty) => {{
                let mut ring = consumer.take().unwrap();
                let c = c.clone();
                device.build_output_stream(
                    &config,
                    move |data: &mut [$ty], _| {
                        let gain = c.volume.load(Ordering::Relaxed).min(100) as f32 / 100.0;
                        let paused = c.paused.load(Ordering::Relaxed);
                        let mut delivered = 0u64;
                        for sample in data {
                            let value = if paused {
                                0.0
                            } else {
                                match ring.pop() {
                                    Ok(value) => {
                                        delivered += 1;
                                        value * gain
                                    }
                                    Err(_) => 0.0,
                                }
                            };
                            *sample = <$ty as cpal::FromSample<f32>>::from_sample_(value);
                        }
                        c.audio_samples
                            .fetch_add(delivered / channels as u64, Ordering::Relaxed);
                        if !paused && delivered == 0 && c.audio_active.load(Ordering::Relaxed) {
                            c.underruns.fetch_add(1, Ordering::Relaxed);
                        }
                    },
                    error,
                    None,
                )?
            }};
        }
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => stream!(f32),
            cpal::SampleFormat::I16 => stream!(i16),
            cpal::SampleFormat::U16 => stream!(u16),
            cpal::SampleFormat::I32 => stream!(i32),
            cpal::SampleFormat::F64 => stream!(f64),
            _ => anyhow::bail!("Unsupported audio sample format"),
        };
        stream.play()?;
        let resample = av::software::resampling::Context::get(
            decoder.format(),
            decoder.channel_layout(),
            decoder.rate(),
            format::Sample::F32(format::sample::Type::Packed),
            av::ChannelLayout::default(channels as i32),
            rate,
        )?;
        Ok(Self {
            _stream: stream,
            producer,
            resample,
            ctrl,
            rate,
            channels,
        })
    }
    fn drain(
        &mut self,
        decoder: &mut codec::decoder::Audio,
        start: f64,
        time: av::Rational,
    ) -> Result<()> {
        let mut f = frame::Audio::empty();
        while decoder.receive_frame(&mut f).is_ok() {
            if start > 0.0
                && f.timestamp()
                    .is_some_and(|pts| pts as f64 * f64::from(time) + 0.001 < start)
            {
                continue;
            }
            if !self.ctrl.audio_active.load(Ordering::Relaxed) {
                let origin = f.timestamp().unwrap_or(0) as f64 * f64::from(time);
                self.ctrl
                    .audio_origin_us
                    .store((origin.max(0.0) * 1_000_000.0) as u64, Ordering::Relaxed);
                self.ctrl.audio_active.store(true, Ordering::Relaxed);
            }
            let mut out = frame::Audio::empty();
            self.resample.run(&f, &mut out)?;
            let bytes = &out.data(0)[..out.samples() * out.channels() as usize * 4];
            for b in bytes.as_chunks::<4>().0 {
                let mut sample = f32::from_ne_bytes(*b);
                loop {
                    if self.ctrl.cancelled.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    match self.producer.push(sample) {
                        Ok(()) => break,
                        Err(rtrb::PushError::Full(v)) => {
                            sample = v;
                            std::thread::sleep(Duration::from_millis(2));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Blocking, bounded stream adapter. Only its decoder worker waits for data.
pub struct StreamReader {
    pub receiver: Receiver<Option<Vec<u8>>>,
    pub cancel: Arc<Controls>,
    pub consumed: Sender<usize>,
    pub current: std::io::Cursor<Vec<u8>>,
}
impl Read for StreamReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if self.cancel.cancelled.load(Ordering::Relaxed) {
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            let n = self.current.read(out)?;
            if n > 0 {
                return Ok(n);
            }
            match self.receiver.recv_timeout(Duration::from_millis(20)) {
                Ok(Some(data)) => {
                    let _ = self.consumed.try_send(data.len());
                    self.current = std::io::Cursor::new(data);
                }
                Ok(None) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return Ok(0),
                Err(_) => {}
            }
        }
    }
}

// The first timed-out receive is part of the stall too. Excluding it moves
// the video clock ahead by 50 ms and presents buffered frames in a burst.
struct FrameClock {
    base: Instant,
    buffering_since: Option<Instant>,
}
impl FrameClock {
    fn new(base: Instant) -> Self {
        Self {
            base,
            buffering_since: None,
        }
    }
    fn starved(&mut self, wait_started: Instant) {
        self.buffering_since.get_or_insert(wait_started);
    }
    fn resume(&mut self, now: Instant) {
        if let Some(started) = self.buffering_since.take() {
            self.base += now.duration_since(started);
        }
    }
}

fn schedule_frames(
    queue: Receiver<Frame>,
    show: Sender<Frame>,
    old: Receiver<Frame>,
    c: Arc<Controls>,
    clock_notice: Sender<String>,
    start: f64,
) {
    let mut clock = FrameClock::new(Instant::now());
    let mut first = None;
    let mut last_audio_samples = 0;
    let mut audio_progress = Instant::now();
    while !c.cancelled.load(Ordering::Relaxed) {
        let waiting_since = Instant::now();
        let f = match queue.recv_timeout(Duration::from_millis(50)) {
            Ok(f) => f,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                c.buffering.store(true, Ordering::Relaxed);
                clock.starved(waiting_since);
                continue;
            }
            Err(_) => break,
        };
        c.buffering.store(false, Ordering::Relaxed);
        clock.resume(Instant::now());
        // A paused seek still presents its first decoded frame.
        let initial = first.is_none();
        let origin = *first.get_or_insert(f.seconds);
        if first == Some(f.seconds) && c.position_ms.load(Ordering::Relaxed) == 0 {
            clock.base = Instant::now();
        }
        loop {
            if c.cancelled.load(Ordering::Relaxed) {
                return;
            }
            if !initial && c.paused.load(Ordering::Relaxed) {
                let t = Instant::now();
                std::thread::sleep(Duration::from_millis(10));
                audio_progress = Instant::now();
                clock.base += t.elapsed();
                continue;
            }
            let target = Duration::from_secs_f64((f.seconds - origin).max(0.0));
            let samples = c.audio_samples.load(Ordering::Relaxed);
            if samples != last_audio_samples {
                last_audio_samples = samples;
                audio_progress = Instant::now();
            }
            let audio_time = c.audio_origin_us.load(Ordering::Relaxed) as f64 / 1_000_000.0
                + c.audio_samples.load(Ordering::Relaxed) as f64
                    / f64::from(c.audio_rate.load(Ordering::Relaxed).max(1));
            let stalled = audio_progress.elapsed() > Duration::from_millis(250);
            let effective_audio = audio_time
                + if stalled {
                    audio_progress.elapsed().as_secs_f64()
                } else {
                    0.0
                };
            if (c.audio_active.load(Ordering::Relaxed) && effective_audio + 0.002 >= f.seconds)
                || (!c.audio_active.load(Ordering::Relaxed) && clock.base.elapsed() >= target)
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        c.position_ms.store(
            ((start + f.seconds - origin) * 1000.0) as u64,
            Ordering::Relaxed,
        );
        if let Err(crossbeam_channel::TrySendError::Full(f)) = show.try_send(f) {
            let _ = old.try_recv();
            c.dropped.fetch_add(1, Ordering::Relaxed);
            let _ = show.try_send(f);
        }
    }
    c.finished.store(true, Ordering::Relaxed);
    let _ = clock_notice.try_send("Playback finished".into());
}

#[cfg(test)]
mod clock_tests {
    use super::*;
    #[test]
    fn buffering_recovery_keeps_frames_paced() {
        let start = Instant::now();
        let mut clock = FrameClock::new(start);
        // Last frame was at 40 ms; the next receive times out at 90 ms.
        // A second timeout must not replace that first wait's origin.
        clock.starved(start + Duration::from_millis(40));
        clock.starved(start + Duration::from_millis(90));
        let recovered = start + Duration::from_millis(140);
        clock.resume(recovered);
        assert_eq!(clock.base + Duration::from_millis(40), recovered);
        assert_eq!(
            clock.base + Duration::from_millis(80),
            recovered + Duration::from_millis(40)
        );
        // Normal receives do not shift the clock again.
        clock.resume(recovered + Duration::from_millis(1));
        assert_eq!(
            clock.base + Duration::from_millis(80),
            recovered + Duration::from_millis(40)
        );
    }
}
