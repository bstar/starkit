//! Native, display-server-independent drawing. Only bounded scene data enters.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, Result};
use base64::Engine as _;
use cosmic_text::{
    Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight, Wrap,
};
use tiny_skia::{FillRule, Paint, PathBuilder, Stroke, Transform};

use super::canvas::Canvas as Pixmap;
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
    line_height: u32,
    bold: bool,
    mono: bool,
    ui_font: bool,
}
#[derive(Clone, Copy)]
struct TextStyle<'a> {
    size: f32,
    color: &'a str,
    bold: bool,
    mono: bool,
    ellipsis: bool,
    centered: bool,
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
    #[cfg(feature = "gpu-renderer")]
    pub(super) gpu: Option<super::gpu::Gpu>,
    pub(super) show_backend: bool,
    ui_font: bool,
    terminal_font: Option<f32>,
    terminal_grid: Option<f32>,
    terminal_symbols: HashSet<usize>,
    pane_corner_radius: u16,
    fonts: FontSystem,
    font_factor: f32,
    glyphs: SwashCache,
    text: HashMap<TextKey, Buffer>,
    text_bytes: usize,
    assets: HashMap<String, Asset>,
    skin_assets: crate::native_surface::skin::assets::AssetCache,
    skin_scaled: HashMap<SkinKey, Pixmap>,
    wallpapers: HashMap<WallpaperKey, Pixmap>,
    surfaces: std::collections::VecDeque<CachedSurface>,
    surface_bytes: usize,
    surface_rasters: u64,
}

