//! Pixel placement of controller regions. Rendering and cell-pointer projection
//! use the same rectangles; spacing is independent of the terminal grid.
use super::protocol::{Component, Rect, Scene, Span, Viewport};
use crate::native_surface::PixelRect;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelPadding {
    pub inset: u16,
    pub gap: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub source: Rect,
    pub target: PixelRect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padding: Option<PanelPadding>,
    /// An overlay carries its own spans, keeping the underlying scene intact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay: Option<Vec<Span>>,
}
impl Placement {
    pub fn new(source: Rect, target: PixelRect) -> Self {
        Self {
            source,
            target,
            overlay: None,
            padding: None,
        }
    }
    pub fn validate(&self, viewport: Viewport) -> anyhow::Result<()> {
        let s = self.source;
        let t = self.target;
        anyhow::ensure!(
            s.width > 0
                && s.height > 0
                && t.width > 0
                && t.height > 0
                && u32::from(s.x) + u32::from(s.width) <= u32::from(viewport.columns)
                && u32::from(s.y) + u32::from(s.height) <= u32::from(viewport.rows)
                && u32::from(t.x) + u32::from(t.width) <= viewport.width
                && u32::from(t.y) + u32::from(t.height) <= viewport.height,
            "Invalid pixel placement"
        );
        anyhow::ensure!(
            self.overlay
                .as_ref()
                .is_none_or(|s| s.len() <= super::protocol::MAX_CELLS),
            "Overlay exceeds limits"
        );
        Ok(())
    }
    /// Map a terminal cell's centre into the controller. A captured scrollbar
    /// may clamp outside its region; ordinary input never hits a gutter.
    pub fn pointer(&self, viewport: Viewport, x: u16, y: u16, clamp: bool) -> Option<(u16, u16)> {
        if viewport.columns == 0
            || viewport.rows == 0
            || self.target.width == 0
            || self.target.height == 0
        {
            return None;
        }
        let px = (f64::from(x) + 0.5) * f64::from(viewport.width) / f64::from(viewport.columns);
        let py = (f64::from(y) + 0.5) * f64::from(viewport.height) / f64::from(viewport.rows);
        self.pointer_position(px, py, clamp)
    }
    /// Use the unrounded terminal position when the client supplies one.
    pub fn pointer_pixels(&self, x: u32, y: u32, clamp: bool) -> Option<(u16, u16)> {
        self.pointer_position(f64::from(x), f64::from(y), clamp)
    }
    fn pointer_position(&self, px: f64, py: f64, clamp: bool) -> Option<(u16, u16)> {
        if self.target.width == 0 || self.target.height == 0 || self.source.height == 0 {
            return None;
        }
        let t = self.target;
        if !clamp
            && (px < f64::from(t.x)
                || py < f64::from(t.y)
                || px >= f64::from(t.x) + f64::from(t.width)
                || py >= f64::from(t.y) + f64::from(t.height))
        {
            return None;
        }
        let axis = |p: f64, start: u16, pixels: u16, cells: u16| {
            ((p - f64::from(start)) * f64::from(cells) / f64::from(pixels))
                .floor()
                .clamp(0., f64::from(cells.saturating_sub(1))) as u16
        };
        Some((
            self.source.x + axis(px, t.x, t.width, self.source.width),
            self.source.y
                + (0..self.source.height)
                    .find(|row| py - f64::from(t.y) < f64::from(self.row_edge(row + 1)))
                    .unwrap_or(self.source.height - 1),
        ))
    }
    /// Physical row edges, including compact panel border/header padding.
    /// Non-content padding rows retain their logical addresses for old controllers.
    pub fn row_edge(&self, row: u16) -> f32 {
        let rows = self.source.height;
        let height = f32::from(self.target.height);
        let row = row.min(rows);
        if let Some(padding) = &self.padding {
            if rows >= 6 {
                let inset = f32::from(padding.inset).min(height / 8.);
                let gap = f32::from(padding.gap).min(height / 8.);
                let unit = (height - 2. * inset - gap) / f32::from(rows - 3);
                return match row {
                    0 => 0.,
                    1 => inset,
                    2 => inset + unit,
                    n if n == rows => height,
                    n => inset + gap + f32::from(n - 2) * unit,
                };
            }
        }
        height * f32::from(row) / f32::from(rows.max(1))
    }

