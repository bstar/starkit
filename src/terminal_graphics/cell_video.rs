//! Bounded video presentation through the probed terminal image protocol or half blocks.
use super::protocol::{Component, Scene};
use crate::{
    graphics::{Graphics, ImageId},
    image::RgbaImage,
    ratatui::{
        backend::{Backend, CrosstermBackend},
        buffer::Buffer,
        layout::Rect,
        widgets::Widget,
    },
    ratatui_image::Image,
};
use std::{
    io::Write,
    sync::Arc,
    time::{Duration, Instant},
};

pub(crate) struct CellVideo {
    graphics: Graphics,
    last: Instant,
    signature: Option<(u64, Rect, Arc<RgbaImage>)>,
    asset: Option<(String, String, Arc<RgbaImage>)>,
}
impl CellVideo {
    pub fn new(mut graphics: Graphics) -> Self {
        graphics.set_capacity(1);
        Self {
            graphics,
            last: Instant::now() - Duration::from_secs(1),
            signature: None,
            asset: None,
        }
    }
    pub fn reset(&mut self) {
        self.signature = None;
        self.asset = None;
        self.graphics.forget_all();
    }
    pub fn assets(
        &mut self,
        scene: &Scene,
        live: Option<&str>,
        out: &mut impl Write,
    ) -> anyhow::Result<()> {
        for component in &scene.components {
            let Component::Image {
                id, png: Some(png), ..
            } = component
            else {
                continue;
            };
            if live == Some(id.as_str()) {
                continue;
            }
            if self
                .asset
                .as_ref()
                .is_none_or(|(key, data, _)| key != id || data != png)
            {
                use base64::Engine as _;
                let bytes = base64::engine::general_purpose::STANDARD.decode(png)?;
                let image = Arc::new(crate::graphics::decode_limited(&bytes, 4096)?.to_rgba8());
                self.asset = Some((id.clone(), png.clone(), image));
            }
            let image = Arc::clone(&self.asset.as_ref().unwrap().2);
            self.present(scene, id, &image, true, out)?;
        }
        Ok(())
    }
    /// Remove placeholder cells from the old poster before a live Kitty layer
    /// takes ownership. Cell diffing cannot see images drawn outside its buffer.
    pub fn clear_live(
        &mut self,
        scene: &Scene,
        id: &str,
        force: bool,
        out: &mut impl Write,
    ) -> anyhow::Result<()> {
        let old_poster = self.asset.as_ref().is_some_and(|(key, _, _)| key == id);
        if !force && !old_poster {
            return Ok(());
        }
        let Some(rect) = scene
            .components
            .iter()
            .find_map(|component| match component {
                Component::Image { rect, id: key, .. } if key == id => {
                    Some(Rect::new(rect.x, rect.y, rect.width, rect.height))
                }
                _ => None,
            })
        else {
            return Ok(());
        };
        let rect = rect.intersection(Rect::new(0, 0, scene.viewport.columns, scene.viewport.rows));
        if rect.is_empty()
            || scene.components.iter().any(|c| match c {
                Component::Menu { rect: r } | Component::Dialog { rect: r, .. } => {
                    rect.intersects(Rect::new(r.x, r.y, r.width, r.height))
                }
                _ => false,
            })
        {
            return Ok(());
        }
        if old_poster {
            self.asset = None;
            self.graphics.forget_all();
        }
        let [r, g, b] = super::native::rgb(&scene.background);
        let frame = Buffer::filled(
            rect,
            crate::ratatui::buffer::Cell::default()
                .set_bg(crate::ratatui::style::Color::Rgb(r, g, b))
                .clone(),
        );
        let mut backend = CrosstermBackend::new(&mut *out);
        backend.draw(frame.content.iter().enumerate().map(|(i, cell)| {
            (
                rect.x + (i % usize::from(rect.width)) as u16,
                rect.y + (i / usize::from(rect.width)) as u16,
                cell,
            )
        }))?;
        Backend::flush(&mut backend)?;
        Ok(())
    }
    pub fn present(
        &mut self,
        scene: &Scene,
        id: &str,
        pixels: &Arc<RgbaImage>,
        force: bool,
        out: &mut impl Write,
    ) -> anyhow::Result<()> {
        let Some(rect) = scene.components.iter().find_map(|c| match c {
            Component::Image { rect, id: key, .. } if key == id => {
                Some(Rect::new(rect.x, rect.y, rect.width, rect.height))
            }
            _ => None,
        }) else {
            return Ok(());
        };
        let rect = rect.intersection(Rect::new(0, 0, scene.viewport.columns, scene.viewport.rows));
        if rect.is_empty()
            || scene.components.iter().any(|c| match c {
                Component::Menu { rect: r } | Component::Dialog { rect: r, .. } => {
                    rect.intersects(Rect::new(r.x, r.y, r.width, r.height))
                }
                _ => false,
            })
        {
            return Ok(());
        }
        if !force
            && (self.last.elapsed() < Duration::from_millis(50)
                || self.signature.as_ref().is_some_and(|(r, a, p)| {
                    *r == scene.revision && *a == rect && Arc::ptr_eq(p, pixels)
                }))
        {
            return Ok(());
        }
        let [r, g, b] = super::native::rgb(&scene.background);
        let mut frame = Buffer::filled(
            rect,
            crate::ratatui::buffer::Cell::default()
                .set_bg(crate::ratatui::style::Color::Rgb(r, g, b))
                .clone(),
        );
        // Only the latest decoded frame and its terminal encoding are retained.
        if self
            .signature
            .as_ref()
            .is_none_or(|(_, area, old)| *area != rect || !Arc::ptr_eq(old, pixels))
        {
            self.graphics.forget_all();
        }
        if let Some(protocol) = self.graphics.protocol(ImageId(0x53564345), pixels, rect) {
            Image::new(protocol).render(rect, &mut frame);
        } else {
            let fitted = super::video_presenter::VideoPresenter::fit_cells(
                rect.into(),
                pixels,
                scene.viewport,
            );
            crate::graphics::halfblocks(
                pixels,
                Rect::new(fitted.x, fitted.y, fitted.width, fitted.height),
                &mut frame,
            );
        }
        let mut backend = CrosstermBackend::new(&mut *out);
        backend.draw(
            frame
                .content
                .iter()
                .enumerate()
                .filter(|(_, c)| c.diff_option != crate::ratatui::buffer::CellDiffOption::Skip)
                .map(|(i, c)| {
                    (
                        rect.x + (i % usize::from(rect.width)) as u16,
                        rect.y + (i / usize::from(rect.width)) as u16,
                        c,
                    )
                }),
        )?;
        Backend::flush(&mut backend)?;
        self.signature = Some((scene.revision, rect, Arc::clone(pixels)));
        self.last = Instant::now();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::protocol::{Rect as WireRect, Viewport};
    use super::*;
    use crate::ratatui::buffer::Buffer;
    #[test]
    fn latest_frames_are_bounded_and_do_not_cover_dialogs() {
        crate::crossterm::style::force_color_output(true);
        let mut video = CellVideo::new(Graphics::disabled());
        let mut scene = Scene::from_buffer(
            &Buffer::empty(Rect::new(0, 0, 20, 10)),
            Viewport {
                columns: 20,
                rows: 10,
                ..Viewport::default()
            },
            1,
        );
        scene.components.push(Component::Image {
            rect: WireRect {
                x: 2,
                y: 2,
                width: 10,
                height: 5,
            },
            id: "video-test".into(),
            png: None,
            scale: Default::default(),
            zoom: 100,
        });
        let image = Arc::new(RgbaImage::from_pixel(
            20,
            20,
            crate::image::Rgba([120, 60, 30, 255]),
        ));
        let mut out = Vec::new();
        video
            .present(&scene, "video-test", &image, true, &mut out)
            .unwrap();
        assert!(!out.is_empty());
        out.clear();
        video
            .present(&scene, "video-test", &image, false, &mut out)
            .unwrap();
        assert!(out.is_empty());
        scene.components.push(Component::Menu {
            rect: WireRect {
                x: 3,
                y: 3,
                width: 5,
                height: 3,
            },
        });
        video
            .present(&scene, "video-test", &image, true, &mut out)
            .unwrap();
        assert!(out.is_empty());
        video.reset();
        assert!(video.signature.is_none());
        scene
            .components
            .retain(|c| !matches!(c, Component::Menu { .. }));
        scene.background = "#20212a".into();
        if let Component::Image { png, .. } = &mut scene.components[0] {
            *png = Some("invalid poster must not be decoded".into());
        }
        video.asset = Some(("video-test".into(), "old poster".into(), image));
        video.assets(&scene, Some("video-test"), &mut out).unwrap();
        assert!(out.is_empty());
        video
            .clear_live(&scene, "video-test", false, &mut out)
            .unwrap();
        assert!(video.asset.is_none());
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(
            text.contains("48;2;32;33;42"),
            "Poster clearing must use the preview background: {text:?}"
        );
        out.clear();
        video
            .clear_live(&scene, "video-test", false, &mut out)
            .unwrap();
        assert!(
            out.is_empty(),
            "Unchanged live frames must not clear the terminal on every frame"
        );
    }
}
