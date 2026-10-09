//! Bounded, reusable native surfaces. Coordinates are pixels local to the surface.
//! Hosts place the surface; its owner retains layout and interaction semantics.
use serde::{Deserialize, Serialize};

pub mod classic;
#[cfg(feature = "image")]
pub mod skin;

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
    pub fn rect(&self) -> PixelRect {
        match self {
            Self::Fill { rect, .. }
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Surface {
    pub width: u16,
    pub height: u16,
    pub background: String,
    /// Fixed editor grid, in source pixels. Ordinary surfaces use shaped text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell_size: Option<[u16; 2]>,
    /// Content-addressed PNG payloads; None references artwork cached by the frontend.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub assets: std::collections::BTreeMap<String, Option<String>>,
    pub nodes: Vec<Primitive>,
    pub hits: Vec<HitRegion>,
}
impl Surface {
    pub fn new(width: u16, height: u16, background: String) -> Self {
        Self {
            width,
            height,
            background,
            cell_size: None,
            assets: Default::default(),
            nodes: vec![],
            hits: vec![],
        }
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
                Primitive::Sprite { rect: r, .. } | Primitive::Icon { rect: r, .. } => rect(r)?,
            }
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
                && u32::from(self.width) * u32::from(self.height) <= 8_000_000,
            "invalid surface size"
        );
        anyhow::ensure!(
            self.nodes.len() <= 4096 && self.hits.len() <= 256,
            "surface exceeds limits"
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
        anyhow::ensure!(color(&self.background), "invalid surface background");
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
}
