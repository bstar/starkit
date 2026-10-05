//! Low latency MPEG-TS / H.264 / AAC preview encoder. Never downloads a file.
use super::*;
use av::{codec, encoder, format, frame, media, Dictionary, Packet, Rational};
use std::io::Write;

struct Video {
    decoder: codec::decoder::Video,
    encoder: encoder::Video,
    scale: Option<av::software::scaling::Context>,
    index: usize,
    time: Rational,
    last: i64,
    start: f64,
    fps: u32,
}
impl Video {
    fn drain(&mut self, out: &mut format::context::Output) -> Result<()> {
        let mut f = frame::Video::empty();
        while self.decoder.receive_frame(&mut f).is_ok() {
            let seconds = f.timestamp().unwrap_or(0) as f64 * f64::from(self.time) - self.start;
            if seconds < -0.001 {
                continue;
            }
            let pts = (seconds * f64::from(self.fps)).round() as i64;
            if pts <= self.last {
                continue;
            }
            self.last = pts;
            let mut resized = scale_frame(
                &f,
                &mut self.scale,
                format::Pixel::YUV420P,
                self.encoder.width(),
                self.encoder.height(),
            )?;
            resized.set_pts(Some(pts));
            resized.set_kind(av::picture::Type::None);
            self.encoder.send_frame(&resized)?;
            self.packets(out)?;
        }
        Ok(())
    }
    fn packets(&mut self, out: &mut format::context::Output) -> Result<()> {
        let mut p = Packet::empty();
        while self.encoder.receive_packet(&mut p).is_ok() {
            p.set_stream(0);
            p.rescale_ts((1, self.fps as i32), out.stream(0).unwrap().time_base());
            p.write_interleaved(out)?;
        }
        Ok(())
    }
}
struct Audio {
    decoder: codec::decoder::Audio,
    encoder: encoder::Audio,
    filter: av::filter::Graph,
    index: usize,
    time: Rational,
    start: f64,
}
impl Audio {
    fn drain(&mut self, out: &mut format::context::Output) -> Result<()> {
        let mut f = frame::Audio::empty();
        while self.decoder.receive_frame(&mut f).is_ok() {
            let seconds = f.timestamp().unwrap_or(0) as f64 * f64::from(self.time) - self.start;
            if seconds < -0.001 {
                continue;
            }
            f.set_pts(Some((seconds / f64::from(self.time)).round() as i64));
            self.filter.get("in").unwrap().source().add(&f)?;
            self.filtered(out)?;
        }
        Ok(())
    }
    fn filtered(&mut self, out: &mut format::context::Output) -> Result<()> {
        let mut f = frame::Audio::empty();
        while self.filter.get("out").unwrap().sink().frame(&mut f).is_ok() {
            anyhow::ensure!(
                f.format() == self.encoder.format()
                    && f.channels() == self.encoder.channels()
                    && f.rate() == self.encoder.rate(),
                "Audio filter returned an incompatible frame"
            );
            self.encoder.send_frame(&f)?;
            self.packets(out)?;
        }
        Ok(())
    }
    fn packets(&mut self, out: &mut format::context::Output) -> Result<()> {
        let mut p = Packet::empty();
        while self.encoder.receive_packet(&mut p).is_ok() {
            p.set_stream(1);
            p.rescale_ts((1, 48000), out.stream(1).unwrap().time_base());
            p.write_interleaved(out)?;
        }
        Ok(())
    }
}
pub fn encode(
    path: &Path,
    start: f64,
    quality: Quality,
    writer: impl Write + Send + 'static,
    cancel: Arc<AtomicBool>,
) -> Result<()> {
    encode_to_fit(path, start, quality, (1280, 720), writer, cancel)
}
pub fn encode_to_fit(
    path: &Path,
    start: f64,
    quality: Quality,
    bounds: (u32, u32),
    writer: impl Write + Send + 'static,
    cancel: Arc<AtomicBool>,
) -> Result<()> {
    init()?;
    let stop = cancel.clone();
    let mut input = format::input_with_interrupt(path, move || stop.load(Ordering::Relaxed))?;
    let s = input
        .streams()
        .best(media::Type::Video)
        .context("No video stream")?;
    let index = s.index();
    let time = s.time_base();
    let decoder = video_decoder(s.parameters())?;
    anyhow::ensure!(
        u64::from(decoder.width()) * u64::from(decoder.height()) <= 32_000_000,
        "Video exceeds preview limits"
    );
    let (height, fps, rate) = quality.parameters();
    let width_height = (f64::from(bounds.0.max(2)) * f64::from(decoder.height())
        / f64::from(decoder.width().max(1))) as u32;
    let (w, h) = dimensions(
        decoder.width(),
        decoder.height(),
        height.min(bounds.1.max(2)).min(width_height.max(2)),
    );
    let io = format::context::StreamIo::from_write(writer)?;
    let mut output = format::output_to_stream(io, None, Some("mpegts"))?;
    let codec = encoder::find_by_name("libx264").context("H.264 preview encoder unavailable")?;
    let mut enc = codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()?;
    enc.set_threading(codec::threading::Config::count(2));
    enc.set_width(w);
    enc.set_height(h);
    enc.set_format(format::Pixel::YUV420P);
    enc.set_time_base((1, fps as i32));
    enc.set_frame_rate(Some((fps as i32, 1)));
    enc.set_bit_rate(rate as usize);
    enc.set_max_bit_rate(rate as usize);
    enc.set_gop(fps);
    enc.set_max_b_frames(0);
    let mut opts = Dictionary::new();
    opts.set("preset", "ultrafast");
    opts.set("tune", "zerolatency");
    opts.set(
        "x264-params",
        &format!(
            "vbv-maxrate={}:vbv-bufsize={}:scenecut=0",
            rate / 1000,
            rate / 2000
        ),
    );
    let enc = enc.open_with(opts)?;
    output.add_stream(codec)?.set_parameters(&enc);
    let mut video = Video {
        decoder,
        encoder: enc,
        scale: None,
        index,
        time,
        last: -1,
        start,
        fps,
    };
    let mut audio = if let Some(s) = input.streams().best(media::Type::Audio) {
        let index = s.index();
        let time = s.time_base();
        let mut decoder = codec::context::Context::from_parameters(s.parameters())?
            .decoder()
            .audio()?;
        if decoder.channel_layout().is_empty() {
            decoder.set_channel_layout(av::ChannelLayout::default(i32::from(decoder.channels())));
        }
        let codec = encoder::find(codec::Id::AAC).context("AAC encoder unavailable")?;
        let mut enc = codec::context::Context::new_with_codec(codec)
            .encoder()
            .audio()?;
        enc.set_rate(48000);
        enc.set_channel_layout(av::ChannelLayout::STEREO);
        enc.set_format(format::Sample::F32(format::sample::Type::Planar));
        enc.set_time_base((1, 48000));
        enc.set_bit_rate(96000);
        let enc = enc.open_as(codec)?;
        output.add_stream(codec)?.set_parameters(&enc);
        let mut filter = av::filter::Graph::new();
        filter.add(
            &av::filter::find("abuffer").context("Audio filter unavailable")?,
            "in",
            &format!(
                "time_base={}:sample_rate={}:sample_fmt={}:channel_layout=0x{:x}",
                time,
                decoder.rate(),
                decoder.format().name(),
                decoder.channel_layout().bits()
            ),
        )?;
        filter.add(&av::filter::find("abuffersink").unwrap(), "out", "")?;
        {
            let mut sink = filter.get("out").unwrap();
            sink.set_sample_format(enc.format());
            sink.set_channel_layout(enc.channel_layout());
            sink.set_sample_rate(enc.rate());
        }
        filter
            .output("in", 0)?
            .input("out", 0)?
            .parse("aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo")?;
        filter.validate()?;
        filter
            .get("out")
            .unwrap()
            .sink()
            .set_frame_size(enc.frame_size());
        Some(Audio {
            decoder,
            encoder: enc,
            filter,
            index,
            time,
            start,
        })
    } else {
        None
    };
    let mut options = Dictionary::new();
    options.set("flush_packets", "1");
    options.set("muxdelay", "0");
    options.set("mpegts_flags", "resend_headers");
    output.write_header_with(options)?;
    if start > 0.0 {
        input.seek((start * 1_000_000.0) as i64, ..)?;
    }
    for (s, p) in input.packets() {
        anyhow::ensure!(!cancel.load(Ordering::Relaxed), "Video cancelled");
        if s.index() == video.index {
            video.decoder.send_packet(&p)?;
            video.drain(&mut output)?;
        } else if let Some(a) = audio.as_mut() {
            if s.index() == a.index {
                a.decoder.send_packet(&p)?;
                a.drain(&mut output)?;
            }
        }
    }
    video.decoder.send_eof()?;
    video.drain(&mut output)?;
    video.encoder.send_eof()?;
    video.packets(&mut output)?;
    if let Some(a) = audio.as_mut() {
        a.decoder.send_eof()?;
        a.drain(&mut output)?;
        a.filter.get("in").unwrap().source().flush()?;
        a.filtered(&mut output)?;
        a.encoder.send_eof()?;
        a.packets(&mut output)?;
    }
    output.write_trailer()?;
    Ok(())
}
