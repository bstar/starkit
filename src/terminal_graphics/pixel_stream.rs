//! Negotiated, lossless native-pixel live canvases with acknowledged PNG deltas.
//! Compression runs off the session/input thread; only untouched raw frames coalesce.
use super::live_canvas::{Frame, LiveFrames, PixelFormat, Update, MAX_STREAMS};
use crate::image::{ImageEncoder, RgbaImage};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use crossbeam_channel::{bounded, Receiver, Sender};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
const MAX_ENCODED: usize = 8 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pixels {
    pub stream: String,
    pub generation: u64,
    pub sequence: u64,
    pub base: Option<u64>,
    pub width: u32,
    pub height: u32,
    pub rect: [u32; 4],
    pub png: String,
}
impl Pixels {
    pub(crate) fn decode(&self) -> Result<RgbaImage> {
        ensure!(
            !self.stream.is_empty()
                && self.stream.len() <= 96
                && self
                    .stream
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b)),
            "Invalid pixel stream"
        );
        ensure!(
            (1..=8192).contains(&self.width)
                && (1..=8192).contains(&self.height)
                && u64::from(self.width) * u64::from(self.height) <= 32_000_000,
            "Invalid lossless frame dimensions"
        );
        let [x, y, w, h] = self.rect;
        ensure!(
            w > 0
                && h > 0
                && x.checked_add(w).is_some_and(|v| v <= self.width)
                && y.checked_add(h).is_some_and(|v| v <= self.height),
            "Pixel patch outside frame"
        );
        ensure!(
            self.base.is_some() || self.rect == [0, 0, self.width, self.height],
            "Initial pixel frame must be complete"
        );
        ensure!(
            self.png.len() <= MAX_ENCODED.div_ceil(3) * 4,
            "Pixel patch exceeds budget"
        );
        let bytes = STANDARD.decode(&self.png)?;
        let dimensions = crate::image::ImageReader::with_format(
            std::io::Cursor::new(&bytes),
            crate::image::ImageFormat::Png,
        )
        .into_dimensions()?;
        ensure!(dimensions == (w, h), "Pixel patch dimensions disagree");
        Ok(
            crate::image::load_from_memory_with_format(&bytes, crate::image::ImageFormat::Png)?
                .to_rgba8(),
        )
    }
}
struct Base {
    sequence: u64,
    pixels: RgbaImage,
}
#[derive(Default)]
struct Slot {
    generation: u64,
    pending: Option<Frame>,
    flight: Option<u64>,
    base: Option<Base>,
    closed: bool,
}
fn encode(
    stream: &str,
    generation: u64,
    sequence: u64,
    frame: Frame,
    old: Option<&Base>,
) -> Result<Option<(Pixels, RgbaImage)>> {
    frame.validate()?;
    let mut rgba = Vec::with_capacity(frame.width as usize * frame.height as usize * 4);
    for row in frame.pixels.chunks_exact(frame.stride as usize) {
        if frame.format == PixelFormat::Rgba8 {
            rgba.extend_from_slice(&row[..frame.width as usize * 4]);
        } else {
            for p in row[..frame.width as usize * 4].as_chunks::<4>().0 {
                rgba.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
            }
        }
    }
    let image =
        RgbaImage::from_raw(frame.width, frame.height, rgba).context("Invalid lossless pixels")?;
    let old = old.filter(|b| b.pixels.dimensions() == image.dimensions());
    let mut rect = [0, 0, frame.width, frame.height];
    if let Some(old) = old {
        let mut minx = frame.width;
        let mut miny = frame.height;
        let mut maxx = 0;
        let mut maxy = 0;
        let stride = frame.width as usize * 4;
        for (y, (a, b)) in image
            .as_raw()
            .chunks_exact(stride)
            .zip(old.pixels.as_raw().chunks_exact(stride))
            .enumerate()
        {
            if a == b {
                continue;
            }
            miny = miny.min(y as u32);
            maxy = y as u32 + 1;
            for (x, (a, b)) in a
                .as_chunks::<4>()
                .0
                .iter()
                .zip(b.as_chunks::<4>().0)
                .enumerate()
            {
                if a != b {
                    minx = minx.min(x as u32);
                    maxx = maxx.max(x as u32 + 1);
                }
            }
        }
        if minx == frame.width {
            return Ok(None);
        }
        rect = [minx, miny, maxx - minx, maxy - miny];
    }
    let [x, y, w, h] = rect;
    let patch = crate::image::imageops::crop_imm(&image, x, y, w, h).to_image();
    let mut bytes = vec![];
    crate::image::codecs::png::PngEncoder::new_with_quality(
        &mut bytes,
        crate::image::codecs::png::CompressionType::Fast,
        crate::image::codecs::png::FilterType::Sub,
    )
    .write_image(patch.as_raw(), w, h, crate::image::ExtendedColorType::Rgba8)?;
    ensure!(
        bytes.len() <= MAX_ENCODED,
        "Lossless patch exceeds 8 MiB transport budget"
    );
    Ok(Some((
        Pixels {
            stream: stream.into(),
            generation,
            sequence,
            base: old.map(|b| b.sequence),
            width: frame.width,
            height: frame.height,
            rect,
            png: STANDARD.encode(bytes),
        },
        image,
    )))
}
pub(crate) enum Output {
    Frame(Pixels),
    Close(String, u64),
    Error(String),
}
pub(crate) struct Transport {
    updates: LiveFrames,
    acknowledgements: Sender<(String, u64, u64)>,
    pub output: Receiver<Output>,
    active: HashSet<String>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Transport {
    pub fn new() -> Result<Self> {
        let updates = LiveFrames::default();
        let input = updates.clone();
        let (ack, acknowledgements) = bounded::<(String, u64, u64)>(32);
        let (tx, output) = bounded(8);
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = stop.clone();
        let worker = std::thread::Builder::new()
            .name("star-lossless-canvas".into())
            .spawn(move || {
                let mut slots: HashMap<String, Slot> = HashMap::new();
                let mut sequence = 0;
                let publish = |mut value| loop {
                    if cancel.load(Ordering::Relaxed) {
                        return false;
                    }
                    match tx.send_timeout(value, Duration::from_millis(10)) {
                        Ok(()) => return true,
                        Err(crossbeam_channel::SendTimeoutError::Timeout(v)) => value = v,
                        Err(_) => return false,
                    }
                };
                while !cancel.load(Ordering::Relaxed) {
                    for (stream, generation, seq) in acknowledgements.try_iter() {
                        if let Some(s) = slots
                            .get_mut(&stream)
                            .filter(|s| s.generation == generation && s.flight == Some(seq))
                        {
                            s.flight = None;
                        }
                    }
                    for update in input.drain() {
                        match update {
                            Update::Close { stream, generation } => {
                                if slots
                                    .get(&stream)
                                    .is_some_and(|s| s.generation > generation)
                                {
                                    continue;
                                }
                                slots.insert(
                                    stream.clone(),
                                    Slot {
                                        generation,
                                        closed: true,
                                        ..Default::default()
                                    },
                                );
                                if !publish(Output::Close(stream, generation)) {
                                    return;
                                }
                            }
                            Update::Frame {
                                stream,
                                generation,
                                frame,
                            } => {
                                if slots.get(&stream).is_some_and(|s| {
                                    s.generation > generation
                                        || (s.generation == generation && s.closed)
                                }) {
                                    continue;
                                }
                                if !slots.contains_key(&stream) {
                                    slots.retain(|_, s| !s.closed);
                                    if slots.len() >= MAX_STREAMS {
                                        if !publish(Output::Error(
                                            "Too many lossless streams".into(),
                                        )) {
                                            return;
                                        }
                                        continue;
                                    }
                                }
                                let bytes: usize = slots
                                    .iter()
                                    .map(|(id, s)| {
                                        s.base.as_ref().map_or(0, |b| b.pixels.as_raw().len())
                                            + if id == &stream {
                                                0
                                            } else {
                                                s.pending.as_ref().map_or(0, |f| f.pixels.len())
                                            }
                                    })
                                    .sum();
                                if bytes.saturating_add(frame.pixels.len()) > 256 * 1024 * 1024 {
                                    if !publish(Output::Error(
                                        "Lossless frame budget exceeded".into(),
                                    )) {
                                        return;
                                    }
                                    continue;
                                }
                                let s = slots.entry(stream).or_default();
                                if s.generation != generation {
                                    *s = Slot {
                                        generation,
                                        ..Default::default()
                                    };
                                }
                                s.pending = Some(frame);
                            }
                        }
                    }
                    for (stream, slot) in &mut slots {
                        if slot.flight.is_some() {
                            continue;
                        }
                        let Some(frame) = slot.pending.take() else {
                            continue;
                        };
                        sequence += 1;
                        match encode(stream, slot.generation, sequence, frame, slot.base.as_ref()) {
                            Ok(Some((message, pixels))) => {
                                slot.flight = Some(sequence);
                                slot.base = Some(Base { sequence, pixels });
                                if !publish(Output::Frame(message)) {
                                    return;
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                if !publish(Output::Error(format!("Lossless canvas: {e:#}"))) {
                                    return;
                                }
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            })?;
        Ok(Self {
            updates,
            acknowledgements: ack,
            output,
            active: HashSet::new(),
            stop,
            worker: Some(worker),
        })
    }
    pub fn submit(&mut self, update: Update) -> Result<()> {
        match update {
            Update::Frame {
                stream,
                generation,
                frame,
            } => {
                ensure!(
                    self.active.contains(&stream) || self.active.len() < MAX_STREAMS,
                    "Too many lossless streams"
                );
                self.updates.submit(&stream, generation, frame)?;
                self.active.insert(stream);
            }
            Update::Close { stream, generation } => {
                self.updates.close(&stream, generation)?;
                self.active.remove(&stream);
            }
        }
        Ok(())
    }
    pub fn acknowledge(&self, stream: &str, generation: u64, sequence: u64) {
        let _ = self
            .acknowledgements
            .try_send((stream.into(), generation, sequence));
    }
    pub fn active(&self) -> bool {
        !self.active.is_empty()
    }
}
impl Drop for Transport {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn frame(image: &RgbaImage) -> Frame {
        Frame {
            width: image.width(),
            height: image.height(),
            stride: image.width() * 4,
            format: PixelFormat::Rgba8,
            presentation_ns: 0,
            pixels: Arc::new(image.as_raw().clone()),
        }
    }
    #[test]
    fn native_pixels_and_small_changes_are_exact_and_never_thumbnail() {
        let mut source = RgbaImage::from_fn(1350, 1086, |x, y| {
            crate::image::Rgba([x as u8, y as u8, ((x + y) % 255) as u8, 255])
        });
        let (first, base) = encode("terminal", 1, 1, frame(&source), None)
            .unwrap()
            .unwrap();
        assert_eq!(first.rect, [0, 0, 1350, 1086]);
        assert_eq!(first.decode().unwrap(), source);
        source.put_pixel(1100, 1040, crate::image::Rgba([1, 2, 3, 255]));
        let (delta, _) = encode(
            "terminal",
            1,
            2,
            frame(&source),
            Some(&Base {
                sequence: 1,
                pixels: base,
            }),
        )
        .unwrap()
        .unwrap();
        assert_eq!(delta.base, Some(1));
        assert_eq!(delta.rect, [1100, 1040, 1, 1]);
        assert_eq!(delta.decode().unwrap().get_pixel(0, 0).0, [1, 2, 3, 255]);
        let mut bad = delta.clone();
        bad.rect[0] = 1350;
        assert!(bad.decode().is_err());
    }
    #[test]
    fn only_latest_unencoded_frame_waits_for_acknowledgement() {
        let mut t = Transport::new().unwrap();
        let image = RgbaImage::from_pixel(64, 64, crate::image::Rgba([1, 2, 3, 255]));
        t.submit(Update::Frame {
            stream: "a".into(),
            generation: 1,
            frame: frame(&image),
        })
        .unwrap();
        let Output::Frame(first) = t.output.recv_timeout(Duration::from_secs(2)).unwrap() else {
            panic!()
        };
        for value in 2..10 {
            let image = RgbaImage::from_pixel(64, 64, crate::image::Rgba([value, 0, 0, 255]));
            t.submit(Update::Frame {
                stream: "a".into(),
                generation: 1,
                frame: frame(&image),
            })
            .unwrap();
        }
        assert!(t.output.recv_timeout(Duration::from_millis(30)).is_err());
        t.acknowledge("a", 1, first.sequence);
        let Output::Frame(last) = t.output.recv_timeout(Duration::from_secs(2)).unwrap() else {
            panic!()
        };
        assert_eq!(last.decode().unwrap().get_pixel(0, 0).0, [9, 0, 0, 255]);
    }
}
