use super::{Align, Id, Node, View, Widget};
use crate::{panes, ratatui::layout::Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presentation {
    Cells,
    Graphics,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport {
    pub width: u16,
    pub height: u16,
    pub presentation: Presentation,
    pub generation: u64,
}
impl Viewport {
    pub fn cells(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            presentation: Presentation::Cells,
            generation: 1,
        }
    }
    pub fn graphics(width: u16, height: u16) -> Self {
        Self {
            presentation: Presentation::Graphics,
            ..Self::cells(width, height)
        }
    }
    pub(crate) fn scale(self, horizontal: bool) -> u16 {
        match self.presentation {
            Presentation::Cells => 1,
            Presentation::Graphics => {
                if horizontal {
                    8
                } else {
                    24
                }
            }
        }
    }
    pub(crate) fn gap(self) -> u16 {
        if self.presentation == Presentation::Cells {
            1
        } else {
            8
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnPlacement {
    pub area: Rect,
    pub align: Align,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    pub id: Id,
    pub parent: Option<Id>,
    pub area: Rect,
    pub body: Rect,
    pub widget: Widget,
    pub enabled: bool,
    pub focusable: bool,
    pub modal: bool,
    pub columns: Vec<ColumnPlacement>,
}

/// List text and its optional scrollbar in the resolved frame's coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListLayout {
    pub content: Rect,
    pub track: Option<Rect>,
    /// Complete data rows, excluding a table's heading.
    pub rows: usize,
}

impl Placement {
    pub fn list_layout(&self, viewport: Viewport) -> Option<ListLayout> {
        list_layout(&self.widget, self.body, viewport)
    }
}

fn list_layout(widget: &Widget, body: Rect, viewport: Viewport) -> Option<ListLayout> {
    let Widget::List { items, columns, .. } = widget else {
        return None;
    };
    let row = viewport.scale(false);
    let header = u16::from(!columns.is_empty()) * row;
    let rows = body.height.saturating_sub(header) / row;
    let width = viewport.scale(true);
    let track = (rows > 0 && items.len() > usize::from(rows) && body.width > width)
        .then(|| Rect::new(body.right() - width, body.y + header, width, rows * row));
    Some(ListLayout {
        content: Rect {
            width: body.width - track.map_or(0, |r| r.width),
            ..body
        },
        track,
        rows: usize::from(rows),
    })
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub viewport: Viewport,
    pub revision: u64,
    pub focus: Option<Id>,
    pub placements: Vec<Placement>,
    pub fits: bool,
}
impl Frame {
    pub fn get(&self, id: &Id) -> Option<&Placement> {
        self.placements.iter().find(|p| &p.id == id)
    }
    pub fn hit(&self, x: u16, y: u16) -> Option<&Placement> {
        let modal = self.placements.iter().any(|p| p.modal);
        self.placements.iter().rev().find(|p| {
            p.enabled && p.focusable && (!modal || p.modal) && p.area.contains((x, y).into())
        })
    }
    pub fn focus_order(&self) -> Vec<Id> {
        let modal = self.placements.iter().any(|p| p.modal);
        self.placements
            .iter()
            .filter(|p| p.focusable && p.enabled && (!modal || p.modal))
            .map(|p| p.id.clone())
            .collect()
    }
    pub fn ancestry(&self, id: &Id) -> Vec<Id> {
        let mut result = vec![];
        let mut at = self.get(id);
        while let Some(p) = at {
            result.push(p.id.clone());
            at = p.parent.as_ref().and_then(|id| self.get(id));
        }
        result
    }
}

pub(super) fn validate(view: &View) -> anyhow::Result<()> {
    fn visit(
        node: &Node,
        depth: usize,
        ids: &mut std::collections::HashSet<Id>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(depth <= 32 && ids.len() < 2048, "UI exceeds tree limits");
        anyhow::ensure!(
            !node.id.0.is_empty() && node.id.0.len() <= 120 && ids.insert(node.id.clone()),
            "Empty or duplicate UI identity {}",
            node.id.0
        );
        match node.widget {
            Widget::Row | Widget::Column => {}
            Widget::Pane(_) => {
                anyhow::ensure!(node.children.len() == 1, "Pane needs one content root")
            }
            _ => anyhow::ensure!(node.children.is_empty(), "Controls cannot contain children"),
        }
        match node.widget {
            Widget::Slider { value, max, step } => {
                anyhow::ensure!(max > 0 && step > 0 && value <= max, "Invalid slider range")
            }
            Widget::Meter { value, max } => {
                anyhow::ensure!(max > 0 && value <= max, "Invalid meter range")
            }
            _ => {}
        }
        if let Widget::List { columns, .. } = &node.widget {
            anyhow::ensure!(columns.len() <= 64, "Table exceeds 64 columns");
            anyhow::ensure!(
                node.table_columns.is_empty()
                    || (node.table_columns.len() == columns.len()
                        && node
                            .table_columns
                            .iter()
                            .zip(columns)
                            .all(|(spec, heading)| &spec.heading == heading
                                && spec.width.min <= spec.width.preferred
                                && spec.width.preferred <= spec.width.max)),
                "Table column specifications must match headings and have valid widths"
            );
        } else {
            anyhow::ensure!(
                node.table_columns.is_empty(),
                "Column specifications require a table"
            );
        }
        for child in &node.children {
            visit(child, depth + 1, ids)?;
        }
        Ok(())
    }
    let mut ids = std::collections::HashSet::new();
    visit(&view.root, 0, &mut ids)?;
    if let Some(modal) = &view.modal {
        visit(modal, 0, &mut ids)?;
    }
    Ok(())
}

fn size(node: &Node, horizontal: bool, viewport: Viewport) -> panes::Size {
    let default = if !horizontal
        && !matches!(
            node.widget,
            Widget::Row
                | Widget::Column
                | Widget::Pane(_)
                | Widget::List { .. }
                | Widget::Custom { .. }
        ) {
        panes::Size::fixed(1)
    } else {
        panes::Size::fill(1)
    };
    let default = if !horizontal
        && matches!(
            node.widget,
            Widget::Field {
                multiline: true,
                ..
            }
        ) {
        panes::Size::fixed(3)
    } else {
        default
    };
    let value = if horizontal { node.width } else { node.height }.unwrap_or(default);
    let scale = viewport.scale(horizontal);
    panes::Size {
        min: value.min.saturating_mul(scale),
        preferred: value.preferred.saturating_mul(scale),
        max: if value.max == u16::MAX {
            u16::MAX
        } else {
            value.max.saturating_mul(scale)
        },
        grow: value.grow,
    }
}
// Shared physical chrome spacing. Drawing and placement use these together;
// terminal cells keep their compact single-border treatment.
pub(super) const PADDING: u16 = 8;
pub(super) const TEXT_PADDING: u16 = 4;
pub(super) const TITLE_HEIGHT: u16 = 24;
pub(super) const BODY_TOP: u16 = TITLE_HEIGHT + PADDING + 4;
pub(super) fn pane_header(area: Rect) -> Rect {
    let mut header = inset(area, TEXT_PADDING, TEXT_PADDING, 0);
    header.height = header.height.min(TITLE_HEIGHT);
    header
}
pub(super) fn text_area(area: Rect) -> Rect {
    inset(area, TEXT_PADDING, 0, 0)
}
fn inset(area: Rect, x: u16, top: u16, bottom: u16) -> Rect {
    Rect::new(
        area.x.saturating_add(x).min(area.right()),
        area.y.saturating_add(top).min(area.bottom()),
        area.width.saturating_sub(x.saturating_mul(2)),
        area.height.saturating_sub(top.saturating_add(bottom)),
    )
}
fn place(
    node: &Node,
    area: Rect,
    parent: Option<Id>,
    enabled: bool,
    modal: bool,
    frame: &mut Frame,
) -> anyhow::Result<()> {
    if area.is_empty() || node.visibility == panes::Visibility::Hidden {
        return Ok(());
    }
    let body = if matches!(node.widget, Widget::Pane(_)) {
        if frame.viewport.presentation == Presentation::Cells {
            inset(area, 1, 1, 1)
        } else {
            inset(area, PADDING, BODY_TOP, PADDING)
        }
    } else {
        area
    };
    let enabled = enabled && node.enabled;
    frame.placements.push(Placement {
        id: node.id.clone(),
        parent,
        area,
        body,
        widget: node.widget.clone(),
        enabled,
        focusable: node.focusable(),
        modal,
        columns: table_columns(node, body, frame.viewport)?,
    });
    if matches!(node.widget, Widget::Pane(_)) {
        if node.visibility != panes::Visibility::Collapsed {
            place(
                &node.children[0],
                body,
                Some(node.id.clone()),
                enabled,
                modal,
                frame,
            )?;
        }
    } else if !node.children.is_empty() {
        let children = node
            .children
            .iter()
            .enumerate()
            .map(|(index, child)| {
                panes::Node::pane(index)
                    .width(size(child, true, frame.viewport))
                    .height(size(child, false, frame.viewport))
                    .visibility(child.visibility)
                    .overflow(child.overflow)
                    .priority(child.priority)
            })
            .collect();
        let group = if matches!(node.widget, Widget::Row) {
            panes::Node::row(children)
        } else {
            panes::Node::column(children)
        };
        let layout = group
            .gap(frame.viewport.gap())
            .wrap(node.wrap)
            .layout(body)?;
        frame.fits &= layout.fits;
        for p in layout.panes {
            if let Some(area) = p.area {
                place(
                    &node.children[p.id],
                    area,
                    Some(node.id.clone()),
                    enabled,
                    modal,
                    frame,
                )?;
            }
        }
    }
    Ok(())
}

fn table_columns(
    node: &Node,
    body: Rect,
    viewport: Viewport,
) -> anyhow::Result<Vec<ColumnPlacement>> {
    let Widget::List { columns, .. } = &node.widget else {
        return Ok(vec![]);
    };
    if columns.is_empty() {
        return Ok(vec![]);
    }
    let body = list_layout(&node.widget, body, viewport)
        .expect("list geometry")
        .content;
    let area = if viewport.presentation == Presentation::Graphics {
        text_area(body)
    } else {
        body
    };
    let scale = viewport.scale(true);
    let children = (0..columns.len())
        .map(|index| {
            let width = node
                .table_columns
                .get(index)
                .map_or(panes::Size::fill(1), |c| c.width);
            panes::Node::pane(index).width(panes::Size {
                min: width.min.saturating_mul(scale),
                preferred: width.preferred.saturating_mul(scale),
                max: if width.max == u16::MAX {
                    u16::MAX
                } else {
                    width.max.saturating_mul(scale)
                },
                grow: width.grow,
            })
        })
        .collect();
    let layout = panes::Node::row(children)
        .gap(viewport.gap())
        .layout(area)?;
    Ok((0..columns.len())
        .map(|index| ColumnPlacement {
            area: layout
                .panes
                .iter()
                .find(|p| p.id == index)
                .and_then(|p| p.area)
                .unwrap_or(Rect::new(area.right(), area.y, 0, area.height)),
            align: node
                .table_columns
                .get(index)
                .map_or(Align::Left, |c| c.align),
        })
        .collect())
}
pub(super) fn solve(
    view: &View,
    viewport: Viewport,
    revision: u64,
    focus: Option<Id>,
) -> anyhow::Result<Frame> {
    anyhow::ensure!(
        viewport.width <= 8192 && viewport.height <= 8192,
        "UI viewport exceeds bounds"
    );
    let pixels = u32::from(viewport.width) * u32::from(viewport.height);
    anyhow::ensure!(
        pixels
            <= if viewport.presentation == Presentation::Cells {
                120_000
            } else {
                32_000_000
            },
        "UI viewport area exceeds bounds"
    );
    let mut frame = Frame {
        viewport,
        revision,
        focus,
        placements: vec![],
        fits: true,
    };
    let bounds = Rect::new(0, 0, viewport.width, viewport.height);
    let bounds = if viewport.presentation == Presentation::Graphics {
        inset(bounds, PADDING, PADDING, PADDING)
    } else {
        bounds
    };
    place(&view.root, bounds, None, true, false, &mut frame)?;
    if let Some(modal) = &view.modal {
        let extent = |horizontal, available| {
            let value = size(modal, horizontal, viewport);
            let preferred = if value.grow > 0 {
                value.preferred.max(available / 2)
            } else {
                value.preferred
            };
            preferred.max(value.min).min(value.max).min(available)
        };
        let width = extent(true, bounds.width);
        let height = extent(false, bounds.height);
        place(
            modal,
            Rect::new(
                (viewport.width - width) / 2,
                (viewport.height - height) / 2,
                width,
                height,
            ),
            None,
            true,
            true,
            &mut frame,
        )?;
    }
    Ok(frame)
}

/// Value, selection and focus changes retain solved geometry.
pub(super) fn same_geometry(a: &View, b: &View) -> bool {
    fn same(a: &Node, b: &Node) -> bool {
        a.id == b.id
            && std::mem::discriminant(&a.widget) == std::mem::discriminant(&b.widget)
            && a.width == b.width
            && a.height == b.height
            && a.visibility == b.visibility
            && a.overflow == b.overflow
            && a.priority == b.priority
            && a.wrap == b.wrap
            && a.table_columns
                .iter()
                .map(|c| c.width)
                .eq(b.table_columns.iter().map(|c| c.width))
            && match (&a.widget, &b.widget) {
                (Widget::Field { multiline: a, .. }, Widget::Field { multiline: b, .. }) => a == b,
                (Widget::List { columns: a, .. }, Widget::List { columns: b, .. }) => {
                    a.len() == b.len()
                }
                _ => true,
            }
            && a.children.len() == b.children.len()
            && a.children.iter().zip(&b.children).all(|(a, b)| same(a, b))
    }
    same(&a.root, &b.root)
        && match (&a.modal, &b.modal) {
            (None, None) => true,
            (Some(a), Some(b)) => same(a, b),
            _ => false,
        }
}
pub(super) fn refresh(view: &View, frame: &mut Frame) -> anyhow::Result<()> {
    fn visit<'a>(
        node: &'a Node,
        enabled: bool,
        nodes: &mut std::collections::BTreeMap<&'a Id, (&'a Node, bool)>,
    ) {
        let enabled = enabled && node.enabled;
        nodes.insert(&node.id, (node, enabled));
        for child in &node.children {
            visit(child, enabled, nodes);
        }
    }
    let mut nodes = std::collections::BTreeMap::new();
    visit(&view.root, true, &mut nodes);
    if let Some(modal) = &view.modal {
        visit(modal, true, &mut nodes);
    }
    for p in &mut frame.placements {
        if let Some((node, enabled)) = nodes.get(&p.id) {
            let previous = p.list_layout(frame.viewport);
            p.widget = node.widget.clone();
            p.enabled = *enabled;
            p.focusable = node.focusable();
            if previous != p.list_layout(frame.viewport) {
                p.columns = table_columns(node, p.body, frame.viewport)?;
            }
            for (index, column) in p.columns.iter_mut().enumerate() {
                column.align = node
                    .table_columns
                    .get(index)
                    .map_or(Align::Left, |c| c.align);
            }
        }
    }
    Ok(())
}

pub(super) fn same_targets(a: &View, b: &View) -> bool {
    fn same(a: &Node, b: &Node) -> bool {
        if a.id != b.id || a.enabled != b.enabled || a.children.len() != b.children.len() {
            return false;
        }
        let same_widget = match (&a.widget, &b.widget) {
            (Widget::Button { command: a }, Widget::Button { command: b })
            | (Widget::Checkbox { command: a }, Widget::Checkbox { command: b }) => a == b,
            (
                Widget::List {
                    items: a,
                    columns: ac,
                    ..
                },
                Widget::List {
                    items: b,
                    columns: bc,
                    ..
                },
            ) => {
                ac.is_empty() == bc.is_empty()
                    && a.len() == b.len()
                    && a.iter()
                        .zip(b.iter())
                        .all(|(a, b)| a.id == b.id && a.enabled == b.enabled)
            }
            (
                Widget::Slider {
                    max: a, step: as_, ..
                },
                Widget::Slider {
                    max: b, step: bs, ..
                },
            ) => a == b && as_ == bs,
            _ => std::mem::discriminant(&a.widget) == std::mem::discriminant(&b.widget),
        };
        same_widget && a.children.iter().zip(&b.children).all(|(a, b)| same(a, b))
    }
    same(&a.root, &b.root)
        && match (&a.modal, &b.modal) {
            (None, None) => true,
            (Some(a), Some(b)) => same(a, b),
            _ => false,
        }
}