// Retained chrome is independent of video cadence. The cache is deliberately
// bounded by both payload bytes and entry count, and does not change the wire.
const SURFACE_CACHE_BYTES: usize = 16_000_000;
struct CachedSurface {
    surface: super::surface::Surface,
    width: u32,
    height: u32,
    pixels: Pixmap,
    bytes: usize,
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct WallpaperKey {
    width: u32,
    height: u32,
    base: [u8; 3],
    accent: [u8; 3],
    secondary: [u8; 3],
}
#[derive(Clone, Hash, PartialEq, Eq)]
struct SkinKey {
    asset: String,
    source: crate::native_surface::PixelRect,
    insets: Option<[u16; 4]>,
    width: u16,
    height: u16,
    tint: Option<String>,
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
    if color == "transparent" {
        return;
    }
    if let Some(rect) = tiny_skia::Rect::from_xywh(rect[0], rect[1], rect[2], rect[3]) {
        let mut p = paint(color);
        p.anti_alias = false;
        canvas.fill_rect(rect, &p, Transform::identity(), None);
    }
}
// Quantize shared edges, rather than rounding origins and widths separately.
fn snap_rect([x, y, w, h]: [f32; 4]) -> [f32; 4] {
    [
        x.round(),
        y.round(),
        (x + w).round() - x.round(),
        (y + h).round() - y.round(),
    ]
}

// Terminal joining glyphs must touch their cell edges independently of the
// selected font's bearings, fallback font or rasterized line height.
fn draw_grid_glyph(canvas: &mut Pixmap, glyph: &str, rect: [f32; 4], color: &str) -> bool {
    let mut chars = glyph.chars();
    let Some(c) = chars.next() else { return false };
    if chars.next().is_some() {
        return false;
    }
    let [x, y, w, h] = rect;
    if matches!(c, '\u{e0b0}' | '\u{e0b1}' | '\u{e0b2}' | '\u{e0b3}') {
        let right = matches!(c, '\u{e0b0}' | '\u{e0b1}');
        let outline = matches!(c, '\u{e0b1}' | '\u{e0b3}');
        let (base, tip) = if right { (x, x + w) } else { (x + w, x) };
        let mut path = PathBuilder::new();
        path.move_to(base, y);
        path.line_to(tip, y + h / 2.);
        path.line_to(base, y + h);
        if !outline {
            path.close();
        }
        if let Some(path) = path.finish() {
            if outline {
                canvas.stroke_path(
                    &path,
                    &paint(color),
                    &Stroke {
                        width: 1.,
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
        return true;
    }
    let block = match c {
        '█' => Some(rect),
        '▀' => Some(snap_rect([x, y, w, h / 2.])),
        '▁'..='▇' => {
            let fraction = (u32::from(c) - u32::from('▁') + 1) as f32 / 8.;
            Some(snap_rect([x, y + h * (1. - fraction), w, h * fraction]))
        }
        '▉'..='▏' => {
            let fraction = (u32::from('▏') - u32::from(c) + 1) as f32 / 8.;
            Some(snap_rect([x, y, w * fraction, h]))
        }
        '▐' => Some(snap_rect([x + w / 2., y, w / 2., h])),
        _ => None,
    };
    if let Some(block) = block {
        fill(canvas, block, color);
        return true;
    }
    let (left, right, top, bottom) = match c {
        '─' | '━' => (true, true, false, false),
        '│' | '┃' => (false, false, true, true),
        '┌' => (false, true, false, true),
        '┐' => (true, false, false, true),
        '└' => (false, true, true, false),
        '┘' => (true, false, true, false),
        '├' => (false, true, true, true),
        '┤' => (true, false, true, true),
        '┬' => (true, true, false, true),
        '┴' => (true, true, true, false),
        '┼' => (true, true, true, true),
        _ => return false,
    };
    let thickness = if matches!(c, '━' | '┃') { 2. } else { 1. };
    let cx = (x + (w - thickness) / 2.).round();
    let cy = (y + (h - thickness) / 2.).round();
    if left || right {
        fill(
            canvas,
            [
                if left { x } else { cx },
                cy,
                (if right { x + w } else { cx + thickness }) - if left { x } else { cx },
                thickness,
            ],
            color,
        );
    }
    if top || bottom {
        fill(
            canvas,
            [
                cx,
                if top { y } else { cy },
                thickness,
                (if bottom { y + h } else { cy + thickness }) - if top { y } else { cy },
            ],
            color,
        );
    }
    true
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

fn font_database() -> cosmic_text::fontdb::Database {
    let mut db = cosmic_text::fontdb::Database::new();
    for bytes in FONTS {
        db.load_font_data(bytes.to_vec());
    }
    if std::env::var("STAR_GRAPHICS_SYSTEM_FONTS").as_deref() != Ok("0") {
        db.load_system_fonts();
    }
    db
}
pub(super) fn terminal_typography(
    font: &super::font::Font,
) -> Option<super::capabilities::TerminalTypography> {
    use cosmic_text::fontdb::{Family as DbFamily, Query};
    let mut db = font_database();
    let family = resolve_font(&db, font.name.as_deref());
    if std::env::var_os("STAR_GRAPHICS_FONT_DEBUG").is_some() {
        eprintln!("Resolved terminal font family: {family}");
    }
    db.set_monospace_family(&family);
    let id = db.query(&Query {
        families: &[DbFamily::Monospace],
        ..Query::default()
    })?;
    // FontSystem maps disk-backed faces before loading them. Font::new alone
    // accepts bundled/in-memory faces but rejects ordinary installed font files.
    let mut fonts = FontSystem::new_with_locale_and_db("en-US".into(), db);
    let face = fonts.get_font(id)?;
    let f = face.rustybuzz();
    let em = f.units_per_em() as f32;
    let advance = f32::from(f.glyph_hor_advance(f.glyph_index('M')?)?) / em;
    let line = f32::from(f.ascender() - f.descender() + f.line_gap()) / em;
    let measure = |size: f32| {
        [
            (size * advance).round().max(1.) as u16,
            (size * line).ceil().max(1.) as u16,
        ]
    };
    let pixels = font
        .pixels
        .unwrap_or_else(|| font.cell.map(|(_, h)| f32::from(h) / line).unwrap_or(16.))
        .clamp(4., 96.);
    let cell = if std::env::var_os("STAR_GRAPHICS_FONT").is_none()
        && std::env::var_os("STAR_GRAPHICS_FONT_SIZE").is_none()
    {
        font.cell
            .map(|(w, h)| [w, h])
            .unwrap_or_else(|| measure(pixels))
    } else {
        measure(pixels)
    };
    let result = super::capabilities::TerminalTypography {
        millipixels: (pixels * 1000.).round() as u32,
        cell,
        sizes: std::array::from_fn(|i| measure((i + 10) as f32)),
    };
    tracing::info!(%family,?result,"Terminal font metrics measured");
    Some(result)
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
    fn surface(
        &mut self,
        canvas: &mut Pixmap,
        surface: &super::surface::Surface,
        area: [f32; 4],
    ) -> Result<()> {
        let [x, y, width, height] = area;
        // Fractional placement retains its original antialiasing path. Native
        // physical chrome normally has exact integral destinations.
        let integral = area
            .iter()
            .all(|value| value.is_finite() && value.fract() == 0.);
        let width = width.max(0.) as u32;
        let height = height.max(0.) as u32;
        let pixels = u64::from(width) * u64::from(height) * 4;
        let cacheable = !canvas.is_gpu()
            && integral
            && width > 0
            && height > 0
            && pixels <= SURFACE_CACHE_BYTES as u64;
        if cacheable {
            if let Some(index) = self.surfaces.iter().position(|cached| {
                cached.width == width && cached.height == height && cached.surface == *surface
            }) {
                let cached = self.surfaces.remove(index).expect("cache index");
                canvas.draw_pixmap(
                    x as i32,
                    y as i32,
                    cached.pixels.as_ref(),
                    &tiny_skia::PixmapPaint::default(),
                    Transform::identity(),
                    None,
                );
                self.surfaces.push_back(cached);
                return Ok(());
            }
            // Account for the retained scene data too; encoded artwork must
            // not bypass the pixel budget by hiding in a cache key.
            let bytes = pixels as usize
                + serde_json::to_vec(surface)?.len() * 4
                + std::mem::size_of::<CachedSurface>();
            if bytes <= SURFACE_CACHE_BYTES {
                let mut raster = Pixmap::new(width, height).context("Allocate retained chrome")?;
                fill(
                    &mut raster,
                    [0., 0., width as f32, height as f32],
                    &surface.background,
                );
                let previous = std::mem::replace(&mut self.ui_font, surface.ui_font);
                let result =
                    self.surface_nodes(&mut raster, surface, [0., 0., width as f32, height as f32]);
                self.ui_font = previous;
                result?;
                self.surface_rasters += 1;
                canvas.draw_pixmap(
                    x as i32,
                    y as i32,
                    raster.as_ref(),
                    &tiny_skia::PixmapPaint::default(),
                    Transform::identity(),
                    None,
                );
                while self.surface_bytes + bytes > SURFACE_CACHE_BYTES || self.surfaces.len() >= 64
                {
                    let Some(old) = self.surfaces.pop_front() else {
                        break;
                    };
                    self.surface_bytes -= old.bytes;
                }
                self.surface_bytes += bytes;
                self.surfaces.push_back(CachedSurface {
                    surface: surface.clone(),
                    width,
                    height,
                    pixels: raster,
                    bytes,
                });
                return Ok(());
            }
        }
        fill(canvas, area, &surface.background);
        let previous = std::mem::replace(&mut self.ui_font, surface.ui_font);
        let result = self.surface_nodes(canvas, surface, area);
        self.ui_font = previous;
        self.surface_rasters += 1;
        result
    }

    fn surface_nodes(
        &mut self,
        canvas: &mut Pixmap,
        surface: &super::surface::Surface,
        area: [f32; 4],
    ) -> Result<()> {
        use super::surface::Primitive;
        for (id, png) in &surface.assets {
            if let Some(png) = png {
                let (bytes, _) = super::assets::skin_payload(id, png)?;
                self.skin_assets.insert_png(id, &bytes)?;
            }
        }
        let [x, y, width, height] = area;
        let sx = width / f32::from(surface.width);
        let sy = height / f32::from(surface.height);
        for (node_index, node) in surface.nodes.iter().enumerate() {
            let r = node.rect();
            let mut rect = [
                x + f32::from(r.x) * sx,
                y + f32::from(r.y) * sy,
                f32::from(r.width) * sx,
                f32::from(r.height) * sy,
            ];
            if surface.cell_size.is_some() {
                rect = snap_rect(rect);
            }
            match node {
                Primitive::Fill { color, radius, .. } => {
                    if surface.cell_size.is_some() && *radius == 0 {
                        fill(canvas, rect, color);
                    } else {
                        rounded(canvas, rect, f32::from(*radius), color, false)
                    }
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
                Primitive::Gradient {
                    base,
                    accent,
                    secondary,
                    ..
                } => {
                    let width = rect[2].round().max(1.) as u32;
                    let height = rect[3].round().max(1.) as u32;
                    let pixels = u64::from(width) * u64::from(height);
                    if pixels > 16_000_000 {
                        fill(canvas, rect, &crate::native_surface::style::hex(*base));
                        continue;
                    }
                    let key = WallpaperKey {
                        width,
                        height,
                        base: *base,
                        accent: *accent,
                        secondary: *secondary,
                    };
                    if !self.wallpapers.contains_key(&key) {
                        let retained: u64 = self
                            .wallpapers
                            .values()
                            .map(|p| u64::from(p.width()) * u64::from(p.height()))
                            .sum();
                        if self.wallpapers.len() >= 3 || retained + pixels > 16_000_000 {
                            self.wallpapers.clear();
                        }
                        let wallpaper = crate::native_surface::style::workspace::Wallpaper {
                            base: *base,
                            accent: *accent,
                            secondary: *secondary,
                        };
                        let mut bytes = wallpaper.bgra(width, height);
                        for pixel in bytes.as_chunks_mut::<4>().0 {
                            pixel.swap(0, 2);
                        }
                        let size = tiny_skia::IntSize::from_wh(width, height)
                            .expect("bounded wallpaper size");
                        self.wallpapers.insert(
                            key.clone(),
                            Pixmap::from_vec(bytes, size).expect("opaque wallpaper pixels"),
                        );
                    }
                    canvas.draw_pixmap(
                        rect[0].round() as i32,
                        rect[1].round() as i32,
                        self.wallpapers[&key].as_ref(),
                        &tiny_skia::PixmapPaint::default(),
                        tiny_skia::Transform::identity(),
                        None,
                    );
                }
                Primitive::Text {
                    text,
                    color,
                    size,
                    bold,
                    mono,
                    ..
                } => {
                    let style = TextStyle {
                        size: surface
                            .terminal_font_millipixels
                            .map(|n| n as f32 / 1000.)
                            .unwrap_or(f32::from(*size))
                            / if surface.text_scale == crate::native_surface::TextScale::Physical {
                                self.font_factor
                            } else {
                                1.
                            },
                        color,
                        bold: *bold,
                        mono: *mono,
                        ellipsis: surface.cell_size.is_none(),
                        centered: surface.cell_size.is_some()
                            || surface.centered_text.contains(&(node_index as u16)),
                    };
                    if let Some([cw, _]) = surface.cell_size {
                        let previous = self.terminal_font;
                        self.terminal_font =
                            surface.terminal_font_millipixels.map(|n| n as f32 / 1000.);
                        self.grid_text(
                            canvas,
                            text,
                            [
                                x + f32::from(r.x) * sx,
                                y + f32::from(r.y) * sy,
                                f32::from(r.width) * sx,
                                f32::from(r.height) * sy,
                            ],
                            f32::from(cw) * sx,
                            style,
                        );
                        self.terminal_font = previous;
                    } else {
                        let caret = surface
                            .text_carets
                            .iter()
                            .find(|caret| usize::from(caret.node) == node_index);
                        let edge = self.text_with_caret(canvas, text, rect, style, caret);
                        if let Some(edge) = edge {
                            for rule in surface
                                .text_rules
                                .iter()
                                .filter(|r| usize::from(r.node) == node_index)
                            {
                                let left = edge + f32::from(rule.gap) * sx;
                                let right = x + f32::from(rule.end) * sx;
                                let thickness = f32::from(rule.width) * sy;
                                if right - left >= thickness {
                                    fill(
                                        canvas,
                                        [
                                            left,
                                            (rect[1] + rect[3] / 2.).floor(),
                                            right - left,
                                            thickness,
                                        ],
                                        &rule.color,
                                    );
                                }
                            }
                        }
                    }
                }
                Primitive::Path {
                    points,
                    color,
                    width,
                    ..
                } => {
                    let mut path = PathBuilder::new();
                    for (i, p) in points.iter().enumerate() {
                        let px = rect[0] + f32::from(p[0]) * sx;
                        let py = rect[1] + f32::from(p[1]) * sy;
                        if i == 0 {
                            path.move_to(px, py);
                        } else {
                            path.line_to(px, py);
                        }
                    }
                    if let Some(path) = path.finish() {
                        canvas.stroke_path(
                            &path,
                            &paint(color),
                            &Stroke {
                                width: f32::from(*width) * sx.min(sy),
                                ..Stroke::default()
                            },
                            Transform::identity(),
                            None,
                        );
                    }
                }
                Primitive::Sprite {
                    asset,
                    source,
                    insets,
                    tint,
                    ..
                } => {
                    let key = SkinKey {
                        asset: asset.clone(),
                        source: *source,
                        insets: *insets,
                        width: rect[2].round().clamp(1., 8192.) as u16,
                        height: rect[3].round().clamp(1., 8192.) as u16,
                        tint: tint.clone(),
                    };
                    if !self.skin_assets.contains(asset) {
                        continue;
                    }
                    if !self.skin_scaled.contains_key(&key) {
                        use crate::native_surface::skin::{Insets, NineSlice, Repeat};
                        let mut sprite = self.skin_assets.sprite(
                            &crate::native_surface::skin::assets::Sprite {
                                asset: asset.clone(),
                                rect: *source,
                                density: 1,
                                content: Insets {
                                    left: 0,
                                    top: 0,
                                    right: 0,
                                    bottom: 0,
                                },
                                nine_slice: None,
                            },
                        )?;
                        if let Some(tint) = tint {
                            let c = rgb(tint);
                            for p in sprite.pixels_mut() {
                                p[0] = c[0];
                                p[1] = c[1];
                                p[2] = c[2];
                            }
                        }
                        anyhow::ensure!(
                            u32::from(key.width) * u32::from(key.height) <= 8_000_000,
                            "Scaled skin exceeds budget"
                        );
                        let mut pixels = RgbaImage::new(key.width.into(), key.height.into());
                        if let Some([left, top, right, bottom]) = insets {
                            NineSlice {
                                insets: Insets {
                                    left: *left,
                                    top: *top,
                                    right: *right,
                                    bottom: *bottom,
                                },
                                horizontal: Repeat::Tile,
                                vertical: Repeat::Tile,
                            }
                            .paint(
                                &sprite,
                                &mut pixels,
                                crate::native_surface::PixelRect::new(0, 0, key.width, key.height),
                            )?;
                        } else {
                            anyhow::ensure!(
                                sprite.dimensions() == pixels.dimensions(),
                                "Sprite scaling requires slice metadata"
                            );
                            pixels = sprite;
                        }
                        let mut raw = pixels.into_raw();
                        for p in raw.as_chunks_mut::<4>().0 {
                            for c in 0..3 {
                                p[c] = ((u16::from(p[c]) * u16::from(p[3]) + 127) / 255) as u8;
                            }
                        }
                        let pixmap = Pixmap::from_vec(
                            raw,
                            tiny_skia::IntSize::from_wh(key.width.into(), key.height.into())
                                .context("Sprite dimensions")?,
                        )
                        .context("Sprite pixels")?;
                        let retained: usize =
                            self.skin_scaled.values().map(|p| p.data().len()).sum();
                        if retained + pixmap.data().len() > 32_000_000
                            || self.skin_scaled.len() >= 256
                        {
                            self.skin_scaled.clear();
                        }
                        self.skin_scaled.insert(key.clone(), pixmap);
                    }
                    canvas.draw_pixmap(
                        rect[0].round() as i32,
                        rect[1].round() as i32,
                        self.skin_scaled[&key].as_ref(),
                        &tiny_skia::PixmapPaint::default(),
                        Transform::identity(),
                        None,
                    );
                }
                Primitive::Icon { name, color, .. } => {
                    draw_icon(canvas, name, rect[0], rect[1], rect[2].min(rect[3]), color)
                }
            }
        }
        Ok(())
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
        let mut db = font_database();
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
            #[cfg(feature = "gpu-renderer")]
            gpu: None,
            show_backend: false,
            ui_font: false,
            terminal_font: None,
            terminal_grid: None,
            terminal_symbols: HashSet::new(),
            pane_corner_radius: radius,
            fonts: FontSystem::new_with_locale_and_db("en-US".into(), db),
            font_factor,
            glyphs: SwashCache::new(),
            text: HashMap::new(),
            text_bytes: 0,
            assets: HashMap::new(),
            skin_assets: crate::native_surface::skin::assets::AssetCache::new(32_000_000),
            skin_scaled: HashMap::new(),
            wallpapers: HashMap::new(),
            surfaces: std::collections::VecDeque::new(),
            surface_bytes: 0,
            surface_rasters: 0,
        }
    }

    fn grid_text(
        &mut self,
        canvas: &mut Pixmap,
        value: &str,
        rect: [f32; 4],
        cw: f32,
        style: TextStyle<'_>,
    ) {
        let [x, y, w, h] = rect;
        if self.terminal_font.is_some() {
            let previous = self.terminal_grid.replace(cw);
            self.terminal_symbols.clear();
            let mut at = 0;
            let mut column = 0;
            while at < value.len() {
                let end = crate::wrap::next_boundary(value, at);
                let glyph = &value[at..end];
                let columns = crate::wrap::width_of(glyph);
                let left = x + f32::from(column) * cw;
                let right = (left + f32::from(columns) * cw).min(x + w);
                if columns > 0
                    && right > left
                    && draw_grid_glyph(
                        canvas,
                        glyph,
                        snap_rect([left, y, right - left, h]),
                        style.color,
                    )
                {
                    self.terminal_symbols.insert(at);
                }
                column += columns;
                at = end;
            }
            self.text(
                canvas,
                value,
                rect,
                TextStyle {
                    centered: false,
                    ..style
                },
            );
            self.terminal_grid = previous;
            self.terminal_symbols.clear();
            return;
        }
        let mut at = 0;
        let mut column = 0;
        while at < value.len() {
            let end = crate::wrap::next_boundary(value, at);
            let glyph = &value[at..end];
            let columns = crate::wrap::width_of(glyph);
            let left = x + f32::from(column) * cw;
            let right = (left + f32::from(columns) * cw).min(x + w);
            if columns > 0 && right > left {
                let cell = snap_rect([left, y, right - left, h]);
                if !draw_grid_glyph(canvas, glyph, cell, style.color) {
                    self.text(canvas, glyph, cell, style);
                }
            }
            column += columns;
            at = end;
        }
    }
    fn text(
        &mut self,
        canvas: &mut Pixmap,
        value: &str,
        rect: [f32; 4],
        style: TextStyle<'_>,
    ) -> Option<f32> {
        self.text_with_caret(canvas, value, rect, style, None)
    }
    fn text_with_caret(
        &mut self,
        canvas: &mut Pixmap,
        value: &str,
        rect: [f32; 4],
        style: TextStyle<'_>,
        caret: Option<&crate::native_surface::TextCaret>,
    ) -> Option<f32> {
        let TextStyle {
            size,
            color,
            bold,
            mono,
            ellipsis,
            centered,
        } = style;
        let [x, y, w, h] = rect;
        if (value.trim().is_empty() && caret.is_none()) || w <= 0. || h <= 0. {
            return None;
        }
        // Placed panel rows may be shorter than the source terminal cells.
        // Shape at a size whose complete line fits the destination, including
        // terminal font scaling; centring an oversized line clips its glyphs.
        let size = if self.terminal_grid.is_some() {
            (size * self.font_factor).clamp(1., 128.)
        } else {
            (size * self.font_factor).clamp(1., 128.).min(h / 1.2)
        };
        let line_height = if self.terminal_grid.is_some() {
            h
        } else {
            size * 1.2
        };
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
            line_height: line_height.to_bits(),
            bold,
            mono,
            ui_font: self.ui_font,
        };
        let fonts = &mut self.fonts;
        let cache_bytes = &mut self.text_bytes;
        let buffer = self.text.entry(key.clone()).or_insert_with(|| {
            *cache_bytes += key.text.len();
            let mut b = Buffer::new(fonts, Metrics::new(size, line_height));
            b.set_wrap(fonts, Wrap::None);
            b.set_text(
                fonts,
                &key.text,
                &Attrs::new()
                    .family(if key.ui_font {
                        Family::Name(if mono {
                            "Liberation Mono"
                        } else {
                            "Liberation Sans"
                        })
                    } else if mono {
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
        let clipped = caret.is_none()
            && ellipsis
            && value.chars().count() > 1
            && w >= size
            && text_width > w + 0.5;
        let caret_x = caret.map(|caret| {
            let byte = value[..caret.byte.min(value.len())]
                .chars()
                .filter(|c| !c.is_control())
                .collect::<String>()
                .len();
            for run in buffer.layout_runs() {
                for glyph in run.glyphs {
                    if glyph.start == byte {
                        return if glyph.level.is_rtl() {
                            glyph.x + glyph.w
                        } else {
                            glyph.x
                        };
                    }
                    if glyph.end == byte {
                        return if glyph.level.is_rtl() {
                            glyph.x
                        } else {
                            glyph.x + glyph.w
                        };
                    }
                }
            }
            text_width
        });
        let clip_w = if clipped { (w - size).max(0.) } else { w };
        let offset_y = (h - line_height) / 2.;
        let offset_x = if let Some(caret) = caret_x {
            -(caret - w + 3.).max(0.)
        } else if centered {
            ((w - text_width) / 2.).max(0.)
        } else {
            0.
        };
        let [r, g, b] = rgb(color);
        let width = canvas.width();
        let height = canvas.height();
        let gpu_text = canvas.is_gpu();
        let left = (x.ceil() as i32).max(0);
        let top = (y.ceil() as i32).max(0);
        let right = ((x + clip_w).floor() as i32).min(width as i32).max(left);
        let bottom = ((y + h).floor() as i32).min(height as i32).max(top);
        let mut coverage = if gpu_text {
            Pixmap::new((right - left).max(1) as u32, (bottom - top).max(1) as u32)
        } else {
            None
        };
        let coverage_width = coverage.as_ref().map_or(width, Pixmap::width);
        let pixels = if let Some(mask) = &mut coverage {
            mask.data_mut()
        } else {
            canvas.data_mut()
        };
        let mut ink_end = None::<f32>;
        let mut draw_pixel = |px: i32, py: i32, _: u32, _: u32, color: Color| {
            let dx = (x + offset_x).round() as i32 + px;
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
            let offset = if gpu_text {
                (((dy - top) as u32 * coverage_width + (dx - left) as u32) * 4) as usize
            } else {
                ((dy as u32 * width + dx as u32) * 4) as usize
            };
            let rgba = color.as_rgba();
            if rgba[3] > 0 {
                ink_end = Some(ink_end.unwrap_or(0.).max((dx + 1) as f32));
            }
            // The canvas stores premultiplied RGBA. Opaque pane text can
            // blend directly, but text over terminal gaps must also add
            // coverage to alpha or Kitty treats the glyph as invisible.
            let dst_alpha = u32::from(pixels[offset + 3]);
            let src_alpha = u32::from(rgba[3]);
            if dst_alpha != 255 {
                let alpha = src_alpha + (dst_alpha * (255 - src_alpha) + 127) / 255;
                if alpha == 0 {
                    return;
                }
                let coverage = ((src_alpha * 255 + alpha / 2) / alpha) as u8;
                for c in 0..3 {
                    let background = (u32::from(pixels[offset + c]) * 255 + dst_alpha / 2)
                        .checked_div(dst_alpha)
                        .unwrap_or(0)
                        .min(255) as u8;
                    let straight = blend_text_channel(rgba[c], background, coverage);
                    pixels[offset + c] = ((u32::from(straight) * alpha + 127) / 255) as u8;
                }
                pixels[offset + 3] = alpha as u8;
                return;
            }
            for c in 0..3 {
                pixels[offset + c] = blend_text_channel(rgba[c], pixels[offset + c], rgba[3]);
            }
        };
        if let Some(cw) = self.terminal_grid {
            // Preserve shaped glyphs/ligatures, but anchor each cluster to its
            // terminal column instead of centering each grapheme separately.
            let mut columns = vec![0u16; key.text.len() + 1];
            let mut at = 0;
            let mut col = 0;
            while at < key.text.len() {
                let end = crate::wrap::next_boundary(&key.text, at);
                columns[at..end].fill(col);
                col += crate::wrap::width_of(&key.text[at..end]);
                columns[end] = col;
                at = end;
            }
            for run in buffer.layout_runs() {
                let mut origins = HashMap::new();
                for glyph in run.glyphs {
                    origins
                        .entry((glyph.start, glyph.end))
                        .and_modify(|x: &mut f32| *x = x.min(glyph.x))
                        .or_insert(glyph.x);
                }
                for glyph in run.glyphs {
                    if self.terminal_symbols.contains(&glyph.start) {
                        continue;
                    }
                    let mut placed = glyph.clone();
                    placed.x = f32::from(columns[glyph.start.min(key.text.len())]) * cw + glyph.x
                        - origins[&(glyph.start, glyph.end)];
                    let physical = placed.physical((0., 0.), 1.);
                    self.glyphs.with_pixels(
                        fonts,
                        physical.cache_key,
                        glyph.color_opt.unwrap_or(Color::rgb(r, g, b)),
                        |px, py, color| {
                            draw_pixel(
                                physical.x + px,
                                run.line_y as i32 + physical.y + py,
                                1,
                                1,
                                color,
                            )
                        },
                    );
                }
            }
        } else {
            buffer.draw(
                fonts,
                &mut self.glyphs,
                Color::rgb(r, g, b),
                &mut draw_pixel,
            );
        }
        if let Some(mask) = coverage {
            canvas.draw_pixmap(
                left,
                top,
                mask.as_ref(),
                &tiny_skia::PixmapPaint::default(),
                Transform::identity(),
                None,
            );
        }
        if let (Some(caret), Some(position)) = (caret, caret_x) {
            fill(
                canvas,
                [
                    (x + offset_x + position).clamp(x, x + (w - 1.).max(0.)),
                    y + (h * 0.15).floor(),
                    1.,
                    (h * 0.7).max(1.),
                ],
                &caret.color,
            );
        }
        if clipped {
            let ellipsis_end = self.text(
                canvas,
                "…",
                [x + w - size, y, size, h],
                TextStyle {
                    size: size / self.font_factor,
                    color,
                    bold,
                    mono,
                    ellipsis: false,
                    centered: false,
                },
            );
            if let Some(edge) = ellipsis_end {
                ink_end = Some(ink_end.unwrap_or(0.).max(edge));
            }
        }
        ink_end
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
                                    centered: false,
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

    pub fn overlay(
        &mut self,
        base: &RgbaImage,
        surface: &super::surface::Surface,
    ) -> Result<RgbaImage> {
        surface.validate()?;
        anyhow::ensure!(
            base.width() == u32::from(surface.width) && base.height() == u32::from(surface.height),
            "Overlay dimensions must match surface"
        );
        let mut pixels = base.as_raw().clone();
        for p in pixels.as_chunks_mut::<4>().0 {
            for c in 0..3 {
                p[c] = ((u16::from(p[c]) * u16::from(p[3]) + 127) / 255) as u8;
            }
        }
        let size = tiny_skia::IntSize::from_wh(base.width(), base.height())
            .context("Overlay dimensions")?;
        let mut canvas = Pixmap::from_vec(pixels, size).context("Overlay pixels")?;
        self.surface_nodes(
            &mut canvas,
            surface,
            [0., 0., base.width() as f32, base.height() as f32],
        )?;
        let mut pixels = canvas.take();
        for p in pixels.as_chunks_mut::<4>().0 {
            if p[3] != 0 {
                for c in 0..3 {
                    p[c] = ((u32::from(p[c]) * 255 + u32::from(p[3]) / 2) / u32::from(p[3]))
                        .min(255) as u8;
                }
            }
        }
        RgbaImage::from_raw(base.width(), base.height(), pixels).context("Overlay output")
    }

    pub fn render(&mut self, scene: &Scene) -> Result<RgbaImage> {
        let v = scene.viewport.validate()?;
        anyhow::ensure!(
            scene.spans.len() <= super::protocol::MAX_CELLS
                && scene.components.len() <= super::protocol::MAX_CELLS,
            "Scene exceeds limits"
        );
        if scene.placements.is_empty() {
            return self.render_grid(scene, None, true, None, false);
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
        let alpha = if scene.background == "transparent" {
            0
        } else {
            255
        };
        let mut frame =
            RgbaImage::from_pixel(v.width, v.height, crate::image::Rgba([r, g, b, alpha]));
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
                false,
            )?;
            crate::image::imageops::overlay(
                &mut frame,
                &layer,
                i64::from(placement.target.x),
                i64::from(placement.target.y),
            );
        }
        Ok(frame)
    }

    /// Punch live-video holes before compositing foreground artwork. A flattened
    /// UI raster cannot supply overlay alpha: its transparent portions already
    /// contain the opaque workspace beneath the video.
    pub(crate) fn render_with_canvases(
        &mut self,
        scene: &Scene,
        active: &[String],
    ) -> Result<RgbaImage> {
        let mut frame = self.render(scene)?;
        for (index, component) in scene.components.iter().enumerate() {
            let Component::LiveCanvas { id, view, .. } = component else {
                continue;
            };
            if !active.contains(id) {
                continue;
            }
            let [x, y, width, height] = view.fitted().map(|n| n.round() as u32);
            for yy in y..y.saturating_add(height).min(frame.height()) {
                for xx in x..x.saturating_add(width).min(frame.width()) {
                    frame.put_pixel(xx, yy, crate::image::Rgba([0; 4]));
                }
            }
            let v = scene.viewport;
            if scene.placements.is_empty() {
                let mut foreground = scene.clone();
                foreground.components = scene.components[index + 1..].to_vec();
                let layer = self.render_grid(&foreground, None, false, None, true)?;
                for yy in y..y.saturating_add(height).min(frame.height()) {
                    for xx in x..x.saturating_add(width).min(frame.width()) {
                        frame.put_pixel(xx, yy, *layer.get_pixel(xx, yy));
                    }
                }
                continue;
            }
            let font = ((v.height as f32 / f32::from(v.rows) * 0.84)
                .min(v.width as f32 / f32::from(v.columns) / 0.6))
            .floor()
            .clamp(1., 64.);
            for placement in &scene.placements {
                let foreground = placement.project_after(scene, index + 1);
                if foreground.components.is_empty() && foreground.spans.is_empty() {
                    continue;
                }
                let layer =
                    self.render_grid(&foreground, Some(font), false, Some(placement), true)?;
                // Only replace pixels inside this canvas. Foreground outside
                // the hole was already composited over its workspace by render.
                let left = x.max(u32::from(placement.target.x));
                let top = y.max(u32::from(placement.target.y));
                let right = x
                    .saturating_add(width)
                    .min(u32::from(placement.target.x) + layer.width())
                    .min(frame.width());
                let bottom = y
                    .saturating_add(height)
                    .min(u32::from(placement.target.y) + layer.height())
                    .min(frame.height());
                for yy in top..bottom {
                    for xx in left..right {
                        use crate::image::Pixel;
                        let pixel = layer.get_pixel(
                            xx - u32::from(placement.target.x),
                            yy - u32::from(placement.target.y),
                        );
                        frame.get_pixel_mut(xx, yy).blend(pixel);
                    }
                }
            }
        }
        Ok(frame)
    }

    fn render_grid(
        &mut self,
        scene: &Scene,
        font_override: Option<f32>,
        manage_assets: bool,
        placement: Option<&super::placement::Placement>,
        transparent_background: bool,
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
        #[cfg(feature = "gpu-renderer")]
        let mut canvas = if let Some(gpu) = &self.gpu {
            Pixmap::Gpu(Box::new(gpu.canvas(viewport.width, viewport.height)))
        } else {
            Pixmap::new(viewport.width, viewport.height).context("Allocate graphical frame")?
        };
        #[cfg(not(feature = "gpu-renderer"))]
        let mut canvas =
            Pixmap::new(viewport.width, viewport.height).context("Allocate graphical frame")?;
        let [r, g, b] = rgb(&scene.background);
        if !transparent_background && scene.background != "transparent" {
            canvas.fill(tiny_skia::Color::from_rgba8(r, g, b, 255));
        }
        if !transparent_background {
            self.spans(&mut canvas, scene, [cw, ch, font], None, placement);
        }
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
                Component::LiveCanvas { rect, .. }
                | Component::Surface { rect, .. }
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
                    self.surface(&mut canvas, surface, area)?;
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
                            centered: false,
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
                                centered: false,
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
                            centered: false,
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
                            centered: false,
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
                Component::Terminal { .. } => {
                    if let Some(rect) =
                        tiny_skia::Rect::from_xywh(area[0], area[1], area[2], area[3])
                    {
                        let paint = tiny_skia::Paint {
                            blend_mode: tiny_skia::BlendMode::Clear,
                            ..Default::default()
                        };
                        canvas.fill_rect(rect, &paint, Transform::identity(), None);
                    }
                }
                Component::LiveCanvas { .. } => {}
            }
        }
        if manage_assets {
            self.assets.retain(|id, _| used.contains(id));
        }
        if self.show_backend && viewport.width >= 64 && viewport.height >= 32 {
            #[cfg(feature = "gpu-renderer")]
            let accelerated = self.gpu.is_some();
            #[cfg(not(feature = "gpu-renderer"))]
            let accelerated = false;
            let x = viewport.width as f32 - 44.;
            let y = viewport.height as f32 - 18.;
            rounded(&mut canvas, [x, y, 42., 16.], 3., "#20242b", false);
            fill(
                &mut canvas,
                [x + 4., y + 6., 4., 4.],
                if accelerated { "#7bd88f" } else { "#8fa8c7" },
            );
            let factor = std::mem::replace(&mut self.font_factor, 1.);
            let grid = self.terminal_grid.take();
            let ui = std::mem::replace(&mut self.ui_font, true);
            self.text(
                &mut canvas,
                if accelerated { "GPU" } else { "CPU" },
                [x + 11., y, 29., 16.],
                TextStyle {
                    size: 10.,
                    color: "#d8dee9",
                    bold: false,
                    mono: true,
                    ellipsis: false,
                    centered: false,
                },
            );
            self.font_factor = factor;
            self.terminal_grid = grid;
            self.ui_font = ui;
        }
        let mut pixels = match canvas {
            Pixmap::Cpu(pixels) => pixels.take(),
            #[cfg(feature = "gpu-renderer")]
            Pixmap::Gpu(scene) => self
                .gpu
                .as_mut()
                .context("GPU rasterizer is unavailable")?
                .render(*scene)?,
        };
        if scene.background == "transparent" {
            // Kitty expects straight RGBA. Unpremultiply antialiased edges so
            // text and rounded corners don't develop dark halos over the terminal.
            for p in pixels.as_chunks_mut::<4>().0 {
                if p[3] != 0 {
                    for c in 0..3 {
                        p[c] = ((u32::from(p[c]) * 255 + u32::from(p[3]) / 2) / u32::from(p[3]))
                            .min(255) as u8;
                    }
                }
            }
        }
        RgbaImage::from_raw(viewport.width, viewport.height, pixels).context("Native frame pixels")
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
    #[cfg(feature = "gpu-renderer")]
    if let Pixmap::Gpu(target) = canvas {
        let transform = Transform::from_scale(ratio, ratio).post_translate(
            (x + (w - image.width() as f32 * ratio) / 2.).round(),
            (y + (h - image.height() as f32 * ratio) / 2.).round(),
        );
        target.rgba(image.as_ref(), transform, area, scale != ImageScale::Smooth);
        return Ok(());
    }
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
            "next" => vec![
                vec![(6., 6.), (14., 12.), (6., 18.)],
                vec![(16., 6.), (18., 6.), (18., 18.), (16., 18.)],
            ],
            "previous" => vec![
                vec![(18., 6.), (10., 12.), (18., 18.)],
                vec![(6., 6.), (8., 6.), (8., 18.), (6., 18.)],
            ],
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
    fn heading_rules_keep_a_fixed_gap_after_shaped_and_ellipsized_text() {
        use crate::native_surface::{PixelRect, Surface, TextRule};
        let mut painter = Painter::new();
        let long = "Album ".repeat(40);
        for density in [1, 2] {
            for size in [10, 12, 16] {
                for title in [
                    "1980 · Heaven And Hell",
                    "1980 · Blizzard Of Ozz [AudioPhil edition]",
                    "1997 · Björk — Homogenic",
                    "東京 · アルバム",
                    &long,
                ] {
                    let mut s = Surface::new(660, 30, "#000000".into());
                    s.text(PixelRect::new(10, 0, 610, 30), title, "#ffffff", size, true);
                    s.text_rules.push(TextRule {
                        node: 0,
                        end: 650,
                        gap: 10,
                        width: 1,
                        color: "#00ff00".into(),
                    });
                    let s = s.at_density(density).unwrap();
                    let mut canvas = Pixmap::new(u32::from(s.width), u32::from(s.height)).unwrap();
                    painter
                        .surface(
                            &mut canvas,
                            &s,
                            [0., 0., f32::from(s.width), f32::from(s.height)],
                        )
                        .unwrap();
                    let mut text_right = None;
                    let mut rule_left = None;
                    for y in 0..canvas.height() {
                        for x in 0..canvas.width() {
                            let pixel = canvas.pixel(x, y).unwrap();
                            if pixel.red() > 0
                                && pixel.red() == pixel.green()
                                && pixel.red() == pixel.blue()
                            {
                                text_right = Some(text_right.unwrap_or(0).max(x));
                            }
                            if pixel.green() > 0 && pixel.red() == 0 && pixel.blue() == 0 {
                                rule_left = Some(rule_left.unwrap_or(u32::MAX).min(x));
                            }
                        }
                    }
                    assert_eq!(
                        rule_left.unwrap() - text_right.unwrap() - 1,
                        u32::from(10 * density),
                        "{title}, size {size}, density {density}"
                    );
                }
            }
        }
    }
    #[test]
    fn skin_sprites_keep_corners_and_render_from_cached_ids_without_resampling() {
        use crate::native_surface::{PixelRect, Primitive, Surface};
        let source = RgbaImage::from_fn(3, 3, |x, y| {
            crate::image::Rgba([x as u8 * 100, y as u8 * 100, 50, 255])
        });
        let png = super::super::assets::encode_png(&source).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&png)
            .unwrap();
        let id = super::super::assets::skin_id(&bytes);
        let mut surface = Surface::new(20, 14, "#ffffff".into());
        surface.assets.insert(id.clone(), Some(png.clone()));
        surface.nodes.push(Primitive::Sprite {
            rect: PixelRect::new(2, 2, 16, 10),
            asset: id.clone(),
            source: PixelRect::new(0, 0, 3, 3),
            insets: Some([1, 1, 1, 1]),
            tint: None,
        });
        surface.validate().unwrap();
        let mut painter = Painter::new();
        let mut canvas = Pixmap::new(20, 14).unwrap();
        painter
            .surface(&mut canvas, &surface, [0., 0., 20., 14.])
            .unwrap();
        assert_eq!(canvas.pixel(2, 2).unwrap().red(), 0);
        assert_eq!(canvas.pixel(17, 2).unwrap().red(), 200);
        assert_eq!(canvas.pixel(17, 11).unwrap().green(), 200);
        assert_eq!(canvas.pixel(8, 6).unwrap().red(), 100);
        assert_eq!(canvas.pixel(0, 0).unwrap().red(), 255);
        let first = canvas.data().to_vec();
        surface.assets.insert(id.clone(), None);
        painter
            .surface(&mut canvas, &surface, [0., 0., 20., 14.])
            .unwrap();
        assert_eq!(canvas.data(), first.as_slice());
        assert_eq!(painter.skin_scaled.len(), 1);
        // A new painter waits for the reliable asset instead of crashing.
        let mut missing = Painter::new();
        missing
            .surface(&mut canvas, &surface, [0., 0., 20., 14.])
            .unwrap();
        assert_eq!(canvas.pixel(8, 6).unwrap().red(), 255);
        surface.assets.insert(id.clone(), Some(png));
        if let Primitive::Sprite { source, .. } = &mut surface.nodes[0] {
            source.x = 3;
        }
        assert!(painter
            .surface(&mut canvas, &surface, [0., 0., 20., 14.])
            .is_err());
        surface.assets.insert(
            id,
            Some(super::super::assets::encode_png(&RgbaImage::new(3, 3)).unwrap()),
        );
        assert!(painter
            .surface(&mut canvas, &surface, [0., 0., 20., 14.])
            .is_err());
    }
    #[test]
    fn editor_cell_edges_join_at_fractional_placements() {
        use crate::native_surface::{PixelRect, Surface};
        for width in [79., 80., 83., 127.] {
            let mut surface = Surface::new(80, 36, "#000000".into());
            surface.cell_size = Some([8, 18]);
            surface.fill(PixelRect::new(0, 0, 24, 36), "#112233", 0);
            surface.fill(PixelRect::new(24, 0, 56, 36), "#445566", 0);
            let mut painter = Painter::new();
            let mut canvas = Pixmap::new(140, 40).unwrap();
            painter
                .surface(&mut canvas, &surface, [0.3, 0.4, width, 36.])
                .unwrap();
            let boundary = (0.3 + 24. * width / 80.).round() as u32;
            for y in 0..36 {
                for x in 0..width.round() as u32 {
                    let pixel = canvas.pixel(x, y).unwrap();
                    assert_eq!(pixel.alpha(), 255, "unpainted cell edge at {x},{y}");
                    assert_eq!(
                        [pixel.red(), pixel.green(), pixel.blue()],
                        if x < boundary {
                            [0x11, 0x22, 0x33]
                        } else {
                            [0x44, 0x55, 0x66]
                        }
                    );
                }
            }
        }
    }

    #[test]
    fn editor_connecting_glyphs_touch_cell_edges() {
        let mut canvas = Pixmap::new(24, 36).unwrap();
        for y in [0., 18.] {
            assert!(draw_grid_glyph(
                &mut canvas,
                "│",
                [0., y, 8., 18.],
                "#ffffff"
            ));
        }
        for y in 0..36 {
            assert_eq!(canvas.pixel(4, y).unwrap().alpha(), 255);
        }
        assert!(draw_grid_glyph(
            &mut canvas,
            "\u{e0b0}",
            [8., 0., 8., 18.],
            "#ffffff"
        ));
        for y in 0..18 {
            assert!(canvas.pixel(8, y).unwrap().alpha() > 0);
        }
        assert!(draw_grid_glyph(
            &mut canvas,
            "█",
            [16., 0., 8., 18.],
            "#ffffff"
        ));
        for y in 0..18 {
            for x in 16..24 {
                assert_eq!(canvas.pixel(x, y).unwrap().alpha(), 255);
            }
        }
    }

    #[test]
    fn editor_grid_positions_ignore_font_advances_and_keep_combining_marks() {
        let mut actual = Pixmap::new(96, 18).unwrap();
        let mut expected = Pixmap::new(96, 18).unwrap();
        let mut painter = Painter::new();
        let style = TextStyle {
            size: 13.,
            color: "#ffffff",
            bold: false,
            mono: true,
            ellipsis: false,
            centered: true,
        };
        painter.grid_text(&mut actual, "a\u{301}    z", [0., 0., 96., 18.], 8., style);
        painter.text(&mut expected, "a\u{301}", [0., 0., 8., 18.], style);
        painter.text(&mut expected, "z", [40., 0., 8., 18.], style);
        assert_eq!(actual.data(), expected.data());
    }

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
    fn transparent_native_canvas_preserves_terminal_gaps_and_straight_edges() {
        use crate::native_surface::{PixelRect, Surface};
        let v = super::super::protocol::Viewport {
            width: 120,
            height: 80,
            columns: 12,
            rows: 4,
            generation: 0,
        };
        let mut scene = Scene::from_buffer(
            &crate::ratatui::buffer::Buffer::empty(crate::ratatui::layout::Rect::new(0, 0, 12, 4)),
            v,
            1,
        );
        scene.background = "transparent".into();
        scene.spans.clear();
        let mut surface = Surface::new(120, 80, "transparent".into());
        surface.fill(PixelRect::new(10, 10, 40, 40), "#ffffff", 8);
        crate::native_surface::classic::label(
            &mut surface,
            PixelRect::new(60, 10, 50, 30),
            "Theme",
            "#ffffff",
            12,
            false,
        );
        surface.validate().unwrap();
        scene.components.push(Component::Surface {
            rect: super::super::protocol::Rect {
                x: 0,
                y: 0,
                width: 12,
                height: 4,
            },
            surface,
        });
        let image = Painter::new().render(&scene).unwrap();
        assert_eq!(image.get_pixel(0, 0).0, [0, 0, 0, 0]);
        assert_eq!(image.get_pixel(20, 20).0, [255, 255, 255, 255]);
        assert!(
            (10..40).any(|y| (60..110).any(|x| image.get_pixel(x, y)[3] > 0)),
            "text on a transparent footer must contribute visible alpha"
        );
        for y in 10..40 {
            for x in 60..110 {
                let pixel = image.get_pixel(x, y);
                if pixel[3] > 0 {
                    assert_eq!(&pixel.0[..3], &[255, 255, 255]);
                }
            }
        }
        assert!(
            image.pixels().any(|p| p[3] > 0
                && p[3] < 255
                && p[0] == 255
                && p[1] == 255
                && p[2] == 255),
            "rounded edges stay white at partial alpha"
        );
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
                centered: false,
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
                    centered: false,
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
                    centered: false,
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
                centered: false,
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
    fn ui_typography_is_opt_in_cached_separately_and_independent_of_terminal_face() {
        use crate::native_surface::{PixelRect, Surface};
        let mut rendered = vec![];
        for (family, size) in [("Liberation Mono", 12.), ("Liberation Sans", 48.)] {
            let mut painter = Painter::with_font(super::super::font::Font {
                name: Some(family.into()),
                pixels: Some(size),
                cell: Some((9, 20)),
            });
            let mut surface = Surface::new(240, 40, "#000000".into()).with_ui_text();
            surface.text(
                PixelRect::new(8, 0, 224, 40),
                "Inspector · 1080p",
                "#ffffff",
                13,
                false,
            );
            let mut canvas = Pixmap::new(240, 40).unwrap();
            painter
                .surface(&mut canvas, &surface, [0., 0., 240., 40.])
                .unwrap();
            rendered.push(canvas.data().to_vec());
            assert!(painter.text.keys().all(|k| k.ui_font));
            assert!(!painter.ui_font);
            surface.ui_font = false;
            painter
                .surface(&mut canvas, &surface, [0., 0., 240., 40.])
                .unwrap();
            assert!(painter.text.keys().any(|k| !k.ui_font));
        }
        assert_eq!(rendered[0], rendered[1]);
    }
    #[test]
    fn physical_surface_text_does_not_inflate_with_terminal_font_zoom() {
        use crate::native_surface::{PixelRect, Surface};
        for font_pixels in [12., 24., 48.] {
            let mut painter = Painter::with_font(super::super::font::Font {
                name: Some("Liberation Mono".into()),
                pixels: Some(font_pixels),
                cell: Some((9, 20)),
            });
            let mut surface = Surface::new(200, 40, "#000000".into()).with_physical_text();
            surface.text(
                PixelRect::new(0, 0, 200, 40),
                "Physical label",
                "#ffffff",
                12,
                false,
            );
            let mut canvas = Pixmap::new(200, 40).unwrap();
            painter
                .surface_nodes(&mut canvas, &surface, [0., 0., 200., 40.])
                .unwrap();
            assert!(painter
                .text
                .keys()
                .all(|key| (f32::from_bits(key.size) - 12.).abs() < 0.001));
        }
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
                    centered: false,
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
                modifiers: 0,
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
            modifiers: 0,
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
    #[cfg(feature = "gpu-renderer")]
    #[test]
    #[ignore = "requires a hardware GPU adapter"]
    fn gpu_labels_survive_live_canvas_redraws() {
        use crate::native_surface::{PixelRect, Surface};
        let mut painter = Painter::new();
        painter.gpu = Some(super::super::gpu::Gpu::new().unwrap());
        let mut scene = scene();
        scene.viewport.width = 1460;
        scene.viewport.height = 1640;
        scene.viewport.columns = 146;
        scene.viewport.rows = 82;
        let mut surface = Surface::new(1460, 1640, "#101620".into()).with_physical_text();
        for (y, label) in [
            (12, "STAR / MUX"),
            (82, "SESSIONS"),
            (122, "Local shell"),
            (202, "Demo agent"),
        ] {
            surface.text(PixelRect::new(20, y, 180, 28), label, "#e1e8f3", 16, true);
        }
        scene.components.push(Component::Surface {
            rect: Rect {
                x: 0,
                y: 0,
                width: 146,
                height: 82,
            },
            surface,
        });
        scene.components.push(Component::LiveCanvas {
            rect: Rect {
                x: 20,
                y: 5,
                width: 124,
                height: 75,
            },
            id: "terminals".into(),
            view: crate::content_view::ContentView::new(
                [1200, 700],
                PixelRect::new(200, 100, 1240, 1500),
            )
            .unwrap(),
            interactive: true,
        });
        let expected = painter
            .render_with_canvases(&scene, &["terminals".into()])
            .unwrap();
        let ink = |image: &RgbaImage| {
            (0..250)
                .flat_map(|y| (0..190).map(move |x| (x, y)))
                .filter(|&(x, y)| image.get_pixel(x, y)[0] > 100)
                .count()
        };
        assert!(ink(&expected) > 100, "initial labels missing");
        for index in 0..120 {
            // A vector-only pass must not discard cached text masks either.
            let mut vectors = scene.clone();
            vectors.components.clear();
            painter.render(&vectors).unwrap();
            let actual = painter
                .render_with_canvases(&scene, &["terminals".into()])
                .unwrap();
            assert_eq!(
                ink(&actual),
                ink(&expected),
                "labels vanished at redraw {index}"
            );
        }
    }

    #[test]
    fn unchanged_surface_reuses_raster_and_cache_is_bounded() {
        let mut painter = Painter::new();
        let mut canvas = Pixmap::new(300, 200).unwrap();
        let style = crate::native_surface::style::Style::default().with_native_skins(false);
        let surface = style.panel(300, 200, "Retained chrome");
        painter
            .surface(&mut canvas, &surface, [0., 0., 300., 200.])
            .unwrap();
        let before = canvas.data().to_vec();
        assert_eq!(painter.surface_rasters, 1);
        painter
            .surface(&mut canvas, &surface, [0., 0., 300., 200.])
            .unwrap();
        assert_eq!(painter.surface_rasters, 1);
        assert_eq!(canvas.data(), before);
        for index in 0..100 {
            let surface = style.panel(300, 200, &format!("Panel {index}"));
            painter
                .surface(&mut canvas, &surface, [0., 0., 300., 200.])
                .unwrap();
        }
        assert!(painter.surface_bytes <= SURFACE_CACHE_BYTES);
        assert!(painter.surfaces.len() <= 64);
    }
    #[test]
    fn shaped_caret_renders_empty_unicode_and_scrolled_fields() {
        use crate::native_surface::{PixelRect, Surface, TextCaret};
        let mut painter = Painter::new();
        for text in [
            String::new(),
            "日本語".into(),
            "Long field with Unicode 日本語 ".repeat(50),
        ] {
            let mut surface = Surface::new(160, 30, "#101010".into()).with_ui_text();
            surface.text(PixelRect::new(4, 0, 152, 30), &text, "#ffffff", 12, false);
            surface.text_carets.push(TextCaret {
                node: 0,
                byte: text.len(),
                color: "#00ff00".into(),
            });
            surface.validate().unwrap();
            let mut canvas = Pixmap::new(160, 30).unwrap();
            painter
                .surface(&mut canvas, &surface, [0., 0., 160., 30.])
                .unwrap();
            assert!(canvas
                .data()
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[0] == 0 && p[1] == 255 && p[2] == 0));
        }
    }
}

#[cfg(test)]
#[path = "typography_tests.rs"]
mod typography_tests;
