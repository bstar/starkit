//! Native, display-server-independent drawing. Only bounded scene data enters.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, Result};
use base64::Engine as _;
use cosmic_text::{
    Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight, Wrap,
};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

use super::protocol::{Component, Rect, Scene};
use crate::image::{ImageDecoder as _, RgbaImage};

const FONTS: [&[u8]; 4] = [
    include_bytes!("../../assets/fonts/LiberationSans-Regular.ttf"),
    include_bytes!("../../assets/fonts/LiberationSans-Bold.ttf"),
    include_bytes!("../../assets/fonts/LiberationMono-Regular.ttf"),
    include_bytes!("../../assets/fonts/LiberationMono-Bold.ttf"),
];

#[derive(Clone, Hash, PartialEq, Eq)]
struct TextKey {
    text: String,
    size: u32,
    bold: bool,
    mono: bool,
}
#[derive(Clone, Copy)]
struct TextStyle<'a> {
    size: f32,
    color: &'a str,
    bold: bool,
    mono: bool,
    ellipsis: bool,
}

struct Asset {
    encoded: String,
    pixels: Arc<RgbaImage>,
    scaled: Option<((u32, u32), Pixmap)>,
}
pub(super) struct Painter {
    fonts: FontSystem,
    glyphs: SwashCache,
    text: HashMap<TextKey, Buffer>,
    text_bytes: usize,
    assets: HashMap<String, Asset>,
}

