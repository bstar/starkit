//! Bounded native media previews shared by STAR applications.
//! Codec contexts live on workers; the renderer receives decoded RGBA only.
use anyhow::{Context, Result};
use ffmpeg_next as av;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
};

pub mod hardware;
pub mod playback;
pub mod processing;
pub mod proxy;
pub mod tracks;

pub const CHUNK: usize = 32 * 1024;
pub const WINDOW: usize = 256 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Quality {
    Low,
    Balanced,
    High,
}
impl Quality {
    pub fn parameters(self) -> (u32, u32, u32) {
        match self {
            Self::Low => (360, 15, 750_000),
            Self::Balanced => (480, 24, 1_500_000),
            Self::High => (720, 30, 3_000_000),
        }
    }
    pub fn down(self) -> Self {
        match self {
            Self::High => Self::Balanced,
            _ => Self::Low,
        }
    }
    pub fn up(self) -> Self {
        match self {
            Self::Low => Self::Balanced,
            _ => Self::High,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Poster {
    pub pixels: Arc<crate::image::RgbaImage>,
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub audio: bool,
}
pub(crate) fn init() -> Result<()> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| {
        av::init().map_err(|e| e.to_string())?;
        av::log::set_level(if std::env::var_os("STAR_MEDIA_FFMPEG_LOG").is_some() {
            av::log::Level::Warning
        } else {
            av::log::Level::Quiet
        });
        Ok(())
    })
    .as_ref()
    .map_err(|e| anyhow::anyhow!(e.clone()))
    .copied()
}
pub(crate) fn video_decoder(
    parameters: av::codec::Parameters,
) -> Result<av::codec::decoder::Video> {
    let mut context = av::codec::context::Context::from_parameters(parameters)?;
    context.set_threading(av::codec::threading::Config::count(2));
    Ok(context.decoder().video()?)
}
pub(crate) fn dimensions(w: u32, h: u32, limit: u32) -> (u32, u32) {
    let factor = (f64::from(limit) / f64::from(h.max(1)))
        .min(1.0)
        .min(1280.0 / f64::from(w.max(1)));
    (
        ((f64::from(w) * factor) as u32 / 2 * 2).max(2),
        ((f64::from(h) * factor) as u32 / 2 * 2).max(2),
    )
}
// Stream headers may not describe a pixel format until the first decoded frame.
// Passing that unknown format to FFmpeg 9's scaler aborts the entire process.
pub(crate) fn scale_frame(
    frame: &av::frame::Video,
    scale: &mut Option<av::software::scaling::Context>,
    format: av::format::Pixel,
    width: u32,
    height: u32,
) -> Result<av::frame::Video> {
    anyhow::ensure!(
        frame.format() != av::format::Pixel::None
            && frame.width() > 0
            && frame.height() > 0
            && u64::from(frame.width()) * u64::from(frame.height()) <= 32_000_000,
        "Invalid decoded video format or dimensions"
    );
    if scale.as_ref().is_none_or(|context| {
        let input = context.input();
        input.format != frame.format()
            || input.width != frame.width()
            || input.height != frame.height()
            || context.output().width != width
            || context.output().height != height
            || context.output().format != format
    }) {
        *scale = Some(av::software::scaling::Context::get(
            frame.format(),
            frame.width(),
            frame.height(),
            format,
            width,
            height,
            av::software::scaling::Flags::LANCZOS,
        )?);
    }
    if format == av::format::Pixel::RGBA {
        let matrix = match frame.color_space() {
            av::color::Space::BT709 => av::ffi::SWS_CS_ITU709,
            av::color::Space::BT2020NCL | av::color::Space::BT2020CL => av::ffi::SWS_CS_BT2020,
            _ => av::ffi::SWS_CS_ITU601,
        };
        let range = i32::from(frame.color_range() == av::color::Range::JPEG);
        unsafe {
            let coefficients = av::ffi::sws_getCoefficients(matrix);
            av::ffi::sws_setColorspaceDetails(
                scale.as_mut().unwrap().as_mut_ptr(),
                coefficients,
                range,
                coefficients,
                1,
                0,
                1 << 16,
                1 << 16,
            );
        }
    }
    let mut out = av::frame::Video::empty();
    scale.as_mut().unwrap().run(frame, &mut out)?;
    Ok(out)
}
pub(crate) fn rgba(
    frame: &av::frame::Video,
    scale: &mut Option<av::software::scaling::Context>,
    bounds: (u32, u32),
) -> Result<crate::image::RgbaImage> {
    let out = scale_frame(frame, scale, av::format::Pixel::RGBA, bounds.0, bounds.1)?;
    let mut pixels = Vec::with_capacity(out.width() as usize * out.height() as usize * 4);
    for row in out
        .data(0)
        .chunks(out.stride(0))
        .take(out.height() as usize)
    {
        pixels.extend_from_slice(&row[..out.width() as usize * 4]);
    }
    crate::image::RgbaImage::from_raw(out.width(), out.height(), pixels)
        .context("Invalid decoded video dimensions")
}
pub fn poster(path: &Path, cancel: Arc<AtomicBool>) -> Result<Poster> {
    init()?;
    let stop = cancel.clone();
    let deadline = std::time::Instant::now();
    let mut input = av::format::input_with_interrupt(path, move || {
        stop.load(Ordering::Relaxed) || deadline.elapsed() > std::time::Duration::from_secs(3)
    })?;
    let stream = input
        .streams()
        .best(av::media::Type::Video)
        .context("No video track")?;
    let index = stream.index();
    let mut decoder = video_decoder(stream.parameters())?;
    anyhow::ensure!(
        decoder.width() > 0
            && decoder.height() > 0
            && u64::from(decoder.width()) * u64::from(decoder.height()) <= 32_000_000,
        "Video dimensions exceed preview limits"
    );
    let (w, h) = dimensions(decoder.width(), decoder.height(), 480);
    let mut scale = None;
    let duration = (input.duration() as f64 / 1_000_000.0).max(0.0);
    let audio = input.streams().best(av::media::Type::Audio).is_some();
    let started = std::time::Instant::now();
    for (s, p) in input.packets() {
        anyhow::ensure!(
            !cancel.load(Ordering::Relaxed) && started.elapsed().as_secs() < 3,
            "Video preview cancelled or timed out"
        );
        if s.index() != index {
            continue;
        }
        decoder.send_packet(&p)?;
        let mut frame = av::frame::Video::empty();
        if decoder.receive_frame(&mut frame).is_ok() {
            return Ok(Poster {
                pixels: Arc::new(rgba(&frame, &mut scale, (w, h))?),
                duration,
                width: decoder.width(),
                height: decoder.height(),
                audio,
            });
        }
    }
    anyhow::bail!("No decodable video frame")
}

/// A full-width seek track, using the shared native surface contract.
pub fn timeline(
    width: u16,
    height: u16,
    position: f64,
    duration: f64,
    background: String,
    track: String,
    accent: String,
) -> crate::native_surface::Surface {
    use crate::native_surface::{PixelRect, Primitive, Surface};
    let mut surface = Surface::new(width, height, background);
    // Keep a small gap on both sides of the progress track.
    let y = (height.saturating_sub(4) / 2)
        .saturating_add(2)
        .min(height.saturating_sub(4));
    surface.nodes.push(Primitive::Fill {
        rect: PixelRect::new(0, y, width, 4.min(height)),
        color: track,
        radius: 2,
    });
    let progress = ((position / duration.max(0.001)).clamp(0.0, 1.0) * f64::from(width)) as u16;
    surface.nodes.push(Primitive::Fill {
        rect: PixelRect::new(0, y, progress, 4.min(height)),
        color: accent,
        radius: 2,
    });
    surface
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scaling_waits_for_valid_frames_and_rebuilds_after_format_changes() {
        init().unwrap();
        let mut scaler = None;
        assert!(scale_frame(
            &av::frame::Video::empty(),
            &mut scaler,
            av::format::Pixel::RGBA,
            16,
            16
        )
        .is_err());
        assert!(scaler.is_none());
        for (format, width, height) in [
            (av::format::Pixel::YUV420P, 32, 24),
            (av::format::Pixel::YUV420P10LE, 48, 32),
            (av::format::Pixel::YUV420P, 32, 24),
        ] {
            let frame = av::frame::Video::new(format, width, height);
            let out = scale_frame(&frame, &mut scaler, av::format::Pixel::RGBA, 16, 16).unwrap();
            assert_eq!((out.width(), out.height()), (16, 16));
            assert_eq!(scaler.as_ref().unwrap().input().format, format);
        }
    }
    #[test]
    fn profiles_are_bounded_and_do_not_upscale() {
        assert_eq!(Quality::Balanced.parameters(), (480, 24, 1_500_000));
        assert_eq!(dimensions(320, 240, 720), (320, 240));
        assert_eq!(dimensions(1920, 1080, 480), (852, 480));
    }
}

#[cfg(all(test, feature = "terminal-graphics"))]
mod integration {
    use super::*;
    #[test]
    #[cfg(feature = "terminal-graphics")]
    fn native_poster_proxy_seek_and_playback() {
        // ffmpeg is only a fixture generator; production uses libav in process.
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.mkv");
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x240:rate=30",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=400:sample_rate=48000",
                "-t",
                "2",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "ac3",
                "-ac",
                "2",
                "-y",
            ])
            .arg(&source)
            .status();
        if status
            .as_ref()
            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        {
            return;
        }
        assert!(status.unwrap().success());
        let poster = poster(&source, Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!((poster.width, poster.height), (320, 240));
        assert!(poster.audio);
        assert!(poster.duration >= 1.9);
        // Each scrub creates a fresh proxy with timestamps relative to the
        // requested position. Verify audio keeps advancing after repeated seeks.
        for position in [0.8, 1.2, 0.2] {
            let output = dir.path().join("seek.ts");
            proxy::encode(
                &source,
                position,
                Quality::Balanced,
                std::fs::File::create(&output).unwrap(),
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
            let mut input = av::format::input(&output).unwrap();
            let stream = input.streams().best(av::media::Type::Audio).unwrap();
            let index = stream.index();
            let time = stream.time_base();
            let timestamps: Vec<_> = input
                .packets()
                .filter(|(s, _)| s.index() == index)
                .filter_map(|(_, p)| p.pts())
                .take(8)
                .map(|pts| pts as f64 * f64::from(time))
                .collect();
            assert!(timestamps.len() >= 4);
            assert!(timestamps.windows(2).all(|pair| pair[1] > pair[0]));
            assert!(
                timestamps[0] < 0.1,
                "Seek must reset proxy timestamps: {timestamps:?}"
            );
        }
        let proxy = dir.path().join("preview.ts");
        proxy::encode(
            &source,
            0.8,
            Quality::Low,
            std::fs::File::create(&proxy).unwrap(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        let input = av::format::input(&proxy).unwrap();
        let video = input.streams().best(av::media::Type::Video).unwrap();
        assert_eq!(video.parameters().id(), av::codec::Id::H264);
        assert_eq!(
            input
                .streams()
                .best(av::media::Type::Audio)
                .unwrap()
                .parameters()
                .id(),
            av::codec::Id::AAC
        );
        drop(input);
        let mut check = av::format::input(&proxy).unwrap();
        let audio = check.streams().best(av::media::Type::Audio).unwrap();
        let audio_index = audio.index();
        let audio_time = audio.time_base();
        let timestamps: Vec<_> = check
            .packets()
            .filter(|(stream, _)| stream.index() == audio_index)
            .filter_map(|(_, packet)| packet.pts())
            .take(4)
            .map(|pts| pts as f64 * f64::from(audio_time))
            .collect();
        assert_eq!(timestamps.len(), 4);
        assert!(
            timestamps[3] - timestamps[0] > 0.06,
            "MKV millisecond timestamps must be rescaled to AAC sample units: {timestamps:?}"
        );
        drop(check);
        let player = playback::Player::stream(std::fs::File::open(&proxy).unwrap(), 0.8).unwrap();
        let first = player
            .frames
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let second = player
            .frames
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(second.seconds > first.seconds);
        assert_eq!(first.pixels.dimensions(), (320, 240));
        assert_eq!(player.controls.volume.load(Ordering::Relaxed), 0);
        player.controls.paused.store(true, Ordering::Relaxed);
        let _ = player.frames.try_iter().last();
        assert!(player
            .frames
            .recv_timeout(std::time::Duration::from_millis(150))
            .is_err());

        let paused_controls = playback::Controls::new();
        paused_controls.paused.store(true, Ordering::Relaxed);
        let paused_seek = playback::Player::stream_with_controls(
            std::fs::File::open(&proxy).unwrap(),
            0.8,
            paused_controls,
        )
        .unwrap();
        paused_seek
            .frames
            .recv_timeout(std::time::Duration::from_secs(3))
            .expect("Paused seek must display its first frame");
        assert!(paused_seek
            .frames
            .recv_timeout(std::time::Duration::from_millis(150))
            .is_err());
        drop(paused_seek);

        let local_seek = playback::Player::file(source.clone(), 1.0).unwrap();
        let frame = local_seek
            .frames
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        assert!(
            frame.seconds >= 0.99,
            "Local seek must discard preceding keyframe video/audio"
        );
        drop(local_seek);
        // Exercise the same credit/session protocol used across SSH, including
        // a seek while old chunks are still queued.
        use crate::terminal_graphics::{
            media::{Frontend, Host, ToClient},
            protocol::{ClientMessage, ServerMessage},
        };
        let mut host = Host::new(source.clone(), "video".into(), 1, false);
        host.set_bounds(240, 180);
        let mut frontend = Frontend::new(false);
        let (send, receive) = crossbeam_channel::bounded(64);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        let mut frames = 0;
        let mut seeked = false;
        let mut second_generation = false;
        while std::time::Instant::now() < deadline {
            for message in host.effects().into_iter().chain(host.chunks()) {
                if let ServerMessage::Media { message } = message {
                    frontend.receive(message, &send).unwrap();
                }
            }
            if let Some((id, pixels)) = frontend.tick(&send) {
                frames += 1;
                assert!(pixels.height() <= 180);
                if seeked {
                    assert!(id.ends_with(&format!("-{}", host.generation)));
                    second_generation = true;
                }
            }
            for message in receive.try_iter() {
                if let ClientMessage::Media { message } = message {
                    host.receive(message);
                }
            }
            if frames >= 4 && !seeked {
                let generation = host.generation;
                host.restart(1.2);
                seeked = true;
                for message in host.effects() {
                    if let ServerMessage::Media { message } = message {
                        frontend.receive(message, &send).unwrap();
                    }
                }
                frontend
                    .receive(
                        ToClient::Chunk {
                            session: 1,
                            generation,
                            data: "not valid base64".into(),
                        },
                        &send,
                    )
                    .unwrap();
            }
            if second_generation {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            second_generation,
            "Credit-based proxy playback did not survive seek: {:?}",
            host.warning
        );
        assert_eq!(host.volume, 0);
    }
}
