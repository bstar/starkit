//! Local pointer feedback: never wait for the remote controller on hover.
use crate::native_surface::PixelRect;
use std::io::Write;

#[derive(Default)]
pub(super) struct Pointer {
    resizing: bool,
    pointing: bool,
    captured: bool,
}
impl Pointer {
    pub fn update(
        &mut self,
        action: &str,
        left: bool,
        pixel: [u32; 2],
        handles: &[PixelRect],
        clickable: bool,
        out: &mut impl Write,
    ) -> std::io::Result<()> {
        let hit = handles.iter().any(|r| {
            pixel[0] >= u32::from(r.x)
                && pixel[0] < u32::from(r.x) + u32::from(r.width)
                && pixel[1] >= u32::from(r.y)
                && pixel[1] < u32::from(r.y) + u32::from(r.height)
        });
        if action == "down" && left {
            self.captured = hit;
        }
        if action == "up" {
            self.captured = false;
        }
        self.set(self.captured || hit, clickable, out)
    }
    fn set(&mut self, resizing: bool, pointing: bool, out: &mut impl Write) -> std::io::Result<()> {
        let pointing = pointing && !resizing;
        if resizing != self.resizing || pointing != self.pointing {
            out.write_all(if resizing {
                b"\x1b]22;ns-resize\x1b\\"
            } else if pointing {
                b"\x1b]22;pointer\x1b\\"
            } else {
                b"\x1b]22;\x1b\\"
            })?;
            out.flush()?;
            self.resizing = resizing;
            self.pointing = pointing;
        }
        Ok(())
    }
    pub fn reset(&mut self, out: &mut impl Write) -> std::io::Result<()> {
        self.captured = false;
        self.set(false, false, out)
    }
}
impl Drop for Pointer {
    fn drop(&mut self) {
        let _ = self.reset(&mut std::io::stdout().lock());
    }
}
pub(super) fn clickable_regions(scene: &super::protocol::Scene) -> Vec<super::protocol::Rect> {
    use super::protocol::{Component, Rect};
    let mut regions = scene.pointer_regions.clone();
    if scene
        .components
        .iter()
        .any(|c| matches!(c, Component::Menu { .. } | Component::Dialog { .. }))
    {
        return regions;
    }
    for component in &scene.components {
        if let Component::Surface { rect, surface } = component {
            for hit in &surface.hits {
                let left =
                    u32::from(hit.rect.x) * u32::from(rect.width) / u32::from(surface.width.max(1));
                let top = u32::from(hit.rect.y) * u32::from(rect.height)
                    / u32::from(surface.height.max(1));
                let right = (u32::from(hit.rect.x) + u32::from(hit.rect.width))
                    * u32::from(rect.width)
                    / u32::from(surface.width.max(1));
                let bottom = (u32::from(hit.rect.y) + u32::from(hit.rect.height))
                    * u32::from(rect.height)
                    / u32::from(surface.height.max(1));
                if right > left && bottom > top {
                    regions.push(Rect {
                        x: rect.x.saturating_add(left as u16),
                        y: rect.y.saturating_add(top as u16),
                        width: (right - left) as u16,
                        height: (bottom - top) as u16,
                    });
                }
            }
        }
    }
    regions
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HoverTarget {
    Row(super::protocol::Rect),
    Surface { component: usize, fill: usize },
}
/// Native surface pixel geometry is shared by hover and embedded input routing.
pub(super) fn surface_point(
    scene: &super::protocol::Scene,
    component: usize,
    point: [f64; 2],
) -> Option<[u16; 2]> {
    use super::protocol::Component;
    let Component::Surface { rect, surface } = scene.components.get(component)? else {
        return None;
    };
    let ordinal=scene.components[..component].iter().filter(|c|matches!(c,Component::Surface{rect:r,surface:s} if r==rect && s.width==surface.width && s.height==surface.height)).count();
    let target = if let Some(p) = scene
        .placements
        .iter()
        .filter(|p| {
            p.source == *rect
                && p.target.width == surface.width
                && p.target.height == surface.height
        })
        .nth(ordinal)
    {
        p.target
    } else if scene.placements.is_empty() {
        let v = scene.viewport;
        let x = u32::from(rect.x) * v.width / u32::from(v.columns.max(1));
        let y = u32::from(rect.y) * v.height / u32::from(v.rows.max(1));
        let right =
            (u32::from(rect.x) + u32::from(rect.width)) * v.width / u32::from(v.columns.max(1));
        let bottom =
            (u32::from(rect.y) + u32::from(rect.height)) * v.height / u32::from(v.rows.max(1));
        crate::native_surface::PixelRect::new(
            x as u16,
            y as u16,
            (right - x) as u16,
            (bottom - y) as u16,
        )
    } else {
        return None;
    };
    if target.width == 0
        || target.height == 0
        || point[0] < f64::from(target.x)
        || point[1] < f64::from(target.y)
        || point[0] >= f64::from(target.x) + f64::from(target.width)
        || point[1] >= f64::from(target.y) + f64::from(target.height)
    {
        return None;
    }
    Some([
        ((point[0] - f64::from(target.x)) * f64::from(surface.width) / f64::from(target.width))
            as u16,
        ((point[1] - f64::from(target.y)) * f64::from(surface.height) / f64::from(target.height))
            as u16,
    ])
}
pub(super) fn hovered(
    scene: &super::protocol::Scene,
    logical: Option<(u16, u16)>,
    pixel: Option<[u32; 2]>,
) -> Option<HoverTarget> {
    use super::protocol::Component;
    let modal = scene
        .components
        .iter()
        .rposition(|c| matches!(c, Component::Menu { .. } | Component::Dialog { .. }));
    if let Some(point) = pixel {
        for (component, c) in scene.components.iter().enumerate().rev() {
            if modal.is_some_and(|index| component <= index) {
                break;
            }
            let Component::Surface { surface, .. } = c else {
                continue;
            };
            let Some([x, y]) = surface_point(scene, component, point.map(f64::from)) else {
                continue;
            };
            if let Some(fill) = surface.hover_fills.iter().rposition(|h| {
                surface
                    .nodes
                    .get(usize::from(h.node))
                    .is_some_and(|n| n.rect().contains(x, y))
            }) {
                return Some(HoverTarget::Surface { component, fill });
            }
        }
    }
    if modal.is_some() {
        None
    } else {
        hovered_row(scene, logical).map(HoverTarget::Row)
    }
}
pub(super) fn paint_hover(
    scene: &super::protocol::Scene,
    hover: Option<HoverTarget>,
) -> super::protocol::Scene {
    use super::protocol::Component;
    use crate::native_surface::Primitive;
    if let Some(HoverTarget::Row(rect)) = hover {
        return hover_scene(scene, Some(rect));
    }
    let mut result = scene.clone();
    if let Some(HoverTarget::Surface { component, fill }) = hover {
        if let Some(Component::Surface { surface, .. }) = result.components.get_mut(component) {
            if let Some(h) = surface.hover_fills.get(fill) {
                if let Some(Primitive::Fill { color, .. }) =
                    surface.nodes.get_mut(usize::from(h.node))
                {
                    color.clone_from(&h.color);
                }
            }
        }
    }
    result
}

// Hover is presentation-only: no controller selection or remote round trip.
pub(super) fn hovered_row(
    scene: &super::protocol::Scene,
    point: Option<(u16, u16)>,
) -> Option<super::protocol::Rect> {
    use super::protocol::Component;
    let (x, y) = point?;
    if scene
        .components
        .iter()
        .any(|c| matches!(c, Component::Menu { .. } | Component::Dialog { .. }))
    {
        return None;
    }
    scene.components.iter().find_map(|c| {
        let Component::ListRow { rect, selected, .. } = c else {
            return None;
        };
        let row = scene
            .pointer_regions
            .iter()
            .find(|r| r.x == rect.x && r.y == rect.y && r.height == 1 && r.width >= rect.width)
            .unwrap_or(rect);
        (!selected && row.contains(x, y)).then_some(*row)
    })
}
pub(super) fn hover_scene(
    scene: &super::protocol::Scene,
    hover: Option<super::protocol::Rect>,
) -> super::protocol::Scene {
    use super::protocol::Component;
    use crate::theme::color::Rgb;
    let mut result = scene.clone();
    let Some(rect) = hover else {
        return result;
    };
    let tint = |background: &str| -> String {
        let Ok(bg) = Rgb::parse_hex(background) else {
            return background.into();
        };
        let Ok(fg) = Rgb::parse_hex(&scene.foreground) else {
            return background.into();
        };
        let mix = |b: u8, f: u8| ((u16::from(b) * 19 + u16::from(f)) / 20) as u8;
        format!(
            "#{:02x}{:02x}{:02x}",
            mix(bg.r, fg.r),
            mix(bg.g, fg.g),
            mix(bg.b, fg.b)
        )
    };
    for c in &mut result.components {
        if let Component::ListRow {
            rect: row,
            background,
            ..
        } = c
        {
            if row.y == rect.y && row.x == rect.x {
                *background = tint(background);
            }
        }
    }
    for span in &mut result.spans {
        if rect.contains(span.x, span.y) {
            span.background = tint(&span.background);
        }
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    fn row_scene() -> super::super::protocol::Scene {
        use super::super::protocol::{Component, Rect, Scene, Span, Viewport};
        let mut scene = Scene::from_buffer(
            &crate::ratatui::buffer::Buffer::empty(crate::ratatui::layout::Rect::new(0, 0, 40, 10)),
            Viewport {
                columns: 40,
                rows: 10,
                width: 400,
                height: 200,
                generation: 1,
            },
            1,
        );
        scene.foreground = "#ffffff".into();
        scene.components.push(Component::ListRow {
            rect: Rect {
                x: 1,
                y: 2,
                width: 10,
                height: 1,
            },
            label: "file".into(),
            icon: "file".into(),
            foreground: "#ffffff".into(),
            background: "#202020".into(),
            selected: false,
            marked: true,
            marking: true,
        });
        scene.pointer_regions.push(Rect {
            x: 1,
            y: 2,
            width: 25,
            height: 1,
        });
        scene.spans.push(Span {
            x: 14,
            y: 2,
            text: "10 KB".into(),
            foreground: "#ffffff".into(),
            background: "#202020".into(),
            bold: false,
            modifiers: 0,
        });
        scene
    }
    #[test]
    fn placed_workspace_menus_have_precise_local_hover() {
        use super::super::protocol::Component;
        use crate::native_surface::{
            style::{workspace::ContextItem, Style},
            PixelRect,
        };
        let style = Style::default().with_soft_workspace();
        let mut scene = style.scene(super::super::protocol::Viewport::default());
        scene.components.push(Component::Menu {
            rect: super::super::protocol::Rect {
                x: 0,
                y: 0,
                width: 30,
                height: 10,
            },
        });
        style
            .mount(
                &mut scene,
                style.context_menu(
                    302,
                    "DESKTOP",
                    &[ContextItem::new("Apps", "", "launch")],
                    None,
                ),
                77,
                91,
            )
            .unwrap();
        assert_eq!(surface_point(&scene, 1, [91., 135.]), Some([14, 44]));
        let target = hovered(&scene, None, Some([91, 135]));
        assert!(matches!(
            target,
            Some(HoverTarget::Surface { component: 1, .. })
        ));
        assert!(hovered(&scene, None, Some([76, 135])).is_none());
        assert_eq!(scene.placements[0].target, PixelRect::new(77, 91, 302, 76));
    }
    #[test]
    fn hover_covers_metadata_without_changing_marks_selection_or_base_scene() {
        use super::super::protocol::Component;
        let scene = row_scene();
        let row = hovered_row(&scene, Some((20, 2))).expect("Metadata is part of the row");
        let painted = hover_scene(&scene, Some(row));
        let Component::ListRow {
            background,
            selected,
            marked,
            ..
        } = &painted.components[0]
        else {
            panic!();
        };
        assert_eq!(background, "#2b2b2b");
        assert!(!selected && *marked);
        assert_eq!(painted.spans.last().unwrap().background, *background);
        assert_eq!(scene.spans.last().unwrap().background, "#202020");
        assert!(hovered_row(&scene, Some((26, 2))).is_none());
        assert!(hovered_row(&scene, None).is_none());
        let mut light = scene.clone();
        light.foreground = "#000000".into();
        light.spans.last_mut().unwrap().background = "#ffffff".into();
        assert_eq!(
            hover_scene(&light, Some(row))
                .spans
                .last()
                .unwrap()
                .background,
            "#f2f2f2"
        );
    }
    #[test]
    fn selected_rows_and_modal_backgrounds_keep_their_existing_appearance() {
        use super::super::protocol::{Component, Rect};
        let mut scene = row_scene();
        if let Component::ListRow { selected, .. } = &mut scene.components[0] {
            *selected = true;
        }
        assert!(hovered_row(&scene, Some((2, 2))).is_none());
        if let Component::ListRow { selected, .. } = &mut scene.components[0] {
            *selected = false;
        }
        scene.components.push(Component::Menu {
            rect: Rect {
                x: 0,
                y: 0,
                width: 30,
                height: 5,
            },
        });
        assert!(hovered_row(&scene, Some((2, 2))).is_none());
    }
    #[test]
    fn surface_actions_supply_pointer_regions_even_without_controller_hints() {
        use super::super::protocol::{Component, Rect};
        use crate::native_surface::{HitRegion, PixelRect, Surface};
        let mut scene = row_scene();
        scene.pointer_regions.clear();
        let mut surface = Surface::new(40, 20, "#202020".into());
        surface.hits.push(HitRegion {
            rect: PixelRect::new(20, 0, 20, 20),
            action: "open".into(),
        });
        scene.components.push(Component::Surface {
            rect: Rect {
                x: 3,
                y: 3,
                width: 4,
                height: 2,
            },
            surface,
        });
        let regions = clickable_regions(&scene);
        assert!(regions.iter().any(|r| r.contains(5, 3)));
        assert!(!regions.iter().any(|r| r.contains(3, 3)));
    }
    #[test]
    fn native_hover_uses_physical_rows_and_never_changes_base_or_selection() {
        use super::super::protocol::{Component, Rect};
        use crate::native_surface::{HoverFill, PixelRect, Primitive, Surface};
        for density in [1, 2] {
            let mut scene = row_scene();
            scene.components.clear();
            let mut surface = Surface::new(400, 200, "#101010".into());
            surface.fill(PixelRect::new(10, 21, 200, 22), "#101010", 0);
            surface.hover_fills.push(HoverFill {
                node: 0,
                color: "#303030".into(),
            });
            surface.fill(PixelRect::new(10, 43, 200, 22), "#555555", 0);
            // A selected row keeps its selection even under the pointer.
            surface.hover_fills.push(HoverFill {
                node: 1,
                color: "#555555".into(),
            });
            let surface = surface.at_density(density).unwrap();
            scene.viewport.width *= u32::from(density);
            scene.viewport.height *= u32::from(density);
            scene.components.push(Component::Surface {
                rect: Rect {
                    x: 0,
                    y: 0,
                    width: 40,
                    height: 10,
                },
                surface,
            });
            let first = hovered(
                &scene,
                None,
                Some([20 * u32::from(density), 42 * u32::from(density)]),
            );
            let second = hovered(
                &scene,
                None,
                Some([20 * u32::from(density), 43 * u32::from(density)]),
            );
            assert_ne!(
                first, second,
                "adjacent physical rows must not snap to cells"
            );
            let painted = paint_hover(&scene, first);
            let Component::Surface { surface: base, .. } = &scene.components[0] else {
                panic!()
            };
            let Component::Surface { surface: drawn, .. } = &painted.components[0] else {
                panic!()
            };
            assert!(matches!(&base.nodes[0], Primitive::Fill { color, .. } if color == "#101010"));
            assert!(matches!(&drawn.nodes[0], Primitive::Fill { color, .. } if color == "#303030"));
            assert_eq!(drawn.nodes[1], base.nodes[1]);
            assert_eq!(paint_hover(&scene, None), scene);
            assert_eq!(paint_hover(&scene, second), scene);
            scene.components.push(Component::Menu {
                rect: Rect {
                    x: 0,
                    y: 0,
                    width: 40,
                    height: 10,
                },
            });
            assert!(hovered(&scene, None, Some([20, 30])).is_none());
        }
        let mut invalid = Surface::new(40, 20, "#101010".into());
        invalid.hover_fills.push(HoverFill {
            node: 0,
            color: "#303030".into(),
        });
        assert!(invalid.validate().is_err());
        invalid.fill(PixelRect::new(0, 0, 40, 20), "#101010", 0);
        assert!(invalid.validate().is_ok());
        invalid.hover_fills[0].color = "bad".into();
        assert!(invalid.validate().is_err());
    }
    #[test]
    fn clickable_hover_uses_hand_and_resets_without_repeated_output() {
        let mut p = Pointer::default();
        let mut out = vec![];
        p.update("move", false, [1, 1], &[], true, &mut out)
            .unwrap();
        assert_eq!(out, b"\x1b]22;pointer\x1b\\");
        let len = out.len();
        p.update("move", false, [1, 1], &[], true, &mut out)
            .unwrap();
        assert_eq!(out.len(), len);
        p.update("move", false, [1, 1], &[], false, &mut out)
            .unwrap();
        assert!(out.ends_with(b"\x1b]22;\x1b\\"));
    }
    #[test]
    fn hover_capture_release_and_reset_use_fixed_sequences_only() {
        let mut p = Pointer::default();
        let mut out = vec![];
        let regions = [PixelRect::new(0, 10, 100, 16)];
        p.update("move", false, [50, 15], &regions, false, &mut out)
            .unwrap();
        assert_eq!(out, b"\x1b]22;ns-resize\x1b\\");
        p.update("down", true, [50, 15], &regions, false, &mut out)
            .unwrap();
        p.update("drag", true, [50, 90], &[], false, &mut out)
            .unwrap();
        assert!(p.resizing);
        assert_eq!(out.len(), b"\x1b]22;ns-resize\x1b\\".len());
        p.update("up", true, [50, 90], &regions, false, &mut out)
            .unwrap();
        assert!(!p.resizing);
        p.update("down", true, [50, 15], &regions, false, &mut out)
            .unwrap();
        p.reset(&mut out).unwrap();
        assert!(!p.captured && !p.resizing);
        assert!(out.ends_with(b"\x1b]22;\x1b\\"));
    }
}
