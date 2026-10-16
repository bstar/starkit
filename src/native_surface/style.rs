//! Default STAR native visual language, extracted from AMP's Classic Rack.
//! Applications own content and actions; KIT owns palette roles and chrome.
use super::{
    classic::{self, Colors},
    HitRegion, HoverFill, PixelRect as R, Primitive, Surface,
};
use serde::{Deserialize, Serialize};
pub mod workspace;
pub use workspace::WorkspaceColors;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct Palette {
    pub bg: [u8; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inset_bg: Option<[u8; 3]>,
    pub fg: [u8; 3],
    pub muted: [u8; 3],
    pub accent: [u8; 3],
    pub selected: [u8; 3],
    pub border: [u8; 3],
    pub error: [u8; 3],
}

pub fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}
/// The original study keeps its artwork colors; theme mode uses the selected
/// palette directly so Classic's surface hues do not bleed into other themes.
pub const REFERENCE_PALETTE: Palette = Palette {
    bg: [23, 24, 32],
    inset_bg: None,
    fg: [215, 217, 207],
    muted: [152, 158, 172],
    accent: [164, 215, 145],
    selected: [48, 63, 61],
    border: [116, 121, 134],
    error: [224, 152, 133],
};
/// Named AMP treatment, recognized by its complete palette so embedded hosts
/// can use the same artwork without changing the surface protocol.
pub const SILVER_PALETTE: Palette = Palette {
    bg: [40, 40, 59],
    inset_bg: Some([6, 7, 11]),
    fg: [207, 216, 225],
    muted: [154, 161, 183],
    accent: [164, 215, 145],
    selected: [49, 49, 73],
    border: [110, 113, 137],
    error: [239, 73, 50],
};
pub fn tone(p: &Palette, original: &str, ink: bool) -> String {
    if *p != REFERENCE_PALETTE {
        return hex(if ink { p.fg } else { p.bg });
    }
    calibrated(
        original,
        &hex(if ink {
            REFERENCE_PALETTE.fg
        } else {
            REFERENCE_PALETTE.bg
        }),
        &hex(if ink { p.fg } else { p.bg }),
    )
}
pub fn generic(p: &Palette) -> Colors {
    Colors::new(p.bg, p.fg, p.muted, p.accent, p.border)
}
pub fn calibrated(original: &str, reference: &str, actual: &str) -> String {
    use crate::theme::color::Rgb;
    let a = Rgb::parse_hex(original).expect("skin source color");
    let b = Rgb::parse_hex(reference).expect("reference theme color");
    let c = Rgb::parse_hex(actual).expect("resolved theme color");
    let channel = |a, b, c| (i16::from(a) + i16::from(c) - i16::from(b)).clamp(0, 255) as u8;
    Rgb::new(
        channel(a.r, b.r, c.r),
        channel(a.g, b.g, c.g),
        channel(a.b, b.b, c.b),
    )
    .to_hex()
}
pub fn colors(p: &Palette) -> Colors {
    if *p == SILVER_PALETTE {
        return Colors {
            panel: "#28283b".into(),
            inset: "#06070b".into(),
            ink: hex(p.fg),
            dim: hex(p.muted),
            accent: hex(p.accent),
            highlight: "#7e8299".into(),
            shadow: "#10101b".into(),
            raised: "#b7c6d0".into(),
            title: "#222235".into(),
        };
    }
    if *p != REFERENCE_PALETTE {
        use crate::theme::color::Rgb;
        let bg = Rgb::new(p.bg[0], p.bg[1], p.bg[2]);
        let fg = Rgb::new(p.fg[0], p.fg[1], p.fg[2]);
        return Colors {
            panel: bg.to_hex(),
            inset: p
                .inset_bg
                .map(hex)
                .unwrap_or_else(|| bg.mix(Rgb::new(0, 0, 0), 0.06).to_hex()),
            ink: fg.to_hex(),
            dim: hex(p.muted),
            accent: hex(p.accent),
            highlight: bg.mix(fg, 0.16).to_hex(),
            shadow: bg.mix(Rgb::new(0, 0, 0), 0.25).to_hex(),
            raised: bg.mix(fg, 0.09).to_hex(),
            title: bg.mix(fg, 0.04).to_hex(),
        };
    }
    let a = generic(p);
    let r = generic(&REFERENCE_PALETTE);
    Colors {
        panel: calibrated("#303340", &r.panel, &a.panel),
        inset: if *p == REFERENCE_PALETTE {
            "#101813".into()
        } else {
            a.inset.clone()
        },
        ink: hex(p.fg),
        dim: hex(p.muted),
        accent: hex(p.accent),
        highlight: calibrated("#7c8294", &r.highlight, &a.highlight),
        shadow: calibrated("#101117", &r.shadow, &a.shadow),
        raised: calibrated("#474d5d", &r.raised, &a.raised),
        title: calibrated("#242733", &r.title, &a.title),
    }
}
pub fn inactive_led(p: &Palette) -> String {
    if *p == SILVER_PALETTE {
        return "#1c2a20".into();
    }
    if *p == REFERENCE_PALETTE {
        return "#1c2a20".into();
    }
    // Unlit segments follow the background hue, not Classic's green phosphor.
    use crate::theme::color::Rgb;
    Rgb::parse_hex(&colors(p).inset)
        .unwrap()
        .mix(Rgb::new(p.fg[0], p.fg[1], p.fg[2]), 0.06)
        .to_hex()
}

