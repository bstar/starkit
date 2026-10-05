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
    pub cancelled: AtomicBool,
    pub paused: AtomicBool,
    pub volume: AtomicU32,
    pub position_ms: AtomicU64,
    pub buffering: AtomicBool,
    pub finished: AtomicBool,
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
        Self::spawn(
            move |stop| {
                format::input_with_interrupt(&path, move || stop.cancelled.load(Ordering::Relaxed))
            },
            start,
            Controls::new(),
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
        )
    }
    fn spawn(
        open: impl FnOnce(Arc<Controls>) -> std::result::Result<format::context::Input, av::Error>
            + Send
            + 'static,
        start: f64,
        controls: Arc<Controls>,
    ) -> Result<Self> {
        init()?;
        let ctrl = controls.clone();
        let (decoded, queue) = bounded::<Frame>(3);
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
                    let mut video = video_decoder(s.parameters())?;
                    anyhow::ensure!(
                        u64::from(video.width()) * u64::from(video.height()) <= 32_000_000,
                        "Video exceeds preview limits"
                    );
                    let (w, h) = dimensions(video.width(), video.height(), 720);
                    let mut scale = None;
                    let mut sound = input.streams().best(media::Type::Audio).and_then(|s| {
                        let mut decoder = codec::context::Context::from_parameters(s.parameters())
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
                            Ok(output) => Some((s.index(), s.time_base(), decoder, output)),
                            Err(e) => {
                                let _ = notice.try_send(format!(
                                    "Audio unavailable; playing silently: {e:#}"
                                ));
                                None
                            }
                        }
                    });
                    let local = input.format().name() != "mpegts";
                    if start > 0.0 && local {
                        input.seek((start * 1_000_000.0) as i64, ..)?;
                    }
                    let mut receive = |decoder: &mut codec::decoder::Video| -> Result<()> {
                        let mut f = frame::Video::empty();
                        while decoder.receive_frame(&mut f).is_ok() {
                            let seconds = f.timestamp().unwrap_or(0) as f64 * f64::from(time);
                            if local && seconds + 0.001 < start {
                                continue;
                            }
                            let mut item = Frame {
                                pixels: Arc::new(rgba(&f, &mut scale, (w, h))?),
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
}
impl Audio {
    fn open(decoder: &codec::decoder::Audio, ctrl: Arc<Controls>) -> Result<Self> {
        let device = cpal::default_host()
            .default_output_device()
            .context("No audio device")?;
        let supported = device.default_output_config()?;
        let config = supported.config();
        let channels = config.channels as usize;
        let rate = config.sample_rate.0;
        anyhow::ensure!(channels <= 8, "Unsupported audio device channel count");
        let (producer, consumer) = rtrb::RingBuffer::new(rate as usize * channels / 2);
        let mut consumer = Some(consumer);
        let c = ctrl.clone();
        let error = |e| tracing::warn!(%e,"Video audio device error");
        macro_rules! stream {
            ($ty:ty) => {{
                let mut ring = consumer.take().unwrap();
                let c = c.clone();
                device.build_output_stream(
                    &config,
                    move |data: &mut [$ty], _| {
                        let gain = c.volume.load(Ordering::Relaxed).min(100) as f32 / 100.0;
                        let paused =
                            c.paused.load(Ordering::Relaxed) || c.buffering.load(Ordering::Relaxed);
                        for sample in data {
                            let value = if paused {
                                0.0
                            } else {
                                ring.pop().unwrap_or(0.0) * gain
                            };
                            *sample = <$ty as cpal::FromSample<f32>>::from_sample_(value);
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
                clock.base += t.elapsed();
                continue;
            }
            let target = Duration::from_secs_f64((f.seconds - origin).max(0.0));
            if clock.base.elapsed() >= target {
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