    pub fn project(&self, scene: &Scene) -> Scene {
        self.project_after(scene, 0)
    }
    /// Project foreground components without losing their original surface ownership.
    pub(crate) fn project_after(&self, scene: &Scene, first: usize) -> Scene {
        use unicode_width::UnicodeWidthChar;
        let source = self.source;
        let spans: Vec<_> = self
            .overlay
            .as_ref()
            .unwrap_or(&scene.spans)
            .iter()
            .filter_map(|span| {
                if first > 0 && self.overlay.is_none() {
                    return None;
                }
                if span.y < source.y || span.y >= source.y + source.height {
                    return None;
                }
                let mut column = u32::from(span.x);
                let mut text = String::new();
                let mut start = None;
                for c in span.text.chars() {
                    let width = c.width().unwrap_or(0) as u32;
                    if column >= u32::from(source.x)
                        && column + width <= u32::from(source.x) + u32::from(source.width)
                    {
                        start.get_or_insert(column as u16);
                        text.push(c);
                    }
                    column += width;
                }
                start.map(|x| Span {
                    x: x - source.x,
                    y: span.y - source.y,
                    text,
                    foreground: span.foreground.clone(),
                    background: span.background.clone(),
                    bold: span.bold,
                    modifiers: 0,
                })
            })
            .collect();
        // Modal components follow the first Menu/Dialog marker. This keeps
        // popup controls (including scrollbars) out of background regions.
        let overlay_start = scene
            .components
            .iter()
            .position(|c| matches!(c, Component::Menu { .. } | Component::Dialog { .. }))
            .unwrap_or(scene.components.len());
        let components: Vec<_> = scene
            .components
            .iter()
            .enumerate()
            .filter_map(|(index, c)| {
                if index < first {
                    return None;
                }
                let overlay = index >= overlay_start;
                if overlay != self.overlay.is_some() {
                    return None;
                }
                // Mounted native surfaces own physical pixel placements. An
                // enclosing pane must not rasterize them again using its own
                // cell transform (for example, a floating desktop dock).
                if let Component::Surface { rect, surface } = c {
                    let matches = |p: &&Placement| {
                        p.source == *rect
                            && p.target.width == surface.width
                            && p.target.height == surface.height
                    };
                    let ordinal = scene.components[..index]
                        .iter()
                        .filter(|other| {
                            matches!(other, Component::Surface { rect: r, surface: s }
                            if r == rect && s.width == surface.width && s.height == surface.height)
                        })
                        .count();
                    if scene
                        .placements
                        .iter()
                        .filter(matches)
                        .nth(ordinal)
                        .is_some_and(|owner| owner != self)
                    {
                        return None;
                    }
                }
                if !encloses(source, c.rect()) {
                    return None;
                }
                let mut c = c.clone();
                let rect = component_rect_mut(&mut c);
                rect.x -= source.x;
                rect.y -= source.y;
                match &mut c {
                    Component::Tab { close: Some(r), .. }
                    | Component::Scrollbar { thumb: r, .. } => {
                        r.x = r.x.saturating_sub(source.x);
                        r.y = r.y.saturating_sub(source.y);
                    }
                    _ => {}
                }
                Some(c)
            })
            .collect();
        let transparent = spans.is_empty()
            && components.iter().any(|c| {
                matches!(c,
            Component::Surface { surface, .. } if surface.background == "transparent")
            })
            && components.iter().all(|c| {
                matches!(c,
                Component::Surface {surface,..} if surface.background == "transparent")
                    || matches!(c, Component::Menu { .. } | Component::Dialog { .. })
            });
        Scene {
            viewport: Viewport {
                columns: source.width,
                rows: source.height,
                width: u32::from(self.target.width),
                height: u32::from(self.target.height),
                ..scene.viewport
            },
            spans,
            components,
            placements: Vec::new(),
            resize_handles: vec![],
            pointer_regions: vec![],
            revision: scene.revision,
            interaction: scene.interaction,
            scroll_interaction: None,
            background: if transparent {
                "transparent".into()
            } else {
                scene.background.clone()
            },
            foreground: scene.foreground.clone(),
            accent: scene.accent.clone(),
            border: scene.border.clone(),
        }
    }
}
pub(crate) fn encloses(outer: Rect, inner: Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && u32::from(inner.x) + u32::from(inner.width)
            <= u32::from(outer.x) + u32::from(outer.width)
        && u32::from(inner.y) + u32::from(inner.height)
            <= u32::from(outer.y) + u32::from(outer.height)
}
fn component_rect_mut(c: &mut Component) -> &mut Rect {
    match c {
        Component::LiveCanvas { rect, .. }
        | Component::Surface { rect, .. }
        | Component::Menu { rect }
        | Component::Dialog { rect, .. }
        | Component::TextField { rect, .. }
        | Component::Panel { rect, .. }
        | Component::ListRow { rect, .. }
        | Component::Tab { rect, .. }
        | Component::Meter { rect, .. }
        | Component::Scrollbar { rect, .. }
        | Component::Image { rect, .. }
        | Component::Terminal { rect } => rect,
    }
}