/// Compact artwork inside the existing pointer target. Small fold controls
/// keep their size; ordinary buttons use the selected 22px Minimal treatment.
pub fn button_rect(rect: R) -> R {
    let height = rect.height.min(22);
    let width = if rect.width == rect.height {
        rect.width.min(22)
    } else {
        (rect.width * 4 / 5).max(height).min(rect.width)
    };
    R::new(
        rect.x + (rect.width - width) / 2,
        rect.y + (rect.height - height) / 2,
        width,
        height,
    )
}

impl Default for Palette {
    fn default() -> Self {
        REFERENCE_PALETTE
    }
}
impl Palette {
    /// Resolve named themes through the shared registry, retaining Classic's
    /// calibrated default artwork while honoring user overrides.
    pub fn from_named<T: crate::theme::Resolve>(
        registry: &crate::theme::Registry<T>,
        id: &str,
    ) -> (Self, String) {
        let (theme, reason) = registry.resolve_named(id);
        let core = theme.core();
        let palette = if core.id == crate::theme::DEFAULT_ID && !reason.starts_with("user theme ") {
            Self::default()
        } else {
            Self::from_theme(core)
        };
        (palette, reason)
    }
    /// Resolve a graphical palette from KIT's existing theme roles.
    pub fn from_theme(theme: &crate::theme::Theme) -> Self {
        let rgb = |c: crate::theme::color::Rgb| [c.r, c.g, c.b];
        Self {
            bg: rgb(theme.bg),
            inset_bg: None,
            fg: rgb(theme.fg),
            muted: rgb(theme.dim),
            accent: rgb(theme.accent),
            selected: rgb(theme.row_selected_bg),
            border: rgb(theme.border),
            error: rgb(theme.error),
        }
    }
}

/// Physical pixels. Density changes must transform layout and hit regions
/// together through `Surface::at_density`, rather than resampling completed UI.
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    pub padding: u16,
    pub gap: u16,
    pub title_height: u16,
    pub control_height: u16,
    pub font: u16,
    pub small_font: u16,
}
impl Default for Metrics {
    fn default() -> Self {
        Self {
            padding: 12,
            gap: 8,
            title_height: 38,
            control_height: 28,
            font: 12,
            small_font: 11,
        }
    }
}

