//! Native, display-server-independent drawing. Only bounded scene data enters.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, Result};
use base64::Engine as _;
use cosmic_text::{
    Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight, Wrap,
};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

use super::protocol::{Component, ImageScale, Rect, Scene};
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
    animation: Option<crate::animation::Animation>,
    started: std::time::Instant,
    frame: usize,
    encoded: String,
    pixels: Arc<RgbaImage>,
    scaled: Option<((u32, u32, ImageScale), Pixmap)>,
}
pub(super) struct Painter {
    pane_corner_radius: u16,
    fonts: FontSystem,
    font_factor: f32,
    glyphs: SwashCache,
    text: HashMap<TextKey, Buffer>,
    text_bytes: usize,
    assets: HashMap<String, Asset>,
}

pub(super) fn rgb(value: &str) -> [u8; 3] {
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
    rounded_width(canvas, r, radius, color, if stroke { 2. } else { 0. });
}
fn rounded_width(canvas: &mut Pixmap, r: [f32; 4], radius: f32, color: &str, stroke: f32) {
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
        if stroke > 0. {
            canvas.stroke_path(
                &path,
                &paint(color),
                &Stroke {
                    width: stroke,
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

// Resolve Kitty's PostScript name or an explicit family against the local font
// database. The client owns this choice, so SSH never needs font files remotely.
fn resolve_font(db: &cosmic_text::fontdb::Database, requested: Option<&str>) -> String {
    for name in requested
        .into_iter()
        .chain(["DejaVu Sans Mono", "Menlo", "Liberation Mono"])
    {
        if let Some(face) = db.faces().find(|f| {
            f.post_script_name.eq_ignore_ascii_case(name)
                || f.families
                    .iter()
                    .any(|(family, _)| family.eq_ignore_ascii_case(name))
        }) {
            if let Some((family, _)) = face.families.first() {
                return family.clone();
            }
        }
    }
    "Liberation Mono".into()
}

// Glyph coverage is linear. Blend in linear light, then encode the resulting
// color back to sRGB, as Kitty's modern text compositor does. Byte-space mixing
// makes light text on dark backgrounds thin and uneven. LUTs avoid powf in the
// raster loop; this affects glyphs only, never image preview pixels.
fn mix_color(foreground: &str, background: &str, amount: f32) -> String {
    let fg = rgb(foreground);
    let bg = rgb(background);
    let c: [u8; 3] = std::array::from_fn(|i| {
        (f32::from(fg[i]) * amount + f32::from(bg[i]) * (1. - amount)).round() as u8
    });
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn blend_text_channel(fg: u8, bg: u8, coverage: u8) -> u8 {
    if coverage == 0 {
        return bg;
    }
    if coverage == 255 {
        return fg;
    }
    static LUT: std::sync::OnceLock<([f32; 256], [u8; 4097])> = std::sync::OnceLock::new();
    let (decode, encode) = LUT.get_or_init(|| {
        let decode = std::array::from_fn(|i| {
            let s = i as f32 / 255.;
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        });
        let encode = std::array::from_fn(|i| {
            let l = i as f32 / 4096.;
            let s = if l <= 0.0031308 {
                l * 12.92
            } else {
                1.055 * l.powf(1. / 2.4) - 0.055
            };
            (s * 255.).round() as u8
        });
        (decode, encode)
    });
    let alpha = f32::from(coverage) / 255.;
    let linear = decode[usize::from(fg)] * alpha + decode[usize::from(bg)] * (1. - alpha);
    encode[(linear * 4096.).round().clamp(0., 4096.) as usize]
}

impl Painter {
    pub(super) fn animation_due(&self) -> bool {
        self.assets.values().any(|a| {
            a.animation
                .as_ref()
                .is_some_and(|seq| seq.index(a.started.elapsed()) != a.frame)
        })
    }
    pub(super) fn animating(&self) -> bool {
        self.assets.values().any(|a| {
            a.animation
                .as_ref()
                .is_some_and(|seq| seq.len() > 1 && !seq.finished(a.started.elapsed()))
        })
    }
    fn surface(&mut self, canvas: &mut Pixmap, surface: &super::surface::Surface, area: [f32; 4]) {
        use super::surface::Primitive;
        let [x, y, width, height] = area;
        let sx = width / f32::from(surface.width);
        let sy = height / f32::from(surface.height);
        fill(canvas, area, &surface.background);
        for node in &surface.nodes {
            let r = node.rect();
            let rect = [
                x + f32::from(r.x) * sx,
                y + f32::from(r.y) * sy,
                f32::from(r.width) * sx,
                f32::from(r.height) * sy,
            ];
            match node {
                Primitive::Fill { color, radius, .. } => {
                    rounded(canvas, rect, f32::from(*radius), color, false)
                }
                Primitive::Border { color, radius, .. } => rounded(
                    canvas,
                    [
                        rect[0] + 1.,
                        rect[1] + 1.,
                        (rect[2] - 2.).max(0.),
                        (rect[3] - 2.).max(0.),
                    ],
                    f32::from(*radius),
                    color,
                    true,
                ),
                Primitive::Text {
                    text,
                    color,
                    size,
                    bold,
                    mono,
                    ..
                } => self.text(
                    canvas,
                    text,
                    rect,
                    TextStyle {
                        size: f32::from(*size),
                        color,
                        bold: *bold,
                        mono: *mono,
                        ellipsis: true,
                    },
                ),
                Primitive::Icon { name, color, .. } => {
                    draw_icon(canvas, name, rect[0], rect[1], rect[2].min(rect[3]), color)
                }
            }
        }
    }
    pub(crate) fn live_image(&mut self, id: String, pixels: Arc<RgbaImage>) {
        self.assets.insert(
            id,
            Asset {
                animation: None,
                started: std::time::Instant::now(),
                frame: 0,
                encoded: "\0live".into(),
                pixels,
                scaled: None,
            },
        );
    }
    pub fn new() -> Self {
        Self::with_font(super::font::Font::default())
    }
    pub(crate) fn with_font(font: super::font::Font) -> Self {
        Self::with_pane_corner_radius(font, 9)
    }
    pub(crate) fn with_pane_corner_radius(font: super::font::Font, radius: u16) -> Self {
        let mut db = cosmic_text::fontdb::Database::new();
        for bytes in FONTS {
            db.load_font_data(bytes.to_vec());
        }
        // Bundled fonts guarantee a consistent baseline; installed fonts supply
        // Unicode fallback on both Linux and macOS. No fontconfig C dependency.
        if std::env::var("STAR_GRAPHICS_SYSTEM_FONTS").as_deref() != Ok("0") {
            db.load_system_fonts();
        }
        let family = resolve_font(&db, font.name.as_deref());
        db.set_sans_serif_family(&family);
        db.set_monospace_family(&family);
        let font_factor = font
            .pixels
            .zip(font.cell)
            .map(|(pixels, (cw, ch))| {
                pixels / f32::from(crate::native_surface::Metrics::from_cell(cw, ch).font)
            })
            .unwrap_or(1.);
        tracing::info!(requested = ?font.name, %family, pixels = ?font.pixels, font_factor, "Native terminal font selected");
        Self {
            pane_corner_radius: radius,
            fonts: FontSystem::new_with_locale_and_db("en-US".into(), db),
            font_factor,
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
        // Placed panel rows may be shorter than the source terminal cells.
        // Shape at a size whose complete line fits the destination, including
        // terminal font scaling; centring an oversized line clips its glyphs.
        let size = (size * self.font_factor).clamp(1., 128.).min(h / 1.2);
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
        // Single glyph surfaces are used for menu mnemonics. Fractional font
        // advances can exceed their integer cell by a fraction of a pixel;
        // replacing each letter with an ellipsis destroys the entire menu.
        let clipped = ellipsis && value.chars().count() > 1 && w >= size && text_width > w + 0.5;
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
                for c in 0..3 {
                    pixels[offset + c] = blend_text_channel(rgba[c], pixels[offset + c], rgba[3]);
                }
            },
        );
        if clipped {
            self.text(
                canvas,
                "…",
                [x + w - size, y, size, h],
                TextStyle {
                    size: size / self.font_factor,
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
        metrics: [f32; 3],
        region: Option<Rect>,
        placement: Option<&super::placement::Placement>,
    ) {
        let [cw, ch, font] = metrics;
        let row_edge = |row: u16| placement.map_or(f32::from(row) * ch, |p| p.row_edge(row));
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
            let y = row_edge(span.y).floor();
            let bottom = row_edge(span.y.saturating_add(1)).floor();
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
        let v = scene.viewport.validate()?;
        anyhow::ensure!(
            scene.spans.len() <= super::protocol::MAX_CELLS
                && scene.components.len() <= super::protocol::MAX_CELLS,
            "Scene exceeds limits"
        );
        if scene.placements.is_empty() {
            return self.render_grid(scene, None, true, None);
        }
        anyhow::ensure!(scene.placements.len() <= 16, "Too many pixel placements");
        let mut area = 0u64;
        for placement in &scene.placements {
            placement.validate(v)?;
            area += u64::from(placement.target.width) * u64::from(placement.target.height);
        }
        anyhow::ensure!(
            area <= 2 * u64::from(v.width) * u64::from(v.height),
            "Pixel layers exceed limits"
        );
        let images: HashSet<_> = scene
            .components
            .iter()
            .filter_map(|c| match c {
                Component::Image { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        anyhow::ensure!(images.len() <= 8, "Too many preview assets");
        self.assets.retain(|id, _| images.contains(id));
        let [r, g, b] = rgb(&scene.background);
        let mut frame =
            RgbaImage::from_pixel(v.width, v.height, crate::image::Rgba([r, g, b, 255]));
        let font = ((v.height as f32 / f32::from(v.rows) * 0.84)
            .min(v.width as f32 / f32::from(v.columns) / 0.6))
        .floor()
        .clamp(1., 64.);
        for placement in &scene.placements {
            // Rasterize at the destination size. Text is never resized as an image.
            let layer = self.render_grid(
                &placement.project(scene),
                Some(font),
                false,
                Some(placement),
            )?;
            crate::image::GenericImage::copy_from(
                &mut frame,
                &layer,
                u32::from(placement.target.x),
                u32::from(placement.target.y),
            )?;
        }
        Ok(frame)
    }

    fn render_grid(
        &mut self,
        scene: &Scene,
        font_override: Option<f32>,
        manage_assets: bool,
        placement: Option<&super::placement::Placement>,
    ) -> Result<RgbaImage> {
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
        let font =
            font_override.unwrap_or_else(|| (ch * 0.84).min(cw / 0.6).floor().clamp(1., 64.));
        let mut canvas =
            Pixmap::new(viewport.width, viewport.height).context("Allocate graphical frame")?;
        let [r, g, b] = rgb(&scene.background);
        canvas.fill(tiny_skia::Color::from_rgba8(r, g, b, 255));
        self.spans(&mut canvas, scene, [cw, ch, font], None, placement);
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
        if manage_assets {
            self.assets.retain(|id, _| active_images.contains(id));
        }
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
        let row_edge = |row: u16| placement.map_or(f32::from(row) * ch, |p| p.row_edge(row));
        for component in &scene.components {
            let rect = match component {
                Component::Surface { rect, .. }
                | Component::Panel { rect, .. }
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
            let y = row_edge(rect.y);
            let w = f32::from(rect.width) * cw;
            let h = row_edge(rect.y.saturating_add(rect.height)) - y;
            let area = [x, y, w, h];
            match component {
                Component::Surface { surface, .. } => {
                    surface.validate()?;
                    self.surface(&mut canvas, surface, area);
                }
                Component::Panel { active, .. } => rounded_width(
                    &mut canvas,
                    [x + 1.5, y + 1.5, w - 3., h - 3.],
                    f32::from(self.pane_corner_radius),
                    if *active { accent } else { border },
                    3.,
                ),
                Component::Menu { .. } | Component::Dialog { .. } => {
                    fill(&mut canvas, area, &scene.background);
                    self.spans(&mut canvas, scene, [cw, ch, font], Some(rect), placement);
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
                    marked,
                    marking,
                    ..
                } => {
                    fill(&mut canvas, area, background);
                    let mark = (*marking || *marked)
                        .then(|| PathBuilder::from_circle(x + 6., y + h / 2., 4.5))
                        .flatten();
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
                    let icon_size = (font + 1.).min((h - 4.).max(1.)).min(16.);
                    draw_icon(
                        &mut canvas,
                        icon,
                        x + 18.,
                        y + (h - icon_size) / 2.,
                        icon_size,
                        accent,
                    );
                    self.text(
                        &mut canvas,
                        label,
                        [x + 40., y, (w - 48.).max(0.), h],
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
                    number,
                    active,
                    close,
                    ..
                } => {
                    fill(&mut canvas, area, &scene.background);
                    if *active {
                        let bg = rgb(&scene.background);
                        let tint = rgb(accent);
                        let tint = format!(
                            "#{:02x}{:02x}{:02x}",
                            (f32::from(bg[0]) * 0.82 + f32::from(tint[0]) * 0.18) as u8,
                            (f32::from(bg[1]) * 0.82 + f32::from(tint[1]) * 0.18) as u8,
                            (f32::from(bg[2]) * 0.82 + f32::from(tint[2]) * 0.18) as u8
                        );
                        rounded(
                            &mut canvas,
                            [x + 2., y + 2., (w - 5.).max(0.), (h - 4.).max(0.)],
                            4.,
                            &tint,
                            false,
                        );
                    }
                    if close.is_none() && matches!(label.as_str(), "‹" | "›" | "+") {
                        tab_control(&mut canvas, label, area, &scene.foreground);
                        continue;
                    }
                    let padding = 14f32.min(w / 4.);
                    let end = close.map_or(x + w, |r| f32::from(r.x) * cw);
                    let inactive = mix_color(&scene.foreground, &scene.background, 0.62);
                    let label_color = if *active {
                        &scene.foreground
                    } else {
                        &inactive
                    };
                    let number_width =
                        number.map_or(0., |n| (n.to_string().len() as f32 * cw + 10.).max(17.));
                    if let Some(n) = number {
                        self.text(
                            &mut canvas,
                            &n.to_string(),
                            [x + padding, y, number_width, h],
                            TextStyle {
                                size: (font - 2.).max(9.),
                                color: if *active { accent } else { &inactive },
                                bold: true,
                                mono: true,
                                ellipsis: false,
                            },
                        );
                    }
                    self.text(
                        &mut canvas,
                        label,
                        [
                            x + padding + number_width,
                            y,
                            (end - x - padding - number_width - 4.).max(0.),
                            h,
                        ],
                        TextStyle {
                            size: font,
                            color: label_color,
                            bold: *active,
                            mono: false,
                            ellipsis: true,
                        },
                    );
                    if let Some(r) = close {
                        tab_control(
                            &mut canvas,
                            "×",
                            [
                                f32::from(r.x) * cw,
                                row_edge(r.y),
                                f32::from(r.width) * cw,
                                row_edge(r.y.saturating_add(r.height)) - row_edge(r.y),
                            ],
                            label_color,
                        );
                    }
                }

                Component::Scrollbar { thumb, .. } => {
                    // Erase the cell block glyphs before drawing an unbroken pixel thumb.
                    fill(&mut canvas, area, &scene.background);
                    let width = w.min(6.);
                    let left = x + (w - width) / 2.;
                    let top = row_edge(thumb.y).max(y);
                    let bottom = row_edge(thumb.y.saturating_add(thumb.height)).min(y + h);
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
                    fill(&mut canvas, area, &scene.background);
                    fill(
                        &mut canvas,
                        [x, y + h / 2. - 2., w.min(96.), 4.],
                        background,
                    );
                    fill(
                        &mut canvas,
                        [
                            x,
                            y + h / 2. - 2.,
                            w.min(96.) * f32::from((*value).min(1000)) / 1000.,
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
                Component::Image {
                    id,
                    png,
                    scale,
                    zoom,
                    ..
                } => {
                    used.insert(id.clone());
                    if let Some(encoded) = png {
                        if self
                            .assets
                            .get(id)
                            .is_none_or(|old| old.encoded != "\0live" && old.encoded != *encoded)
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
                            let (animation, pixels) = if decoder.is_apng()? {
                                use crate::image::AnimationDecoder as _;
                                let decoder = decoder.apng()?;
                                let plays = match decoder.loop_count() {
                                    crate::image::metadata::LoopCount::Infinite => 0,
                                    crate::image::metadata::LoopCount::Finite(n) => n.get(),
                                };
                                let started = std::time::Instant::now();
                                let animation = crate::animation::Animation::collect(
                                    decoder.into_frames(),
                                    plays,
                                    64_000_000,
                                    || started.elapsed() > std::time::Duration::from_secs(15),
                                )?;
                                anyhow::ensure!(
                                    !animation.truncated,
                                    "Animation cache exceeds limit"
                                );
                                let pixels = animation.frame(0)?;
                                (Some(animation), pixels)
                            } else {
                                (
                                    None,
                                    Arc::new(
                                        crate::image::DynamicImage::from_decoder(decoder)?
                                            .into_rgba8(),
                                    ),
                                )
                            };
                            let retained_bytes = self
                                .assets
                                .iter()
                                .filter(|(old, _)| *old != id)
                                .map(|(_, asset)| {
                                    asset
                                        .animation
                                        .as_ref()
                                        .map_or(asset.pixels.as_raw().len(), |seq| seq.bytes())
                                })
                                .sum::<usize>();
                            anyhow::ensure!(
                                retained_bytes
                                    + animation
                                        .as_ref()
                                        .map_or(pixels.as_raw().len(), |seq| seq.bytes())
                                    <= 64_000_000,
                                "Preview cache exceeds limit"
                            );
                            self.assets.insert(
                                id.clone(),
                                Asset {
                                    encoded: encoded.clone(),
                                    pixels,
                                    animation,
                                    started: std::time::Instant::now(),
                                    frame: 0,
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
                        if let Some(seq) = &asset.animation {
                            let index = seq.index(asset.started.elapsed());
                            if index != asset.frame {
                                asset.frame = index;
                                asset.pixels = seq.frame(index)?;
                                asset.scaled = None;
                            }
                        }
                        draw_image(
                            &mut canvas,
                            asset,
                            area,
                            *scale,
                            *zoom,
                            128_000_000usize.saturating_sub(retained),
                        )?;
                    }
                }
                Component::Terminal { .. } => {}
            }
        }
        if manage_assets {
            self.assets.retain(|id, _| used.contains(id));
        }
        RgbaImage::from_raw(viewport.width, viewport.height, canvas.take())
            .context("Native frame pixels")
    }
}

fn draw_image(
    canvas: &mut Pixmap,
    asset: &mut Asset,
    area: [f32; 4],
    scale: ImageScale,
    zoom: u16,
    budget: usize,
) -> Result<()> {
    let [x, y, w, h] = area;
    if w <= 0. || h <= 0. {
        return Ok(());
    }
    let image = &asset.pixels;
    let ratio = (w / image.width() as f32).min(h / image.height() as f32);
    let ratio = match scale {
        ImageScale::Smooth => ratio,
        ImageScale::Pixels if ratio >= 1. => ratio.floor(),
        ImageScale::Pixels => 1. / (1. / ratio).ceil(),
        ImageScale::One => ratio.min(1.),
    };
    let zoom = zoom.clamp(25, 800);
    let ratio = ratio * f32::from(zoom) / 100.;
    let ratio = if scale == ImageScale::Pixels {
        if ratio >= 1. {
            ratio.round().max(1.)
        } else {
            1. / (1. / ratio).round().max(1.)
        }
    } else {
        ratio
    };
    // Zoomed previews sample the bounded source directly and clip to the viewport.
    // Never allocate an image whose dimensions grow with the zoom factor.
    let width = if zoom != 100 {
        image.width()
    } else {
        (image.width() as f32 * ratio).round().max(1.) as u32
    };
    let height = if zoom != 100 {
        image.height()
    } else {
        (image.height() as f32 * ratio).round().max(1.) as u32
    };
    anyhow::ensure!(
        u64::from(width) * u64::from(height) * 4 <= budget as u64,
        "Scaled preview cache exceeds limit"
    );
    // Reuse the resized and premultiplied preview for ordinary UI repaints.
    if asset
        .scaled
        .as_ref()
        .is_none_or(|(size, _)| *size != (width, height, scale))
    {
        let mut pixels = crate::image::imageops::resize(
            image.as_ref(),
            width,
            height,
            match scale {
                ImageScale::Smooth => crate::image::imageops::FilterType::Triangle,
                _ => crate::image::imageops::FilterType::Nearest,
            },
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
                .map(|p| ((width, height, scale), p));
    }
    if let Some((_, pixmap)) = &asset.scaled {
        if zoom != 100 {
            let mut mask = tiny_skia::Mask::new(canvas.width(), canvas.height())
                .context("Allocate preview clip")?;
            if let Some(rect) = tiny_skia::Rect::from_xywh(x, y, w, h) {
                mask.fill_path(
                    &PathBuilder::from_rect(rect),
                    FillRule::Winding,
                    false,
                    Transform::identity(),
                );
            }
            let transform = Transform::from_scale(ratio, ratio).post_translate(
                (x + (w - width as f32 * ratio) / 2.).round(),
                (y + (h - height as f32 * ratio) / 2.).round(),
            );
            canvas.draw_pixmap(
                0,
                0,
                pixmap.as_ref(),
                &tiny_skia::PixmapPaint {
                    quality: if scale == ImageScale::Smooth {
                        tiny_skia::FilterQuality::Bilinear
                    } else {
                        tiny_skia::FilterQuality::Nearest
                    },
                    ..Default::default()
                },
                transform,
                Some(&mask),
            );
            return Ok(());
        }
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

/// Tab controls use centred vector strokes so their weight and click area
/// stay consistent across fonts and terminal scales.
fn tab_control(canvas: &mut Pixmap, label: &str, rect: [f32; 4], color: &str) {
    let [x, y, w, h] = rect;
    let size = 11f32.min(w - 6.).min(h - 6.);
    if size <= 0. {
        return;
    }
    let cx = x + w / 2.;
    let cy = y + h / 2.;
    let r = size / 2.;
    let mut p = PathBuilder::new();
    match label {
        "+" => {
            p.move_to(cx - r, cy);
            p.line_to(cx + r, cy);
            p.move_to(cx, cy - r);
            p.line_to(cx, cy + r);
        }
        "×" => {
            let r = r * 0.75;
            p.move_to(cx - r, cy - r);
            p.line_to(cx + r, cy + r);
            p.move_to(cx + r, cy - r);
            p.line_to(cx - r, cy + r);
        }
        "‹" | "›" => {
            let dir = if label == "‹" { -1. } else { 1. };
            p.move_to(cx - dir * r * 0.4, cy - r);
            p.line_to(cx + dir * r * 0.4, cy);
            p.line_to(cx - dir * r * 0.4, cy + r);
        }
        _ => return,
    }
    if let Some(path) = p.finish() {
        canvas.stroke_path(
            &path,
            &paint(color),
            &Stroke {
                width: 1.6,
                line_cap: tiny_skia::LineCap::Round,
                line_join: tiny_skia::LineJoin::Round,
                ..Stroke::default()
            },
            Transform::identity(),
            None,
        );
    }
}

fn draw_icon(canvas: &mut Pixmap, kind: &str, x: f32, y: f32, size: f32, color: &str) {
    if matches!(
        kind,
        "media-speaker" | "media-headphones" | "media-subtitles" | "media-fullscreen"
    ) {
        let transform = Transform::from_row(size / 24., 0., 0., size / 24., x, y);
        let mut p = PathBuilder::new();
        match kind {
            "media-speaker" => {
                p.move_to(3., 9.);
                p.line_to(7., 9.);
                p.line_to(12., 5.);
                p.line_to(12., 19.);
                p.line_to(7., 15.);
                p.line_to(3., 15.);
                p.close();
            }
            "media-headphones" => {
                p.move_to(4., 13.);
                p.line_to(7., 13.);
                p.line_to(7., 20.);
                p.line_to(4., 20.);
                p.close();
                p.move_to(17., 13.);
                p.line_to(20., 13.);
                p.line_to(20., 20.);
                p.line_to(17., 20.);
                p.close();
            }
            _ => {}
        }
        if let Some(path) = p.finish() {
            canvas.fill_path(
                &path,
                &paint(color),
                tiny_skia::FillRule::Winding,
                transform,
                None,
            );
        }
        let mut p = PathBuilder::new();
        match kind {
            "media-speaker" => {
                p.move_to(16., 8.);
                p.cubic_to(19., 10., 19., 14., 16., 16.);
                p.move_to(19., 5.);
                p.cubic_to(24., 9., 24., 15., 19., 19.);
            }
            "media-headphones" => {
                p.move_to(4., 17.);
                p.line_to(4., 12.);
                p.cubic_to(4., 1., 20., 1., 20., 12.);
                p.line_to(20., 17.);
            }
            "media-subtitles" => {
                p.move_to(4., 4.);
                p.line_to(20., 4.);
                p.quad_to(22., 4., 22., 6.);
                p.line_to(22., 18.);
                p.quad_to(22., 20., 20., 20.);
                p.line_to(4., 20.);
                p.quad_to(2., 20., 2., 18.);
                p.line_to(2., 6.);
                p.quad_to(2., 4., 4., 4.);
                p.close();
                for (x, y, end) in [(6., 10., 10.), (14., 10., 18.), (6., 15., 18.)] {
                    p.move_to(x, y);
                    p.line_to(end, y);
                }
            }
            _ => {
                for (x, y, dx, dy) in [
                    (3., 3., 1., 1.),
                    (21., 3., -1., 1.),
                    (3., 21., 1., -1.),
                    (21., 21., -1., -1.),
                ] {
                    p.move_to(x + dx * 6., y);
                    p.line_to(x, y);
                    p.line_to(x, y + dy * 6.);
                }
            }
        }
        if let Some(path) = p.finish() {
            canvas.stroke_path(
                &path,
                &paint(color),
                &Stroke {
                    width: 1.8,
                    line_cap: tiny_skia::LineCap::Round,
                    line_join: tiny_skia::LineJoin::Round,
                    ..Stroke::default()
                },
                transform,
                None,
            );
        }
        return;
    }
    // Solid media faces match AMP's Material-style 24-unit transport artwork.
    // Seeking uses double triangles, deliberately distinct from track skipping.
    if let Some(kind) = kind.strip_prefix("media-") {
        let mut p = PathBuilder::new();
        let polygons: Vec<Vec<(f32, f32)>> = match kind {
            "play" => vec![vec![(8., 5.14), (8., 19.14), (19., 12.14)]],
            "pause" => vec![
                vec![(6., 5.), (10., 5.), (10., 19.), (6., 19.)],
                vec![(14., 5.), (18., 5.), (18., 19.), (14., 19.)],
            ],
            "stop" => vec![vec![(6., 6.), (18., 6.), (18., 18.), (6., 18.)]],
            "forward" => vec![
                vec![(3., 6.), (12., 12.), (3., 18.)],
                vec![(12., 6.), (21., 12.), (12., 18.)],
            ],
            "rewind" => vec![
                vec![(21., 6.), (12., 12.), (21., 18.)],
                vec![(12., 6.), (3., 12.), (12., 18.)],
            ],
            _ => vec![],
        };
        for polygon in polygons {
            p.move_to(polygon[0].0, polygon[0].1);
            for (px, py) in polygon.into_iter().skip(1) {
                p.line_to(px, py);
            }
            p.close();
        }
        if let Some(path) = p.finish() {
            canvas.fill_path(
                &path,
                &paint(color),
                tiny_skia::FillRule::Winding,
                Transform::from_row(size / 24., 0., 0., size / 24., x, y),
                None,
            );
        }
        return;
    }
    let mut p = PathBuilder::new();
    if matches!(
        kind,
        "play" | "pause" | "stop" | "previous" | "next" | "close"
    ) {
        match kind {
            "play" | "next" => {
                p.move_to(4., 2.);
                p.line_to(13., 8.);
                p.line_to(4., 14.);
                p.close();
                if kind == "next" {
                    p.move_to(14., 2.);
                    p.line_to(14., 14.);
                }
            }
            "previous" => {
                p.move_to(12., 2.);
                p.line_to(3., 8.);
                p.line_to(12., 14.);
                p.close();
                p.move_to(2., 2.);
                p.line_to(2., 14.);
            }
            "pause" => {
                p.move_to(5., 2.);
                p.line_to(5., 14.);
                p.move_to(11., 2.);
                p.line_to(11., 14.);
            }
            "stop" => {
                p.move_to(3., 3.);
                p.line_to(13., 3.);
                p.line_to(13., 13.);
                p.line_to(3., 13.);
                p.close();
            }
            _ => {
                p.move_to(4., 4.);
                p.line_to(12., 12.);
                p.move_to(12., 4.);
                p.line_to(4., 12.);
            }
        }
    } else if kind == "folder" {
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
            Transform::from_row(size / 16., 0., 0., size / 16., x, y),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn media_faces_are_solid_and_seek_arrows_are_mirrored() {
        for size in [16, 24, 40] {
            let mut play = Pixmap::new(size, size).unwrap();
            draw_icon(&mut play, "media-play", 0., 0., size as f32, "#ffffff");
            assert_eq!(play.pixel(size / 2, size / 2).unwrap().alpha(), 255);
            let mut rewind = Pixmap::new(size, size).unwrap();
            let mut forward = Pixmap::new(size, size).unwrap();
            draw_icon(&mut rewind, "media-rewind", 0., 0., size as f32, "#ffffff");
            draw_icon(
                &mut forward,
                "media-forward",
                0.,
                0.,
                size as f32,
                "#ffffff",
            );
            for y in 0..size {
                for x in 0..size {
                    assert!(
                        rewind
                            .pixel(x, y)
                            .unwrap()
                            .alpha()
                            .abs_diff(forward.pixel(size - x - 1, y).unwrap().alpha())
                            <= 1
                    );
                }
            }
        }
    }
    #[test]
    fn numbered_tabs_keep_numbers_distinct_from_labels_and_controls() {
        let mut scene = scene();
        scene.components.push(Component::Tab {
            rect: Rect {
                x: 0,
                y: 0,
                width: 26,
                height: 2,
            },
            label: "Downloads".into(),
            number: Some(12),
            active: true,
            close: Some(Rect {
                x: 22,
                y: 0,
                width: 3,
                height: 2,
            }),
        });
        let mut painter = Painter::new();
        painter.render(&scene).unwrap();
        assert!(painter.text.keys().any(|k| k.text == "12"));
        assert!(painter.text.keys().any(|k| k.text == "Downloads" && k.bold));
        let encoded = serde_json::to_value(&scene.components[0]).unwrap();
        assert_eq!(encoded["number"], 12);
        let mut old = encoded;
        old.as_object_mut().unwrap().remove("number");
        assert!(matches!(
            serde_json::from_value::<Component>(old).unwrap(),
            Component::Tab { number: None, .. }
        ));
    }

    #[test]
    fn text_coverage_uses_linear_light_on_dark_and_light_backgrounds() {
        assert_eq!(blend_text_channel(255, 0, 0), 0);
        assert_eq!(blend_text_channel(255, 0, 255), 255);
        assert!((186..=189).contains(&blend_text_channel(255, 0, 128)));
        assert!((186..=189).contains(&blend_text_channel(0, 255, 128)));
        for v in 0..=255 {
            assert!((i16::from(blend_text_channel(v, v, 128)) - i16::from(v)).abs() <= 1);
        }
    }

    #[test]
    fn narrow_menu_letters_remain_glyphs_at_nine_point_terminal_size() {
        let mut painter = Painter::with_font(super::super::font::Font {
            name: Some("Liberation Mono".into()),
            pixels: Some(12.),
            cell: Some((7, 16)),
        });
        let mut canvas = Pixmap::new(7, 16).unwrap();
        painter.text(
            &mut canvas,
            "h",
            [0., 0., 7., 16.],
            TextStyle {
                size: 11.,
                color: "#ffffff",
                bold: false,
                mono: true,
                ellipsis: true,
            },
        );
        assert!(!painter.text.keys().any(|k| k.text == "…"));
        let ink_rows = canvas
            .data()
            .as_chunks::<28>()
            .0
            .iter()
            .filter(|row| row.iter().any(|b| *b > 0))
            .count();
        assert!(ink_rows >= 6, "letter collapsed to an ellipsis or vanished");
    }

    #[test]
    fn compressed_surface_rows_keep_complete_scaled_glyphs() {
        let mut painter = Painter::with_font(super::super::font::Font {
            name: Some("Liberation Mono".into()),
            pixels: Some(24.),
            cell: Some((10, 20)),
        });
        for height in [12, 16, 20, 28] {
            let mut canvas = Pixmap::new(240, height).unwrap();
            painter.text(
                &mut canvas,
                "Agj 00:59 / 228:10",
                [0., 0., 240., height as f32],
                TextStyle {
                    size: 16.,
                    color: "#ffffff",
                    bold: false,
                    mono: true,
                    ellipsis: false,
                },
            );
            let size = painter
                .text
                .keys()
                .map(|k| f32::from_bits(k.size))
                .filter(|size| *size <= height as f32 / 1.2 + 0.001)
                .max_by(f32::total_cmp)
                .unwrap();
            assert!(size * 1.2 <= height as f32 + 0.001);
            assert!(canvas.data().iter().any(|value| *value != 0));
            // Compare with the same glyphs painted into an unclipped taller
            // row. Every ink pixel must survive the compact destination.
            let mut tall = Pixmap::new(240, height + 20).unwrap();
            painter.text(
                &mut tall,
                "Agj 00:59 / 228:10",
                [0., 0., 240., (height + 20) as f32],
                TextStyle {
                    size: size / painter.font_factor,
                    color: "#ffffff",
                    bold: false,
                    mono: true,
                    ellipsis: false,
                },
            );
            let ink = |p: &Pixmap| {
                p.data()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|pixel| pixel[3] != 0)
                    .count()
            };
            assert_eq!(ink(&canvas), ink(&tall), "clipped glyphs at {height}px");
        }
    }

    #[test]
    fn truncation_does_not_scale_the_ellipsis_twice() {
        let mut painter = Painter::with_font(super::super::font::Font {
            name: Some("Liberation Mono".into()),
            pixels: Some(12.),
            cell: Some((7, 16)),
        });
        let mut canvas = Pixmap::new(40, 16).unwrap();
        painter.text(
            &mut canvas,
            "long filename",
            [0., 0., 40., 16.],
            TextStyle {
                size: 11.,
                color: "#ffffff",
                bold: false,
                mono: true,
                ellipsis: true,
            },
        );
        let ellipsis = painter.text.keys().find(|k| k.text == "…").unwrap();
        assert!((f32::from_bits(ellipsis.size) - 12.).abs() < 0.001);
    }

    #[test]
    fn font_resolution_accepts_terminal_postscript_names_and_family_overrides() {
        let mut db = cosmic_text::fontdb::Database::new();
        for font in FONTS {
            db.load_font_data(font.to_vec());
        }
        assert_eq!(resolve_font(&db, Some("LiberationMono")), "Liberation Mono");
        assert_eq!(
            resolve_font(&db, Some("Liberation Mono")),
            "Liberation Mono"
        );
        assert_eq!(resolve_font(&db, Some("Missing Font")), "Liberation Mono");
        assert_eq!(resolve_font(&db, None), "Liberation Mono");
    }

    #[test]
    fn terminal_font_size_applies_to_both_cell_and_surface_text() {
        let mut painter = Painter::with_font(super::super::font::Font {
            name: Some("Liberation Mono".into()),
            pixels: Some(15.),
            cell: Some((9, 20)),
        });
        let base = f32::from(crate::native_surface::Metrics::from_cell(9, 20).font);
        let mut canvas = Pixmap::new(200, 30).unwrap();
        for mono in [true, false] {
            painter.text(
                &mut canvas,
                "Terminal text",
                [0., 0., 200., 30.],
                TextStyle {
                    size: base,
                    color: "#ffffff",
                    bold: false,
                    mono,
                    ellipsis: false,
                },
            );
        }
        assert!(painter
            .text
            .keys()
            .all(|k| (f32::from_bits(k.size) - 15.).abs() < 0.001));
        let widths: Vec<_> = painter
            .text
            .values()
            .map(|b| b.layout_runs().next().unwrap().line_w)
            .collect();
        assert_eq!(
            widths[0], widths[1],
            "native labels and cell text must use the same face"
        );
    }

    fn scene() -> Scene {
        Scene {
            revision: 1,
            interaction: 1,
            scroll_interaction: None,
            viewport: super::super::Viewport::default(),
            background: "#1e1e2e".into(),
            foreground: "#cdd6f4".into(),
            accent: "#89b4fa".into(),
            border: "#45475a".into(),
            spans: vec![],
            placements: vec![],
            resize_handles: vec![],
            pointer_regions: vec![],
            components: vec![],
        }
    }

    #[test]
    fn renderer_plays_animation_without_new_controller_scenes_and_stops() {
        use super::super::renderer::{RenderMessage, Renderer};
        let delay = std::time::Duration::from_millis(200);
        let seq = crate::anim::FrameSequence {
            frames: vec![
                crate::anim::Frame {
                    img: Arc::new(RgbaImage::from_pixel(
                        2,
                        2,
                        crate::image::Rgba([255, 0, 0, 255]),
                    )),
                    delay,
                },
                crate::anim::Frame {
                    img: Arc::new(RgbaImage::from_pixel(
                        2,
                        2,
                        crate::image::Rgba([0, 255, 0, 255]),
                    )),
                    delay,
                },
            ],
            total: delay * 2,
            plays: 1,
            id: 0,
            truncated: false,
        };
        let frames = seq
            .frames
            .iter()
            .map(|f| {
                Ok(crate::image::Frame::from_parts(
                    (*f.img).clone(),
                    0,
                    0,
                    crate::image::Delay::from_numer_denom_ms(f.delay.as_millis() as u32, 1),
                ))
            })
            .collect::<Vec<_>>();
        let seq = crate::animation::Animation::collect(
            crate::image::Frames::new(Box::new(frames.into_iter())),
            1,
            64_000_000,
            || false,
        )
        .unwrap();
        let mut scene = scene();
        scene.viewport = super::super::Viewport {
            columns: 20,
            rows: 10,
            width: 200,
            height: 100,
            generation: 1,
        };
        scene.components.push(Component::Image {
            rect: Rect {
                x: 0,
                y: 0,
                width: 20,
                height: 10,
            },
            id: "gif".into(),
            png: Some(super::super::assets::encode_animation(&seq).unwrap()),
            zoom: 100,
            scale: ImageScale::Pixels,
        });
        let mut renderer = Renderer::spawn().unwrap();
        renderer.scene(&scene).unwrap();
        let mut colors = Vec::new();
        for _ in 0..2 {
            let message = renderer
                .output
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            let RenderMessage::Frame {
                revision,
                pixels: Some(pixels),
                ..
            } = message
            else {
                panic!("{message:?}")
            };
            assert_eq!(revision, 1);
            colors.push(*pixels.get_pixel(100, 50));
        }
        assert_eq!(
            colors,
            vec![
                crate::image::Rgba([255, 0, 0, 255]),
                crate::image::Rgba([0, 255, 0, 255])
            ]
        );
        assert!(renderer
            .output
            .recv_timeout(std::time::Duration::from_millis(500))
            .is_err());
        scene.components.clear();
        renderer.scene(&scene).unwrap();
        assert!(matches!(
            renderer
                .output
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            RenderMessage::Frame { .. }
        ));
        assert!(renderer
            .output
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err());
    }

    #[test]
    fn zoom_clips_to_preview_and_does_not_allocate_at_enlarged_dimensions() {
        let source = RgbaImage::from_pixel(2, 1, crate::image::Rgba([255, 0, 0, 255]));
        let mut asset = Asset {
            animation: None,
            started: std::time::Instant::now(),
            frame: 0,
            encoded: String::new(),
            pixels: Arc::new(source),
            scaled: None,
        };
        let mut canvas = Pixmap::new(20, 20).unwrap();
        draw_image(
            &mut canvas,
            &mut asset,
            [5., 5., 10., 10.],
            ImageScale::Pixels,
            800,
            8,
        )
        .unwrap();
        assert_eq!(asset.scaled.as_ref().unwrap().1.data().len(), 8);
        for y in 0..20 {
            for x in 0..20 {
                let p = canvas.pixel(x, y).unwrap();
                assert_eq!(
                    p.alpha(),
                    if (5..15).contains(&x) && (5..15).contains(&y) {
                        255
                    } else {
                        0
                    }
                );
            }
        }
    }

    #[test]
    fn pixel_preview_uses_exact_blocks_and_invalidates_sampling_cache() {
        let mut source = RgbaImage::new(2, 1);
        source.put_pixel(0, 0, crate::image::Rgba([255, 0, 0, 255]));
        source.put_pixel(1, 0, crate::image::Rgba([0, 0, 255, 255]));
        let mut asset = Asset {
            animation: None,
            started: std::time::Instant::now(),
            frame: 0,
            encoded: String::new(),
            pixels: Arc::new(source),
            scaled: None,
        };
        let mut canvas = Pixmap::new(9, 5).unwrap();
        draw_image(
            &mut canvas,
            &mut asset,
            [0., 0., 9., 5.],
            ImageScale::Pixels,
            100,
            1000,
        )
        .unwrap();
        let (size, pixels) = asset.scaled.as_ref().unwrap();
        assert_eq!(*size, (8, 4, ImageScale::Pixels));
        for y in 0..4 {
            for x in 0..8 {
                let pixel = pixels.pixel(x, y).unwrap();
                assert_eq!(
                    (pixel.red(), pixel.blue()),
                    if x < 4 { (255, 0) } else { (0, 255) }
                );
            }
        }
        draw_image(
            &mut canvas,
            &mut asset,
            [0., 0., 8., 4.],
            ImageScale::Smooth,
            100,
            1000,
        )
        .unwrap();
        let pixel = asset.scaled.as_ref().unwrap().1.pixel(3, 0).unwrap();
        assert!(pixel.red() > 0 && pixel.blue() > 0);
        draw_image(
            &mut canvas,
            &mut asset,
            [0., 0., 9., 5.],
            ImageScale::One,
            100,
            1000,
        )
        .unwrap();
        assert_eq!(asset.scaled.as_ref().unwrap().0, (2, 1, ImageScale::One));
    }

    #[test]
    fn pixel_layers_share_preview_cache_when_payloads_are_omitted() {
        use super::super::placement::Placement;
        use crate::native_surface::PixelRect;
        let mut scene = scene();
        for i in 0..2 {
            let rect = Rect {
                x: i * 50,
                y: 0,
                width: 50,
                height: 40,
            };
            let png = super::super::assets::encode_png(&RgbaImage::from_pixel(
                4,
                4,
                crate::image::Rgba([i as u8 * 100, 80, 60, 255]),
            ))
            .unwrap();
            scene.components.push(Component::Image {
                rect,
                id: format!("asset-{i}"),
                png: Some(png),
                scale: Default::default(),
                zoom: 100,
            });
            scene
                .placements
                .push(Placement::new(rect, PixelRect::new(i * 600, 0, 600, 800)));
        }
        let mut painter = Painter::new();
        let first = painter.render(&scene).unwrap();
        for c in &mut scene.components {
            if let Component::Image { png, .. } = c {
                *png = None;
            }
        }
        assert_eq!(first, painter.render(&scene).unwrap());
        assert_eq!(painter.assets.len(), 2);
    }

    #[test]
    fn pixel_layers_keep_exact_gutters_borders_and_modal_background() {
        use super::super::placement::Placement;
        use crate::native_surface::PixelRect;
        let mut scene = scene();
        scene.viewport.width = 1003;
        scene.viewport.height = 803;
        let left = Rect {
            x: 0,
            y: 0,
            width: 48,
            height: 30,
        };
        let right = Rect {
            x: 50,
            y: 0,
            width: 50,
            height: 30,
        };
        scene.components = vec![
            Component::Panel {
                rect: left,
                active: false,
            },
            Component::Panel {
                rect: right,
                active: true,
            },
        ];
        scene.placements = vec![
            Placement::new(left, PixelRect::new(8, 8, 489, 700)),
            Placement::new(right, PixelRect::new(505, 8, 490, 700)),
        ];
        let mut painter = Painter::new();
        let image = painter.render(&scene).unwrap();
        for x in 497..505 {
            for y in 8..708 {
                assert_eq!(image.get_pixel(x, y).0, [30, 30, 46, 255]);
            }
        }
        assert_eq!(image.get_pixel(8, 100).0, [69, 71, 90, 255]);
        assert_eq!(image.get_pixel(9, 100).0, [69, 71, 90, 255]);
        assert_eq!(image.get_pixel(10, 100).0, [69, 71, 90, 255]);
        assert_eq!(image.get_pixel(11, 100).0, [30, 30, 46, 255]);
        // A popup can cross a pane boundary without erasing the base scene or
        // using a different projection for its text and its hit region.
        let rect = Rect {
            x: 40,
            y: 5,
            width: 20,
            height: 5,
        };
        scene.components.push(Component::Menu { rect });
        scene.placements.push(Placement {
            source: rect,
            target: PixelRect::new(450, 150, 200, 100),
            padding: None,
            overlay: Some(vec![super::super::protocol::Span {
                x: 40,
                y: 5,
                text: "                    ".into(),
                foreground: "#ffffff".into(),
                background: "#123456".into(),
                bold: false,
            }]),
        });
        scene.components.push(Component::Scrollbar {
            rect: Rect {
                x: 59,
                y: 6,
                width: 1,
                height: 3,
            },
            thumb: Rect {
                x: 59,
                y: 6,
                width: 1,
                height: 1,
            },
        });
        let popup = painter.render(&scene).unwrap();
        assert_eq!(popup.get_pixel(550, 160).0, [18, 52, 86, 255]);
        assert_eq!(popup.get_pixel(645, 178).0, [137, 180, 250, 255]);
        assert_eq!(popup.get_pixel(505, 300), image.get_pixel(505, 300));
    }

    #[test]
    fn borders_are_three_pixels_and_scrollbar_has_no_cell_gaps() {
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
        assert_eq!(pixels.get_pixel(60, 42).0, [69, 71, 90, 255]);
        assert_eq!(pixels.get_pixel(60, 43).0, [30, 30, 46, 255]);
        // Every interior pixel is continuous across the three terminal rows.
        for y in 82..138 {
            assert_eq!(pixels.get_pixel(245, y).0, [137, 180, 250, 255]);
        }
        // The track outside the thumb blends into the panel.
        assert_eq!(pixels.get_pixel(245, 65).0, [30, 30, 46, 255]);
        assert_eq!(pixels.get_pixel(241, 90).0, [30, 30, 46, 255]);
        assert_eq!(pixels.get_pixel(25, 41).0, [30, 30, 46, 255]);
        painter.pane_corner_radius = 0;
        let square = painter.render(&scene).unwrap();
        assert_eq!(square.get_pixel(25, 41).0, [69, 71, 90, 255]);
        assert_eq!(square.get_pixel(60, 40), pixels.get_pixel(60, 40));
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
            scale: Default::default(),
            zoom: 100,
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
    fn mark_circles_appear_only_during_marking() {
        let mut painter = Painter::new();
        let mut scene = scene();
        for (marking, marked) in [(false, false), (true, false), (true, true), (false, true)] {
            scene.components = vec![Component::ListRow {
                rect: Rect {
                    x: 2,
                    y: 2,
                    width: 30,
                    height: 1,
                },
                label: "file".into(),
                icon: "file".into(),
                foreground: "#ffffff".into(),
                background: scene.background.clone(),
                selected: false,
                marked,
                marking,
            }];
            let pixels = painter.render(&scene).unwrap();
            let visible =
                (24..36).any(|x| (40..60).any(|y| pixels.get_pixel(x, y).0 != [30, 30, 46, 255]));
            assert_eq!(visible, marking || marked);
        }
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
            marking: true,
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
            scale: Default::default(),
            zoom: 100,
        });
        assert!(painter.render(&scene).is_err());
        scene.viewport.width = 9000;
        assert!(painter.render(&scene).is_err());
    }
}
