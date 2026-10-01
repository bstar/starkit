//! Optional visual primitives shared by terminal decoration and desktop UI.
//! Files, operations and application commands never enter this module.
use crate::theme::{color::Rgb, Theme};
use std::{collections::VecDeque, sync::Arc, time::Duration};

pub const CACHE_LIMIT: usize = 64 * 1024 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Folder,
    File,
    Link,
    Image,
    Audio,
    Video,
    Archive,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tokens {
    pub background: Rgb,
    pub surface: Rgb,
    pub foreground: Rgb,
    pub muted: Rgb,
    pub accent: Rgb,
    pub border: Rgb,
    pub selected: Rgb,
    pub selected_text: Rgb,
}
impl Tokens {
    pub fn from_theme(theme: &Theme) -> Self {
        Self {
            background: theme.bg,
            surface: theme.panel_bg,
            foreground: theme.fg,
            muted: theme.dim,
            accent: theme.accent,
            border: theme.border,
            selected: theme.row_selected_bg,
            selected_text: theme.row_selected_fg,
        }
    }
}
/// Capabilities are deliberately independent: displaying an image does not
/// imply that it can safely be placed behind terminal text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub images: bool,
    pub persistent_placements: bool,
    pub layering: bool,
    pub cell_pixels: Option<(u16, u16)>,
    pub synchronized_updates: bool,
}
impl Capabilities {
    pub fn decorations(self) -> bool {
        self.images && self.cell_pixels.is_some()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceKey {
    pub icon: Icon,
    pub width: u16,
    pub height: u16,
    pub foreground: Rgb,
    pub background: Rgb,
}
#[derive(Debug)]
pub struct SurfaceCache {
    entries: VecDeque<(SurfaceKey, Arc<image::DynamicImage>)>,
    bytes: usize,
    limit: usize,
}
impl Default for SurfaceCache {
    fn default() -> Self {
        Self::new(CACHE_LIMIT)
    }
}
impl SurfaceCache {
    pub fn new(limit: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
            limit,
        }
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
    pub fn icon(&mut self, key: SurfaceKey) -> Option<Arc<image::DynamicImage>> {
        if let Some(index) = self.entries.iter().position(|(k, _)| *k == key) {
            let pair = self.entries.remove(index)?;
            let value = pair.1.clone();
            self.entries.push_back(pair);
            return Some(value);
        }
        let count = usize::from(key.width)
            .checked_mul(usize::from(key.height))?
            .checked_mul(4)?;
        if count > self.limit
            || key.width == 0
            || key.height == 0
            || key.width > 512
            || key.height > 512
        {
            return None;
        }
        let image = Arc::new(raster_icon(key)?);
        while self.bytes + count > self.limit {
            let (_, old) = self.entries.pop_front()?;
            self.bytes -= old.width() as usize * old.height() as usize * 4;
        }
        self.bytes += count;
        self.entries.push_back((key, image.clone()));
        Some(image)
    }
}
fn colour(rgb: Rgb) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(rgb.r, rgb.g, rgb.b, 255)
}
fn raster_icon(key: SurfaceKey) -> Option<image::DynamicImage> {
    use tiny_skia::{Paint, PathBuilder, Pixmap, Stroke, Transform};
    let mut pixmap = Pixmap::new(key.width.into(), key.height.into())?;
    pixmap.fill(colour(key.background));
    let scale = Transform::from_scale(key.width as f32 / 24., key.height as f32 / 24.);
    let mut paint = Paint::default();
    paint.set_color(colour(key.foreground));
    paint.anti_alias = true;
    let mut path = PathBuilder::new();
    match key.icon {
        Icon::Folder => {
            path.move_to(3., 7.);
            path.line_to(3., 5.);
            path.line_to(9., 5.);
            path.line_to(12., 8.);
            path.line_to(21., 8.);
            path.line_to(21., 20.);
            path.line_to(3., 20.);
            path.close();
        }
        _ => {
            path.move_to(6., 3.);
            path.line_to(15., 3.);
            path.line_to(20., 8.);
            path.line_to(20., 21.);
            path.line_to(6., 21.);
            path.close();
        }
    }
    pixmap.stroke_path(
        &path.finish()?,
        &paint,
        &Stroke {
            width: 1.6,
            ..Default::default()
        },
        scale,
        None,
    );
    let mut detail = PathBuilder::new();
    match key.icon {
        Icon::Audio => {
            detail.move_to(14., 8.);
            detail.line_to(14., 16.);
            detail.line_to(10., 17.);
            detail.move_to(14., 9.);
            detail.line_to(18., 8.);
        }
        Icon::Image => {
            detail.move_to(8., 17.);
            detail.line_to(12., 12.);
            detail.line_to(16., 17.);
            detail.line_to(18., 15.);
        }
        Icon::Video => {
            detail.move_to(10., 10.);
            detail.line_to(16., 14.);
            detail.line_to(10., 18.);
            detail.close();
        }
        Icon::Archive => {
            detail.move_to(13., 6.);
            detail.line_to(13., 18.);
        }
        Icon::Link => {
            detail.move_to(8., 17.);
            detail.line_to(17., 9.);
            detail.move_to(12., 9.);
            detail.line_to(17., 9.);
            detail.line_to(17., 14.);
        }
        _ => {
            detail.move_to(8., 13.);
            detail.line_to(17., 13.);
            detail.move_to(8., 17.);
            detail.line_to(17., 17.);
        }
    }
    if let Some(path) = detail.finish() {
        pixmap.stroke_path(
            &path,
            &paint,
            &Stroke {
                width: 1.2,
                ..Default::default()
            },
            scale,
            None,
        );
    }
    // Pixmap pixels are premultiplied; the background is opaque, so RGBA is
    // already suitable for both image protocols and desktop previews.
    image::RgbaImage::from_raw(key.width.into(), key.height.into(), pixmap.take())
        .map(image::DynamicImage::ImageRgba8)
}
/// Bounded diagnostics; timings describe application rendering, not emulator
/// presentation latency or a monitor's refresh rate.
#[derive(Debug, Default)]
pub struct Metrics {
    samples: VecDeque<Duration>,
    pub frames: u64,
    pub surface_bytes: u64,
}
impl Metrics {
    pub fn frame(&mut self, time: Duration) {
        self.frames += 1;
        if self.samples.len() == 2048 {
            self.samples.pop_front();
        }
        self.samples.push_back(time);
    }
    pub fn p95_ms(&self) -> f64 {
        let mut samples: Vec<_> = self.samples.iter().copied().collect();
        samples.sort();
        samples
            .get(samples.len().saturating_mul(95) / 100)
            .map_or(0., |s| s.as_secs_f64() * 1000.)
    }
}
#[cfg(feature = "desktop")]
pub mod desktop {
    use super::*;
    use gpui::{div, px, rgb, InteractiveElement, IntoElement, ParentElement, Styled};
    pub fn rgb24(value: Rgb) -> gpui::Rgba {
        rgb((u32::from(value.r) << 16) | (u32::from(value.g) << 8) | u32::from(value.b))
    }
    pub fn card(tokens: Tokens) -> gpui::Div {
        div()
            .bg(rgb24(tokens.surface))
            .border_1()
            .border_color(rgb24(tokens.border))
            .rounded_lg()
            .p_3()
    }
    pub fn tab(label: impl Into<gpui::SharedString>, selected: bool, tokens: Tokens) -> gpui::Div {
        div()
            .px_4()
            .py_2()
            .rounded_md()
            .bg(rgb24(if selected {
                tokens.selected
            } else {
                tokens.surface
            }))
            .text_color(rgb24(if selected {
                tokens.selected_text
            } else {
                tokens.foreground
            }))
            .child(label.into())
    }
    pub fn meter(fraction: f32, tokens: Tokens) -> impl IntoElement {
        let value = if fraction.is_finite() {
            fraction.clamp(0., 1.)
        } else {
            0.
        };
        div()
            .h(px(4.))
            .w_full()
            .rounded_full()
            .overflow_hidden()
            .bg(rgb24(tokens.border))
            .child(
                div()
                    .h_full()
                    .w(gpui::relative(value))
                    .bg(rgb24(tokens.accent)),
            )
    }
    pub fn menu_item(label: impl Into<gpui::SharedString>, tokens: Tokens) -> gpui::Div {
        div()
            .px_3()
            .py_2()
            .rounded_md()
            .text_color(rgb24(tokens.foreground))
            .hover(|style| style.bg(rgb24(tokens.selected)))
            .child(label.into())
    }
}
/// One end of a filled terminal tab, rasterized only in its padding cells.
/// The caller must keep text outside this surface; no protocol layering is assumed.
pub fn rounded_edge(
    width: u16,
    height: u16,
    background: Rgb,
    surface: Rgb,
    left: bool,
) -> Option<image::RgbaImage> {
    use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};
    if width == 0 || height == 0 || width > 512 || height > 512 {
        return None;
    }
    let mut pixels = Pixmap::new(width.into(), height.into())?;
    pixels.fill(colour(background));
    let w = f32::from(width);
    let h = f32::from(height);
    let mut path = PathBuilder::new();
    // An ellipse meets a rectangular fill without crossing the text boundary.
    path.push_oval(Rect::from_xywh(if left { 0. } else { -w }, 0., w * 2., h)?);
    let mut paint = Paint::default();
    paint.set_color(colour(surface));
    paint.anti_alias = true;
    pixels.fill_path(
        &path.finish()?,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    image::RgbaImage::from_raw(width.into(), height.into(), pixels.take())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(size: u16) -> SurfaceKey {
        SurfaceKey {
            icon: Icon::Folder,
            width: size,
            height: size,
            foreground: Rgb::new(210, 210, 210),
            background: Rgb::new(20, 20, 20),
        }
    }
    #[test]
    fn cache_bounds_and_reuses() {
        let mut cache = SurfaceCache::new(4096);
        let first = cache.icon(key(16)).unwrap();
        assert!(Arc::ptr_eq(&first, &cache.icon(key(16)).unwrap()));
        assert!(cache.icon(key(33)).is_none());
        cache.icon(key(24)).unwrap();
        assert!(cache.bytes() <= 4096);
        cache.clear();
        assert_eq!(cache.bytes(), 0);
    }
    proptest::proptest! {
        #[test]
        fn foreign_dimensions_stay_bounded(width in proptest::prelude::any::<u16>(),height in proptest::prelude::any::<u16>()) {
            let mut cache=SurfaceCache::new(16_384);
            let key=SurfaceKey{width,height,..key(16)};
            let surface=cache.icon(key);
            proptest::prop_assert!(cache.bytes()<=16_384);
            if let Some(surface)=surface {proptest::prop_assert_eq!(surface.width(),u32::from(width));proptest::prop_assert_eq!(surface.height(),u32::from(height));}
        }
    }
    #[test]
    fn capability_requires_pixel_dimensions() {
        assert!(!Capabilities {
            images: true,
            ..Default::default()
        }
        .decorations());
    }
    #[test]
    fn metrics_are_bounded() {
        let mut metrics = Metrics::default();
        for _ in 0..10_000 {
            metrics.frame(Duration::from_millis(2));
        }
        assert_eq!(metrics.samples.len(), 2048);
        assert_eq!(metrics.p95_ms(), 2.);
    }
}