/// The default Classic Rack chrome. Construct once, then compose application
/// panels and controls without duplicating colors, metrics or artwork setup.
#[derive(Clone)]
pub struct Style {
    pub palette: Palette,
    pub colors: Colors,
    pub workspace: WorkspaceColors,
    pub metrics: Metrics,
    bitmap: bool,
    soft_workspace: bool,
    gradients: bool,
    #[cfg(feature = "terminal-graphics")]
    frames: std::sync::Arc<Frames>,
}
impl Default for Style {
    fn default() -> Self {
        Self::new(Palette::default())
    }
}
impl Style {
    /// Build default native chrome from a named registry theme.
    pub fn from_named<T: crate::theme::Resolve>(
        registry: &crate::theme::Registry<T>,
        id: &str,
    ) -> (Self, String) {
        let (palette, reason) = Palette::from_named(registry, id);
        (Self::new(palette), reason)
    }
    pub fn new(palette: Palette) -> Self {
        Self {
            palette,
            colors: colors(&palette),
            workspace: WorkspaceColors::new(&palette),
            metrics: Metrics::default(),
            bitmap: true,
            soft_workspace: false,
            gradients: true,
            #[cfg(feature = "terminal-graphics")]
            frames: {
                use std::sync::{Arc, OnceLock};
                static DEFAULT: OnceLock<Arc<Frames>> = OnceLock::new();
                if palette == Palette::default() {
                    DEFAULT
                        .get_or_init(|| Arc::new(Frames::new(&palette)))
                        .clone()
                } else {
                    Arc::new(Frames::new(&palette))
                }
            },
        }
    }
    /// Negotiate `native_skins` before sending sprites to a terminal frontend.
    /// Disabling artwork uses the same palette and geometry with primitives.
    pub fn with_native_skins(mut self, supported: bool) -> Self {
        self.bitmap = supported;
        self
    }
    /// Negotiate gradient support separately from legacy bitmap skins.
    pub fn with_native_gradients(mut self, supported: bool) -> Self {
        self.gradients = supported;
        self
    }
    pub fn surface(&self, width: u16, height: u16) -> Surface {
        Surface::new(width, height, hex(self.palette.bg)).with_physical_text()
    }
    #[cfg(feature = "terminal-graphics")]
    pub fn scene(
        &self,
        viewport: crate::terminal_graphics::protocol::Viewport,
    ) -> crate::terminal_graphics::protocol::Scene {
        use crate::terminal_graphics::protocol::Scene;
        Scene {
            revision: 0,
            interaction: 1,
            scroll_interaction: Some(1),
            viewport,
            background: hex(self.palette.bg),
            foreground: self.colors.ink.clone(),
            accent: self.colors.accent.clone(),
            border: self.colors.shadow.clone(),
            spans: vec![],
            components: vec![],
            placements: vec![],
            resize_handles: vec![],
            pointer_regions: vec![],
        }
    }
    /// Place a physical surface once for rendering, pointer projection and
    /// clipping. Menu/Dialog markers keep subsequent surfaces in the overlay.
    #[cfg(feature = "terminal-graphics")]
    pub fn mount(
        &self,
        scene: &mut crate::terminal_graphics::protocol::Scene,
        surface: Surface,
        x: u16,
        y: u16,
    ) -> anyhow::Result<()> {
        use crate::terminal_graphics::{placement::Placement, protocol::Component};
        let viewport = scene.viewport.validate()?;
        let target = R::new(x, y, surface.width, surface.height);
        anyhow::ensure!(
            u32::from(x) + u32::from(surface.width) <= viewport.width
                && u32::from(y) + u32::from(surface.height) <= viewport.height,
            "Surface outside viewport"
        );
        surface.validate()?;
        let rect = logical_rect(target, viewport);
        let mut placement = Placement::new(rect, target);
        if scene
            .components
            .iter()
            .any(|c| matches!(c, Component::Menu { .. } | Component::Dialog { .. }))
        {
            placement.overlay = Some(vec![]);
        }
        scene.placements.push(placement);
        scene.components.push(Component::Surface { rect, surface });
        Ok(())
    }
    pub fn frame(&self, s: &mut Surface, rect: R, inset: bool) {
        self.frame_at_density(s, rect, inset, false, 1);
    }
    /// Select original 1x/2x artwork before drawing at physical density.
    /// The application scales its layout, text and hit regions together.
    pub fn frame_at_density(
        &self,
        s: &mut Surface,
        rect: R,
        inset: bool,
        rigid: bool,
        density: u16,
    ) {
        #[cfg(feature = "terminal-graphics")]
        if self.bitmap
            && matches!(density, 1 | 2)
            && rect.width >= 12 * density
            && rect.height >= 12 * density
        {
            self.frames.paint(
                s,
                rect,
                if inset {
                    "well"
                } else if rigid {
                    "panel-rigid"
                } else {
                    "panel"
                },
                density,
            );
            return;
        }
        classic::frame(
            s,
            rect,
            &self.colors,
            if inset || rigid {
                0
            } else {
                4 * density.clamp(1, 4)
            },
            inset,
        );
    }
    pub fn panel(&self, width: u16, height: u16, title: &str) -> Surface {
        let mut s = self.surface(width, height);
        self.frame(&mut s, R::new(0, 0, width, height), false);
        if !title.is_empty() && height >= self.metrics.title_height && width >= 24 {
            panel_header(&mut s, width, &self.colors);
            s.text(
                R::new(18, 7, width.saturating_sub(36), 24),
                title,
                &self.colors.ink,
                self.metrics.small_font,
                true,
            );
        }
        s
    }
    pub fn label(&self, s: &mut Surface, rect: R, text: impl Into<String>) {
        s.text(rect, text, &self.colors.ink, self.metrics.font, false);
    }
    pub fn caption(&self, s: &mut Surface, rect: R, text: impl Into<String>) {
        s.text(rect, text, &self.colors.dim, self.metrics.small_font, true);
    }
    pub fn focus(&self, s: &mut Surface, rect: R) {
        if rect.width <= 4 || rect.height <= 4 {
            return;
        }
        s.nodes.push(Primitive::Border {
            rect: R::new(rect.x + 2, rect.y + 2, rect.width - 4, rect.height - 4),
            color: self.colors.accent.clone(),
            radius: 3,
        });
    }
    pub fn button(&self, s: &mut Surface, rect: R, text: &str, action: &str, active: bool) {
        if self.soft_workspace {
            self.tool(s, rect, text, action, active);
            return;
        }
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        let face = button_rect(rect);
        let node = s.nodes.len() as u16;
        s.fill(face, &self.colors.panel, 0);
        let hover = Rgb::parse_hex(&self.colors.panel)
            .unwrap()
            .mix(Rgb::parse_hex(&self.colors.ink).unwrap(), 0.06)
            .to_hex();
        s.hover_fills.push(HoverFill { node, color: hover });
        minimal_button(
            s,
            rect,
            &self.palette,
            action,
            active,
            false,
            false,
            false,
            false,
        );
        let pad = 3.min(face.width / 4);
        s.text_centered(
            R::new(
                face.x + pad,
                face.y,
                face.width.saturating_sub(pad * 2),
                face.height,
            ),
            text,
            if active {
                &self.colors.accent
            } else {
                &self.colors.ink
            },
            self.metrics.small_font,
            true,
        );
        s.hits.push(HitRegion {
            rect,
            action: action.into(),
        });
    }
    pub fn primary_button(
        &self,
        s: &mut Surface,
        rect: R,
        text: &str,
        action: &str,
        recording: bool,
    ) {
        let color = if recording {
            hex(self.palette.error)
        } else {
            self.colors.accent.clone()
        };
        let bg = Rgb::parse_hex(&self.colors.panel)
            .unwrap()
            .mix(Rgb::parse_hex(&color).unwrap(), 0.12)
            .to_hex();
        s.fill(rect, &bg, if self.soft_workspace { 6 } else { 2 });
        if !self.soft_workspace {
            s.nodes.push(Primitive::Border {
                rect,
                color: color.clone(),
                radius: 2,
            });
        }
        s.text_centered(rect, text, &self.colors.ink, self.metrics.font, true);
        s.hits.push(HitRegion {
            rect,
            action: action.into(),
        });
    }
    #[allow(clippy::too_many_arguments)]
    pub fn row(
        &self,
        s: &mut Surface,
        rect: R,
        title: &str,
        detail: &str,
        action: &str,
        selected: bool,
        checked: bool,
    ) {
        if rect.width < 12 || rect.height < 8 {
            return;
        }
        s.fill(
            rect,
            &if selected {
                hex(self.palette.selected)
            } else {
                self.workspace.well.clone()
            },
            0,
        );
        if selected {
            s.fill(
                R::new(rect.x, rect.y, 2, rect.height),
                &self.colors.accent,
                0,
            );
        }
        let text = if checked {
            format!("✓ {title}")
        } else {
            title.into()
        };
        s.text(
            R::new(
                rect.x + 10,
                rect.y + 2,
                rect.width - 12,
                20.min(rect.height - 2),
            ),
            text,
            &self.colors.ink,
            self.metrics.font,
            selected,
        );
        if !detail.is_empty() && rect.height > 24 {
            s.text(
                R::new(
                    rect.x + 10,
                    rect.y + 23,
                    rect.width - 12,
                    (rect.height - 23).min(18),
                ),
                detail,
                &self.colors.dim,
                self.metrics.small_font,
                false,
            );
        }
        s.hits.push(HitRegion {
            rect,
            action: action.into(),
        });
    }
    /// Read-only instrument meter; it deliberately has no pointer target.
    pub fn meter(&self, s: &mut Surface, rect: R, value: f32) {
        if rect.width < 2 || rect.height < 2 {
            return;
        }
        s.fill(rect, &self.colors.inset, 0);
        let width = ((rect.width - 2) as f32 * value.clamp(0., 1.)).round() as u16;
        if width > 0 {
            s.fill(
                R::new(rect.x + 1, rect.y + 1, width, rect.height - 2),
                &self.colors.accent,
                0,
            );
        }
    }
    /// Subtle category colors for a ledger or timeline; applications decide
    /// which category describes their data.
    pub fn category_color(&self, category: usize) -> String {
        let colors = [self.palette.accent, self.palette.border, self.palette.error];
        hex(colors[category % colors.len()])
    }
    pub fn clip(&self, s: &mut Surface, rect: R, accent: &str, selected: bool) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        let base = Rgb::parse_hex(&self.colors.panel).unwrap();
        let accent = Rgb::parse_hex(accent).unwrap();
        s.fill(
            rect,
            &base
                .mix(accent, if selected { 0.28 } else { 0.10 })
                .to_hex(),
            2,
        );
        s.fill(
            R::new(rect.x, rect.y, rect.width.min(2), rect.height),
            &accent.to_hex(),
            0,
        );
        if selected {
            s.nodes.push(Primitive::Border {
                rect,
                color: self.colors.accent.clone(),
                radius: 2,
            });
        }
    }
}
use crate::theme::color::Rgb;

