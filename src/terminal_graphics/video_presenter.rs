//! Video frames bypass the application rasterizer and PNG encoder.
use super::protocol::{Component, Rect, Scene, Viewport};
use crate::image::RgbaImage;
use base64::Engine as _;
use std::{io::Write, path::PathBuf, sync::Arc};

pub struct VideoPresenter {
    next: u32,
    local_files: bool,
    stalled_since: Option<std::time::Instant>,
    signature: Option<(Rect, Viewport, Arc<RgbaImage>)>,
    previous: Option<u32>,
    background: Option<(Rect, u32)>,
    files: std::collections::VecDeque<PathBuf>,
    root: PathBuf,
    pub frame: Option<(String, Arc<RgbaImage>)>,
}
impl VideoPresenter {
    fn fitted_rect(rect: Rect, pixels: &RgbaImage, viewport: Viewport) -> Rect {
        let cw = f64::from(viewport.width) / f64::from(viewport.columns);
        let ch = f64::from(viewport.height) / f64::from(viewport.rows);
        let factor = (f64::from(rect.width) * cw / f64::from(pixels.width()))
            .min(f64::from(rect.height) * ch / f64::from(pixels.height()));
        let width = (f64::from(pixels.width()) * factor / cw)
            .round()
            .clamp(1.0, f64::from(rect.width)) as u16;
        let height = (f64::from(pixels.height()) * factor / ch)
            .round()
            .clamp(1.0, f64::from(rect.height)) as u16;
        Rect {
            x: rect.x + (rect.width - width) / 2,
            y: rect.y + (rect.height - height) / 2,
            width,
            height,
        }
    }
    pub fn new() -> anyhow::Result<Self> {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!(
            "tty-graphics-protocol-star-video-{}",
            std::process::id()
        ));
        std::fs::create_dir(&root)?;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            next: 0x53560000,
            local_files: std::env::var_os("KITTY_PID").is_some()
                && std::env::var_os("STAR_VIDEO_DIRECT").is_none(),
            stalled_since: None,
            signature: None,
            previous: None,
            background: None,
            files: Default::default(),
            root,
            frame: None,
        })
    }
    pub fn image_rect(scene: &Scene, id: &str, terminal: Viewport) -> Option<Rect> {
        let rect = scene.components.iter().find_map(|c| match c {
            Component::Image { rect, id: key, .. } if key == id => Some(*rect),
            _ => None,
        })?;
        let (x, y, w, h) = if let Some(p) = scene
            .placements
            .iter()
            .find(|p| p.overlay.is_none() && super::placement::encloses(p.source, rect))
        {
            let x = f64::from(p.target.x)
                + f64::from(rect.x - p.source.x) * f64::from(p.target.width)
                    / f64::from(p.source.width);
            let y = f64::from(p.target.y) + f64::from(p.row_edge(rect.y - p.source.y));
            let w = f64::from(rect.width) * f64::from(p.target.width) / f64::from(p.source.width);
            let h = f64::from(
                p.row_edge(rect.y - p.source.y + rect.height) - p.row_edge(rect.y - p.source.y),
            );
            (x, y, w, h)
        } else {
            let v = scene.viewport;
            (
                f64::from(rect.x) * f64::from(v.width) / f64::from(v.columns),
                f64::from(rect.y) * f64::from(v.height) / f64::from(v.rows),
                f64::from(rect.width) * f64::from(v.width) / f64::from(v.columns),
                f64::from(rect.height) * f64::from(v.height) / f64::from(v.rows),
            )
        };
        let cw = f64::from(terminal.width) / f64::from(terminal.columns);
        let ch = f64::from(terminal.height) / f64::from(terminal.rows);
        Some(Rect {
            x: (x / cw).ceil() as u16,
            y: (y / ch).ceil() as u16,
            width: (w / cw).floor() as u16,
            height: (h / ch).floor() as u16,
        })
    }
    pub fn mask(
        &self,
        scene: &Scene,
        pixels: Arc<RgbaImage>,
        terminal: Viewport,
    ) -> Arc<RgbaImage> {
        let Some((id, video)) = &self.frame else {
            return pixels;
        };
        let Some(rect) = Self::image_rect(scene, id, terminal) else {
            return pixels;
        };
        let mut output = (*pixels).clone();
        let v = terminal;
        let x = u32::from(rect.x) * v.width / u32::from(v.columns);
        let y = u32::from(rect.y) * v.height / u32::from(v.rows);
        let endx =
            (u32::from(rect.x + rect.width) * v.width / u32::from(v.columns)).min(output.width());
        let endy =
            (u32::from(rect.y + rect.height) * v.height / u32::from(v.rows)).min(output.height());
        for row in y..endy {
            for column in x..endx {
                output.put_pixel(column, row, crate::image::Rgba([0; 4]));
            }
        }
        // Clip the live layer with small chrome overlays. Source frames stay
        // untouched, avoiding a full-resolution copy on every decoded frame.
        if rect.width > 0 && rect.height > 0 {
            let fitted = Self::fitted_rect(rect, video, terminal);
            let left = u32::from(fitted.x) * v.width / u32::from(v.columns);
            let top = u32::from(fitted.y) * v.height / u32::from(v.rows);
            let right = (u32::from(fitted.x + fitted.width) * v.width / u32::from(v.columns))
                .min(output.width());
            let bottom = (u32::from(fitted.y + fitted.height) * v.height / u32::from(v.rows))
                .min(output.height());
            let radius = 12u32
                .min(right.saturating_sub(left) / 2)
                .min(bottom.saturating_sub(top) / 2);
            for dy in 0..radius {
                for dx in 0..radius {
                    let distance = ((radius as f32 - dx as f32 - 0.5).powi(2)
                        + (radius as f32 - dy as f32 - 0.5).powi(2))
                    .sqrt();
                    let alpha =
                        ((distance - radius as f32 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8;
                    if alpha == 0 {
                        continue;
                    }
                    for (column, row) in [
                        (left + dx, top + dy),
                        (right - 1 - dx, top + dy),
                        (left + dx, bottom - 1 - dy),
                        (right - 1 - dx, bottom - 1 - dy),
                    ] {
                        output.put_pixel(column, row, crate::image::Rgba([0, 0, 0, alpha]));
                    }
                }
            }
        }
        // Restore separately placed popups over the video without stopping it.
        for placement in scene.placements.iter().filter(|p| p.overlay.is_some()) {
            let r = placement.target;
            let sx = u32::from(r.x).min(output.width());
            let ex = (u32::from(r.x) + u32::from(r.width)).min(output.width());
            let sy = u32::from(r.y).min(output.height());
            let ey = (u32::from(r.y) + u32::from(r.height)).min(output.height());
            for row in sy..ey {
                let start = ((row * output.width() + sx) * 4) as usize;
                let end = ((row * output.width() + ex) * 4) as usize;
                output.as_mut()[start..end].copy_from_slice(&pixels.as_raw()[start..end]);
            }
        }
        // Preserve native surfaces and modal pixels above the live image.
        for component in &scene.components {
            if !matches!(
                component,
                Component::Surface { .. }
                    | Component::Menu { .. }
                    | Component::Dialog { .. }
                    | Component::TextField { .. }
            ) {
                continue;
            }
            let r = component.rect();
            // Fullscreen scenes use the terminal coordinate space directly.
            if !scene.placements.is_empty() {
                continue;
            }
            let sx = u32::from(r.x) * v.width / u32::from(v.columns);
            let sy = u32::from(r.y) * v.height / u32::from(v.rows);
            let ex =
                (u32::from(r.x + r.width) * v.width / u32::from(v.columns)).min(output.width());
            let ey =
                (u32::from(r.y + r.height) * v.height / u32::from(v.rows)).min(output.height());
            for row in sy..ey {
                let start = ((row * output.width() + sx) * 4) as usize;
                let end = ((row * output.width() + ex) * 4) as usize;
                output.as_mut()[start..end].copy_from_slice(&pixels.as_raw()[start..end]);
            }
        }
        Arc::new(output)
    }
    pub fn present(
        &mut self,
        scene: &Scene,
        viewport: Viewport,
        out: &mut impl Write,
    ) -> anyhow::Result<()> {
        let Some((id, pixels)) = &self.frame else {
            return Ok(());
        };
        let Some(rect) =
            Self::image_rect(scene, id, viewport).filter(|r| r.width > 0 && r.height > 0)
        else {
            return self.clear(out);
        };
        let pixels = pixels.clone();
        if self
            .signature
            .as_ref()
            .is_some_and(|(old_rect, old_viewport, old_pixels)| {
                *old_rect == rect && *old_viewport == viewport && Arc::ptr_eq(old_pixels, &pixels)
            })
        {
            return Ok(());
        }
        let start = std::time::Instant::now();
        // Bound pending files even if Kitty stops consuming graphics commands.
        self.files.retain(|path| path.exists());
        if self.files.len() >= 3 {
            let waiting = self
                .stalled_since
                .get_or_insert_with(std::time::Instant::now);
            if waiting.elapsed() < std::time::Duration::from_secs(2) {
                return Ok(());
            }
            tracing::warn!("Kitty did not consume video files; switching to direct transfer");
            self.local_files = false;
            for path in self.files.drain(..) {
                let _ = std::fs::remove_file(path);
            }
        } else {
            self.stalled_since = None;
        }
        if self.background.as_ref().is_none_or(|(old, _)| *old != rect) {
            if let Some((_, id)) = self.background.take() {
                write!(out, "\x1b_Ga=d,d=I,i={id},q=2;\x1b\\")?
            }
            self.next = self.next.wrapping_add(1);
            let id = self.next;
            write!(
                out,
                "\x1b[{};{}H\x1b_Ga=T,f=32,s=1,v=1,i={id},p=1,C=1,z=0,q=2,c={},r={};AAAA/w==\x1b\\",
                rect.y + 1,
                rect.x + 1,
                rect.width,
                rect.height
            )?;
            self.background = Some((rect, id));
        }
        let fitted = Self::fitted_rect(rect, &pixels, viewport);
        let cols = fitted.width;
        let rows = fitted.height;
        self.next = self.next.wrapping_add(1);
        let id = self.next;
        write!(out, "\x1b[?2026h\x1b[{};{}H", fitted.y + 1, fitted.x + 1)?;
        // A terminal-owned local frontend shares Kitty's filesystem, including
        // SSH playback. Kitty unlinks t=t payloads after reading them.
        if self.local_files {
            let path = self.root.join(format!("tty-graphics-protocol-{id}"));
            std::fs::write(&path, pixels.as_raw())?;
            let payload = base64::engine::general_purpose::STANDARD
                .encode(path.as_os_str().as_encoded_bytes());
            write!(out,"\x1b_Ga=T,f=32,t=t,s={},v={},i={id},p=1,C=1,z=1,q=2,c={cols},r={rows};{payload}\x1b\\",pixels.width(),pixels.height())?;
            self.files.push_back(path);
        } else {
            let data = base64::engine::general_purpose::STANDARD.encode(pixels.as_raw());
            let chunks = data.as_bytes().chunks(4096);
            let count = chunks.len();
            for (part, chunk) in chunks.enumerate() {
                let more = usize::from(part + 1 < count);
                if part == 0 {
                    write!(out,"\x1b_Ga=T,f=32,t=d,s={},v={},i={id},p=1,C=1,z=1,q=2,c={cols},r={rows},m={more};",pixels.width(),pixels.height())?
                } else {
                    write!(out, "\x1b_Gm={more};")?
                }
                out.write_all(chunk)?;
                out.write_all(b"\x1b\\")?;
            }
        }
        if let Some(old) = self.previous.replace(id) {
            write!(out, "\x1b_Ga=d,d=I,i={old},q=2;\x1b\\")?
        }
        out.write_all(b"\x1b[?2026l")?;
        out.flush()?;
        self.signature = Some((rect, viewport, pixels.clone()));
        tracing::trace!(
            present_us = start.elapsed().as_micros(),
            width = pixels.width(),
            height = pixels.height(),
            "Video presented"
        );
        Ok(())
    }
    pub fn clear(&mut self, out: &mut impl Write) -> anyhow::Result<()> {
        self.signature = None;
        if let Some(id) = self.previous.take() {
            write!(out, "\x1b_Ga=d,d=I,i={id},q=2;\x1b\\")?
        }
        if let Some((_, id)) = self.background.take() {
            write!(out, "\x1b_Ga=d,d=I,i={id},q=2;\x1b\\")?
        }
        out.flush()?;
        Ok(())
    }
}
impl Drop for VideoPresenter {
    fn drop(&mut self) {
        let _ = self.clear(&mut std::io::stdout().lock());
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn video_layer_keeps_source_pixels_and_masks_only_the_video_area() {
        let mut scene = Scene::from_buffer(
            &crate::ratatui::buffer::Buffer::empty(crate::ratatui::layout::Rect::new(0, 0, 80, 24)),
            Viewport {
                columns: 80,
                rows: 24,
                width: 800,
                height: 480,
                generation: 1,
            },
            1,
        );
        let rect = Rect {
            x: 10,
            y: 5,
            width: 60,
            height: 15,
        };
        scene.components.push(Component::Image {
            rect,
            id: "movie".into(),
            png: None,
            scale: Default::default(),
            zoom: 100,
        });
        let mut presenter = VideoPresenter::new().unwrap();
        presenter.frame = Some(("movie".into(), Arc::new(RgbaImage::new(1920, 1080))));
        let frame = Arc::new(RgbaImage::from_pixel(
            800,
            480,
            crate::image::Rgba([20, 30, 40, 255]),
        ));
        let masked = presenter.mask(&scene, frame, scene.viewport);
        let fitted =
            VideoPresenter::fitted_rect(rect, &presenter.frame.as_ref().unwrap().1, scene.viewport);
        let left = u32::from(fitted.x) * 10;
        let top = u32::from(fitted.y) * 20;
        assert_eq!(masked.get_pixel(left, top).0, [0, 0, 0, 255]);
        assert_eq!(masked.get_pixel(left + 11, top + 11).0, [0; 4]);
        assert_eq!(masked.get_pixel(150, 150).0, [0; 4]);
        assert_eq!(masked.get_pixel(20, 20).0, [20, 30, 40, 255]);
        let mut wire = vec![];
        presenter
            .present(&scene, scene.viewport, &mut wire)
            .unwrap();
        let text = String::from_utf8(wire).unwrap();
        assert!(text.contains("f=32"));
        assert!(text.contains("s=1920,v=1080"));
        assert!(!text.contains("f=100"));
        let mut duplicate = vec![];
        presenter
            .present(&scene, scene.viewport, &mut duplicate)
            .unwrap();
        assert!(
            duplicate.is_empty(),
            "Chrome repaint uploaded the same video frame twice"
        );
        presenter.clear(&mut duplicate).unwrap();
        duplicate.clear();
        presenter.local_files = true;
        for i in 0..3 {
            let path = presenter.root.join(format!("pending-{i}"));
            std::fs::write(&path, []).unwrap();
            presenter.files.push_back(path);
        }
        presenter
            .present(&scene, scene.viewport, &mut duplicate)
            .unwrap();
        assert!(
            duplicate.is_empty(),
            "Pending frame bound was exceeded during resize"
        );
        assert!(
            presenter.local_files,
            "A short resize delay triggered direct transfer"
        );
        presenter.stalled_since =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(3));
        presenter
            .present(&scene, scene.viewport, &mut duplicate)
            .unwrap();
        assert!(
            !presenter.local_files,
            "An unsupported file transport did not fall back"
        );
    }
}
