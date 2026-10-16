//! Quiet workspace chrome for viewers, property inspectors and timelines.
//! Palette roles and typography are shared; apps own layout and actions.
use super::*;

pub const HEADER_HEIGHT: u16 = 32;
/// Recording is a semantic state, independent of the selected theme accent.
pub const RECORDING_RED: &str = "#ff3b30";
pub const RECORDING_BACKGROUND: &str = "#991b1b";
#[derive(Clone, Debug)]
pub struct WorkspaceColors {
    pub panel: String,
    pub header: String,
    pub well: String,
    pub line: String,
    pub control: String,
    pub hover: String,
}
impl WorkspaceColors {
    pub fn new(p: &Palette) -> Self {
        let bg = Rgb::new(p.bg[0], p.bg[1], p.bg[2]);
        let fg = Rgb::new(p.fg[0], p.fg[1], p.fg[2]);
        Self {
            panel: bg.mix(fg, 0.025).to_hex(),
            header: bg.mix(fg, 0.065).to_hex(),
            well: bg.mix(fg, 0.008).to_hex(),
            line: bg.mix(fg, 0.14).to_hex(),
            control: bg.mix(fg, 0.09).to_hex(),
            hover: bg.mix(fg, 0.16).to_hex(),
        }
    }
}
/// Static, theme-derived desktop wallpaper. Coordinates are normalized so a
/// resize preserves the composition. No animation or frame-by-frame allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wallpaper {
    pub base: [u8; 3],
    pub accent: [u8; 3],
    pub secondary: [u8; 3],
}
impl Wallpaper {
    pub fn solid(color: [u8; 3]) -> Self {
        Self {
            base: color,
            accent: color,
            secondary: color,
        }
    }
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            base: p.bg,
            accent: p.accent,
            secondary: p.selected,
        }
    }
    /// Two broad, low-contrast pools, as in the SCREEN desktop design study.
    pub fn sample(&self, x: f64, y: f64) -> [u8; 3] {
        let pool = |cx: f64, cy: f64, radius: f64| {
            let distance = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() / radius;
            let t = (1. - distance).clamp(0., 1.);
            t * t * (3. - 2. * t)
        };
        let mut a = 0.105 * pool(0.18, 0.82, 0.86);
        let mut b = 0.10 * pool(0.84, 0.16, 0.92);
        let scale = (0.105 / (a + b).max(0.105)).min(1.);
        a *= scale;
        b *= scale;
        std::array::from_fn(|i| {
            (f64::from(self.base[i]) * (1. - a - b)
                + f64::from(self.accent[i]) * a
                + f64::from(self.secondary[i]) * b)
                .round() as u8
        })
    }
    /// Native little-endian ARGB pixels for an opaque compositor background.
    pub fn bgra(&self, width: u32, height: u32) -> Vec<u8> {
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            for x in 0..width {
                let [r, g, b] = self.sample(
                    f64::from(x) / f64::from(width.max(2) - 1),
                    f64::from(y) / f64::from(height.max(2) - 1),
                );
                pixels.extend_from_slice(&[b, g, r, 255]);
            }
        }
        pixels
    }
}
/// Application-owned commands rendered by the shared workspace menu.
#[derive(Clone, Debug)]
pub struct ContextItem {
    pub label: String,
    pub shortcut: String,
    pub action: String,
    pub enabled: bool,
    pub destructive: bool,
}
impl ContextItem {
    pub fn new(label: &str, shortcut: &str, action: &str) -> Self {
        Self {
            label: label.into(),
            shortcut: shortcut.into(),
            action: action.into(),
            enabled: true,
            destructive: false,
        }
    }
}
impl Style {
    /// Opt into the soft, flat workspace treatment. Legacy native chrome keeps
    /// its existing appearance until the application explicitly selects this.
    pub fn with_soft_workspace(mut self) -> Self {
        self.soft_workspace = true;
        let rgb = |c: [u8; 3]| Rgb::new(c[0], c[1], c[2]);
        let bg = rgb(self.palette.bg);
        let fg = rgb(self.palette.fg);
        self.workspace.panel = bg.mix(fg, 0.025).to_hex();
        self.workspace.header = bg.mix(fg, 0.045).to_hex();
        self.workspace.line = bg.mix(fg, 0.10).to_hex();
        self.workspace.control = bg.mix(fg, 0.045).to_hex();
        self.workspace.hover = bg.mix(fg, 0.09).to_hex();
        self.colors.ink = hex(self.palette.fg);
        self.colors.dim = hex(self.palette.muted);
        self.colors.accent = hex(self.palette.accent);
        self
    }
    /// Anchored context menu with keyboard selection and local hover feedback.
    /// Apps own positioning, dismissal and command dispatch.
    pub fn context_menu(
        &self,
        width: u16,
        title: &str,
        items: &[ContextItem],
        selected: Option<usize>,
    ) -> Surface {
        let count = items.len().min(20);
        let height = 36 + count as u16 * 32 + 8;
        let mut s = self.floating_panel(width, height);
        if width < 100 {
            return s;
        }
        s.text(
            R::new(14, 8, width - 28, 22),
            title,
            &self.colors.dim,
            10,
            true,
        );
        for (i, item) in items.iter().take(count).enumerate() {
            let rect = R::new(6, 36 + i as u16 * 32, width - 12, 30);
            let node = s.nodes.len() as u16;
            s.fill(
                rect,
                if selected == Some(i) {
                    &self.workspace.hover
                } else {
                    &self.workspace.header
                },
                5,
            );
            if item.enabled {
                s.hover_fills.push(HoverFill {
                    node,
                    color: self.workspace.hover.clone(),
                });
                s.hits.push(HitRegion {
                    rect,
                    action: item.action.clone(),
                });
            }
            let hint_width = (item.shortcut.chars().count() as u16 * 7).min(width / 3);
            let color = if !item.enabled {
                &self.colors.dim
            } else if item.destructive {
                &hex(self.palette.error)
            } else {
                &self.colors.ink
            };
            s.text(
                R::new(14, rect.y, width.saturating_sub(38 + hint_width), 30),
                &item.label,
                color,
                12,
                false,
            );
            if hint_width > 0 {
                s.text(
                    R::new(width - 14 - hint_width, rect.y, hint_width, 30),
                    &item.shortcut,
                    &self.colors.dim,
                    10,
                    false,
                );
            }
        }
        s
    }
    /// Rounded floating chrome for docks and compact overlays.
    pub fn floating_panel(&self, width: u16, height: u16) -> Surface {
        let mut s = Surface::new(width, height, "transparent".into()).with_ui_text();
        if width > 2 && height > 2 {
            let radius = 12.min(width / 2).min(height / 2);
            let rgb = |c: [u8; 3]| Rgb::new(c[0], c[1], c[2]);
            let shadow = rgb(self.palette.bg).mix(Rgb::new(0, 0, 0), 0.18).to_hex();
            s.fill(R::new(1, 2, width - 2, height - 2), &shadow, radius);
            s.fill(
                R::new(1, 1, width - 2, height - 2),
                &self.workspace.header,
                radius,
            );
            s.nodes.push(Primitive::Border {
                rect: R::new(1, 1, width - 2, height - 2),
                color: self.workspace.line.clone(),
                radius,
            });
        }
        s
    }
    /// Theme wallpaper is a small primitive; the frontend caches bounded rasters.
    /// Older frontends receive the solid theme base when gradients are disabled.
    pub fn wallpaper(&self, s: &mut Surface, rect: R) {
        s.fill(rect, &hex(self.palette.bg), 0);
        if self.gradients && rect.width > 0 && rect.height > 0 {
            s.nodes.push(Primitive::Gradient {
                rect,
                base: self.palette.bg,
                accent: self.palette.accent,
                secondary: self.palette.selected,
            });
        }
    }

