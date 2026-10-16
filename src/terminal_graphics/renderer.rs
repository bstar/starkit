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
        /// Exact layout rasterized into these pixels, including local hover.
        /// Never compose a queued frame against a newer controller scene.
        scene: Arc<Scene>,
    },
    Error {
        message: String,
    },
}

pub struct Renderer {
    live: Sender<(String, Arc<RgbaImage>)>,
    old_live: Receiver<(String, Arc<RgbaImage>)>,
    input: Option<Sender<(Scene, Vec<String>)>>,
    stale: Receiver<(Scene, Vec<String>)>,
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
        let (input, scenes) = bounded::<(Scene, Vec<String>)>(1);
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
                    let (mut scene, active_canvases) = match received {
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
                        painter.render_with_canvases(&scene, &active_canvases)
                    }));
                    // Decoded assets belong to the painter. Presentation only
                    // needs layout, so frame snapshots do not duplicate artwork.
                    for component in &mut scene.components {
                        match component {
                            super::protocol::Component::Image { png, .. } => *png = None,
                            super::protocol::Component::Surface { surface, .. } => {
                                for png in surface.assets.values_mut() { *png = None; }
                            }
                            _ => {}
                        }
                    }
                    let message = match rendered {
                        Ok(Ok(pixels)) => RenderMessage::Frame {
                            revision: scene.revision,
                            generation: scene.viewport.generation,
                            width: pixels.width(),
                            height: pixels.height(),
                            pixels: Some(Arc::new(pixels)),
                            scene: Arc::new(scene.clone()),
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
                    let failed = matches!(message, RenderMessage::Error { .. });
                    if let RenderMessage::Error { message } = &message {
                        tracing::error!(revision = scene.revision, generation = scene.viewport.generation, %message, "Native renderer failed");
                    }
                    previous = Some((scene, active_canvases));
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
        self.scene_with_canvases(scene, Vec::new())
    }
    pub(crate) fn scene_with_canvases(&mut self, scene: &Scene, active: Vec<String>) -> Result<()> {
        let input = self.input.as_ref().context("Native renderer closed")?;
        if let Err(error) = input.try_send((scene.clone(), active)) {
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
    invalidated: bool,
    repair_cursor: usize,
    repaired_at: std::time::Instant,
}
struct RegionCache {
    pixels: Arc<RgbaImage>,
    columns: u16,
    rows: u16,
    generation: u64,
    placements: Vec<(super::protocol::Rect, u32)>,
    encoded: Vec<String>,
}
impl Default for KittyPresenter {
    fn default() -> Self {
        Self {
            next: 0x534b0000,
            previous: None,
            cached: None,
            regions: None,
            invalidated: false,
            repair_cursor: 0,
            repaired_at: std::time::Instant::now(),
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
        if !self.invalidated
            && self
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
        self.invalidated = false;
        Ok(png.len())
    }
    pub fn clear(&mut self, out: &mut impl Write) -> io::Result<()> {
        self.cached = None;
        self.invalidated = false;
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

    /// Terminal placements are disposable. Restore from the retained framebuffer
    /// without asking the application to rebuild state or restarting its renderer.
    pub fn repaint(&mut self, out: &mut impl Write) -> anyhow::Result<usize> {
        self.invalidated = true;
        if let Some(old) = &self.regions {
            let pixels = old.pixels.clone();
            let viewport = super::protocol::Viewport {
                columns: old.columns,
                rows: old.rows,
                width: pixels.width(),
                height: pixels.height(),
                generation: old.generation,
            };
            self.present_pixels(pixels, viewport, out)
        } else if let Some((png, columns, rows)) = self.cached.clone() {
            Ok(self.present(&png, columns, rows, out)?)
        } else {
            Ok(0)
        }
    }

    /// Refresh a bounded portion of the terminal's backing store, including
    /// unchanged regions. Quiet panels must recover even while another animates.
    /// Reuse encoded pixels rather than rerasterizing or reencoding the desktop.
    pub fn maintain(&mut self, out: &mut impl Write) -> anyhow::Result<usize> {
        self.maintain_at(std::time::Instant::now(), out)
    }

    fn maintain_at(
        &mut self,
        now: std::time::Instant,
        out: &mut impl Write,
    ) -> anyhow::Result<usize> {
        let count = self
            .regions
            .as_ref()
            .map_or(1, |r| r.placements.len())
            .max(1);
        let interval = std::time::Duration::from_secs(2) / count as u32;
        if now.saturating_duration_since(self.repaired_at) < interval {
            return Ok(0);
        }
        let Some(regions) = &mut self.regions else {
            self.repaired_at = now;
            return self.repaint(out);
        };
        // Catch up after expensive raster/live-frame work without an unbounded
        // upload burst. A slow event loop must not starve quiet regions forever.
        let due = (now.saturating_duration_since(self.repaired_at).as_nanos() / interval.as_nanos())
            .min(count as u128)
            .min(8) as usize;
        let mut payload = 0;
        let mut repaired = 0;
        let mut retired = Vec::new();
        out.write_all(b"\x1b[?2026h")?;
        for _ in 0..due {
            let index = self.repair_cursor % regions.placements.len();
            self.repair_cursor = self.repair_cursor.wrapping_add(1);
            let (rect, old) = regions.placements[index];
            self.next = self.next.wrapping_add(1);
            let image = self.next;
            write_region(out, image, rect, &regions.encoded[index])?;
            retired.push(old);
            regions.placements[index].1 = image;
            payload += regions.encoded[index].len();
            repaired += 1;
            if payload >= 256_000 {
                break;
            }
        }
        for old in retired {
            write!(out, "\x1b_Ga=d,d=I,i={old},q=2;\x1b\\")?;
        }
        out.write_all(b"\x1b[?2026l")?;
        out.flush()?;
        self.repaired_at += interval * repaired;
        Ok(payload)
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
        let same_geometry = !self.invalidated
            && self.regions.as_ref().is_some_and(|old| {
                old.generation == viewport.generation
                    && old.columns == viewport.columns
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
        let same_geometry = !self.invalidated
            && self.regions.as_ref().is_some_and(|old| {
                old.generation == viewport.generation
                    && old.columns == viewport.columns
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
        let mut retained = if same_geometry {
            self.regions
                .as_ref()
                .unwrap()
                .encoded
                .iter()
                .map(String::len)
                .sum::<usize>()
        } else {
            0
        };
        for (index, _, encoded) in &changes {
            if same_geometry {
                retained -= self.regions.as_ref().unwrap().encoded[*index].len();
            }
            retained += encoded.len();
        }
        if retained > 15_000_000 {
            return Ok(self.present(
                &encode_pixels(&pixels)?,
                viewport.columns,
                viewport.rows,
                out,
            )?);
        }
        let mut placements = if same_geometry {
            self.regions.as_ref().unwrap().placements.clone()
        } else {
            grid.into_iter().map(|rect| (rect, 0)).collect()
        };
        let mut encoded_regions = if same_geometry {
            std::mem::take(&mut self.regions.as_mut().unwrap().encoded)
        } else {
            vec![String::new(); placements.len()]
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
                write_region(out, id, rect, &encoded)?;
                encoded_regions[index] = encoded;
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
        self.invalidated = false;
        self.regions = Some(RegionCache {
            pixels,
            columns: viewport.columns,
            rows: viewport.rows,
            generation: viewport.generation,
            placements,
            encoded: encoded_regions,
        });
        Ok(payload_bytes)
    }
}

fn write_region(
    out: &mut impl Write,
    id: u32,
    rect: super::protocol::Rect,
    encoded: &str,
) -> io::Result<()> {
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
    Ok(())
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

/// Reusable native compositor for bitmap skins. Retains shaped text and glyph caches.
/// Explicit font families make visual fixtures independent of terminal preferences.
pub struct SurfaceOverlayRenderer {
    painter: super::native::Painter,
}
impl SurfaceOverlayRenderer {
    pub fn new(font_family: &str) -> Self {
        Self {
            painter: super::native::Painter::with_font(super::font::Font {
                name: Some(font_family.into()),
                ..Default::default()
            }),
        }
    }
    pub fn render(
        &mut self,
        base: &RgbaImage,
        surface: &super::surface::Surface,
    ) -> anyhow::Result<RgbaImage> {
        self.painter.overlay(base, surface)
    }
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
    fn live_canvas_worker_preserves_overlay_alpha_and_stream_lifecycle() {
        use super::super::protocol::{Component, Viewport};
        use crate::{
            content_view::ContentView,
            native_surface::{style::Style, PixelRect as R, Surface},
        };
        let style = Style::default().with_soft_workspace();
        let viewport = Viewport {
            width: 796,
            height: 407,
            columns: 79,
            rows: 21,
            ..Viewport::default()
        };
        let mut scene = style.scene(viewport);
        style
            .mount(&mut scene, Surface::new(796, 407, "#345678".into()), 0, 0)
            .unwrap();
        let area = R::new(31, 25, 730, 360);
        scene.components.push(Component::LiveCanvas {
            rect: crate::native_surface::style::logical_rect(area, viewport),
            id: "desktop".into(),
            view: ContentView::new([730, 360], area).unwrap(),
            interactive: true,
        });
        for bottom in [false, true] {
            let mut strip = Surface::new(730, 12, "transparent".into());
            strip.fill(
                R::new(12, if bottom { 10 } else { 0 }, 706, 2),
                "#abcdef",
                0,
            );
            strip.fill(R::new(0, if bottom { 9 } else { 0 }, 3, 3), "#345678", 0);
            style
                .mount(&mut scene, strip, 31, if bottom { 373 } else { 25 })
                .unwrap();
        }
        style
            .mount(&mut scene, style.floating_panel(160, 54), 320, 300)
            .unwrap();
        let mut renderer = Renderer::spawn().unwrap();
        renderer
            .scene_with_canvases(&scene, vec!["desktop".into()])
            .unwrap();
        let next = |renderer: &Renderer| loop {
            match renderer
                .output
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap()
            {
                RenderMessage::Frame {
                    pixels: Some(pixels),
                    ..
                } => break pixels,
                RenderMessage::Error { message } => panic!("{message}"),
                _ => {}
            }
        };
        let pixels = next(&renderer);
        assert_eq!(pixels.get_pixel(100, 25).0, [171, 205, 239, 255]);
        assert_eq!(
            pixels.get_pixel(31, 25).0,
            [52, 86, 120, 255],
            "corner cutout covers the square stream corner"
        );
        for (x, y) in [(100, 30), (100, 378), (320, 300), (400, 200)] {
            assert_eq!(
                pixels.get_pixel(x, y).0[3],
                0,
                "flattened workspace leaked into the stream at {x},{y}"
            );
        }
        assert_eq!(
            pixels.get_pixel(400, 325).0[3],
            255,
            "dock controls stay opaque"
        );
        assert_eq!(
            pixels.get_pixel(0, 0).0,
            [52, 86, 120, 255],
            "workspace outside the video stays unchanged"
        );
        renderer.scene(&scene).unwrap();
        let stopped = next(&renderer);
        assert_eq!(
            stopped.get_pixel(100, 30).0,
            [52, 86, 120, 255],
            "closing the stream restores the workspace"
        );
    }

    #[test]
    fn skin_overlay_preserves_background_and_checks_dimensions() {
        let base = RgbaImage::from_pixel(20, 20, crate::image::Rgba([30, 40, 50, 255]));
        let mut surface = super::super::surface::Surface::new(20, 20, "#ffffff".into());
        let mut renderer = SurfaceOverlayRenderer::new("Liberation Mono");
        assert_eq!(renderer.render(&base, &surface).unwrap(), base);
        surface.nodes.push(super::super::surface::Primitive::Fill {
            rect: crate::native_surface::PixelRect::new(5, 5, 5, 5),
            color: "#ff0000".into(),
            radius: 0,
        });
        let drawn = renderer.render(&base, &surface).unwrap();
        assert_eq!(drawn.get_pixel(0, 0), base.get_pixel(0, 0));
        assert_eq!(drawn.get_pixel(7, 7).0, [255, 0, 0, 255]);
        surface.width = 21;
        assert!(renderer.render(&base, &surface).is_err());
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
    fn queued_frames_retain_their_own_layout_even_at_the_same_scene_revision() {
        use super::super::protocol::{Component, Rect, Viewport};
        let viewport = Viewport {
            columns: 20,
            rows: 10,
            width: 80,
            height: 80,
            generation: 1,
        };
        let mut renderer = Renderer::spawn().unwrap();
        let mut outputs = Vec::new();
        for (x, color) in [(0, "#ff0000"), (10, "#0000ff")] {
            let mut scene = Scene::from_buffer(
                &crate::ratatui::buffer::Buffer::empty(crate::ratatui::layout::Rect::new(
                    0, 0, 20, 10,
                )),
                viewport,
                1,
            );
            scene.components.push(Component::Surface {
                rect: Rect {
                    x,
                    y: 0,
                    width: 10,
                    height: 10,
                },
                surface: crate::native_surface::Surface::new(40, 80, color.into()),
            });
            renderer.scene(&scene).unwrap();
            let output = renderer
                .output
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            let RenderMessage::Frame {
                pixels: Some(pixels),
                scene: rendered,
                ..
            } = output
            else {
                panic!("{output:?}")
            };
            assert_eq!(rendered.as_ref(), &scene);
            outputs.push((pixels, rendered));
        }
        // Local hover paints can share a controller revision. Frame/layout
        // identity must survive that case as well as asynchronous scene changes.
        let (old_pixels, old_scene) = &outputs[0];
        assert_eq!(old_scene.components[0].rect().x, 0);
        assert_eq!(old_pixels.get_pixel(20, 40).0, [255, 0, 0, 255]);
        assert_eq!(outputs[1].1.components[0].rect().x, 10);
        assert_eq!(outputs[1].0.get_pixel(60, 40).0, [0, 0, 255, 255]);
    }

    #[test]
    fn lost_terminal_regions_recover_while_an_unrelated_panel_keeps_animating() {
        let viewport = super::super::protocol::Viewport {
            columns: 80,
            rows: 20,
            width: 320,
            height: 160,
            generation: 1,
        };
        let mut pixels = RgbaImage::from_pixel(320, 160, crate::image::Rgba([20, 30, 40, 255]));
        let mut presenter = KittyPresenter::default();
        let mut images = BTreeMap::new();
        let mut wire = Vec::new();
        presenter
            .present_pixels(Arc::new(pixels.clone()), viewport, &mut wire)
            .unwrap();
        apply_wire(&wire, &mut images);
        let count = images.len();
        // Model a terminal discarding quiet placements, without changing the
        // application's scene or asking it to resend any assets.
        images.clear();
        let mut now = std::time::Instant::now();
        for step in 0..count {
            pixels.put_pixel(0, 0, crate::image::Rgba([step as u8, 200, 0, 255]));
            wire.clear();
            presenter
                .present_pixels(Arc::new(pixels.clone()), viewport, &mut wire)
                .unwrap();
            apply_wire(&wire, &mut images);
            wire.clear();
            now += std::time::Duration::from_secs(2);
            assert!(presenter.maintain_at(now, &mut wire).unwrap() > 0);
            let commands = String::from_utf8_lossy(&wire);
            assert!(
                (1..=8).contains(&commands.matches("a=T").count()),
                "Repair must stay bounded"
            );
            assert!(commands.find("a=T").unwrap() < commands.find("a=d").unwrap());
            apply_wire(&wire, &mut images);
            assert!(images.len() <= count);
        }
        let mut composed = RgbaImage::new(320, 160);
        for (x, y, image) in images.values() {
            crate::image::imageops::replace(&mut composed, image, i64::from(*x), i64::from(*y));
        }
        assert_eq!(composed, pixels);
        images.clear();
        wire.clear();
        presenter.repaint(&mut wire).unwrap();
        apply_wire(&wire, &mut images);
        assert_eq!(
            images.len(),
            count,
            "Focus return must restore the entire framebuffer"
        );
        wire.clear();
        presenter.clear(&mut wire).unwrap();
        apply_wire(&wire, &mut images);
        assert!(images.is_empty());
        wire.clear();
        assert_eq!(
            presenter
                .maintain_at(now + std::time::Duration::from_secs(10), &mut wire)
                .unwrap(),
            0
        );
        assert!(
            wire.is_empty(),
            "Retired desktops must never be resurrected"
        );
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
    fn coalesced_resize_replaces_placements_even_when_final_size_is_unchanged() {
        let viewport = super::super::protocol::Viewport {
            columns: 80,
            rows: 20,
            width: 320,
            height: 160,
            generation: 1,
        };
        let frame = RgbaImage::from_pixel(320, 160, crate::image::Rgba([20, 30, 40, 255]));
        let mut presenter = KittyPresenter::default();
        let mut images = BTreeMap::new();
        let mut wire = Vec::new();
        let png = encode_frame(&frame);
        presenter
            .present_regions(&png, viewport, &mut wire)
            .unwrap();
        apply_wire(&wire, &mut images);
        let old_ids: Vec<_> = images.keys().copied().collect();
        // A terminal can clip/reflow placements while intermediate layouts
        // are skipped. Equal final dimensions do not make those IDs reusable.
        images.clear();
        wire.clear();
        presenter
            .present_regions(
                &png,
                super::super::protocol::Viewport {
                    generation: 3,
                    ..viewport
                },
                &mut wire,
            )
            .unwrap();
        apply_wire(&wire, &mut images);
        assert_eq!(images.len(), old_ids.len());
        assert!(old_ids.iter().all(|id| !images.contains_key(id)));
        let mut composed = RgbaImage::new(320, 160);
        for (x, y, image) in images.values() {
            crate::image::imageops::replace(&mut composed, image, i64::from(*x), i64::from(*y));
        }
        assert_eq!(composed, frame);
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
