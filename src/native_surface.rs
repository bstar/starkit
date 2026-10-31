//! Bounded, reusable native surfaces. Coordinates are pixels local to the surface.
//! Hosts place the surface; its owner retains layout and interaction semantics.
use serde::{Deserialize, Serialize};

pub mod classic;
#[cfg(feature = "image")]
pub mod skin;
pub mod style;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PixelRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}
impl PixelRect {
    pub fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
    pub fn contains(self, x: u16, y: u16) -> bool {
        x >= self.x
            && y >= self.y
            && u32::from(x) < u32::from(self.x) + u32::from(self.width)
            && u32::from(y) < u32::from(self.y) + u32::from(self.height)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Primitive {
    /// Negotiate `native_gradients` before sending theme wallpaper primitives.
    Gradient {
        rect: PixelRect,
        base: [u8; 3],
        accent: [u8; 3],
        secondary: [u8; 3],
    },
    Fill {
        rect: PixelRect,
        color: String,
        radius: u16,
    },
    Border {
        rect: PixelRect,
        color: String,
        radius: u16,
    },
    Text {
        rect: PixelRect,
        text: String,
        color: String,
        size: u16,
        bold: bool,
        mono: bool,
    },
    /// A bounded polyline in local pixels; negotiate `native_paths` first.
    Path {
        rect: PixelRect,
        points: Vec<[u16; 2]>,
        color: String,
        width: u16,
    },
    /// Cached artwork in physical source pixels; negotiate `native_skins` first.
    Sprite {
        rect: PixelRect,
        asset: String,
        source: PixelRect,
        /// Left/top/right/bottom source-pixel corner insets; None forbids scaling.
        insets: Option<[u16; 4]>,
        tint: Option<String>,
    },
    Icon {
        rect: PixelRect,
        name: String,
        color: String,
    },
}
impl Primitive {
    /// Presentation adapters move bounds through KIT so new primitive variants
    /// do not require exhaustive geometry matches in every application.
    pub fn rect_mut(&mut self) -> &mut PixelRect {
        match self {
            Self::Gradient { rect, .. }
            | Self::Fill { rect, .. }
            | Self::Border { rect, .. }
            | Self::Text { rect, .. }
            | Self::Icon { rect, .. }
            | Self::Sprite { rect, .. }
            | Self::Path { rect, .. } => rect,
        }
    }
    pub fn rect(&self) -> PixelRect {
        match self {
            Self::Gradient { rect, .. }
            | Self::Fill { rect, .. }
            | Self::Border { rect, .. }
            | Self::Text { rect, .. }
            | Self::Icon { rect, .. }
            | Self::Sprite { rect, .. }
            | Self::Path { rect, .. } => *rect,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HitRegion {
    pub rect: PixelRect,
    pub action: String,
}
/// A divider following a text node's rendered ink, measured on the frontend.
/// Older renderers ignore this optional surface decoration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRule {
    pub node: u16,
    pub end: u16,
    pub gap: u16,
    pub width: u16,
    pub color: String,
}

/// Optional shaped caret for a text node. Byte offsets refer to its UTF-8 text;
/// older frontends may ignore this additive presentation hint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextCaret {
    pub node: u16,
    pub byte: usize,
    pub color: String,
}
/// A presentation-only hover color for an existing background fill. It does
/// not carry an action or alter the controller's selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoverFill {
    pub node: u16,
    pub color: String,
}
/// Legacy surfaces follow terminal font scaling. Physical surfaces keep glyphs
/// at the declared pixel size while hosts resize their containing layout boxes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextScale {
    #[default]
    Terminal,
    Physical,
}
impl TextScale {
    fn is_terminal(&self) -> bool {
        *self == Self::Terminal
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Surface {
    pub width: u16,
    pub height: u16,
    /// Opaque RGB, or `"transparent"` to preserve the canvas beneath this surface.
    pub background: String,
    /// Additive hint. Older frontends ignore it and keep legacy font scaling.
    #[serde(default, skip_serializing_if = "TextScale::is_terminal")]
    pub text_scale: TextScale,
    /// Opt into bundled UI sans/mono faces instead of the terminal font.
    /// Older peers ignore this additive presentation hint.
    #[serde(default, skip_serializing_if = "is_false")]
    pub ui_font: bool,
    /// Fixed editor grid, in source pixels. Ordinary surfaces use shaped text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell_size: Option<[u16; 2]>,
    /// Terminal font size in thousandths of a physical pixel. Also opts fixed
    /// grids into whole-run shaping, preserving ligatures and cluster bearings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_font_millipixels: Option<u32>,
    /// Content-addressed PNG payloads; None references artwork cached by the frontend.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub assets: std::collections::BTreeMap<String, Option<String>>,
    pub nodes: Vec<Primitive>,
    pub hits: Vec<HitRegion>,
    /// Negotiate `surface_hover` before relying on frontend hover presentation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hover_fills: Vec<HoverFill>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text_rules: Vec<TextRule>,
    /// Text nodes centered within their rectangles using frontend-shaped widths.
    /// Older frontends ignore this optional presentation hint.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub centered_text: Vec<u16>,
    /// Right-aligned using frontend-shaped advances. Older peers keep text
    /// readable with their ordinary left alignment.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub right_aligned_text: Vec<u16>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text_carets: Vec<TextCaret>,
}
fn is_false(value: &bool) -> bool {
    !*value
}
impl Surface {
    /// Filter drawing nodes while retaining and remapping their presentation metadata.
    /// Hit regions have their own geometry and remain the caller's responsibility.
    pub fn retain_nodes(&mut self, mut keep: impl FnMut(&Primitive) -> bool) {
        let mut mapping = vec![None; self.nodes.len()];
        let mut old = 0;
        let mut next = 0u16;
        self.nodes.retain(|node| {
            let retained = keep(node);
            if retained {
                mapping[old] = Some(next);
                next += 1;
            }
            old += 1;
            retained
        });
        self.hover_fills.retain_mut(|hover| {
            if let Some(Some(node)) = mapping.get(usize::from(hover.node)) {
                hover.node = *node;
                true
            } else {
                false
            }
        });
        self.text_rules.retain_mut(|rule| {
            if let Some(Some(node)) = mapping.get(usize::from(rule.node)) {
                rule.node = *node;
                true
            } else {
                false
            }
        });
        self.text_carets.retain_mut(|caret| {
            if let Some(Some(node)) = mapping.get(usize::from(caret.node)) {
                caret.node = *node;
                true
            } else {
                false
            }
        });
        self.centered_text = self
            .centered_text
            .iter()
            .filter_map(|node| mapping.get(usize::from(*node)).copied().flatten())
            .collect();
        self.right_aligned_text = self
            .right_aligned_text
            .iter()
            .filter_map(|node| mapping.get(usize::from(*node)).copied().flatten())
            .collect();
    }
    pub fn new(width: u16, height: u16, background: String) -> Self {
        Self {
            width,
            height,
            background,
            text_scale: TextScale::Terminal,
            ui_font: false,
            cell_size: None,
            terminal_font_millipixels: None,
            assets: Default::default(),
            nodes: vec![],
            hits: vec![],
            hover_fills: vec![],
            text_rules: vec![],
            centered_text: vec![],
            right_aligned_text: vec![],
            text_carets: vec![],
        }
    }
    pub fn with_physical_text(mut self) -> Self {
        self.text_scale = TextScale::Physical;
        self
    }
    /// Stable UI typography in physical pixels, independent of terminal font.
    pub fn with_ui_text(mut self) -> Self {
        self.text_scale = TextScale::Physical;
        self.ui_font = true;
        self
    }
    /// Lay out logical design units at an explicit density. Text is reshaped at
    /// the resulting physical size; source atlas pixels are already selected by
    /// the application and are never resampled here.
    pub fn at_density(mut self, density: u16) -> anyhow::Result<Self> {
        anyhow::ensure!((1..=4).contains(&density), "Invalid surface density");
        let scale = |n: u16| {
            n.checked_mul(density)
                .ok_or_else(|| anyhow::anyhow!("Surface density overflow"))
        };
        let rect = |r: &mut PixelRect| -> anyhow::Result<()> {
            r.x = scale(r.x)?;
            r.y = scale(r.y)?;
            r.width = scale(r.width)?;
            r.height = scale(r.height)?;
            Ok(())
        };
        self.width = scale(self.width)?;
        self.height = scale(self.height)?;
        if let Some([w, h]) = &mut self.cell_size {
            *w = scale(*w)?;
            *h = scale(*h)?;
        }
        for node in &mut self.nodes {
            match node {
                Primitive::Fill {
                    rect: r, radius, ..
                }
                | Primitive::Border {
                    rect: r, radius, ..
                } => {
                    rect(r)?;
                    *radius = scale(*radius)?;
                }
                Primitive::Text { rect: r, size, .. } => {
                    rect(r)?;
                    *size = scale(*size)?;
                }
                Primitive::Path {
                    rect: r,
                    points,
                    width,
                    ..
                } => {
                    rect(r)?;
                    *width = scale(*width)?;
                    for [x, y] in points {
                        *x = scale(*x)?;
                        *y = scale(*y)?;
                    }
                }
                Primitive::Gradient { rect: r, .. }
                | Primitive::Sprite { rect: r, .. }
                | Primitive::Icon { rect: r, .. } => rect(r)?,
            }
        }
        for rule in &mut self.text_rules {
            rule.end = scale(rule.end)?;
            rule.gap = scale(rule.gap)?;
            rule.width = scale(rule.width)?;
        }
        for hit in &mut self.hits {
            rect(&mut hit.rect)?;
        }
        self.validate()?;
        Ok(self)
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.width > 0
                && self.height > 0
                && self.width <= 8192
                && self.height <= 8192
                // Surfaces describe canvas coordinates, not decoded images.
                // Match the terminal viewport budget so a 4K rack does not
                // have to shrink its canvas and stretch fixed bitmap glyphs.
                && u32::from(self.width) * u32::from(self.height) <= 32_000_000,
            "invalid surface size"
        );
        anyhow::ensure!(
            self.nodes.len() <= 4096
                && self.hits.len() <= 256
                && self.hover_fills.len() <= 256
                && self.text_rules.len() <= 256
                && self.centered_text.len() <= 4096
                && self.right_aligned_text.len() <= 4096
                && self.text_carets.len() <= 256,
            "surface exceeds limits"
        );
        anyhow::ensure!(
            self.terminal_font_millipixels
                .is_none_or(|n| (4000..=96000).contains(&n)),
            "invalid terminal font size"
        );
        if let Some([width, height]) = self.cell_size {
            anyhow::ensure!(
                width > 0 && height > 0 && width <= 256 && height <= 256,
                "invalid surface cell size"
            );
        }
        let inside = |r: PixelRect| {
            u32::from(r.x) + u32::from(r.width) <= u32::from(self.width)
                && u32::from(r.y) + u32::from(r.height) <= u32::from(self.height)
        };
        let color = |s: &str| {
            s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|c| c.is_ascii_hexdigit())
        };
        let mut centered = vec![false; self.nodes.len()];
        anyhow::ensure!(
            self.centered_text.iter().all(|node| matches!(
                self.nodes.get(usize::from(*node)),
                Some(Primitive::Text { .. })
            )),
            "invalid centered text node"
        );
        for node in &self.centered_text {
            centered[usize::from(*node)] = true;
        }
        anyhow::ensure!(
            self.right_aligned_text.iter().all(|node| matches!(
                self.nodes.get(usize::from(*node)),
                Some(Primitive::Text { .. })
            ) && !centered[usize::from(*node)]),
            "invalid right-aligned text node"
        );
        anyhow::ensure!(
            self.background == "transparent" || color(&self.background),
            "invalid surface background"
        );
        anyhow::ensure!(
            self.hover_fills.iter().all(|hover| color(&hover.color)
                && matches!(
                    self.nodes.get(usize::from(hover.node)),
                    Some(Primitive::Fill { .. })
                )),
            "invalid surface hover fill"
        );
        anyhow::ensure!(
            self.text_rules.iter().all(|rule| color(&rule.color)
                && rule.end <= self.width
                && rule.gap <= 256
                && (1..=8).contains(&rule.width)
                && matches!(self.nodes.get(usize::from(rule.node)),
                    Some(Primitive::Text { rect, .. }) if rect.x <= rule.end)),
            "invalid surface text rule"
        );
        anyhow::ensure!(self.text_carets.iter().all(|caret| color(&caret.color) && matches!(self.nodes.get(usize::from(caret.node)), Some(Primitive::Text { text, .. }) if text.is_char_boundary(caret.byte))), "invalid surface text caret");
        let skin_id = |id: &str| {
            id.strip_prefix("skin/").is_some_and(|h| {
                h.len() == 64
                    && h.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        };
        anyhow::ensure!(
            self.assets.len() <= 256
                && self.assets.iter().all(|(id, png)| skin_id(id)
                    && png.as_ref().is_none_or(|s| s.len() <= 1_000_000
                        && s.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))))
                && self
                    .assets
                    .values()
                    .flatten()
                    .map(String::len)
                    .sum::<usize>()
                    <= 4_000_000,
            "Invalid skin asset bundle"
        );
        let mut path_points = 0usize;
        for node in &self.nodes {
            anyhow::ensure!(inside(node.rect()), "primitive outside surface");
            match node {
                Primitive::Gradient { .. } => {}
                Primitive::Path {
                    rect,
                    points,
                    color: c,
                    width,
                } => {
                    path_points = path_points.saturating_add(points.len());
                    anyhow::ensure!(
                        (2..=2048).contains(&points.len())
                            && path_points <= 8192
                            && (1..=8).contains(width)
                            && color(c)
                            && points
                                .iter()
                                .all(|p| p[0] < rect.width && p[1] < rect.height),
                        "invalid surface path"
                    );
                }
                Primitive::Sprite {
                    rect,
                    asset,
                    source,
                    insets,
                    tint,
                } => {
                    anyhow::ensure!(
                        self.assets.contains_key(asset)
                            && skin_id(asset)
                            && source.width > 0
                            && source.height > 0
                            && u32::from(source.x) + u32::from(source.width) <= 8192
                            && u32::from(source.y) + u32::from(source.height) <= 8192
                            && tint.as_ref().is_none_or(|c| color(c))
                            && (insets.is_some()
                                || (rect.width == source.width && rect.height == source.height))
                            && insets.is_none_or(|[l, t, r, b]| u32::from(l) + u32::from(r)
                                < u32::from(source.width)
                                && u32::from(t) + u32::from(b) < u32::from(source.height)),
                        "Invalid surface sprite"
                    );
                }
                Primitive::Text {
                    text,
                    size,
                    color: c,
                    ..
                } => anyhow::ensure!(
                    text.len() <= 8192 && (1..=128).contains(size) && color(c),
                    "invalid surface text"
                ),
                Primitive::Fill { color: c, .. }
                | Primitive::Border { color: c, .. }
                | Primitive::Icon { color: c, .. } => {
                    anyhow::ensure!(color(c), "invalid surface color")
                }
            }
        }
        anyhow::ensure!(
            self.hits
                .iter()
                .all(|h| inside(h.rect) && h.action.len() <= 128),
            "invalid surface hit region"
        );
        Ok(())
    }
    pub fn hit(&self, x: u16, y: u16) -> Option<&HitRegion> {
        self.hits.iter().rev().find(|hit| hit.rect.contains(x, y))
    }
    pub fn fill(&mut self, rect: PixelRect, color: &str, radius: u16) {
        self.nodes.push(Primitive::Fill {
            rect,
            color: color.into(),
            radius,
        });
    }
    pub fn text(
        &mut self,
        rect: PixelRect,
        text: impl Into<String>,
        color: &str,
        size: u16,
        bold: bool,
    ) {
        self.nodes.push(Primitive::Text {
            rect,
            text: text.into(),
            color: color.into(),
            size,
            bold,
            mono: false,
        });
    }
    pub fn text_centered(
        &mut self,
        rect: PixelRect,
        text: impl Into<String>,
        color: &str,
        size: u16,
        bold: bool,
    ) {
        let node = self.nodes.len() as u16;
        self.text(rect, text, color, size, bold);
        self.centered_text.push(node);
    }
    pub fn text_right(
        &mut self,
        rect: PixelRect,
        text: impl Into<String>,
        color: &str,
        size: u16,
        bold: bool,
    ) {
        let node = self.nodes.len() as u16;
        self.text(rect, text, color, size, bold);
        self.right_aligned_text.push(node);
    }
}

/// One spacing vocabulary for native applications and embedded content.
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    pub font: u16,
    pub small: u16,
    pub gap: u16,
    pub inset: u16,
    pub control: u16,
}
impl Metrics {
    pub fn from_cell(width: u16, height: u16) -> Self {
        let font = (f32::from(height) * 0.84)
            .min(f32::from(width) / 0.6)
            .floor()
            .clamp(1., 64.) as u16;
        let scale = (f32::from(font) / 14.).max(1.);
        Self {
            font,
            small: (4. * scale).round() as u16,
            gap: (8. * scale).round() as u16,
            inset: (12. * scale).round() as u16,
            control: height.max((font + 16).min(96)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn right_aligned_text_is_additive_bounded_and_remapped_when_nodes_are_removed() {
        let legacy = r##"{"width":200,"height":40,"background":"#000000","nodes":[],"hits":[]}"##;
        let mut surface: Surface = serde_json::from_str(legacy).unwrap();
        assert!(surface.right_aligned_text.is_empty());
        assert!(!serde_json::to_string(&surface)
            .unwrap()
            .contains("right_aligned_text"));
        surface.fill(PixelRect::new(0, 0, 200, 40), "#101010", 0);
        surface.text_right(PixelRect::new(10, 0, 80, 20), "12", "#ffffff", 12, false);
        surface.text_right(PixelRect::new(10, 20, 80, 20), "3", "#ffffff", 12, false);
        surface.validate().unwrap();
        surface.retain_nodes(|node| matches!(node, Primitive::Text { text, .. } if text == "12"));
        assert_eq!(surface.right_aligned_text, [0]);
        surface.validate().unwrap();
        let roundtrip: Surface =
            serde_json::from_str(&serde_json::to_string(&surface).unwrap()).unwrap();
        assert_eq!(roundtrip.right_aligned_text, [0]);
        surface.centered_text.push(0);
        assert!(surface.validate().is_err());
        surface.centered_text.clear();
        surface.right_aligned_text[0] = 1;
        assert!(surface.validate().is_err());
        surface.right_aligned_text[0] = 0;
        surface.nodes[0] = Primitive::Fill {
            rect: PixelRect::new(0, 0, 20, 20),
            color: "#ffffff".into(),
            radius: 0,
        };
        assert!(surface.validate().is_err());
    }
    #[test]
    fn right_aligned_metadata_supports_dense_numeric_tables_within_the_node_budget() {
        let mut surface = Surface::new(200, 40, "#000000".into());
        for _ in 0..300 {
            surface.text_right(PixelRect::new(0, 0, 80, 20), "12", "#ffffff", 12, false);
            surface.text_centered(PixelRect::new(90, 0, 80, 20), "3", "#ffffff", 12, false);
        }
        surface.validate().unwrap();
        surface.right_aligned_text.resize(4097, 0);
        assert!(surface.validate().is_err());
    }
    #[test]
    fn retaining_nodes_remaps_decorations_and_drops_removed_targets() {
        let mut s = Surface::new(200, 120, "#000000".into());
        s.fill(PixelRect::new(0, 0, 200, 120), "#101010", 0);
        s.text_centered(
            PixelRect::new(10, 7, 100, 24),
            "Header",
            "#ffffff",
            12,
            true,
        );
        s.text_centered(
            PixelRect::new(10, 70, 100, 24),
            "Album",
            "#ffffff",
            12,
            true,
        );
        for node in [1, 2] {
            s.text_rules.push(TextRule {
                node,
                end: 190,
                gap: 8,
                width: 1,
                color: "#ffffff".into(),
            });
        }
        s.hover_fills.push(HoverFill {
            node: 0,
            color: "#333333".into(),
        });
        s.retain_nodes(|node| node.rect().y < 32 && node.rect().height <= 32);
        s.height = 38;
        assert_eq!(s.centered_text, [0]);
        assert_eq!(s.text_rules.len(), 1);
        assert_eq!(s.text_rules[0].node, 0);
        assert!(s.hover_fills.is_empty());
        s.validate().unwrap();
    }
    #[test]
    fn text_rules_validate_targets_and_scale_with_density() {
        let mut s = Surface::new(200, 30, "#000000".into());
        s.text(PixelRect::new(10, 0, 100, 30), "Album", "#ffffff", 12, true);
        s.text_rules.push(TextRule {
            node: 0,
            end: 190,
            gap: 10,
            width: 1,
            color: "#808080".into(),
        });
        s.validate().unwrap();
        let doubled = s.clone().at_density(2).unwrap();
        assert_eq!(
            (
                doubled.text_rules[0].end,
                doubled.text_rules[0].gap,
                doubled.text_rules[0].width
            ),
            (380, 20, 2)
        );
        s.text_rules[0].node = 1;
        assert!(s.validate().is_err());
        s.text_rules[0].node = 0;
        s.text_rules[0].end = 201;
        assert!(s.validate().is_err());
    }
    #[test]
    fn full_screen_surface_coordinates_match_the_viewport_budget() {
        assert!(Surface::new(3840, 2160, "#000000".into())
            .validate()
            .is_ok());
        assert!(Surface::new(7680, 4320, "#000000".into())
            .validate()
            .is_err());
        assert!(Surface::new(8193, 1, "#000000".into()).validate().is_err());
    }
    #[test]
    fn density_scales_layout_text_and_hits_without_scaling_source_art() {
        let mut s = Surface::new(40, 20, "#000000".into());
        let id = format!("skin/{}", "a".repeat(64));
        s.assets.insert(id.clone(), None);
        s.nodes.push(Primitive::Sprite {
            rect: PixelRect::new(2, 3, 20, 10),
            asset: id,
            source: PixelRect::new(0, 0, 6, 6),
            insets: Some([2, 2, 2, 2]),
            tint: None,
        });
        s.text(PixelRect::new(0, 0, 10, 10), "Hi", "#ffffff", 10, false);
        s.hits.push(HitRegion {
            rect: PixelRect::new(2, 3, 20, 10),
            action: "open".into(),
        });
        let double = s.at_density(2).unwrap();
        assert_eq!(double.width, 80);
        assert_eq!(double.hits[0].rect, PixelRect::new(4, 6, 40, 20));
        assert_eq!(double.nodes[0].rect(), double.hits[0].rect);
        assert!(
            matches!(&double.nodes[0],Primitive::Sprite{source,insets:Some([2,2,2,2]),..} if source.width==6)
        );
        assert!(matches!(&double.nodes[1], Primitive::Text { size: 20, .. }));
        assert!(double.at_density(0).is_err());
    }
    #[cfg(feature = "terminal-graphics")]
    #[test]
    fn legacy_surfaces_default_to_no_assets_and_sprite_metadata_is_explicit() {
        let legacy = r##"{"width":20,"height":20,"background":"#000000","nodes":[],"hits":[]}"##;
        let mut surface: Surface = serde_json::from_str(legacy).unwrap();
        assert!(!surface.ui_font);
        assert!(surface.assets.is_empty());
        let id = format!("skin/{}", "a".repeat(64));
        surface.assets.insert(id.clone(), None);
        surface.nodes.push(Primitive::Sprite {
            rect: PixelRect::new(0, 0, 20, 20),
            asset: id,
            source: PixelRect::new(0, 0, 3, 3),
            insets: None,
            tint: None,
        });
        assert!(surface.validate().is_err()); // no implicit artwork resampling
        if let Primitive::Sprite { insets, .. } = &mut surface.nodes[0] {
            *insets = Some([1, 1, 1, 1]);
        }
        surface.validate().unwrap();
        if let Primitive::Sprite { insets, .. } = &mut surface.nodes[0] {
            *insets = Some([2, 1, 1, 1]);
        }
        assert!(surface.validate().is_err());
        surface.assets.clear();
        assert!(surface.validate().is_err());
    }
    #[test]
    fn paths_refuse_unbounded_points_and_strokes() {
        let mut s = Surface::new(100, 60, "#112233".into());
        s.nodes.push(Primitive::Path {
            rect: PixelRect::new(10, 10, 80, 40),
            points: vec![[0, 0], [79, 39]],
            color: "#ffffff".into(),
            width: 1,
        });
        assert!(s.validate().is_ok());
        let Primitive::Path { points, .. } = &mut s.nodes[0] else {
            unreachable!()
        };
        points.push([80, 40]);
        assert!(s.validate().is_err());
    }
    #[test]
    fn bounds_and_shared_hit_geometry() {
        let mut s = Surface::new(200, 100, "#112233".into());
        let rect = PixelRect::new(10, 10, 30, 30);
        s.fill(rect, "#ffffff", 4);
        s.hits.push(HitRegion {
            rect,
            action: "play".into(),
        });
        assert!(s.validate().is_ok());
        assert_eq!(s.hit(10, 10).unwrap().action, "play");
        assert!(s.hit(40, 40).is_none());
        s.fill(PixelRect::new(190, 0, 20, 1), "#ffffff", 0);
        assert!(s.validate().is_err());
    }
    #[test]
    fn shaped_caret_offsets_validate_and_remap_with_retained_nodes() {
        let mut surface = Surface::new(200, 40, "#000000".into()).with_ui_text();
        surface.fill(PixelRect::new(0, 0, 200, 40), "#101010", 0);
        surface.text(
            PixelRect::new(4, 4, 192, 32),
            "日本語",
            "#ffffff",
            12,
            false,
        );
        surface.text_carets.push(TextCaret {
            node: 1,
            byte: 3,
            color: "#ffffff".into(),
        });
        surface.validate().unwrap();
        surface.retain_nodes(|node| matches!(node, Primitive::Text { .. }));
        assert_eq!(surface.text_carets[0].node, 0);
        surface.validate().unwrap();
        surface.text_carets[0].byte = 1;
        assert!(surface.validate().is_err());
    }
}