/// Divide a pixel region horizontally with an exact gap (including odd widths).
pub fn split(area: PixelRect, gap: u16) -> [PixelRect; 2] {
    let gap = gap.min(area.width);
    let left = (area.width - gap) / 2;
    [
        PixelRect::new(area.x, area.y, left, area.height),
        PixelRect::new(
            area.x + left + gap,
            area.y,
            area.width - gap - left,
            area.height,
        ),
    ]
}

/// A column with one flexible row. Fixed rows retain their requested height
/// when possible; undersized viewports reduce all rows proportionally.
pub fn column(area: PixelRect, gap: u16, heights: &[u16], flexible: usize) -> Vec<PixelRect> {
    if heights.is_empty() {
        return Vec::new();
    }
    let gap = gap.min(area.height / heights.len() as u16);
    let available = area.height.saturating_sub(gap * (heights.len() as u16 - 1));
    let fixed: u32 = heights
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != flexible)
        .map(|(_, h)| u32::from(*h))
        .sum();
    let mut sizes = heights.to_vec();
    if flexible < sizes.len() {
        sizes[flexible] = available
            .saturating_sub(fixed.min(u32::from(u16::MAX)) as u16)
            .max(1);
    }
    let total: u32 = sizes.iter().map(|h| u32::from(*h)).sum();
    let mut y = area.y;
    let mut consumed = 0u32;
    sizes
        .iter()
        .enumerate()
        .map(|(i, h)| {
            consumed += u32::from(*h);
            let end = if total > u32::from(available) {
                u32::from(available) * consumed / total
            } else {
                consumed
            } as u16;
            let start = y - area.y - gap * i as u16;
            let rect = PixelRect::new(area.x, y, area.width, end.saturating_sub(start));
            y = y.saturating_add(rect.height).saturating_add(gap);
            rect
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn floating_surfaces_are_not_repainted_inside_the_enclosing_desktop() {
        use crate::native_surface::{style::Style, Surface};
        for (width, height, columns, rows) in [
            (1200, 800, 100, 40),
            (1301, 803, 97, 41),
            (796, 482, 88, 28),
        ] {
            let v = Viewport {
                width,
                height,
                columns,
                rows,
                ..Viewport::default()
            };
            let style = Style::default();
            let mut scene = style.scene(v);
            scene.background = "#101010".into();
            let base = PixelRect::new(8, 80, width as u16 - 16, height as u16 - 110);
            let dock = PixelRect::new((width as u16 - 440) / 2, height as u16 - 81, 440, 42);
            style
                .mount(
                    &mut scene,
                    Surface::new(base.width, base.height, "#202020".into()),
                    base.x,
                    base.y,
                )
                .unwrap();
            style
                .mount(
                    &mut scene,
                    Surface::new(dock.width, dock.height, "#f000ff".into()),
                    dock.x,
                    dock.y,
                )
                .unwrap();
            for placement in &scene.placements {
                let projected = placement.project(&scene);
                assert_eq!(
                    projected
                        .components
                        .iter()
                        .filter(|c| matches!(c, Component::Surface { .. }))
                        .count(),
                    1
                );
            }
            let image = super::super::renderer::render_reference(&scene).unwrap();
            for (x, y, pixel) in image.enumerate_pixels() {
                let expected = if dock.contains(x as u16, y as u16) {
                    [240, 0, 255, 255]
                } else if base.contains(x as u16, y as u16) {
                    [32, 32, 32, 255]
                } else {
                    [16, 16, 16, 255]
                };
                assert_eq!(
                    pixel.0, expected,
                    "Shifted duplicate at {x},{y} in {width}x{height}"
                );
            }
        }
    }
    #[test]
    fn subcell_surfaces_with_matching_logical_bounds_keep_distinct_owners() {
        use crate::native_surface::{style::Style, Surface};
        let style = Style::default();
        let mut scene = style.scene(Viewport::default());
        style
            .mount(&mut scene, Surface::new(1200, 800, "#202020".into()), 0, 0)
            .unwrap();
        style
            .mount(&mut scene, Surface::new(4, 4, "#ff0000".into()), 14, 21)
            .unwrap();
        style
            .mount(&mut scene, Surface::new(4, 4, "#00ff00".into()), 17, 24)
            .unwrap();
        assert_eq!(scene.placements[1].source, scene.placements[2].source);
        for (index, color) in ["#202020", "#ff0000", "#00ff00"].into_iter().enumerate() {
            let projected = scene.placements[index].project(&scene);
            assert_eq!(projected.components.len(), 1);
            assert!(
                matches!(&projected.components[0],Component::Surface{surface,..} if surface.background==color)
            );
        }
    }
    #[test]
    fn surfaces_without_an_independent_placement_still_project_into_the_parent() {
        use crate::native_surface::{style::Style, Surface};
        let style = Style::default();
        let mut scene = style.scene(Viewport::default());
        style
            .mount(&mut scene, Surface::new(1200, 800, "#202020".into()), 0, 0)
            .unwrap();
        scene.components.push(Component::Surface {
            rect: Rect {
                x: 1,
                y: 1,
                width: 2,
                height: 2,
            },
            surface: Surface::new(24, 40, "#ff0000".into()),
        });
        assert_eq!(scene.placements[0].project(&scene).components.len(), 2);
    }
    #[test]
    fn panel_padding_uses_the_same_edges_for_paint_and_input() {
        for (cw, ch) in [(8, 16), (10, 20), (17, 34)] {
            let v = Viewport {
                columns: 100,
                rows: 40,
                width: 100 * cw,
                height: 40 * ch,
                generation: 1,
            };
            let metrics = crate::native_surface::Metrics::from_cell(cw as u16, ch as u16);
            let mut p = Placement::new(
                Rect {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 40,
                },
                PixelRect::new(0, 0, v.width as u16, v.height as u16),
            );
            p.padding = Some(PanelPadding {
                inset: metrics.inset,
                gap: metrics.gap,
            });
            assert_eq!(p.row_edge(1), f32::from(metrics.inset));
            assert!((p.row_edge(3) - p.row_edge(2) - f32::from(metrics.gap)).abs() < 0.001);
            assert!((p.row_edge(40) - p.row_edge(39) - f32::from(metrics.inset)).abs() < 0.001);
            for row in 0..v.rows {
                let (_, logical) = p.pointer(v, 10, row, false).unwrap();
                let centre = (f32::from(row) + 0.5) * ch as f32;
                assert!(centre >= p.row_edge(logical) && centre < p.row_edge(logical + 1));
            }
        }
    }

    #[test]
    fn exact_gaps_bounds_and_pointer_projection() {
        for (cw, ch) in [(8, 16), (10, 20), (17, 34)] {
            let v = Viewport {
                columns: 100,
                rows: 40,
                width: 100 * cw + 3,
                height: 40 * ch + 1,
                generation: 1,
            };
            let metrics = crate::native_surface::Metrics::from_cell(cw as u16, ch as u16);
            let area = PixelRect::new(8, 8, v.width as u16 - 16, v.height as u16 - 16);
            let rows = column(area, metrics.gap, &[32, 0, 160, 32], 1);
            assert_eq!(
                rows.last().unwrap().y + rows.last().unwrap().height,
                area.y + area.height
            );
            for pair in rows.windows(2) {
                assert_eq!(pair[1].y - pair[0].y - pair[0].height, metrics.gap);
            }
            let panes = split(rows[1], metrics.gap);
            assert_eq!(panes[1].x - panes[0].x - panes[0].width, metrics.gap);
            let p = Placement::new(
                Rect {
                    x: 2,
                    y: 4,
                    width: 45,
                    height: 20,
                },
                panes[0],
            );
            p.validate(v).unwrap();
            assert!(p.pointer(v, 0, 0, false).is_none());
            assert_eq!(p.pointer(v, 99, 39, true), Some((46, 23)));
        }
    }
    #[test]
    fn rejects_outside_source_and_target() {
        let v = Viewport::default();
        assert!(Placement::new(
            Rect {
                x: 99,
                y: 0,
                width: 2,
                height: 1
            },
            PixelRect::new(0, 0, 10, 10)
        )
        .validate(v)
        .is_err());
        assert!(Placement::new(
            Rect {
                x: 0,
                y: 0,
                width: 2,
                height: 1
            },
            PixelRect::new(1199, 0, 10, 10)
        )
        .validate(v)
        .is_err());
    }
}