#[cfg(feature = "terminal-graphics")]
pub fn logical_rect(
    target: R,
    v: crate::terminal_graphics::protocol::Viewport,
) -> crate::terminal_graphics::protocol::Rect {
    let x = (u32::from(target.x) * u32::from(v.columns) / v.width.max(1)).min(u32::from(v.columns))
        as u16;
    let y =
        (u32::from(target.y) * u32::from(v.rows) / v.height.max(1)).min(u32::from(v.rows)) as u16;
    let right = ((u32::from(target.x) + u32::from(target.width)) * u32::from(v.columns))
        .div_ceil(v.width.max(1))
        .min(u32::from(v.columns)) as u16;
    let bottom = ((u32::from(target.y) + u32::from(target.height)) * u32::from(v.rows))
        .div_ceil(v.height.max(1))
        .min(u32::from(v.rows)) as u16;
    crate::terminal_graphics::protocol::Rect {
        x,
        y,
        width: right.saturating_sub(x),
        height: bottom.saturating_sub(y),
    }
}

/// Shared title-strip geometry for skinned and ordinary rack modules.
pub fn panel_header(surface: &mut Surface, width: u16, colors: &Colors) {
    if width < 14 || surface.height < 31 {
        return;
    }
    let rect = R::new(7, 7, width - 14, 24);
    surface.fill(rect, &colors.title, 2);
    heading_depth(surface, rect, &colors.title, &colors.ink);
}
pub fn heading_depth(surface: &mut Surface, rect: R, background: &str, ink: &str) {
    if rect.width < 4 || rect.height < 2 {
        return;
    }
    let bg = Rgb::parse_hex(background).unwrap();
    surface.fill(
        R::new(rect.x + 2, rect.y, rect.width - 4, 1),
        &bg.mix(Rgb::parse_hex(ink).unwrap(), 0.06).to_hex(),
        0,
    );
    surface.fill(
        R::new(rect.x + 2, rect.y + rect.height - 1, rect.width - 4, 1),
        &bg.mix(Rgb::new(0, 0, 0), 0.18).to_hex(),
        0,
    );
}
pub fn close_button(surface: &mut Surface, rect: R, colors: &Colors, action: &str, hovered: bool) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    if hovered {
        surface.fill(rect, &colors.raised, 3);
    }
    let size = rect.width.min(rect.height).min(28).saturating_sub(2);
    if size > 0 {
        surface.nodes.push(Primitive::Icon {
            rect: R::new(
                rect.x + (rect.width - size) / 2,
                rect.y + (rect.height - size) / 2,
                size,
                size,
            ),
            name: "close".into(),
            color: if hovered { &colors.accent } else { &colors.ink }.clone(),
        });
    }
    surface.hits.push(HitRegion {
        rect,
        action: action.into(),
    });
}
/// AMP's compact non-transport control face. State belongs to the caller;
/// neither actions nor player behavior are interpreted by KIT.
#[allow(clippy::too_many_arguments)]
pub fn minimal_button(
    s: &mut Surface,
    rect: R,
    p: &Palette,
    _action: &str,
    active: bool,
    hovered: bool,
    pressed: bool,
    focused: bool,
    disabled: bool,
) {
    if disabled {
        return;
    }
    let rect = button_rect(rect);
    if hovered || pressed {
        let c = colors(p);
        let bg = if pressed {
            c.title
        } else if *p == REFERENCE_PALETTE {
            "#383d49".into()
        } else {
            Rgb::parse_hex(&c.panel)
                .unwrap()
                .mix(Rgb::parse_hex(&c.ink).unwrap(), 0.06)
                .to_hex()
        };
        s.fill(rect, &bg, 0);
        s.nodes.push(Primitive::Border {
            rect,
            color: if *p == REFERENCE_PALETTE {
                if pressed { "#11131a" } else { "#555c6b" }.into()
            } else {
                hex(p.border)
            },
            radius: 0,
        });
    }
    if active || focused {
        s.nodes.push(Primitive::Border {
            rect,
            color: hex(p.accent),
            radius: 0,
        });
    }
}