fn rgb(value: &str) -> [u8; 3] {
    let Some(hex) = value
        .strip_prefix('#')
        .filter(|s| s.len() == 6 && s.is_ascii())
    else {
        return [205, 214, 244];
    };
    [0, 2, 4].map(|start| u8::from_str_radix(&hex[start..start + 2], 16).unwrap_or(0))
}
fn paint(value: &str) -> Paint<'static> {
    let [r, g, b] = rgb(value);
    let mut p = Paint::default();
    p.set_color_rgba8(r, g, b, 255);
    p
}
fn fill(canvas: &mut Pixmap, rect: [f32; 4], color: &str) {
    if let Some(rect) = tiny_skia::Rect::from_xywh(rect[0], rect[1], rect[2], rect[3]) {
        let mut p = paint(color);
        p.anti_alias = false;
        canvas.fill_rect(rect, &p, Transform::identity(), None);
    }
}
fn rounded(canvas: &mut Pixmap, r: [f32; 4], radius: f32, color: &str, stroke: bool) {
    let [x, y, w, h] = r;
    if w <= 0. || h <= 0. {
        return;
    }
    let radius = radius.min(w / 2.).min(h / 2.);
    let mut path = PathBuilder::new();
    path.move_to(x + radius, y);
    path.line_to(x + w - radius, y);
    path.quad_to(x + w, y, x + w, y + radius);
    path.line_to(x + w, y + h - radius);
    path.quad_to(x + w, y + h, x + w - radius, y + h);
    path.line_to(x + radius, y + h);
    path.quad_to(x, y + h, x, y + h - radius);
    path.line_to(x, y + radius);
    path.quad_to(x, y, x + radius, y);
    path.close();
    if let Some(path) = path.finish() {
        if stroke {
            canvas.stroke_path(
                &path,
                &paint(color),
                &Stroke {
                    width: 2.,
                    ..Stroke::default()
                },
                Transform::identity(),
                None,
            );
        } else {
            canvas.fill_path(
                &path,
                &paint(color),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
}

impl Painter {
    pub fn new() -> Self {
        let mut db = cosmic_text::fontdb::Database::new();
        for bytes in FONTS {
            db.load_font_data(bytes.to_vec());
        }
        // Bundled fonts guarantee a consistent baseline; installed fonts supply
        // Unicode fallback on both Linux and macOS. No fontconfig C dependency.
        if std::env::var("STAR_GRAPHICS_SYSTEM_FONTS").as_deref() != Ok("0") {
            db.load_system_fonts();
        }
        db.set_sans_serif_family("Liberation Sans");
        db.set_monospace_family("Liberation Mono");
        Self {
            fonts: FontSystem::new_with_locale_and_db("en-US".into(), db),
            glyphs: SwashCache::new(),
            text: HashMap::new(),
            text_bytes: 0,
            assets: HashMap::new(),
        }
    }

    fn text(&mut self, canvas: &mut Pixmap, value: &str, rect: [f32; 4], style: TextStyle<'_>) {
        let TextStyle {
            size,
            color,
            bold,
            mono,
            ellipsis,
        } = style;
        let [x, y, w, h] = rect;
        if value.trim().is_empty() || w <= 0. || h <= 0. {
            return;
        }
        if self.text.len() >= 2048 || self.text_bytes > 1_000_000 {
            self.text.clear();
            self.text_bytes = 0;
        }
        if self.glyphs.image_cache.len() > 8192 {
            self.glyphs.image_cache.clear();
        }
        let key = TextKey {
            text: value
                .chars()
                .filter(|c| !c.is_control())
                .take(8192)
                .collect(),
            size: size.to_bits(),
            bold,
            mono,
        };
        let fonts = &mut self.fonts;
        let cache_bytes = &mut self.text_bytes;
        let buffer = self.text.entry(key.clone()).or_insert_with(|| {
            *cache_bytes += key.text.len();
            let mut b = Buffer::new(fonts, Metrics::new(size, size * 1.2));
            b.set_wrap(fonts, Wrap::None);
            b.set_text(
                fonts,
                &key.text,
                &Attrs::new()
                    .family(if mono {
                        Family::Monospace
                    } else {
                        Family::SansSerif
                    })
                    .weight(if bold { Weight::BOLD } else { Weight::NORMAL }),
                Shaping::Advanced,
            );
            b.shape_until_scroll(fonts, false);
            b
        });
        let text_width = buffer.layout_runs().map(|r| r.line_w).fold(0., f32::max);
        let clipped = ellipsis && text_width > w;
        let clip_w = if clipped { (w - size).max(0.) } else { w };
        let offset_y = (h - size * 1.2) / 2.;
        let [r, g, b] = rgb(color);
        let width = canvas.width();
        let height = canvas.height();
        let pixels = canvas.data_mut();
        buffer.draw(
            fonts,
            &mut self.glyphs,
            Color::rgb(r, g, b),
            |px, py, _, _, color| {
                let dx = x.round() as i32 + px;
                let dy = (y + offset_y).round() as i32 + py;
                if dx < x.ceil() as i32
                    || dy < y.ceil() as i32
                    || dx >= (x + clip_w).floor() as i32
                    || dy >= (y + h).floor() as i32
                    || dx < 0
                    || dy < 0
                    || dx >= width as i32
                    || dy >= height as i32
                {
                    return;
                }
                let offset = ((dy as u32 * width + dx as u32) * 4) as usize;
                let rgba = color.as_rgba();
                let alpha = u32::from(rgba[3]);
                for c in 0..3 {
                    pixels[offset + c] = ((u32::from(rgba[c]) * alpha
                        + u32::from(pixels[offset + c]) * (255 - alpha)
                        + 127)
                        / 255) as u8;
                }
            },
        );
        if clipped {
            self.text(
                canvas,
                "…",
                [x + w - size, y, size, h],
                TextStyle {
                    size,
                    color,
                    bold,
                    mono,
                    ellipsis: false,
                },
            );
        }
    }

    fn spans(
        &mut self,
        canvas: &mut Pixmap,
        scene: &Scene,
        cw: f32,
        ch: f32,
        font: f32,
        region: Option<Rect>,
    ) {
        for span in &scene.spans {
            if span.x >= scene.viewport.columns || span.y >= scene.viewport.rows {
                continue;
            }
            if region.is_some_and(|r| {
                span.y < r.y || u32::from(span.y) >= u32::from(r.y) + u32::from(r.height)
            }) {
                continue;
            }
            let cells = unicode_width::UnicodeWidthStr::width(span.text.as_str());
            let x = (f32::from(span.x) * cw).floor();
            let right = ((f32::from(span.x) + cells as f32) * cw).floor();
            let y = (f32::from(span.y) * ch).floor();
            let bottom = (f32::from(span.y.saturating_add(1)) * ch).floor();
            let mut rect = [x, y, right - x, bottom - y];
            if let Some(r) = region {
                let left = (f32::from(r.x) * cw).floor();
                let right = (f32::from(r.x.saturating_add(r.width)) * cw).floor();
                let l = rect[0].max(left);
                let end = (rect[0] + rect[2]).min(right);
                rect[0] = l;
                rect[2] = end - l;
            }
            fill(canvas, rect, &span.background);
            // Preserve cell positions and natural shaping within each word.
            let mut column = f32::from(span.x);
            let mut word = String::new();
            let mut start = column;
            for c in span.text.chars().chain(std::iter::once(' ')) {
                let width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0) as f32;
                if c.is_whitespace() || "╔╗╚╝║═┌┐└┘│─╭╮╰╯".contains(c)
                {
                    if !word.is_empty() {
                        let mut text_rect = [
                            (start * cw).floor(),
                            y,
                            ((column - start) * cw).ceil(),
                            bottom - y,
                        ];
                        if region.is_none()
                            || (text_rect[0] >= rect[0] && text_rect[0] < rect[0] + rect[2])
                        {
                            text_rect[2] = text_rect[2].min(rect[0] + rect[2] - text_rect[0]);
                            self.text(
                                canvas,
                                &word,
                                text_rect,
                                TextStyle {
                                    size: font,
                                    color: &span.foreground,
                                    bold: span.bold,
                                    mono: true,
                                    ellipsis: false,
                                },
                            );
                        }
                        word.clear();
                    }
                } else {
                    if word.is_empty() {
                        start = column;
                    }
                    word.push(c);
                }
                column += width;
            }
        }
    }

    pub fn render(&mut self, scene: &Scene) -> Result<RgbaImage> {
        let viewport = scene.viewport.validate()?;
        if self
            .glyphs
            .image_cache
            .values()
            .filter_map(Option::as_ref)
            .map(|image| image.data.len())
            .sum::<usize>()
            > 32_000_000
        {
            self.glyphs.image_cache.clear();
        }
        anyhow::ensure!(
            scene.spans.len() <= super::protocol::MAX_CELLS
                && scene.components.len() <= super::protocol::MAX_CELLS,
            "Scene exceeds limits"
        );
        let cw = viewport.width as f32 / f32::from(viewport.columns);
        let ch = viewport.height as f32 / f32::from(viewport.rows);
        // Match the terminal cell metrics; never expand text beyond its cell advance.
        let font = (ch * 0.84).min(cw / 0.6).floor().clamp(1., 64.);
        let mut canvas =
            Pixmap::new(viewport.width, viewport.height).context("Allocate graphical frame")?;
        let [r, g, b] = rgb(&scene.background);
        canvas.fill(tiny_skia::Color::from_rgba8(r, g, b, 255));
        self.spans(&mut canvas, scene, cw, ch, font, None);
        let active_images: HashSet<_> = scene
            .components
            .iter()
            .filter_map(|c| {
                if let Component::Image { id, .. } = c {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect();
        anyhow::ensure!(active_images.len() <= 8, "Too many preview assets");
        self.assets.retain(|id, _| active_images.contains(id));
        let mut used = HashSet::new();
        let accent = if scene.accent.is_empty() {
            &scene.foreground
        } else {
            &scene.accent
        };
        let border = if scene.border.is_empty() {
            &scene.foreground
        } else {
            &scene.border
        };
        for component in &scene.components {
            let rect = match component {
                Component::Panel { rect, .. }
                | Component::Menu { rect }
                | Component::Dialog { rect, .. }
                | Component::TextField { rect, .. }
                | Component::ListRow { rect, .. }
                | Component::Tab { rect, .. }
                | Component::Meter { rect, .. }
                | Component::Scrollbar { rect, .. }
                | Component::Image { rect, .. }
                | Component::Terminal { rect } => *rect,
            };
            let right =
                (u32::from(rect.x) + u32::from(rect.width)).min(u32::from(viewport.columns));
            let bottom = (u32::from(rect.y) + u32::from(rect.height)).min(u32::from(viewport.rows));
            if u32::from(rect.x) >= right || u32::from(rect.y) >= bottom {
                continue;
            }
            let rect = Rect {
                width: (right - u32::from(rect.x)) as u16,
                height: (bottom - u32::from(rect.y)) as u16,
                ..rect
            };
            let x = f32::from(rect.x) * cw;
            let y = f32::from(rect.y) * ch;
            let w = f32::from(rect.width) * cw;
            let h = f32::from(rect.height) * ch;
            let area = [x, y, w, h];
            match component {
                Component::Panel { active, .. } => rounded(
                    &mut canvas,
                    [x + 1., y + 1., w - 2., h - 2.],
                    9.,
                    if *active { accent } else { border },
                    true,
                ),
                Component::Menu { .. } | Component::Dialog { .. } => {
                    fill(&mut canvas, area, &scene.background);
                    self.spans(&mut canvas, scene, cw, ch, font, Some(rect));
                    rounded(
                        &mut canvas,
                        [x + 1., y + 1., w - 2., h - 2.],
                        9.,
                        border,
                        true,
                    );
                }
                Component::ListRow {
                    label,
                    icon,
                    foreground,
                    background,
                    selected,
                    marked,
                    ..
                } => {
                    fill(&mut canvas, area, background);
                    if *selected {
                        fill(&mut canvas, [x + 1., y + 2., 2., (h - 4.).max(0.)], accent);
                    }
                    let mark = PathBuilder::from_circle(x + 15., y + h / 2., 4.5);
                    if let Some(mark) = mark {
                        if *marked {
                            canvas.fill_path(
                                &mark,
                                &paint(accent),
                                FillRule::Winding,
                                Transform::identity(),
                                None,
                            );
                        } else {
                            canvas.stroke_path(
                                &mark,
                                &paint(border),
                                &Stroke {
                                    width: 1.,
                                    ..Stroke::default()
                                },
                                Transform::identity(),
                                None,
                            );
                        }
                    }
                    draw_icon(&mut canvas, icon, x + 29., y + (h - 16.) / 2., accent);
                    self.text(
                        &mut canvas,
                        label,
                        [x + 55., y, (w - 65.).max(0.), h],
                        TextStyle {
                            size: font,
                            color: foreground,
                            bold: false,
                            mono: false,
                            ellipsis: true,
                        },
                    );
                }
                Component::Tab {
                    label,
                    active,
                    close,
                    ..
                } => {
                    fill(&mut canvas, area, &scene.background);
                    if *active {
                        rounded(&mut canvas, area, 6., border, false);
                        fill(
                            &mut canvas,
                            [x + 6., y + h - 2., (w - 12.).max(0.), 2.],
                            accent,
                        );
                    }
                    let padding = (w * 0.15).min(20.);
                    let end = close.map_or(x + w, |r| f32::from(r.x) * cw);
                    self.text(
                        &mut canvas,
                        label,
                        [
                            x + padding,
                            y + 2.,
                            (end - x - 2. * padding).max(0.),
                            (h - 4.).max(0.),
                        ],
                        TextStyle {
                            size: font,
                            color: &scene.foreground,
                            bold: false,
                            mono: false,
                            ellipsis: true,
                        },
                    );
                    if let Some(r) = close {
                        self.text(
                            &mut canvas,
                            "×",
                            [
                                f32::from(r.x) * cw,
                                f32::from(r.y) * ch,
                                f32::from(r.width) * cw,
                                f32::from(r.height) * ch,
                            ],
                            TextStyle {
                                size: font,
                                color: &scene.foreground,
                                bold: false,
                                mono: false,
                                ellipsis: false,
                            },
                        );
                    }
                }
                Component::Scrollbar { thumb, .. } => {
                    // Erase the cell block glyphs before drawing an unbroken pixel thumb.
                    fill(&mut canvas, area, &scene.background);
                    let width = w.min(6.);
                    let left = x + (w - width) / 2.;
                    rounded(&mut canvas, [left, y, width, h], width / 2., border, false);
                    let top = (f32::from(thumb.y) * ch).max(y);
                    let bottom = (f32::from(thumb.y.saturating_add(thumb.height)) * ch).min(y + h);
                    if bottom > top {
                        rounded(
                            &mut canvas,
                            [left, top, width, bottom - top],
                            width / 2.,
                            accent,
                            false,
                        );
                    }
                }
                Component::Meter {
                    value,
                    foreground,
                    background,
                    ..
                } => {
                    fill(&mut canvas, [x, y + h / 2. - 2., w, 4.], background);
                    fill(
                        &mut canvas,
                        [
                            x,
                            y + h / 2. - 2.,
                            w * f32::from((*value).min(1000)) / 1000.,
                            4.,
                        ],
                        foreground,
                    );
                }
                Component::TextField {
                    text,
                    caret,
                    secret,
                    ..
                } => {
                    fill(&mut canvas, area, &scene.background);
                    let text = if *secret {
                        "•".repeat(text.chars().count().min(64))
                    } else {
                        text.clone()
                    };
                    self.text(
                        &mut canvas,
                        &text,
                        area,
                        TextStyle {
                            size: font,
                            color: &scene.foreground,
                            bold: false,
                            mono: false,
                            ellipsis: false,
                        },
                    );
                    let caret = (x + *caret as f32 * cw).min(x + w - 1.);
                    fill(&mut canvas, [caret, y + 3., 1., (h - 6.).max(0.)], accent);
                    fill(&mut canvas, [x, y + h - 1., w, 1.], accent);
                }
                Component::Image { id, png, .. } => {
                    used.insert(id.clone());
                    if let Some(encoded) = png {
                        if self
                            .assets
                            .get(id)
                            .is_none_or(|old| old.encoded != *encoded)
                        {
                            super::assets::validate_png(encoded)?;
                            let bytes =
                                base64::engine::general_purpose::STANDARD.decode(encoded)?;
                            let mut decoder = crate::image::codecs::png::PngDecoder::new(
                                std::io::Cursor::new(bytes),
                            )?;
                            let mut limits = crate::image::Limits::default();
                            limits.max_alloc = Some(64_000_000);
                            decoder.set_limits(limits)?;
                            let pixels =
                                crate::image::DynamicImage::from_decoder(decoder)?.into_rgba8();
                            let retained_bytes = self
                                .assets
                                .iter()
                                .filter(|(old, _)| *old != id)
                                .map(|(_, asset)| asset.pixels.as_raw().len())
                                .sum::<usize>();
                            anyhow::ensure!(
                                retained_bytes + pixels.as_raw().len() <= 64_000_000,
                                "Preview cache exceeds limit"
                            );
                            self.assets.insert(
                                id.clone(),
                                Asset {
                                    encoded: encoded.clone(),
                                    pixels: Arc::new(pixels),
                                    scaled: None,
                                },
                            );
                        }
                    }
                    let retained = self
                        .assets
                        .iter()
                        .filter(|(old, _)| *old != id)
                        .filter_map(|(_, asset)| asset.scaled.as_ref())
                        .map(|(_, pixels)| pixels.data().len())
                        .sum::<usize>();
                    if let Some(asset) = self.assets.get_mut(id) {
                        draw_image(
                            &mut canvas,
                            asset,
                            area,
                            128_000_000usize.saturating_sub(retained),
                        )?;
                    }
                }
                Component::Terminal { .. } => {}
            }
        }
        self.assets.retain(|id, _| used.contains(id));
        RgbaImage::from_raw(viewport.width, viewport.height, canvas.take())
            .context("Native frame pixels")
    }
}

fn draw_image(canvas: &mut Pixmap, asset: &mut Asset, area: [f32; 4], budget: usize) -> Result<()> {
    let [x, y, w, h] = area;
    if w <= 0. || h <= 0. {
        return Ok(());
    }
    let image = &asset.pixels;
    let ratio = (w / image.width() as f32).min(h / image.height() as f32);
    let width = (image.width() as f32 * ratio).round().max(1.) as u32;
    let height = (image.height() as f32 * ratio).round().max(1.) as u32;
    anyhow::ensure!(
        u64::from(width) * u64::from(height) * 4 <= budget as u64,
        "Scaled preview cache exceeds limit"
    );
    // Reuse the resized and premultiplied preview for ordinary UI repaints.
    if asset
        .scaled
        .as_ref()
        .is_none_or(|(size, _)| *size != (width, height))
    {
        let mut pixels = crate::image::imageops::resize(
            image.as_ref(),
            width,
            height,
            crate::image::imageops::FilterType::Triangle,
        )
        .into_raw();
        for pixel in pixels.as_chunks_mut::<4>().0 {
            let alpha = u32::from(pixel[3]);
            for c in &mut pixel[..3] {
                *c = (u32::from(*c) * alpha / 255) as u8;
            }
        }
        asset.scaled =
            Pixmap::from_vec(pixels, tiny_skia::IntSize::from_wh(width, height).unwrap())
                .map(|p| ((width, height), p));
    }
    if let Some((_, pixmap)) = &asset.scaled {
        canvas.draw_pixmap(
            (x + (w - width as f32) / 2.).round() as i32,
            (y + (h - height as f32) / 2.).round() as i32,
            pixmap.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
    Ok(())
}

fn draw_icon(canvas: &mut Pixmap, kind: &str, x: f32, y: f32, color: &str) {
    let mut p = PathBuilder::new();
    if kind == "folder" {
        p.move_to(1., 3.);
        p.line_to(6., 3.);
        p.line_to(8., 5.);
        p.line_to(15., 5.);
        p.line_to(15., 14.);
        p.line_to(1., 14.);
        p.close();
    } else {
        p.move_to(3., 1.);
        p.line_to(10., 1.);
        p.line_to(14., 5.);
        p.line_to(14., 15.);
        p.line_to(3., 15.);
        p.close();
        p.move_to(10., 1.);
        p.line_to(10., 5.);
        p.line_to(14., 5.);
        match kind {
            "image" => {
                p.move_to(4., 12.);
                p.line_to(7., 8.);
                p.line_to(10., 11.);
                p.line_to(12., 9.);
            }
            "audio" => {
                p.move_to(6., 12.);
                p.line_to(6., 7.);
                p.line_to(11., 6.);
            }
            "video" => {
                p.move_to(6., 7.);
                p.line_to(10., 10.);
                p.line_to(6., 12.);
                p.close();
            }
            "archive" => {
                p.move_to(8., 6.);
                p.line_to(8., 13.);
            }
            _ => {
                p.move_to(5., 8.);
                p.line_to(11., 8.);
                p.move_to(5., 11.);
                p.line_to(11., 11.);
            }
        }
    }
    if let Some(path) = p.finish() {
        canvas.stroke_path(
            &path,
            &paint(color),
            &Stroke {
                width: 1.2,
                ..Stroke::default()
            },
            Transform::from_translate(x, y),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scene() -> Scene {
        Scene {
            revision: 1,
            interaction: 1,
            viewport: super::super::Viewport::default(),
            background: "#1e1e2e".into(),
            foreground: "#cdd6f4".into(),
            accent: "#89b4fa".into(),
            border: "#45475a".into(),
            spans: vec![],
            components: vec![],
        }
    }

    #[test]
    fn borders_are_two_pixels_and_scrollbar_has_no_cell_gaps() {
        let mut painter = Painter::new();
        let mut scene = scene();
        scene.components.push(Component::Panel {
            rect: Rect {
                x: 2,
                y: 2,
                width: 10,
                height: 10,
            },
            active: false,
        });
        scene.spans.push(super::super::protocol::Span {
            x: 20,
            y: 4,
            text: "█".into(),
            foreground: "#ffffff".into(),
            background: "#000000".into(),
            bold: false,
        });
        scene.components.push(Component::Scrollbar {
            rect: Rect {
                x: 20,
                y: 2,
                width: 1,
                height: 8,
            },
            thumb: Rect {
                x: 20,
                y: 4,
                width: 1,
                height: 3,
            },
        });
        let pixels = painter.render(&scene).unwrap();
        assert_eq!(pixels.get_pixel(60, 40).0, [69, 71, 90, 255]);
        assert_eq!(pixels.get_pixel(60, 41).0, [69, 71, 90, 255]);
        assert_eq!(pixels.get_pixel(60, 42).0, [30, 30, 46, 255]);
        // Every interior pixel is continuous across the three terminal rows.
        for y in 82..138 {
            assert_eq!(pixels.get_pixel(245, y).0, [137, 180, 250, 255]);
        }
        assert_eq!(pixels.get_pixel(245, 65).0, [69, 71, 90, 255]);
        assert_eq!(pixels.get_pixel(241, 90).0, [30, 30, 46, 255]);
    }

    #[test]
    fn preview_pixels_survive_asset_reuse_resize_and_replacement() {
        let mut painter = Painter::new();
        let mut scene = scene();
        let rect = Rect {
            x: 10,
            y: 10,
            width: 20,
            height: 10,
        };
        let png = super::super::assets::encode_png(&RgbaImage::from_pixel(
            4,
            4,
            crate::image::Rgba([30, 200, 80, 255]),
        ))
        .unwrap();
        scene.components.push(Component::Image {
            rect,
            id: "photo".into(),
            png: Some(png),
        });
        let first = painter.render(&scene).unwrap();
        assert_eq!(first.get_pixel(240, 300).0, [30, 200, 80, 255]);
        if let Component::Image { png, .. } = &mut scene.components[0] {
            *png = None;
        }
        assert_eq!(painter.render(&scene).unwrap(), first);
        scene.viewport.width = 600;
        scene.viewport.height = 400;
        let resized = painter.render(&scene).unwrap();
        assert_eq!(resized.dimensions(), (600, 400));
        assert_eq!(resized.get_pixel(120, 150).0, [30, 200, 80, 255]);
        scene.components.clear();
        assert_eq!(
            painter.render(&scene).unwrap().get_pixel(120, 150).0,
            [30, 30, 46, 255]
        );
        assert!(painter.assets.is_empty());
    }

    #[test]
    fn filenames_are_single_line_clipped_and_secret_fields_are_masked() {
        let mut painter = Painter::new();
        let mut scene = scene();
        scene.components.push(Component::ListRow {
            rect: Rect {
                x: 2,
                y: 2,
                width: 30,
                height: 1,
            },
            label: "Résumé 界 العربية e\u{301} ".repeat(30),
            icon: "file".into(),
            foreground: "#ffffff".into(),
            background: "#334455".into(),
            selected: true,
            marked: true,
        });
        let rendered = painter.render(&scene).unwrap();
        // Text must not escape the row on either the right or lower edge.
        assert_eq!(rendered.get_pixel(390, 50).0, [30, 30, 46, 255]);
        assert_eq!(rendered.get_pixel(100, 70).0, [30, 30, 46, 255]);
        scene.components = vec![Component::TextField {
            rect: Rect {
                x: 2,
                y: 2,
                width: 20,
                height: 1,
            },
            text: "secret".into(),
            caret: 0,
            secret: true,
        }];
        let hidden = painter.render(&scene).unwrap();
        if let Component::TextField { text, .. } = &mut scene.components[0] {
            *text = "xxxxxx".into();
        }
        assert_eq!(painter.render(&scene).unwrap(), hidden);
    }

    #[test]
    fn invalid_geometry_and_assets_are_errors_and_outside_components_are_clipped() {
        let mut painter = Painter::new();
        let mut scene = scene();
        scene.components.push(Component::Panel {
            rect: Rect {
                x: u16::MAX,
                y: u16::MAX,
                width: u16::MAX,
                height: u16::MAX,
            },
            active: true,
        });
        assert!(painter.render(&scene).is_ok());
        scene.components.push(Component::Image {
            rect: Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            id: "broken".into(),
            png: Some("invalid".into()),
        });
        assert!(painter.render(&scene).is_err());
        scene.viewport.width = 9000;
        assert!(painter.render(&scene).is_err());
    }
}
