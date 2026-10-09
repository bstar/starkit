//! Bounded, reusable native surfaces. Coordinates are pixels local to the surface.
//! Hosts place the surface; its owner retains layout and interaction semantics.
use serde::{Deserialize, Serialize};

pub mod classic;
#[cfg(feature = "image")]
pub mod skin;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
            nodes: vec![],
            hits: vec![],
        }
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
