//! Live layers below ordinary scene artwork. No media decoder is involved.
use super::{
    live_canvas::{self, Descriptor},
    protocol::{Component, Scene, Viewport},
};
use crate::image::RgbaImage;
use anyhow::{ensure, Result};
use std::{
    collections::HashMap,
    io::Write,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
};
use tempfile::{NamedTempFile, TempDir};
static IMAGE_ID: AtomicU32 = AtomicU32::new(0x6c000000);
struct Layer {
    generation: u64,
    pixels: Arc<RgbaImage>,
    dirty: bool,
    image: Option<u32>,
    files: Vec<NamedTempFile>,
    view: Option<crate::content_view::ContentView>,
    presented_at: Option<std::time::Instant>,
    inline_interval: std::time::Duration,
}

pub(crate) struct Frontend {
    layers: HashMap<String, Layer>,
    root: TempDir,
    terminal_files: bool,
    resizer: fast_image_resize::Resizer,
    stats_began: std::time::Instant,
    stats_frames: u64,
    stats_bytes: u64,
}
impl Frontend {
    pub fn new() -> Result<Self> {
        Ok(Self {
            layers: HashMap::new(),
            root: tempfile::Builder::new()
                .prefix("starkit-canvas-")
                .tempdir()?,
            terminal_files: true,
            resizer: fast_image_resize::Resizer::new(),
            stats_began: std::time::Instant::now(),
            stats_frames: 0,
            stats_bytes: 0,
        })
    }
    pub fn with_terminal_files(mut self, available: bool) -> Self {
        self.terminal_files = available;
        self
    }
    pub fn receive_encoded(&mut self, frame: &live_canvas::EncodedFrame) -> Result<bool> {
        if self
            .layers
            .get(&frame.stream)
            .is_some_and(|l| l.generation > frame.generation)
        {
            return Ok(false);
        }
        ensure!(
            self.layers.contains_key(&frame.stream) || self.layers.len() < live_canvas::MAX_STREAMS,
            "Too many live layers"
        );
        let pixels = frame.decode()?;
        let first = !self.layers.contains_key(&frame.stream);
        let layer = self
            .layers
            .entry(frame.stream.clone())
            .or_insert_with(|| Layer {
                generation: frame.generation,
                pixels: pixels.clone(),
                dirty: true,
                image: None,
                files: vec![],
                view: None,
                presented_at: None,
                inline_interval: INLINE_FRAME_INTERVAL,
            });
        layer.generation = frame.generation;
        layer.pixels = pixels;
        layer.dirty = true;
        Ok(first)
    }
    pub fn active(&self) -> bool {
        !self.layers.is_empty()
    }
    pub fn video_ready(&mut self) -> bool {
        self.layers.values_mut().all(|layer| {
            layer.files.retain(|file| file.path().exists());
            layer.files.len() < 3
        })
    }
    pub fn receive_pixels(
        &mut self,
        stream: String,
        generation: u64,
        pixels: Arc<RgbaImage>,
    ) -> Result<bool> {
        if self
            .layers
            .get(&stream)
            .is_some_and(|l| l.generation > generation)
        {
            return Ok(false);
        }
        ensure!(
            self.layers.contains_key(&stream) || self.layers.len() < live_canvas::MAX_STREAMS,
            "Too many live layers"
        );
        let first = !self.layers.contains_key(&stream);
        let layer = self.layers.entry(stream).or_insert_with(|| Layer {
            generation,
            pixels: pixels.clone(),
            dirty: true,
            image: None,
            files: vec![],
            view: None,
            presented_at: None,
            inline_interval: INLINE_FRAME_INTERVAL,
        });
        layer.generation = generation;
        layer.pixels = pixels;
        layer.dirty = true;
        Ok(first)
    }
    pub fn receive(&mut self, d: &Descriptor, local: bool) -> Result<bool> {
        if self
            .layers
            .get(&d.stream)
            .is_some_and(|l| l.generation > d.generation)
        {
            return Ok(false);
        }
        ensure!(
            self.layers.contains_key(&d.stream) || self.layers.len() < live_canvas::MAX_STREAMS,
            "Too many live layers"
        );
        let first = !self.layers.contains_key(&d.stream);
        let pixels = match live_canvas::import(d, local) {
            Ok(pixels) => pixels,
            // Older retained controllers expire leases after two seconds. A
            // generation change/close can also retire a queued descriptor.
            // Acknowledge that obsolete frame and keep the last usable pixels.
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                tracing::warn!(stream = %d.stream, sequence = d.sequence, %error, "Discarding retired live frame");
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        let layer = self
            .layers
            .entry(d.stream.clone())
            .or_insert_with(|| Layer {
                generation: d.generation,
                pixels: pixels.clone(),
                dirty: true,
                image: None,
                files: vec![],
                view: None,
                presented_at: None,
                inline_interval: INLINE_FRAME_INTERVAL,
            });
        layer.generation = d.generation;
        layer.pixels = pixels;
        layer.dirty = true;
        Ok(first)
    }
    pub fn close(&mut self, id: &str, generation: u64, out: &mut impl Write) -> Result<()> {
        if self
            .layers
            .get(id)
            .is_some_and(|l| l.generation <= generation)
        {
            if let Some(mut l) = self.layers.remove(id) {
                if let Some(i) = l.image.take() {
                    write!(out, "\x1b_Ga=d,d=I,i={i},q=2;\x1b\\")?;
                }
            }
        }
        Ok(())
    }
    pub fn clear(&mut self, out: &mut impl Write) -> Result<()> {
        for (_, mut l) in self.layers.drain() {
            if let Some(i) = l.image.take() {
                write!(out, "\x1b_Ga=d,d=I,i={i},q=2;\x1b\\")?;
            }
        }
        out.flush()?;
        Ok(())
    }
    pub fn invalidate(&mut self) {
        for l in self.layers.values_mut() {
            l.dirty = true;
        }
    }
    /// Remove terminal placements during a resize, retaining decoded pixels for
    /// the next scene. A screen clear/terminal clip invalidates their positions.
    pub fn hide(&mut self, out: &mut impl Write) -> Result<()> {
        for layer in self.layers.values_mut() {
            if let Some(image) = layer.image.take() {
                write!(out, "\x1b_Ga=d,d=I,i={image},q=2;\x1b\\")?;
            }
            layer.dirty = true;
        }
        out.flush()?;
        Ok(())
    }
    pub fn present(&mut self, scene: &Scene, v: Viewport, out: &mut impl Write) -> Result<()> {
        if scene.viewport.generation != v.generation
            || scene.viewport.width != v.width
            || scene.viewport.height != v.height
        {
            // A terminal resize is immediate; its replacement scene is
            // asynchronous. Geometry from the previous viewport is obsolete,
            // rather than an invalid canvas in the new viewport.
            return self.hide(out);
        }
        let visible: Vec<_> = scene
            .components
            .iter()
            .filter_map(|c| {
                if let Component::LiveCanvas { id, view, .. } = c {
                    Some((id, *view))
                } else {
                    None
                }
            })
            .collect();
        for (id, layer) in &mut self.layers {
            if !visible.iter().any(|(key, _)| *key == id) {
                if let Some(i) = layer.image.take() {
                    write!(out, "\x1b_Ga=d,d=I,i={i},q=2;\x1b\\")?;
                }
                layer.dirty = true;
            }
        }
        for (id, view) in visible {
            let Some(layer) = self.layers.get_mut(id) else {
                continue;
            };
            crate::content_view::ContentView::new(view.source, view.target)?;
            ensure!(
                u32::from(view.target.x) + u32::from(view.target.width) <= v.width
                    && u32::from(view.target.y) + u32::from(view.target.height) <= v.height,
                "Live canvas outside viewport"
            );
            if layer.view != Some(view) {
                layer.view = Some(view);
                layer.dirty = true;
            }
            if !self.terminal_files
                && layer
                    .presented_at
                    .is_some_and(|t| t.elapsed() < layer.inline_interval)
            {
                continue;
            }
            if !layer.dirty
                && layer
                    .presented_at
                    .is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(2))
            {
                continue;
            }
            layer.files.retain(|f| f.path().exists());
            if layer.files.len() >= 3 {
                continue;
            }
            let [x, y, w, h] = view.fitted();
            let began = std::time::Instant::now();
            let width = (w.round() as u32).max(1);
            let height = (h.round() as u32).max(1);
            let scaled = if layer.pixels.dimensions() == (width, height) {
                std::borrow::Cow::Borrowed(layer.pixels.as_ref())
            } else {
                std::borrow::Cow::Owned(resize_preview(
                    &mut self.resizer,
                    layer.pixels.as_ref(),
                    width,
                    height,
                )?)
            };
            let cw = f64::from(v.width) / f64::from(v.columns);
            let ch = f64::from(v.height) / f64::from(v.rows);
            let col = (x / cw).floor() as u32;
            let row = (y / ch).floor() as u32;
            let ox = (x - f64::from(col) * cw).round() as u32;
            let oy = (y - f64::from(row) * ch).round() as u32;
            let image = IMAGE_ID.fetch_add(1, Ordering::Relaxed);
            write!(out, "\x1b[?2026h\x1b[{};{}H", row + 1, col + 1)?;
            if self.terminal_files {
                self.stats_bytes += scaled.as_raw().len() as u64;
                let mut file = tempfile::Builder::new()
                    .prefix("tty-graphics-protocol-")
                    .tempfile_in(self.root.path())?;
                file.write_all(scaled.as_raw())?;
                use base64::Engine as _;
                let path = base64::engine::general_purpose::STANDARD
                    .encode(file.path().as_os_str().as_encoded_bytes());
                write!(out,"\x1b_Ga=T,f=32,t=t,s={width},v={height},i={image},p=1,C=1,z=1,X={ox},Y={oy},q=2;{path}\x1b\\")?;
                layer.files.push(file);
            } else {
                let encoded = super::renderer::encode_pixels(&scaled)?;
                self.stats_bytes += encoded.len() as u64;
                layer.inline_interval = inline_interval(encoded.len());
                let chunks = encoded.as_bytes().chunks(4096);
                let count = chunks.len();
                for (i, chunk) in chunks.enumerate() {
                    let more = usize::from(i + 1 < count);
                    if i == 0 {
                        write!(
                            out,
                            "\x1b_Ga=T,f=100,t=d,i={image},p=1,C=1,z=1,X={ox},Y={oy},q=2,m={more};"
                        )?;
                    } else {
                        write!(out, "\x1b_Gm={more};")?;
                    }
                    out.write_all(chunk)?;
                    out.write_all(b"\x1b\\")?;
                }
            }
            if let Some(old) = layer.image.replace(image) {
                write!(out, "\x1b_Ga=d,d=I,i={old},q=2;\x1b\\")?;
            }
            out.write_all(b"\x1b[?2026l")?;
            out.flush()?;
            layer.dirty = false;
            self.stats_frames += 1;
            // Encoding and terminal writes count toward the frame interval;
            // adding a complete interval afterwards compounds latency.
            layer.presented_at = Some(began);
        }
        if self.stats_began.elapsed() >= std::time::Duration::from_secs(2) {
            tracing::debug!(
                frames = self.stats_frames,
                bytes = self.stats_bytes,
                elapsed_ms = self.stats_began.elapsed().as_millis(),
                inline = !self.terminal_files,
                "Live canvas presentation"
            );
            self.stats_frames = 0;
            self.stats_bytes = 0;
            self.stats_began = std::time::Instant::now();
        }
        Ok(())
    }
    pub fn mask(&self, scene: &Scene, pixels: Arc<RgbaImage>) -> Arc<RgbaImage> {
        if self.layers.is_empty() {
            return pixels;
        }
        let mut out = pixels.as_ref().clone();
        let mut cleared = false;
        for component in &scene.components {
            if let Component::LiveCanvas { id, view, .. } = component {
                if !self.layers.contains_key(id) {
                    continue;
                }
                let [x, y, w, h] = view.fitted();
                let x = (x.round() as u32).min(out.width());
                let y = (y.round() as u32).min(out.height());
                for yy in y..(y + h.round() as u32).min(out.height()) {
                    for xx in x..(x + w.round() as u32).min(out.width()) {
                        out.put_pixel(xx, yy, crate::image::Rgba([0; 4]));
                    }
                }
                cleared = true;
            } else if cleared
                && matches!(
                    component,
                    Component::Surface { .. }
                        | Component::Menu { .. }
                        | Component::Dialog { .. }
                        | Component::TextField { .. }
                )
            {
                let r = component.rect();
                let v = scene.viewport;
                let (x, y, ex, ey) =
                    if let Some(p) = scene.placements.iter().rev().find(|p| p.source == r) {
                        let t = p.target;
                        (
                            u32::from(t.x),
                            u32::from(t.y),
                            u32::from(t.x) + u32::from(t.width),
                            u32::from(t.y) + u32::from(t.height),
                        )
                    } else {
                        (
                            u32::from(r.x) * v.width / u32::from(v.columns),
                            u32::from(r.y) * v.height / u32::from(v.rows),
                            u32::from(r.x + r.width) * v.width / u32::from(v.columns),
                            u32::from(r.y + r.height) * v.height / u32::from(v.rows),
                        )
                    };
                let ex = ex.min(out.width());
                let ey = ey.min(out.height());
                for yy in y.min(out.height())..ey {
                    for xx in x.min(out.width())..ex {
                        out.put_pixel(xx, yy, *pixels.get_pixel(xx, yy));
                    }
                }
            }
        }
        Arc::new(out)
    }
}

