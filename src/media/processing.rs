//! In-process color conversion and subtitle composition. No external player.
use super::*;
use av::{codec, format, frame, media};
use tracks::Selection;

/// FFmpeg filter arguments need escaping at both the option and graph layers.
fn filter_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    let option: String = value
        .chars()
        .flat_map(|c| {
            if matches!(c, '\\' | '\'' | ':') {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect();
    option
        .chars()
        .flat_map(|c| {
            if matches!(c, '\\' | '\'' | ',' | ';' | '[' | ']') {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect()
}

pub struct Processor {
    path: std::path::PathBuf,
    selection: Selection,
    graph: Option<av::filter::Graph>,
    bitmap: Option<BitmapSubtitles>,
    time: av::Rational,
    rotation: i32,
}
impl Processor {
    pub fn new(
        path: &Path,
        input: &format::context::Input,
        selection: &Selection,
        time: av::Rational,
        start: f64,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self> {
        let selection = tracks::subtitle_selection(input, selection);
        let mut bitmap = None;
        if let Selection::Stream(index) = &selection {
            let stream = input
                .stream(*index)
                .context("Subtitle track no longer exists")?;
            anyhow::ensure!(
                stream.parameters().medium() == media::Type::Subtitle,
                "Not a subtitle track"
            );
            if matches!(
                stream.parameters().id(),
                codec::Id::HDMV_PGS_SUBTITLE | codec::Id::DVD_SUBTITLE | codec::Id::DVB_SUBTITLE
            ) {
                bitmap = Some(BitmapSubtitles::open(path, *index, start, cancel)?);
            }
        }
        let mut rotation = 0;
        if let Some(s) = input.streams().best(media::Type::Video) {
            for data in s.side_data() {
                if data.kind() == av::packet::side_data::Type::DisplayMatrix
                    && data.data().len() >= 36
                {
                    // Copy to aligned storage before passing the matrix to FFmpeg.
                    let matrix: [i32; 9] = std::array::from_fn(|i| {
                        i32::from_ne_bytes(data.data()[i * 4..i * 4 + 4].try_into().unwrap())
                    });
                    let angle = unsafe { av::ffi::av_display_rotation_get(matrix.as_ptr()) };
                    if angle.is_finite() {
                        rotation = (-angle).round() as i32;
                    }
                }
            }
        }
        Ok(Self {
            path: path.into(),
            selection,
            graph: None,
            bitmap,
            time,
            rotation,
        })
    }
    pub fn process(&mut self, source: &frame::Video) -> Result<frame::Video> {
        if self.graph.is_none() {
            let hdr = matches!(
                source.color_transfer_characteristic(),
                av::color::TransferCharacteristic::SMPTE2084
                    | av::color::TransferCharacteristic::ARIB_STD_B67
            );
            let mut filters = vec![];
            if hdr {
                let matrix: av::ffi::AVColorSpace = source.color_space().into();
                let primaries: av::ffi::AVColorPrimaries = source.color_primaries().into();
                let transfer: av::ffi::AVColorTransferCharacteristic =
                    source.color_transfer_characteristic().into();
                let range = if source.color_range() == av::color::Range::JPEG {
                    "full"
                } else {
                    "limited"
                };
                filters.push(format!("zscale=min={}:pin={}:tin={}:rin={range}:t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=iec61966-2-1:m=gbr:r=full", matrix as i32, primaries as i32, transfer as i32));
            }
            if self.bitmap.is_none() {
                match &self.selection {
                    Selection::Stream(index) => {
                        let input = format::input(&self.path)?;
                        let relative = input
                            .streams()
                            .filter(|s| s.parameters().medium() == media::Type::Subtitle)
                            .position(|s| s.index() == *index)
                            .context("Subtitle track unavailable")?;
                        filters.push(format!(
                            "subtitles=filename={}:si={relative}",
                            filter_path(&self.path)
                        ));
                    }
                    Selection::External(path) => {
                        filters.push(format!("subtitles=filename={}", filter_path(path)))
                    }
                    _ => {}
                }
            }
            // Normalize sample aspect ratio before Kitty scales square pixels.
            let sar = source.aspect_ratio();
            if sar.numerator() > 0 && sar.denominator() > 0 && sar.numerator() != sar.denominator()
            {
                filters.push("scale=w=trunc(iw*sar):h=ih:flags=lanczos,setsar=1".into());
            }
            match self.rotation.rem_euclid(360) {
                90 => filters.push("transpose=clock".into()),
                180 => filters.push("hflip,vflip".into()),
                270 => filters.push("transpose=cclock".into()),
                _ => {}
            }
            if filters.is_empty() {
                filters.push("null".into());
            }
            let mut graph = av::filter::Graph::new();
            let sar = if sar.numerator() > 0 && sar.denominator() > 0 {
                sar
            } else {
                (1, 1).into()
            };
            graph.add(
                &av::filter::find("buffer").context("Video filters unavailable")?,
                "in",
                &format!(
                    "video_size={}x{}:pix_fmt={}:time_base={}:pixel_aspect={}",
                    source.width(),
                    source.height(),
                    av::ffi::AVPixelFormat::from(source.format()) as i32,
                    self.time,
                    sar
                ),
            )?;
            graph.add(
                &av::filter::find("buffersink").context("Video filter output unavailable")?,
                "out",
                "",
            )?;
            graph
                .output("in", 0)?
                .input("out", 0)?
                .parse(&filters.join(","))?;
            graph.validate()?;
            self.graph = Some(graph);
        }
        let graph = self.graph.as_mut().unwrap();
        graph.get("in").unwrap().source().add(source)?;
        let mut output = frame::Video::empty();
        graph.get("out").unwrap().sink().frame(&mut output)?;
        Ok(output)
    }
    pub fn compose(
        &mut self,
        pixels: &mut crate::image::RgbaImage,
        seconds: f64,
        source: (u32, u32),
    ) -> Result<()> {
        if let Some(bitmap) = &mut self.bitmap {
            bitmap.compose(pixels, seconds, source)?;
        }
        Ok(())
    }
}
struct Cue {
    start: f64,
    end: f64,
    images: Vec<(u32, u32, crate::image::RgbaImage)>,
}
struct BitmapSubtitles {
    input: format::context::Input,
    decoder: codec::decoder::Subtitle,
    index: usize,
    time: av::Rational,
    pending: Option<av::Packet>,
    cues: std::collections::VecDeque<Cue>,
}
impl BitmapSubtitles {
    fn open(path: &Path, index: usize, start: f64, cancel: Arc<AtomicBool>) -> Result<Self> {
        let mut input = format::input_with_interrupt(path, move || cancel.load(Ordering::Relaxed))?;
        let stream = input.stream(index).context("Subtitle stream unavailable")?;
        let time = stream.time_base();
        let decoder = codec::context::Context::from_parameters(stream.parameters())?
            .decoder()
            .subtitle()?;
        if start > 0.0 {
            input.seek(((start - 30.0).max(0.0) * 1_000_000.0) as i64, ..)?;
        }
        Ok(Self {
            input,
            decoder,
            index,
            time,
            pending: None,
            cues: Default::default(),
        })
    }
    fn compose(
        &mut self,
        pixels: &mut crate::image::RgbaImage,
        seconds: f64,
        source: (u32, u32),
    ) -> Result<()> {
        loop {
            let mut packet = self.pending.take().unwrap_or_else(av::Packet::empty);
            if packet.size() == 0 && packet.read(&mut self.input).is_err() {
                break;
            }
            if packet.stream() != self.index {
                continue;
            }
            let pts = packet.pts().unwrap_or(0) as f64 * f64::from(self.time);
            if pts > seconds + 0.5 {
                self.pending = Some(packet);
                break;
            }
            let mut sub = av::Subtitle::new();
            let result = (|| -> Result<()> {
                if self.decoder.decode(&packet, &mut sub)? {
                    let base = sub.pts().map_or(pts, |p| p as f64 / 1_000_000.0);
                    let start = base + f64::from(sub.start()) / 1000.0;
                    let end = if sub.end() == u32::MAX || sub.end() == 0 {
                        start + 30.0
                    } else {
                        base + f64::from(sub.end()) / 1000.0
                    };
                    let mut images = vec![];
                    for rect in sub.rects() {
                        if let codec::subtitle::Rect::Bitmap(b) = rect {
                            anyhow::ensure!(
                                u64::from(b.width()) * u64::from(b.height()) <= 16_000_000,
                                "Subtitle bitmap too large"
                            );
                            let raw = unsafe { &*b.as_ptr() };
                            if raw.data[0].is_null()
                                || raw.data[1].is_null()
                                || raw.linesize[0] < b.width() as i32
                            {
                                continue;
                            }
                            let mut image = crate::image::RgbaImage::new(b.width(), b.height());
                            for (x, y, p) in image.enumerate_pixels_mut() {
                                let color = unsafe {
                                    let i = *raw.data[0]
                                        .add(y as usize * raw.linesize[0] as usize + x as usize)
                                        as usize;
                                    if i >= b.colors() {
                                        0
                                    } else {
                                        std::ptr::read_unaligned(
                                            raw.data[1].add(i * 4).cast::<u32>(),
                                        )
                                    }
                                };
                                *p = crate::image::Rgba([
                                    (color >> 16) as u8,
                                    (color >> 8) as u8,
                                    color as u8,
                                    (color >> 24) as u8,
                                ]);
                            }
                            images.push((b.x() as u32, b.y() as u32, image));
                        }
                    }
                    if images.is_empty() {
                        for cue in &mut self.cues {
                            cue.end = cue.end.min(start);
                        }
                    } else {
                        self.cues.push_back(Cue { start, end, images });
                    }
                }
                Ok(())
            })();
            unsafe {
                av::ffi::avsubtitle_free(sub.as_mut_ptr());
            }
            result?;
            while self.cues.len() > 128 {
                self.cues.pop_front();
            }
        }
        self.cues.retain(|cue| cue.end > seconds);
        for cue in &self.cues {
            if seconds < cue.start {
                continue;
            }
            for (x, y, image) in &cue.images {
                let sx = pixels.width() as f64 / source.0.max(1) as f64;
                let sy = pixels.height() as f64 / source.1.max(1) as f64;
                let image = crate::image::imageops::resize(
                    image,
                    (image.width() as f64 * sx).round().max(1.0) as u32,
                    (image.height() as f64 * sy).round().max(1.0) as u32,
                    crate::image::imageops::FilterType::Lanczos3,
                );
                crate::image::imageops::overlay(
                    pixels,
                    &image,
                    (*x as f64 * sx).round() as i64,
                    (*y as f64 * sy).round() as i64,
                );
            }
        }
        Ok(())
    }
}