    /// Noninteractive tutorial keycaps, sized in source pixels. The compact
    /// surface can be mounted over a live canvas without covering its viewport.
    pub fn keycaps(&self, chord: &str, scale: f64, max_width: u16) -> Surface {
        let keys: Vec<String> = chord
            .split(" + ")
            .take(5)
            .map(|key| key.chars().take(16).collect())
            .collect();
        let widths: Vec<f64> = keys
            .iter()
            .map(|key| (key.chars().count() as f64 * 12. + 24.).max(46.))
            .collect();
        let total = widths.iter().sum::<f64>() + 16. * keys.len().saturating_sub(1) as f64 + 20.;
        let scale = if scale.is_finite() {
            scale.clamp(0.1, 4.)
        } else {
            1.
        }
        .min(f64::from(max_width.max(1)) / total);
        let px = |n: f64| (n * scale).round() as u16;
        let mut s = Surface::new(
            px(total).max(1),
            px(62.).max(1),
            self.workspace.well.clone(),
        )
        .with_ui_text();
        let mut x = 10.;
        for (index, (key, width)) in keys.iter().zip(widths).enumerate() {
            let rect = R::new(
                px(x),
                px(8.),
                px(width).min(s.width.saturating_sub(px(x))),
                px(42.),
            );
            s.fill(
                R::new(rect.x, px(11.), rect.width, rect.height),
                &self.colors.shadow,
                px(5.),
            );
            s.fill(rect, &self.workspace.control, px(5.));
            s.text_centered(rect, key, &self.colors.ink, px(20.).max(1), true);
            if index + 1 == keys.len() && rect.width > 4 && rect.height > 4 {
                s.fill(
                    R::new(rect.x + 2, rect.y + rect.height - 2, rect.width - 4, 1),
                    &self.colors.accent,
                    0,
                );
            }
            x += width;
            if index + 1 < keys.len() {
                s.text_centered(
                    R::new(px(x), px(8.), px(16.), px(42.)),
                    "+",
                    &self.colors.dim,
                    px(15.).max(1),
                    false,
                );
                x += 16.;
            }
        }
        s
    }
    /// Persistent capture state; no hover or pointer target.
    pub fn recording_badge(&self, s: &mut Surface, rect: R, label: &str) {
        s.fill(rect, RECORDING_BACKGROUND, 2);
        s.text_centered(rect, label, "#ffffff", 12, true);
    }
    /// Focused dialog chrome, with a continuous two-pixel theme-accent border.
    pub fn modal_panel(&self, width: u16, height: u16, title: &str) -> Surface {
        if self.soft_workspace {
            let mut s = self.floating_panel(width, height);
            if width >= 28 && height >= 38 {
                s.text(
                    R::new(14, 6, width - 28, 24),
                    title,
                    &self.colors.ink,
                    11,
                    true,
                );
                s.fill(R::new(14, 36, width - 28, 1), &self.workspace.line, 0);
            }
            return s;
        }
        let mut s = self.workspace_panel(width, height, title);
        if width >= 8 && height >= 8 {
            for inset in 0..2 {
                s.nodes.push(Primitive::Border {
                    rect: R::new(inset, inset, width - inset * 2, height - inset * 2),
                    color: self.colors.accent.clone(),
                    radius: 4 - inset,
                });
            }
        }
        s
    }
    pub fn workspace_panel(&self, width: u16, height: u16, title: &str) -> Surface {
        let mut s = Surface::new(width, height, self.workspace.panel.clone()).with_ui_text();
        if width > 0 && height > 0 {
            s.fill(R::new(0, 0, width, 1), &self.workspace.line, 0);
        }
        if !title.is_empty() && height >= HEADER_HEIGHT && width >= 24 {
            s.fill(
                R::new(0, 1, width, HEADER_HEIGHT - 1),
                &self.workspace.header,
                0,
            );
            s.text(
                R::new(12, 4, width - 24, 24),
                title,
                &self.colors.ink,
                11,
                true,
            );
            s.fill(
                R::new(0, HEADER_HEIGHT - 1, width, 1),
                &self.workspace.line,
                0,
            );
        }
        s
    }
    /// A full-size compact control; one rectangle governs face, hover and hit.
    pub fn tool(&self, s: &mut Surface, rect: R, text: &str, action: &str, active: bool) {
        if rect.width < 4 || rect.height < 4 {
            return;
        }
        let bg = if active && self.soft_workspace {
            Rgb::new(self.palette.bg[0], self.palette.bg[1], self.palette.bg[2])
                .mix(
                    Rgb::new(
                        self.palette.accent[0],
                        self.palette.accent[1],
                        self.palette.accent[2],
                    ),
                    0.12,
                )
                .to_hex()
        } else if active {
            hex(self.palette.selected)
        } else {
            self.workspace.control.clone()
        };
        let node = s.nodes.len() as u16;
        s.fill(rect, &bg, if self.soft_workspace { 6 } else { 3 });
        s.hover_fills.push(HoverFill {
            node,
            color: self.workspace.hover.clone(),
        });
        if active && !self.soft_workspace {
            s.fill(
                R::new(rect.x + 2, rect.y + rect.height - 2, rect.width - 4, 2),
                &self.colors.accent,
                0,
            );
        }
        s.text_centered(
            R::new(
                rect.x + 3,
                rect.y,
                rect.width - 6.min(rect.width),
                rect.height,
            ),
            text,
            if active && self.soft_workspace {
                &self.colors.accent
            } else {
                &self.colors.ink
            },
            12,
            false,
        );
        s.hits.push(HitRegion {
            rect,
            action: action.into(),
        });
    }
    /// A aligned label and inset value. An empty action makes it read-only.
    pub fn property(&self, s: &mut Surface, rect: R, label: &str, value: &str, action: &str) {
        if rect.width < 40 || rect.height < 16 {
            return;
        }
        let label_width = (rect.width * 9 / 20).min(104);
        s.text(
            R::new(rect.x, rect.y, label_width.saturating_sub(6), rect.height),
            label,
            &self.colors.dim,
            12,
            false,
        );
        let value_rect = R::new(
            rect.x + label_width,
            rect.y,
            rect.width - label_width,
            rect.height,
        );
        if !action.is_empty() {
            let node = s.nodes.len() as u16;
            s.fill(value_rect, &self.workspace.control, 2);
            s.hover_fills.push(HoverFill {
                node,
                color: self.workspace.hover.clone(),
            });
            s.hits.push(HitRegion {
                rect,
                action: action.into(),
            });
        }
        s.text(
            R::new(
                value_rect.x + 6,
                value_rect.y,
                value_rect.width - 8,
                value_rect.height,
            ),
            value,
            &self.colors.ink,
            12,
            false,
        );
    }
    pub fn section(&self, s: &mut Surface, rect: R, title: &str) {
        if rect.width < 24 || rect.height < 20 {
            return;
        }
        s.text(
            R::new(rect.x, rect.y, rect.width, 18),
            title,
            &self.colors.dim,
            10,
            true,
        );
        s.fill(
            R::new(rect.x, rect.y + rect.height - 1, rect.width, 1),
            &self.workspace.line,
            0,
        );
    }
    pub fn numeric(&self, s: &mut Surface, rect: R, text: impl Into<String>, accent: bool) {
        s.nodes.push(Primitive::Text {
            rect,
            text: text.into(),
            color: if accent {
                self.colors.accent.clone()
            } else {
                self.colors.ink.clone()
            },
            size: 12,
            bold: false,
            mono: true,
        });
    }
    /// Draw a time ruler against the same transform used by seeking and clips.
    pub fn time_ruler(&self, s: &mut Surface, view: crate::editor_controls::TimeView, fps: u32) {
        let r = view.rect;
        if r.width == 0 || r.height < 16 || fps == 0 {
            return;
        }
        s.fill(r, &self.workspace.header, 0);
        let target = view.frames_per_pixel * 100.;
        let base = [1., 2., 5., 10., 15., 30.];
        let mut steps: Vec<f64> = base.into_iter().collect();
        for seconds in [
            1., 2., 5., 10., 15., 30., 60., 120., 300., 600., 1800., 3600., 7200., 18000.,
        ] {
            steps.push(seconds * f64::from(fps));
        }
        let step = steps
            .into_iter()
            .find(|n| *n >= target)
            .unwrap_or_else(|| (target / f64::from(fps)).ceil() * f64::from(fps))
            .max(1.) as i64;
        let first = view.first_frame.div_euclid(step) * step;
        let last = view.frame_at(f64::from(r.x + r.width));
        for frame in (first..=last).step_by(step as usize).take(256) {
            let x = view.x_at(frame);
            if frame >= 0 && x >= f64::from(r.x) && x < f64::from(r.x + r.width) {
                let x = x.round().min(f64::from(r.x + r.width - 1)) as u16;
                s.fill(R::new(x, r.y + r.height - 6, 1, 6), &self.workspace.line, 0);
                let width = 98.min(r.x + r.width - x);
                if width > 30 {
                    self.numeric(
                        s,
                        R::new(x + 4, r.y, width.saturating_sub(4), r.height - 5),
                        crate::editor_controls::timecode(frame as f64 / f64::from(fps), fps),
                        false,
                    );
                }
            }
            for minor in 1..5 {
                let x = view.x_at(frame + step * minor / 5);
                if x >= f64::from(r.x) && x < f64::from(r.x + r.width) {
                    s.fill(
                        R::new(
                            x.round().min(f64::from(r.x + r.width - 1)) as u16,
                            r.y + r.height - 3,
                            1,
                            3,
                        ),
                        &self.workspace.line,
                        0,
                    );
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "terminal-graphics")]
    #[test]
    fn floating_panel_rounded_corners_preserve_the_desktop_beneath() {
        let style = Style::default().with_soft_workspace();
        let v = crate::terminal_graphics::protocol::Viewport {
            width: 796,
            height: 382,
            ..Default::default()
        };
        let mut scene = style.scene(v);
        let base = Surface::new(796, 382, "#345678".into());
        style.mount(&mut scene, base, 0, 0).unwrap();
        style
            .mount(&mut scene, style.floating_panel(440, 54), 178, 282)
            .unwrap();
        let image = crate::terminal_graphics::renderer::render_reference(&scene).unwrap();
        assert_eq!(image.get_pixel(178, 282).0, [0x34, 0x56, 0x78, 255]);
        assert_ne!(image.get_pixel(398, 309).0, [0x34, 0x56, 0x78, 255]);
    }
    #[cfg(feature = "terminal-graphics")]
    #[test]
    fn native_wallpaper_is_continuous_at_fractional_and_4k_sizes_without_assets() {
        let style = Style::default().with_soft_workspace();
        for (width, height) in [(796, 382), (1301, 803), (3840, 2160)] {
            let viewport = crate::terminal_graphics::protocol::Viewport {
                width,
                height,
                ..Default::default()
            };
            let mut scene = style.scene(viewport);
            let mut surface = Surface::new(width as u16, height as u16, "transparent".into());
            style.wallpaper(&mut surface, R::new(0, 0, width as u16, height as u16));
            assert!(surface.assets.is_empty());
            surface.validate().unwrap();
            style.mount(&mut scene, surface, 0, 0).unwrap();
            let image = crate::terminal_graphics::renderer::render_reference(&scene).unwrap();
            let wallpaper = Wallpaper::from_palette(&style.palette);
            for (x, y) in [
                (0, 0),
                (width / 2, height / 2),
                (width - 1, height - 1),
                (width / 4, height * 3 / 4),
            ] {
                let expected = wallpaper.sample(
                    f64::from(x) / f64::from(width - 1),
                    f64::from(y) / f64::from(height - 1),
                );
                assert_eq!(image.get_pixel(x, y).0[..3], expected);
            }
        }
        let style = style.with_native_gradients(false);
        let mut surface = style.workspace_panel(320, 240, "");
        style.wallpaper(&mut surface, R::new(0, 0, 320, 240));
        assert!(!surface
            .nodes
            .iter()
            .any(|n| matches!(n, Primitive::Gradient { .. })));
    }
    #[test]
    fn wallpaper_is_subtle_static_theme_derived_and_has_solid_override() {
        for palette in [
            REFERENCE_PALETTE,
            Palette {
                bg: [245, 240, 230],
                fg: [35, 40, 45],
                ..REFERENCE_PALETTE
            },
        ] {
            let wallpaper = Wallpaper::from_palette(&palette);
            let bytes = wallpaper.bgra(192, 128);
            assert_eq!(bytes.len(), 192 * 128 * 4);
            assert_eq!(bytes, wallpaper.bgra(192, 128));
            assert_ne!(wallpaper.sample(0.18, 0.82), wallpaper.sample(0.84, 0.16));
            for pixel in bytes.as_chunks::<4>().0 {
                assert_eq!(pixel[3], 255);
                for i in 0..3 {
                    assert!((i16::from(pixel[2 - i]) - i16::from(palette.bg[i])).abs() <= 28);
                }
            }
        }
        assert_eq!(
            Wallpaper::solid([12, 34, 56]).bgra(2, 2),
            [56, 34, 12, 255].repeat(4)
        );
    }
    #[test]
    fn soft_workspace_is_opt_in_and_context_menus_keep_disabled_rows_inert() {
        let legacy = Style::default();
        let modern = legacy.clone().with_soft_workspace();
        let mut a = Surface::new(300, 100, "transparent".into());
        let mut b = a.clone();
        legacy.tool(&mut a, R::new(4, 4, 100, 28), "Tool", "tool", true);
        modern.tool(&mut b, R::new(4, 4, 100, 28), "Tool", "tool", true);
        assert_ne!(a, b);
        let mut disabled = ContextItem::new("Delete", "Del", "delete");
        disabled.enabled = false;
        let menu = modern.context_menu(
            302,
            "DESKTOP",
            &[ContextItem::new("Launch", "Ctrl K", "launch"), disabled],
            Some(0),
        );
        menu.validate().unwrap();
        assert_eq!(menu.hits.len(), 1);
        assert_eq!(menu.hits[0].action, "launch");
        modern.floating_panel(440, 54).validate().unwrap();
    }
    #[test]
    fn keycaps_stay_bounded_without_intercepting_input() {
        for width in [1, 64, 320, 1920] {
            for scale in [0.2, 1., 2.] {
                let surface =
                    Style::default().keycaps("Ctrl + Alt + Shift + Backspace", scale, width);
                surface.validate().unwrap();
                assert!(surface.width <= width && surface.hits.is_empty());
            }
        }
    }
    #[test]
    fn modals_have_a_bounded_accent_outline() {
        let style = Style::default();
        for (w, h) in [(1, 1), (8, 8), (360, 144), (760, 540)] {
            let surface = style.modal_panel(w, h, "DIALOG");
            surface.validate().unwrap();
            if w >= 8 {
                let borders = surface.nodes.iter().filter(|node| matches!(node,Primitive::Border {color,..} if color == &style.colors.accent)).count();
                assert_eq!(borders, 2);
            }
        }
    }
    #[test]
    fn workspace_fields_share_bounds_hover_and_typography() {
        for palette in [
            REFERENCE_PALETTE,
            Palette {
                bg: [245, 240, 230],
                fg: [40, 35, 30],
                ..REFERENCE_PALETTE
            },
        ] {
            let style = Style::new(palette);
            for (w, h) in [(1, 1), (100, 100), (360, 240)] {
                let mut s = style.workspace_panel(w, h, "INSPECTOR");
                assert!(s.ui_font);
                if w == 360 {
                    style.tool(&mut s, R::new(12, 40, 110, 28), "Fit", "fit", true);
                    style.property(
                        &mut s,
                        R::new(12, 80, 336, 28),
                        "Resolution",
                        "1920 × 1080",
                        "resolution",
                    );
                    let view =
                        crate::editor_controls::TimeView::new(0, 2., R::new(12, 130, 336, 24))
                            .unwrap();
                    style.time_ruler(&mut s, view, 60);
                    assert_eq!(s.hits[0].rect, R::new(12, 40, 110, 28));
                    assert_eq!(s.hover_fills.len(), 2);
                }
                s.validate().unwrap();
            }
        }
    }
}