const INLINE_FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
// Legacy remote TTY images have no terminal decode acknowledgement. Keep their
// rate and payload budget conservative; local video frontends use file feedback.
// Slow terminal writes apply backpressure; only the latest decoded frame waits.
fn inline_interval(bytes: usize) -> std::time::Duration {
    std::time::Duration::from_secs_f64(bytes as f64 / (8. * 1024. * 1024.))
        .max(INLINE_FRAME_INTERVAL)
}

// Bilinear convolution preserves the existing Triangle preview filter, with
// SIMD acceleration. Reuse one scratch buffer rather than retaining one per
// stream. Source pixels and input coordinates are unchanged.
fn resize_preview(
    resizer: &mut fast_image_resize::Resizer,
    source: &RgbaImage,
    width: u32,
    height: u32,
) -> Result<RgbaImage> {
    let mut output = RgbaImage::new(width, height);
    let options = fast_image_resize::ResizeOptions::new()
        .resize_alg(fast_image_resize::ResizeAlg::Convolution(
            fast_image_resize::FilterType::Bilinear,
        ))
        .use_alpha(false);
    resizer.resize(source, &mut output, &options)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_canvas::{Frame, PixelFormat, Transport, Update};
    #[test]
    fn accelerated_preview_retains_bilinear_pixels_and_alpha() {
        let source = RgbaImage::from_fn(13, 9, |x, y| {
            crate::image::Rgba([(x * 17) as u8, (y * 29) as u8, 80, ((x + y) * 11) as u8])
        });
        let mut resizer = fast_image_resize::Resizer::new();
        for (width, height) in [(1, 1), (7, 5), (23, 17)] {
            let expected = crate::image::imageops::resize(
                &source,
                width,
                height,
                crate::image::imageops::FilterType::Triangle,
            );
            let actual = resize_preview(&mut resizer, &source, width, height).unwrap();
            assert_eq!(actual.dimensions(), expected.dimensions());
            assert!(actual
                .as_raw()
                .iter()
                .zip(expected.as_raw())
                .all(|(a, b)| a.abs_diff(*b) <= 2));
        }
    }
    #[test]
    fn legacy_inline_preview_limits_decode_load_and_bounds_large_payloads() {
        use std::time::{Duration, Instant};
        assert_eq!(inline_interval(100_000), INLINE_FRAME_INTERVAL);
        assert_eq!(inline_interval(8 * 1024 * 1024), Duration::from_secs(1));
        let mut frontend = Frontend::new().unwrap().with_terminal_files(false);
        let mut transport = Transport::new().unwrap();
        transport
            .submit(Update::Frame {
                stream: "demo".into(),
                generation: 1,
                frame: Frame {
                    width: 1,
                    height: 1,
                    stride: 4,
                    format: PixelFormat::Rgba8,
                    presentation_ns: 0,
                    pixels: Arc::new(vec![10, 20, 30, 255]),
                },
            })
            .unwrap();
        let frame = transport.flush_streamed().unwrap().pop().unwrap();
        frontend.receive_encoded(&frame).unwrap();
        let v = Viewport::default();
        let scene = scene(v, crate::native_surface::PixelRect::new(10, 10, 300, 200));
        frontend.present(&scene, v, &mut vec![]).unwrap();
        frontend.receive_encoded(&frame).unwrap();
        // Advance the presentation clock without sleeping.
        frontend.layers.get_mut("demo").unwrap().presented_at =
            Some(Instant::now() - Duration::from_millis(120));
        let mut output = vec![];
        frontend.present(&scene, v, &mut output).unwrap();
        assert!(!output.is_empty());
        frontend.receive_encoded(&frame).unwrap();
        frontend.layers.get_mut("demo").unwrap().presented_at = Some(Instant::now());
        output.clear();
        frontend.present(&scene, v, &mut output).unwrap();
        assert!(
            output.is_empty(),
            "a burst retains its latest frame rather than flooding the terminal"
        );
        assert!(frontend.layers["demo"].dirty);
    }
    #[test]
    fn ssh_terminal_receives_inline_pixels_without_reading_remote_files() {
        let mut transport = Transport::new().unwrap();
        transport
            .submit(Update::Frame {
                stream: "demo".into(),
                generation: 1,
                frame: Frame {
                    width: 1,
                    height: 1,
                    stride: 4,
                    format: PixelFormat::Rgba8,
                    presentation_ns: 0,
                    pixels: Arc::new(vec![10, 20, 30, 255]),
                },
            })
            .unwrap();
        let frame = transport.flush_streamed().unwrap().pop().unwrap();
        let mut frontend = Frontend::new().unwrap().with_terminal_files(false);
        frontend.receive_encoded(&frame).unwrap();
        let v = Viewport::default();
        let scene = scene(v, crate::native_surface::PixelRect::new(10, 10, 300, 200));
        let mut output = vec![];
        frontend.present(&scene, v, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("f=100,t=d"));
        assert!(!text.contains("t=t"));
        assert!(frontend.layers["demo"].files.is_empty());
        let encoded = text
            .split("\x1b_G")
            .skip(1)
            .filter_map(|s| {
                s.split_once(';')
                    .map(|(_, p)| p.split("\x1b\\").next().unwrap())
            })
            .collect::<String>();
        use base64::Engine as _;
        let png = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let pixels = crate::image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(pixels.width(), 300);
        assert!((pixels.get_pixel(0, 0)[0] as i32 - 10).abs() < 8);
    }
    #[test]
    fn canvas_holes_preserve_overlay_pixels_at_their_physical_placement() {
        use crate::native_surface::{PixelRect, Surface};
        let mut frontend = Frontend::new().unwrap();
        frontend.layers.insert(
            "demo".into(),
            Layer {
                generation: 1,
                pixels: Arc::new(RgbaImage::new(1, 1)),
                dirty: true,
                image: None,
                files: vec![],
                view: None,
                presented_at: None,
                inline_interval: INLINE_FRAME_INTERVAL,
            },
        );
        let viewport = Viewport {
            columns: 4,
            rows: 4,
            width: 16,
            height: 16,
            ..Viewport::default()
        };
        let mut scene = scene(viewport, PixelRect::new(0, 0, 16, 16));
        let rect = super::super::protocol::Rect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        scene.components.push(Component::Surface {
            rect,
            surface: Surface::new(3, 4, "#c86432".into()),
        });
        scene
            .placements
            .push(super::super::placement::Placement::new(
                rect,
                PixelRect::new(3, 5, 3, 4),
            ));
        let original = crate::image::Rgba([200, 100, 50, 255]);
        let masked = frontend.mask(&scene, Arc::new(RgbaImage::from_pixel(16, 16, original)));
        assert_eq!(
            *masked.get_pixel(3, 6),
            original,
            "Modal pixels must stay above the live image"
        );
        assert_eq!(
            masked.get_pixel(2, 6).0,
            [0; 4],
            "The video outside the modal must remain visible"
        );
        assert_eq!(
            *masked.get_pixel(0, 2),
            original,
            "The surrounding recording guide is outside the canvas hole"
        );
    }

    fn scene(viewport: Viewport, target: crate::native_surface::PixelRect) -> Scene {
        let buffer = crate::ratatui::buffer::Buffer::empty(crate::ratatui::layout::Rect::new(
            0,
            0,
            viewport.columns,
            viewport.rows,
        ));
        let mut scene = Scene::from_buffer(&buffer, viewport, 1);
        scene.components.push(Component::LiveCanvas {
            rect: super::super::protocol::Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            id: "demo".into(),
            view: crate::content_view::ContentView::new([1920, 1080], target).unwrap(),
            interactive: true,
        });
        scene
    }

    #[test]
    fn resize_hides_obsolete_geometry_and_reuses_pixels_for_the_new_scene() {
        use crate::native_surface::PixelRect;
        let mut frontend = Frontend::new().unwrap();
        frontend.layers.insert(
            "demo".into(),
            Layer {
                generation: 1,
                pixels: Arc::new(RgbaImage::from_pixel(
                    1,
                    1,
                    crate::image::Rgba([10, 20, 30, 255]),
                )),
                dirty: true,
                image: None,
                files: vec![],
                view: None,
                presented_at: None,
                inline_interval: INLINE_FRAME_INTERVAL,
            },
        );
        let old = Viewport::default();
        let before = scene(old, PixelRect::new(900, 300, 280, 400));
        let mut output = vec![];
        frontend.present(&before, old, &mut output).unwrap();
        assert!(frontend.layers["demo"].image.is_some());
        let smaller = Viewport {
            width: 600,
            height: 400,
            columns: 50,
            rows: 20,
            generation: 2,
        };
        for viewport in [
            smaller,
            Viewport {
                generation: 1,
                ..smaller
            },
            Viewport {
                generation: 2,
                ..old
            },
        ] {
            output.clear();
            frontend.present(&before, viewport, &mut output).unwrap();
            assert!(!String::from_utf8_lossy(&output).contains("a=T"));
            assert!(frontend.layers["demo"].image.is_none());
            assert!(frontend.layers["demo"].dirty);
            assert_eq!(frontend.layers["demo"].pixels.as_raw(), &[10, 20, 30, 255]);
        }
        output.clear();
        let after = scene(smaller, PixelRect::new(20, 20, 550, 350));
        frontend.present(&after, smaller, &mut output).unwrap();
        assert!(String::from_utf8_lossy(&output).contains("a=T"));
        assert!(frontend.layers["demo"].image.is_some());
        output.clear();
        frontend.present(&after, smaller, &mut output).unwrap();
        assert!(
            output.is_empty(),
            "Unchanged live pixels should stay cached"
        );
        frontend.layers.get_mut("demo").unwrap().presented_at =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(3));
        frontend.present(&after, smaller, &mut output).unwrap();
        assert!(
            String::from_utf8_lossy(&output).contains("a=T"),
            "Idle canvases must recover without a producer update"
        );
        // Matching scenes still enforce the canvas bounds contract.
        let invalid = scene(smaller, PixelRect::new(580, 20, 100, 100));
        assert!(frontend.present(&invalid, smaller, &mut output).is_err());
    }

    #[test]
    fn retired_frames_preserve_pixels_and_do_not_hide_capability_errors() {
        let mut transport = Transport::new().unwrap();
        transport
            .submit(Update::Frame {
                stream: "demo".into(),
                generation: 1,
                frame: Frame {
                    width: 1,
                    height: 1,
                    stride: 4,
                    format: PixelFormat::Rgba8,
                    presentation_ns: 0,
                    pixels: Arc::new(vec![10, 20, 30, 255]),
                },
            })
            .unwrap();
        let descriptor = transport.flush().unwrap().pop().unwrap();
        let mut frontend = Frontend::new().unwrap();
        assert!(frontend.receive(&descriptor, true).unwrap());
        transport.acknowledge("demo", 1, descriptor.sequence);
        assert!(!descriptor.path.exists());
        assert!(!frontend.receive(&descriptor, true).unwrap());
        assert_eq!(frontend.layers["demo"].pixels.as_raw(), &[10, 20, 30, 255]);
        assert!(frontend.receive(&descriptor, false).is_err());
        drop(transport);
        assert!(!frontend.receive(&descriptor, true).unwrap());
    }
}
