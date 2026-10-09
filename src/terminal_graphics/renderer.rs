//! Native worker lifecycle and Kitty image presentation.
use std::io::{self, Write};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::image::{ImageDecoder as _, ImageEncoder as _, RgbaImage};
use anyhow::{Context, Result};
use base64::Engine as _;
use crossbeam_channel::{bounded, Receiver, Sender};

use super::protocol::Scene;

#[derive(Debug)]
pub enum RenderMessage {
    Ready,
    Frame {
        revision: u64,
        generation: u64,
        width: u32,
        height: u32,
        pixels: Option<Arc<RgbaImage>>,
    },
    Error {
        message: String,
    },
}

pub struct Renderer {
    live: Sender<(String, Arc<RgbaImage>)>,
    old_live: Receiver<(String, Arc<RgbaImage>)>,
    input: Option<Sender<Scene>>,
    stale: Receiver<Scene>,
    pub output: Receiver<RenderMessage>,
    worker: Option<JoinHandle<()>>,
}
impl Renderer {
    pub fn spawn() -> Result<Self> {
        Self::spawn_with_font(super::font::Font::default().configured())
    }
    pub(crate) fn spawn_with_font(font: super::font::Font) -> Result<Self> {
        Self::spawn_with_options(font, super::client::PresentationOptions::default())
    }
    pub(crate) fn spawn_with_options(
        font: super::font::Font,
        options: super::client::PresentationOptions,
    ) -> Result<Self> {
        let (input, scenes) = bounded::<Scene>(1);
        let stale = scenes.clone();
        let (live, images) = bounded::<(String, Arc<RgbaImage>)>(1);
        let old_live = images.clone();
        let (frames, output) = bounded(2);
        let old = output.clone();
        let worker = std::thread::Builder::new()
            .name("star-native-renderer".into())
            .spawn(move || {
                let mut painter = super::native::Painter::with_pane_corner_radius(
                    font,
                    options.pane_corner_radius,
                );
                let mut previous = None;
                loop {
                    let timeout = if painter.animating() { std::time::Duration::from_millis(20) } else { std::time::Duration::from_secs(3600) };
                    let received = crossbeam_channel::select_biased! {
                        recv(scenes) -> result => result.map_err(|_| crossbeam_channel::RecvTimeoutError::Disconnected),
                        recv(images) -> result => {
                            if let Ok((id, pixels)) = result { painter.live_image(id, pixels); }
                            if let Some(scene) = previous.clone() { Ok(scene) } else { continue; }
                        },
                        default(timeout) => Err(crossbeam_channel::RecvTimeoutError::Timeout),
                    };
                    let mut scene = match received {
                        Ok(scene) => scenes.try_iter().last().unwrap_or(scene),
                        Err(crossbeam_channel::RecvTimeoutError::Timeout)
                            if painter.animation_due() =>
                        {
                            let Some(scene) = previous.clone() else {
                                continue;
                            };
                            scene
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    };
                    let started = std::time::Instant::now();
                    let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        painter.render(&scene)
                    }));
                    let message = match rendered {
                        Ok(Ok(pixels)) => RenderMessage::Frame {
                            revision: scene.revision,
                            generation: scene.viewport.generation,
                            width: pixels.width(),
                            height: pixels.height(),
                            pixels: Some(Arc::new(pixels)),
                        },
                        Ok(Err(error)) => RenderMessage::Error {
                            message: format!("Native renderer: {error:#}"),
                        },
                        Err(_) => RenderMessage::Error {
                            message: "Native renderer worker failed".into(),
                        },
                    };
                    tracing::debug!(
                        revision = scene.revision,
                        render_us = started.elapsed().as_micros(),
                        "Native scene rasterized"
                    );
                    // The painter owns decoded assets now. Timer frames must not
                    // clone or recompare a potentially large encoded animation.
                    for component in &mut scene.components {
                        if let super::protocol::Component::Image { png, .. } = component {
                            *png = None;
                        }
                    }
                    previous = Some(scene);
                    let failed = matches!(message, RenderMessage::Error { .. });
                    if let Err(crossbeam_channel::TrySendError::Full(message)) =
                        frames.try_send(message)
                    {
                        let _ = old.try_recv();
                        let _ = frames.try_send(message);
                    }
                    if failed {
                        break;
                    }
                }
            })?;
        tracing::info!("Native Rust graphical renderer started");
        Ok(Self {
            live,
            old_live,
            input: Some(input),
            stale,
            output,
            worker: Some(worker),
        })
    }
    pub fn live_image(&mut self, id: String, pixels: Arc<RgbaImage>) {
        if let Err(crossbeam_channel::TrySendError::Full(item)) = self.live.try_send((id, pixels)) {
            let _ = self.old_live.try_recv();
            let _ = self.live.try_send(item);
        }
    }
    pub fn scene(&mut self, scene: &Scene) -> Result<()> {
        let input = self.input.as_ref().context("Native renderer closed")?;
        if let Err(error) = input.try_send(scene.clone()) {
            match error {
                crossbeam_channel::TrySendError::Full(scene) => {
                    let _ = self.stale.try_recv();
                    // The consumer can race the replacement. A full queue
                    // retains a newer pending scene rather than blocking input.
                    if let Err(crossbeam_channel::TrySendError::Disconnected(_)) =
                        input.try_send(scene)
                    {
                        anyhow::bail!("Native renderer stopped");
                    }
                }
                crossbeam_channel::TrySendError::Disconnected(_) => {
                    anyhow::bail!("Native renderer stopped")
                }
            }
        }
        Ok(())
    }
    pub fn clipboard(&mut self, text: &str) -> Result<()> {
        super::cells::clipboard(text)
    }
    pub fn alive(&mut self) -> Result<bool> {
        Ok(self
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished()))
    }
}
impl Drop for Renderer {
    fn drop(&mut self) {
        self.input.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// A stable placement is replaced atomically. Never clear between frames.
pub struct KittyPresenter {
    next: u32,
    previous: Option<u32>,
    cached: Option<(String, u16, u16)>,
    regions: Option<RegionCache>,
}
struct RegionCache {
    pixels: Arc<RgbaImage>,
    columns: u16,
    rows: u16,
    placements: Vec<(super::protocol::Rect, u32)>,
}
impl Default for KittyPresenter {
    fn default() -> Self {
        Self {
            next: 0x534b0000,
            previous: None,
            cached: None,
            regions: None,
        }
    }
}
impl KittyPresenter {
    pub fn present(
        &mut self,
        png: &str,
        columns: u16,
        rows: u16,
        out: &mut impl Write,
    ) -> io::Result<usize> {
        if png.len() > 15_000_000
            || !png
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid PNG payload",
            ));
        }
        if self
            .cached
            .as_ref()
            .is_some_and(|(old, cols, lines)| old == png && *cols == columns && *lines == rows)
        {
            // Still acknowledge this scene revision: its input targets may
            // change even when its pixels do not.
            return Ok(0);
        }
        self.next = self.next.wrapping_add(1);
        let id = self.next;
        write!(out, "\x1b[?2026h\x1b[H")?;
        let chunks: Vec<_> = png.as_bytes().chunks(4096).collect();
        for (index, chunk) in chunks.iter().enumerate() {
            let more = usize::from(index + 1 < chunks.len());
            if index == 0 {
                write!(
                    out,
                    "\x1b_Ga=T,f=100,t=d,z=2,i={id},p=1,q=2,C=1,c={columns},r={rows},m={more};"
                )?;
            } else {
                write!(out, "\x1b_Gm={more};")?;
            }
            out.write_all(chunk)?;
            out.write_all(b"\x1b\\")?;
        }
        if let Some(old) = self.previous {
            write!(out, "\x1b_Ga=d,d=I,i={old},q=2;\x1b\\")?;
        }
        if let Some(regions) = self.regions.take() {
            for (_, old) in regions.placements {
                write!(out, "\x1b_Ga=d,d=I,i={old},q=2;\x1b\\")?;
            }
        }
        out.write_all(b"\x1b[?2026l")?;
        out.flush()?;
        self.previous = Some(id);
        self.cached = Some((png.to_owned(), columns, rows));
        Ok(png.len())
    }
    pub fn clear(&mut self, out: &mut impl Write) -> io::Result<()> {
        self.cached = None;
        if let Some(id) = self.previous.take() {
            write!(out, "\x1b_Ga=d,d=I,i={id},q=2;\x1b\\")?;
        }
        if let Some(regions) = self.regions.take() {
            for (_, id) in regions.placements {
                write!(out, "\x1b_Ga=d,d=I,i={id},q=2;\x1b\\")?;
            }
        }
        out.flush()
    }

    /// Compare a self-contained renderer frame with the pixels actually sent
    /// to the terminal. Dropped renderer frames never become delta bases.
    pub fn present_regions(
        &mut self,
        png: &str,
        viewport: super::protocol::Viewport,
        out: &mut impl Write,
    ) -> anyhow::Result<usize> {
        viewport.validate()?;
        anyhow::ensure!(png.len() <= 15_000_000, "Frame exceeds encoded limit");
        let same_geometry = self.regions.as_ref().is_some_and(|old| {
            old.columns == viewport.columns
                && old.rows == viewport.rows
                && old.pixels.dimensions() == (viewport.width, viewport.height)
        });
        if same_geometry && self.cached.as_ref().is_some_and(|(old, _, _)| old == png) {
            return Ok(0);
        }
        let bytes = base64::engine::general_purpose::STANDARD.decode(png)?;
        let mut decoder = crate::image::codecs::png::PngDecoder::new(std::io::Cursor::new(bytes))?;
        anyhow::ensure!(
            decoder.dimensions() == (viewport.width, viewport.height),
            "Frame dimensions do not match viewport"
        );
        let mut limits = crate::image::Limits::default();
        limits.max_alloc = Some(128_000_000);
        decoder.set_limits(limits)?;
        let pixels = crate::image::DynamicImage::from_decoder(decoder)?.into_rgba8();
        if viewport.width < u32::from(viewport.columns)
            || viewport.height < u32::from(viewport.rows)
        {
            return Ok(self.present(
                &encode_pixels(&pixels)?,
                viewport.columns,
                viewport.rows,
                out,
            )?);
        }
        self.present_pixels(Arc::new(pixels), viewport, out)
    }

    pub fn present_pixels(
        &mut self,
        pixels: Arc<RgbaImage>,
        viewport: super::protocol::Viewport,
        out: &mut impl Write,
    ) -> anyhow::Result<usize> {
        viewport.validate()?;
        anyhow::ensure!(
            pixels.dimensions() == (viewport.width, viewport.height),
            "Frame dimensions do not match viewport"
        );
        if viewport.width < u32::from(viewport.columns)
            || viewport.height < u32::from(viewport.rows)
        {
            return Ok(self.present(
                &encode_pixels(&pixels)?,
                viewport.columns,
                viewport.rows,
                out,
            )?);
        }
        let same_geometry = self.regions.as_ref().is_some_and(|old| {
            old.columns == viewport.columns
                && old.rows == viewport.rows
                && old.pixels.dimensions() == pixels.dimensions()
        });
        let grid = region_grid(viewport.columns, viewport.rows);
        let mut changes = Vec::new();
        let mut payload_bytes = 0;
        for (index, rect) in grid.iter().copied().enumerate() {
            let (x, y, width, height) = pixel_rect(rect, viewport);
            let unchanged = same_geometry
                && self.regions.as_ref().is_some_and(|old| {
                    (y..y + height).all(|row| {
                        let start = ((row * viewport.width + x) * 4) as usize;
                        let end = start + width as usize * 4;
                        pixels.as_raw()[start..end] == old.pixels.as_raw()[start..end]
                    })
                });
            if unchanged {
                continue;
            }
            let crop =
                crate::image::imageops::crop_imm(pixels.as_ref(), x, y, width, height).to_image();
            let mut encoded = Vec::new();
            crate::image::codecs::png::PngEncoder::new_with_quality(
                &mut encoded,
                crate::image::codecs::png::CompressionType::Fast,
                crate::image::codecs::png::FilterType::Adaptive,
            )
            .write_image(
                crop.as_raw(),
                width,
                height,
                crate::image::ExtendedColorType::Rgba8,
            )?;
            let encoded = base64::engine::general_purpose::STANDARD.encode(encoded);
            payload_bytes += encoded.len();
            // Noise-heavy frames can compress better as one image. Preserve
            // the existing wire ceiling instead of accumulating huge patches.
            if payload_bytes > 15_000_000 {
                return Ok(self.present(
                    &encode_pixels(&pixels)?,
                    viewport.columns,
                    viewport.rows,
                    out,
                )?);
            }
            changes.push((index, rect, encoded));
        }
        let mut placements = if same_geometry {
            self.regions.as_ref().unwrap().placements.clone()
        } else {
            grid.into_iter().map(|rect| (rect, 0)).collect()
        };
        let mut retired = Vec::new();
        if !same_geometry {
            if let Some(old) = &self.regions {
                retired.extend(old.placements.iter().map(|(_, id)| *id));
            }
        }
        if let Some(id) = self.previous {
            retired.push(id);
        }
        if !changes.is_empty() {
            out.write_all(b"\x1b[?2026h")?;
            for (index, rect, encoded) in changes {
                self.next = self.next.wrapping_add(1);
                let id = self.next;
                if same_geometry {
                    retired.push(placements[index].1);
                }
                placements[index] = (rect, id);
                write!(out, "\x1b[{};{}H", rect.y + 1, rect.x + 1)?;
                let chunks = encoded.as_bytes().chunks(4096);
                let count = chunks.len();
                for (part, chunk) in chunks.enumerate() {
                    let more = usize::from(part + 1 < count);
                    if part == 0 {
                        write!(
                            out,
                            "\x1b_Ga=T,f=100,t=d,z=2,i={id},p=1,q=2,C=1,c={},r={},m={more};",
                            rect.width, rect.height
                        )?;
                    } else {
                        write!(out, "\x1b_Gm={more};")?;
                    }
                    out.write_all(chunk)?;
                    out.write_all(b"\x1b\\")?;
                }
            }
            // All replacement regions are placed before retiring any old
            // region, including after resize or a complete-frame fallback.
            for old in retired {
                write!(out, "\x1b_Ga=d,d=I,i={old},q=2;\x1b\\")?;
            }
            out.write_all(b"\x1b[?2026l")?;
            out.flush()?;
        }
        self.previous = None;
        self.cached = None;
        self.regions = Some(RegionCache {
            pixels,
            columns: viewport.columns,
            rows: viewport.rows,
            placements,
        });
        Ok(payload_bytes)
    }
}

