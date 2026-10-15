//! Engine-independent live frames. Local sessions retain lossless mapped leases.
//! Remote previews negotiate bounded encoded frames; source recording stays independent.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tempfile::{NamedTempFile, TempDir};

pub const MAX_STREAMS: usize = 8;
pub const MAX_BYTES: usize = 128 * 1024 * 1024;
const TOTAL_BYTES: usize = 256 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelFormat {
    Rgba8,
    Bgra8,
}
#[derive(Clone, Debug)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
    pub presentation_ns: u64,
    pub pixels: Arc<Vec<u8>>,
}
impl Frame {
    pub fn validate(&self) -> Result<()> {
        validate(self.width, self.height, self.stride, self.pixels.len())
    }
}
fn validate(width: u32, height: u32, stride: u32, len: usize) -> Result<()> {
    ensure!(
        (1..=8192).contains(&width)
            && (1..=8192).contains(&height)
            && u64::from(width) * u64::from(height) <= 32_000_000,
        "Invalid live frame dimensions"
    );
    ensure!(
        stride >= width * 4 && stride <= 8192 * 4,
        "Invalid live frame stride"
    );
    ensure!(
        len == stride as usize * height as usize && len <= MAX_BYTES,
        "Invalid live frame size"
    );
    Ok(())
}
fn stream_valid(stream: &str) -> bool {
    !stream.is_empty()
        && stream.len() <= 96
        && stream
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b))
}
#[derive(Clone, Debug)]
pub enum Update {
    Frame {
        stream: String,
        generation: u64,
        frame: Frame,
    },
    Close {
        stream: String,
        generation: u64,
    },
}
/// Clone into compositor/render workers. Each stream retains only its newest update.
#[derive(Clone, Default)]
pub struct LiveFrames {
    updates: Arc<Mutex<HashMap<String, Update>>>,
}
impl LiveFrames {
    pub fn submit(&self, stream: &str, generation: u64, frame: Frame) -> Result<()> {
        frame.validate()?;
        ensure!(stream_valid(stream), "Invalid live stream name");
        let mut updates = self.updates.lock().unwrap();
        ensure!(
            updates.contains_key(stream) || updates.len() < MAX_STREAMS,
            "Too many live streams"
        );
        let bytes: usize = updates
            .iter()
            .filter(|(id, _)| id.as_str() != stream)
            .map(|(_, u)| match u {
                Update::Frame { frame, .. } => frame.pixels.len(),
                _ => 0,
            })
            .sum();
        ensure!(
            bytes + frame.pixels.len() <= TOTAL_BYTES,
            "Live frame budget exceeded"
        );
        if updates.get(stream).is_some_and(|u| match u {
            Update::Frame { generation: g, .. } => *g > generation,
            Update::Close { generation: g, .. } => *g >= generation,
        }) {
            return Ok(());
        }
        updates.insert(
            stream.into(),
            Update::Frame {
                stream: stream.into(),
                generation,
                frame,
            },
        );
        Ok(())
    }
    pub fn close(&self, stream: &str, generation: u64) -> Result<()> {
        ensure!(stream_valid(stream), "Invalid live stream name");
        let mut updates = self.updates.lock().unwrap();
        ensure!(
            updates.contains_key(stream) || updates.len() < MAX_STREAMS,
            "Too many live streams"
        );
        if updates.get(stream).is_some_and(|u| match u {
            Update::Frame { generation: g, .. } | Update::Close { generation: g, .. } => {
                *g > generation
            }
        }) {
            return Ok(());
        }
        updates.insert(
            stream.into(),
            Update::Close {
                stream: stream.into(),
                generation,
            },
        );
        Ok(())
    }
    pub fn drain(&self) -> Vec<Update> {
        self.updates
            .lock()
            .unwrap()
            .drain()
            .map(|(_, u)| u)
            .collect()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Descriptor {
    pub stream: String,
    pub generation: u64,
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
    pub presentation_ns: u64,
    pub path: PathBuf,
}
struct Lease {
    _file: Option<NamedTempFile>,
    descriptor: Descriptor,
}
struct Slot {
    generation: u64,
    pending: Option<Frame>,
    flight: Option<Lease>,
    closed: bool,
}
pub(crate) struct Transport {
    root: TempDir,
    slots: HashMap<String, Slot>,
    sequence: u64,
}
impl Transport {
    pub fn active(&self) -> bool {
        self.slots.values().any(|slot| !slot.closed)
    }
    pub fn new() -> Result<Self> {
        let root = tempfile::Builder::new().prefix("starkit-live-").tempdir()?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            root,
            slots: HashMap::new(),
            sequence: 0,
        })
    }
    pub fn submit(&mut self, update: Update) -> Result<Option<(String, u64)>> {
        let (stream, generation) = match &update {
            Update::Frame {
                stream, generation, ..
            }
            | Update::Close { stream, generation } => (stream, *generation),
        };
        ensure!(stream_valid(stream), "Invalid live stream name");
        if self
            .slots
            .get(stream)
            .is_some_and(|s| s.generation > generation || (s.generation == generation && s.closed))
        {
            return Ok(None);
        }
        if let Update::Close { stream, generation } = update {
            ensure!(
                self.slots.contains_key(&stream) || self.slots.len() < MAX_STREAMS,
                "Too many live streams"
            );
            self.slots.insert(
                stream.clone(),
                Slot {
                    generation,
                    pending: None,
                    flight: None,
                    closed: true,
                },
            );
            return Ok(Some((stream, generation)));
        }
        let Update::Frame {
            stream,
            generation,
            frame,
        } = update
        else {
            unreachable!()
        };
        frame.validate()?;
        ensure!(
            self.slots.contains_key(&stream) || self.slots.len() < MAX_STREAMS,
            "Too many live streams"
        );
        let bytes: usize = self
            .slots
            .iter()
            .map(|(id, s)| {
                s.pending
                    .as_ref()
                    .filter(|_| id != &stream)
                    .map_or(0, |f| f.pixels.len())
                    + s.flight.as_ref().map_or(0, |f| {
                        f.descriptor.stride as usize * f.descriptor.height as usize
                    })
            })
            .sum();
        ensure!(
            bytes + frame.pixels.len() <= TOTAL_BYTES,
            "Live frame budget exceeded"
        );
        let slot = self.slots.entry(stream).or_insert(Slot {
            generation,
            pending: None,
            flight: None,
            closed: false,
        });
        if slot.generation != generation {
            slot.flight = None;
            slot.generation = generation;
        }
        slot.closed = false;
        slot.pending = Some(frame);
        Ok(None)
    }
    pub fn acknowledge(&mut self, stream: &str, generation: u64, sequence: u64) {
        if let Some(s) = self.slots.get_mut(stream) {
            if s.generation == generation
                && s.flight
                    .as_ref()
                    .is_some_and(|l| l.descriptor.sequence == sequence)
            {
                s.flight = None;
            }
        }
    }
    pub fn flush(&mut self) -> Result<Vec<Descriptor>> {
        let mut out = vec![];
        for (stream, slot) in &mut self.slots {
            // A stalled frontend may still have this descriptor queued. One
            // outstanding lease and one latest pending frame bound resources;
            // time alone cannot prove that its reader has finished with a file.
            if slot.flight.is_some() {
                continue;
            }
            let Some(frame) = slot.pending.take() else {
                continue;
            };
            let file = tempfile::Builder::new()
                .prefix("frame-")
                .tempfile_in(self.root.path())?;
            file.as_file().set_len(frame.pixels.len() as u64)?;
            // SAFETY: this private file has just been sized; the producer alone owns it.
            let mut mapping = unsafe { memmap2::MmapOptions::new().map_mut(file.as_file()) }?;
            mapping.copy_from_slice(&frame.pixels);
            drop(mapping);
            self.sequence += 1;
            let descriptor = Descriptor {
                stream: stream.clone(),
                generation: slot.generation,
                sequence: self.sequence,
                width: frame.width,
                height: frame.height,
                stride: frame.stride,
                format: frame.format,
                presentation_ns: frame.presentation_ns,
                path: file.path().into(),
            };
            out.push(descriptor.clone());
            slot.flight = Some(Lease {
                _file: Some(file),
                descriptor,
            });
        }
        Ok(out)
    }
    /// One encoded preview in flight per stream. Recording pixels stay untouched.
    pub fn flush_streamed(&mut self) -> Result<Vec<EncodedFrame>> {
        let mut out = Vec::new();
        for (stream, slot) in &mut self.slots {
            if slot.flight.is_some() {
                continue;
            }
            let Some(frame) = slot.pending.take() else {
                continue;
            };
            let mut rgba = Vec::with_capacity(frame.width as usize * frame.height as usize * 4);
            for row in frame.pixels.chunks_exact(frame.stride as usize) {
                for px in row[..frame.width as usize * 4].as_chunks::<4>().0 {
                    match frame.format {
                        PixelFormat::Rgba8 => rgba.extend_from_slice(px),
                        PixelFormat::Bgra8 => rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]),
                    }
                }
            }
            let source = crate::image::RgbaImage::from_raw(frame.width, frame.height, rgba)
                .context("Invalid preview pixels")?;
            let mut preview = crate::image::DynamicImage::ImageRgba8(source)
                .thumbnail(1280, 720)
                .to_rgb8();
            let bytes = loop {
                let mut bytes = Vec::new();
                crate::image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 85)
                    .encode_image(&preview)?;
                if bytes.len() <= 512 * 1024 {
                    break bytes;
                }
                preview = crate::image::imageops::resize(
                    &preview,
                    (preview.width() / 2).max(1),
                    (preview.height() / 2).max(1),
                    crate::image::imageops::FilterType::Triangle,
                );
            };
            self.sequence += 1;
            use base64::Engine as _;
            out.push(EncodedFrame {
                stream: stream.clone(),
                generation: slot.generation,
                sequence: self.sequence,
                width: preview.width(),
                height: preview.height(),
                presentation_ns: frame.presentation_ns,
                jpeg: base64::engine::general_purpose::STANDARD.encode(bytes),
            });
            slot.flight = Some(Lease {
                _file: None,
                descriptor: Descriptor {
                    stream: stream.clone(),
                    generation: slot.generation,
                    sequence: self.sequence,
                    width: frame.width,
                    height: frame.height,
                    stride: frame.stride,
                    format: frame.format,
                    presentation_ns: frame.presentation_ns,
                    path: PathBuf::new(),
                },
            });
        }
        Ok(out)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EncodedFrame {
    pub stream: String,
    pub generation: u64,
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub presentation_ns: u64,
    pub jpeg: String,
}
impl EncodedFrame {
    pub(crate) fn decode(&self) -> Result<Arc<crate::image::RgbaImage>> {
        ensure!(stream_valid(&self.stream), "Invalid live stream name");
        ensure!(
            self.width > 0 && self.width <= 1280 && self.height > 0 && self.height <= 720,
            "Invalid streamed preview dimensions"
        );
        ensure!(
            self.jpeg.len() <= 700_000,
            "Streamed preview exceeds budget"
        );
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD.decode(&self.jpeg)?;
        let dimensions = crate::image::ImageReader::with_format(
            std::io::Cursor::new(&bytes),
            crate::image::ImageFormat::Jpeg,
        )
        .into_dimensions()?;
        ensure!(
            dimensions == (self.width, self.height),
            "Streamed preview dimensions disagree"
        );
        Ok(Arc::new(
            crate::image::load_from_memory_with_format(&bytes, crate::image::ImageFormat::Jpeg)?
                .to_rgba8(),
        ))
    }
}
/// Import only on a negotiated, same-machine session. No remote path access.
#[cfg(unix)]
pub(crate) fn import(descriptor: &Descriptor, local: bool) -> Result<Arc<crate::image::RgbaImage>> {
    use std::os::unix::fs::MetadataExt;
    ensure!(local, "Live mapped frames require a local session");
    ensure!(stream_valid(&descriptor.stream), "Invalid live stream name");
    let len = descriptor.stride as usize * descriptor.height as usize;
    validate(descriptor.width, descriptor.height, descriptor.stride, len)?;
    let parent = descriptor
        .path
        .parent()
        .context("Invalid live frame path")?;
    let name = descriptor
        .path
        .file_name()
        .context("Invalid live frame path")?;
    ensure!(
        parent
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("starkit-live-"))
            && name.to_string_lossy().starts_with("frame-"),
        "Invalid live frame path"
    );
    let dir = rustix::fs::open(
        parent,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::NOFOLLOW,
        rustix::fs::Mode::empty(),
    )
    .map_err(std::io::Error::from)
    .with_context(|| format!("Open live frame directory {}", parent.display()))?;
    let dir_meta = File::from(dir.try_clone()?).metadata()?;
    ensure!(
        dir_meta.uid() == rustix::process::geteuid().as_raw() && dir_meta.mode() & 0o077 == 0,
        "Live frame directory is not private"
    );
    let file = File::from(
        rustix::fs::openat(
            &dir,
            name,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW,
            rustix::fs::Mode::empty(),
        )
        .map_err(std::io::Error::from)
        .with_context(|| format!("Open live frame {}", descriptor.path.display()))?,
    );
    let meta = file.metadata()?;
    ensure!(
        meta.is_file()
            && meta.len() == len as u64
            && meta.uid() == dir_meta.uid()
            && meta.mode() & 0o077 == 0,
        "Invalid mapped frame file"
    );
    // SAFETY: the authenticated local producer retains this immutable file until acknowledgement.
    let mapping = unsafe { memmap2::MmapOptions::new().len(len).map(&file) }?;
    let mut pixels = Vec::with_capacity(descriptor.width as usize * descriptor.height as usize * 4);
    for row in mapping.chunks_exact(descriptor.stride as usize) {
        pixels.extend_from_slice(&row[..descriptor.width as usize * 4]);
    }
    if descriptor.format == PixelFormat::Bgra8 {
        for p in pixels.as_chunks_mut::<4>().0 {
            p.swap(0, 2);
        }
    }
    Ok(Arc::new(
        crate::image::RgbaImage::from_raw(descriptor.width, descriptor.height, pixels)
            .context("Invalid live frame")?,
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streamed_frames_are_self_contained_coalesced_and_acknowledged() {
        let mut transport = Transport::new().unwrap();
        let frame = |value| Update::Frame {
            stream: "remote".into(),
            generation: 2,
            frame: Frame {
                width: 8,
                height: 8,
                stride: 32,
                format: PixelFormat::Rgba8,
                presentation_ns: 123,
                pixels: Arc::new([value, 20, 30, 255].repeat(64)),
            },
        };
        transport.submit(frame(10)).unwrap();
        let first = transport.flush_streamed().unwrap().pop().unwrap();
        transport.submit(frame(40)).unwrap();
        transport.submit(frame(80)).unwrap();
        assert!(transport.flush_streamed().unwrap().is_empty());
        transport.acknowledge("remote", 1, first.sequence);
        assert!(transport.flush_streamed().unwrap().is_empty());
        transport.acknowledge("remote", 2, first.sequence);
        let next = transport.flush_streamed().unwrap().pop().unwrap();
        assert!(next.sequence > first.sequence);
        drop(transport); // No producer path or file is needed by the receiver.
        let wire = serde_json::to_string(&next).unwrap();
        assert!(!wire.contains("path"));
        let decoded: EncodedFrame = serde_json::from_str(&wire).unwrap();
        assert!((decoded.decode().unwrap().get_pixel(0, 0)[0] as i32 - 80).abs() < 8);
        let mut bad = decoded;
        bad.width = 7;
        assert!(bad.decode().is_err());
    }
    fn frame() -> Frame {
        Frame {
            width: 2,
            height: 1,
            stride: 12,
            format: PixelFormat::Bgra8,
            presentation_ns: 42,
            pixels: Arc::new(vec![1, 2, 3, 255, 4, 5, 6, 255, 0, 0, 0, 0]),
        }
    }
    #[test]
    fn mapped_handoff_converts_stride_and_retires_on_ack() {
        let mut t = Transport::new().unwrap();
        t.submit(Update::Frame {
            stream: "demo".into(),
            generation: 1,
            frame: frame(),
        })
        .unwrap();
        let d = t.flush().unwrap().pop().unwrap();
        assert!(d.path.exists());
        let p = import(&d, true).unwrap();
        assert_eq!(p.as_raw(), &[3, 2, 1, 255, 6, 5, 4, 255]);
        assert!(import(&d, false).is_err());
        t.acknowledge("demo", 0, d.sequence);
        assert!(d.path.exists());
        t.acknowledge("demo", 1, d.sequence);
        assert!(!d.path.exists());
    }
    #[test]
    fn slow_streams_coalesce_independently() {
        let mut t = Transport::new().unwrap();
        assert!(!t.active());
        for stream in ["a", "b"] {
            t.submit(Update::Frame {
                stream: stream.into(),
                generation: 1,
                frame: frame(),
            })
            .unwrap();
        }
        assert_eq!(t.flush().unwrap().len(), 2);
        assert!(t.active());
        for _ in 0..20 {
            t.submit(Update::Frame {
                stream: "a".into(),
                generation: 1,
                frame: frame(),
            })
            .unwrap();
        }
        assert!(t.flush().unwrap().is_empty());
        t.submit(Update::Close {
            stream: "a".into(),
            generation: 2,
        })
        .unwrap();
        assert_eq!(t.slots.values().filter(|s| !s.closed).count(), 1);
        t.submit(Update::Frame {
            stream: "a".into(),
            generation: 1,
            frame: frame(),
        })
        .unwrap();
        assert!(t.flush().unwrap().is_empty());
        assert!(
            t.active(),
            "Closing one producer must keep the other responsive"
        );
        t.submit(Update::Close {
            stream: "b".into(),
            generation: 2,
        })
        .unwrap();
        assert!(
            !t.active(),
            "Closing all producers restores quiet controller polling"
        );
    }
}
