//! Browser lifecycle and Kitty image presentation. No application commands here.
use std::fs;
use std::io::{self, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use crate::image::{ImageDecoder as _, ImageEncoder as _, RgbaImage};
use anyhow::{bail, Context, Result};
use base64::Engine as _;
use crossbeam_channel::{bounded, Receiver};
use serde::Deserialize;

use super::protocol::{read_message, write_message, Scene};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RenderMessage {
    Ready,
    Frame {
        revision: u64,
        generation: u64,
        width: u32,
        height: u32,
        png: String,
    },
    Error {
        message: String,
    },
}

pub struct Renderer {
    child: Child,
    input: Option<ChildStdin>,
    pub output: Receiver<RenderMessage>,
    directory: PathBuf,
}
impl Renderer {
    pub fn spawn() -> Result<Self> {
        let root = std::env::temp_dir();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let directory = root.join(format!("starkit-graphics-{}-{stamp}", std::process::id()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&directory)?;
        }
        #[cfg(not(unix))]
        fs::create_dir(&directory)?;
        fs::write(directory.join("main.cjs"), super::RUNTIME_MAIN)?;
        fs::write(
            directory.join("capture.cjs"),
            include_str!("../../runtime/terminal-graphics/capture.cjs"),
        )?;
        fs::write(directory.join("index.html"), super::RUNTIME_HTML)?;
        fs::write(directory.join("preload.cjs"), super::RUNTIME_PRELOAD)?;
        let executable =
            std::env::var_os("STAR_GRAPHICS_ELECTRON").unwrap_or_else(|| "electron".into());
        let log = fs::File::create(directory.join("renderer.log"))?;
        let mut command = Command::new(executable);
        #[cfg(target_os = "linux")]
        if let Ok(platform) = std::env::var("STAR_GRAPHICS_PLATFORM") {
            if matches!(platform.as_str(), "x11" | "wayland") {
                command.arg(format!("--ozone-platform={platform}"));
            }
        }
        let child = command
            .arg(directory.join("main.cjs"))
            .env_remove("ELECTRON_RUN_AS_NODE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(log)
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(e) => {
                let _ = fs::remove_dir_all(&directory);
                return Err(e)
                    .context("Install the graphical runtime or set STAR_GRAPHICS_ELECTRON");
            }
        };
        let input = child.stdin.take();
        let stdout = child.stdout.take().context("renderer stdout unavailable")?;
        let (tx, rx) = bounded(2);
        let drop_old = rx.clone();
        std::thread::Builder::new()
            .name("star-graphics-frames".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    let message = match read_message::<RenderMessage>(&mut reader) {
                        Ok(Some(m)) => m,
                        Ok(None) => break,
                        Err(error) => RenderMessage::Error {
                            message: format!("Invalid renderer response: {error}. For npm Electron, download the runtime with install-electron before launching."),
                        },
                    };
                    let error = matches!(message, RenderMessage::Error { .. });
                    if let Err(crossbeam_channel::TrySendError::Full(message)) =
                        tx.try_send(message)
                    {
                        let _ = drop_old.try_recv();
                        let _ = tx.try_send(message);
                    }
                    if error {
                        break;
                    }
                }
            })?;
        let mut renderer = Self {
            child,
            input,
            output: rx,
            directory,
        };
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match renderer.output.recv_timeout(Duration::from_millis(100)) {
                Ok(RenderMessage::Ready) => return Ok(renderer),
                Ok(RenderMessage::Error { message }) => bail!("Graphical renderer: {message}"),
                _ => {}
            }
            if let Some(status) = renderer.child.try_wait()? {
                let log =
                    fs::read_to_string(renderer.directory.join("renderer.log")).unwrap_or_default();
                bail!(
                    "Graphical renderer exited ({status}): {}",
                    log.chars().take(2000).collect::<String>()
                );
            }
            if Instant::now() > deadline {
                bail!("Graphical renderer did not start within 20 seconds");
            }
        }
    }
    pub fn scene(&mut self, scene: &Scene) -> Result<()> {
        write_message(
            &serde_json::json!({"type":"scene","scene":scene}),
            self.input.as_mut().context("renderer closed")?,
        )?;
        Ok(())
    }
    pub fn clipboard(&mut self, text: &str) -> Result<()> {
        write_message(
            &serde_json::json!({"type":"clipboard","text":text}),
            self.input.as_mut().context("renderer closed")?,
        )?;
        Ok(())
    }
    pub fn alive(&mut self) -> Result<bool> {
        Ok(self.child.try_wait()?.is_none())
    }
}
impl Drop for Renderer {
    fn drop(&mut self) {
        self.input.take();
        let deadline = Instant::now() + Duration::from_millis(500);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.directory);
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
    pixels: RgbaImage,
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
                    "\x1b_Ga=T,f=100,t=d,i={id},p=1,q=2,C=1,c={columns},r={rows},m={more};"
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
            return Ok(self.present(png, viewport.columns, viewport.rows, out)?);
        }
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
            let crop = crate::image::imageops::crop_imm(&pixels, x, y, width, height).to_image();
            let mut encoded = Vec::new();
            crate::image::codecs::png::PngEncoder::new_with_quality(
                &mut encoded,
                crate::image::codecs::png::CompressionType::Default,
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
                return Ok(self.present(png, viewport.columns, viewport.rows, out)?);
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
                            "\x1b_Ga=T,f=100,t=d,i={id},p=1,q=2,C=1,c={},r={},m={more};",
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
        self.cached = Some((png.to_owned(), viewport.columns, viewport.rows));
        self.regions = Some(RegionCache {
            pixels,
            columns: viewport.columns,
            rows: viewport.rows,
            placements,
        });
        Ok(payload_bytes)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

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