#[cfg(feature = "terminal-graphics")]
mod artwork;
#[cfg(feature = "terminal-graphics")]
struct FrameAsset {
    id: String,
    png: String,
    source: R,
    insets: [u16; 4],
}
#[cfg(feature = "terminal-graphics")]
struct Frames {
    assets: std::collections::BTreeMap<String, FrameAsset>,
}
#[cfg(feature = "terminal-graphics")]
impl Frames {
    fn new(palette: &Palette) -> Self {
        use super::skin::assets::{AssetCache, Layer, Sprite};
        use std::collections::BTreeMap;
        #[derive(Deserialize)]
        struct Manifest {
            sprites: BTreeMap<String, Sprite>,
            layers: BTreeMap<String, Vec<Layer>>,
            roles: BTreeMap<String, String>,
        }
        let manifest: Manifest =
            serde_json::from_str(include_str!("../../assets/chrome/manifest.json"))
                .expect("bundled chrome manifest");
        let mut source = AssetCache::new(1_000_000);
        for (id, png) in artwork::MASKS {
            source.insert_png(id, png).expect("bundled chrome mask");
        }
        let c = colors(palette);
        let roles = manifest
            .roles
            .iter()
            .map(|(role, original)| {
                let color = if *palette == REFERENCE_PALETTE {
                    original.clone()
                } else {
                    match role.as_str() {
                        "panel" => c.panel.clone(),
                        "highlight" | "well_highlight" => c.highlight.clone(),
                        "shadow" | "well_shadow" => c.shadow.clone(),
                        "well" => c.inset.clone(),
                        "well_border" | "border" => hex(palette.border),
                        _ => original.clone(),
                    }
                };
                let rgb = Rgb::parse_hex(&color).expect("chrome palette role");
                (role.clone(), [rgb.r, rgb.g, rgb.b])
            })
            .collect();
        let assets = manifest
            .layers
            .iter()
            .map(|(name, layers)| {
                let sprite = &manifest.sprites[name];
                let pixels = source
                    .compose(sprite.rect.width, sprite.rect.height, layers, &roles)
                    .expect("bundled chrome composition");
                let (id, png) = crate::terminal_graphics::assets::encode_skin(&pixels)
                    .expect("bundled chrome PNG");
                let sprite = &manifest.sprites[name];
                let slice = sprite.nine_slice.as_ref().expect("chrome frame slices");
                (
                    name.clone(),
                    FrameAsset {
                        id,
                        png,
                        source: R::new(0, 0, pixels.width() as u16, pixels.height() as u16),
                        insets: [
                            slice.insets.left,
                            slice.insets.top,
                            slice.insets.right,
                            slice.insets.bottom,
                        ],
                    },
                )
            })
            .collect();
        Self { assets }
    }
    fn paint(&self, s: &mut Surface, rect: R, kind: &str, density: u16) {
        let asset = &self.assets[&format!("{density}/{kind}")];
        s.assets.insert(asset.id.clone(), Some(asset.png.clone()));
        s.nodes.push(Primitive::Sprite {
            rect,
            asset: asset.id.clone(),
            source: asset.source,
            insets: Some(asset.insets),
            tint: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_chrome_and_media_palette_share_one_contract() {
        let p = Palette::default();
        let media: crate::media_controls::Palette = p;
        assert_eq!(media, REFERENCE_PALETTE);
        let c = colors(&p);
        assert_eq!(c.panel, "#303340");
        assert_eq!(c.title, "#242733");
        assert_eq!(c.inset, "#101813");
        assert!(
            Rgb::parse_hex(&c.panel)
                .unwrap()
                .contrast(Rgb::parse_hex(&c.ink).unwrap())
                > 4.5
        );
    }
    #[test]
    fn controls_keep_full_hits_shaped_alignment_and_local_hover_metadata() {
        let style = Style::default().with_native_skins(false);
        let mut surface = style.panel(360, 160, "SOURCES");
        let rect = R::new(12, 50, 200, 30);
        style.button(&mut surface, rect, "東京 · Microphone", "choose-mic", true);
        style.row(
            &mut surface,
            R::new(12, 88, 320, 44),
            "Studio microphone",
            "USB input",
            "device",
            true,
            true,
        );
        style.meter(&mut surface, R::new(12, 142, 200, 6), 0.4);
        surface.validate().unwrap();
        assert_eq!(
            surface.hit(13, 51).map(|h| h.action.as_str()),
            Some("choose-mic")
        );
        assert_eq!(
            surface.hits.len(),
            2,
            "Read-only meters must not capture input"
        );
        assert_eq!(surface.centered_text.len(), 1);
        assert_eq!(surface.hover_fills.len(), 1);
        assert!(surface.assets.is_empty());
        assert!(surface
            .nodes
            .iter()
            .all(|n| !matches!(n, Primitive::Sprite { .. })));
    }
    #[test]
    fn tiny_and_large_chrome_remains_inside_surface_bounds() {
        let style = Style::default();
        for (w, h) in [
            (1, 1),
            (8, 8),
            (24, 24),
            (100, 40),
            (640, 480),
            (3840, 2160),
        ] {
            let mut surface = style.panel(w, h, "PANEL");
            style.button(
                &mut surface,
                R::new(0, 0, w, h.min(28)),
                "Action",
                "action",
                false,
            );
            surface.validate().unwrap();
        }
    }
    #[cfg(feature = "terminal-graphics")]
    #[test]
    fn original_frame_densities_use_matching_slice_corners() {
        let style = Style::default();
        for density in [1, 2] {
            let mut s = style.surface(300 * density, 180 * density);
            let rect = R::new(0, 0, s.width, s.height);
            style.frame_at_density(&mut s, rect, false, false, density);
            s.validate().unwrap();
            let Primitive::Sprite {
                source,
                insets: Some(insets),
                ..
            } = &s.nodes[0]
            else {
                panic!("Default chrome lost its artwork")
            };
            assert_eq!((source.width, source.height), (24 * density, 24 * density));
            assert_eq!(*insets, [6 * density; 4]);
        }
    }
    #[cfg(feature = "terminal-graphics")]
    #[test]
    fn mounting_uses_one_physical_transform_and_preserves_modal_layering() {
        use crate::terminal_graphics::protocol::{Component, Viewport};
        let style = Style::default();
        let viewport = Viewport {
            width: 1301,
            height: 803,
            columns: 97,
            rows: 41,
            ..Viewport::default()
        };
        let mut scene = style.scene(viewport);
        style
            .mount(&mut scene, style.panel(331, 217, "PANEL"), 17, 23)
            .unwrap();
        let placement = &scene.placements[0];
        assert_eq!(placement.target, R::new(17, 23, 331, 217));
        placement.validate(viewport).unwrap();
        assert!(placement.overlay.is_none());
        scene.components.push(Component::Menu {
            rect: logical_rect(R::new(100, 100, 200, 100), viewport),
        });
        style
            .mount(&mut scene, style.panel(200, 100, "DIALOG"), 100, 100)
            .unwrap();
        assert!(scene.placements.last().unwrap().overlay.is_some());
        assert!(style
            .mount(&mut scene, style.surface(200, 100), 1200, 750)
            .is_err());
    }
}