pub fn encode_pixels(pixels: &RgbaImage) -> anyhow::Result<String> {
    let mut bytes = Vec::new();
    crate::image::codecs::png::PngEncoder::new_with_quality(
        &mut bytes,
        crate::image::codecs::png::CompressionType::Fast,
        crate::image::codecs::png::FilterType::Adaptive,
    )
    .write_image(
        pixels.as_raw(),
        pixels.width(),
        pixels.height(),
        crate::image::ExtendedColorType::Rgba8,
    )?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn region_grid(columns: u16, rows: u16) -> Vec<super::protocol::Rect> {
    let mut width = 32u16;
    let mut height = 8u16;
    while u32::from(columns.div_ceil(width)) * u32::from(rows.div_ceil(height)) > 256 {
        if columns.div_ceil(width) > rows.div_ceil(height) {
            width = width.saturating_mul(2);
        } else {
            height = height.saturating_mul(2);
        }
    }
    let mut grid = Vec::new();
    for y in (0..rows).step_by(usize::from(height)) {
        for x in (0..columns).step_by(usize::from(width)) {
            grid.push(super::protocol::Rect {
                x,
                y,
                width: width.min(columns - x),
                height: height.min(rows - y),
            });
        }
    }
    grid
}

fn pixel_rect(
    rect: super::protocol::Rect,
    viewport: super::protocol::Viewport,
) -> (u32, u32, u32, u32) {
    let x = u32::from(rect.x) * viewport.width / u32::from(viewport.columns);
    let y = u32::from(rect.y) * viewport.height / u32::from(viewport.rows);
    let right = u32::from(rect.x + rect.width) * viewport.width / u32::from(viewport.columns);
    let bottom = u32::from(rect.y + rect.height) * viewport.height / u32::from(viewport.rows);
    (x, y, right - x, bottom - y)
}

/// Render a self-contained scene without opening a display or terminal.
/// Useful for application visual acceptance fixtures and exported previews.
pub fn render_reference(scene: &Scene) -> anyhow::Result<RgbaImage> {
    super::native::Painter::with_font(super::font::Font::default().configured()).render(scene)
}

/// Draw native text and shapes over a skin bitmap without clearing its background.
/// The surface and bitmap must have identical pixel dimensions. No scaling occurs.
pub fn render_surface_overlay(
    base: &RgbaImage,
    surface: &super::surface::Surface,
) -> anyhow::Result<RgbaImage> {
    super::native::Painter::with_font(super::font::Font::default().configured())
        .overlay(base, surface)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn skin_overlay_preserves_background_and_checks_dimensions() {
        let base = RgbaImage::from_pixel(20, 20, crate::image::Rgba([30, 40, 50, 255]));
        let mut surface = super::super::surface::Surface::new(20, 20, "#ffffff".into());
        assert_eq!(render_surface_overlay(&base, &surface).unwrap(), base);
        surface.nodes.push(super::super::surface::Primitive::Fill {
            rect: crate::native_surface::PixelRect::new(5, 5, 5, 5),
            color: "#ff0000".into(),
            radius: 0,
        });
        let drawn = render_surface_overlay(&base, &surface).unwrap();
        assert_eq!(drawn.get_pixel(0, 0), base.get_pixel(0, 0));
        assert_eq!(drawn.get_pixel(7, 7).0, [255, 0, 0, 255]);
        surface.width = 21;
        assert!(render_surface_overlay(&base, &surface).is_err());
    }

    #[test]
    fn native_worker_reports_invalid_data_and_shuts_down_without_a_runtime() {
        let mut renderer = Renderer::spawn().unwrap();
        let mut scene = super::super::Scene::from_buffer(
            &crate::ratatui::buffer::Buffer::empty(crate::ratatui::layout::Rect::new(
                0, 0, 100, 40,
            )),
            super::super::Viewport::default(),
            1,
        );
        scene.components.push(super::super::Component::Image {
            rect: super::super::Rect {
                x: 1,
                y: 1,
                width: 10,
                height: 10,
            },
            id: "broken".into(),
            png: Some("invalid".into()),
            scale: Default::default(),
            zoom: 100,
        });
        renderer.scene(&scene).unwrap();
        assert!(matches!(
            renderer
                .output
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            RenderMessage::Error { .. }
        ));
        // Drop joins the bounded worker; no subprocess remains to kill/reap.
        drop(renderer);
    }

    fn encode_frame(image: &RgbaImage) -> String {
        let mut bytes = Vec::new();
        crate::image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                crate::image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    // Interpret emitted image uploads/deletions independently of the cache.
    // Compose the remaining placements to catch missing/incorrect patches.
    fn apply_wire(wire: &[u8], images: &mut BTreeMap<u32, (u32, u32, RgbaImage)>) {
        let wire = std::str::from_utf8(wire).unwrap();
        let mut rest = wire;
        let mut position = (0, 0);
        let mut id = 0;
        let mut payload = String::new();
        while let Some(start) = rest.find("\x1b_G") {
            if let Some(cursor) = rest[..start]
                .rsplit("\x1b[")
                .next()
                .and_then(|s| s.strip_suffix('H'))
            {
                let (row, column) = cursor.split_once(';').unwrap_or(("1", "1"));
                position = (
                    column.parse::<u32>().unwrap() - 1,
                    row.parse::<u32>().unwrap() - 1,
                );
            }
            rest = &rest[start + 3..];
            let end = rest.find("\x1b\\").unwrap();
            let (header, chunk) = rest[..end].split_once(';').unwrap();
            let fields: BTreeMap<_, _> = header
                .split(',')
                .filter_map(|s| s.split_once('='))
                .collect();
            if fields.get("a") == Some(&"d") {
                images.remove(&fields["i"].parse().unwrap());
            } else {
                if let Some(value) = fields.get("i") {
                    id = value.parse().unwrap();
                    payload.clear();
                }
                payload.push_str(chunk);
                if fields.get("m") == Some(&"0") {
                    let png = base64::engine::general_purpose::STANDARD
                        .decode(&payload)
                        .unwrap();
                    let image = crate::image::load_from_memory(&png).unwrap().into_rgba8();
                    images.insert(id, (position.0 * 4, position.1 * 8, image));
                }
            }
            rest = &rest[end + 2..];
        }
    }

    #[test]
    fn dirty_regions_reconstruct_skipped_frames_and_retire_replaced_images() {
        let viewport = super::super::protocol::Viewport {
            columns: 80,
            rows: 20,
            width: 320,
            height: 160,
            generation: 1,
        };
        let mut frame = RgbaImage::from_pixel(320, 160, crate::image::Rgba([20, 30, 40, 255]));
        let mut presenter = KittyPresenter::default();
        let mut images = BTreeMap::new();
        let mut wire = Vec::new();
        let full = presenter
            .present_regions(&encode_frame(&frame), viewport, &mut wire)
            .unwrap();
        apply_wire(&wire, &mut images);
        assert_eq!(images.len(), 9);
        // A renderer may skip an intermediate scene. Compare the next
        // complete frame to the last presented pixels, never to that scene.
        frame.put_pixel(140, 70, crate::image::Rgba([250, 80, 70, 255]));
        frame.put_pixel(141, 71, crate::image::Rgba([70, 250, 80, 255]));
        wire.clear();
        let delta = presenter
            .present_regions(&encode_frame(&frame), viewport, &mut wire)
            .unwrap();
        assert!(delta < full / 2, "delta {delta}, full {full}");
        let commands = std::str::from_utf8(&wire).unwrap();
        assert_eq!(commands.matches("a=T").count(), 1);
        assert_eq!(commands.matches("a=d").count(), 1);
        assert!(commands.find("a=T").unwrap() < commands.find("a=d").unwrap());
        apply_wire(&wire, &mut images);
        assert_eq!(images.len(), 9);
        let mut composed = RgbaImage::new(320, 160);
        for (x, y, image) in images.values() {
            crate::image::imageops::replace(&mut composed, image, i64::from(*x), i64::from(*y));
        }
        assert_eq!(composed, frame);
        wire.clear();
        assert_eq!(
            presenter
                .present_regions(&encode_frame(&frame), viewport, &mut wire)
                .unwrap(),
            0
        );
        assert!(wire.is_empty());
        presenter.clear(&mut wire).unwrap();
        apply_wire(&wire, &mut images);
        assert!(images.is_empty());
    }

    #[test]
    fn region_grid_is_bounded_and_covers_fractional_cell_geometry() {
        for (columns, rows, width, height) in
            [(97, 47, 1850, 1998), (1, 8192, 1, 8192), (8192, 1, 8192, 1)]
        {
            let viewport = super::super::protocol::Viewport {
                columns,
                rows,
                width,
                height,
                generation: 1,
            };
            let grid = region_grid(columns, rows);
            assert!(grid.len() <= 256);
            let mut covered = vec![false; width as usize * height as usize];
            for rect in grid {
                let (x, y, w, h) = pixel_rect(rect, viewport);
                for row in y..y + h {
                    for col in x..x + w {
                        let index = (row * width + col) as usize;
                        assert!(!covered[index]);
                        covered[index] = true;
                    }
                }
            }
            assert!(covered.into_iter().all(|pixel| pixel));
        }
    }

    #[test]
    fn invalid_frame_dimensions_do_not_replace_a_presented_surface() {
        let viewport = super::super::protocol::Viewport {
            columns: 80,
            rows: 20,
            width: 320,
            height: 160,
            generation: 1,
        };
        let png = encode_frame(&RgbaImage::new(320, 160));
        let mut presenter = KittyPresenter::default();
        let mut wire = Vec::new();
        presenter
            .present_regions(&png, viewport, &mut wire)
            .unwrap();
        wire.clear();
        assert!(presenter
            .present_regions(
                &png,
                super::super::protocol::Viewport {
                    width: 640,
                    ..viewport
                },
                &mut wire
            )
            .is_err());
        assert!(wire.is_empty());
        assert_eq!(
            presenter
                .present_regions(&png, viewport, &mut wire)
                .unwrap(),
            0
        );
    }

    #[test]
    fn resize_and_complete_frame_transitions_leave_no_retired_placements() {
        let viewport = super::super::protocol::Viewport {
            columns: 80,
            rows: 20,
            width: 320,
            height: 160,
            generation: 1,
        };
        let mut frame = RgbaImage::new(320, 160);
        let mut presenter = KittyPresenter::default();
        let mut images = BTreeMap::new();
        let mut wire = Vec::new();
        let png = encode_frame(&frame);
        presenter
            .present_regions(&png, viewport, &mut wire)
            .unwrap();
        apply_wire(&wire, &mut images);
        let resized = super::super::protocol::Viewport {
            columns: 40,
            generation: 2,
            ..viewport
        };
        wire.clear();
        presenter.present_regions(&png, resized, &mut wire).unwrap();
        assert_eq!(
            std::str::from_utf8(&wire).unwrap().matches("a=T").count(),
            6
        );
        apply_wire(&wire, &mut images);
        assert_eq!(images.len(), 6);
        frame.put_pixel(0, 0, crate::image::Rgba([255, 255, 255, 255]));
        let png = encode_frame(&frame);
        wire.clear();
        presenter.present(&png, 40, 20, &mut wire).unwrap();
        apply_wire(&wire, &mut images);
        assert_eq!(images.len(), 1);
        wire.clear();
        presenter.present_regions(&png, resized, &mut wire).unwrap();
        apply_wire(&wire, &mut images);
        assert_eq!(images.len(), 6);
        wire.clear();
        presenter.clear(&mut wire).unwrap();
        apply_wire(&wire, &mut images);
        assert!(images.is_empty());
    }
    #[test]
    fn repaint_and_resize_burst_reconstructs_every_presented_frame() {
        let mut presenter = KittyPresenter::default();
        let mut images = BTreeMap::new();
        let mut wire = Vec::new();
        for step in 0..96u32 {
            let columns = 80 + (step / 8 % 3) as u16 * 8;
            let rows = 20 + (step / 8 % 2) as u16 * 4;
            let viewport = super::super::protocol::Viewport {
                columns,
                rows,
                width: u32::from(columns) * 4,
                height: u32::from(rows) * 8,
                generation: u64::from(step / 8 + 1),
            };
            let mut frame = RgbaImage::from_pixel(
                viewport.width,
                viewport.height,
                crate::image::Rgba([20, 30, 40, 255]),
            );
            for pixel in 0..17 {
                frame.put_pixel(
                    (step * 19 + pixel * 7) % viewport.width,
                    (step * 13 + pixel * 11) % viewport.height,
                    crate::image::Rgba([step as u8, pixel as u8, 200, 255]),
                );
            }
            wire.clear();
            presenter
                .present_regions(&encode_frame(&frame), viewport, &mut wire)
                .unwrap();
            apply_wire(&wire, &mut images);
            assert!(images.len() <= 256);
            let mut composed = RgbaImage::new(viewport.width, viewport.height);
            for (x, y, image) in images.values() {
                crate::image::imageops::replace(&mut composed, image, i64::from(*x), i64::from(*y));
            }
            assert_eq!(composed, frame, "incorrect pixels at burst step {step}");
        }
        wire.clear();
        presenter.clear(&mut wire).unwrap();
        apply_wire(&wire, &mut images);
        assert!(images.is_empty());
    }

    #[test]
    fn unchanged_pixels_reuse_placement_but_geometry_and_cleanup_invalidate_it() {
        let mut p = KittyPresenter::default();
        let mut out = Vec::new();
        assert_eq!(p.present("AAAA", 80, 24, &mut out).unwrap(), 4);
        out.clear();
        assert_eq!(p.present("AAAA", 80, 24, &mut out).unwrap(), 0);
        assert!(out.is_empty());
        assert_eq!(p.present("AAAA", 81, 24, &mut out).unwrap(), 4);
        assert!(!out.is_empty());
        p.clear(&mut out).unwrap();
        out.clear();
        assert_eq!(p.present("AAAA", 81, 24, &mut out).unwrap(), 4);
        assert!(!out.is_empty());
    }

    #[test]
    fn replaces_before_retiring_and_rejects_escape_injection() {
        let mut p = KittyPresenter::default();
        let mut out = vec![];
        p.present("AAAA", 80, 24, &mut out).unwrap();
        out.clear();
        p.present("BBBB", 80, 24, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.find("a=T").unwrap() < s.find("a=d").unwrap());
        assert!(p.present("\x1b", 80, 24, &mut vec![]).is_err());
    }
}
