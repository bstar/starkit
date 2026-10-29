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
    preview_bounds: Option<(u32, u32)>,
    stream_aspect: Option<av::Rational>,
    graph_aspect: Option<av::Rational>,
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
        let mut stream_aspect = None;
        if let Some(s) = input.streams().best(media::Type::Video) {
            // Match av_guess_sample_aspect_ratio: valid container metadata
            // precedes frame metadata. Some FFmpeg decoders report square or
            // unknown pixels even when the container declares anamorphic video.
            // SAFETY: the selected stream is borrowed from this live input.
            let aspect = unsafe { av::Rational::from((*s.as_ptr()).sample_aspect_ratio) };
            stream_aspect = (aspect.numerator() > 0 && aspect.denominator() > 0).then_some(aspect);
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
            preview_bounds: None,
            stream_aspect,
            graph_aspect: None,
        })
    }
    pub(super) fn set_preview_bounds(&mut self, width: u32, height: u32) {
        let bounds = Some((width, height));
        if self.preview_bounds != bounds {
            self.preview_bounds = bounds;
            self.graph = None;
        }
    }
    fn aspect_ratio(&self, source: &frame::Video) -> av::Rational {
        let frame = source.aspect_ratio();
        self.stream_aspect.unwrap_or_else(|| {
            if frame.numerator() > 0 && frame.denominator() > 0 {
                frame
            } else {
                (1, 1).into()
            }
        })
    }
    pub(super) fn display_dimensions(
        &self,
        width: u32,
        height: u32,
        fallback: av::Rational,
    ) -> Result<(u32, u32)> {
        let aspect = self.stream_aspect.unwrap_or(fallback);
        let aspect = if aspect.numerator() > 0 && aspect.denominator() > 0 {
            f64::from(aspect)
        } else {
            1.0
        };
        let width = (f64::from(width) * aspect).round();
        anyhow::ensure!(
            width.is_finite() && width >= 1.0 && width <= f64::from(u32::MAX),
            "Invalid display aspect ratio"
        );
        let mut dimensions = (width as u32, height);
        if matches!(self.rotation.rem_euclid(360), 90 | 270) {
            std::mem::swap(&mut dimensions.0, &mut dimensions.1);
        }
        Ok(dimensions)
    }
    /// Fit before float HDR conversion, retaining source pixels for fullscreen.
    /// Bounds are presentation hints, not a change to the original media stream.
    pub(super) fn fit_output(&mut self, source: &frame::Video, bounds: u64) {
        if bounds == 0 {
            if self.preview_bounds.take().is_some() {
                self.graph = None;
            }
            return;
        }
        let (mut width, mut height) = ((bounds >> 32) as u32, bounds as u32);
        if matches!(self.rotation.rem_euclid(360), 90 | 270) {
            std::mem::swap(&mut width, &mut height);
        }
        let aspect = f64::from(self.aspect_ratio(source));
        let factor = (f64::from(width) / (f64::from(source.width()) * aspect))
            .min(f64::from(height) / f64::from(source.height()))
            .min(1.0);
        let width = ((f64::from(source.width()) * factor) as u32 / 2 * 2).max(2);
        let height = ((f64::from(source.height()) * factor) as u32 / 2 * 2).max(2);
        self.set_preview_bounds(width, height);
    }
    pub fn process(&mut self, source: &frame::Video) -> Result<frame::Video> {
        let sar = self.aspect_ratio(source);
        let height = self
            .preview_bounds
            .map_or(source.height(), |(_, height)| height);
        let width = f64::from(source.width()) * f64::from(sar) * f64::from(height)
            / f64::from(source.height().max(1));
        anyhow::ensure!(
            width >= 1.0 && height > 0 && width * f64::from(height) <= 32_000_000.0,
            "Normalized video dimensions exceed preview limits"
        );
        if self.graph_aspect != Some(sar) {
            self.graph = None;
        }
        if self.graph.is_none() {
            let hdr = matches!(
                source.color_transfer_characteristic(),
                av::color::TransferCharacteristic::SMPTE2084
                    | av::color::TransferCharacteristic::ARIB_STD_B67
            );
            // Set metadata in the graph without cloning the source pixel data.
            // Buffer input metadata alone does not override a frame's own SAR.
            let mut filters = vec![format!("setsar={sar}")];
            // Small previews must reduce 4K frames before HDR tone mapping and
            // RGBA/subtitle composition, rather than doing all work at source size.
            if let Some((width, height)) = self.preview_bounds {
                filters.push(format!("scale=w={width}:h={height}:flags=bilinear"));
            }
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
                // Preserve shadows and in-range midtones; Hable darkened even
                // ordinary diffuse content while compressing HDR highlights.
                filters.push(format!("zscale=min={}:pin={}:tin={}:rin={range}:t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=mobius:param=0.3:desat=2,zscale=t=iec61966-2-1:m=gbr:r=full", matrix as i32, primaries as i32, transfer as i32));
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
            if sar.numerator() != sar.denominator() {
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
            self.graph_aspect = Some(sar);
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

#[cfg(test)]
mod tests {
    use super::*;
    fn processor(stream_aspect: Option<av::Rational>) -> Processor {
        Processor {
            path: "unused".into(),
            selection: Selection::Off,
            graph: None,
            bitmap: None,
            time: (1, 24).into(),
            rotation: 0,
            preview_bounds: None,
            stream_aspect,
            graph_aspect: None,
        }
    }
    fn source(aspect: av::Rational) -> frame::Video {
        let mut source = frame::Video::new(av::format::Pixel::YUV420P, 320, 180);
        for plane in 0..source.planes() {
            source.data_mut(plane).fill(128);
        }
        // SAFETY: this frame exclusively owns its AVFrame metadata.
        unsafe {
            (*source.as_mut_ptr()).sample_aspect_ratio = aspect.into();
        }
        source
    }
    #[test]
    fn container_aspect_is_applied_when_decoder_reports_square_or_unknown_pixels() {
        av::init().unwrap();
        for aspect in [(0, 1), (1, 1), (2, 1)] {
            let mut processor = processor(Some((2, 1).into()));
            let output = processor.process(&source(aspect.into())).unwrap();
            assert_eq!((output.width(), output.height()), (640, 180));
            assert_eq!(output.aspect_ratio(), (1, 1).into());
        }
    }
    #[test]
    fn hostile_aspect_metadata_is_refused_before_filter_allocation() {
        let mut processor = processor(Some((i32::MAX, 1).into()));
        let Err(error) = processor.process(&source((1, 1).into())) else {
            panic!("Hostile aspect metadata reached the filter graph");
        };
        assert!(error.to_string().contains("dimensions exceed"));
        assert!(processor.graph.is_none());
    }
    #[test]
    fn frame_aspect_changes_rebuild_the_graph_when_container_is_unspecified() {
        av::init().unwrap();
        let mut processor = processor(None);
        for (aspect, width) in [((2, 1), 640), ((1, 1), 320), ((0, 1), 320)] {
            let output = processor.process(&source(aspect.into())).unwrap();
            assert_eq!((output.width(), output.height()), (width, 180));
            assert_eq!(output.aspect_ratio(), (1, 1).into());
        }
    }
}
